//! Quạt và cảm biến nhiệt qua WMI (port từ SensorsController.cs của bản C#
//! và WMI.LenovoFanMethod / LenovoFanTableData / LenovoOtherMethod).
//!
//! Chưa có: profile `FanTestData`, bảng quạt (Fan_Set_Table/Fan_Get_Table),
//! cảm biến CPU/GPU qua LibreHardwareMonitor/NVAPI.

use std::collections::{HashMap, HashSet};
use std::io;

use serde::Deserialize;
use wmi::Variant;

use crate::wmi_util::{Wmi, variant_to_i32};

const FAN: &str = "LENOVO_FAN_METHOD";
const OTHER: &str = "LENOVO_OTHER_METHOD";

const CAP_CPU_FAN: i32 = 0x0403_0001;
const CAP_GPU_FAN: i32 = 0x0403_0002;
const CAP_PCH_FAN: i32 = 0x0403_0004;
const CAP_PCH_TEMP: i32 = 0x0501_0000;
const CAP_CPU_TEMP: i32 = 0x0504_0000;
const CAP_GPU_TEMP: i32 = 0x0505_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub sensor: i32,
    pub fan: i32,
}

const fn e(sensor: i32, fan: i32) -> Endpoint {
    Endpoint { sensor, fan }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Capability,
    FanMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaxSource {
    CurrentTable,
    DefaultTable,
}

#[derive(Debug)]
pub struct Profile {
    cpu: Endpoint,
    gpu: Endpoint,
    required: &'static [Endpoint],
    pch_fans: &'static [Endpoint],
    source: Source,
    max_source: MaxSource,
    cpu_sensor: i32,
    gpu_sensor: i32,
    has_pch_temp: bool,
}

impl Profile {
    fn required_endpoints(&self) -> impl Iterator<Item = Endpoint> + '_ {
        [self.cpu, self.gpu]
            .into_iter()
            .chain(self.pch_fans.iter().copied())
            .chain(self.required.iter().copied())
    }
}

const PROFILES: [Profile; 6] = [
    Profile { cpu: e(4, 1), gpu: e(5, 2), required: &[], pch_fans: &[e(1, 4), e(5, 5)], source: Source::Capability, max_source: MaxSource::CurrentTable, cpu_sensor: 0, gpu_sensor: 0, has_pch_temp: true },
    Profile { cpu: e(1, 1), gpu: e(5, 2), required: &[], pch_fans: &[e(4, 4)], source: Source::Capability, max_source: MaxSource::CurrentTable, cpu_sensor: 0, gpu_sensor: 0, has_pch_temp: true },
    Profile { cpu: e(1, 1), gpu: e(5, 2), required: &[e(4, 1)], pch_fans: &[], source: Source::Capability, max_source: MaxSource::CurrentTable, cpu_sensor: 0, gpu_sensor: 0, has_pch_temp: true },
    Profile { cpu: e(4, 1), gpu: e(5, 2), required: &[], pch_fans: &[], source: Source::Capability, max_source: MaxSource::CurrentTable, cpu_sensor: 0, gpu_sensor: 0, has_pch_temp: false },
    Profile { cpu: e(3, 0), gpu: e(4, 1), required: &[], pch_fans: &[], source: Source::FanMethod, max_source: MaxSource::CurrentTable, cpu_sensor: 3, gpu_sensor: 4, has_pch_temp: false },
    Profile { cpu: e(0, 0), gpu: e(0, 1), required: &[], pch_fans: &[], source: Source::FanMethod, max_source: MaxSource::DefaultTable, cpu_sensor: 3, gpu_sensor: 4, has_pch_temp: false },
];

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct TableRow {
    Sensor_ID: Variant,
    Fan_Id: Variant,
}

/// Chọn profile đầu tiên có đủ endpoint trong LENOVO_FAN_TABLE_DATA.
pub fn detect(w: &Wmi) -> io::Result<&'static Profile> {
    let rows: Vec<TableRow> = w.query("SELECT Sensor_ID, Fan_Id FROM LENOVO_FAN_TABLE_DATA")?;
    let present: HashSet<Endpoint> = rows
        .into_iter()
        .filter_map(|r| Some(e(variant_to_i32(r.Sensor_ID).ok()?, variant_to_i32(r.Fan_Id).ok()?)))
        .collect();
    PROFILES
        .iter()
        .find(|p| p.required_endpoints().all(|ep| present.contains(&ep)))
        .ok_or_else(|| io::Error::other("không có profile quạt phù hợp (máy không hỗ trợ?)"))
}

fn capability(w: &Wmi, id: i32) -> io::Result<i32> {
    w.call_i32(OTHER, "GetFeatureValue", &[("IDs", id)], "Value")
}

fn temp_or_unknown(v: i32) -> i32 {
    if v < 1 { -1 } else { v }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sensors {
    pub cpu_temp: i32,
    pub gpu_temp: i32,
    pub pch_temp: i32,
    pub cpu_fan_rpm: i32,
    pub gpu_fan_rpm: i32,
    pub pch_fan_rpm: i32,
}

pub fn read(w: &Wmi, p: &Profile) -> io::Result<Sensors> {
    let temp = |cap: i32, sensor: i32| -> io::Result<i32> {
        Ok(temp_or_unknown(match p.source {
            Source::Capability => capability(w, cap)?,
            Source::FanMethod => w.call_i32(FAN, "Fan_GetCurrentSensorTemperature", &[("SensorID", sensor)], "CurrentSensorTemperature")?,
        }))
    };
    let fan = |cap: i32, fan_id: i32| -> io::Result<i32> {
        match p.source {
            Source::Capability => capability(w, cap),
            Source::FanMethod => w.call_i32(FAN, "Fan_GetCurrentFanSpeed", &[("FanID", fan_id)], "CurrentFanSpeed"),
        }
    };
    Ok(Sensors {
        cpu_temp: temp(CAP_CPU_TEMP, p.cpu_sensor)?,
        gpu_temp: temp(CAP_GPU_TEMP, p.gpu_sensor)?,
        pch_temp: if p.has_pch_temp { capability(w, CAP_PCH_TEMP)? } else { -1 },
        cpu_fan_rpm: fan(CAP_CPU_FAN, p.cpu.fan)?,
        gpu_fan_rpm: fan(CAP_GPU_FAN, p.gpu.fan)?,
        pch_fan_rpm: if p.pch_fans.is_empty() { -1 } else { capability(w, CAP_PCH_FAN)? },
    })
}

/// Tốc độ tối đa của quạt CPU và GPU theo bảng của profile.
pub fn max_speeds(w: &Wmi, p: &Profile) -> io::Result<(i32, i32)> {
    // Chỉ chọn đúng một cột: lớp WMI có thể không có cột còn lại (lỗi 0x80041002).
    let column = match p.max_source {
        MaxSource::CurrentTable => "CurrentFanMaxSpeed",
        MaxSource::DefaultTable => "DefaultFanMaxSpeed",
    };
    let one = |ep: Endpoint| -> io::Result<i32> {
        let rows: Vec<HashMap<String, Variant>> = w.query(&format!(
            "SELECT {column} FROM LENOVO_FAN_TABLE_DATA WHERE Sensor_ID = {} AND Fan_Id = {}",
            ep.sensor, ep.fan
        ))?;
        rows.into_iter()
            .filter_map(|mut r| r.remove(column))
            .filter_map(|v| variant_to_i32(v).ok())
            .max()
            .ok_or_else(|| io::Error::other("không đọc được tốc độ tối đa của quạt"))
    };
    Ok((one(p.cpu)?, one(p.gpu)?))
}

pub fn full_speed_get(w: &Wmi) -> io::Result<bool> {
    Ok(w.call_i32(FAN, "Fan_Get_FullSpeed", &[], "Status")? != 0)
}

pub fn full_speed_set(w: &Wmi, on: bool) -> io::Result<()> {
    w.call(FAN, "Fan_Set_FullSpeed", &[("Status", i32::from(on))])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_temperature_is_minus_one() {
        assert_eq!(temp_or_unknown(0), -1);
        assert_eq!(temp_or_unknown(-5), -1);
        assert_eq!(temp_or_unknown(62), 62);
    }

    #[test]
    fn required_endpoints_cover_cpu_gpu_pch_and_extra() {
        let p = &PROFILES[2];
        let eps: Vec<_> = p.required_endpoints().collect();
        assert_eq!(eps, vec![e(1, 1), e(5, 2), e(4, 1)]);
        assert_eq!(PROFILES[0].required_endpoints().count(), 4);
    }

    #[test]
    fn profile_with_more_requirements_never_shadows_earlier_match() {
        // Profile 5 (index 4) chỉ cần (3,0),(4,1); tập chỉ có 2 endpoint đó khớp profile 4.
        let present: HashSet<Endpoint> = [e(3, 0), e(4, 1)].into_iter().collect();
        let found = PROFILES.iter().position(|p| p.required_endpoints().all(|ep| present.contains(&ep)));
        assert_eq!(found, Some(4));
    }
}
