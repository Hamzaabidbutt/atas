<#
.SYNOPSIS
    One-shot setup for the ATAS desktop app on Windows.

.DESCRIPTION
    Installs Git, Rust, Node, the MSVC build tools and the WebView2 runtime via
    winget, then pulls project dependencies and offers to launch the app.
    Safe to re-run: every step checks whether it is already satisfied first.

.PARAMETER Check
    Report what is missing and change nothing.

.PARAMETER Run
    Launch the app when setup finishes, without asking.

.EXAMPLE
    .\scripts\setup.ps1
    .\scripts\setup.ps1 -Check
    .\scripts\setup.ps1 -Run
#>
[CmdletBinding()]
param(
    [switch]$Check,
    [switch]$Run
)

$ErrorActionPreference = 'Stop'

function Write-Step { param($Text) Write-Host "`n==> $Text" -ForegroundColor Cyan }
function Write-Ok   { param($Text) Write-Host "  [ok] $Text" -ForegroundColor Green }
function Write-Warn { param($Text) Write-Host "  [!]  $Text" -ForegroundColor Yellow }

# After winget installs something, this PowerShell session still has the old
# PATH. Without refreshing it, a freshly installed git is "not recognized" —
# which is exactly the failure that sends people looking for a second problem
# that does not exist.
function Update-SessionPath {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user    = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = "$machine;$user"
}

function Test-Command {
    param($Name)
    $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

$missing = @()

Write-Step 'Checking winget'
if (Test-Command 'winget') {
    Write-Ok "winget $(winget --version)"
} else {
    Write-Warn 'winget is not available.'
    Write-Warn 'Install "App Installer" from the Microsoft Store, then re-run this script.'
    Write-Warn 'Alternatively install each prerequisite by hand; see README.md.'
    exit 1
}

function Install-WingetPackage {
    param($Id, $Label, [string]$Override)

    if ($Check) { $script:missing += $Label; Write-Warn "$Label missing"; return }

    Write-Host "  installing $Label ..."
    $wingetArgs = @(
        'install', '-e', '--id', $Id,
        '--accept-source-agreements', '--accept-package-agreements'
    )
    if ($Override) { $wingetArgs += @('--override', $Override) }

    winget @wingetArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Warn "$Label install returned $LASTEXITCODE - continuing, but verify it"
    }
    Update-SessionPath
}

# --- Git ------------------------------------------------------------------
Write-Step 'Git'
if (Test-Command 'git') {
    Write-Ok (git --version)
} else {
    Install-WingetPackage -Id 'Git.Git' -Label 'Git'
    Update-SessionPath
    if (Test-Command 'git') { Write-Ok (git --version) } else { $missing += 'Git' }
}

# --- Rust -----------------------------------------------------------------
Write-Step 'Rust'
$minRustMinor = 82
$rustOk = $false
if (Test-Command 'rustc') {
    $version = (rustc --version) -replace '^rustc\s+([0-9.]+).*', '$1'
    $minor = [int]($version.Split('.')[1])
    if ($minor -ge $minRustMinor) {
        Write-Ok "rustc $version"
        $rustOk = $true
    } else {
        Write-Warn "rustc $version is older than 1.$minRustMinor"
        if (-not $Check -and (Test-Command 'rustup')) {
            rustup update stable
            rustup default stable
            $rustOk = $true
            Write-Ok (rustc --version)
        }
    }
}
if (-not $rustOk) {
    Install-WingetPackage -Id 'Rustlang.Rustup' -Label 'Rust'
    Update-SessionPath
    if (Test-Command 'rustc') { Write-Ok (rustc --version) } else { $missing += 'Rust' }
}

# --- Node -----------------------------------------------------------------
Write-Step 'Node'
$minNodeMajor = 20
$nodeOk = $false
if (Test-Command 'node') {
    $nodeVersion = (node --version).TrimStart('v')
    if ([int]($nodeVersion.Split('.')[0]) -ge $minNodeMajor) {
        Write-Ok "node v$nodeVersion"
        $nodeOk = $true
    } else {
        Write-Warn "node v$nodeVersion is older than v$minNodeMajor"
    }
}
if (-not $nodeOk) {
    Install-WingetPackage -Id 'OpenJS.NodeJS.LTS' -Label 'Node LTS'
    Update-SessionPath
    if (Test-Command 'node') { Write-Ok (node --version) } else { $missing += 'Node' }
}

# --- MSVC build tools -----------------------------------------------------
# Rust's default Windows toolchain links with MSVC, so the C++ build tools are
# required even though nothing here is written in C++.
Write-Step 'MSVC build tools'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$haveMsvc = $false
if (Test-Path $vswhere) {
    $installed = & $vswhere -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath 2>$null
    if ($installed) { $haveMsvc = $true; Write-Ok "found at $installed" }
}
if (-not $haveMsvc) {
    Write-Warn 'not found - this download is several GB and takes a while'
    Install-WingetPackage -Id 'Microsoft.VisualStudio.2022.BuildTools' -Label 'VS Build Tools' `
        -Override '--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
}

# --- WebView2 -------------------------------------------------------------
# Present by default on Windows 11; Windows 10 often needs it installed.
Write-Step 'WebView2 runtime'
$webviewKey = 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
if (Test-Path $webviewKey) {
    Write-Ok 'installed'
} else {
    Install-WingetPackage -Id 'Microsoft.EdgeWebView2Runtime' -Label 'WebView2 runtime'
}

if ($Check) {
    Write-Step 'Result'
    if ($missing.Count -eq 0) {
        Write-Ok 'everything needed is present'
        exit 0
    }
    Write-Warn "missing: $($missing -join ', ')"
    exit 1
}

if ($missing.Count -ne 0) {
    Write-Step 'Cannot continue'
    Write-Warn "still missing: $($missing -join ', ')"
    Write-Warn 'Close this window, open a new PowerShell, and run the script again.'
    Write-Warn 'A new session is needed so PATH picks up what was just installed.'
    exit 1
}

# --- Project --------------------------------------------------------------
$repoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $repoRoot

Write-Step 'Project dependencies'
npm install --no-audit --no-fund
if ($LASTEXITCODE -ne 0) { Write-Warn 'npm install failed'; exit 1 }
Write-Ok 'Tauri CLI'

npm --prefix ui install --no-audit --no-fund
if ($LASTEXITCODE -ne 0) { Write-Warn 'UI npm install failed'; exit 1 }
Write-Ok 'UI packages'

Write-Step 'Checking the engine'
cargo test --workspace --quiet
if ($LASTEXITCODE -eq 0) {
    Write-Ok 'engine tests pass'
} else {
    Write-Warn 'engine tests failed - the app may still run, but something is wrong'
}

Write-Step 'Ready'
Write-Host @'
  npm run dev                     launch the app (first build takes 5-10 minutes)
  npm run build -- --no-bundle    build a standalone .exe
  cargo test --workspace          run the engine test suite

  The app starts on a built-in synthetic feed: no network or exchange
  account is needed, and everything on screen is simulated.
'@

if ($Run) {
    Write-Step 'Launching'
    npm run dev
    exit $LASTEXITCODE
}

Write-Host ''
$reply = Read-Host 'Launch it now? [y/N]'
if ($reply -match '^[yY]') {
    npm run dev
} else {
    Write-Host "Run 'npm run dev' when you are ready."
}
