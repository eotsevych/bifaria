# Homebrew cask for Bifaria, published through the tap repo github.com/eotsevych/homebrew-tap.
# Users install with:  brew install --cask eotsevych/tap/bifaria
#
# After each release, run scripts/update_homebrew_cask.sh <version> to refresh version and sha256,
# then copy this file to Casks/bifaria.rb in the tap repo (see DEPLOYMENT.md).
cask "bifaria" do
  version "1.0.4"
  sha256 "09712f4b51a1f6f3ed2163f8c9f5b13fdab4678e0dbba98b7c7f17cd23b0051a"

  url "https://github.com/eotsevych/bifaria/releases/download/v#{version}/Bifaria_Universal.dmg"
  name "Bifaria"
  desc "Local-first voice dictation and translation"
  homepage "https://bifaria.com/"

  livecheck do
    url :url
    strategy :github_latest
  end

  # The app updates itself in place; brew should not fight it.
  auto_updates true
  depends_on :macos

  app "Bifaria.app"

  uninstall quit: "com.bifaria.desktop"

  zap trash: [
    "~/Library/Application Support/com.bifaria.desktop",
    "~/Library/Application Support/bifaria",
    "~/Library/Caches/com.bifaria.desktop",
    "~/Library/LaunchAgents/Bifaria.plist",
    "~/Library/Preferences/com.bifaria.desktop.plist",
    "~/Library/Saved Application State/com.bifaria.desktop.savedState",
    "~/Library/WebKit/com.bifaria.desktop",
  ]
end
