//! In-memory WAV (spec §5.4): a 44-byte RIFF header plus 16-bit PCM,
//! about 32 KB per second at 16 kHz mono. Also a small reader for the
//! same layout, for test fixtures and `dev-inject`.

use crate::ring::SAMPLE_RATE;

/// Size of the canonical header [`encode`] writes.
pub const HEADER_LEN: usize = 44;

/// Encodes 16 kHz mono samples as a WAV file.
#[must_use]
pub fn encode(samples: &[i16]) -> Vec<u8> {
    encode_with_rate(samples, SAMPLE_RATE)
}

/// Encodes mono samples at `rate` as a WAV file.
#[must_use]
pub fn encode_with_rate(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len() * 2).unwrap_or(u32::MAX - 36);
    let mut out = Vec::with_capacity(HEADER_LEN + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

/// Why a WAV file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WavError {
    /// Not a RIFF/WAVE file, or cut short.
    #[error("not a WAV file")]
    NotWav,
    /// A WAV file in some other format.
    #[error(
        "need 16 kHz mono 16-bit PCM, got {channels} ch, {rate} Hz, {bits}-bit (format {format})"
    )]
    Unsupported {
        /// Format tag (1 = PCM).
        format: u16,
        /// Channel count.
        channels: u16,
        /// Sample rate.
        rate: u32,
        /// Bits per sample.
        bits: u16,
    },
}

/// Reads a 16 kHz mono 16-bit PCM WAV file. Chunks other than `fmt ` and
/// `data` are skipped.
///
/// # Errors
/// Returns [`WavError`] for anything else.
pub fn decode(bytes: &[u8]) -> Result<Vec<i16>, WavError> {
    let (rate, samples) = decode_any_rate(bytes)?;
    if rate == SAMPLE_RATE {
        Ok(samples)
    } else {
        Err(WavError::Unsupported {
            format: 1,
            channels: 1,
            rate,
            bits: 16,
        })
    }
}

/// Reads a mono 16-bit PCM WAV of any sample rate; returns the rate and
/// the samples.
///
/// # Errors
/// Returns [`WavError`] if it is not a WAV or in another format.
pub fn decode_any_rate(bytes: &[u8]) -> Result<(u32, Vec<i16>), WavError> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWav);
    }
    let mut rest = &bytes[12..];
    let mut format = None;
    while rest.len() >= 8 {
        let id = &rest[..4];
        let len = u32::from_le_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
        let body = rest.get(8..8 + len).ok_or(WavError::NotWav)?;
        match id {
            b"fmt " => format = Some(parse_format(body)?),
            b"data" => {
                let (format, channels, rate, bits) = format.ok_or(WavError::NotWav)?;
                if (format, channels, bits) != (1, 1, 16) {
                    return Err(WavError::Unsupported {
                        format,
                        channels,
                        rate,
                        bits,
                    });
                }
                let (pairs, _) = body.as_chunks::<2>();
                let samples = pairs.iter().map(|&b| i16::from_le_bytes(b)).collect();
                return Ok((rate, samples));
            }
            _ => {}
        }
        // Chunks are padded to an even length.
        rest = rest.get(8 + len + (len & 1)..).unwrap_or(&[]);
    }
    Err(WavError::NotWav)
}

fn parse_format(body: &[u8]) -> Result<(u16, u16, u32, u16), WavError> {
    if body.len() < 16 {
        return Err(WavError::NotWav);
    }
    let u16_at = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
    let rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
    Ok((u16_at(0), u16_at(2), rate, u16_at(14)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_canonical_header() {
        let wav = encode(&[1, -2, 32_767]);
        assert_eq!(wav.len(), HEADER_LEN + 6);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 6);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(&wav[44..], [1, 0, 0xFE, 0xFF, 0xFF, 0x7F]);
    }

    #[test]
    fn round_trips() {
        let samples: Vec<i16> = (0..1000i16)
            .map(|i| i.wrapping_mul(37) % 2000 - 1000)
            .collect();
        assert_eq!(decode(&encode(&samples)).unwrap(), samples);
    }

    #[test]
    fn skips_unknown_chunks() {
        let wav = encode(&[5, 6]);
        let mut odd = wav[..36].to_vec();
        odd[4..8].copy_from_slice(&(36u32 + 6 + 8 + 3 + 1).to_le_bytes());
        odd.extend_from_slice(b"LIST");
        odd.extend_from_slice(&3u32.to_le_bytes());
        odd.extend_from_slice(&[9, 9, 9, 0]); // padded to even
        odd.extend_from_slice(&wav[36..]);
        assert_eq!(decode(&odd).unwrap(), [5, 6]);
    }

    #[test]
    fn rejects_other_formats() {
        assert_eq!(decode(b"nope"), Err(WavError::NotWav));
        let stereo = encode_with_rate(&[0, 0], 44_100);
        assert!(matches!(
            decode(&stereo),
            Err(WavError::Unsupported { rate: 44_100, .. })
        ));
    }
}
