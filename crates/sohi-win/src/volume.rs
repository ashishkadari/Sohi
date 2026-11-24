//! The speakers' volume, and the media keys.

use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE,
    VK_MEDIA_PREV_TRACK, VK_VOLUME_MUTE,
};

use crate::WinError;
use crate::apps::with_com;

/// The volume of the default speakers, 0 to 100.
///
/// # Errors
/// Returns a [`WinError`] if there are no speakers.
pub fn volume() -> Result<u8, WinError> {
    with_com(|| level(&endpoint()?))
}

/// Sets the volume of the default speakers to `percent` (at most 100), and
/// unmutes them unless it is 0.
///
/// # Errors
/// Returns a [`WinError`] if there are no speakers.
pub fn set_volume(percent: u8) -> Result<(), WinError> {
    with_com(|| {
        let endpoint = endpoint()?;
        let scalar = f32::from(percent.min(100)) / 100.0;
        // SAFETY: a live endpoint; no event context.
        unsafe { endpoint.SetMasterVolumeLevelScalar(scalar, std::ptr::null()) }
            .map_err(|e| WinError::api("SetMasterVolumeLevelScalar", &e))?;
        if percent > 0 {
            // SAFETY: as above.
            unsafe { endpoint.SetMute(false, std::ptr::null()) }
                .map_err(|e| WinError::api("IAudioEndpointVolume::SetMute", &e))?;
        }
        Ok(())
    })
}

/// The default speakers. COM must be initialized.
fn endpoint() -> Result<IAudioEndpointVolume, WinError> {
    // SAFETY: COM is initialized (see `with_com`).
    let devices: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| WinError::api("CoCreateInstance(MMDeviceEnumerator)", &e))?;
    // SAFETY: as above.
    let device = unsafe { devices.GetDefaultAudioEndpoint(eRender, eMultimedia) }
        .map_err(|e| WinError::api("GetDefaultAudioEndpoint", &e))?;
    // SAFETY: as above.
    unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| WinError::api("IMMDevice::Activate", &e))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=100"
)]
fn level(endpoint: &IAudioEndpointVolume) -> Result<u8, WinError> {
    // SAFETY: a live endpoint; the wrapper supplies the out-pointer.
    let scalar = unsafe { endpoint.GetMasterVolumeLevelScalar() }
        .map_err(|e| WinError::api("GetMasterVolumeLevelScalar", &e))?;
    Ok((scalar * 100.0).round().clamp(0.0, 100.0) as u8)
}

/// A key that controls whatever is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    /// Play or pause.
    PlayPause,
    /// The next track.
    Next,
    /// The previous track.
    Previous,
    /// Sound off, or back on.
    Mute,
}

/// Presses and releases a media key.
///
/// # Errors
/// Returns a [`WinError`] if Windows refuses the key press.
pub fn press_media_key(key: MediaKey) -> Result<(), WinError> {
    let vk = match key {
        MediaKey::PlayPause => VK_MEDIA_PLAY_PAUSE,
        MediaKey::Next => VK_MEDIA_NEXT_TRACK,
        MediaKey::Previous => VK_MEDIA_PREV_TRACK,
        MediaKey::Mute => VK_VOLUME_MUTE,
    };
    let inputs = [
        input(vk, KEYEVENTF_EXTENDEDKEY),
        input(vk, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
    ];
    let size = i32::try_from(size_of::<INPUT>()).unwrap_or(0);
    // SAFETY: an array of initialized INPUTs and their true size.
    let sent = unsafe { SendInput(&inputs, size) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(WinError::last("SendInput"))
    }
}

fn input(key: VIRTUAL_KEY, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: flags,
                ..KEYBDINPUT::default()
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "changes the volume for a moment"]
    fn volume_round_trip() {
        let before = volume().unwrap();
        for percent in [30, 31, 0, 100] {
            set_volume(percent.min(40)).unwrap();
            assert_eq!(volume().unwrap(), percent.min(40));
        }
        set_volume(before).unwrap();
        assert_eq!(volume().unwrap(), before);
    }
}
