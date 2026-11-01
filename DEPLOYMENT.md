# Deployment & Release Guide for Maintainers

This guide explains how to build, package, and publish Bifaria for public distribution.

---

## 1. Where to Store Installers (Git vs. GitHub Releases)

### Do Not Store Installers Directly in Git

Do not commit `.dmg`, `.exe`, or `.deb` files into Git repository commits.

Here is why:
1. **File size limits**: GitHub blocks any file over 100 MB and warns at 50 MB.
2. **Repository bloat**: Git keeps every past copy of binary files forever. A 200 MB release makes the repository large, slow to clone, and difficult to manage.

### The Open-Source Solution: GitHub Releases

Use **GitHub Releases** (a GitHub service to host application download packages).

- **Free storage**: Unlimited download traffic.
- **Fast speed**: Delivered via GitHub CDN (Content Delivery Network - fast global file distribution).
- **Direct links**: Users download straight from your repository "Releases" page.

---

## 2. Developer Warnings (Gatekeeper & SmartScreen)

### Why Do Warnings Happen?

Operating systems (OS - the main computer software) show security warnings on downloaded files:
- **macOS Gatekeeper**: Warns if an app lacks a paid Apple Developer certificate ($99 per year) and Apple Notarization (Apple automated security review).
- **Windows SmartScreen**: Warns if an app lacks a paid Microsoft code signing certificate ($200 to $400 per year).

Being open-source does not stop these warnings automatically. The OS checks for a cryptographic certificate.

---

### Solutions for Open-Source Maintainers

#### Solution 1: Zero Cost (What Bifaria Used Before 1.0.7)
Most open-source tools (like Blender, Audacity, and Ollama when they started) use this method.

1. **Install paths without warnings**: the 1-line Terminal installer and the Homebrew cask both clear the quarantine flag, so Gatekeeper never blocks the app. Promote these first.
2. **Free self-signed certificate (macOS)**: every build is signed with the same "Bifaria Self-Signed" certificate (see [section 5](#5-one-time-setup-free-signing--auto-updates)). This does not remove the Gatekeeper warning, but macOS keeps the Microphone and Accessibility permissions across updates. Without it, builds fall back to ad-hoc signing and users grant permissions again after every update.
3. **In-app updater**: updates downloaded by Bifaria itself are not quarantined, so users pass Gatekeeper only once, at first install.
4. **Document the manual bypass** in [INSTALL.md](INSTALL.md) for DMG and `.exe` downloads:
   - **macOS 15+**: System Settings → Privacy & Security → **Open Anyway**. (Right-click → Open no longer works on Sequoia.)
   - **Windows**: Click **More info** → **Run anyway**.

---

#### Solution 2: Free Certificate for Open Source (Windows)
[SignPath Foundation](https://signpath.org) signs approved open-source projects for free, under its own trusted certificate:
- Apply at `https://signpath.org/apply` **after** the first public release: the application asks for a public repository, an existing release and signs of real usage (downloads, stars, posts).
- Do not use the certificate buttons in a regular SignPath.io account: self-signed certificates there do not help with SmartScreen, and CA certificates cost money.
- Signing runs as a GitHub Actions step on binaries built by CI.

#### Solution 2b: Microsoft Store (Windows, What Bifaria Uses)
Store installs show no SmartScreen warning: Microsoft signs MSIX packages itself on publish, and individual developer accounts are free.
- The product is reserved in [Partner Center](https://partner.microsoft.com/dashboard) as an **MSIX or PWA app** (Store ID `9NVMTPSDZFWR`). Its identity is in `packaging/msix/AppxManifest.xml`.
- CI builds the package with `scripts/build_msix.ps1` and uploads it as the `bifaria-microsoft-store` workflow artifact (not to the GitHub Release: it is unsigned and only installs through the Store).
- From the Store package, Bifaria turns its own updater off (`updater::store_managed`): the Store delivers updates, and its policies forbid apps replacing their own files.

---

#### Solution 3: Official Paid Signing (Commercial Grade)
If you want zero dialogs and zero user warnings:

1. **For macOS**:
   - Buy an Apple Developer account ($99/year).
   - Create a "Developer ID Application" certificate in Xcode or Apple Developer Portal.
   - Sign the app with your certificate:
     ```bash
     codesign --sign "Developer ID Application: Your Name (ID)" "Bifaria.app"
     ```
   - Send the DMG to Apple for Notarization:
     ```bash
     xcrun notarytool submit "Bifaria_Universal.dmg" --keychain-profile "AC_PASSWORD" --wait
     xcrun stapler staple "Bifaria_Universal.dmg"
     ```

2. **For Windows**:
   - Buy a standard code signing certificate (e.g. Sectigo or DigiCert) or use Azure Trusted Signing.
   - Sign the `.exe` and `.msi` in your build script:
     ```cmd
     signtool sign /fd sha256 /tr http://timestamp.digicert.com /td sha256 /f YourCert.pfx /p YourPassword installer.exe
     ```

---

## 3. How to Build Installers Locally

### On macOS (Universal, Apple Silicon & Intel)
Run the automated installer script:
```bash
bash scripts/build_installers.sh
```

This creates:
- `installers/Bifaria_Universal.dmg` (Works on all Macs: M1-M4 and Intel)
- `installers/Bifaria_Apple_Silicon_arm64.dmg` (Apple Silicon only)
- `installers/Bifaria_Intel_x86_64.dmg` (Intel only)
- `installers/Bifaria_Universal.app.tar.gz` + `.sig` (in-app update package; only when the updater key is set, see section 5)

The script signs with the "Bifaria Self-Signed" certificate when it is in your keychain, and prints which identity it used.
To also build the update package locally:
```bash
export TAURI_SIGNING_PRIVATE_KEY_PATH="$HOME/.tauri/bifaria-updater.key"
bash scripts/build_installers.sh
```

### On Windows
Run the Windows build batch script:
```cmd
scripts\build_windows.bat
```
Output files:
- `src-tauri\target\release\bundle\nsis\Bifaria_<version>_x64-setup.exe`
- `src-tauri\target\release\bundle\msi\Bifaria_<version>_x64_en-US.msi`

### On Linux
Run the Linux build script:
```bash
bash scripts/build_linux.sh
```
Output files:
- `src-tauri/target/release/bundle/appimage/Bifaria_<version>_amd64.AppImage`
- `src-tauri/target/release/bundle/deb/Bifaria_<version>_amd64.deb`
- `src-tauri/target/release/bundle/rpm/Bifaria-<version>-1.x86_64.rpm`

---

## 4. How to Publish a Release (Step-by-Step)

### Option A: Manual First Release (Fastest - You Already Have the DMG)

Do not commit the `.dmg` into Git. Follow these exact steps:

#### Step 1: Push Your Code to GitHub
Run this in Terminal:
```bash
# Add your GitHub remote repository (if not added yet)
git remote add origin https://github.com/eotsevych/bifaria.git

# Push the main code branch
git push -u origin main
```

#### Step 2: Locate Your Ready DMG File
You already have the signed universal DMG on your Desktop:
```
~/Desktop/Bifaria_Universal.dmg
```
*(If you ever need to rebuild it, run: `bash scripts/build_installers.sh`)*.

#### Step 3: Create Release on GitHub
1. Open your repository on GitHub in a web browser.
2. Click **Releases** on the right side.
3. Click **Draft a new release**.
4. In **Choose a tag**, type: `v0.1.0` and click **Create new tag**.
5. Set the release title: `Bifaria v0.1.0`.
6. Drag `Bifaria_Universal.dmg` into the **Attach binaries** box.
7. Click **Publish release**.

Both your manual DMG download and the 1-line install command will now work immediately.

---

### Option B: Automated Cloud Release (GitHub Actions)

When you want GitHub servers to build Windows `.exe`, Linux, and macOS packages automatically:

#### Step 1: Bump the Version
Set the same version in all three files:
- `package.json`
- `src-tauri/tauri.conf.json`
- `src-tauri/Cargo.toml`

The workflow fails if the tag does not match the version in `src-tauri/tauri.conf.json`.

#### Step 2: Push a Version Tag
```bash
git tag v0.1.0
git push origin v0.1.0
```

#### Step 3: Automated Cloud Compilation
The workflow in `.github/workflows/build-all-platforms.yml` starts automatically and creates a draft release.

It compiles and attaches:
- macOS `Bifaria_Universal.dmg`, `Bifaria_Apple_Silicon_arm64.dmg`, `Bifaria_Intel_x86_64.dmg`
- Windows `.exe` and `.msi`
- Linux `.AppImage`, `.deb` and `.rpm`
- Updater files: `.sig` signatures, `Bifaria_Universal.app.tar.gz` and `latest.json` (the update manifest the app reads)

Pushes to `main` and pull requests run the same builds without a release. Download their installers from the workflow run's **Artifacts** section.

#### Step 4: Review and Publish
1. Open GitHub -> **Releases**.
2. Click the new draft release created by GitHub Actions.
3. Review the attached installers.
4. Click **Publish release**.

The 1-line macOS installer and the in-app updater both read the latest *published* release, so they do not see drafts. Publishing the release is what rolls the update out to users.

#### Step 5: Update the Homebrew Cask
After publishing, refresh the cask with the new version and checksum, and push it to the tap:
```bash
bash scripts/update_homebrew_cask.sh 0.1.0 ../homebrew-tap
cd ../homebrew-tap && git commit -am "bifaria 0.1.0" && git push
```

#### Step 6: Submit to the Microsoft Store
1. Download the package: `gh run download <run-id> -n bifaria-microsoft-store` (the run built for the version tag).
2. In Partner Center, open Bifaria → **Start submission** (or **Update** on the last one) → **Packages**, and upload `Bifaria_<version>_x64.msix`. Each submission needs a higher version than the last.
3. On the first submission, explain the `runFullTrust` capability: "Bifaria is a desktop dictation app: it registers global hotkeys, records the microphone, and pastes text into the app the user is typing in, which needs a full-trust desktop process."
4. Submit. Certification usually takes from a few hours to a few days.

---

## 5. One-Time Setup: Free Signing & Auto-Updates

Do this once. Keep every file below backed up (for example in a password manager) and never commit it.

### Apple Developer ID (removes the Gatekeeper warning)
With a paid Apple Developer account, the build signs with your Developer ID and gets each `.app` and `.dmg` notarized automatically. Notarization is an automated scan that takes a few minutes per file, not an App Review.

1. Create the certificate (only the Account Holder can): Xcode → Settings → Accounts → Manage Certificates → **+** → **Developer ID Application**. It lands in your login keychain, and `scripts/lib_codesign.sh` picks it over the self-signed one.
2. Create an App Store Connect API key: [appstoreconnect.apple.com](https://appstoreconnect.apple.com) → Users and Access → Integrations → **Team Keys** → **+**, access **Developer**. Note the **Key ID** and the **Issuer ID** above the list, and download the `AuthKey_<KeyID>.p8` file (Apple lets you download it only once). An API key avoids the Apple ID + app-specific password login, which can fail with "401 Invalid credentials" even when both are right.
3. Local builds: `export APPLE_API_KEY="$(cat AuthKey_XXXX.p8)" APPLE_API_KEY_ID=XXXX APPLE_API_ISSUER=<issuer-uuid>` before `bash scripts/build_installers.sh`. Without them the build is signed but not notarized.
4. CI: in Keychain Access, export the "Developer ID Application" certificate with its private key as a `.p12`, then replace the self-signed secrets:
   ```bash
   base64 -i DeveloperID.p12 | gh secret set MACOS_SIGNING_P12
   gh secret set MACOS_SIGNING_P12_PASSWORD   # the .p12 export password
   gh secret set APPLE_API_KEY < AuthKey_XXXX.p8
   gh secret set APPLE_API_KEY_ID
   gh secret set APPLE_API_ISSUER
   ```
5. Check a build: `spctl -a -vv /Applications/Bifaria.app` should say `source=Notarized Developer ID`.

Switching from the self-signed certificate to the Developer ID changes the app's signature, so every user grants Microphone and Accessibility once more after that update. Say so in the release notes.

### A. macOS Self-Signed Certificate (keeps permissions across updates)
```bash
bash scripts/create_macos_signing_cert.sh
```
This creates the "Bifaria Self-Signed" certificate, installs it into your login keychain, and writes the files for CI to `~/.bifaria-signing/`. The first build may ask whether `codesign` may use the key: choose **Always Allow**.

Add it to GitHub so CI signs releases the same way:
```bash
gh secret set MACOS_SIGNING_P12 < ~/.bifaria-signing/bifaria-signing.p12.b64
gh secret set MACOS_SIGNING_P12_PASSWORD < ~/.bifaria-signing/p12-password.txt
```

Use the same certificate forever. A new certificate makes every user grant permissions once more.

### B. Updater Signing Key (required for in-app updates)
Updates are verified with a key pair. The public key is already in `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`); the private key is at `~/.tauri/bifaria-updater.key`.

Add the private key to GitHub:
```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/bifaria-updater.key
```
The key has no password, so `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is not needed.

**If you lose this key, installed copies can never be updated again**: users would have to reinstall manually. To replace it, run `bun tauri signer generate -w ~/.tauri/bifaria-updater.key` and put the new public key in `tauri.conf.json`; only builds made after that can update to builds signed with the new key.

Without these secrets CI still builds everything, just ad-hoc signed and without update files.

---

## 6. Homebrew Tap (One-Time)

1. Create a public GitHub repository named **`homebrew-tap`** (the `homebrew-` prefix is required).
2. After the first release is published, run:
   ```bash
   git clone https://github.com/eotsevych/homebrew-tap.git ../homebrew-tap
   bash scripts/update_homebrew_cask.sh 0.1.0 ../homebrew-tap
   cd ../homebrew-tap && git add Casks/bifaria.rb && git commit -m "Add bifaria" && git push
   ```
3. Users can now run `brew install --cask eotsevych/tap/bifaria`.

The source of truth is `packaging/homebrew/Casks/bifaria.rb` in this repository. The cask removes the quarantine flag after install, so Homebrew users see no Gatekeeper warning.
