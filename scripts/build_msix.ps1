# Packs the Windows release build into an MSIX for the Microsoft Store.
# Run after `tauri build` on Windows. The package is unsigned: the Store signs it on publish.
# Usage: pwsh scripts/build_msix.ps1   → src-tauri/target/release/bundle/msix/Bifaria_<version>_x64.msix
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$release = Join-Path $root "src-tauri/target/release"

# The Store wants four-part versions with the last part 0.
$version = (Get-Content (Join-Path $root "src-tauri/tauri.conf.json") -Raw | ConvertFrom-Json).version
$msixVersion = "$version.0"

$layout = Join-Path $release "msix-layout"
Remove-Item $layout -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path (Join-Path $layout "Assets") | Out-Null

Copy-Item (Join-Path $release "bifaria.exe") (Join-Path $layout "Bifaria.exe")
# Native libraries built next to the app, if any (most are linked statically).
Get-ChildItem $release -Filter *.dll -ErrorAction SilentlyContinue | Copy-Item -Destination $layout

$icons = Join-Path $root "src-tauri/icons"
foreach ($icon in "StoreLogo.png", "Square150x150Logo.png", "Square44x44Logo.png") {
  Copy-Item (Join-Path $icons $icon) (Join-Path $layout "Assets/$icon")
}

(Get-Content (Join-Path $root "packaging/msix/AppxManifest.xml") -Raw).Replace("__VERSION__", $msixVersion) |
  Set-Content (Join-Path $layout "AppxManifest.xml") -Encoding utf8

# makeappx ships with the Windows SDK on GitHub's Windows runners.
$makeappx = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" |
  Sort-Object { [version]$_.Directory.Parent.Name } | Select-Object -Last 1
if (-not $makeappx) { throw "makeappx.exe not found; install the Windows SDK" }

$outDir = Join-Path $release "bundle/msix"
New-Item -ItemType Directory -Path $outDir -Force | Out-Null
$out = Join-Path $outDir "Bifaria_${version}_x64.msix"
& $makeappx.FullName pack /d $layout /p $out /o
if ($LASTEXITCODE -ne 0) { throw "makeappx failed with exit code $LASTEXITCODE" }
Write-Host "Built $out (version $msixVersion)"
