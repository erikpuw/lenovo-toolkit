//! Đèn qua WMI LENOVO_LIGHTING_METHOD (port từ AbstractLenovoLightingFeature.cs,
//! PortsBacklightFeature, PanelLogoLenovoLightingBacklightFeature,
//! WhiteKeyboardLenovoLightingBacklightFeature và WMI.LenovoLightingData).
//!
//! Chưa có: Spectrum (RGB 4 vùng/từng phím qua HID), hiệu ứng, LampArray.

use std::io;

use serde::Deserialize;
use wmi::Variant;

use crate::wmi_util::{Wmi, variant_to_i32};

const METHOD: &str = "LENOVO_LIGHTING_METHOD";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Light {
    /// Đèn cổng: 0 = Off, 1 = On
    Ports,
    /// Logo màn hình: 0 = Off, 1 = On
    Logo,
    /// Bàn phím trắng: 0 = Off, 1 = Low, 2 = High
    Keyboard,
}

impl Light {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ports" => Self::Ports,
            "logo" => Self::Logo,
            "kbd" => Self::Keyboard,
            _ => return None,
        })
    }

    /// (lighting_id, control_interface, type)
    const fn ids(self) -> (i32, i32, i32) {
        match self {
            Self::Ports => (5, 1, 0),
            Self::Logo => (3, 1, 0),
            Self::Keyboard => (0, 0, 1),
        }
    }

    const fn max_value(self) -> i32 {
        match self {
            Self::Keyboard => 2,
            _ => 1,
        }
    }

    /// (state_type, level) gửi cho WMI.
    fn to_internal(self, value: i32) -> (i32, i32) {
        match self {
            Self::Keyboard => (0, value + 1),
            _ => (value, 0),
        }
    }

    fn from_internal(self, state_type: i32, level: i32) -> i32 {
        match self {
            Self::Keyboard => level - 1,
            _ => state_type,
        }
    }
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct TypeRow {
    Lighting_Type: Variant,
}

/// Loại bàn phím (bit 1..3 của Lighting_Type); `None` nếu không có dữ liệu đèn.
pub fn keyboard_type(w: &Wmi) -> io::Result<Option<i32>> {
    let rows: Vec<TypeRow> = w.query("SELECT Lighting_Type FROM LENOVO_LIGHTING_DATA")?;
    if rows.is_empty() {
        return Ok(None);
    }
    for row in rows {
        let t = (variant_to_i32(row.Lighting_Type)? >> 1) & 7;
        if t != 0 {
            return Ok(Some(t));
        }
    }
    Ok(Some(0))
}

pub fn is_supported(w: &Wmi, light: Light) -> io::Result<bool> {
    let (id, ci, ty) = light.ids();
    let rows: Vec<TypeRow> = w.query(&format!(
        "SELECT Lighting_Type FROM LENOVO_LIGHTING_DATA WHERE Lighting_ID = {id} AND Control_Interface = {ci} AND Lighting_Type = {ty}"
    ))?;
    Ok(!rows.is_empty() && get(w, light).is_ok())
}

pub fn get(w: &Wmi, light: Light) -> io::Result<i32> {
    let (id, ..) = light.ids();
    let out = w
        .call(METHOD, "Get_Lighting_Current_Status", &[("Lighting_ID", id)])?
        .ok_or_else(|| io::Error::other("Get_Lighting_Current_Status không trả về kết quả"))?;
    let prop = |name: &str| -> io::Result<i32> {
        variant_to_i32(out.get_property(name).map_err(|e| io::Error::other(e.to_string()))?)
    };
    Ok(light.from_internal(prop("Current_State_Type")?, prop("Current_Brightness_Level")?))
}

pub fn set(w: &Wmi, light: Light, value: i32) -> io::Result<()> {
    if !(0..=light.max_value()).contains(&value) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{light:?}: giá trị phải trong 0..={}", light.max_value()),
        ));
    }
    let (id, ..) = light.ids();
    let (state_type, level) = light.to_internal(value);
    w.call(
        METHOD,
        "Set_Lighting_Current_Status",
        &[
            ("Lighting_ID", id),
            ("Current_State_Type", state_type),
            ("Current_Brightness_Level", level),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_level_is_offset_by_one() {
        assert_eq!(Light::Keyboard.to_internal(0), (0, 1));
        assert_eq!(Light::Keyboard.to_internal(2), (0, 3));
        assert_eq!(Light::Keyboard.from_internal(0, 1), 0);
        assert_eq!(Light::Keyboard.from_internal(0, 3), 2);
    }

    #[test]
    fn ports_and_logo_use_state_type() {
        assert_eq!(Light::Ports.to_internal(1), (1, 0));
        assert_eq!(Light::Logo.from_internal(1, 99), 1);
    }

    #[test]
    fn ids_match_csharp() {
        assert_eq!(Light::Ports.ids(), (5, 1, 0));
        assert_eq!(Light::Logo.ids(), (3, 1, 0));
        assert_eq!(Light::Keyboard.ids(), (0, 0, 1));
    }
}
