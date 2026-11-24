# Sohi

An ultra-lightweight desktop voice assistant for Windows 11. Sohi stays hidden off-screen at the top-right corner of
the primary display. It slides in when you park the mouse in that corner or press a hotkey, listens, answers aloud and
slides away again. One `sohi.exe` of about 2.3 MB, no runtime to install.

The design and its budgets are in [docs/MASTER_SPEC.md](docs/MASTER_SPEC.md), the measurements in
[docs/perf/](docs/perf/).

## Setup

You need Windows 11 (x64) and a Mistral API key.

1. Unpack `sohi-<version>-windows-x64.zip` where Sohi may stay, for example `C:\Tools\Sohi`.
2. Store your key (next section).
3. Start `sohi.exe`. It shows itself for a moment; its icon is in the notification area.
4. Optional: right-click the icon and choose **Start with Windows**. If you move `sohi.exe` later, choose it again.

The exe is not signed, so Windows may warn when it starts the first time: **More info → Run anyway**.

## API keys

Sohi uses [Mistral](https://console.mistral.ai) to understand speech, to answer and to speak. Copy your key, then run
in PowerShell, in the folder with `sohi.exe`:

```powershell
Get-Clipboard | .\sohi.exe --set-key mistral
```

`.\sohi.exe --set-key mistral` alone asks for the key instead, without showing it. Keys are kept in Windows Credential
Manager, never in the config or the log. To replace one, run the command again; to remove one, delete it in Credential
Manager. It works while Sohi is running.

If Mistral fails, Sohi can ask Anthropic for the answer: store a key with `--set-key anthropic` and set
`anthropic_model` in the config. That path has only been tested against a stand-in for the service.

## Using Sohi

| To | Do |
|---|---|
| talk | `Ctrl+Shift+Space`, or hold the mouse in the top-right corner for a third of a second |
| interrupt a reply, stop listening | `Ctrl+Shift+Space` again, or `Esc` |
| type | `Ctrl+Alt+T`, then `Enter` to send, `↑` for your last message, `Esc` to close |
| go on talking | just speak: after a reply Sohi listens again until 8 s of silence |
| open full mode | say or type "full mode", or use the tray menu |
| be reminded | "remind me in 20 minutes to …", "every weekday at 9 …", "what are my reminders?", "cancel reminder 2" |
| everything else | right-click the tray icon or the head |

**Reminders** are shown, said and sent as a notification, also without the internet. They are kept in
`%APPDATA%\Sohi\reminders.json` as plain text. While the PC is locked or asleep, or Sohi is not running, they wait and come
when you are back. While something is full screen they are shown but not said; `speak = false` under `[reminders]` in the
config never says them. Sohi does not listen after a reminder unless you ask it to.

**Full mode** turns Sohi into a full-screen chat, for longer answers and code. What you type there may get long, formatted
answers; what you say gets short, spoken ones.

| In full mode, to | Do |
|---|---|
| send, break the line | `Enter`, `Shift+Enter` |
| talk, or stop a reply | `Ctrl+M` or the Talk button |
| copy the last code block | `Ctrl+Shift+C` or Copy code |
| see your reminders | `Ctrl+R` or Reminders |
| start a new chat | `Ctrl+N` or New chat |
| leave | say or type "exit full mode", `Esc` when Sohi is idle, or Exit |

Sohi can open apps and web addresses, read and write the clipboard, play, pause and skip music, and change the volume.

**It asks first** before it reads your clipboard, and before everything it wants to do after that in the same turn, as
text on the clipboard could contain instructions. Answer by voice, or with `Enter` (yes) and `Esc` (no). No answer
within 10 s is a no.

**Offline** Sohi takes typed commands only: `open <app>`, `go to <address>`, `copy <text>`, `play`, `pause`, `next`,
`previous`, `mute`, `volume up`, `volume down`, `volume 30`, `what time is it`. With [Ollama](https://ollama.com)
running and `offline_model` set it also answers questions. **Force offline** in the tray menu keeps it that way.

## Settings

Tray menu → **Open config**, or `%APPDATA%\Sohi\config.toml`. It is written with commented defaults when Sohi first
starts; restart Sohi after a change. Useful ones:

| Setting | Meaning |
|---|---|
| `[wake] hotkey_voice`, `hotkey_text` | the hotkeys |
| `[wake] follow_up` | `false`: do not listen again after a reply |
| `[persona] user_name` | what Sohi calls you |
| `[llm] model`, `offline_model` | the Mistral model; an Ollama model |
| `[tts] voice`, `offline_voice` | the Mistral voice; a Windows voice for offline use |
| `[tools] confirm`, `disabled` | tools to ask about first, and tools never to use |

## Privacy

The mic is open only while Sohi listens. What you say or type goes to Mistral, and clipboard text only if you allow it.
The conversation is kept in memory until Sohi quits. The log has times and sizes, not what was said.

## Troubleshooting

| Problem | What to do |
|---|---|
| "No API key is set" or "The API key was rejected" | Store the key again (above). |
| The hotkey does nothing | Another program has it: Sohi says so in a notification. Choose another in the config. |
| "Sohi couldn't use the microphone." | Settings → Privacy & security → Microphone: allow desktop apps. Check the default input device. |
| Sohi cannot hear you well | Settings → System → Sound → Input: raise the volume and choose the right mic. |
| The first syllable is missing | Some mics take up to 0.3 s to start: wait until the head shows that it listens. |
| The corner does nothing | It is off while a full-screen app runs, and for 1.5 s after Sohi hides. |
| No reply, or only the pill appears | You are offline, or **Force offline** is on in the tray menu. |
| "No music or video player is open." | Start the player once; Sohi controls the players Windows shows in its media controls. |
| Sohi does not start with Windows | Settings → Apps → Startup: Sohi must be on. Choose **Start with Windows** again if you moved it. |
| Anything else | Tray menu → **Open log folder**. Only one Sohi runs at a time: starting it again wakes the first. |

To remove Sohi: switch off **Start with Windows**, quit it, and delete its folder, `%APPDATA%\Sohi`,
`%LOCALAPPDATA%\Sohi` and its entries in Credential Manager.

## Building

Requirements:

- Windows 11 x64
- [rustup](https://rustup.rs). `rust-toolchain.toml` pins the exact toolchain, and rustup installs it on first build.
- Visual Studio 2022 Build Tools with the **Desktop development with C++** workload and a Windows 11 SDK

```powershell
cargo build --release
```

| Command | What it does |
|---|---|
| `cargo xtask check` | fmt, clippy with warnings as errors, all tests, and a scan for key-shaped strings. Every commit must pass it. |
| `cargo xtask perf [--idle-secs N] [--report PATH]` | Runs the release build and fails if a budget is exceeded. Quit Sohi first. |
| `cargo xtask soak --wav FILE [--turns N] [--hours H]` | Voice turns with an injected question (uses your Mistral key), then hours of being parked. Runs beside Sohi. |
| `cargo xtask release` | The portable zip, in `target\dist`. |
| `cargo xtask bloat` | Binary size per crate (needs `cargo install cargo-bloat --locked`). |
| `cargo xtask snap` | Pictures of every visual state. |

`cargo build --release --features shell` adds a tool that runs commands. It always asks first, and is never part of a
release.
