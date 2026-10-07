# OpenHop installer for Windows 10 / 11.
#
#   irm https://raw.githubusercontent.com/maykano-dev/open-hop/main/scripts/install.ps1 | iex
#
# Downloads the latest release's setup .exe from GitHub, installs it silently
# for the current user, opens the firewall for private networks and starts OpenHop.

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$Repo = 'maykano-dev/open-hop'
function Info($m) { Write-Host "  > $m" -ForegroundColor Cyan }
function Ok($m)   { Write-Host "  + $m" -ForegroundColor Green }

Write-Host "`nInstalling OpenHop" -ForegroundColor White

Info 'Looking up the latest release...'
try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers @{ 'User-Agent' = 'openhop-installer' }
} catch {
    throw "Couldn't find a published release. Download manually from https://github.com/$Repo/releases"
}
Ok "Latest version: $($release.tag_name)"

$asset = $release.assets | Where-Object { $_.name -match '_x64-setup\.exe$' } | Select-Object -First 1
if (-not $asset) { $asset = $release.assets | Where-Object { $_.name -match '\.msi$' } | Select-Object -First 1 }
if (-not $asset) { throw "No Windows installer in release $($release.tag_name)." }

$file = Join-Path $env:TEMP $asset.name
Info "Downloading $($asset.name)..."
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $file -UseBasicParsing

Get-Process -Name 'openhop-app' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

Info 'Installing...'
if ($file -like '*.msi') {
    Start-Process msiexec.exe -ArgumentList "/i `"$file`" /passive /norestart" -Wait
} else {
    Start-Process -FilePath $file -ArgumentList '/S' -Wait
}
Remove-Item $file -ErrorAction SilentlyContinue

$candidates = @(
    "$env:LOCALAPPDATA\OpenHop\openhop-app.exe",
    "$env:ProgramFiles\OpenHop\openhop-app.exe",
    "${env:ProgramFiles(x86)}\OpenHop\openhop-app.exe"
)
$exe = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1

# Allow OpenHop on private networks (needs admin; skipped quietly otherwise).
if ($exe) {
    $isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if ($isAdmin) {
        Get-NetFirewallRule -DisplayName 'OpenHop' -ErrorAction SilentlyContinue | Remove-NetFirewallRule
        New-NetFirewallRule -DisplayName 'OpenHop' -Direction Inbound -Program $exe -Action Allow -Profile Private | Out-Null
        Ok 'Firewall: allowed on private networks'
    } else {
        Info 'When Windows Firewall asks, allow OpenHop on Private networks.'
    }
    Ok "Installed $exe"
    Start-Process $exe
} else {
    Ok 'Installed. Open OpenHop from the Start menu.'
}

Write-Host "`nNext" -ForegroundColor White
Write-Host '  Set the same passphrase on every computer, and choose "Control Others"'
Write-Host '  on the one whose keyboard and mouse you want to use.'
