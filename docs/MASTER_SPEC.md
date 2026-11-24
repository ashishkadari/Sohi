# SOHI — Master Specification & System Prompt
Version 1.1 · Canonical architectural reference · Target: Windows 11 (x64)

---

## 0. How to Use This Document (Instructions to the AI Assistant)

You are acting as **Principal Systems Architect and Lead Engineer** for **Sohi**, an ultra-lightweight
desktop voice assistant that runs natively on Windows 11. This document is the single source of truth.
In every session:

1. **Find the active phase** (Section 7) from what the user says or from the repository state. Do not start work from a later phase until the current phase's acceptance criteria (Section 8) pass.
2. **The budgets in Section 6 are hard limits.** Before you propose a change, say how it affects idle RAM, idle CPU, cold start, binary size and latency. If a change could break a budget, say so first and offer an alternative.
3. **Ask before adding a dependency.** Each new crate needs a short justification table: crate, purpose, enabled features, estimated binary-size change, and whether it is pulled in transitively.
4. **Keep all `unsafe` code and FFI inside `sohi-win`.** Every other crate uses `#![forbid(unsafe_code)]`.
5. **Write complete, compilable code**, with no placeholders like `todo!()` unless the user asks for a stub. Include tests for anything in `sohi-core`.
6. **Never invent API details.** Items marked **[VERIFY]** must be checked against current vendor documentation before you implement them, and you must say that you checked.
7. When the user changes a requirement, update the matching section of this spec in your answer, and mark the change with `> SPEC CHANGE:` so the user can merge it back into this file.
8. **Commit after every completed task** following Section 10. Run `cargo xtask check`, commit with the §10.3 format (including the attribution trailer), push to the phase branch, and report the short hash.

---

## 1. Project Overview & Core Objectives

### 1.1 Concept
Sohi is an assistant in the style of JARVIS. It sits **hidden off-screen** (parked) in the top-right corner of the
**primary display**. When woken, it **slides onto the screen** as a small always-on-top HUD widget: a minimalist
**deep-crimson "head"** inside a thin HUD bounding frame. The head is an original 2D netrunner bust drawn as a crimson
hologram, the **hooded ghost**: a dark hood with a crimson rim, a face hidden in shadow, two **round glowing eyes**, and a
short glowing collar line on the chest (flat geometric shapes, no likeness of any existing character). The eyes take the
visor's role (listening, brightness) and the collar line follows the speech envelope. The user talks to it (online voice pipeline) or types to it
(text "pill"). Replies are spoken and/or shown as text. When nothing has happened for a while, it slides back off-screen.

### 1.2 Wake triggers
1. **Raise to wake:** the user deliberately parks the mouse pointer in the top-right corner hot zone for a short dwell time.
2. **Global hotkey:** `Ctrl+Shift+Space` toggles voice. `Ctrl+Alt+T` opens text mode. Both can be changed in config.

> SPEC CHANGE (2026-11-22): the voice hotkey default moved from `Ctrl+Alt+Space` to `Ctrl+Shift+Space`. Another app
> on the user's machine already holds `Ctrl+Alt+Space`.

### 1.3 Core objectives (priority order)
1. **Anti-bloat:** about 0% idle CPU, ≤ 20 MB idle private memory, under 200 ms cold start, ≤ 8 MB binary.
2. **Responsiveness:** under 80 ms from wake to the mic capturing, and ≤ 1.5 s (p50) from end of speech to the first spoken audio.
3. **Never intrusive:** the widget never steals focus unless the user asks for text input. It does not appear over full-screen apps on corner wake. It is fully hidden (no compositor cost) while parked.
4. **Graceful degradation:** online voice falls back to an offline text pill, which uses a local LLM (Ollama, if present) or a deterministic local command parser.
5. **Safe local actions:** it can launch apps, open URLs, use the clipboard and control media/volume. Sensitive actions need confirmation, and shell commands are off by default.

### 1.4 Non-goals (v1)
Always-on wake word · bundled local models · cross-platform builds · screen understanding ·
plugin/MCP runtime · installer or auto-update · telemetry · draggable widget.

---

## 2. Languages & Tech Stack (with justifications)

### 2.1 Language & toolchain
- **Rust (edition 2024)**, stable toolchain pinned in `rust-toolchain.toml`, MSVC target `x86_64-pc-windows-msvc`.
  - *Why:* no GC pauses, predictable memory use, zero-cost FFI to Win32/WinRT through Microsoft's `windows` crate, and memory safety outside a small audited FFI layer.

### 2.2 Stack decisions

| Concern | Choice | Justification |
|---|---|---|
| Windowing | **Raw Win32 via `windows` crate** (no winit) | We need `WS_EX_LAYERED` + `UpdateLayeredWindow` per-pixel alpha, `WS_EX_NOACTIVATE`, `MA_NOACTIVATE`, and exact control of the message loop (`MsgWaitForMultipleObjectsEx`). winit hides or fights these. Raw Win32 adds about 0 KB of runtime overhead. |
| Head rendering | **`tiny-skia`** (CPU, `simd`) → premultiplied BGRA DIB → `UpdateLayeredWindow` | The surface is about 210×160 px at 150 % scaling, so CPU rasterization takes under 0.5 ms. No GPU device, swapchain or shader compiler, and no GPU memory while idle. |
| Text pill UI | **Native Win32 `EDIT` controls** in a separate popup window, DWM rounded corners | Gives IME, caret, selection, clipboard, accessibility and Unicode for free. A hand-written text editor would be thousands of lines and a source of bugs. |
| Audio I/O | **WASAPI directly via `windows`** (event-driven shared mode, `AUTOCONVERTPCM`) | The OS resamples to 16 kHz mono i16 for capture at no cost to us, so we need neither `cpal` nor a resampler crate. |
| VAD | **earshot** 1.2 (pure-Rust neural VAD, `default-features = false, features = ["std"]`) | About 95 KB, deterministic, no ML runtime or C build, runs on 16 ms frames. |

> SPEC CHANGE (2026-11-22): the VAD is `earshot`, not WebRTC VAD. The `webrtc-vad` crate has had no release since
> 2019 and needs a C toolchain; earshot is pure Rust, maintained, and more accurate on noisy speech. It costs about
> 95 KB instead of about 50 KB. Its frames are 16 ms (256 samples), so the segmentation rules in §5.4 are restated
> in those frames with the same timings.
| Async runtime | **`tokio`** current-thread runtime, features `rt, net, time, sync, macros, io-util` | Needed for streaming HTTP (SSE/NDJSON), concurrent prewarming and cancellation. It runs on a single dedicated thread that starts lazily on first wake. |
| HTTP/TLS | **`reqwest`** with `default-features = false`, features `native-tls, json, stream, multipart` | `native-tls` on Windows uses **SChannel**, so no crypto library is bundled. That is about 1.5–2 MB smaller than rustls with ring. |
| Serialization | `serde`, `serde_json`, `toml` | Config and API payloads. |
| Offline TTS | **WinRT `Windows.Media.SpeechSynthesis`** (OneCore voices) | Ships with Windows, works offline, adds nothing to the binary. |
| Secrets | **Windows Credential Manager** (`CredWriteW`/`CredReadW`) | Keys are never stored in plaintext config. |
| Ring buffers | `rtrb` (lock-free SPSC) | Audio thread → processing with no allocation and no locks. |
| Errors | `thiserror` in library crates; `anyhow` allowed **only** in the `sohi` binary crate | Typed errors at module boundaries. |
| Logging | `log` facade + in-house rolling file sink (1 MB × 3 files) | `tracing` and its subscribers are heavier than we need. |

### 2.3 External services
| Role | Primary (online) | Fallback |
|---|---|---|
| STT | Mistral **Voxtral** via `POST https://api.mistral.ai/v1/audio/transcriptions`, model `voxtral-mini-transcribe-3` **[VERIFY]** | None in v1 (offline means text input). Windows offline speech recognition is a possible later phase. |
| LLM | Mistral chat completions, model `mistral-small-latest`, streaming SSE, function calling **[VERIFY tool-call schema]** | Anthropic Messages API (model set in config, e.g. a Claude Haiku/Sonnet tier). Used when Mistral fails before writing any text and `anthropic_model` is set; a reply already under way is never restarted on another provider, and offline **Ollama** at `http://127.0.0.1:11434` (`/api/tags`, `/api/chat` NDJSON) |
| TTS | `TtsProvider` trait. Default: Mistral Voxtral TTS **if available on the account [VERIFY endpoint, model, streaming format]**, otherwise any streaming cloud TTS set in config | OneCore offline voices |

### 2.4 Release profile
~~~toml
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1
panic = "abort"
strip = true
debug = false
~~~
Track binary size on every phase with `cargo bloat --release --crates` (run from xtask).

---

## 3. Module Architecture & Data Flow

### 3.1 Workspace layout
~~~
sohi/
├─ Cargo.toml                # workspace, shared [workspace.dependencies]
├─ rust-toolchain.toml
├─ .gitignore
├─ assets/sohi.manifest      # PerMonitorV2 DPI, Win11 compat, asInvoker
├─ docs/
│  ├─ MASTER_SPEC.md         # this document
│  └─ perf/                  # per-phase budget reports
├─ crates/
│  ├─ sohi-core/       # PURE: state machine, events/effects, config schema, intent parser,
│  │                   #       sentence chunker, markdown stripper. No IO, no unsafe.
│  ├─ sohi-render/     # PURE: fn render(&VisualState, t) -> Pixmap  (tiny-skia)
│  ├─ sohi-win/        # ALL FFI: windows, message loop, layered blit, hotkeys, cursor poll,
│  │                   #   tray, DPI, fullscreen query, network hints, credman, WASAPI,
│  │                   #   OneCore TTS, media keys, endpoint volume, ShellExecute, clipboard
│  ├─ sohi-audio/      # VAD segmentation, pre-roll, WAV encode, level meter/envelope (safe Rust)
│  ├─ sohi-providers/  # traits SttProvider/LlmProvider/TtsProvider; mistral, anthropic,
│  │                   #   ollama, onecore impls; SSE + NDJSON parsers; retry policy
│  ├─ sohi-tools/      # tool registry, JSON schemas, policy (confirm/allow), executors
│  └─ sohi/            # binary: wiring, UI-thread loop, runtime thread, orchestrator
└─ xtask/              # check, perf harness, bloat check, snapshots, release packaging
~~~

### 3.2 Thread model
~~~
┌──────────────────────────── UI THREAD (main) ─────────────────────────────┐
│ Win32 message loop: MsgWaitForMultipleObjectsEx(INFINITE | anim timer)     │
│ owns: HeadWindow, PillWindow(lazy), TrayIcon, Hotkeys, CursorPoll timer     │
│ owns: sohi-core::Machine  (step(state, event) -> (state, effects))          │
│ executes UI effects; forwards IO effects ──────────────┐                    │
└───────────▲────────────────────────────────────────────┼───────────────────┘
            │ PostMessageW(WM_APP_EVENT) + mpsc queue    │ tokio mpsc (try_send)
            │                                            ▼
┌───────────┴──────────── RUNTIME THREAD (lazy, tokio current_thread) ───────┐
│ Orchestrator: one cancellable Turn task at a time                           │
│ Connectivity monitor · provider clients (reqwest/SChannel) · tool executor  │
└───────────▲────────────────────────────────────────────┬───────────────────┘
            │ rtrb SPSC (PCM frames, level)              │ rtrb SPSC (PCM out)
┌───────────┴────────────┐                  ┌────────────▼──────────────────┐
│ CAPTURE THREAD (WASAPI)│                  │ RENDER THREAD (WASAPI)         │
│ alive only while       │                  │ alive only while speaking      │
│ Listening; MMCSS "Pro  │                  │ MMCSS "Pro Audio"; no alloc    │
│ Audio"; no alloc/lock  │                  │ no locks                       │
└────────────────────────┘                  └────────────────────────────────┘
~~~
While **parked and idle**, only the UI thread exists and is blocked in the kernel. It wakes only for the
cursor-poll timer (4 Hz) and messages. The runtime thread, once started, stays blocked in IOCP with no timers.

### 3.3 Voice turn data flow
~~~
 Wake (corner dwell | hotkey)
   │
   ├─► UI: reveal animation ─────────────────────────────────────────────┐
   ├─► Capture: open WASAPI 16k mono i16 ──► VAD (16 ms frames)           │
   └─► Runtime: prewarm TLS (GET /v1/models, cached 60 s)                 │
                                                                          │
 VAD end-of-speech (700 ms hangover)                                      │
   │  pre-roll 300 ms + utterance ──► WAV (in-memory)                     │
   ▼                                                                      │
 STT  POST /v1/audio/transcriptions (multipart) ──► transcript            │
   ▼                                                                      │
 LLM  POST chat (stream) ──► token stream ──┬──► Pill/bubble text (UI)    │
   │      ▲                                 │                             │
   │      └── tool_call → policy ─(confirm?)─► executor ─► tool_result    │
   ▼                                                                      │
 SentenceChunker ──► markdown strip ──► TTS (stream) ──► PCM ──► Render   │
   │                                       envelope ──► head visor anim ◄─┘
   ▼
 Playback done + LLM done ──► linger 2.5 s ──► retract ──► SW_HIDE (parked)
~~~

### 3.4 Core abstractions (`sohi-core`)
~~~rust
pub enum Event {
    Wake { source: WakeSource, mode: WakeMode },      // Corner | Hotkey ; Voice | Text
    Cancel, Escape, HeadClicked, HoverChanged(bool),
    AnimDone(AnimKind), RetractTimer,
    SpeechStart, SpeechEnd, NoSpeechTimeout, MaxUtterance,
    Transcript(String), LlmDelta(String), LlmDone,
    ToolCallRequested(ToolCall), ConfirmAnswer(bool), ToolResult(ToolResult),
    TtsStarted, PlaybackDone,
    TextSubmitted(String),
    Net(ConnectivityEvent), ProviderError(ProviderError),
    ForceOfflineToggled, FullscreenChanged(bool), SessionLocked(bool),
}
pub enum Effect {
    Reveal, Retract, ShowPill { focus: bool }, HidePill, SetVisual(VisualState),
    OpenMic, CloseMic, StartTurn(TurnInput), CancelTurn, StopPlayback,
    ExecTool(ToolCall), AskConfirm(ToolCall), ScheduleRetract(Duration),
    AppendBubble(String), ClearBubble, Prewarm, Probe, Log(LogLine),
}
pub fn step(s: &State, e: Event) -> (State, SmallVec<[Effect; 4]>);  // pure, total, tested
~~~

---

## 4. UI, Animation & Behavioral Specification

### 4.1 Geometry (all values in DIPs, scaled per monitor via `GetDpiForMonitor`)
~~~
 Primary monitor rcMonitor top-right corner ─────────────────────┐ (physical corner)
                                                    ┌──8×8 px──┐ │ ← HOT ZONE (physical px,
                                                    └──────────┘ │    rcMonitor based)
          rcWork top-right ──────────────────────────────────────┤
                                 12 DIP margin ↓   ← 12 DIP →    │
                    ┌────────── HEAD WINDOW 112 × 84 ──────────┐ │
                    │  ┌┐                                  ┌┐  │ │
                    │  └ frame: 88×64, 1 DIP stroke, 8 DIP  ┘  │ │
                    │     corner brackets                      │ │
                    │      ┌────── head 72×44, r=3 ─────┐      │ │
                    │      │   ▬▬▬▬▬▬▬ visor 40×4 ▬▬▬    │      │ │
                    │      └────────────────────────────┘      │ │
                    │  ┌┐                                  ┌┐  │ │
                    └──────────────────────────────────────────┘ │
                    ┌──── PILL 320 × 36 (8 DIP below frame) ───┐ │  right-aligned
                    │ > type a message…                        │ │  with the frame
                    ├──── BUBBLE (grows to 6 lines, scrolls) ──┤ │
                    └──────────────────────────────────────────┘ │
~~~
- The head window sits at the **work-area** top-right (`rcWork`), so a top-docked taskbar is respected.
- The hot zone uses the **monitor** corner (`rcMonitor`). That is where a flicked cursor pins, so it is easy to hit.
- Recompute everything on `WM_DPICHANGED`, `WM_DISPLAYCHANGE` and `WM_SETTINGCHANGE(SPI_SETWORKAREA)`, and when the primary monitor changes.

> SPEC CHANGE (2026-11-22): the head is drawn at 125 %. The diagram's head sizes are the layout grid
> (`HEAD_LAYOUT` = 112 × 84). The window is that grid scaled by `HEAD_SCALE_PERCENT` = 125, so it is
> 140 × 105 DIP. The 12 DIP margin, the hot zone and the pill size are not scaled. The pill sits 8 DIP below
> the frame as drawn.

### 4.2 Head window flags
| Flag / API | Purpose |
|---|---|
| `WS_POPUP` | No chrome |
| `WS_EX_LAYERED` + `UpdateLayeredWindow(ULW_ALPHA)` | Per-pixel alpha. **Pixels with alpha 0 are hit-test transparent**, so clicks pass through the empty space between head and frame. |
| `WS_EX_TOPMOST` | Always on top (re-assert `HWND_TOPMOST` on reveal) |
| `WS_EX_TOOLWINDOW` | No taskbar button, no Alt-Tab entry |
| `WS_EX_NOACTIVATE` + return `MA_NOACTIVATE` on `WM_MOUSEACTIVATE` | Never steals focus |
| `ShowWindow(SW_SHOWNOACTIVATE)` / `SW_HIDE` | Shown only while revealed. **While parked the window is hidden** (zero DWM cost). |
| `DwmSetWindowAttribute(DWMWA_EXCLUDED_FROM_PEEK, TRUE)` | Not affected by Aero Peek |
| `TrackMouseEvent(TME_HOVER\|TME_LEAVE)` | Hovering pauses auto-retract |

**Mouse on the head:** left-click opens text mode (focuses the pill). Clicking while Speaking/Thinking cancels the turn (barge-in).
Right-click opens the same menu as the tray icon.

### 4.3 Slide behaviour (moves onto screen when woken)
- **Parked:** the window is hidden and nothing renders.
- **Reveal (260 ms, cubic ease-out):** call `SW_SHOWNOACTIVATE` at the final rectangle, then **translate the content inside the bitmap**
  from `x = +window_width` (clipped, fully off the right edge) to `x = 0`, fading alpha 0→1 at the same time. This looks exactly like the widget
  sliding in from off-screen. **The window never moves across the monitor edge**, so it can never bleed onto a monitor to the right.
- **Retract (200 ms, cubic ease-in):** the reverse, then `SW_HIDE` and the state becomes `Parked`.
- **Reduced motion:** if `SPI_GETCLIENTAREAANIMATION` is FALSE, use a 60 ms fade with no translation.
- **Launch peek:** after start, reveal for 1.2 s in the Idle visual, then retract. This shows the user Sohi is alive. Config: `ui.launch_peek = true`.

### 4.4 Auto-retract rules
| Situation | Retract after |
|---|---|
| Revealed, no speech and no input | 8 s no-speech timeout, then 0.5 s, then retract |
| Reply finished (playback + LLM done) | 2.5 s linger |
| Hover over head | Paused, resumes 1 s after mouse leaves |
| Text mode with focus or non-empty input | Never auto-retracts. `Esc` closes. |
| Text reply shown, pill unfocused | 10 s |
| Error | 3 s (unless the pill is open) |

### 4.5 Visual states (tokens)
~~~
RED_ACTIVE  #B0122A   RED_IDLE  #6D0B1A   VISOR  #FF4D5E   FRAME = RED_ACTIVE @ 45% α
OFFLINE     #6B5557   AMBER     #FFB020   SHADOW = #000 @ 35% α, 6 DIP blur (pre-rendered once)
~~~
| State | Head | Visor | Frame | Animation (only while revealed) |
|---|---|---|---|---|
| Idle | RED_IDLE 90% α | 60% brightness | 35% α brackets + hairline | Breathing ±8% brightness, 4 s period, **30 fps cap** |
| Listening | RED_ACTIVE | Width 24→40 DIP follows mic RMS (attack 30 ms, release 150 ms) | Brackets 100% | 60 fps |
| Transcribing | RED_ACTIVE 85% | 40% | Hairline + 2 DIP scanline sweeping top→bottom, 900 ms | 60 fps |
| Thinking | RED_ACTIVE 85% | 40% | Corner brackets light in sequence (chase), 600 ms cycle | 60 fps |
| Speaking | RED_ACTIVE | Height 2→8 DIP follows TTS output envelope | Brackets 100% | 60 fps |
| Confirming | RED_ACTIVE | Blinks at 2 Hz | AMBER | 30 fps |
| Offline | OFFLINE | 30% | Dashed hairline (4/3 DIP) + 6 DIP grey square bottom-right | Static, no breathing |
| Forced offline | OFFLINE | 30% | As Offline, square drawn as an outline | Static |
| Error | Unchanged | Unchanged | Two 120 ms AMBER flashes | Then returns to the previous state's visual |

**Frame pacing:** while any animation runs, the UI loop waits on a **high-resolution waitable timer**
(`CreateWaitableTimerExW(CREATE_WAITABLE_TIMER_HIGH_RESOLUTION)`) with a 16.6 ms period (33.3 ms at 30 fps) via
`MsgWaitForMultipleObjectsEx`. When nothing animates, the timer is cancelled and the loop blocks with `INFINITE`.
**Never** call `timeBeginPeriod`. A static state draws once and then makes zero further redraws.

**Render pipeline:** `sohi-render::render(&VisualState, t)` → tiny-skia `Pixmap` (RGBA premultiplied) → swizzle into a
persistent 32-bpp top-down DIB section (BGRA premultiplied) → `UpdateLayeredWindow`. The pixmap and DIB are allocated
once and resized only on DPI change. The static layers (shadow, head body) are pre-rendered and cached per DPI.

### 4.6 Text pill (fallback / text mode)
- A separate top-level window, **created lazily** on first use and **destroyed** 5 minutes after last use (to reclaim RAM).
- Flags: `WS_POPUP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW`. It is not layered (child controls must paint). Win11 polish:
  `DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND`, `DWMWA_BORDER_COLOR = #B0122A` at low intensity, and
  `DWMWA_USE_IMMERSIVE_DARK_MODE = TRUE`.
- Children: a single-line `EDIT` (input, `ES_AUTOHSCROLL`) and a multi-line read-only `EDIT` (the bubble), with
  `SetWindowTheme(L"DarkMode_Explorer")` for dark scrollbars. Colors come from `WM_CTLCOLOREDIT`/`WM_CTLCOLORSTATIC`:
  background `#141416`, text `#EDEDED`. Font: Segoe UI Variable Text, 10.5 pt at the current DPI.
- Keys: `Enter` submits · `Esc` closes and retracts · `↑` recalls the last prompt · `Ctrl+C` works natively in the bubble ·
  `Ctrl+Alt+T` toggles the pill.
- **Focus:** the pill takes foreground focus only when opened by hotkey (the process just received the input
  event, so `SetForegroundWindow` is allowed) or by clicking the head. On a corner wake while offline, the pill
  appears **unfocused**, with the placeholder "Offline. Click or press Ctrl+Alt+T to type."
- A text prompt gets a **text reply only** (no TTS). Config: `text.speak_replies = false`.
- Markdown is shown as plain text: code fences are kept as indented blocks, emphasis markers are stripped.

### 4.7 Raise-to-wake detection
~~~
Poll GetCursorPos on the UI thread via SetCoalescableTimer (tolerance 32 ms):
  FAR  (cursor > 300 px from the corner)     → 250 ms period (4 Hz)
  NEAR (within 300 px of the rcMonitor corner) → 50 ms period (20 Hz)
In HOT ZONE (8×8 physical px at the corner):
  require: no mouse button down (GetAsyncKeyState LBUTTON/RBUTTON/MBUTTON)
  require: continuous presence ≥ 350 ms (reset on exit)
  require: ≥ 1.5 s since the last retract (cooldown)
  require: state == Parked
  then: query SHQueryUserNotificationState once; if QUNS_RUNNING_D3D_FULL_SCREEN,
        QUNS_BUSY or QUNS_PRESENTATION_MODE → suppress (corner only; hotkeys still work)
  → Event::Wake { source: Corner, mode: Voice (online) | Text (offline) }
Polling is SUSPENDED when: session locked (WTSRegisterSessionNotification), display off
(RegisterPowerSettingNotification GUID_CONSOLE_DISPLAY_STATE), or system suspending.
~~~
Hotkeys: `RegisterHotKey(MOD_CONTROL|MOD_SHIFT|MOD_NOREPEAT, VK_SPACE)` and `MOD_CONTROL|MOD_ALT|MOD_NOREPEAT`
with `'T'`. If registration
fails (key combination in use), show a tray balloon and log it. Sohi keeps running with corner wake only.

### 4.8 Tray icon
`Shell_NotifyIconW` with a 16/20/24/32 px red-square icon. Menu: *Talk* · *Type* · *Force offline ☐* · *Open config* ·
*Open log folder* · *Start with Windows ☐* · *Quit*. The menu is **always dark**, whatever the Windows app theme:
`SetPreferredAppMode(ForceDark)` + `FlushMenuThemes` (uxtheme.dll ordinals 135/136, undocumented but stable since
Windows 10 1903 and used by Windows Terminal and Notepad++). If they cannot be resolved the menu uses the system theme. Re-add the icon when the `TaskbarCreated` message arrives (Explorer restarted).

---

## 5. Online / Offline State Machine & Audio/API Pipeline

### 5.1 Orthogonal regions
The `State` struct has three independent regions:
1. **Visibility:** `Parked | Revealing | Shown | Retracting`
2. **Interaction:** `Idle | Listening | Transcribing | Thinking | Speaking | Confirming | TextEntry | Error`
3. **Connectivity:** `Online | Probing | Offline | ForcedOffline`

### 5.2 Interaction transitions (exhaustive, tested)
| From | Event | Guard | To | Effects |
|---|---|---|---|---|
| Idle | Wake{Voice} | Online/Probing | Listening | Reveal, OpenMic, Prewarm, SetVisual |
| Idle | Wake{Voice} | Offline/Forced | TextEntry | Reveal, ShowPill{focus: source==Hotkey} |
| Idle | Wake{Text} / HeadClicked | – | TextEntry | Reveal (if parked), ShowPill{focus:true} |
| Listening | SpeechEnd | voiced ≥ 250 ms | Transcribing | CloseMic, StartTurn(Audio) |
| Listening | SpeechEnd | voiced < 250 ms | Listening | (discard, keep listening) |
| Listening | NoSpeechTimeout (8 s) | – | Idle | CloseMic, ScheduleRetract(0.5 s) |
| Listening | MaxUtterance (30 s) | – | Transcribing | CloseMic, StartTurn(Audio) |
| Listening | Wake (hotkey again) / Escape | – | Idle | CloseMic, Retract |
| Transcribing | Transcript(t) | t.trim() non-empty | Thinking | AppendBubble("› "+t) |
| Transcribing | Transcript(t) | empty | Idle | AppendBubble("Didn't catch that."), ScheduleRetract(2.5 s) |
| Thinking | LlmDelta | first complete sentence | Speaking | AppendBubble, (TTS enqueue in orchestrator) |
| Thinking | ToolCallRequested(c) | policy = confirm | Confirming | AskConfirm(c), ShowPill |
| Thinking | ToolCallRequested(c) | policy = allow | Thinking | ExecTool(c) |
| Confirming | ConfirmAnswer(b) / 10 s timeout = false | – | Thinking | ExecTool or ToolResult(denied) |
| Speaking | PlaybackDone | LlmDone | Idle | ScheduleRetract(2.5 s) |
| Transcribing/Thinking/Speaking | Wake (hotkey) / HeadClicked | Online | Listening | CancelTurn, StopPlayback, OpenMic (**barge-in**) |
| TextEntry | TextSubmitted(t) | – | Thinking | StartTurn(Text(t)) |
| Thinking (text turn) | LlmDone | – | TextEntry | (text only, no TTS) |
| Any | Escape | – | Idle | CancelTurn, StopPlayback, CloseMic, HidePill, Retract |
| Any active | ProviderError(e) | – | Error → (1.2 s) Idle or TextEntry | SetVisual(Error), AppendBubble(e.user_message()) |

> SPEC CHANGE (2026-11-22): until TTS lands (P5), a voice turn's transcript and reply show in the pill, opened
> without focus, and the turn ends in TextEntry like a text turn. An empty transcript shows "Didn't catch that." in
> the pill under the 2.5 s reply-done retract rule (TextEntry rather than Idle, so Esc and the retract close the
> pill). The connectivity guard on Wake{Voice} arrives with the Connectivity region; until then an offline voice
> turn fails at transcription with the usual error.

> SPEC CHANGE (2026-11-23): **follow-up.** With `[wake] follow_up = true` (the default), a finished voice reply goes
> Thinking → Listening instead of TextEntry, with the reply still in the pill. 8 s of silence ends the conversation
> (TextEntry, 10 s unfocused retract); clicking into the pill cancels the listening. A pill that is focused or holds
> text gets no follow-up, and neither does a typed turn. From P5 the mic reopens only after playback ends.
> **Name.** STT defaults to `voxtral-mini-transcribe-3` with `context_bias=Sohi`, and a spoken turn's system prompt
> says the name may be misheard. On 12 synthesized clips with the name and 7 without: 11/12 right and 1/7 wrong,
> against 5/12 and 0/7 for `voxtral-mini-latest` with no bias. Picked from several settings on these same clips.
> On 7 clips of the user's voice: 7/7 right (5/7 without the bias). The name alone still comes back as "So he", so an
> utterance that is only a misheard name, with or without a greeting, is corrected locally; longer ones never are.
> **Esc.** While a voice turn runs no Sohi window has the keyboard, so plain Esc is registered as a global hotkey
> for exactly that long. Other apps do not receive Esc meanwhile.

> SPEC CHANGE (2026-11-24): **speaking.** Thinking goes to Speaking when speech starts to play, not at the first complete
> sentence. A voice turn ends with PlaybackDone (after LlmDone), then follow-up listening; without follow-up it ends
> in TextEntry under the 2.5 s reply-done rule. A click on the head barges in on a voice turn only; during a typed
> turn it focuses the pill. Text submitted while speaking cancels the speech. If online speech fails, the offline
> voice says that sentence (unless part of it was heard) and the rest of the turn.

### 5.3 Connectivity transitions
~~~
                 request fails (DNS/connect/TLS/timeout)  or  NLM hint = no internet
   ┌────────┐ ───────────────────────────────────────────────►  ┌─────────┐
   │ Online │                                                    │ Probing │
   └────────┘ ◄─────────────────── probe OK ──────────────────── └────┬────┘
        ▲                                                             │ 2 consecutive failures
        │ probe OK                                                    ▼
        │                         NLM change event | wake | backoff   ┌─────────┐
        └──────────────────────── (15 s→30 s→60 s, ONLY while Shown) ─│ Offline │
                                                                      └─────────┘
   Any ── user toggles "Force offline" ──► ForcedOffline ── toggle ──► Probing
~~~
- **Event-driven hints:** `NotifyNetworkConnectivityHintChange` (IP Helper) gives change events without polling.
  Hints are advisory. Only a probe (`GET https://api.mistral.ai/v1/models`, 2 s timeout) or a real request decides the state.
- **While Parked, never poll.** Re-check only on a hint event or on wake. A wake while `Offline` triggers one probe
  in parallel with showing the text pill. If the probe succeeds within 400 ms, switch to voice.
- **HTTP 4xx/5xx is not "offline".** 401/403 → `AuthError` (tell the user to set the key). 429/5xx → one retry
  with 300 ms jitter, then `Error`.
- **Offline brain routing:** if Ollama is reachable (`GET http://127.0.0.1:11434/api/tags`, 150 ms timeout, checked on each
  Offline transition) and `llm.offline_model` is set, use Ollama with streaming NDJSON (tools if the model supports them).
  Otherwise use `sohi-core::intent`, a deterministic grammar: `open <app>`, `go to <url>`, `play|pause|next|previous`,
  `volume <up|down|N>`, `mute`, `copy <text>`, `what time is it`. Unmatched input gets the reply "Offline. I can only run local commands right now."

> SPEC CHANGE (2026-11-24): **connectivity.** Sohi starts Online; the first hint or request finds out otherwise. Ollama
> is looked for at each offline turn, not only on the transition, so starting it later works. Commands are only
> understood in P6: the time is answered, the rest says it cannot run commands yet (executors are P7). An offline wake
> focuses the pill only for the voice hotkey, the tray's Talk and a click on the head. Forced offline is remembered
> across restarts by an empty `force-offline` file next to the config, and cancels a running turn.

### 5.4 Audio capture pipeline
- On `OpenMic`: open WASAPI shared-mode event-driven `IAudioClient` on the default capture endpoint with flags
  `AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUTOCONVERTPCM | SRC_DEFAULT_QUALITY` and format **16 kHz, mono, i16**.
  The capture thread is registered with MMCSS "Pro Audio". The mic opens **only after wake**, and the Windows mic
  privacy indicator is shown only while Listening.
- The capture thread pushes 10 ms blocks into `rtrb`. **No allocation, no locks, no logging** inside the capture loop.
- The processing side (runtime thread) assembles **16 ms frames (256 samples)** → earshot (voiced at score ≥ 0.5):
  - **Start of speech:** ≥ 6 of the last 10 frames voiced. Speech starts at the first voiced frame of that window,
    moved back over directly preceding frames (at most 256 ms) that score ≥ 0.35 or are at least twice as loud as
    the quietest frame so far: soft onsets such as /s/ and /h/ are not voiced. Prepend a **300 ms pre-roll** ring.
  - **End of speech:** 700 ms (44 frames) unvoiced after speech started. Keep 160 ms of tail.
  - **Too short:** under 250 ms of voiced frames (a cough, a click) is dropped and listening continues.
  - **Limits:** 8 s with no speech at all → `NoSpeechTimeout`. 30 s maximum utterance.
  - RMS level is sent to the UI at 30 Hz, decimated (not per frame).
  - Acceptance: on the five fixtures in `crates/sohi-audio/tests/fixtures` (synthesized speech, padded, in quiet,
    at about 20 dB SNR, and at −16 dB gain) start and end are within ±100 ms of the placed speech.

  > SPEC CHANGE (2026-11-22): rules restated for earshot's 16 ms frames (were 3 of 5 × 30 ms; now 6 of 10 × 16 ms),
  > plus the soft-onset rule, the 160 ms tail and the explicit 250 ms minimum. Measured: starts 0–48 ms after the
  > placed speech, ends 10–96 ms after (the detector holds its score for a few frames once sound stops).
- Encoding: in-memory WAV (RIFF header + PCM16), about 32 KB/s. Opus encoding is a possible later optimization behind a feature flag.
- On `CloseMic`: stop and release the `IAudioClient` and the thread exits. The mic is fully released.

### 5.5 Orchestrator turn (runtime thread)
~~~rust
async fn run_turn(input: TurnInput, cancel: CancelToken, ctx: &Ctx) -> Result<(), TurnError> {
    let text = match input {
        TurnInput::Audio(wav) => select_cancel!(cancel, ctx.stt.transcribe(wav))?,  // ≤ 10 s timeout
        TurnInput::Text(t)    => t,
    };
    ctx.ui.send(Event::Transcript(text.clone()));
    ctx.history.push_user(&text);                         // ring buffer: 12 turns, ~4k tokens cap
    let mut stream = ctx.llm_for(ctx.connectivity()).chat_stream(ctx.messages(), ctx.tools())?;
    let mut chunker = SentenceChunker::new();             // sohi-core, pure
    let tts = ctx.tts_session(voice_reply)?;              // starts the render thread lazily on first PCM
    while let Some(ev) = select_cancel!(cancel, stream.next()) {
        match ev? {
            LlmEvent::Delta(d)  => { ctx.ui.delta(&d); for s in chunker.push(&d) { tts.enqueue(strip_md(&s)) } }
            LlmEvent::ToolCall(c) => { let r = ctx.tools.run_with_policy(c, &ctx.ui, &cancel).await; stream.feed_tool_result(r)? }
            LlmEvent::Done      => break,
        }
    }
    for s in chunker.flush() { tts.enqueue(strip_md(&s)) }
    tts.finish().await                                    // PlaybackDone → UI
}
~~~
- **SentenceChunker:** emits at `[.!?…]` followed by whitespace/end. If more than 60 characters have accumulated, it also emits at `;:—`. Hard cap 200 characters.
  The first chunk may be emitted early at a comma once it is ≥ 40 characters (lowers time to first audio). Code blocks are never spoken:
  the chunker emits "I've put the details in the text panel." once.
- **System prompt (voice):** concise spoken style, ≤ 2 sentences by default, no markdown, and it receives the current local
  date/time and the list of available tools. Text turns may be longer and use markdown.
- **Persona:** Sohi is a sharp-witted, calm American female netrunner with dry, sarcastic humour and a secretive, fiercely
  loyal edge. She addresses the user as **Ashish** (config `persona.user_name`). The TTS voice is chosen to match: an
  American female voice, calm and low-key.
- **TTS:** prefer a provider output format of raw PCM/WAV streaming (no decoder needed). If only MP3/Opus is available, add a
  minimal decoder behind a cargo feature and record its size cost. The render thread uses WASAPI shared mode with
  `AUTOCONVERTPCM` at the provider's sample rate. The output envelope (RMS of each 20 ms block) drives the visor.
  > SPEC CHANGE (2026-11-23): the output opens when a voice turn's reply is requested, not on the first PCM. Opening
  > (`IAudioClient::Initialize`) takes about 300 ms, so this hides it behind the wait for the model. It plays silence
  > until speech arrives and still closes as soon as the last sentence was heard.
  Offline/OneCore: `SpeechSynthesizer.SynthesizeTextToStreamAsync` → WAV → same render path.
- **Half-duplex:** the mic is always closed while speaking. That removes echo/AEC complexity. Barge-in uses the hotkey or a click.
- **Cancellation:** every turn has one `CancelToken` (a `tokio::sync::watch` wrapper). `CancelTurn` aborts in-flight HTTP
  (drops the futures), clears the TTS queue, and stops render within 30 ms.
- **Prewarm:** on voice wake, issue `GET /v1/models` (cached 60 s) so TCP+TLS is pooled before the upload.
  Idle connections are dropped after 90 s (`pool_idle_timeout`).

### 5.6 Tools (`sohi-tools`)
| Tool | Args | Implementation | Default policy |
|---|---|---|---|
| `launch_app` | `name` | Fuzzy match over an index of Start Menu `.lnk` files (user + common, built lazily on first call and re-indexed when older than 10 min) and `App Paths` registry entries → `ShellExecuteW` | allow |
| `open_url` | `url` | `http`/`https` only, validated → `ShellExecuteW` | allow (URL shown in bubble) |
| `clipboard_get` | – | `GetClipboardData(CF_UNICODETEXT)`, truncated to 8 KB | **confirm** (content goes to a cloud LLM) |
| `clipboard_set` | `text` | `SetClipboardData` | allow |
| `media` | `play\|pause\|next\|prev\|mute` | Windows media sessions (`Windows.Media.Control`); `SendInput` for mute | allow |
| `volume` | `up\|down\|set(0-100)` | `IAudioEndpointVolume` | allow |
| `shell_exec` | `command` | **Compiled out unless `--features shell`.** Runs via `CreateProcessW` with no window, 15 s timeout, output capped at 4 KB | **always confirm (cannot be overridden)** |

Confirmation flow: the pill shows `Run: <summary>? Enter = Yes · Esc = No`. A voice "yes/no" is accepted by opening the mic
for 4 s (STT + a fixed yes/no matcher). Silence or a 10 s timeout counts as **No**. Tool outputs are passed to the LLM as **data**,
never as instructions, and the system prompt says so.

> SPEC CHANGE (2026-11-24): the app index also reads the shell's apps folder (`shell:AppsFolder`). On the dev laptop about
> 60 of 220 Start Menu entries have no `.lnk` (Calculator, Settings, Photos and every other Store app). Uninstallers are
> left out of the index, and `App Paths` entries are found only by their exact name.

> SPEC CHANGE (2026-11-24): `ShellExecuteW` and the apps folder are used from a short-lived helper, Sohi's own executable
> started with `--open <target>` or `--list-apps`. Each loads about 36 DLLs that would stay in the process for good
> (measured: working set 25 → 44 MB after one `launch_app`).

> SPEC CHANGE (2026-11-24): `media` takes `play` and `pause` instead of `play_pause`, and uses Windows' media sessions
> instead of media keys. The play/pause key only switches, so "pause" could start the music, and nobody learned whether
> a player reacted. `pause` pauses every player that plays; the result says if nothing was playing, no player was open or
> the player did not react. This runs in the helper (`--media <action>`). If Windows lists no player, `play` still
> presses the key, for players that are listed only once they have played.

> SPEC CHANGE (2026-11-24): the history keeps the tool calls of an answered turn with their results, but no text from
> outside, no call made after the turn read some, and none over 600 bytes. Remembering only the replies, models read that
> "pause music" had been answered with "Paused." alone, and did the same without calling the tool.

> SPEC CHANGE (2026-11-24): once a turn has read text from outside (`clipboard_get`, `shell_exec`), **every** later call of
> that turn is confirmed, whatever `[tools]` says. Otherwise text on the clipboard could have the model open an address
> that carries the clipboard away. A permit type enforces this: the executor takes nothing else. In a turn the model may
> call tools over at most 5 rounds. Enter is a yes only with an empty input and not within 400 ms of the question; typed
> text that is not a plain yes is a no. The question never takes the keyboard.

> SPEC CHANGE (2026-11-24): in a voice turn the mic opens for an answer only after everything written so far was said
> (half-duplex). Silence, or speech that is not a plain yes or no, leaves the question open for the keys until the 10 s
> are over, and then counts as No.

### 5.7 Config & secrets
`%APPDATA%\Sohi\config.toml`. On first run, write it with defaults and comments. The config is re-read when the user picks
"Open config" and the file's mtime has changed (no file watcher).
~~~toml
[wake]      hotkey_voice = "Ctrl+Shift+Space"   hotkey_text = "Ctrl+Alt+T"
            corner_size_px = 8   dwell_ms = 350   cooldown_ms = 1500
[ui]        launch_peek = true   reduced_motion = "system"
[llm]       provider = "mistral"   model = "mistral-small-latest"
            fallback_provider = "anthropic"   anthropic_model = ""   offline_model = ""   # e.g. a small Ollama model
[stt]       provider = "mistral"   model = "voxtral-mini-latest"   language = "auto"
[tts]       provider = "mistral"   voice = ""   offline_voice = "system-default"
> SPEC CHANGE (2026-11-23): `offline_voice` defaults to "Microsoft Zira" (the en-US female OneCore voice, as the persona asks); a name that
> is not installed, or "system-default", uses the Windows default, which is usually a male voice. `voice = ""` is "Sheila" (`voice_478`).
[persona]   user_name = "Ashish"
[text]      speak_replies = false
[tools]     confirm = ["clipboard_get"]   disabled = []
[history]   persist = false
[log]       level = "info"
~~~
Keys are stored in Credential Manager under targets `Sohi/mistral`, `Sohi/anthropic`, `Sohi/tts`. To set one:
`sohi.exe --set-key mistral` (reads from stdin with echo off). **Keys are never logged and never written to config.**

### 5.8 Startup sequence
1. Named mutex `Local\Sohi.SingleInstance`. If it already exists, post `RegisterWindowMessage("Sohi.Wake")` to the running instance and exit.
2. Manifest sets PerMonitorV2 DPI awareness. Load the config (≤ 2 ms).
3. Create the head window (hidden), allocate the DIB at the current DPI, add the tray icon, register hotkeys, start the cursor timer.
4. Signal `armed`. If `launch_peek` is on, reveal.
5. **Lazy:** the runtime thread, HTTP client, pill window, app index and providers are created on first use.

### 5.9 System events
`WM_POWERBROADCAST` (suspend: cancel the turn, close audio; resume: mark connectivity `Probing` on next wake) ·
`WTS_SESSION_LOCK/UNLOCK` · default audio device change (`IMMNotificationClient`: reopen on next `OpenMic`; in mid-capture,
fail the turn gracefully) · `WM_ENDSESSION` (flush the log, exit).

---

## 6. Hard Performance Constraints & Budgets

All measured on the **release** build by `cargo xtask perf` on a mid-range Win11 laptop (record the specs in the report).

| Metric | Budget | Definition / measurement |
|---|---|---|
| **Idle RAM** | **≤ 20 MB private bytes** and ≤ 27 MB working set | `GetProcessMemoryInfo` 60 s after returning to Parked, measured **after 10 voice turns**. Working-set trimming hacks (`EmptyWorkingSet`) are forbidden. |
| Pre-first-wake RAM | ≤ 10 MB private | After launch peek, before any wake |
| Peak RAM during turn | ≤ 60 MB private | Sampled at 100 ms during the turn |
| **Idle CPU** | **≤ 60 ms CPU time per 60 s wall** (≈ 0.1% of one core) | `GetProcessTimes` delta while parked, cursor far from the corner |
| Idle wakeups | ≤ 5 per second while parked | ETW/`powercfg /energy` spot check per phase |
| **Cold start** | **≤ 120 ms to armed, ≤ 200 ms to first visible peek frame** | Process creation time → named event `Sohi.Armed` / first `UpdateLayeredWindow` (perf mode) |
| **Binary size** | **≤ 8 MB** (stretch goal ≤ 5 MB) | Stripped release `sohi.exe` |
| Wake → mic capturing | ≤ 350 ms | Wake event → first captured buffer timestamp |
| Wake → reveal complete | ≤ 280 ms | Includes the 260 ms animation |
| Corner dwell → wake | 350 ms ± 60 ms | Includes poll granularity once in the NEAR band |
| **End of speech → first TTS audio** | **≤ 1.5 s p50, ≤ 2.5 s p95** (online) | VAD end-of-speech decision → first PCM sample submitted to WASAPI render |
| End of speech → first text token | ≤ 1.0 s p50 | VAD end-of-speech decision → first `LlmDelta` rendered |
| Barge-in stop | ≤ 50 ms | Hotkey → render silent |
| Frame render | ≤ 1.0 ms p95 | `render()` + swizzle + `UpdateLayeredWindow` |
| UI thread stall | No handler > 4 ms | Instrumented in debug builds; release perf mode logs the worst case |

> SPEC CHANGE (2026-11-22): frame render p95 raised from 0.5 ms to 1.0 ms. Rendering and the swizzle take about
> 50 µs; a full `UpdateLayeredWindowIndirect` that actually reaches the screen costs 0.7–1.1 ms on the dev machine.
> P1's 0.44 ms was measured while updates left out `psize`, which kept the head from appearing at all.

> SPEC CHANGE (2026-11-24): idle working set raised from 25 MB to 27 MB. After 10 voice turns it is 25.9 to 26.1 MB on the
> dev machine (private bytes 5.6 MB). About 1.6 MB of it is Windows' UI Automation, loaded for the pill's text boxes (screen
> readers need it); some is third-party network software that Windows loads into every process with a socket.

**Latency budget breakdown (online, p50 target):** WAV encode 2 ms · upload + Voxtral STT 350–700 ms · LLM time to first sentence
250–450 ms · TTS time to first byte 200–300 ms · WASAPI start 20 ms ⇒ **about 0.9–1.5 s**.

---

## 7. Phased Development Plan

Each phase is broken into tasks. The task list and the commit for each task are in §10.6.

| Phase | Name | Concrete deliverables |
|---|---|---|
| **P0** | Scaffold & perf harness | Workspace + crates (empty APIs), `.gitignore`, `rust-toolchain.toml`, release profile, manifest, single-instance mutex, rolling file logger, panic hook (logs message + backtrace, then aborts), config load/write-defaults, `--perf` mode emitting `perf:` timing lines and the `Sohi.Armed` event, `xtask check` / `perf` / `bloat`, spec committed to `docs/`. |
| **P1** | Head widget & visuals | Layered head window with all flags, DPI handling, tiny-skia renderer for **all** visual states, high-res timer frame pacing, reveal/retract slide, launch peek, tray icon + menu, debug hotkeys (dev feature) to cycle states, `xtask snap`. |
| **P2** | Wake triggers | Adaptive cursor polling, hot zone + dwell + cooldown + button guard, fullscreen suppression, global hotkeys, auto-retract rules incl. hover, lock/display/suspend handling, second-instance wake. `sohi-core` state machine for the Visibility region + tests. |
| **P3** | Text pill + online LLM | Lazy pill window (EDIT controls, dark, DWM corners), focus rules, runtime thread (lazy), reqwest/SChannel client, Credential Manager + `--set-key`, Mistral streaming chat, SSE parser, streaming into the bubble, Anthropic provider, history ring buffer, cancellation, error mapping. Interaction region (text paths) + tests. |
| **P4** | Voice capture + STT | WASAPI event-driven capture @16k via AUTOCONVERTPCM, rtrb, VAD segmentation with pre-roll and limits, WAV encode, Voxtral transcription, prewarm, level → visor, `dev-inject` feature (WAV file replaces mic) for repeatable tests. |
| **P5** | TTS + speaking | `TtsProvider` trait, online streaming TTS **[VERIFY]**, OneCore offline TTS, WASAPI render thread, SentenceChunker + markdown stripper, envelope → visor, barge-in, half-duplex enforcement, full voice turn end-to-end. |
| **P6** | Connectivity & offline | Connectivity region, IP Helper hint subscription, probes with backoff only while Shown, Force-offline toggle, Ollama detection + NDJSON provider, deterministic intent parser, offline visuals and pill placeholder, online-recovery-on-wake. |
| **P7** | Tools & safety | Tool registry + JSON schemas for Mistral/Anthropic/Ollama, executors (launch_app index, open_url, clipboard, media, volume), policy engine + confirmation UI (keyboard + voice yes/no), `shell` feature (off by default), tool-call round trip in the orchestrator. |
| **P8** | Hardening & release | Full budget pass and fixes, soak test (8 h parked + 60 injected turns), device hot-plug, Explorer restart, DPI/monitor changes, "Start with Windows" (`HKCU\...\Run`), portable zip via `xtask release`, README with setup and key instructions. |

---

## 8. Acceptance Criteria & Verification Checklist

Every phase also requires:
- `cargo xtask check` green (fmt --check, `clippy --workspace --all-targets -D warnings`, `test --workspace`).
- A `cargo xtask perf` report committed under `docs/perf/`.
- No budget regression larger than 5% versus the previous phase without sign-off.
- Every §10.6 task committed and pushed, and the phase PR merged with `--no-ff` and tagged.

### P0
- [ ] `cargo build --release` produces `sohi.exe` ≤ 2 MB (empty shell).
- [ ] A second launch exits within 50 ms and the first instance logs `wake requested`.
- [ ] A forced panic writes a log entry with a backtrace, then the process exits with a non-zero code.
- [ ] `xtask perf` prints: armed time, private bytes, working set, CPU ms/60 s, binary size.
- [ ] Config file is created with commented defaults. A malformed config falls back to defaults and logs a warning (no crash).
- [ ] `.gitignore` covers everything in §10.4. The GitHub remote is set up, and `main` + `phase/p0-scaffold` are pushed.

### P1
- [ ] Every visual state renders correctly at 100%, 150% and 200% scaling (screenshots saved by `xtask snap`).
- [ ] Clicks on transparent pixels reach the window underneath. Clicking the head never activates Sohi (foreground window unchanged).
- [ ] No taskbar button, not in Alt-Tab, stays above a normal topmost-free window.
- [ ] Reveal is visually a slide from off-screen and **no pixels appear on a monitor to the right** (tested on a two-monitor setup).
- [ ] When Parked: the window is hidden and the timer is cancelled. CPU ≤ 60 ms/60 s. Frame render p95 ≤ 1.0 ms.
- [ ] Cold start to first peek frame ≤ 200 ms. Pre-first-wake RAM ≤ 10 MB.

### P2
- [ ] Flicking the cursor to the corner and holding for 350 ms wakes Sohi. Passing through in under 300 ms does not wake it (20 manual trials, 0 false wakes).
- [ ] Dragging a window into the corner (button held) never wakes Sohi.
- [ ] A full-screen D3D game or a presentation suppresses corner wake. The hotkey still works.
- [ ] After a retract, there is no re-wake for 1.5 s even if the cursor stays in the zone.
- [ ] Locking the workstation stops polling (verified by the log and by CPU time).
- [ ] State machine tests cover every Visibility transition, including events that arrive mid-animation.

### P3
- [ ] `Ctrl+Alt+T` opens a focused pill. Typing, IME (test with the Japanese IME), Enter, Esc and ↑ recall all work.
  > SPEC CHANGE (2026-11-22): the Japanese IME test is dropped; the user does not type with an IME. The pill's EDIT
  > controls get IME support from Windows, but it is unverified.
- [ ] A streamed reply appears token by token. First token ≤ 1.0 s p50 for short prompts.
- [ ] Esc mid-stream cancels within 30 ms and the network request is dropped (verified in the log).
- [ ] Missing or invalid key → a clear message in the pill with `--set-key` instructions. The key never appears in logs (grep test).
- [ ] RAM after the pill is destroyed (5 min idle) returns to within 2 MB of the pre-pill value.

### P4
- [ ] Wake → first captured buffer ≤ 350 ms (p95 over 20 trials).

  > SPEC CHANGE (2026-11-22): wake → mic capturing relaxed from 80 ms to 350 ms. The mic still opens only at wake
  > (privacy indicator only while Listening). On SoundWire mics `IAudioClient::Initialize` takes 70–260 ms while the
  > endpoint powers up (first packet 105–310 ms measured); keeping a stream initialized while idle would buy about
  > 37 ms but risks the privacy indicator while idle. Anything said before the first packet is
  > lost. In the second hand test (2026-11-23) speech was found as early as 256 ms into the audio,
  > so a quick speaker can lose the first syllable.
- [ ] The mic privacy indicator shows only while Listening.
- [ ] Injected WAV fixtures: correct segmentation (start/end within ±100 ms of the labelled boundaries), cough-only clip ignored, silence clip → NoSpeechTimeout.
- [ ] Voxtral transcript matches the fixtures (WER sanity check ≤ 15% on the 5 fixture clips).
- [ ] The capture thread performs zero allocations (checked with a counting allocator in a debug test).

### P5
- [ ] End-to-end injected voice turn: end of speech → first audio ≤ 1.5 s p50 / ≤ 2.5 s p95 over 20 turns.
- [ ] The visor animates with the TTS envelope. Markdown and code are never spoken.
- [ ] Barge-in via hotkey: playback stops ≤ 50 ms, a new Listening session starts.
- [ ] Offline OneCore TTS works with the network adapter disabled.
- [ ] After 10 turns plus 60 s parked: private bytes ≤ 20 MB.

### P6
- [ ] Disabling Wi-Fi → next wake shows the Offline visual and the text pill within 200 ms. Re-enabling → the next wake is voice-capable.
- [ ] While parked and offline, there is **no network activity and no probe timers** (verified by log and CPU time).
- [ ] With Ollama running and `offline_model` set, offline text turns stream from Ollama. Without it, the intent parser handles every listed command (unit-tested grammar).
- [ ] Forced offline persists until toggled off and is shown in the tray menu check state.
- [ ] HTTP 401/429/5xx produce the correct distinct user messages and never flip connectivity to Offline.

### P7
- [ ] "Open Notepad", "go to github.com", "pause music", "volume 30" each work by voice and by text (online and offline).
- [ ] `clipboard_get` always asks for confirmation. Denial or timeout produces a "denied" tool result, and the LLM replies gracefully.
- [ ] `shell_exec` is absent from the default build's tool list and binary symbols. In a `shell` build it always confirms.
- [ ] `open_url` rejects `file:`, `javascript:` and UNC paths (unit tests).
- [ ] A prompt-injection fixture inside clipboard text does not trigger an unconfirmed tool call.

### P8
- [ ] All Section 6 budgets pass, and the report is committed under `docs/perf/`.
- [ ] 8 h parked soak: no RAM growth > 1 MB, CPU within budget. 60 injected turns: no leaks, no crashes.

  > SPEC CHANGE (2026-11-24): 60 injected turns instead of 200. The turns use the owner's Mistral account, and a leak per
  > turn shows within 60.

- [ ] Survives: Explorer restart (tray returns), audio device unplug and replug, primary monitor swap, DPI change, sleep/resume.
- [ ] "Start with Windows" toggle works. A portable zip runs on a clean Win11 VM with no runtime installs.
- [ ] `v1.0.0` tag pushed. The README covers setup, key configuration, hotkeys and troubleshooting.

---

## 9. Coding Style, Error-Handling & Concurrency Rules

### 9.1 General
- `rustfmt` default settings. `clippy::pedantic` enabled at warn level, with allowances listed in `clippy.toml`. CI runs with warnings as errors.
- **No `unwrap()`/`expect()`** outside tests. The only exceptions are invariants established in `main` init, and each must have a `// INVARIANT:` comment.
- Functions ≤ 60 lines and modules ≤ 600 lines. Split anything larger.
- `sohi-core` and `sohi-render` are **pure**: no IO, no clock reads (time is passed in), no threads. They are 100% unit-testable,
  and the state machine `step` is exhaustively tested (table-driven tests mirroring Section 5.2/5.3).
- Public items have doc comments. Every `unsafe` block has a `// SAFETY:` comment stating the upheld invariants.
- `unsafe` only in `sohi-win`. It exposes safe RAII wrappers (`OwnedHwnd`, `OwnedHandle`, `ComPtr` drop, `HotkeyRegistration`)
  whose `Drop` releases the resource. Never leak a Win32 handle or COM pointer.
- COM: initialize per thread explicitly (`COINIT_APARTMENTTHREADED` on UI, `COINIT_MULTITHREADED` on audio/runtime) with a guard type.

### 9.2 Errors
- One `thiserror` enum per crate (`WinError`, `AudioError`, `ProviderError`, `ToolError`), each with a
  `fn user_message(&self) -> &'static str` (or a short `String`) for the bubble, and full detail for the log.
- `ProviderError` classifies failures as: `Offline` (DNS/connect/TLS/timeout), `Auth`, `RateLimited`, `Server`, `BadResponse`, `Cancelled`.
  Only `Offline` affects connectivity.
> SPEC CHANGE (2026-11-22): `ProviderError` gains `Refused`: the Anthropic model stopped with `stop_reason: "refusal"`
> before writing any text. It is not retried and does not affect connectivity. A refusal after some text keeps that text.
- Retries: at most **1** automatic retry, and only for idempotent or safe-to-repeat requests (STT upload, probe). Never retry
  a tool execution.
- Timeouts on everything: probe 2 s · STT 10 s · LLM first byte 8 s, inter-chunk 15 s · TTS first byte 5 s · tool 15 s.
- The UI never shows raw error strings, stack traces or HTTP bodies. The log gets them, with secrets redacted.

### 9.3 Concurrency
- **The UI thread never blocks:** no network, no disk IO > 1 ms (config read at startup excepted), no waiting on
  channels. Handlers return in ≤ 4 ms.
- Runtime → UI: push onto a `std::sync::mpsc` queue, then `PostMessageW(hwnd, WM_APP_EVENT)`. The UI drains the whole queue per message.
  UI → runtime: `tokio::sync::mpsc::Sender::try_send` (bounded 64). If it is full, drop non-critical events and log.
- **Audio threads are real-time:** no allocation, no locks, no syscalls besides WASAPI, no logging. Communicate only through
  `rtrb` SPSC rings with fixed capacity (2 s). On overflow, drop the oldest data and increment an atomic counter that the runtime logs.
  > SPEC CHANGE (2026-11-22): on overflow the capture ring drops the *newest* samples (what does not fit). The writer cannot
  > discard the reader's oldest data without a lock; a reader 2 s behind has already broken the turn either way.
- **Exactly one active Turn.** Starting a new turn cancels the previous one first and waits for its cleanup (≤ 30 ms).
- **No busy-waiting and no `sleep` polling.** Timers exist only while their state requires them, and every timer has an owner that cancels it on state exit.
- No `static mut`. Global state is limited to `OnceLock` for immutable config and logger handles.
- Shared mutable state across threads requires a written justification. Prefer message passing.

### 9.4 Dependencies & size discipline
- Every crate is declared once in `[workspace.dependencies]` with `default-features = false` and explicit features.
- Before adding a dependency, provide: purpose, alternatives considered, `cargo bloat` delta, and transitive crate count. Reject it if the size
  delta is > 300 KB without a strong reason.
- `windows` crate features are listed narrowly. Never enable broad feature sets.
- No proc-macro-heavy frameworks, no GUI toolkits, no webviews, no bundled ML runtimes.

### 9.5 Security & privacy
- Keys only in Credential Manager. Redact `Authorization`/`x-api-key` headers in all logs.
- No listening sockets. Ollama is contacted on `127.0.0.1` only.
- Tool outputs and clipboard contents are treated as untrusted data in prompts. Policy enforcement lives in `sohi-tools`, never in the LLM prompt alone.
- Logs are local only, rolling, and contain no audio and no full transcripts at `info` level (transcripts only at `debug`, opt-in).

### 9.6 Testing
- Unit tests: state machine (all transitions), chunker, markdown stripper, SSE/NDJSON parsers (incl. split UTF-8 across chunks),
  intent grammar, URL validator, VAD segmentation with fixtures.
- Integration tests (feature `dev-inject`): WAV fixtures → full turn against a **mock provider server** (local `tokio` test
  server with canned SSE), so CI is deterministic and offline.
- Perf tests: `xtask perf` simulates corner wake with `SetCursorPos`, simulates hotkeys with `SendInput`, injects audio,
  parses the `perf:` lines and fails if any Section 6 budget is exceeded.

---

## 10. Version Control & Commit Discipline

### 10.1 Principles
- **One completed task = one commit.** A task is one row in the phase commit plan (§10.6). When the task's code
  builds, its tests pass and clippy is clean, it gets committed right away. Tasks are never batched into one big commit.
- **Every commit is green.** Before each commit, run `cargo xtask check` (fmt --check, clippy -D warnings, test).
  Broken commits are never pushed.
- **Small and focused.** Aim for 50–400 changed lines per commit. If a task grows bigger, split it into sub-tasks,
  each with its own commit. Refactors, formatting and features always go in separate commits.
- **Push often.** Push to GitHub after every commit, or at the latest at the end of each work session.
  Unpushed work is not "done".
- **History is preserved.** Never rewrite history that has been pushed. Never force-push `main`. Phase branches are merged
  with merge commits (no squashing), so every task commit stays visible on GitHub.

### 10.2 Branching & releases
~~~
main ──●────────────────●─────────────────●──────── … ──●  (tags: v0.0.1, v0.1.0 … v1.0.0)
        \              / \               /
         phase/p0-scaffold   phase/p1-head-widget   …
          ●─●─●─●─●─●─●        ●─●─●─●─●─●─●─●─●─●
~~~
- One branch per phase: `phase/p<N>-<short-name>`, created from the latest `main`.
- At the end of a phase, open a PR titled `Phase N: <name>`. The PR body contains the ticked Section 8 checklist,
  the perf report, and the phase/task references. Merge with `--no-ff`, then tag:
  - P0 → `v0.0.1`
  - P1–P7 → `v0.<N>.0`
  - P8 → `v1.0.0`
- Hotfixes on `main` use `fix/<short-name>` branches, follow the same rules, and bump the patch version.

### 10.3 Commit message format (Conventional Commits)
~~~
<type>(<scope>): <imperative summary, ≤ 72 chars>

<what changed and why, wrapped at 72 chars — written for a human
reader skimming history; one to three sentences is usually enough>

Budget: RAM ±x MB · CPU ±x ms/60s · size ±x KB · latency ±x ms
Co-Authored-By: <AI assistant attribution line specified by the session>
~~~
- **types:** `feat` `fix` `perf` `refactor` `test` `docs` `build` `chore` `ci`
- **scopes:** crate names without the prefix (`core` `render` `win` `audio` `providers` `tools` `app` `xtask`) or `spec`
- **Budget footer is conditional:** include it only when the commit touches a budget-relevant path
  (`perf` commits, new or upgraded dependencies, audio threads, render/frame pacing, startup path, network
  pooling). Omit it everywhere else.
- **Attribution is kept:** every commit the AI assistant creates ends with its `Co-Authored-By` trailer, exactly as
  the session specifies. Commits the user writes by hand don't need it.
- **The body explains the why:** what changed and why, plus any trade-off or rejected alternative. Leave out
  boilerplate, "this commit…" phrasing, and lists of every file changed.
- The phase/task reference goes in the PR description, not in each commit.

### 10.4 Never commit
`target/`, `.env*`, any file containing API keys, user `config.toml`, logs, raw perf dumps > 1 MB,
or audio fixtures > 200 KB each. A `.gitignore` covering these is part of P0 task 1.
Before every commit, check that the diff contains no key-shaped strings (`xtask check` greps for them).

### 10.5 Assistant workflow per task
1. State the task (phase + number from §10.6).
2. Implement it, then run `cargo xtask check` and show the result.
3. Show a short diff summary and the planned commit message.
4. Commit (with attribution trailer), and push to the phase branch. Report the short hash.
5. Update the plan: mark the task done, then name the next one.
At the end of a phase: produce the perf report commit, open the PR, and after the user approves, merge and tag.
Ask before anything outside this flow: force operations, deleting branches, changing remotes, or editing repository settings.

### 10.6 Phase commit plan (≈ 90 commits to v1.0.0)
| Phase | # | Commit (subject) |
|---|---|---|
| **P0** | 1 | `chore: initialize cargo workspace, crate skeletons and .gitignore` |
| | 2 | `build: pin toolchain, add release profile and app manifest` |
| | 3 | `feat(app): add single-instance guard with wake forwarding` |
| | 4 | `feat(app): add rolling file logger and panic hook` |
| | 5 | `feat(core): add config schema with defaults and tolerant loading` |
| | 6 | `feat(app): add perf mode timing lines and armed event` |
| | 7 | `feat(xtask): add check, perf and bloat commands` |
| | 8 | `docs(spec): add master specification and README skeleton` |
| | 9 | `perf(p0): record baseline budget report` → PR, tag `v0.0.1` |
| **P1** | 1 | `feat(win): add RAII handle wrappers and message loop` |
| | 2 | `feat(win): create layered no-activate head window` |
| | 3 | `feat(render): draw head, frame and visor at any DPI` |
| | 4 | `feat(win): blit pixmap via UpdateLayeredWindow` |
| | 5 | `feat(render): implement all animated visual states` |
| | 6 | `feat(win): add high-resolution frame pacing timer` |
| | 7 | `feat(app): add reveal/retract slide and launch peek` |
| | 8 | `feat(win): handle DPI, display and work-area changes` |
| | 9 | `feat(win): add tray icon and menu` |
| | 10 | `test(render): add snapshot harness (xtask snap)` |
| | 11 | `perf(p1): budget report` → PR, tag `v0.1.0` |
| **P2** | 1 | `feat(core): add visibility state machine with tests` |
| | 2 | `feat(win): add adaptive cursor polling` |
| | 3 | `feat(app): add hot zone dwell, cooldown and button guard` |
| | 4 | `feat(win): suppress corner wake for fullscreen apps` |
| | 5 | `feat(win): register global hotkeys` |
| | 6 | `feat(app): add auto-retract rules and hover pause` |
| | 7 | `feat(win): pause on lock, display off and suspend` |
| | 8 | `feat(app): wake running instance on second launch` |
| | 9 | `perf(p2): budget report` → PR, tag `v0.2.0` |
| **P3** | 1 | `feat(app): add lazy runtime thread and UI channels` |
| | 2 | `feat(win): store keys in Credential Manager; add --set-key` |
| | 3 | `feat(providers): add HTTP client with SChannel TLS` |
| | 4 | `feat(providers): add SSE parser with tests` |
| | 5 | `feat(providers): add Mistral streaming chat` |
| | 6 | `feat(win): add text pill window` |
| | 7 | `feat(app): stream replies into bubble` |
| | 8 | `feat(app): add turn cancellation` |
| | 9 | `feat(providers): add Anthropic provider` |
| | 10 | `feat(core): add conversation history ring buffer` |
| | 11 | `feat(providers): map errors to user messages` |
| | 12 | `perf(p3): budget report` → PR, tag `v0.3.0` |
| **P4** | 1 | `feat(win): add WASAPI 16k capture thread` |
| | 2 | `feat(audio): add lock-free capture ring` |
| | 3 | `feat(audio): add VAD segmentation with fixtures` |
| | 4 | `feat(audio): add in-memory WAV encoder` |
| | 5 | `feat(providers): add Voxtral transcription` |
| | 6 | `perf(providers): prewarm connection on wake` |
| | 7 | `feat(render): drive visor from mic level` |
| | 8 | `test(audio): add dev-inject WAV source` |
| | 9 | `perf(p4): budget report` → PR, tag `v0.4.0` |
| **P5** | 1 | `feat(providers): add TtsProvider trait` |
| | 2 | `feat(core): add sentence chunker and markdown stripper` |
| | 3 | `feat(win): add WASAPI render thread` |
| | 4 | `feat(providers): add online streaming TTS` |
| | 5 | `feat(win): add OneCore offline TTS` |
| | 6 | `feat(render): drive visor from TTS envelope` |
| | 7 | `feat(app): add barge-in and half-duplex enforcement` |
| | 8 | `test(app): add end-to-end voice turn test` |
| | 9 | `perf(p5): budget report` → PR, tag `v0.5.0` |
| **P6** | 1 | `feat(core): add connectivity state machine with tests` |
| | 2 | `feat(win): subscribe to network connectivity hints` |
| | 3 | `feat(providers): add probes with shown-only backoff` |
| | 4 | `feat(app): add force-offline toggle` |
| | 5 | `feat(providers): add Ollama detection and NDJSON chat` |
| | 6 | `feat(core): add offline intent parser` |
| | 7 | `feat(render): add offline visuals and pill placeholder` |
| | 8 | `perf(p6): budget report` → PR, tag `v0.6.0` |
| **P7** | 1 | `feat(tools): add registry and JSON schemas` |
| | 2 | `feat(tools): add launch_app with Start Menu index` |
| | 3 | `feat(tools): add open_url with validator` |
| | 4 | `feat(tools): add clipboard tools` |
| | 5 | `feat(tools): add media and volume tools` |
| | 6 | `feat(tools): add policy engine and confirmation UI` |
| | 7 | `feat(app): add voice yes/no confirmation` |
| | 8 | `feat(tools): add feature-gated shell_exec` |
| | 9 | `feat(app): add tool-call round trip in orchestrator` |
| | 10 | `test(tools): add prompt-injection fixtures` |
| | 11 | `perf(p7): budget report` → PR, tag `v0.7.0` |
| **P8** | 1 | `test(app): add 8 h soak and 200-turn stress test` |
| | 2 | `fix(win): recover from device hot-plug and Explorer restart` |
| | 3 | `feat(app): add start-with-Windows toggle` |
| | 4 | `build(xtask): add portable release packaging` |
| | 5 | `docs: write README with setup and key instructions` |
| | 6 | `perf(p8): final budget report` → PR, tag `v1.0.0` |

Extra fix-up commits (`fix`, `refactor`, `test`) between tasks are expected and encouraged. The plan sets the
minimum granularity, not a cap.

---

## Appendix A — [VERIFY] Register (check against current vendor docs before implementing)
1. Voxtral transcription endpoint path, model ID (`voxtral-mini-latest`?), multipart field names, supported audio formats and size limits.
   *(Verified 2026-11-22: `POST /v1/audio/transcriptions`, Bearer auth, multipart fields `file`, `model` and optional
   `language` (ISO 639-1; left out to auto-detect); answer JSON `text`, `language`. `voxtral-mini-latest` is listed
   for the account and transcribes the five P4 fixtures with 0/39 word errors in 215–393 ms; `voxtral-mini-transcribe-3`
   scores the same. A WAV upload works; size limits not yet measured.)*
2. Mistral TTS availability, endpoint, voice IDs, streaming support and output formats (PCM preferred). If unavailable, pick and document an alternative cloud TTS.
   *(Verified 2026-11-23 live: `POST /v1/audio/speech`, Bearer auth, JSON `model`, `input`, `voice_id`,
   `response_format`, `stream`. A voice is required (400 without). `stream: true` answers with server-sent events:
   `speech.audio.delta` with base64 `audio_data`, then `speech.audio.done`. `pcm` is float32 LE, 24 kHz mono. Model
   `voxtral-mini-tts-3`; first audio about 0.4 s. `GET /v1/audio/voices` lists the voices. US-English female:
   Sheila `voice_478`, Margaret `voice_447`, and Jane `gb_jane_*` in several moods, tagged `en_us` but with a `gb_` id.)*
3. Mistral function-calling schema and streaming tool-call delta format.
   *(Verified 2026-11-24 live with `mistral-small-latest`: `tools: [{type: "function", function: {name, description,
   parameters}}]`; all calls arrive whole in one event's `delta.tool_calls` (`id` of 9 letters and digits, `index`,
   `function.name`, `function.arguments` as a JSON string), then `[DONE]`. Results go back as `role: "tool"` with
   `tool_call_id`, after the assistant message with its `tool_calls`.)*
4. Anthropic Messages API current model IDs and streaming event types for tool use. *(Text streaming verified 2026-11-22:
   `POST /v1/messages`, `x-api-key`, `anthropic-version: 2026-10-21` (no `temperature`/`top_p`/`top_k` on that version),
   text in `content_block_delta`/`text_delta`, end at `message_stop`, mid-stream `error` events, `ping`. Current cheap tier:
   `claude-haiku-5-5`.) (Tool use implemented 2026-11-24 from the documentation and tested against mocks only, as no
   Anthropic key is stored: `tool_use` blocks whose input arrives as `input_json_delta`, `tool_result` blocks in one user
   message, the assistant's blocks sent back unchanged, thinking included.)*
5. Ollama `/api/chat` tool-calling support for the chosen offline model. *(2026-11-24: not used. Offline commands run
   through the grammar of §5.3; Ollama answers the rest without tools. Ollama is not installed on the dev laptop.)*
6. ~~Maintenance status of the `webrtc-vad` (or `fvad`) crate and whether it builds under MSVC.~~ *(Resolved
   2026-11-22: unmaintained since 2019; replaced by `earshot`, see §2.2.)*
7. `NotifyNetworkConnectivityHintChange` behaviour on Win11 builds in use.
