//! The capture ring: the capture thread writes 16 kHz mono samples, the
//! runtime thread reads them (spec §5.4, §9.3).
//!
//! It is a lock-free single-producer single-consumer ring with a fixed
//! capacity of 2 s. The writer never allocates, locks or blocks: whatever
//! does not fit is dropped and counted, and the reader logs the count.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};

/// Capture sample rate.
pub const SAMPLE_RATE: u32 = 16_000;
/// Ring capacity in samples (2 s).
pub const RING_SAMPLES: usize = SAMPLE_RATE as usize * 2;

/// A new ring, split into its two ends.
#[must_use]
pub fn capture_ring() -> (CaptureWriter, CaptureReader) {
    let (producer, consumer) = RingBuffer::new(RING_SAMPLES);
    let dropped = Arc::new(AtomicU64::new(0));
    (
        CaptureWriter {
            producer,
            dropped: Arc::clone(&dropped),
        },
        CaptureReader { consumer, dropped },
    )
}

/// The capture thread's end.
pub struct CaptureWriter {
    producer: Producer<i16>,
    dropped: Arc<AtomicU64>,
}

impl CaptureWriter {
    /// Writes `samples`. Samples that do not fit are dropped and counted.
    /// Never allocates, locks or blocks.
    pub fn push(&mut self, samples: &[i16]) {
        let (_, rest) = self.producer.push_partial_slice(samples);
        self.count_dropped(rest.len());
    }

    /// Writes `count` zero samples (WASAPI's "silent" packets).
    pub fn push_silence(&mut self, count: usize) {
        let fits = count.min(self.producer.slots());
        if let Ok(chunk) = self.producer.write_chunk_uninit(fits) {
            chunk.fill_from_iter(std::iter::repeat(0));
        }
        self.count_dropped(count - fits);
    }

    /// Whether the reader is gone (the turn ended).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.producer.is_abandoned()
    }

    fn count_dropped(&self, count: usize) {
        if count > 0 {
            self.dropped.fetch_add(count as u64, Ordering::Relaxed);
        }
    }
}

/// The runtime thread's end.
pub struct CaptureReader {
    consumer: Consumer<i16>,
    dropped: Arc<AtomicU64>,
}

impl CaptureReader {
    /// Samples waiting to be read.
    #[must_use]
    pub fn available(&self) -> usize {
        self.consumer.slots()
    }

    /// Reads up to `out.len()` samples. Returns how many were read.
    pub fn read(&mut self, out: &mut [i16]) -> usize {
        let (filled, _) = self.consumer.pop_partial_slice(out);
        filled.len()
    }

    /// Samples dropped since the last call because the ring was full.
    pub fn take_dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }

    /// Whether the writer is gone (the capture thread stopped).
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.consumer.is_abandoned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_come_out_in_order() {
        let (mut writer, mut reader) = capture_ring();
        writer.push(&[1, 2, 3]);
        writer.push_silence(2);
        writer.push(&[4]);
        assert_eq!(reader.available(), 6);
        let mut out = [9; 4];
        assert_eq!(reader.read(&mut out), 4);
        assert_eq!(out, [1, 2, 3, 0]);
        let mut out = [9; 8];
        assert_eq!(reader.read(&mut out), 2);
        assert_eq!(out[..2], [0, 4]);
        assert_eq!(reader.take_dropped(), 0);
    }

    #[test]
    fn overflow_drops_and_counts() {
        let (mut writer, mut reader) = capture_ring();
        let block = [7i16; 160];
        for _ in 0..(RING_SAMPLES / 160) {
            writer.push(&block);
        }
        writer.push(&block);
        writer.push_silence(40);
        assert_eq!(reader.available(), RING_SAMPLES);
        assert_eq!(reader.take_dropped(), 200);
        assert_eq!(reader.take_dropped(), 0, "the count resets");
        let mut out = vec![0i16; 160];
        reader.read(&mut out);
        writer.push(&block);
        assert_eq!(reader.take_dropped(), 0, "room again after a read");
    }

    #[test]
    fn each_end_sees_the_other_close() {
        let (writer, reader) = capture_ring();
        drop(reader);
        assert!(writer.is_closed());
        let (writer, reader) = capture_ring();
        drop(writer);
        assert!(reader.is_closed());
    }
}
