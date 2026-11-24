//! Speech playback (spec §5.5): WASAPI shared mode, event-driven, at the
//! speech's own sample rate, with Windows converting to the device format
//! (`AUTOCONVERTPCM`).
//!
//! [`Playback::open`] starts a thread that opens the default output device
//! and plays a [`PlaybackReader`]. It lives only while Sohi speaks: it
//! ends by itself once the writer is gone and the last sample was heard,
//! or within one wait when stopped. After setup its loop does not
//! allocate, lock or log.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use sohi_audio::playback::PlaybackReader;
use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{IAudioClient, IAudioRenderClient, eRender};
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects};

use crate::audio::{event, initialize, open_endpoint, pro_audio};
use crate::{OwnedHandle, WinError};

/// WASAPI's buffer: 60 ms, in 100 ns units. It also bounds how far the
/// measured level runs ahead of the sound.
const BUFFER_HNS: i64 = 600_000;
/// A device that asks for nothing for this long has stalled or gone.
const STALL_MS: u32 = 2_000;
/// "Nothing played yet" in [`Playback::first_audio_micros`].
const NONE_YET: u64 = u64::MAX;

/// A running render thread. Dropping it stops the sound.
pub struct Playback {
    stop: Arc<OwnedHandle>,
    thread: Option<JoinHandle<Result<(), WinError>>>,
    shared: Arc<Shared>,
}

/// What the render thread tells the outside.
struct Shared {
    /// See [`Playback::first_audio_micros`].
    first_audio: AtomicU64,
    /// Nothing more will be heard.
    silent: AtomicBool,
    started: Instant,
}

impl Shared {
    /// Notes the first speech handed to the device.
    fn note_audio(&self) {
        if self.first_audio.load(Ordering::Relaxed) == NONE_YET {
            let micros = u64::try_from(self.started.elapsed().as_micros()).unwrap_or(NONE_YET - 1);
            self.first_audio.store(micros, Ordering::Relaxed);
        }
    }
}

impl Playback {
    /// Starts playing `reader`. Opening the device happens on the new
    /// thread, so this returns at once; a failure there ends the thread,
    /// which the writer sees as a closed ring, and [`Self::finish`]
    /// returns the error.
    ///
    /// `started` is the moment [`Self::first_audio_micros`] counts from.
    ///
    /// # Errors
    /// Returns [`WinError`] if the stop event or the thread cannot be
    /// created.
    pub fn open(reader: PlaybackReader, started: Instant) -> Result<Self, WinError> {
        let stop = Arc::new(event()?);
        let shared = Arc::new(Shared {
            first_audio: AtomicU64::new(NONE_YET),
            silent: AtomicBool::new(false),
            started,
        });
        let thread = {
            let stop = Arc::clone(&stop);
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("sohi-render".to_owned())
                .spawn(move || pro_audio(|| session(reader, &stop, &shared)))
                .map_err(|_| WinError::invalid("CreateThread", "render thread"))?
        };
        Ok(Self {
            stop,
            thread: Some(thread),
            shared,
        })
    }

    /// Microseconds from `started` until speech was first handed to the
    /// device, once it was.
    #[must_use]
    pub fn first_audio_micros(&self) -> Option<u64> {
        let value = self.shared.first_audio.load(Ordering::Relaxed);
        (value != NONE_YET).then_some(value)
    }

    /// Whether the sound is over: all was heard, it was stopped, or it
    /// failed. The thread may still be closing the device (about 80 ms).
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.shared.silent.load(Ordering::Acquire)
            || self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Silences the device without waiting for the thread; the writer
    /// sees the ring close.
    pub fn stop(&self) {
        self.signal_stop();
    }

    /// Stops the sound and waits for the thread to close the device
    /// (about 100 ms).
    ///
    /// # Errors
    /// Returns the error that ended the thread, if any.
    pub fn finish(mut self) -> Result<(), WinError> {
        self.signal_stop();
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(result)) => result,
            Some(Err(_)) => Err(WinError::invalid("render thread", "panic")),
            None => Ok(()),
        }
    }

    fn signal_stop(&self) {
        // SAFETY: the event lives as long as `self.stop`.
        let _ = unsafe { SetEvent(self.stop.raw()) };
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        // The thread exits on its own within one wait; it is not joined
        // here so a drop never waits.
        self.signal_stop();
    }
}

/// Opens the default output device and runs the render loop.
fn session(
    mut reader: PlaybackReader,
    stop: &OwnedHandle,
    shared: &Shared,
) -> Result<(), WinError> {
    if reader.is_ended() {
        return Ok(());
    }
    let client = open_endpoint(eRender)?;
    initialize(&client, reader.sample_rate(), BUFFER_HNS)?;
    let ready = event()?;
    // SAFETY: the client is initialized for event callbacks; the event
    // outlives the client's use of it (both live in this session).
    unsafe { client.SetEventHandle(ready.raw()) }
        .map_err(|e| WinError::api("IAudioClient::SetEventHandle", &e))?;
    // SAFETY: the client is initialized.
    let service: IAudioRenderClient = unsafe { client.GetService() }
        .map_err(|e| WinError::api("IAudioClient::GetService", &e))?;
    // SAFETY: as above.
    let capacity = unsafe { client.GetBufferSize() }
        .map_err(|e| WinError::api("IAudioClient::GetBufferSize", &e))?;
    let mut output = Output {
        client,
        render: service,
        capacity,
    };
    output.fill(&mut reader, shared)?;
    // SAFETY: the client is initialized and has its first buffer.
    unsafe { output.client.Start() }.map_err(|e| WinError::api("IAudioClient::Start", &e))?;
    let result = render_loop(&mut output, &mut reader, stop, &ready, shared);
    // SAFETY: stopping a started client; silences it at once.
    let _ = unsafe { output.client.Stop() };
    // Say so before closing the device, which takes a while.
    shared.silent.store(true, Ordering::Release);
    drop(reader);
    result
}

/// An open output device.
struct Output {
    client: IAudioClient,
    render: IAudioRenderClient,
    /// The device buffer, in frames.
    capacity: u32,
}

impl Output {
    /// Frames handed over but not played yet.
    fn padding(&self) -> Result<u32, WinError> {
        // SAFETY: the client is initialized.
        unsafe { self.client.GetCurrentPadding() }
            .map_err(|e| WinError::api("IAudioClient::GetCurrentPadding", &e))
    }

    /// Tops the device buffer up with speech, then silence.
    fn fill(&mut self, reader: &mut PlaybackReader, shared: &Shared) -> Result<(), WinError> {
        let frames = self.capacity.saturating_sub(self.padding()?);
        if frames == 0 {
            return Ok(());
        }
        // SAFETY: at most the free part of the buffer is asked for.
        let data = unsafe { self.render.GetBuffer(frames) }
            .map_err(|e| WinError::api("IAudioRenderClient::GetBuffer", &e))?;
        #[allow(
            clippy::cast_ptr_alignment,
            reason = "WASAPI buffers are aligned to the format's block (2 bytes)"
        )]
        let data = data.cast::<i16>();
        // SAFETY: WASAPI hands room for `frames` frames of the format we
        // asked for (one i16 each), aligned and ours until ReleaseBuffer;
        // any bit pattern is a valid i16.
        let out = unsafe { std::slice::from_raw_parts_mut(data, frames as usize) };
        let speech = reader.read(out);
        // SAFETY: releases exactly the frames GetBuffer returned.
        unsafe { self.render.ReleaseBuffer(frames, 0) }
            .map_err(|e| WinError::api("IAudioRenderClient::ReleaseBuffer", &e))?;
        if speech > 0 {
            shared.note_audio();
        }
        Ok(())
    }
}

/// Plays until stopped or everything was heard. Allocation-free.
fn render_loop(
    output: &mut Output,
    reader: &mut PlaybackReader,
    stop: &OwnedHandle,
    ready: &OwnedHandle,
    shared: &Shared,
) -> Result<(), WinError> {
    let events = [stop.raw(), ready.raw()];
    #[cfg(test)]
    let _tracking = crate::capture::alloc_check::Tracking::start();
    loop {
        // SAFETY: both handles live for the whole loop.
        let woke = unsafe { WaitForMultipleObjects(&events, false, STALL_MS) };
        if woke == WAIT_OBJECT_0 {
            return Ok(());
        }
        if woke == WAIT_TIMEOUT {
            return Err(WinError::invalid("render", "device took no audio for 2 s"));
        }
        if !reader.is_ended() {
            output.fill(reader, shared)?;
        } else if output.padding()? == 0 {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use sohi_audio::playback::playback_ring;

    use super::*;
    use crate::capture::alloc_check;

    /// Plays 500 ms of digital silence on the real output device.
    /// Run with `cargo test -p sohi-win -- --ignored render`.
    #[test]
    #[ignore = "uses the real output device"]
    fn plays_to_the_end_without_allocating() {
        for rate in [16_000, 24_000] {
            let (mut writer, reader) = playback_ring(rate);
            let half = vec![0; rate as usize / 2];
            assert_eq!(writer.push(&half), half.len());
            let started = Instant::now();
            let playback = Playback::open(reader, started).unwrap();
            drop(writer);
            while !playback.is_finished() {
                assert!(started.elapsed() < Duration::from_secs(3), "never ended");
                std::thread::sleep(Duration::from_millis(1));
            }
            let total = started.elapsed();
            let first = playback.first_audio_micros().unwrap();
            playback.finish().unwrap();
            println!("{rate} Hz: first audio after {first} us, all heard after {total:?}");
            assert!(total >= Duration::from_millis(480), "{total:?}");
            assert_eq!(alloc_check::IN_LOOP.load(Ordering::Relaxed), 0);
        }
    }

    /// Spec §5.5: a cancelled turn stops the sound within 30 ms.
    #[test]
    #[ignore = "uses the real output device"]
    fn stops_within_30_ms() {
        let (mut writer, reader) = playback_ring(24_000);
        writer.push(&vec![0; 24_000]);
        let playback = Playback::open(reader, Instant::now()).unwrap();
        while playback.first_audio_micros().is_none() {
            assert!(!playback.is_finished(), "playback failed to start");
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(100));
        let stopping = Instant::now();
        playback.stop();
        while !playback.is_finished() {
            std::thread::yield_now();
        }
        let took = stopping.elapsed();
        println!("stopped after {took:?}");
        assert!(took < Duration::from_millis(30), "{took:?}");
        playback.finish().unwrap();
        assert!(writer.is_closed());
    }

    #[test]
    fn nothing_to_play_ends_without_opening_a_device() {
        let (writer, reader) = playback_ring(24_000);
        drop(writer);
        let playback = Playback::open(reader, Instant::now()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !playback.is_finished() {
            assert!(Instant::now() < deadline, "render thread still running");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
