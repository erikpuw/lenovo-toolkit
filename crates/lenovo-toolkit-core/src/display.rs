//! Quản lý tần số quét (Refresh Rate) của màn hình chính và tính toán vòng lặp Fn+R.
//! Port từ `RefreshRateFeature.cs` & `SpecialKeyListener.cs` của bản C#.

use std::{io, mem};

use windows::Win32::Graphics::Gdi::{
    CDS_UPDATEREGISTRY, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL,
    DISPLAY_DEVICE_ACTIVE, DISPLAY_DEVICEW, DM_DISPLAYFREQUENCY, ENUM_CURRENT_SETTINGS,
    ENUM_DISPLAY_SETTINGS_MODE, EnumDisplayDevicesW, EnumDisplaySettingsW,
};
use windows::core::PCWSTR;

fn find_active_display_name() -> Option<[u16; 32]> {
    let mut idx = 0u32;
    loop {
        let mut dev = DISPLAY_DEVICEW {
            cb: mem::size_of::<DISPLAY_DEVICEW>() as u32,
            ..Default::default()
        };
        let ok = unsafe { EnumDisplayDevicesW(PCWSTR::null(), idx, &mut dev, 0).as_bool() };
        if !ok {
            break;
        }
        if (dev.StateFlags & DISPLAY_DEVICE_ACTIVE).0 != 0 {
            return Some(dev.DeviceName);
        }
        idx += 1;
    }
    None
}

fn current_devmode(device_ptr: PCWSTR) -> io::Result<DEVMODEW> {
    let mut mode = DEVMODEW {
        dmSize: mem::size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    let ok = unsafe { EnumDisplaySettingsW(device_ptr, ENUM_CURRENT_SETTINGS, &mut mode).as_bool() };
    if !ok {
        return Err(io::Error::other("EnumDisplaySettingsW(ENUM_CURRENT_SETTINGS) thất bại"));
    }
    Ok(mode)
}

/// Trả về `(current_hz, supported_hz_list)` của màn hình đang hoạt động.
pub fn refresh_rates() -> io::Result<(u32, Vec<u32>)> {
    let dev_name = find_active_display_name();
    let dev_ptr = dev_name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let cur = current_devmode(dev_ptr)?;
    let cur_hz = cur.dmDisplayFrequency;
    let cur_orient = unsafe { cur.Anonymous1.Anonymous2.dmDisplayOrientation };
    let cur_flags = unsafe { cur.Anonymous2.dmDisplayFlags };

    let mut rates = Vec::new();
    if cur_hz >= 30 {
        rates.push(cur_hz);
    }

    let mut i = 0u32;
    loop {
        let mut m = DEVMODEW {
            dmSize: mem::size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let ok = unsafe { EnumDisplaySettingsW(dev_ptr, ENUM_DISPLAY_SETTINGS_MODE(i), &mut m).as_bool() };
        if !ok {
            break;
        }
        let orient = unsafe { m.Anonymous1.Anonymous2.dmDisplayOrientation };
        let flags = unsafe { m.Anonymous2.dmDisplayFlags };
        if m.dmPelsWidth == cur.dmPelsWidth
            && m.dmPelsHeight == cur.dmPelsHeight
            && m.dmBitsPerPel == cur.dmBitsPerPel
            && orient == cur_orient
            && flags == cur_flags
            && m.dmDisplayFrequency >= 30
            && !rates.contains(&m.dmDisplayFrequency)
        {
            rates.push(m.dmDisplayFrequency);
        }
        i += 1;
    }

    rates.sort_unstable();
    Ok((cur_hz, rates))
}

/// Đặt tần số quét của màn hình đang hoạt động sang `hz`.
pub fn set_refresh_rate(hz: u32) -> io::Result<()> {
    let dev_name = find_active_display_name();
    let dev_ptr = dev_name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let mut cur = current_devmode(dev_ptr)?;
    if cur.dmDisplayFrequency == hz {
        return Ok(());
    }
    cur.dmDisplayFrequency = hz;
    cur.dmFields = DM_DISPLAYFREQUENCY;
    let code = unsafe { ChangeDisplaySettingsExW(dev_ptr, Some(&cur), None, CDS_UPDATEREGISTRY, None) };
    if code != DISP_CHANGE_SUCCESSFUL {
        return Err(io::Error::other(format!("ChangeDisplaySettingsExW trả về mã lỗi {}", code.0)));
    }
    Ok(())
}

/// Tính tần số quét kế tiếp khi nhấn `Fn+R`:
/// - Bỏ qua các mức nằm trong `excluded`.
/// - Nếu sau khi loại trừ chỉ còn 1 mức (ví dụ chỉ giữ `144 Hz`, loại trừ `60 Hz`):
///   nếu màn hình đang ở mức khác thì chuyển về mức duy nhất đó; nếu đã ở mức đó thì giữ cố định (`None`).
/// - Nếu còn `>= 2` mức thì xoay vòng sang mức kế tiếp.
pub fn next_refresh_rate(all: &[u32], excluded: &[u32], current: u32) -> Option<u32> {
    let available: Vec<u32> = all.iter().copied().filter(|hz| !excluded.contains(hz)).collect();
    if available.is_empty() {
        return None;
    }
    if available.len() == 1 {
        let only = available[0];
        return (current != only).then_some(only);
    }
    let next_idx = match available.iter().position(|&hz| hz == current) {
        Some(idx) => (idx + 1) % available.len(),
        None => 0,
    };
    Some(available[next_idx])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_between_all_non_excluded_rates() {
        let all = [60, 144];
        assert_eq!(next_refresh_rate(&all, &[], 60), Some(144));
        assert_eq!(next_refresh_rate(&all, &[], 144), Some(60));
    }

    #[test]
    fn locks_to_144_when_60_is_excluded() {
        let all = [60, 144];
        let excluded = [60];
        // Đang ở 60 Hz mà bấm Fn+R -> nhảy lên 144 Hz
        assert_eq!(next_refresh_rate(&all, &excluded, 60), Some(144));
        // Đang ở 144 Hz mà bấm Fn+R -> giữ cố định 144 Hz (không quay về 60 Hz)
        assert_eq!(next_refresh_rate(&all, &excluded, 144), None);
    }

    #[test]
    fn returns_none_when_all_excluded() {
        let all = [60, 144];
        assert_eq!(next_refresh_rate(&all, &[60, 144], 60), None);
    }
}
