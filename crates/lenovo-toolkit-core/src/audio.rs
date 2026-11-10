//! Tắt/bật mic bằng mute của Windows (port rút gọn từ MicrophoneFeature.cs).
//!
//! Áp dụng cho mọi thiết bị thu đang hoạt động. Chưa có: đèn LED trên phím F4
//! (SpecialKeyLedHelper) nên đèn phím có thể lệch với trạng thái thật.

use std::io;

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator, eCapture};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};

fn err(e: windows::core::Error) -> io::Error {
    io::Error::other(format!("Core Audio lỗi: {e}"))
}

/// Trạng thái mute mới khi bấm phím: nếu mọi mic đang mute thì bỏ mute, ngược lại mute hết.
pub fn next_mute(current: &[bool]) -> bool {
    !(!current.is_empty() && current.iter().all(|m| *m))
}

/// Đảo trạng thái mic. Trả về `true` nếu mic đang BẬT sau khi đổi, `None` nếu không có mic nào.
pub fn toggle_microphone() -> io::Result<Option<bool>> {
    unsafe {
        // Lỗi RPC_E_CHANGED_MODE (luồng đã có COM) vẫn dùng được; chỉ Uninitialize khi ta khởi tạo thành công.
        let inited = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| -> io::Result<Option<bool>> {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(err)?;
            let devices = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE).map_err(err)?;
            let mut volumes = Vec::new();
            for i in 0..devices.GetCount().map_err(err)? {
                let dev = devices.Item(i).map_err(err)?;
                volumes.push(dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).map_err(err)?);
            }
            if volumes.is_empty() {
                return Ok(None);
            }
            let states = volumes
                .iter()
                .map(|v| v.GetMute().map(|m| m.as_bool()).map_err(err))
                .collect::<io::Result<Vec<_>>>()?;
            let mute = next_mute(&states);
            for v in &volumes {
                v.SetMute(mute, std::ptr::null()).map_err(err)?;
            }
            Ok(Some(!mute))
        })();
        if inited {
            CoUninitialize();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_mute_unmutes_only_when_everything_is_muted() {
        assert!(next_mute(&[false]));
        assert!(next_mute(&[true, false]));
        assert!(!next_mute(&[true, true]));
        assert!(next_mute(&[])); // không có mic: giá trị không dùng, nhưng không panic
    }
}
