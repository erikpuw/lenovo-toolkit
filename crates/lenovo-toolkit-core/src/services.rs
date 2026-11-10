//! Điều khiển Windows service/driver qua Service Control Manager
//! (thay cho `ServiceController` + `ChangeStartMode` của bản C#).

use std::io;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, ControlService, ENUM_SERVICE_STATUS_PROCESSW, EnumServicesStatusExW,
    OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_ALL_ACCESS,
    SC_MANAGER_ENUMERATE_SERVICE,
    SERVICE_ACCEPT_STOP, SERVICE_AUTO_START, SERVICE_CHANGE_CONFIG, SERVICE_CONTROL_STOP, SERVICE_DISABLED,
    SERVICE_DRIVER, SERVICE_ERROR, SERVICE_NO_CHANGE, SERVICE_QUERY_STATUS, SERVICE_START, SERVICE_STATE_ALL,
    SERVICE_STATUS, SERVICE_STOP, SERVICE_WIN32, StartServiceW,
};
use windows::Win32::System::Services::ENUM_SERVICE_TYPE;
use windows::core::PCWSTR;

const SERVICE_STOPPED: u32 = 1;
const SERVICE_RUNNING: u32 = 4;
const WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceInfo {
    pub name: String,
    /// Trạng thái khác "Stopped" (giống `IsServiceEnabled` của bản C#).
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopResult {
    Stopped,
    /// Không dừng được (không cho dừng hoặc quá thời gian) — cần khởi động lại máy.
    NotStopped,
}

pub struct Scm(SC_HANDLE);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn os_err(what: &str, e: windows::core::Error) -> io::Error {
    io::Error::other(format!("{what}: {e}"))
}

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

impl Scm {
    pub fn open() -> io::Result<Self> {
        Self::open_with(SC_MANAGER_ALL_ACCESS)
    }

    /// Chỉ liệt kê service (không cần quyền admin): đủ để đọc trạng thái.
    pub fn open_read() -> io::Result<Self> {
        Self::open_with(SC_MANAGER_ENUMERATE_SERVICE)
    }

    fn open_with(access: u32) -> io::Result<Self> {
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access) }
            .map(Self)
            .map_err(|e| os_err("không mở được Service Manager", e))
    }

    fn service(&self, name: &str, access: u32) -> io::Result<Handle> {
        let n = wide(name);
        unsafe { OpenServiceW(self.0, PCWSTR(n.as_ptr()), access) }
            .map(Handle)
            .map_err(|e| os_err(&format!("không mở được service {name}"), e))
    }

    /// Tất cả service Win32 và driver (giống `GetServices().Concat(GetDevices())`).
    pub fn list(&self) -> io::Result<Vec<ServiceInfo>> {
        let mut resume = 0u32;
        let mut result = Vec::new();
        loop {
            let (mut needed, mut count) = (0u32, 0u32);
            // Bộ đệm u64 để căn lề đúng cho ENUM_SERVICE_STATUS_PROCESSW.
            let mut buf = vec![0u64; 16 * 1024];
            let bytes = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), buf.len() * 8) };
            let r = unsafe {
                EnumServicesStatusExW(
                    self.0,
                    SC_ENUM_PROCESS_INFO,
                    ENUM_SERVICE_TYPE(SERVICE_WIN32.0 | SERVICE_DRIVER.0),
                    SERVICE_STATE_ALL,
                    Some(bytes),
                    &mut needed,
                    &mut count,
                    Some(&mut resume),
                    PCWSTR::null(),
                )
            };
            let base = buf.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>();
            for i in 0..count as usize {
                let e = unsafe { base.add(i).read_unaligned() };
                let name = unsafe { e.lpServiceName.to_string() }.unwrap_or_default();
                result.push(ServiceInfo { name, active: e.ServiceStatusProcess.dwCurrentState.0 != SERVICE_STOPPED });
            }
            match r {
                Ok(()) => return Ok(result),
                // ERROR_MORE_DATA: còn dữ liệu, lặp lại với resume handle
                Err(e) if e.code().0 as u32 == 0x8007_00EA => continue,
                Err(e) => return Err(os_err("EnumServicesStatusEx lỗi", e)),
            }
        }
    }

    /// Đặt kiểu khởi động: tự động khi bật, vô hiệu hóa khi tắt (`ChangeStartMode`).
    pub fn set_start_enabled(&self, name: &str, enabled: bool) -> io::Result<()> {
        let svc = self.service(name, SERVICE_CHANGE_CONFIG)?;
        unsafe {
            ChangeServiceConfigW(
                svc.0,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                if enabled { SERVICE_AUTO_START } else { SERVICE_DISABLED },
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            )
        }
        .map_err(|e| os_err(&format!("không đổi được kiểu khởi động của {name}"), e))
    }

    fn status(svc: &Handle) -> io::Result<SERVICE_STATUS> {
        let mut s = SERVICE_STATUS::default();
        unsafe { QueryServiceStatus(svc.0, &mut s) }.map_err(|e| os_err("QueryServiceStatus lỗi", e))?;
        Ok(s)
    }

    fn wait_for(svc: &Handle, state: u32) -> bool {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if Self::status(svc).is_ok_and(|s| s.dwCurrentState.0 == state) {
                return true;
            }
            thread::sleep(Duration::from_millis(200));
        }
        false
    }

    /// Khởi động service nếu chưa chạy; lỗi khởi động chỉ trả `Ok(false)` (có thể thiếu file thực thi).
    pub fn start(&self, name: &str) -> io::Result<bool> {
        let svc = self.service(name, SERVICE_START | SERVICE_QUERY_STATUS)?;
        if Self::status(&svc)?.dwCurrentState.0 == SERVICE_RUNNING {
            return Ok(true);
        }
        if unsafe { StartServiceW(svc.0, None) }.is_err() {
            return Ok(false);
        }
        Ok(Self::wait_for(&svc, SERVICE_RUNNING))
    }

    pub fn stop(&self, name: &str) -> io::Result<StopResult> {
        let svc = self.service(name, SERVICE_STOP | SERVICE_QUERY_STATUS)?;
        let status = Self::status(&svc)?;
        if status.dwCurrentState.0 == SERVICE_STOPPED {
            return Ok(StopResult::Stopped);
        }
        if status.dwControlsAccepted & SERVICE_ACCEPT_STOP == 0 {
            return Ok(StopResult::NotStopped);
        }
        let mut s = SERVICE_STATUS::default();
        if unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut s) }.is_err() {
            return Ok(StopResult::NotStopped);
        }
        Ok(if Self::wait_for(&svc, SERVICE_STOPPED) { StopResult::Stopped } else { StopResult::NotStopped })
    }
}

impl Drop for Scm {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_core_windows_services() {
        let scm = Scm::open_read().expect("liệt kê service không cần admin");
        let all = scm.list().unwrap();
        assert!(all.len() > 50);
        let rpc = all.iter().find(|s| s.name.eq_ignore_ascii_case("RpcSs")).expect("RpcSs");
        assert!(rpc.active);
    }
}
