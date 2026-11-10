//! Chế độ sạc pin và thông tin chi tiết pin qua EnergyDrv + Windows Battery IOCTL
//! (port từ BatteryFeature.cs và System/Battery.cs).

use std::{io, ptr, thread, time::Duration};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, GUID_DEVCLASS_BATTERY, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows::Win32::System::Power::{
    BATTERY_INFORMATION, BATTERY_QUERY_INFORMATION, BATTERY_STATUS, BATTERY_WAIT_STATUS,
    BatteryInformation, GetSystemPowerStatus, IOCTL_BATTERY_QUERY_INFORMATION,
    IOCTL_BATTERY_QUERY_STATUS, IOCTL_BATTERY_QUERY_TAG, SYSTEM_POWER_STATUS,
};
use windows::core::PCWSTR;

use crate::driver::{ACCESS_READ_WRITE, Device, ENERGY_DRV};

const IOCTL_ENERGY_BATTERY_CHARGE_MODE: u32 = 0x8310_20F8;
const IOCTL_ENERGY_BATTERY_INFORMATION: u32 = 0x8310_2138;
const QUERY: u32 = 0xFF;
const VERIFY_RETRIES: u32 = 10;
const VERIFY_DELAY: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    Normal,
    RapidCharge,
    Conservation,
}

impl BatteryState {
    pub fn from_raw(raw: u32) -> Self {
        if raw & 0x20 != 0 {
            Self::Conservation
        } else if raw & 0x04 != 0 {
            Self::RapidCharge
        } else {
            Self::Normal
        }
    }

    fn codes(self) -> [u32; 2] {
        match self {
            Self::Conservation => [0x08, 0x03],
            Self::Normal => [0x05, 0x08],
            Self::RapidCharge => [0x05, 0x07],
        }
    }
}

pub fn open() -> io::Result<Device> {
    Device::open(ENERGY_DRV, ACCESS_READ_WRITE)
}

pub fn get(dev: &Device) -> io::Result<BatteryState> {
    dev.ioctl(IOCTL_ENERGY_BATTERY_CHARGE_MODE, QUERY)
        .map(BatteryState::from_raw)
}

/// Đặt chế độ rồi đọc lại; trả về trạng thái thực tế sau tối đa 10 lần thử.
pub fn set(dev: &Device, state: BatteryState) -> io::Result<BatteryState> {
    for code in state.codes() {
        dev.ioctl(IOCTL_ENERGY_BATTERY_CHARGE_MODE, code)?;
    }
    let mut actual = get(dev)?;
    for _ in 1..VERIFY_RETRIES {
        if actual == state {
            break;
        }
        thread::sleep(VERIFY_DELAY);
        actual = get(dev)?;
    }
    Ok(actual)
}

// ---- Chi tiết pin (trang Battery, port từ System/Battery.cs) ----

/// Theo dõi tốc độ sạc/xả nhỏ nhất và lớn nhất trong phiên (mW), giống `Battery.SetMinMaxDischargeRate`.
#[derive(Debug, Clone, Copy)]
pub struct RateTracker {
    pub min_mw: i32,
    pub max_mw: i32,
}

impl Default for RateTracker {
    fn default() -> Self {
        Self { min_mw: i32::MAX, max_mw: 0 }
    }
}

impl RateTracker {
    /// Cập nhật theo tốc độ hiện tại (mW) và trả về `(min_mw, max_mw)` để hiển thị.
    pub fn update(&mut self, rate_mw: i32) -> (Option<i32>, Option<i32>) {
        if rate_mw == i32::MIN {
            let min = (self.min_mw != i32::MAX).then_some(self.min_mw);
            return (min, Some(self.max_mw));
        }
        let sign_flipped = (rate_mw > 0 && (self.min_mw < 0 || self.max_mw < 0))
            || (rate_mw < 0 && (self.min_mw > 0 || self.max_mw > 0));
        if rate_mw == 0 || sign_flipped {
            self.min_mw = i32::MAX;
            self.max_mw = 0;
        }
        if rate_mw != 0 {
            let abs = rate_mw.unsigned_abs();
            if self.max_mw == 0 || abs < self.min_mw.unsigned_abs() {
                self.min_mw = rate_mw;
            }
            if abs > self.max_mw.unsigned_abs() {
                self.max_mw = rate_mw;
            }
        }
        let min_out = if rate_mw == 0 { 0 } else { self.min_mw };
        (Some(min_out), Some(self.max_mw))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatteryDetail {
    pub ac_online: bool,
    pub is_charging: bool,
    pub is_low_battery: bool,
    pub percent: u32,
    pub life_remaining_secs: Option<u32>,
    pub rate_mw: Option<i32>,
    pub min_rate_mw: Option<i32>,
    pub max_rate_mw: Option<i32>,
    pub current_capacity_mwh: u32,
    pub full_charge_capacity_mwh: u32,
    pub design_capacity_mwh: u32,
    pub cycle_count: u32,
    pub temperature_c: Option<f64>,
    pub manufacture_date: Option<String>,
    pub first_use_date: Option<String>,
}

impl BatteryDetail {
    pub fn health_percent(&self) -> Option<f64> {
        (self.design_capacity_mwh > 0)
            .then(|| (f64::from(self.full_charge_capacity_mwh) / f64::from(self.design_capacity_mwh)) * 100.0)
    }
}

pub fn decode_temp_c(raw: u16) -> Option<f64> {
    if raw == 0 || raw == u16::MAX {
        return None;
    }
    let c = (f64::from(raw) - 2731.6) / 10.0;
    (0.0..=120.0).contains(&c).then_some(c)
}

pub fn decode_date(raw: u16) -> Option<String> {
    if raw == 0 || raw == u16::MAX {
        return None;
    }
    let year = (raw >> 9) + 1980;
    let month = (raw >> 5) & 0x0F;
    let day = raw & 0x1F;
    if !(2018..=2030).contains(&year) || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LenovoBatteryInfo {
    bytes1: [u8; 13],
    // C# LayoutKind.Sequential căn chỉnh ushort ở offset 14 (1 byte padding sau bytes1[13]).
    temperature: u16,
    manufacture_date: u16,
    first_use_date: u16,
    bytes2: [u8; 64],
}

fn read_lenovo_battery(energy_dev: &Device) -> Option<(Option<f64>, Option<String>, Option<String>)> {
    for index in 0..3u32 {
        let mut buf = [0u8; size_of::<LenovoBatteryInfo>()];
        if energy_dev
            .ioctl_buf(IOCTL_ENERGY_BATTERY_INFORMATION, &index.to_ne_bytes(), &mut buf)
            .is_ok()
        {
            // Đọc theo căn chỉnh chuẩn C# (offset 14); nếu nhiệt độ không hợp lệ thì thử offset 13 (packed).
            let u16_at = |off: usize| u16::from_le_bytes([buf[off], buf[off + 1]]);
            for base in [14usize, 13usize] {
                let raw_t = u16_at(base);
                if let Some(temp) = decode_temp_c(raw_t) {
                    let mfg = decode_date(u16_at(base + 2));
                    let first = decode_date(u16_at(base + 4));
                    return Some((Some(temp), mfg, first));
                }
            }
        }
    }
    None
}

struct DevInfoGuard(HDEVINFO);
impl Drop for DevInfoGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

/// Mở thiết bị pin đầu tiên qua `GUID_DEVCLASS_BATTERY` (giống `Devices.GetBattery` của bản C#).
pub fn open_system_battery() -> io::Result<Device> {
    unsafe {
        let hdev = SetupDiGetClassDevsW(
            Some(&GUID_DEVCLASS_BATTERY),
            PCWSTR::null(),
            None,
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
        .map_err(|e| io::Error::other(format!("SetupDiGetClassDevsW lỗi: {e}")))?;
        let _guard = DevInfoGuard(hdev);

        let mut iface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        SetupDiEnumDeviceInterfaces(hdev, None, &GUID_DEVCLASS_BATTERY, 0, &mut iface)
            .map_err(|e| io::Error::other(format!("SetupDiEnumDeviceInterfaces lỗi: {e}")))?;

        let mut required: u32 = 0;
        let _ = SetupDiGetDeviceInterfaceDetailW(hdev, &iface, None, 0, Some(&mut required), None);
        if required < size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32 {
            return Err(io::Error::other("SetupDiGetDeviceInterfaceDetailW không trả kích thước hợp lệ"));
        }

        // Cấp phát mảng u16 đủ chứa SP_DEVICE_INTERFACE_DETAIL_DATA_W (căn chỉnh 4/8 byte an toàn).
        let words = (required as usize).div_ceil(2) + 2;
        let mut buf = vec![0u16; words];
        let detail_ptr = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        (*detail_ptr).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;

        SetupDiGetDeviceInterfaceDetailW(hdev, &iface, Some(detail_ptr), required, None, None)
            .map_err(|e| io::Error::other(format!("SetupDiGetDeviceInterfaceDetailW lỗi: {e}")))?;

        let path_ptr = ptr::addr_of!((*detail_ptr).DevicePath).cast::<u16>();
        let max_chars = words.saturating_sub(2);
        let slice = std::slice::from_raw_parts(path_ptr, max_chars);
        let len = slice.iter().position(|&c| c == 0).unwrap_or(max_chars);
        let path = String::from_utf16_lossy(&slice[..len]);
        Device::open(&path, ACCESS_READ_WRITE)
    }
}

pub fn detail(energy_dev: Option<&Device>, tracker: &mut RateTracker) -> io::Result<BatteryDetail> {
    let mut sps = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut sps) }
        .map_err(|e| io::Error::other(format!("GetSystemPowerStatus lỗi: {e}")))?;

    let bat_dev = open_system_battery()?;
    let tag: u32 = bat_dev.ioctl(IOCTL_BATTERY_QUERY_TAG, 0)?;

    let q_info = BATTERY_QUERY_INFORMATION {
        BatteryTag: tag,
        InformationLevel: BatteryInformation,
        AtRate: 0,
    };
    let mut info = BATTERY_INFORMATION::default();
    unsafe {
        let in_bytes = std::slice::from_raw_parts(ptr::from_ref(&q_info).cast::<u8>(), size_of::<BATTERY_QUERY_INFORMATION>());
        let out_bytes = std::slice::from_raw_parts_mut(ptr::from_mut(&mut info).cast::<u8>(), size_of::<BATTERY_INFORMATION>());
        bat_dev.ioctl_buf(IOCTL_BATTERY_QUERY_INFORMATION, in_bytes, out_bytes)?;
    }

    let wait = BATTERY_WAIT_STATUS {
        BatteryTag: tag,
        ..Default::default()
    };
    let mut status = BATTERY_STATUS::default();
    unsafe {
        let in_bytes = std::slice::from_raw_parts(ptr::from_ref(&wait).cast::<u8>(), size_of::<BATTERY_WAIT_STATUS>());
        let out_bytes = std::slice::from_raw_parts_mut(ptr::from_mut(&mut status).cast::<u8>(), size_of::<BATTERY_STATUS>());
        bat_dev.ioctl_buf(IOCTL_BATTERY_QUERY_STATUS, in_bytes, out_bytes)?;
    }

    let (min_rate_mw, max_rate_mw) = tracker.update(status.Rate);
    let (temperature_c, manufacture_date, first_use_date) = energy_dev
        .and_then(read_lenovo_battery)
        .unwrap_or((None, None, None));

    let ac_online = sps.ACLineStatus == 1;
    let is_charging = ac_online && (sps.BatteryFlag & 8 != 0 || status.Rate > 0);
    let life_remaining_secs = (sps.BatteryLifeTime != u32::MAX).then_some(sps.BatteryLifeTime);

    Ok(BatteryDetail {
        ac_online,
        is_charging,
        is_low_battery: !ac_online && info.DefaultAlert2 >= status.Capacity,
        percent: if sps.BatteryLifePercent <= 100 { u32::from(sps.BatteryLifePercent) } else { 0 },
        life_remaining_secs,
        rate_mw: (status.Rate != i32::MIN).then_some(status.Rate),
        min_rate_mw,
        max_rate_mw,
        current_capacity_mwh: status.Capacity,
        full_charge_capacity_mwh: info.FullChargedCapacity,
        design_capacity_mwh: info.DesignedCapacity,
        cycle_count: info.CycleCount,
        temperature_c,
        manufacture_date,
        first_use_date,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_raw_prefers_storage_over_express() {
        assert_eq!(BatteryState::from_raw(0x24), BatteryState::Conservation);
        assert_eq!(BatteryState::from_raw(0x04), BatteryState::RapidCharge);
        assert_eq!(BatteryState::from_raw(0x00), BatteryState::Normal);
    }

    #[test]
    fn codes_match_csharp() {
        assert_eq!(BatteryState::Conservation.codes(), [0x08, 0x03]);
        assert_eq!(BatteryState::Normal.codes(), [0x05, 0x08]);
        assert_eq!(BatteryState::RapidCharge.codes(), [0x05, 0x07]);
    }

    #[test]
    fn decodes_lenovo_battery_temperature_and_dates() {
        // (3032 - 2731.6) / 10.0 = 30.04 °C
        let t = decode_temp_c(3032).unwrap();
        assert!((t - 30.04).abs() < 0.01);
        assert_eq!(decode_temp_c(0), None);
        assert_eq!(decode_temp_c(u16::MAX), None);
        assert_eq!(decode_temp_c(2500), None); // âm độ C

        // 2024-05-17: ((2024 - 1980) << 9) | (5 << 5) | 17
        let raw_date = ((2024 - 1980) << 9) | (5 << 5) | 17;
        assert_eq!(decode_date(raw_date).as_deref(), Some("2024-05-17"));
        assert_eq!(decode_date(0), None);
        // Năm ngoài 2018..=2030 bị loại như bản C#
        let old_date = ((2015 - 1980) << 9) | (1 << 5) | 1;
        assert_eq!(decode_date(old_date), None);
    }

    #[test]
    fn rate_tracker_follows_csharp_min_max_rules() {
        let mut tr = RateTracker::default();
        assert_eq!(tr.update(-15_000), (Some(-15_000), Some(-15_000)));
        assert_eq!(tr.update(-10_000), (Some(-10_000), Some(-15_000)));
        assert_eq!(tr.update(-22_000), (Some(-10_000), Some(-22_000)));
        // Đổi dấu sang sạc (+) phải reset min/max
        assert_eq!(tr.update(18_000), (Some(18_000), Some(18_000)));
        assert_eq!(tr.update(0), (Some(0), Some(0)));
        assert_eq!(tr.update(i32::MIN), (None, Some(0)));
    }

    #[test]
    fn reads_real_battery_details_if_present() {
        let dev = open().ok();
        let mut tr = RateTracker::default();
        if let Ok(d) = detail(dev.as_ref(), &mut tr) {
            assert!(d.percent <= 100);
            assert!(d.design_capacity_mwh > 0);
            assert!(d.full_charge_capacity_mwh > 0);
            assert!(d.health_percent().is_some_and(|h| h > 10.0 && h < 150.0));
        }
    }
}
