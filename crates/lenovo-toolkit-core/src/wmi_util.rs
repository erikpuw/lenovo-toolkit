//! Gọi method WMI trong namespace ROOT\WMI cho các lớp LENOVO_*.
//!
//! `Wmi` giữ một kết nối COM và giải phóng khi drop (RAII). Mỗi lệnh gọi
//! là instance method trên instance đầu tiên của lớp, giống `WMI.CallAsync` của bản C#.

use std::io;

use serde::Deserialize;
use wmi::{IWbemClassWrapper, Variant, WMIConnection};

const NAMESPACE: &str = r"ROOT\WMI";

pub struct Wmi(WMIConnection);

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct Instance {
    __Path: String,
}

fn err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

impl Wmi {
    pub fn open() -> io::Result<Self> {
        WMIConnection::with_namespace_path(NAMESPACE).map(Self).map_err(err)
    }

    /// Kết nối namespace mặc định ROOT\CIMV2 (Win32_* như ComputerSystemProduct).
    pub fn open_cimv2() -> io::Result<Self> {
        WMIConnection::new().map(Self).map_err(err)
    }

    /// Truy vấn WQL, giải mã mỗi hàng thành `T` (dùng `Variant` cho cột kiểu số không chắc).
    pub fn query<T: serde::de::DeserializeOwned>(&self, wql: &str) -> io::Result<Vec<T>> {
        self.0.raw_query(wql).map_err(err)
    }

    /// Đăng ký truy vấn sự kiện WMI, gọi `ready` khi đăng ký xong rồi chặn, gọi `on_event`
    /// cho mỗi sự kiện cho đến khi nó trả về `false` hoặc có lỗi.
    pub fn watch(
        &self,
        wql: &str,
        ready: impl FnOnce(),
        mut on_event: impl FnMut(std::collections::HashMap<String, Variant>) -> bool,
    ) -> io::Result<()> {
        let events = self
            .0
            .raw_notification::<std::collections::HashMap<String, Variant>>(wql)
            .map_err(err)?;
        ready();
        for item in events {
            if !on_event(item.map_err(err)?) {
                break;
            }
        }
        Ok(())
    }

    /// Gọi `method` của `class` với các tham số số nguyên; trả về object kết quả (nếu có).
    pub fn call(&self, class: &str, method: &str, args: &[(&str, i32)]) -> io::Result<Option<IWbemClassWrapper>> {
        let mut rows: Vec<Instance> = self
            .0
            .raw_query(format!("SELECT __Path FROM {class}"))
            .map_err(err)?;
        let path = rows
            .pop()
            .ok_or_else(|| io::Error::other(format!("không có instance {class} (máy không hỗ trợ)")))?
            .__Path;

        let in_params = match self.0.get_object(class).map_err(err)?.get_method(method).map_err(err)? {
            Some(def) => {
                let inst = def.spawn_instance().map_err(err)?;
                for (name, value) in args {
                    inst.put_property(name, Variant::I4(*value)).map_err(err)?;
                }
                Some(inst)
            }
            None => None,
        };
        self.0.exec_method(&path, method, in_params.as_ref()).map_err(err)
    }

    /// Gọi method và đọc một property số nguyên của kết quả.
    pub fn call_i32(&self, class: &str, method: &str, args: &[(&str, i32)], out: &str) -> io::Result<i32> {
        let result = self
            .call(class, method, args)?
            .ok_or_else(|| io::Error::other(format!("{method} không trả về kết quả")))?;
        variant_to_i32(result.get_property(out).map_err(err)?)
    }
}

pub fn variant_to_i32(v: Variant) -> io::Result<i32> {
    Ok(match v {
        Variant::I1(x) => x.into(),
        Variant::I2(x) => x.into(),
        Variant::I4(x) => x,
        Variant::UI1(x) => x.into(),
        Variant::UI2(x) => x.into(),
        Variant::UI4(x) => x as i32,
        Variant::Bool(b) => i32::from(b),
        other => return Err(io::Error::other(format!("kiểu WMI không phải số nguyên: {other:?}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_integer_variants() {
        assert_eq!(variant_to_i32(Variant::UI1(7)).unwrap(), 7);
        assert_eq!(variant_to_i32(Variant::I4(-1)).unwrap(), -1);
        assert_eq!(variant_to_i32(Variant::Bool(true)).unwrap(), 1);
        assert!(variant_to_i32(Variant::String("x".into())).is_err());
    }
}
