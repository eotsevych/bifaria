# Installation Guide for Bifaria

This guide explains how to install Bifaria on macOS, Windows, and Linux.

---

## 1. macOS (Apple Silicon M-series & Intel)

Since version 1.0.7, Bifaria is signed with an Apple Developer ID and notarized by Apple: Apple scans every build for malware before it ships. It opens like any other Mac app, with no "unidentified developer" warning, whichever way you install it.

### Option A: Download the DMG

1. Download **`Bifaria_Universal.dmg`** (works on M1–M4 Apple Silicon and Intel) from [GitHub Releases](https://github.com/eotsevych/bifaria/releases/latest).
2. Double-click the `.dmg` file to open it.
3. Drag **Bifaria** into your **Applications** folder, then open it.

---

### Option B: Homebrew

If you use [Homebrew](https://brew.sh) (a popular Mac package manager):
```bash
brew install --cask eotsevych/tap/bifaria
```

To uninstall later: `brew uninstall --cask bifaria` (add `--zap` to also remove settings and models).

---

### Option C: 1-Line Terminal Install

This command downloads the latest app and installs it to Applications:
```bash
curl -fsSL https://raw.githubusercontent.com/eotsevych/bifaria/main/scripts/install_mac.sh | bash
```

---

### Updates

Bifaria updates itself. It checks once a day, and when a new version is out you get a notification. Install it in either place:
- **Menu bar icon** → **Install Update & Restart**
- **Settings** → **General** → **Updates** → **Install & Restart** (with a download progress bar)

You can also check at any time with **Check for Updates** in either place.

Bifaria keeps its Microphone and Accessibility permissions across updates. The one exception is the update to 1.0.7, the first Apple-signed version: macOS asks for both once more. If pasting stops working after it, remove Bifaria from System Settings → Privacy & Security → Accessibility with **–**, then add it again.

---

## 2. Windows 10 & 11

### Option A: Microsoft Store (Recommended)
Install Bifaria from the **[Microsoft Store](https://apps.microsoft.com/detail/9NVMTPSDZFWR)**. There's no SmartScreen warning, and the Store installs updates for you.

### Option B: Download the Installer
Download `Bifaria_<version>_x64-setup.exe` (installer) or `Bifaria_<version>_x64_en-US.msi` from [GitHub Releases](https://github.com/eotsevych/bifaria/releases).
1. Double-click the downloaded `.exe` file.
2. Follow the setup steps on screen.

#### How to Pass the "SmartScreen" Warning
Microsoft SmartScreen (a Windows security scanner) flags new open-source software:
1. When the blue "Windows protected your PC" window appears, click **More info**.
2. Click **Run anyway**.
3. You only need to do this once.

### Updates
The Store version is updated by the Microsoft Store. The installer version checks for updates once a day and shows a notification when a new version is out. Install it from the tray icon (**Install Update & Restart**) or **Settings** → **General** → **Updates**. No need to download new installers by hand.

---

## 3. Linux (Ubuntu, Debian, Fedora, Arch)

Linux does not show developer warnings. An X11 session is recommended: Wayland restricts global hotkeys and simulated paste.

Requires a recent distribution (glibc 2.39 or newer): Ubuntu 24.04+, Debian 13+, Fedora 40+ or a current rolling release such as Arch.

### Option A: AppImage (All Linux distributions)
1. Download `Bifaria_<version>_amd64.AppImage`.
2. Make it runnable:
   ```bash
   chmod +x Bifaria_*_amd64.AppImage
   ./Bifaria_*_amd64.AppImage
   ```

### Option B: Debian / Ubuntu package (.deb)
1. Download `Bifaria_<version>_amd64.deb`.
2. Install with your package manager:
   ```bash
   sudo apt install ./Bifaria_*_amd64.deb
   ```

### Option C: Fedora / RHEL package (.rpm)
1. Download `Bifaria-<version>-1.x86_64.rpm`.
2. Install it:
   ```bash
   sudo dnf install ./Bifaria-*.x86_64.rpm
   ```

### Updates
The AppImage updates itself (tray icon or **Settings** → **General** → **Updates**). For `.deb` and `.rpm`, install the new package from GitHub Releases.

---

## 4. Permissions Setup

1. **Microphone** (all platforms): Required to capture your voice.
2. **Accessibility** (macOS only): Required to read the global hotkey and paste text.

On first launch, your operating system will ask you to approve these permissions.
On macOS, permissions carry over to future updates, so you grant them only once.

### Apple translation (macOS 26 and later)
**Settings** → **Translations** → **Apple Native** translates on your Mac with Apple's built-in engine: no API key or internet needed. Bifaria runs on macOS 11 and later, but this option needs macOS 26; on older versions it is shown but disabled.

Each language pair is downloaded once. Under **Language packs**, click **Download** next to a language; macOS asks for confirmation in a small window and downloads it. You can also manage them in **System Settings** › **General** › **Language & Region** › **Translation Languages**. Bifaria updates the list when you return to its window, or click **Refresh**.

### Hotkeys
- **macOS**: Single modifier keys (`Right Option`, `Right Control`) or key combos such as `⌘+Shift+Space`.
- **Windows & Linux**: Key combos only. The default is `Ctrl+Shift+Space`.
