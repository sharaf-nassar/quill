<#
    Launches a sandboxed Quill instance against pre-seeded dummy data so a maintainer
    can capture marketing-site screenshots without touching their personal Quill state.

    Contract: specs/001-marketing-site/contracts/launcher-cli.md

    Usage:
        scripts/run_quill_demo.ps1 [-Clean] [-Bin PATH] [-KeepOnExit]

    Exit codes:
        0   demo Quill exited cleanly
        1   bad argument(s)
        2   sandbox setup failed
        3   seeder failed
        4   no Quill binary found
        >4  forwarded from the Quill child process
#>

[CmdletBinding()]
param(
    [switch]$Clean,
    [string]$Bin = "",
    [switch]$KeepOnExit
)

$ErrorActionPreference = "Stop"

# ── Resolve repo root ─────────────────────────────────────────────────────────

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..") | Select-Object -ExpandProperty Path

# Sandbox path is stable per user so re-running the launcher reuses the same dataset.
$User = $env:USERNAME
if (-not $User) { $User = [Environment]::UserName }
$Sandbox = Join-Path $env:TEMP "quill-demo-$User"

# ── Locate Quill binary ───────────────────────────────────────────────────────

function Find-QuillBin {
    if ($Bin) {
        if (Test-Path $Bin -PathType Leaf) { return (Resolve-Path $Bin).Path }
        return $null
    }
    $cmd = Get-Command quill -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    foreach ($candidate in @(
        (Join-Path $RepoRoot "src-tauri\target\release\quill.exe"),
        (Join-Path $RepoRoot "src-tauri\target\debug\quill.exe")
    )) {
        if (Test-Path $candidate -PathType Leaf) { return $candidate }
    }
    return $null
}

$QuillBin = Find-QuillBin
if (-not $QuillBin) {
    Write-Error "[demo] ERROR: no Quill binary found."
    Write-Error "[demo] Tried: -Bin override, PATH, $RepoRoot\src-tauri\target\{release,debug}\quill.exe"
    Write-Error "[demo] Install Quill or build it first:  cargo build --release --manifest-path src-tauri\Cargo.toml"
    exit 4
}

# ── Sandbox prep ──────────────────────────────────────────────────────────────

if ($Clean -and (Test-Path $Sandbox)) {
    try { Remove-Item -Recurse -Force $Sandbox }
    catch { Write-Error "[demo] ERROR: could not clean $Sandbox"; exit 2 }
}

try {
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "data")     | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "rules")    | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $Sandbox "projects") | Out-Null
}
catch {
    Write-Error "[demo] ERROR: could not create sandbox dirs under $Sandbox"
    exit 2
}

$env:QUILL_DEMO_MODE            = "1"
$env:QUILL_DATA_DIR             = Join-Path $Sandbox "data"
$env:QUILL_RULES_DIR            = Join-Path $Sandbox "rules"
$env:QUILL_CLAUDE_PROJECTS_DIR  = Join-Path $Sandbox "projects"

Write-Host "[demo] sandbox at $Sandbox"
Write-Host "[demo] data:     $env:QUILL_DATA_DIR"
Write-Host "[demo] rules:    $env:QUILL_RULES_DIR"
Write-Host "[demo] projects: $env:QUILL_CLAUDE_PROJECTS_DIR"

# ── Seed ──────────────────────────────────────────────────────────────────────

$seederArgs = @(
    (Join-Path $RepoRoot "scripts\populate_dummy_data.py"),
    "--data-dir",     $env:QUILL_DATA_DIR,
    "--rules-dir",    $env:QUILL_RULES_DIR,
    "--projects-dir", $env:QUILL_CLAUDE_PROJECTS_DIR,
    "--no-backup",
    "--quiet"
)
$seeder = Start-Process -FilePath "python3" -ArgumentList $seederArgs -NoNewWindow -PassThru -Wait
if ($seeder.ExitCode -ne 0) {
    Write-Error "[demo] ERROR: seeder failed; sandbox left at $Sandbox for inspection"
    exit 3
}

# ── Launch Quill ──────────────────────────────────────────────────────────────

Write-Host "[demo] launching $QuillBin ..."

$child = Start-Process -FilePath $QuillBin -NoNewWindow -PassThru -Wait
$childRc = $child.ExitCode

# ── Teardown hint ─────────────────────────────────────────────────────────────

if (-not $KeepOnExit) {
    Write-Host ""
    Write-Host "[demo] sandbox preserved at $Sandbox"
    Write-Host "[demo] to clean up:  Remove-Item -Recurse -Force $Sandbox"
}

exit $childRc
