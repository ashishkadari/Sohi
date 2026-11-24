//! Audio processing: VAD segmentation, pre-roll, WAV encoding and level metering.

#![forbid(unsafe_code)]

pub mod playback;
pub mod ring;
pub mod vad;
pub mod wav;
