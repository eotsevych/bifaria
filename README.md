# Bifaria

<div align="center">

**The fast, private, local-first desktop voice assistant with instant AI translation.**

<img src="site/assets/og.png" alt="Bifaria: speak in any language, paste clean text anywhere" width="720" />

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Windows%20%7C%20Linux-brightgreen.svg)](INSTALL.md)
[![Tauri](https://img.shields.io/badge/Built%20With-Tauri%20v2-orange.svg)](https://v2.tauri.app)
[![React](https://img.shields.io/badge/Frontend-React%2019%20%2B%20Tailwind-blueviolet.svg)](https://react.dev)

[Website](https://eotsevych.github.io/Bifaria/) • [Quick Install](INSTALL.md) • [Deployment Guide](DEPLOYMENT.md) • [Features](#features)

</div>

---

## Overview

**Bifaria** is a lightweight desktop assistant that records your speech, transcribes it locally on your computer in RAM (Random Access Memory), translates foreign languages with AI, and automatically pastes the result directly into your active window.

No cloud audio upload. No browser required. Just tap your hotkey and speak.

<p align="center">
  <img src="site/assets/guide/input.webp" alt="Settings, General tab: launch and translate hotkeys, microphone, audio output and noise reduction" width="49%" />
  <img src="site/assets/guide/translation.webp" alt="Settings, Translations tab: translation mode and AI provider presets" width="49%" />
</p>

---

## Key Features

### 1. Instant Global Hotkey
- **Single-Tap Modifier Keys**: Use `Right Option` (⌥) or `Right Control` (⌃) directly on macOS.
- **Key Combos Everywhere**: Use any shortcut such as `Ctrl+Shift+Space` on Windows and Linux.
- **Press to Start, Press to Finish**: Press the hotkey once to begin recording, press it again to stop and paste. The pill shows a reminder for your first few recordings and after you change a hotkey; turn it off in **Settings** → **General** → **Show hints**.
- **Translate When You Mean To**: **Settings** → **General** → **Translation** sets when Bifaria translates: **Off**, **Separate hotkey** (default: the launch hotkey keeps the language you speak, a second **Translate hotkey** translates to your target language) or **Automatic** (the launch hotkey translates anything not in the target language). A translating recording shows on the pill in teal with the target language, such as `EN`; automatic translation shows an outlined `AUTO→EN`. What you actually said is kept: **Second Try** on a translated recording in **History** shows it next to the translation in Audio Labs, ready to copy.
- **Retry What Failed**: A recording whose transcription or translation failed is marked in **History**. **Retry in Audio Lab** opens it there: transcribe the saved recording again or translate the text again, then **Save to history** replaces the entry. The most recent failed translation can also be retried from the pill or the tray menu.
- **Global Cancel**: Tap `Escape` at any time to instantly stop recording or cancel translation.
- **Micro-Sounds**: Subtly chimes on recording start and completion.

### 2. Local-First In-RAM Speech Recognition
- **Zero Cloud Audio**: Audio records directly into computer memory (RAM). Zero audio files are saved to disk during speech.
- **Parakeet TDT 0.6B**: Runs on Apple Neural Engine (ANE) for ultra-fast, near-zero latency English recognition.
- **Whisper Models**: Bundled with `whisper.cpp` (Medium `q5_0`, Small, Base) for high accuracy multilingual recognition offline.
- **Smart RAM Management**: Automatically unloads models from memory when idle to conserve system memory.

### 3. Voice Activity Detection (VAD)
- **Silero VAD**: Analyzes speech locally via ONNX Runtime.
- **Smart Silence Trimming**: Strips out background silence and pauses.
- **Speech Protection Margins**: Maintains a 400 millisecond safety buffer before and after speech to protect whispers and soft consonants.

### 4. 0.0001s Instant Skip Engine
- **Instant Language Check**: Checks the recognized language locally before calling the network.
- **Zero-Latency Bypass**: If you already speak your target language (e.g. English), or you set a specific **Spoken language** and speak a different one, translation is bypassed in **0.0001 seconds** with zero API calls.
- **Excluded Languages**: Languages you don't speak but that sound close to yours (for example Russian for Ukrainian speakers). If Whisper detects one by mistake, Bifaria transcribes the speech again in your language.

### 5. Multi-Provider AI Translation
- **Google Gemini**: Fast, fluent translation powered by `gemini-3.6-flash`.
- **Local LLMs**: Connects to local AI models via Ollama or LM Studio (e.g. `llama3.2`, `mistral`).
- **Custom Endpoints**: Compatible with any OpenAI-style REST API endpoint with custom system prompt templates.
- **Apple Native**: macOS 26 and later. Uses Apple's built-in Translation framework, the engine behind the Translate app: no API key, no cost, works offline, and nothing leaves your Mac. Each language pair is downloaded once, from **Settings** → **Translations** → **Language packs** (or System Settings › General › Language & Region › Translation Languages). The **Fast** model translates a sentence in tens of milliseconds; **Best quality** takes about half a second. Apple translates as-is, so keep an LLM for "translate and also rephrase". If Apple Translation fails (for example, a pair that isn't downloaded), Bifaria pastes what you said, or translates with your LLM provider or Apple Intelligence if you choose that as the fallback.
- **Apple Intelligence**: macOS 26 and later, with Apple Intelligence turned on. Translates with Apple's on-device language model and follows your prompt template, so it can also fix grammar or change the tone. Free and offline; slower than Apple Native (about a second per dictation).
- **Offline (on-device)**: Windows, Linux and Macs older than macOS 26. Translates with the models behind Firefox's translations, on your CPU: no API key, no cost, works offline, and nothing leaves the computer. Each language is a 20–55 MB download from **Settings** → **Translations** → **Language packs**. Every model translates to or from English, so a pair like Ukrainian → German goes through English and needs both. Needs a processor with AVX2 (x86) or i8mm (ARM, e.g. M2 and later). Like Apple Native, it can fall back to your LLM provider.

### 6. Automatic Text Normalization
- **Cleaner Transcripts**: Removes verbal stuttering, duplicate words, and spoken filler phrases ("um", "uh", "you know", "like").
- **Smart Formatter**: Automatically converts spoken numbers, currency ($100), dates, times, and web addresses (URLs) into standard written text.
- **False-Start Cleanup**: Corrects spoken corrections automatically (e.g., *"I think, sorry, I know"* becomes *"I know"*).

### 7. Confidentiality Guard & Data Masking
- **Sensitive Data Shield**: Masks personal names, project codenames, credit card numbers, phone numbers, and email addresses before sending text to external translation APIs.
- **Custom Replacement Tokens**: Replaces sensitive data with tags like `[CONFIDENTIAL]` or `***`.

### 8. Floating Animated Voice Pill
- **Always-on-Top Minimal HUD**: Small floating indicator that stays above other windows with transparent glass design.
- **4 Visual States**:
  - **Listening**: Real-time equalizer bars reacting to your microphone audio volume, with what happens to your speech (kept in your language, translated, or auto-translated) and, for your first few recordings, which key finishes it.
  - **Transcribing**: Three pulsing dots while Whisper or Parakeet runs locally in RAM.
  - **Translating**: Pulsing violet glow displaying language direction (`Auto → English`).
  - **Done**: Quick emerald green checkmark as text pastes.
- **Draggable**: Drag the pill anywhere on your screen.

### 9. Automatic Text Paste
- **Direct App Insertion**: Copies translated text to system clipboard and simulates keyboard paste (`Cmd+V` on macOS, `Ctrl+V` on Windows/Linux) into your active cursor position.
- **Keep My Clipboard**: **Settings** → **General** → **Keep my clipboard** puts back what was on the clipboard (text, images, files, rich text) half a second after the paste, so what you copied is still what `Cmd+V` pastes next. macOS and Windows get back every format; Linux gets back the richest one (files, then an image, then HTML, then text). If you copy something else in that half second, it is kept; if the paste may not have gone through, the dictated text stays for you to paste.
- Works in any application: Slack, Telegram, WhatsApp, VS Code, Google Docs, Chrome, Terminal, and more.

### 10. Privacy & History Storage Controls
- **3 History Modes**:
  - *Text Only (Default)*: Stores only text transcripts in local SQLite database.
  - *Text + Audio*: Saves high-efficiency Opus voice recordings alongside text.
  - *Private Mode*: Stores everything in RAM only. Zero history is written to disk.
- **Auto-Pruning**: Automatic 500 MB storage cap (FIFO auto-cleanup) and 30-day retention schedule.

---

## Architecture

```
[ Microphone Stream ]
         │
         ▼ (16 kHz mono float32 into RAM)
[ Silero VAD ] ── (Trims silence, preserves 0.4s margins)
         │
         ▼
[ Local Speech Recognition ] ── (Parakeet TDT / Whisper Medium q5_0 in RAM)
         │
         ▼
[ Language & Skip Check ] ── (0.0001s: if already target language) ──► [ Direct Auto-Paste ]
         │ (If foreign speech)
         ▼
[ Confidentiality Masker ] ── (Hides emails, cards, and custom secrets)
         │
         ▼
[ AI Translation Engine ] ── (Google Gemini / Local Ollama / Custom API / Apple on-device)
         │
         ▼
[ Auto-Paste (Cmd+V / Ctrl+V) ] + [ Local SQLite History ]
```

---

## Installation

See **[INSTALL.md](INSTALL.md)** for detailed installation steps.

### macOS Quick Install (1-Line Terminal Command)

Bifaria is signed and notarized by Apple, so it opens without security warnings. Download `Bifaria_Universal.dmg` from [Releases](https://github.com/eotsevych/bifaria/releases/latest), or run this in Terminal to download and install it:

```bash
curl -fsSL https://raw.githubusercontent.com/eotsevych/bifaria/main/scripts/install_mac.sh | bash
```

Or with [Homebrew](https://brew.sh):

```bash
brew install --cask eotsevych/tap/bifaria
```

### Windows

Get Bifaria from the **[Microsoft Store](https://apps.microsoft.com/detail/9NVMTPSDZFWR)**: no SmartScreen warning, and the Store keeps it updated.

On macOS and Linux, Bifaria updates itself: you get a notification when a new version is out, and install it from the menu bar, tray or Settings.

### Manual Downloads

| Platform | Recommended Installer | System Architecture |
|---|---|---|
| **macOS** | `Bifaria_Universal.dmg` | Apple Silicon (M1–M4) & Intel (`x86_64`) |
| **Windows** | [Microsoft Store](https://apps.microsoft.com/detail/9NVMTPSDZFWR), or `Bifaria_<version>_x64-setup.exe` / `.msi` | 64-bit Windows 10 & 11 |
| **Linux** | `Bifaria_<version>_amd64.AppImage`, `.deb` or `.rpm` | 64-bit Linux (Ubuntu, Debian, Fedora) |

All installers are published on [GitHub Releases](https://github.com/eotsevych/bifaria/releases).

### Linux: Pasting and Hotkeys

**Pasting.** Bifaria sends Ctrl+V with whichever of these tools is installed, and falls back to its built-in method (which on Wayland reaches only XWayland apps):

| Session | Tools, in the order Bifaria tries them | Install (Ubuntu/Debian) |
|---|---|---|
| X11 | `xdotool`, `ydotool` | `sudo apt install xdotool` |
| Wayland (GNOME, KDE) | `dotool`, `ydotool` | `sudo apt install ydotool`, then make sure its daemon `ydotoold` runs (how depends on the distro's package) |
| Wayland (Sway, Hyprland, others) | `wtype`, `dotool`, `ydotool` | `sudo apt install wtype` |

On Wayland, also install `wl-clipboard` (`wl-copy`) so native Wayland apps see the copied text. If Bifaria can't paste, the pill says so, and the text stays on the clipboard for Ctrl+V.

**Hotkeys on Wayland.** Wayland doesn't let apps register global hotkeys. Add custom shortcuts in your desktop settings that run these commands instead:

| Command | Does |
|---|---|
| `bifaria --toggle` | Starts or stops a recording, like the main hotkey |
| `bifaria --translate` | Starts or stops a recording that is translated |
| `bifaria --cancel` | Cancels the recording in progress |

- **GNOME:** Settings › Keyboard › Keyboard Shortcuts › Custom Shortcuts › **+**, with the command above.
- **KDE Plasma:** System Settings › Shortcuts › **Add New** › **Command or Script**.
- **Sway / i3:** `bindsym Mod4+o exec bifaria --toggle` in your config.

For the AppImage, use the AppImage's full path instead of `bifaria`. The commands work on macOS and Windows too, for example from Raycast or a Stream Deck.

---

## Development & Build

### Prerequisites
- Node.js or [Bun](https://bun.sh)
- Rust & Cargo (1.77.2+)
- CMake and Clang (for the Whisper build)
- macOS only: Xcode or the Command Line Tools with the **macOS 26 SDK** (Xcode 26+), for the Apple Translation helper. `src-tauri/build.rs` builds it with `scripts/build_apple_translate.sh` (Swift, universal arm64 + x86_64) and bundles it next to the app as `bifaria-translate`. With an older SDK the build still works, but the Apple provider reports that it isn't included; set `BIFARIA_REQUIRE_APPLE_TRANSLATE=1` to fail the build instead (release builds do). `scripts/test_apple_translate.sh` smoke-tests the helper.
- Linux only: the system packages listed in `scripts/build_linux.sh`

### Run in Development
```bash
# Install frontend packages
bun install

# Start desktop app in dev mode
bun run tauri dev
```

### Build Production Packages
```bash
# macOS: Universal, Apple Silicon and Intel DMGs (output: installers/)
bash scripts/build_installers.sh

# Windows: .exe and .msi installers
scripts\build_windows.bat

# Linux: .AppImage, .deb and .rpm packages
bash scripts/build_linux.sh
```

Each platform is built on its own OS. GitHub Actions builds all three on every push; see [DEPLOYMENT.md](DEPLOYMENT.md) for releases.

---

## License

This project is licensed under the [MIT License](LICENSE).

Linux pasting (`linux_paste.rs`) and the Linux memory fix (`memory.rs`) are adapted from [Handy](https://github.com/cjpais/Handy), MIT License, Copyright (c) 2025 CJ Pais.
