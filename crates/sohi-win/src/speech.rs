//! Offline speech with the Windows `OneCore` voices (spec §2.3, §5.5).
//!
//! `Windows.Media.SpeechSynthesis` renders text to a WAV in memory; the
//! caller plays it through the same path as online speech. It works
//! without a network.

use sohi_audio::wav;
use windows::Media::SpeechSynthesis::{SpeechSynthesizer, VoiceInformation};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::HSTRING;

use crate::WinError;

/// Use whichever voice Windows is set to.
const SYSTEM_DEFAULT: &str = "system-default";

/// Renders `text` with the installed voice whose name contains `voice`
/// (any case), or the system default if there is none. Returns the
/// sample rate and the mono samples.
///
/// Blocks until done; call it on a worker thread.
///
/// # Errors
/// Returns [`WinError`] if Windows cannot synthesize speech.
pub fn synthesize_offline(text: &str, voice: &str) -> Result<(u32, Vec<i16>), WinError> {
    // SAFETY: balanced below if it succeeded.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let result = synthesize(text, voice);
    if com.is_ok() {
        // SAFETY: balances the CoInitializeEx above; every COM object was
        // dropped in `synthesize`.
        unsafe { CoUninitialize() };
    }
    let wav = result.map_err(|e| WinError::api("SpeechSynthesizer", &e))?;
    wav::decode_any_rate(&wav).map_err(|_| WinError::invalid("SpeechSynthesizer", "wav"))
}

/// The WAV bytes.
fn synthesize(text: &str, voice: &str) -> windows::core::Result<Vec<u8>> {
    let synthesizer = SpeechSynthesizer::new()?;
    if let Some(voice) = find_voice(voice)? {
        synthesizer.SetVoice(&voice)?;
    }
    let stream = synthesizer
        .SynthesizeTextToStreamAsync(&HSTRING::from(text))?
        .join()?;
    let size = u32::try_from(stream.Size()?).unwrap_or(u32::MAX);
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    let loaded = reader.LoadAsync(size)?.join()?;
    let mut bytes = vec![0; loaded as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

fn find_voice(wanted: &str) -> windows::core::Result<Option<VoiceInformation>> {
    let wanted = wanted.trim().to_lowercase();
    if wanted.is_empty() || wanted == SYSTEM_DEFAULT {
        return Ok(None);
    }
    for voice in SpeechSynthesizer::AllVoices()? {
        let name = voice.DisplayName()?.to_string_lossy().to_lowercase();
        if name.contains(&wanted) {
            return Ok(Some(voice));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints the installed voices and renders a sentence.
    #[test]
    #[ignore = "needs the Windows voices"]
    fn renders_a_sentence() {
        let default = SpeechSynthesizer::DefaultVoice().unwrap();
        println!("default: {}", default.DisplayName().unwrap());
        for voice in SpeechSynthesizer::AllVoices().unwrap() {
            println!(
                "{} {} {:?}",
                voice.DisplayName().unwrap(),
                voice.Language().unwrap(),
                voice.Gender().unwrap()
            );
        }
        for name in ["system-default", "zira", "no such voice"] {
            let started = std::time::Instant::now();
            let (rate, samples) = synthesize_offline("Systems are up.", name).unwrap();
            println!(
                "{name}: {rate} Hz, {} samples in {:?}",
                samples.len(),
                started.elapsed()
            );
            assert!(samples.len() > rate as usize / 4);
            assert!(samples.iter().any(|s| s.unsigned_abs() > 1_000));
        }
    }
}
