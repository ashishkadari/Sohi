//! What the capture and render threads share: COM and MMCSS around the
//! session, and opening a WASAPI shared-mode endpoint in Sohi's format
//! (mono i16, converted by Windows).

use windows::Win32::Media::Audio::{
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, EDataFlow,
    IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX, eConsole,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, CreateEventW,
};
use windows::core::w;

use crate::{OwnedHandle, WinError};

/// Runs `session` with COM initialized and the thread registered with
/// MMCSS "Pro Audio". Call on a new thread; every COM object must be
/// dropped inside `session`.
pub(crate) fn pro_audio(session: impl FnOnce() -> Result<(), WinError>) -> Result<(), WinError> {
    // SAFETY: first COM call on this new thread; balanced below.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|e| WinError::api("CoInitializeEx", &e))?;
    let mut task_index = 0u32;
    // SAFETY: a valid task name and out-pointer; reverted below.
    let mmcss = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &raw mut task_index) };
    let result = session();
    if let Ok(handle) = mmcss {
        // SAFETY: the handle came from AvSetMmThreadCharacteristicsW above.
        let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
    }
    // SAFETY: balances CoInitializeEx; every COM object was dropped in
    // `session`.
    unsafe { CoUninitialize() };
    result
}

/// An auto-reset event, not signalled.
pub(crate) fn event() -> Result<OwnedHandle, WinError> {
    // SAFETY: plain auto-reset event, no name or security attributes.
    let event = unsafe { CreateEventW(None, false, false, None) }
        .map_err(|e| WinError::api("CreateEventW", &e))?;
    // SAFETY: the event was just created and is owned only here.
    Ok(unsafe { OwnedHandle::from_raw(event) })
}

/// The default endpoint for `flow`. COM must be initialized.
pub(crate) fn open_endpoint(flow: EDataFlow) -> Result<IAudioClient, WinError> {
    // SAFETY: COM is initialized on this thread (see `pro_audio`).
    let devices: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|e| WinError::api("CoCreateInstance(MMDeviceEnumerator)", &e))?;
    // SAFETY: as above.
    let device = unsafe { devices.GetDefaultAudioEndpoint(flow, eConsole) }
        .map_err(|e| WinError::api("GetDefaultAudioEndpoint", &e))?;
    // SAFETY: as above.
    unsafe { device.Activate(CLSCTX_ALL, None) }
        .map_err(|e| WinError::api("IMMDevice::Activate", &e))
}

/// Initializes `client` for event-driven mono i16 at `sample_rate` with a
/// buffer of `buffer_hns` (100 ns units).
pub(crate) fn initialize(
    client: &IAudioClient,
    sample_rate: u32,
    buffer_hns: i64,
) -> Result<(), WinError> {
    let format = WAVEFORMATEX {
        wFormatTag: u16::try_from(WAVE_FORMAT_PCM).unwrap_or(1),
        nChannels: 1,
        nSamplesPerSec: sample_rate,
        nAvgBytesPerSec: sample_rate * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    // SAFETY: `format` outlives the call; shared mode takes a periodicity
    // of 0.
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            buffer_hns,
            0,
            &raw const format,
            None,
        )
    }
    .map_err(|e| WinError::api("IAudioClient::Initialize", &e))
}
