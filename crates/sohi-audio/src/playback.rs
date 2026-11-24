//! The playback ring: the runtime thread writes synthesized speech, the
//! render thread plays it (spec §5.5, §9.3).
//!
//! A lock-free single-producer single-consumer ring of mono samples.
//! Dropping the writer ends the speech once the ring has run dry;
//! dropping the reader (the render thread stopped) closes it at once.
//! The reader also measures what it plays, for the visor.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

/// How much speech the ring holds, in milliseconds.
const CAPACITY_MS: u32 = 1_000;
/// Spec §5.5: the level is the RMS of each 20 ms block.
const BLOCK_MS: u32 = 20;

/// Connects a writer and a reader for speech at `sample_rate`.
#[must_use]
pub fn playback_ring(sample_rate: u32) -> (PlaybackWriter, PlaybackReader) {
    let (producer, consumer) = RingBuffer::new(samples(sample_rate, CAPACITY_MS));
    let level = Arc::new(AtomicU32::new(0));
    (
        PlaybackWriter {
            producer,
            level: Arc::clone(&level),
        },
        PlaybackReader {
            consumer,
            sample_rate,
            block: samples(sample_rate, BLOCK_MS),
            counted: 0,
            energy: 0.0,
            level,
        },
    )
}

/// The number of samples in `ms` milliseconds.
fn samples(sample_rate: u32, ms: u32) -> usize {
    usize::try_from(u64::from(sample_rate) * u64::from(ms) / 1_000).unwrap_or(usize::MAX)
}

/// The runtime thread's end.
pub struct PlaybackWriter {
    producer: Producer<i16>,
    level: Arc<AtomicU32>,
}

impl PlaybackWriter {
    /// Writes as many samples as fit and returns how many that was.
    pub fn push(&mut self, samples: &[i16]) -> usize {
        let (written, _) = self.producer.push_partial_slice(samples);
        written.len()
    }

    /// Whether samples are still waiting to be played.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.producer.slots() < self.producer.buffer().capacity()
    }

    /// Whether the render thread is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.producer.is_abandoned()
    }

    /// A handle that reads the level even after the writer is dropped.
    #[must_use]
    pub fn meter(&self) -> PlaybackMeter {
        PlaybackMeter(Arc::clone(&self.level))
    }
}

/// Reads the level of what is being played.
#[derive(Debug, Clone)]
pub struct PlaybackMeter(Arc<AtomicU32>);

impl PlaybackMeter {
    /// The RMS of the latest 20 ms block, from 0 to 1.
    #[must_use]
    pub fn rms(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// The render thread's end. Nothing here allocates, locks or blocks.
pub struct PlaybackReader {
    consumer: Consumer<i16>,
    sample_rate: u32,
    block: usize,
    counted: usize,
    energy: f32,
    level: Arc<AtomicU32>,
}

impl PlaybackReader {
    /// Samples per second.
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Fills `out` with speech, then silence, and returns how many samples
    /// were speech.
    pub fn read(&mut self, out: &mut [i16]) -> usize {
        let (speech, silence) = self.consumer.pop_partial_slice(out);
        let count = speech.len();
        silence.fill(0);
        self.measure(out);
        count
    }

    /// Whether the writer is gone and everything was read.
    #[must_use]
    pub fn is_ended(&self) -> bool {
        // In this order: whatever was written before the writer went is
        // visible once it is seen gone.
        self.consumer.is_abandoned() && self.consumer.is_empty()
    }

    /// Adds what is about to play to the running block.
    fn measure(&mut self, played: &[i16]) {
        for &sample in played {
            let value = f32::from(sample) / 32_768.0;
            self.energy += value * value;
            self.counted += 1;
            if self.counted >= self.block {
                #[allow(clippy::cast_precision_loss, reason = "a block is under 1000 samples")]
                let rms = (self.energy / self.counted as f32).sqrt();
                self.level.store(rms.to_bits(), Ordering::Relaxed);
                self.counted = 0;
                self.energy = 0.0;
            }
        }
    }
}

impl Drop for PlaybackReader {
    fn drop(&mut self) {
        self.level.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_was_written_then_silence() {
        let (mut writer, mut reader) = playback_ring(24_000);
        assert_eq!(writer.push(&[1, 2, 3]), 3);
        let mut out = [9; 5];
        assert_eq!(reader.read(&mut out), 3);
        assert_eq!(out, [1, 2, 3, 0, 0]);
        assert_eq!(reader.read(&mut out), 0);
        assert_eq!(out, [0; 5]);
    }

    #[test]
    fn knows_whether_samples_wait() {
        let (mut writer, mut reader) = playback_ring(16_000);
        assert!(!writer.is_pending());
        writer.push(&[1; 10]);
        assert!(writer.is_pending());
        reader.read(&mut [0; 10]);
        assert!(!writer.is_pending());
    }

    #[test]
    fn a_full_ring_takes_only_what_fits() {
        let (mut writer, mut reader) = playback_ring(16_000);
        assert_eq!(writer.push(&vec![1; 20_000]), 16_000);
        assert_eq!(writer.push(&[1]), 0);
        reader.read(&mut [0; 100]);
        assert_eq!(writer.push(&[1; 200]), 100);
    }

    #[test]
    fn ends_once_the_writer_is_gone_and_the_ring_is_dry() {
        let (mut writer, mut reader) = playback_ring(24_000);
        writer.push(&[5; 10]);
        assert!(!reader.is_ended());
        drop(writer);
        assert!(!reader.is_ended());
        reader.read(&mut [0; 10]);
        assert!(reader.is_ended());
    }

    #[test]
    fn closing_the_reader_is_seen_by_the_writer() {
        let (writer, reader) = playback_ring(24_000);
        assert!(!writer.is_closed());
        drop(reader);
        assert!(writer.is_closed());
    }

    #[test]
    fn measures_each_block() {
        let (mut writer, mut reader) = playback_ring(24_000);
        let meter = writer.meter();
        writer.push(&[16_384; 480]);
        let mut out = [0; 479];
        reader.read(&mut out);
        assert!(meter.rms().abs() < 1e-6, "no block is complete yet");
        reader.read(&mut out[..1]);
        assert!((meter.rms() - 0.5).abs() < 1e-3);
        reader.read(&mut vec![0; 480]);
        assert!(meter.rms().abs() < 1e-6, "silence");
        writer.push(&[16_384; 480]);
        reader.read(&mut vec![0; 480]);
        drop(reader);
        assert!(meter.rms().abs() < 1e-6, "nothing plays any more");
    }
}
