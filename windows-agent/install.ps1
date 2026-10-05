<#
.SYNOPSIS
    RemoteApp Launcher - Windows Guest Agent Installer
.DESCRIPTION
    Compiles remoteapp-launcher.exe, installs to standard program directory,
    registers with Windows Terminal Server TSAppAllowList, and updates PATH.
#>

[CmdletBinding()]
param(
    [string]$InstallDir = "$env:LOCALAPPDATA\Programs\RemoteAppLauncher"
)

$ErrorActionPreference = "Stop"

Write-Host "====================================================" -ForegroundColor Cyan
Write-Host "  RemoteApp Launcher - Windows Guest Agent Installer" -ForegroundColor Cyan
Write-Host "====================================================" -ForegroundColor Cyan

# 1. Check Rust toolchain
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo was not found in PATH. Please install Rust from https://rustup.rs."
}

# 2. Build release binary
Write-Host "[1/4] Building remoteapp-launcher in release mode..." -ForegroundColor Yellow
$AgentDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Push-Location $AgentDir
try {
    cargo build --release
} finally {
    Pop-Location
}

$SourceExe = Join-Path $AgentDir "target\release\remoteapp-launcher.exe"
if (-not (Test-Path $SourceExe)) {
    Write-Error "Build output binary not found at: $SourceExe"
}

# 3. Copy to destination directory
Write-Host "[2/4] Installing binary to '$InstallDir'..." -ForegroundColor Yellow
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

$DestExe = Join-Path $InstallDir "remoteapp-launcher.exe"
# Terminate existing instance if running
Get-Process -Name "remoteapp-launcher" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

Copy-Item -Path $SourceExe -Destination $DestExe -Force
Write-Host "  Installed: $DestExe" -ForegroundColor Green

# 4. Add to User PATH
Write-Host "[3/4] Adding '$InstallDir' to User PATH environment variable..." -ForegroundColor Yellow
$UserPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
if ($UserPath -notlike "*$InstallDir*") {
    $NewPath = "$UserPath;$InstallDir".Trim(';')
    [Environment]::SetEnvironmentVariable("Path", $NewPath, [EnvironmentVariableTarget]::User)
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "  Added to PATH successfully." -ForegroundColor Green
} else {
    Write-Host "  Directory is already in PATH." -ForegroundColor DarkGray
}

# 5. Register in Terminal Server TSAppAllowList
Write-Host "[4/4] Registering application in Terminal Server TSAppAllowList..." -ForegroundColor Yellow
$RegBase = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Terminal Server\TSAppAllowList\Applications"
if (Test-Path "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Terminal Server\TSAppAllowList") {
    try {
        $AppReg = Join-Path $RegBase "RemoteAppLauncher"
        if (-not (Test-Path $AppReg)) {
            New-Item -Path $AppReg -Force | Out-Null
        }
        Set-ItemProperty -Path $AppReg -Name "Name" -Value "RemoteApp Launcher" -Force
        Set-ItemProperty -Path $AppReg -Name "Path" -Value $DestExe -Force
        Set-ItemProperty -Path $AppReg -Name "CommandLineSetting" -Value 1 -Type DWord -Force # Allow parameters
        Set-ItemProperty -Path $AppReg -Name "ShowInTSWA" -Value 0 -Type DWord -Force
        Write-Host "  Registered in TSAppAllowList successfully." -ForegroundColor Green
    } catch {
        Write-Warning "Failed to write to HKLM registry (requires Administrator privileges). Skipping TSAppAllowList registration."
    }
}

Write-Host "`n====================================================" -ForegroundColor Green
Write-Host "  Installation completed successfully!" -ForegroundColor Green
Write-Host "====================================================" -ForegroundColor Green
Write-Host "Agent Path: $DestExe" -ForegroundColor Cyan
Write-Host "`nOn your Linux host, you can update your config by running:" -ForegroundColor White
Write-Host "  rdp-launcher config set remoteapp.default_app '$DestExe'" -ForegroundColor Yellow
Write-Host ""
