//! Playing, pausing and skipping in whatever player is open.
//!
//! The play/pause key only switches, and goes to the player Windows picks:
//! "pause" could start the music. Players that show up in Windows' media
//! controls are told what to do instead, and watched until they did it.

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::{MediaKey, WinError, press_media_key};

/// How long a player gets to react before it is told once more.
const REACT: Duration = Duration::from_millis(1200);
/// How long a player that Windows does not list yet gets.
const APPEAR: Duration = Duration::from_millis(2500);
const POLL: Duration = Duration::from_millis(40);

/// What a player is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCommand {
    /// Play, or go on playing.
    Play,
    /// Pause.
    Pause,
    /// The next track.
    Next,
    /// The previous track.
    Previous,
}

/// What came of a [`MediaCommand`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaResult {
    /// It worked.
    Done,
    /// It was playing, or paused, already.
    Already,
    /// No player is open.
    NoPlayer,
    /// The player did not do it.
    Ignored,
}

/// Tells the open players. Blocks until they did it, a few seconds at most
/// for a player that behaves.
///
/// This loads system DLLs that stay in the process for good, so use it from
/// a helper process.
///
/// # Errors
/// Returns a [`WinError`] if Windows' media controls cannot be used.
pub fn control_media(command: MediaCommand) -> Result<MediaResult, WinError> {
    // SAFETY: balanced below if it succeeded.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let result = control(command).map_err(|e| WinError::api("MediaTransportControls", &e));
    if com.is_ok() {
        // SAFETY: balances the CoInitializeEx above; every COM object was
        // dropped in `control`.
        unsafe { CoUninitialize() };
    }
    result
}

fn control(command: MediaCommand) -> windows::core::Result<MediaResult> {
    let manager = Manager::RequestAsync()?.join()?;
    let sessions: Vec<Session> = manager.GetSessions()?.into_iter().collect();
    // The one Windows shows first, then the others.
    let mut order: Vec<&Session> = sessions.iter().collect();
    if let Ok(current) = manager.GetCurrentSession() {
        let id = current.SourceAppUserModelId()?;
        order.sort_by_key(|s| s.SourceAppUserModelId().ok().as_ref() != Some(&id));
    }
    match command {
        MediaCommand::Play if order.is_empty() => play_unlisted(&manager),
        _ if order.is_empty() => Ok(MediaResult::NoPlayer),
        MediaCommand::Play => Ok(play(&order)),
        MediaCommand::Pause => Ok(pause(&order)),
        MediaCommand::Next | MediaCommand::Previous => Ok(skip(&order, command)),
    }
}

/// Whether it plays. Opening a track counts, its player is on it.
fn playing(session: &Session) -> bool {
    let status = session
        .GetPlaybackInfo()
        .and_then(|info| info.PlaybackStatus());
    matches!(status, Ok(Status::Playing | Status::Changing))
}

fn play(sessions: &[&Session]) -> MediaResult {
    if sessions.iter().any(|s| playing(s)) {
        return MediaResult::Already;
    }
    let played = sessions.iter().any(|session| {
        insist(
            || session.TryPlayAsync().and_then(|asked| asked.join()),
            || playing(session),
        )
    });
    done_if(played)
}

/// Pauses all that play.
fn pause(sessions: &[&Session]) -> MediaResult {
    let playing_now: Vec<_> = sessions.iter().filter(|s| playing(s)).collect();
    if playing_now.is_empty() {
        return MediaResult::Already;
    }
    let mut paused = true;
    for session in playing_now {
        paused &= insist(
            || session.TryPauseAsync().and_then(|asked| asked.join()),
            || !playing(session),
        );
    }
    done_if(paused)
}

/// Skips in the player that plays, or else in the first that can.
fn skip(sessions: &[&Session], command: MediaCommand) -> MediaResult {
    let mut order = sessions.to_vec();
    order.sort_by_key(|s| !playing(s));
    let skipped = order.iter().any(|session| {
        let asked = if command == MediaCommand::Next {
            session.TrySkipNextAsync()
        } else {
            session.TrySkipPreviousAsync()
        };
        asked.and_then(|asked| asked.join()).unwrap_or(false)
    });
    done_if(skipped)
}

/// Some players (Spotify just after it was opened) are listed only once
/// they have played something. Nothing is listed, so nothing plays that
/// the key could pause.
fn play_unlisted(manager: &Manager) -> windows::core::Result<MediaResult> {
    if press_media_key(MediaKey::PlayPause).is_err() {
        return Ok(MediaResult::NoPlayer);
    }
    let deadline = Instant::now() + APPEAR;
    while Instant::now() < deadline {
        if manager.GetSessions()?.into_iter().any(|s| playing(&s)) {
            return Ok(MediaResult::Done);
        }
        sleep(POLL);
    }
    Ok(MediaResult::NoPlayer)
}

/// Asks up to twice, and waits for `happened` each time.
fn insist(ask: impl Fn() -> windows::core::Result<bool>, happened: impl Fn() -> bool) -> bool {
    for _ in 0..2 {
        if !ask().unwrap_or(false) {
            return false;
        }
        let deadline = Instant::now() + REACT;
        while Instant::now() < deadline {
            if happened() {
                return true;
            }
            sleep(POLL);
        }
    }
    false
}

fn done_if(worked: bool) -> MediaResult {
    if worked {
        MediaResult::Done
    } else {
        MediaResult::Ignored
    }
}
