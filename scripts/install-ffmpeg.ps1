<#
.SYNOPSIS
  ffmpeg・ffprobe(ビルド済み)をダウンロードして、このスクリプトのあるフォルダに置く。
.DESCRIPTION
  script2video を PATH の通ったフォルダに展開してあれば、このスクリプトで ffmpeg も同じ場所に入る。
  配布元: https://www.gyan.dev/ffmpeg/builds/ (release-essentials。libx264・aac を含む)。
  ダウンロードしたファイルは SHA-256 を配布元の値と照合してから展開する。
.PARAMETER Destination
  置き先。省略時はこのスクリプトのあるフォルダ。
.PARAMETER Force
  すでに使える ffmpeg があっても入れ直す。
#>
[CmdletBinding()]
param(
    [string]$Destination,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
if (-not $Destination) { $Destination = Split-Path -Parent $MyInvocation.MyCommand.Path }
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$ZipUrl = 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip'
$HashUrl = "$ZipUrl.sha256"

function Get-FfmpegMajor([string]$exe) {
    # 6.0 以上、または 2023 年以降の日付版・git 版を「使える」とみなす(script2video の検査に合わせる)
    try { $line = (& $exe -version 2>$null | Select-Object -First 1) } catch { return $null }
    if ($line -match 'version (\d+)\.') { return [int]$Matches[1] }
    if ($line -match 'version (20\d\d)-') { return [int]$Matches[1] - 2017 }
    if ($line -match 'version N-') { return 6 }
    return $null
}

if (-not $Force) {
    $ok = $true
    foreach ($name in 'ffmpeg', 'ffprobe') {
        $cmd = Get-Command $name -ErrorAction SilentlyContinue
        $v = if ($cmd) { Get-FfmpegMajor $cmd.Source } else { $null }
        if ($null -eq $v -or $v -lt 6) { $ok = $false }
    }
    if ($ok) {
        Write-Host 'ffmpeg・ffprobe(6.0 以上)はすでに使えます。入れ直すときは -Force を付けてください。'
        exit 0
    }
}

if (-not (Test-Path -LiteralPath $Destination -PathType Container)) {
    throw "置き先のフォルダがありません: $Destination"
}

$work = Join-Path ([IO.Path]::GetTempPath()) ("s2v-ffmpeg-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $zip = Join-Path $work 'ffmpeg.zip'
    Write-Host "ダウンロード中: $ZipUrl"
    Invoke-WebRequest -Uri $ZipUrl -OutFile $zip -UseBasicParsing
    $expected = ((Invoke-WebRequest -Uri $HashUrl -UseBasicParsing).Content.ToString().Trim() -split '\s+')[0]
    $actual = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
    if ($expected -notmatch '^[0-9A-Fa-f]{64}$') { throw "配布元の SHA-256 を読めません: $expected" }
    if ($actual -ne $expected.ToUpperInvariant()) {
        throw "SHA-256 が一致しません(何も置いていません)。期待: $expected / 実際: $actual"
    }

    $ex = Join-Path $work 'x'
    Expand-Archive -LiteralPath $zip -DestinationPath $ex
    foreach ($name in 'ffmpeg.exe', 'ffprobe.exe') {
        $src = Get-ChildItem -LiteralPath $ex -Recurse -Filter $name | Select-Object -First 1
        if (-not $src) { throw "zip の中に $name がありません" }
        Copy-Item -LiteralPath $src.FullName -Destination (Join-Path $Destination $name) -Force
    }
}
finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "ffmpeg.exe・ffprobe.exe を置きました: $Destination"
$dest = (Resolve-Path -LiteralPath $Destination).Path.TrimEnd('\')
$onPath = ($env:PATH -split ';' | ForEach-Object { $_.TrimEnd('\') }) -contains $dest
if (-not $onPath) {
    Write-Warning "$dest は PATH に入っていません。PATH に追加するか、config.toml の [render] ffmpeg / ffprobe で場所を指定してください。"
}
