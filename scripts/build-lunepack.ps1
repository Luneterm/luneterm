# Builds the lunepack binaries the Windows release needs, from the source at the exact revision
# Luneterm pins (the `lunepack` dependency in Cargo.toml): the runtime, the launcher stub and the
# update window's UI. The same job as scripts/build-lunepack.sh for the other OSes; it works on
# Windows PowerShell 5.1 (the release runner has no pwsh), which lunepack's own
# scripts/release/build.ps1 does not.
#
#   powershell -File scripts/build-lunepack.ps1 -OutDir <dir>
#
# <OutDir> must not exist or be empty. It gets lunepack-runtime.exe, lunepack-stub.exe,
# lunepack-ui.exe and lunepack-rev.txt. The checkout goes to target\lunepack-src-<rev12> (reused when
# clean at the pinned revision, else the script stops: nothing is deleted), the build to
# target\lunepack-target. Before anything is built, `cargo metadata --locked` of the checkout must
# show every package from crates.io, the checkout itself, or the one git source lunepack's own
# boundary check allows (the luneui-rs toolkit its UI links, at a full commit): this code ships
# inside every user's install, and scripts/check-deps.sh covers only Luneterm's own graph. Each binary is checked as
# lunepack's release script checks it (a PE of the x86-64 machine, the subsystem it needs).
param(
    [Parameter(Mandatory = $true)][string]$OutDir
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Run([string]$what, [scriptblock]$cmd) {
    & $cmd
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$OutDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutDir)
if (Test-Path -LiteralPath $OutDir) {
    if (-not (Test-Path -LiteralPath $OutDir -PathType Container) -or @(Get-ChildItem -Force -LiteralPath $OutDir).Count -ne 0) {
        throw "$OutDir exists and is not an empty directory"
    }
}

# The pin: the one `lunepack = { git = ..., rev = ... }` line of the root manifest.
$pin = Select-String -LiteralPath (Join-Path $root 'Cargo.toml') -Pattern '^lunepack = \{ git = "([^"]+)", rev = "([^"]+)" \}$'
if (@($pin).Count -ne 1) { throw 'the lunepack dependency is not found once in Cargo.toml' }
$repo = $pin.Matches[0].Groups[1].Value
$rev = $pin.Matches[0].Groups[2].Value
if ($repo -notmatch '^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') { throw 'the lunepack dependency is not a github.com git dependency' }
if ($rev -notmatch '^[0-9a-f]{40}$') { throw 'the lunepack dependency has no 40-hex rev' }

$src = Join-Path $root "target\lunepack-src-$($rev.Substring(0, 12))"
New-Item -ItemType Directory -Force -Path (Join-Path $root 'target') | Out-Null
if (Test-Path -LiteralPath $src) {
    if ((git -C $src rev-parse HEAD) -ne $rev) { throw "$src is not at $rev; remove it by hand" }
    if (@(git -C $src status --porcelain).Count -ne 0) { throw "$src has local changes; remove it by hand" }
} else {
    $tmp = Join-Path $root ('target\lunepack-src.' + [guid]::NewGuid().ToString('N').Substring(0, 8))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    Run 'git init' { git -C $tmp init -q }
    Run 'git remote add' { git -C $tmp remote add origin $repo }
    git -C $tmp fetch -q --depth 1 origin $rev
    if ($LASTEXITCODE -ne 0) { Run 'git fetch' { git -C $tmp fetch -q origin } }
    Run "git checkout $rev" { git -C $tmp checkout -q --detach $rev }
    if ((git -C $tmp rev-parse HEAD) -ne $rev) { throw 'checked out the wrong commit' }
    Move-Item -LiteralPath $tmp -Destination $src
}

# lunepack's own rustflags (the C runtime is linked in) and build id rule: nothing from the caller.
foreach ($name in 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS', 'LUNEPACK_BUILD_ID') {
    Remove-Item -ErrorAction SilentlyContinue "Env:$name"
}
$env:CARGO_TARGET_DIR = Join-Path $root 'target\lunepack-target'
Push-Location $src
try {
    if (Get-Command rustup -ErrorAction SilentlyContinue) {
        Run "the toolchain $src pins" { rustup toolchain install }
    }
    # Every package of the checkout's graph: crates.io, a path inside the checkout, or luneui-rs.
    $meta = (cargo metadata --locked --format-version 1) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'cargo metadata failed' }
    $crates = 'registry+https://github.com/rust-lang/crates.io-index'
    $srcFull = [IO.Path]::GetFullPath($src).TrimEnd('\')
    $bad = @()
    foreach ($p in $meta.packages) {
        if ($null -eq $p.source) {
            $dir = [IO.Path]::GetFullPath((Split-Path -Parent $p.manifest_path)).TrimEnd('\')
            if ($dir -ne $srcFull -and -not $dir.StartsWith($srcFull + '\', [StringComparison]::OrdinalIgnoreCase)) {
                $bad += "$($p.name) $($p.version): path outside the checkout"
            }
        } elseif ($p.source -ne $crates -and $p.source -notmatch '^git\+https://github\.com/lune-rs/luneui-rs\?rev=([0-9a-f]{40})#\1$') {
            $bad += "$($p.name) $($p.version): source $(($p.source -split '[#?]')[0])"
        }
    }
    if ($bad.Count -gt 0) { throw ("the lunepack checkout has a dependency from an unexpected source:`n  " + ($bad -join "`n  ")) }
    Write-Output "lunepack graph: $(@($meta.packages).Count) packages, all crates.io, in the checkout or luneui-rs"

    $triple = 'x86_64-pc-windows-msvc'
    Run 'cargo build' {
        cargo build --locked --release --target $triple --config profile.release.strip=true `
            -p lunepack-runtime -p lunepack-ui -p lunepack-stub
    }
    $release = Join-Path $env:CARGO_TARGET_DIR "$triple\release"
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    # kind -> required subsystem (0 for any): 2 GUI, 3 console.
    $kinds = [ordered]@{ 'runtime' = 0; 'stub' = 2; 'ui' = 0 }
    foreach ($kind in $kinds.Keys) {
        $bin = Join-Path $release "lunepack-$kind.exe"
        if (-not (Test-Path -LiteralPath $bin -PathType Leaf)) { throw "the build left no $bin" }
        $bytes = [IO.File]::ReadAllBytes($bin)
        if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) { throw "$bin is not a PE file (no MZ)" }
        $pe = [BitConverter]::ToInt32($bytes, 0x3C)
        if ($pe -lt 0 -or $pe + 0x5E -gt $bytes.Length) { throw "$bin has no PE header in the file" }
        if ($bytes[$pe] -ne 0x50 -or $bytes[$pe + 1] -ne 0x45 -or $bytes[$pe + 2] -ne 0 -or $bytes[$pe + 3] -ne 0) { throw "$bin is not a PE file (no PE signature)" }
        $machine = [BitConverter]::ToUInt16($bytes, $pe + 4)
        if ($machine -ne 0x8664) { throw "$bin is not an x86-64 PE (machine 0x$('{0:X4}' -f $machine))" }
        $subsystem = [BitConverter]::ToUInt16($bytes, $pe + 24 + 68)
        $want = $kinds[$kind]
        if ($want -ne 0 -and $subsystem -ne $want) { throw "$bin has subsystem $subsystem, not $want" }
        Copy-Item -LiteralPath $bin -Destination (Join-Path $OutDir "lunepack-$kind.exe")
        Write-Output "checked: lunepack-$kind.exe (PE x86-64, subsystem $subsystem)"
    }
} finally {
    Pop-Location
}
[IO.File]::WriteAllText((Join-Path $OutDir 'lunepack-rev.txt'), "$rev`n")
Write-Output "build-lunepack: windows-x86_64 built from $rev into $OutDir"
