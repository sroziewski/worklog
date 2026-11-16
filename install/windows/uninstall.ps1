<#
.SYNOPSIS
    Removes worklog installed by install.ps1: the daily prompt, the Start menu shortcut, the
    Installed apps entry and the program folder. Your settings and history in %APPDATA%\worklog
    stay unless -Purge is given.
#>
[CmdletBinding()]
param(
    [switch]$Purge,
    [switch]$NoSchedule,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\worklog'),
    [string]$StartMenuDir = [Environment]::GetFolderPath('Programs'),
    [string]$ConfigDir = (Join-Path $env:APPDATA 'worklog'),
    [string]$UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\worklog'
)

$ErrorActionPreference = 'Stop'
function Step([string]$text) { Write-Host "==> $text" -ForegroundColor Cyan }
function Ok([string]$text) { Write-Host "    $([char]0x2713) $text" -ForegroundColor Green }
function Warn([string]$text) { Write-Host "    ! $text" -ForegroundColor Yellow }
function Fail([string]$text) { Write-Host "    x $text" -ForegroundColor Red; exit 1 }

$exe = Join-Path $InstallDir 'worklog.exe'
# Deleting the folder is only safe when it is an installation, not, say, the repository.
if ((Test-Path $InstallDir) -and (-not (Test-Path $exe) -or (Test-Path (Join-Path $InstallDir 'Cargo.toml')))) {
    Fail "$InstallDir does not look like a worklog installation; nothing removed"
}
$full = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\') + '\'
$running = Get-Process worklog, worklog-popup -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($full, [StringComparison]::OrdinalIgnoreCase) }
if ($running) { Fail 'worklog is running; close its window and run this again' }

if (-not $NoSchedule -and (Test-Path $exe)) {
    Step 'Daily prompt'
    $out = & $exe uninstall 2>&1
    if ($LASTEXITCODE -eq 0) { Ok ($out | Select-Object -Last 1) }
    else {
        # `worklog uninstall` reads the settings first; broken settings must not leave the task behind.
        schtasks.exe /Delete /F /TN '\Worklog\DailyPrompt' 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { Ok 'removed \Worklog\DailyPrompt' } else { Ok 'no scheduled task' }
    }
}

Step 'Start menu shortcut'
$lnk = Join-Path $StartMenuDir 'Worklog.lnk'
if (Test-Path $lnk) { Remove-Item $lnk -Force; Ok "removed $lnk" } else { Ok 'none' }

Step 'Installed apps entry'
if (Test-Path $UninstallKey) { Remove-Item $UninstallKey -Recurse -Force; Ok 'removed' } else { Ok 'none' }

Step 'Program folder'
if (Test-Path $InstallDir) {
    # This script may be running from that folder; step out of it before deleting.
    Set-Location ([IO.Path]::GetTempPath())
    Remove-Item $InstallDir -Recurse -Force
    Ok "removed $InstallDir"
} else { Ok 'none' }

if ($Purge) {
    Step 'Settings and history'
    if (Test-Path $ConfigDir) { Remove-Item $ConfigDir -Recurse -Force; Ok "removed $ConfigDir" } else { Ok 'none' }
} else {
    Write-Host "    settings and history kept in $ConfigDir (-Purge removes them)" -ForegroundColor DarkGray
}
Write-Host ''
Write-Host '  worklog is uninstalled.' -ForegroundColor Green
