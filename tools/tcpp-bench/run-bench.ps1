# Measures transcribe-cpp (Handy's engine) on this PC: Parakeet Ultra and Parakeet v3, each on the
# GPU (Vulkan) and the CPU, on one of your recordings. Bifaria itself isn't touched.
#
#   powershell -ExecutionPolicy Bypass -File run-bench.ps1 "C:\path\to\recording.m4a"
#   powershell -ExecutionPolicy Bypass -File run-bench.ps1 "C:\path\to\recording.m4a" -MaxMinutes 20
#
# The first run downloads two models (740 MB each) into .\models. Results go to results.txt here,
# and each transcript to a .txt file next to the recording.
param(
  [Parameter(Mandatory = $true)][string]$Audio,
  [double]$MaxMinutes = 0
)
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$models = Join-Path $here "models"
New-Item -ItemType Directory -Force $models | Out-Null
if (-not (Test-Path $Audio)) { throw "Recording not found: $Audio" }

# Pinned to the revisions and checksums in Handy's catalog.
$list = @(
  @{ File = "parakeet-ultra-Q8_0.gguf"; Repo = "handy-computer/parakeet-ultra-gguf"; Rev = "39eeb55181f0d354fd934f06e92fd8d5037fed8e"; Sha = "007a59761e9258779f189df396b50d12b83bb5b79e66df4b955c230d2f2a0a59" },
  @{ File = "parakeet-tdt-0.6b-v3-Q8_0.gguf"; Repo = "handy-computer/parakeet-tdt-0.6b-v3-gguf"; Rev = "85ac09ea12fc4b1112fa76810059364bc6adc9de"; Sha = "5859f77944efcd8eafa23a6350731960b2b55b2203df51f319665c807d802cc7" }
)
foreach ($m in $list) {
  $path = Join-Path $models $m.File
  if (-not (Test-Path $path)) {
    Write-Host "Downloading $($m.File) (740 MB)..."
    & curl.exe -L --fail -o "$path.part" "https://huggingface.co/$($m.Repo)/resolve/$($m.Rev)/$($m.File)"
    if ($LASTEXITCODE -ne 0) { throw "Download failed: $($m.File)" }
    Move-Item -Force "$path.part" $path
  }
  $hash = (Get-FileHash -Algorithm SHA256 $path).Hash.ToLower()
  if ($hash -ne $m.Sha) { Remove-Item $path; throw "Checksum mismatch for $($m.File); deleted it, run again." }
}

# ggml logs to stderr; with "Stop", Windows PowerShell would treat its first line as a failure.
$ErrorActionPreference = "Continue"
$log = Join-Path $here "results.txt"
"tcpp-bench on $env:COMPUTERNAME, $(Get-Date -Format 'yyyy-MM-dd HH:mm'), recording: $(Split-Path -Leaf $Audio)" | Out-File -Encoding utf8 $log
$extra = @()
if ($MaxMinutes -gt 0) { $extra = @("--max-minutes", "$MaxMinutes") }
foreach ($m in $list) {
  foreach ($backend in @("vulkan", "cpu")) {
    Write-Host "`n=== $($m.File) on $backend ===" -ForegroundColor Cyan
    "`n=== $($m.File) on $backend ===" | Out-File -Append -Encoding utf8 $log
    & (Join-Path $here "tcpp-bench.exe") (Join-Path $models $m.File) $Audio --backend $backend @extra 2>&1 |
      Tee-Object -Variable out | ForEach-Object { Write-Host $_ }
    $out | Out-File -Append -Encoding utf8 $log
  }
}
Write-Host "`nDone. Please send results.txt (in $here) and the four .txt transcripts next to your recording." -ForegroundColor Green
