//! Microphone capture (spec §5.4): WASAPI shared mode, event-driven, with
//! Windows converting the device format to 16 kHz mono i16
//! (`AUTOCONVERTPCM`), so Sohi needs no resampler.
//!
//! [`Capture::open`] starts a thread that opens the default capture
//! device and writes every packet into a [`CaptureWriter`]. The thread is
//! registered with MMCSS "Pro Audio". After setup its loop does not
//! allocate, lock or log: it waits on two events, copies samples into the
//! lock-free ring and releases the buffer. Dropping the [`Capture`] (or
//! the ring's reader) stops it and releases the microphone, which also
//! turns off Windows' microphone indicator.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use sohi_audio::ring::{CaptureWriter, SAMPLE_RATE};
use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, IAudioCaptureClient, IAudioClient, eCapture,
};
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects};

use crate::audio::{event, initialize, open_endpoint, pro_audio};
use crate::{OwnedHandle, WinError};

/// WASAPI's buffer: 100 ms, in 100 ns units. Packets arrive every device
/// period (about 10 ms); the slack covers a late wake-up.
const BUFFER_HNS: i64 = 1_000_000;
/// A device that sends nothing for this long has stalled or gone.
const STALL_MS: u32 = 2_000;
/// "No packet yet" in [`Capture::first_packet_micros`].
const NONE_YET: u64 = u64::MAX;

/// A running capture thread. Dropping it stops the thread.
pub struct Capture {
    stop: Arc<OwnedHandle>,
    thread: Option<JoinHandle<Result<(), WinError>>>,
    first_packet: Arc<AtomicU64>,
}

impl Capture {
    /// Starts capturing into `writer`. Opening the device happens on the
    /// new thread, so this returns at once; a failure there ends the
    /// thread, which the reader sees as a closed ring, and [`Self::finish`]
    /// returns the error.
    ///
    /// `started` is the moment the user asked (for
    /// [`Self::first_packet_micros`]).
    ///
    /// # Errors
    /// Returns [`WinError`] if the stop event or the thread cannot be
    /// created.
    pub fn open(writer: CaptureWriter, started: Instant) -> Result<Self, WinError> {
        let stop = Arc::new(event()?);
        let first_packet = Arc::new(AtomicU64::new(NONE_YET));
        let thread = {
            let stop = Arc::clone(&stop);
            let first_packet = Arc::clone(&first_packet);
            std::thread::Builder::new()
                .name("sohi-capture".to_owned())
                .spawn(move || pro_audio(|| session(writer, &stop, &first_packet, started)))
                .map_err(|_| WinError::invalid("CreateThread", "capture thread"))?
        };
        Ok(Self {
            stop,
            thread: Some(thread),
            first_packet,
        })
    }

    /// Microseconds from `started` to the first packet, once one arrived.
    #[must_use]
    pub fn first_packet_micros(&self) -> Option<u64> {
        let value = self.first_packet.load(Ordering::Relaxed);
        (value != NONE_YET).then_some(value)
    }

    /// Whether the thread has ended (stopped, or failed).
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Stops the thread and waits for it: the microphone is released when
    /// this returns. Takes at most a few milliseconds.
    ///
    /// # Errors
    /// Returns the error that ended the thread, if any.
    pub fn finish(mut self) -> Result<(), WinError> {
        self.signal_stop();
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            Some(Err(_)) => Err(WinError::invalid("capture thread", "panic")),
            None => Ok(()),
        }
    }

    fn signal_stop(&self) {
        // SAFETY: the event lives as long as `self.stop`.
        let _ = unsafe { SetEvent(self.stop.raw()) };
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // The thread exits on its own within one wait; it is not joined
        // here so a drop on the UI thread never waits.
        self.signal_stop();
    }
}

/// Opens the default capture device and runs the capture loop.
fn session(
    mut writer: CaptureWriter,
    stop: &OwnedHandle,
    first_packet: &AtomicU64,
    started: Instant,
) -> Result<(), WinError> {
    if writer.is_closed() {
        // Nobody is listening any more: do not open the microphone.
        return Ok(());
    }
    let (client, capture, ready) = open_device()?;
    // SAFETY: the client was initialized in `open_device`.
    unsafe { client.Start() }.map_err(|e| WinError::api("IAudioClient::Start", &e))?;
    let result = capture_loop(&capture, &mut writer, stop, &ready, first_packet, started);
    // SAFETY: as above; stopping a started client.
    let _ = unsafe { client.Stop() };
    result
}

/// The default capture endpoint, initialized for 16 kHz mono i16.
fn open_device() -> Result<(IAudioClient, IAudioCaptureClient, OwnedHandle), WinError> {
    let client = open_endpoint(eCapture)?;
    initialize(&client, SAMPLE_RATE, BUFFER_HNS)?;
    let ready = event()?;
    // SAFETY: the client is initialized for event callbacks; the event
    // outlives the client's use of it (both live in this session).
    unsafe { client.SetEventHandle(ready.raw()) }
        .map_err(|e| WinError::api("IAudioClient::SetEventHandle", &e))?;
    // SAFETY: as above.
    let capture: IAudioCaptureClient = unsafe { client.GetService() }
        .map_err(|e| WinError::api("IAudioClient::GetService", &e))?;
    Ok((client, capture, ready))
}

/// Copies packets into the ring until stopped. Allocation-free.
fn capture_loop(
    capture: &IAudioCaptureClient,
    writer: &mut CaptureWriter,
    stop: &OwnedHandle,
    ready: &OwnedHandle,
    first_packet: &AtomicU64,
    started: Instant,
) -> Result<(), WinError> {
    let events = [stop.raw(), ready.raw()];
    #[cfg(test)]
    let _tracking = alloc_check::Tracking::start();
    loop {
        // SAFETY: both handles live for the whole loop.
        let woke = unsafe { WaitForMultipleObjects(&events, false, STALL_MS) };
        if woke == WAIT_OBJECT_0 || writer.is_closed() {
            return Ok(());
        }
        if woke == WAIT_TIMEOUT {
            return Err(WinError::invalid("capture", "device sent no audio for 2 s"));
        }
        drain_packets(capture, writer, first_packet, started)?;
    }
}

/// Moves every waiting packet into the ring.
fn drain_packets(
    capture: &IAudioCaptureClient,
    writer: &mut CaptureWriter,
    first_packet: &AtomicU64,
    started: Instant,
) -> Result<(), WinError> {
    loop {
        // SAFETY: the capture client is started.
        let size = unsafe { capture.GetNextPacketSize() }
            .map_err(|e| WinError::api("GetNextPacketSize", &e))?;
        if size == 0 {
            return Ok(());
        }
        let mut data = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        // SAFETY: valid out-pointers; the buffer is released below before
        // the next GetBuffer.
        unsafe { capture.GetBuffer(&raw mut data, &raw mut frames, &raw mut flags, None, None) }
            .map_err(|e| WinError::api("IAudioCaptureClient::GetBuffer", &e))?;
        let count = frames as usize;
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0.cast_unsigned() != 0 || data.is_null() {
            writer.push_silence(count);
        } else {
            #[allow(
                clippy::cast_ptr_alignment,
                reason = "WASAPI buffers are aligned to the format's block (2 bytes)"
            )]
            let data = data.cast::<i16>();
            // SAFETY: WASAPI hands `frames` frames of the format we asked
            // for (one i16 each), aligned and valid until ReleaseBuffer.
            let samples = unsafe { std::slice::from_raw_parts(data, count) };
            writer.push(samples);
        }
        // SAFETY: releases exactly the frames GetBuffer returned.
        unsafe { capture.ReleaseBuffer(frames) }
            .map_err(|e| WinError::api("IAudioCaptureClient::ReleaseBuffer", &e))?;
        if first_packet.load(Ordering::Relaxed) == NONE_YET {
            let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(NONE_YET - 1);
            first_packet.store(micros, Ordering::Relaxed);
        }
    }
}

/// Counts heap allocations made by the capture loop (spec §8 P4: the
/// capture thread performs zero allocations).
#[cfg(test)]
pub(crate) mod alloc_check {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    thread_local! {
        static TRACKING: Cell<bool> = const { Cell::new(false) };
    }
    /// Allocations made while a loop was tracked.
    pub static IN_LOOP: AtomicUsize = AtomicUsize::new(0);

    /// Tracks allocations on this thread until dropped.
    pub struct Tracking;

    impl Tracking {
        pub fn start() -> Self {
            TRACKING.with(|t| t.set(true));
            Self
        }
    }

    impl Drop for Tracking {
        fn drop(&mut self) {
            TRACKING.with(|t| t.set(false));
        }
    }

    pub struct Counting;

    // SAFETY: forwards to the system allocator unchanged.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if TRACKING.with(Cell::get) {
                IN_LOOP.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: same contract as our caller's.
            unsafe { System.alloc(layout) }
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            // SAFETY: same contract as our caller's.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: Counting = Counting;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sohi_audio::ring::capture_ring;

    use super::*;

    /// Opens the real microphone for 1 s (the privacy indicator shows).
    /// Run with `cargo test -p sohi-win -- --ignored capture`.
    #[test]
    #[ignore = "uses the real microphone"]
    fn captures_16k_without_allocating() {
        let (writer, mut reader) = capture_ring();
        let started = Instant::now();
        let capture = Capture::open(writer, started).unwrap();
        let mut buffer = vec![0i16; 4096];
        while capture.first_packet_micros().is_none() {
            assert!(!capture.is_finished(), "capture failed to start");
            std::thread::sleep(Duration::from_millis(1));
        }
        // Count one second of samples from the first packet on.
        let from = Instant::now();
        let mut got = reader.read(&mut buffer);
        while from.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(30));
            got += reader.read(&mut buffer);
            assert!(!capture.is_finished(), "capture stopped early");
        }
        let first = capture.first_packet_micros().unwrap();
        capture.finish().unwrap();
        println!(
            "first packet after {} ms; {got} samples in 1 s",
            first / 1000
        );
        assert!((15_000..=17_600).contains(&got), "{got} samples");
        assert_eq!(alloc_check::IN_LOOP.load(Ordering::Relaxed), 0);
    }

    /// Prints how long each open takes, cold and warm.
    #[test]
    #[ignore = "uses the real microphone"]
    fn open_latency() {
        for round in 0..3 {
            let (writer, mut reader) = capture_ring();
            let started = Instant::now();
            let capture = Capture::open(writer, started).unwrap();
            while capture.first_packet_micros().is_none() {
                assert!(!capture.is_finished(), "capture failed");
                std::thread::sleep(Duration::from_millis(1));
            }
            let first = capture.first_packet_micros().unwrap();
            let mut buffer = vec![0i16; 4096];
            reader.read(&mut buffer);
            capture.finish().unwrap();
            println!("round {round}: first packet after {first} us");
        }
    }

    #[test]
    fn dropping_the_reader_stops_the_thread() {
        let (writer, reader) = capture_ring();
        drop(reader);
        // The thread sees the closed ring before opening the device, so
        // this never touches the microphone.
        let capture = Capture::open(writer, Instant::now()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !capture.is_finished() {
            assert!(Instant::now() < deadline, "capture thread still running");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
