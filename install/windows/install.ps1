<#
.SYNOPSIS
    Installs worklog for the current user: no administrator rights needed.

.DESCRIPTION
    Copies worklog.exe and worklog-popup.exe to %LOCALAPPDATA%\Programs\worklog, adds a
    "Worklog" Start menu shortcut (opens the window for today, also when the day is already
    full), schedules the daily prompt in Task Scheduler and lists worklog under
    Settings > Apps > Installed apps, where it can be uninstalled.

    Run it from a clone of the repository:
        powershell -ExecutionPolicy Bypass -File install\windows\install.ps1
    The release build is made first when it is missing (needs Rust: https://rustup.rs).
    Running it again updates an existing installation; settings are kept.

.PARAMETER Build
    Rebuild (cargo build --release) even when the executables already exist.
.PARAMETER NoSchedule
    Skip the Task Scheduler entry (add it later with: worklog install).
#>
[CmdletBinding()]
param(
    [switch]$Build,
    [switch]$NoSchedule,
    [switch]$NoRegister,
    [string]$Source,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\worklog'),
    [string]$StartMenuDir = [Environment]::GetFolderPath('Programs'),
    [string]$ConfigDir = (Join-Path $env:APPDATA 'worklog'),
    # Where the Installed apps entry goes; a parameter so tests can use a key of their own.
    [string]$UninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\worklog'
)

$ErrorActionPreference = 'Stop'
$repo = Resolve-Path (Join-Path $PSScriptRoot '..\..')
if (-not $Source) { $Source = Join-Path $repo 'target\release' }
$exes = 'worklog.exe', 'worklog-popup.exe'

function Step([string]$text) { Write-Host "==> $text" -ForegroundColor Cyan }
function Ok([string]$text) { Write-Host "    $([char]0x2713) $text" -ForegroundColor Green }
function Warn([string]$text) { Write-Host "    ! $text" -ForegroundColor Yellow }
function Fail([string]$text) { Write-Host "    x $text" -ForegroundColor Red; exit 1 }

Write-Host ''
Write-Host '  worklog installer' -ForegroundColor White
Write-Host "  -> $InstallDir" -ForegroundColor DarkGray
Write-Host ''

Step 'Checking the executables'
$missing = $exes | Where-Object { -not (Test-Path (Join-Path $Source $_)) }
if ($Build -or $missing) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Fail "no build in $Source and cargo is not installed; get Rust from https://rustup.rs and run this again"
    }
    Write-Host '    building the release (a few minutes the first time)...'
    Push-Location $repo
    try { cargo build --release } finally { Pop-Location }
    if ($LASTEXITCODE -ne 0) { Fail 'cargo build --release failed' }
}
foreach ($e in $exes) {
    if (-not (Test-Path (Join-Path $Source $e))) { Fail "$e not found in $Source" }
}
Ok "found in $Source"

# A running copy keeps its exe locked, and replacing it under a scheduled prompt would fail halfway.
$full = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\') + '\'
$running = Get-Process worklog, worklog-popup -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($full, [StringComparison]::OrdinalIgnoreCase) }
if ($running) { Fail 'worklog is running from the install folder; close its window and run this again' }

Step 'Copying the program'
New-Item -ItemType Directory -Force $InstallDir | Out-Null
foreach ($e in $exes) { Copy-Item (Join-Path $Source $e) $InstallDir -Force }
Copy-Item (Join-Path $PSScriptRoot 'uninstall.ps1') $InstallDir -Force
Copy-Item (Join-Path $repo 'assets\icon\worklog.ico') $InstallDir -Force
$exe = Join-Path $InstallDir 'worklog.exe'
Ok $InstallDir

Step 'Settings'
New-Item -ItemType Directory -Force $ConfigDir | Out-Null
$envFile = Join-Path $ConfigDir '.env'
if (Test-Path $envFile) {
    Ok "keeping $envFile"
} elseif (Test-Path (Join-Path $repo '.env')) {
    # The installed exe no longer sits under the repository, where it found this file before.
    Copy-Item (Join-Path $repo '.env') $envFile
    Ok "copied the repository's .env to $envFile"
} else {
    Copy-Item (Join-Path $repo '.env.example') (Join-Path $ConfigDir '.env.example') -Force
    Warn "no .env yet: copy $ConfigDir\.env.example to .env there and fill it in (see README)"
}

Step 'Start menu shortcut'
New-Item -ItemType Directory -Force $StartMenuDir | Out-Null
$lnk = Join-Path $StartMenuDir 'Worklog.lnk'
$shell = New-Object -ComObject WScript.Shell
$s = $shell.CreateShortcut($lnk)
$s.TargetPath = $exe
$s.Arguments = 'popup --force'
$s.WorkingDirectory = $InstallDir
$s.WindowStyle = 7
$s.IconLocation = "$exe,0"
$s.Description = "Log today's work to Jira"
$s.Save()
Ok $lnk

if ($NoSchedule) {
    Step 'Daily prompt'
    Warn 'skipped (-NoSchedule); add it later with: worklog install'
} else {
    Step 'Daily prompt (Task Scheduler)'
    $out = & $exe install 2>&1
    if ($LASTEXITCODE -eq 0) { Ok ($out | Select-Object -Last 1) }
    else { Warn "not scheduled: $(($out | Out-String).Trim())"; Warn "fix the settings, then run: `"$exe`" install" }
}

if (-not $NoRegister) {
    Step 'Installed apps entry'
    $version = ((& $exe --version) -split ' ')[-1]
    $size = [int]((Get-ChildItem $InstallDir -File | Measure-Object Length -Sum).Sum / 1KB)
    New-Item -Force $UninstallKey | Out-Null
    $uninstall = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$InstallDir\uninstall.ps1`" -InstallDir `"$InstallDir`""
    $values = @{
        DisplayName = 'Worklog'; DisplayVersion = $version; Publisher = 'worklog'
        DisplayIcon = "$exe,0"; InstallLocation = $InstallDir; UninstallString = $uninstall
        URLInfoAbout = 'https://github.com/sroziewski/worklog'
    }
    foreach ($k in $values.Keys) { New-ItemProperty -Path $UninstallKey -Name $k -Value $values[$k] -PropertyType String -Force | Out-Null }
    foreach ($k in 'NoModify', 'NoRepair', 'EstimatedSize') {
        $v = if ($k -eq 'EstimatedSize') { $size } else { 1 }
        New-ItemProperty -Path $UninstallKey -Name $k -Value $v -PropertyType DWord -Force | Out-Null
    }
    Ok "Worklog $version"
}

Write-Host ''
Write-Host "  Done. Start it from the Start menu: Worklog" -ForegroundColor Green
Write-Host "  Uninstall: Settings > Apps > Installed apps > Worklog, or $InstallDir\uninstall.ps1" -ForegroundColor DarkGray
Write-Host ''
