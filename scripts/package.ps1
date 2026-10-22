<#
.SYNOPSIS
  配布用 zip(dist\script2video-<版>.zip)を作る。
.DESCRIPTION
  release ビルド → 依存 DLL の検査 → zip 作成。zip の中身(平らに展開される):
    script2video.exe / install-ffmpeg.ps1 / Operating Manual.html / README.md
  展開先は PATH の通ったフォルダを想定。ffmpeg は同梱せず、install-ffmpeg.ps1 が入れる。
#>
[CmdletBinding()]
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    $m = Select-String -Path 'crates\s2v-cli\Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $m) { throw 'crates\s2v-cli\Cargo.toml に version がありません' }
    $version = $m.Matches[0].Groups[1].Value

    if (-not $SkipBuild) {
        cargo build --release -p s2v-cli
        if ($LASTEXITCODE -ne 0) { throw "cargo build が失敗しました(終了コード $LASTEXITCODE)" }
    }

    $exe = Join-Path $root 'target\release\script2video.exe'
    if (-not (Test-Path -LiteralPath $exe)) { throw "ビルド成果物がありません: $exe" }

    # VC++ ランタイム等に依存していないこと(.cargo\config.toml の crt-static が効いていること)
    $text = [Text.Encoding]::ASCII.GetString([IO.File]::ReadAllBytes($exe))
    foreach ($dll in 'VCRUNTIME140', 'MSVCP140', 'api-ms-win-crt-') {
        if ($text.Contains($dll)) { throw "script2video.exe が $dll に依存しています。crt-static でビルドされていません。" }
    }

    $files = @(
        @{ From = $exe; To = 'script2video.exe' },
        @{ From = (Join-Path $root 'scripts\install-ffmpeg.ps1'); To = 'install-ffmpeg.ps1' },
        @{ From = (Join-Path $root 'docs\Operating Manual.html'); To = 'Operating Manual.html' },
        @{ From = (Join-Path $root 'README.md'); To = 'README.md' }
    )
    $stage = Join-Path ([IO.Path]::GetTempPath()) ("s2v-pkg-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage | Out-Null
    try {
        foreach ($f in $files) {
            if (-not (Test-Path -LiteralPath $f.From)) { throw "同梱するファイルがありません: $($f.From)" }
            Copy-Item -LiteralPath $f.From -Destination (Join-Path $stage $f.To)
        }
        $dist = Join-Path $root 'dist'
        New-Item -ItemType Directory -Path $dist -Force | Out-Null
        $zip = Join-Path $dist "script2video-$version.zip"
        if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip }
        Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    }
    finally {
        Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
    }

    Write-Host "作成しました: $zip"
    Write-Host ("SHA-256: " + (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $z = [IO.Compression.ZipFile]::OpenRead($zip)
    try { $z.Entries | ForEach-Object { '  {0}  ({1:N0} bytes)' -f $_.FullName, $_.Length } } finally { $z.Dispose() }
}
finally {
    Pop-Location
}
