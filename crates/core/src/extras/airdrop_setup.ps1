# OpenHop: set up AirDrop on Windows (run as administrator; OpenHop starts it).
#
# AirDrop needs Apple's own Wi-Fi link (AWDL). Windows can't drive a Wi-Fi
# card that way, so this runs omdrop + omdrop-owl in Linux under WSL2, on a
# MediaTek MT7612U USB adapter handed to Linux with usbipd-win. Steps:
#   1. WSL2 with Ubuntu       2. usbipd-win
#   3. a WSL kernel with the adapter's driver (built here, ~20-40 minutes)
#   4. omdrop-owl and omdrop in Ubuntu
#   5. the adapter handed to Linux, and a check
# Safe to run again: finished steps are skipped.
param([string]$Folder, [string]$Adapter = "0e8d:7612")

$ErrorActionPreference = "Stop"
$Host.UI.RawUI.WindowTitle = "OpenHop: setting up AirDrop"
function Say($t) { Write-Host ""; Write-Host "== $t" -ForegroundColor Cyan }
function Fail($t) { Write-Host ""; Write-Host $t -ForegroundColor Red; Read-Host "Press Enter to close"; exit 1 }
function Wsl([string[]]$a) { & wsl.exe @a; if ($LASTEXITCODE -ne 0) { throw "wsl $($a -join ' ') failed ($LASTEXITCODE)" } }

try {
New-Item -ItemType Directory -Force -Path $Folder | Out-Null
Remove-Item -ErrorAction SilentlyContinue (Join-Path $Folder "ready")

Say "1/5  Linux (WSL2) with Ubuntu"
$distros = ((& wsl.exe -l -q) -join "`n") -replace "`0", ""
if (-not ($distros -match "\S")) {
  & wsl.exe --install -d Ubuntu --no-launch
  Write-Host ""
  Write-Host "Ubuntu is being installed. If Windows asks to restart, restart." -ForegroundColor Yellow
  Write-Host "Then open Ubuntu from the Start menu once to create your Linux user, and press Set Up in OpenHop again."
  Read-Host "Press Enter to close"; exit 0
}
& wsl.exe --set-default-version 2 | Out-Null
$user = ((& wsl.exe -e whoami) -join "").Trim()
if (-not $user -or $user -eq "root") { Fail "Open Ubuntu from the Start menu once to create your Linux user, then press Set Up in OpenHop again." }
Write-Host "Ubuntu is ready (user $user)."

Say "2/5  usbipd-win (hands the USB adapter to Linux)"
$usbipd = Join-Path $env:ProgramFiles "usbipd-win\usbipd.exe"
if (-not (Test-Path $usbipd)) {
  & winget install --exact --id dorssel.usbipd-win --accept-source-agreements --accept-package-agreements --silent
  if (-not (Test-Path $usbipd)) { Fail "usbipd-win didn't install. Install it from https://github.com/dorssel/usbipd-win/releases and press Set Up again." }
}
Write-Host "usbipd-win is installed."

Say "3/5  Linux kernel with the adapter's driver, and 4/5 omdrop (this takes a while)"
$sh = (Join-Path $Folder "setup.sh")
$shL = ((& wsl.exe -e wslpath -a ($sh -replace '\\', '/')) -join "").Trim()
$folderL = ((& wsl.exe -e wslpath -a ($Folder -replace '\\', '/')) -join "").Trim()
$downloads = (New-Object -ComObject Shell.Application).NameSpace('shell:Downloads').Self.Path
if (-not $downloads) { $downloads = Join-Path $env:USERPROFILE "Downloads" }
$downloadsL = ((& wsl.exe -e wslpath -a ($downloads -replace '\\', '/')) -join "").Trim()
Wsl @("-u", "root", "-e", "bash", $shL, "root", $folderL, $user)

# The kernel, for every Linux under WSL (.wslconfig, kept otherwise as it was).
$kernel = Join-Path $Folder "bzImage"
if (-not (Test-Path $kernel)) { Fail "The kernel wasn't built (see above)." }
$cfg = Join-Path $env:USERPROFILE ".wslconfig"
$line = "kernel=" + ($kernel -replace '\\', '\\')
$text = if (Test-Path $cfg) { Get-Content -Raw $cfg } else { "" }
if ($text -notmatch "(?m)^\[wsl2\]") { $text = $text.TrimEnd() + "`r`n[wsl2]`r`n" }
$text = [regex]::Replace($text, "(?m)^kernel=.*\r?\n?", "")
$text = [regex]::Replace($text, "(?m)^\[wsl2\]\r?\n", "[wsl2]`r`n$line`r`n")
Set-Content -Path $cfg -Value $text -Encoding ASCII
& wsl.exe --shutdown
Start-Sleep 3
$k = ((& wsl.exe -e uname -r) -join "").Trim()
if ($k -notmatch "openhop") { Fail "Linux didn't start with the new kernel ($k). Check $cfg." }
Write-Host "Linux runs the kernel with the adapter's driver ($k)."

# omdrop's own set-up, as the user (it may ask for the Linux password).
Wsl @("-e", "bash", $shL, "user", $folderL, $user, $downloadsL)

Say "5/5  The USB Wi-Fi adapter"
while ($true) {
  $list = (& $usbipd list) -join "`n"
  if ($list -match [regex]::Escape($Adapter)) { break }
  Write-Host "Plug in the MediaTek MT7612U USB Wi-Fi adapter (any brand that uses this chip)." -ForegroundColor Yellow
  Read-Host "Then press Enter"
}
& $usbipd bind --hardware-id $Adapter
Start-Process -WindowStyle Hidden wsl.exe -ArgumentList "-e","sleep","60"
Start-Sleep 2
& $usbipd attach --wsl --hardware-id $Adapter
Start-Sleep 4
Wsl @("-e", "bash", $shL, "check", $folderL, $user)

Set-Content -Path (Join-Path $Folder "ready") -Value $Adapter -Encoding ASCII
Write-Host ""
Write-Host "AirDrop is set up. Keep the adapter plugged in." -ForegroundColor Green
Write-Host "Send with OpenHop > AirDrop sends to iPhones and Macs nearby; Settings > Sharing > AirDrop lets them send here."
Read-Host "Press Enter to close"
} catch {
  Fail ("Set-up stopped: " + $_.Exception.Message)
}
