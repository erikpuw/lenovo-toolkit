//! Số liệu hệ thống cho thẻ cảm biến: CPU %, GPU % & nhiệt độ (NVAPI), RAM, pin
//! (GetSystemTimes, GlobalMemoryStatusEx, GetSystemPowerStatus, Registry).

use std::ffi::c_void;
use std::io;
use std::sync::OnceLock;

use windows::Win32::Foundation::FILETIME;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::core::{s, w};
use winreg::RegKey;
use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

#[derive(serde::Deserialize)]
#[allow(non_snake_case)]
struct NameRow {
    Name: Option<String>,
}

fn names(wql: &str) -> Vec<String> {
    crate::wmi_util::Wmi::open_cimv2()
        .and_then(|w| w.query::<NameRow>(wql))
        .map(|rows| rows.into_iter().filter_map(|r| r.Name).map(|n| n.trim().to_string()).collect())
        .unwrap_or_default()
}

/// "13th Gen Intel(R) Core(TM) i7-13650HX" -> "Intel Core i7-13650HX" (như tiêu đề thẻ CPU bản gốc).
pub fn short_cpu_name(name: &str) -> String {
    let mut s = name.replace("(R)", "").replace("(TM)", "");
    if let Some(i) = s.find(" Gen ") {
        let prefix = &s[..i];
        if prefix.len() <= 5 && prefix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            s = s[i + 5..].to_string();
        }
    }
    for cut in [" CPU", " @", " with "] {
        if let Some(i) = s.find(cut) {
            s.truncate(i);
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// "NVIDIA GeForce RTX 4060 Laptop GPU" -> "GeForce RTX 4060".
pub fn short_gpu_name(name: &str) -> String {
    let s = name.trim().strip_prefix("NVIDIA ").unwrap_or(name.trim());
    let s = s.strip_suffix(" Laptop GPU").or_else(|| s.strip_suffix(" Laptop")).unwrap_or(s);
    s.trim().to_string()
}

pub fn cpu_name() -> Option<String> {
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", KEY_READ)
        .and_then(|k| k.get_value::<String, _>("ProcessorNameString"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| names("SELECT Name FROM Win32_Processor").into_iter().find(|n| !n.is_empty()))
}

/// Chọn GPU rời nếu có (NVIDIA/GeForce/Radeon RX/Pro/Arc), nếu không thì GPU đầu tiên không phải Basic.
pub fn pick_gpu(names: &[String]) -> Option<String> {
    let lower = |s: &str| s.to_lowercase();
    let discrete = ["nvidia", "geforce", "radeon rx", "radeon pro", "arc a"];
    names
        .iter()
        .find(|n| discrete.iter().any(|d| lower(n).contains(d)))
        .or_else(|| names.iter().find(|n| !lower(n).contains("basic")))
        .cloned()
}

pub fn gpu_name() -> Option<String> {
    let mut reg_names = Vec::new();
    if let Ok(class) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}",
        KEY_READ,
    ) {
        for i in 0..8 {
            if let Ok(sub) = class.open_subkey_with_flags(format!("{i:04}"), KEY_READ)
                && let Ok(desc) = sub.get_value::<String, _>("DriverDesc")
            {
                let trimmed = desc.trim();
                if !trimmed.is_empty() {
                    reg_names.push(trimmed.to_string());
                }
            }
        }
    }
    pick_gpu(&reg_names).or_else(|| pick_gpu(&names("SELECT Name FROM Win32_VideoController")))
}

// ---- NVAPI (nvapi64.dll) nhẹ cho % tải GPU và nhiệt độ GPU rời ----

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuStats {
    pub utilization: Option<u32>,
    pub temp_c: Option<i32>,
}

type QueryInterfaceFn = unsafe extern "C" fn(u32) -> *const c_void;
type InitFn = unsafe extern "C" fn() -> i32;
type EnumGpusFn = unsafe extern "C" fn(*mut [*mut c_void; 64], *mut u32) -> i32;
type GetPstatesFn = unsafe extern "C" fn(*mut c_void, *mut NvDynamicPstatesInfoEx) -> i32;
type GetThermalFn = unsafe extern "C" fn(*mut c_void, u32, *mut NvGpuThermalSettingsV2) -> i32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NvUtilizationDomain {
    is_present: u32,
    percentage: u32,
}

#[repr(C)]
struct NvDynamicPstatesInfoEx {
    version: u32,
    flags: u32,
    utilization: [NvUtilizationDomain; 8],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NvThermalSensor {
    controller: i32,
    default_min_temp: i32,
    default_max_temp: i32,
    current_temp: i32,
    target: i32,
}

#[repr(C)]
struct NvGpuThermalSettingsV2 {
    version: u32,
    count: u32,
    sensor: [NvThermalSensor; 3],
}

struct NvApi {
    enum_gpus: EnumGpusFn,
    get_pstates: GetPstatesFn,
    get_thermal: GetThermalFn,
}

unsafe impl Send for NvApi {}
unsafe impl Sync for NvApi {}

static NVAPI: OnceLock<Option<NvApi>> = OnceLock::new();

fn nvapi() -> Option<&'static NvApi> {
    NVAPI
        .get_or_init(|| unsafe {
            let dll = LoadLibraryW(w!("nvapi64.dll")).ok()?;
            let qi_addr = GetProcAddress(dll, s!("nvapi_QueryInterface"))?;
            let qi: QueryInterfaceFn = std::mem::transmute(qi_addr);
            let init_ptr = qi(0x0150_E828);
            if init_ptr.is_null() {
                return None;
            }
            let init: InitFn = std::mem::transmute(init_ptr);
            if init() != 0 {
                return None;
            }
            let enum_ptr = qi(0xE5AC_921F);
            let pstates_ptr = qi(0x60DE_D2ED);
            let thermal_ptr = qi(0xE364_0A56);
            if enum_ptr.is_null() || pstates_ptr.is_null() || thermal_ptr.is_null() {
                return None;
            }
            Some(NvApi {
                enum_gpus: std::mem::transmute::<*const c_void, EnumGpusFn>(enum_ptr),
                get_pstates: std::mem::transmute::<*const c_void, GetPstatesFn>(pstates_ptr),
                get_thermal: std::mem::transmute::<*const c_void, GetThermalFn>(thermal_ptr),
            })
        })
        .as_ref()
}

/// Mã hãng NVIDIA trong Hardware ID của thiết bị PCI.
fn is_nvidia(hardware_id: &str) -> bool {
    hardware_id.to_ascii_uppercase().contains("VEN_10DE")
}

/// Card NVIDIA đang chạy (D0) hay đang ngủ (D3), theo trạng thái nguồn Windows ghi lại cho thiết bị.
/// Hỏi Windows chứ không hỏi card, nên không đánh thức card đang ngủ. `None` nếu không có card NVIDIA.
///
/// Bản C# kiểm tra `GetCoprocInfo` (GC6/GcOff) trước khi gọi NVAPI vì gọi NVAPI lúc card ngủ sẽ đánh thức nó,
/// giữ card rời chạy, nóng máy và tốn pin. Ở đây dùng trạng thái nguồn của Windows cho cùng mục đích.
pub fn nvidia_gpu_awake() -> Option<bool> {
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        DIGCF_PRESENT, GUID_DEVCLASS_DISPLAY, SP_DEVINFO_DATA, SPDRP_DEVICE_POWER_DATA, SPDRP_HARDWAREID,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceRegistryPropertyW,
    };
    use windows::Win32::System::Power::{CM_POWER_DATA, PowerDeviceD0};
    use windows::core::PCWSTR;

    unsafe {
        let set = SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_DISPLAY), PCWSTR::null(), None, DIGCF_PRESENT).ok()?;
        let mut found = None;
        for index in 0.. {
            let mut dev = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set, index, &mut dev).is_err() {
                break;
            }
            let mut buf = [0u8; 1024];
            if SetupDiGetDeviceRegistryPropertyW(set, &dev, SPDRP_HARDWAREID, None, Some(&mut buf), None).is_err() {
                continue;
            }
            let wide: Vec<u16> = buf.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            if !is_nvidia(&String::from_utf16_lossy(&wide)) {
                continue;
            }
            let mut power = CM_POWER_DATA { PD_Size: size_of::<CM_POWER_DATA>() as u32, ..Default::default() };
            let bytes = std::slice::from_raw_parts_mut((&raw mut power).cast::<u8>(), size_of::<CM_POWER_DATA>());
            if SetupDiGetDeviceRegistryPropertyW(set, &dev, SPDRP_DEVICE_POWER_DATA, None, Some(bytes), None).is_ok() {
                found = Some(power.PD_MostRecentPowerState == PowerDeviceD0);
                break;
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        found
    }
}

/// Đọc % tải và nhiệt độ GPU NVIDIA qua `nvapi64.dll`. Chỉ gọi khi `nvidia_gpu_awake()` là `Some(true)`,
/// nếu không sẽ đánh thức card rời đang ngủ.
pub fn gpu_stats() -> GpuStats {
    let Some(api) = nvapi() else {
        return GpuStats::default();
    };
    unsafe {
        let mut handles = [std::ptr::null_mut(); 64];
        let mut count = 0u32;
        if (api.enum_gpus)(&mut handles, &mut count) != 0 || count == 0 || handles[0].is_null() {
            return GpuStats::default();
        }
        let gpu = handles[0];

        let mut pstates = NvDynamicPstatesInfoEx {
            version: (size_of::<NvDynamicPstatesInfoEx>() as u32) | (1 << 16),
            flags: 0,
            utilization: [NvUtilizationDomain::default(); 8],
        };
        let utilization = if (api.get_pstates)(gpu, &mut pstates) == 0 && pstates.utilization[0].is_present != 0 {
            let gpu_pct = pstates.utilization[0].percentage;
            let vid_pct = if pstates.utilization[2].is_present != 0 { pstates.utilization[2].percentage } else { 0 };
            Some(gpu_pct.max(vid_pct).min(100))
        } else {
            None
        };

        let mut thermal = NvGpuThermalSettingsV2 {
            version: (size_of::<NvGpuThermalSettingsV2>() as u32) | (2 << 16),
            count: 0,
            sensor: [NvThermalSensor::default(); 3],
        };
        let temp_c = if (api.get_thermal)(gpu, 0x7FFF_FFFF, &mut thermal) == 0 && thermal.count > 0 {
            let t = thermal.sensor[0].current_temp;
            (1..150).contains(&t).then_some(t)
        } else {
            None
        };

        GpuStats { utilization, temp_c }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Memory {
    pub used_gb: f64,
    pub total_gb: f64,
    pub load_percent: u32,
}

pub fn memory() -> io::Result<Memory> {
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m) }.map_err(|e| io::Error::other(format!("GlobalMemoryStatusEx lỗi: {e}")))?;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    Ok(Memory {
        used_gb: (m.ullTotalPhys - m.ullAvailPhys) as f64 / GB,
        total_gb: m.ullTotalPhys as f64 / GB,
        load_percent: m.dwMemoryLoad,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    /// `None` nếu Windows không biết (255).
    pub percent: Option<u8>,
    pub ac_online: bool,
    pub charging: bool,
}

pub fn battery() -> io::Result<Battery> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s) }.map_err(|e| io::Error::other(format!("GetSystemPowerStatus lỗi: {e}")))?;
    Ok(battery_from(s.ACLineStatus, s.BatteryLifePercent, s.BatteryFlag))
}

fn battery_from(ac_line: u8, percent: u8, flag: u8) -> Battery {
    Battery {
        percent: (percent <= 100).then_some(percent),
        ac_online: ac_line == 1,
        charging: flag & 8 != 0,
    }
}

fn filetime(ft: FILETIME) -> u64 {
    (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)
}

/// Tải CPU tính từ hai lần lấy mẫu `GetSystemTimes` liên tiếp (lần đầu chưa có giá trị).
#[derive(Debug, Default)]
pub struct CpuMeter {
    prev: Option<(u64, u64, u64)>,
}

/// `kernel` đã bao gồm `idle`; trả về 0..=100.
pub fn cpu_percent(prev: (u64, u64, u64), cur: (u64, u64, u64)) -> Option<u32> {
    let idle = cur.0.checked_sub(prev.0)?;
    let kernel = cur.1.checked_sub(prev.1)?;
    let user = cur.2.checked_sub(prev.2)?;
    let total = kernel + user;
    if total == 0 {
        return None;
    }
    Some((100 - (idle * 100 / total).min(100)) as u32)
}

impl CpuMeter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sample(&mut self) -> Option<u32> {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        let cur = (filetime(idle), filetime(kernel), filetime(user));
        let result = self.prev.and_then(|p| cpu_percent(p, cur));
        self.prev = Some(cur);
        result
    }
}

// ---- Thông tin phần cứng chi tiết cho cửa sổ "Về thiết bị của bạn" (chỉ nạp khi bấm nút Model) ----

#[derive(Debug, Clone, Default)]
pub struct GpuHwInfo {
    pub name: String,
    pub vram_gb: Option<f64>,
    pub is_shared: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RamHwInfo {
    pub slot: usize,
    pub generation: String,
    pub capacity_gb: f64,
    pub speed_mhz: u32,
    pub manufacturer: String,
}

#[derive(Debug, Clone, Default)]
pub struct DriveHwInfo {
    pub name: String,
    pub label: String,
    pub used_gb: f64,
    pub total_gb: f64,
    pub model: String,
}

#[derive(Debug, Clone, Default)]
pub struct DisplayHwInfo {
    pub index: usize,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

#[derive(Debug, Clone, Default)]
pub struct NetHwInfo {
    pub name: String,
    pub mac: String,
}

#[derive(Debug, Clone, Default)]
pub struct HardwareOverview {
    pub os_name: String,
    pub serial_number: String,
    pub motherboard_product: String,
    pub motherboard_manufacturer: String,
    pub cpu_full_name: String,
    pub cpu_cores: u32,
    pub cpu_threads: u32,
    pub gpus: Vec<GpuHwInfo>,
    pub rams: Vec<RamHwInfo>,
    pub drives: Vec<DriveHwInfo>,
    pub displays: Vec<DisplayHwInfo>,
    pub networks: Vec<NetHwInfo>,
}

fn os_friendly_name() -> String {
    let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", KEY_READ)
    else {
        return "Windows".to_string();
    };
    let mut prod = k.get_value::<String, _>("ProductName").unwrap_or_else(|_| "Windows".to_string());
    let disp = k
        .get_value::<String, _>("DisplayVersion")
        .or_else(|_| k.get_value::<String, _>("ReleaseId"))
        .unwrap_or_default();
    let build = k
        .get_value::<String, _>("CurrentBuildNumber")
        .or_else(|_| k.get_value::<String, _>("CurrentBuild"))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);
    if build >= 22000 {
        prod = prod.replace("Windows 10", "Windows 11");
    }
    if disp.is_empty() { prod } else { format!("{prod} {disp}") }
}

fn is_integrated_gpu_name(name: &str) -> bool {
    let l = name.to_lowercase();
    (l.contains("intel") && !l.contains("arc"))
        || l.contains("iris")
        || l.contains("uhd")
        || l.starts_with("amd radeon graphics")
        || l.starts_with("amd radeon(tm) graphics")
        || l.contains("vega")
        || (l.contains("amd radeon") && !l.contains("rx") && (l.ends_with('m') || l.ends_with('s')))
}

fn read_gpus_from_reg() -> Vec<GpuHwInfo> {
    let mut out = Vec::new();
    let Ok(class) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}",
        KEY_READ,
    ) else {
        return out;
    };
    for i in 0..8 {
        let Ok(sub) = class.open_subkey_with_flags(format!("{i:04}"), KEY_READ) else { continue };
        let Ok(desc) = sub.get_value::<String, _>("DriverDesc") else { continue };
        let name = desc.trim().to_string();
        if name.is_empty() || name.to_lowercase().contains("basic") {
            continue;
        }
        let is_shared = is_integrated_gpu_name(&name);
        let vram_bytes = sub
            .get_raw_value("HardwareInformation.qwMemorySize")
            .ok()
            .and_then(|raw| raw.bytes.get(..8).and_then(|b| b.try_into().ok()).map(u64::from_le_bytes))
            .unwrap_or(0);
        let vram_gb = (!is_shared && vram_bytes > 0).then_some(vram_bytes as f64 / 1_073_741_824.0);
        if !out.iter().any(|g: &GpuHwInfo| g.name.eq_ignore_ascii_case(&name)) {
            out.push(GpuHwInfo { name, vram_gb, is_shared });
        }
    }
    // Ưu tiên card rời đứng trước giống bản C#
    out.sort_by_key(|g| g.is_shared);
    out
}

/// Nạp toàn bộ thông tin phần cứng cho cửa sổ "Về thiết bị của bạn" (chỉ gọi khi người dùng mở).
pub fn hardware_overview() -> HardwareOverview {
    let mut hw = HardwareOverview {
        os_name: os_friendly_name(),
        cpu_full_name: cpu_name().unwrap_or_default(),
        gpus: read_gpus_from_reg(),
        ..Default::default()
    };

    if let Ok(k) = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"HARDWARE\DESCRIPTION\System\BIOS", KEY_READ)
    {
        hw.motherboard_product = k.get_value::<String, _>("BaseBoardProduct").unwrap_or_default().trim().to_string();
        hw.motherboard_manufacturer = k
            .get_value::<String, _>("BaseBoardManufacturer")
            .unwrap_or_default()
            .trim()
            .to_string();
    }

    let Ok(cim) = crate::wmi_util::Wmi::open_cimv2() else {
        return hw;
    };

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct BiosRow {
        SerialNumber: Option<String>,
    }
    if let Ok(rows) = cim.query::<BiosRow>("SELECT SerialNumber FROM Win32_BIOS")
        && let Some(sn) = rows.into_iter().find_map(|r| r.SerialNumber)
    {
        hw.serial_number = sn.trim().to_string();
    }

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct CpuRow {
        Name: Option<String>,
        NumberOfCores: Option<u32>,
        NumberOfLogicalProcessors: Option<u32>,
    }
    if let Ok(rows) = cim.query::<CpuRow>("SELECT Name, NumberOfCores, NumberOfLogicalProcessors FROM Win32_Processor")
        && let Some(r) = rows.into_iter().next()
    {
        if let Some(n) = r.Name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            hw.cpu_full_name = n;
        }
        hw.cpu_cores = r.NumberOfCores.unwrap_or(0);
        hw.cpu_threads = r.NumberOfLogicalProcessors.unwrap_or(0);
    }

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct MemRow {
        Capacity: Option<u64>,
        Speed: Option<u32>,
        SMBIOSMemoryType: Option<u32>,
        MemoryType: Option<u32>,
        Manufacturer: Option<String>,
    }
    if let Ok(rows) = cim.query::<MemRow>(
        "SELECT Capacity, Speed, SMBIOSMemoryType, MemoryType, Manufacturer FROM Win32_PhysicalMemory",
    ) {
        for (idx, r) in rows.into_iter().enumerate() {
            let cap = r.Capacity.unwrap_or(0) as f64 / 1_073_741_824.0;
            let code = r.SMBIOSMemoryType.or(r.MemoryType).unwrap_or(0);
            let generation = match code {
                26 => "DDR4",
                34 | 35 => "DDR5",
                _ => "RAM",
            }
            .to_string();
            let mfg = r
                .Manufacturer
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && !s.starts_with("00") && !s.eq_ignore_ascii_case("unknown"))
                .unwrap_or_default();
            hw.rams.push(RamHwInfo {
                slot: idx + 1,
                generation,
                capacity_gb: cap,
                speed_mhz: r.Speed.unwrap_or(0),
                manufacturer: mfg,
            });
        }
    }

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct DiskDriveRow {
        Index: Option<u32>,
        Model: Option<String>,
    }
    let disk_by_index: std::collections::HashMap<u32, String> = cim
        .query::<DiskDriveRow>("SELECT Index, Model FROM Win32_DiskDrive")
        .unwrap_or_default()
        .into_iter()
        .filter_map(|d| Some((d.Index?, d.Model?.trim().to_string())))
        .filter(|(_, m)| !m.is_empty())
        .collect();
    let default_model = disk_by_index.get(&0).cloned().unwrap_or_default();

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct DiskToPartRow {
        Antecedent: Option<String>,
        Dependent: Option<String>,
    }
    let mut drive_to_model = std::collections::HashMap::new();
    if let Ok(links) = cim.query::<DiskToPartRow>("SELECT Antecedent, Dependent FROM Win32_LogicalDiskToPartition") {
        for link in links {
            let (Some(ant), Some(dep)) = (link.Antecedent, link.Dependent) else { continue };
            let disk_idx = ant
                .find("Disk #")
                .and_then(|i| ant[i + 6..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u32>().ok());
            let letter = dep.rfind("DeviceID=\"").and_then(|i| dep[i + 10..].split('"').next());
            if let (Some(idx), Some(dev_id)) = (disk_idx, letter)
                && let Some(m) = disk_by_index.get(&idx)
            {
                drive_to_model.insert(dev_id.to_uppercase(), m.clone());
            }
        }
    }

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct LogicalDiskRow {
        DeviceID: Option<String>,
        VolumeName: Option<String>,
        Size: Option<u64>,
        FreeSpace: Option<u64>,
    }
    if let Ok(rows) = cim.query::<LogicalDiskRow>(
        "SELECT DeviceID, VolumeName, Size, FreeSpace FROM Win32_LogicalDisk WHERE DriveType = 3",
    ) {
        for r in rows {
            let total = r.Size.unwrap_or(0) as f64 / 1_073_741_824.0;
            if total <= 0.0 {
                continue;
            }
            let free = r.FreeSpace.unwrap_or(0) as f64 / 1_073_741_824.0;
            let used = (total - free).max(0.0);
            let dev_id = r.DeviceID.unwrap_or_default().trim_end_matches('\\').to_uppercase();
            let model = drive_to_model.get(&dev_id).cloned().unwrap_or_else(|| default_model.clone());
            let name = format!("{dev_id}\\");
            let label = r.VolumeName.unwrap_or_default().trim().to_string();
            hw.drives.push(DriveHwInfo { name, label, used_gb: used, total_gb: total, model });
        }
    }

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct VideoRow {
        CurrentHorizontalResolution: Option<u32>,
        CurrentVerticalResolution: Option<u32>,
        CurrentRefreshRate: Option<u32>,
    }
    if let Ok(rows) = cim.query::<VideoRow>(
        "SELECT CurrentHorizontalResolution, CurrentVerticalResolution, CurrentRefreshRate FROM Win32_VideoController",
    ) {
        let mut idx = 1usize;
        for r in rows {
            if let (Some(w), Some(h)) = (r.CurrentHorizontalResolution, r.CurrentVerticalResolution)
                && w > 0
                && h > 0
            {
                let hz = r.CurrentRefreshRate.filter(|&z| z > 1).unwrap_or(60);
                if !hw.displays.iter().any(|d| d.width == w && d.height == h && d.hz == hz) {
                    hw.displays.push(DisplayHwInfo {
                        index: idx,
                        name: "Generic Display".to_string(),
                        width: w,
                        height: h,
                        hz,
                    });
                    idx += 1;
                }
            }
        }
    }

    #[derive(serde::Deserialize)]
    #[allow(non_snake_case)]
    struct NetRow {
        Name: Option<String>,
        MACAddress: Option<String>,
    }
    const BLACKLIST: [&str; 10] = [
        "tap-windows", "qmtap", "virtual", "vmware", "virtualbox", "hyper-v", "vpn", "pseudo-interface", "loopback",
        "pnpgeneric",
    ];
    if let Ok(rows) = cim.query::<NetRow>(
        "SELECT Name, MACAddress FROM Win32_NetworkAdapter WHERE PhysicalAdapter = True AND MACAddress IS NOT NULL",
    ) {
        for r in rows {
            let (Some(name), Some(mac)) = (r.Name, r.MACAddress) else { continue };
            let name = name.trim().to_string();
            let mac = mac.trim().to_string();
            if name.is_empty() || mac.is_empty() {
                continue;
            }
            let lower = name.to_lowercase();
            if BLACKLIST.iter().any(|b| lower.contains(b)) {
                continue;
            }
            hw.networks.push(NetHwInfo { name, mac });
        }
    }

    hw
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_is_matched_by_vendor_id() {
        assert!(is_nvidia(r"PCI\VEN_10DE&DEV_28A0&SUBSYS_3D861414"));
        assert!(is_nvidia(r"pci\ven_10de&dev_28a0"));
        assert!(!is_nvidia(r"PCI\VEN_8086&DEV_A788")); // Intel iGPU
        assert!(!is_nvidia(r"PCI\VEN_1002&DEV_73DF")); // AMD
    }

    #[test]
    fn reading_dgpu_power_state_does_not_fail() {
        // Chỉ đọc trạng thái Windows ghi lại, chạy được không cần quyền admin; in ra để đối chiếu trên máy thật.
        println!("nvidia_gpu_awake = {:?}", nvidia_gpu_awake());
    }

    #[test]
    fn cpu_percent_from_deltas() {
        // idle tăng 75, kernel(gồm idle) tăng 80, user tăng 20 => tổng 100, idle 75% => tải 25%
        assert_eq!(cpu_percent((0, 0, 0), (75, 80, 20)), Some(25));
        assert_eq!(cpu_percent((0, 0, 0), (0, 50, 50)), Some(100));
        assert_eq!(cpu_percent((0, 0, 0), (0, 0, 0)), None);
        assert_eq!(cpu_percent((10, 10, 10), (5, 20, 20)), None); // bộ đếm lùi
    }

    #[test]
    fn names_are_shortened_for_card_titles() {
        assert_eq!(short_cpu_name("13th Gen Intel(R) Core(TM) i7-13650HX"), "Intel Core i7-13650HX");
        assert_eq!(short_cpu_name("Intel(R) Core(TM) i5-13450HX CPU @ 2.40GHz"), "Intel Core i5-13450HX");
        assert_eq!(short_cpu_name("AMD Ryzen 7 7840HS with Radeon 780M Graphics"), "AMD Ryzen 7 7840HS");
        assert_eq!(short_cpu_name(""), "");
        assert_eq!(short_gpu_name("NVIDIA GeForce RTX 4060 Laptop GPU"), "GeForce RTX 4060");
        assert_eq!(short_gpu_name("AMD Radeon RX 7600M XT"), "AMD Radeon RX 7600M XT");
    }

    #[test]
    fn gpu_prefers_discrete() {
        let n = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            pick_gpu(&n(&["Intel(R) UHD Graphics", "NVIDIA GeForce RTX 4060 Laptop GPU"])).as_deref(),
            Some("NVIDIA GeForce RTX 4060 Laptop GPU")
        );
        assert_eq!(pick_gpu(&n(&["Intel(R) Iris Xe"])).as_deref(), Some("Intel(R) Iris Xe"));
        assert_eq!(pick_gpu(&n(&["Microsoft Basic Display Adapter"])), None);
        assert_eq!(pick_gpu(&[]), None);
    }

    #[test]
    fn battery_fields() {
        let b = battery_from(1, 78, 8);
        assert_eq!(b, Battery { percent: Some(78), ac_online: true, charging: true });
        assert_eq!(battery_from(0, 255, 128).percent, None);
        assert!(!battery_from(0, 50, 0).ac_online);
    }

    #[test]
    fn reads_real_system_values() {
        let m = memory().unwrap();
        assert!(m.total_gb > 0.0 && m.used_gb <= m.total_gb);
        let mut cpu = CpuMeter::new();
        assert_eq!(cpu.sample(), None); // lần đầu chưa có độ lệch
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(cpu.sample().is_none_or(|p| p <= 100));
        assert!(battery().is_ok());
    }
}
