param(
    [string]$Root = "$env:USERPROFILE\LANSyncE2E",
    [int]$LargeMB = 16
)

$ErrorActionPreference = "Stop"

$source = Join-Path $Root "source"
$target = Join-Path $Root "target"
$manifest = Join-Path $Root "manifest.txt"

New-Item -ItemType Directory -Force -Path $source | Out-Null
New-Item -ItemType Directory -Force -Path $target | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $source "small") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $source "nested\a\b") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $source "many") | Out-Null

Set-Content -Path (Join-Path $source "small\hello.txt") -Value "hello from windows" -Encoding UTF8
Set-Content -Path (Join-Path $source "nested\a\b\report.txt") -Value "nested windows report" -Encoding UTF8

1..20 | ForEach-Object {
    $name = "file-{0:D3}.txt" -f $_
    Set-Content -Path (Join-Path $source "many\$name") -Value "windows many file $_" -Encoding UTF8
}

$large = Join-Path $source "large.bin"
$bytes = New-Object byte[] (1024 * 1024)
$stream = [System.IO.File]::Create($large)
try {
    for ($i = 0; $i -lt $LargeMB; $i++) {
        [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)
        $stream.Write($bytes, 0, $bytes.Length)
    }
}
finally {
    $stream.Dispose()
}

Get-ChildItem $source -Recurse -File |
    Where-Object { $_.FullName -notmatch "\\.lanbridge-history\\" } |
    Sort-Object FullName |
    ForEach-Object {
        $relative = $_.FullName.Substring($source.Length + 1).Replace("\", "/")
        $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash
        "$hash  $relative"
    } | Set-Content -Path $manifest -Encoding UTF8

Write-Host "LanBridge Windows test data ready"
Write-Host "Source: $source"
Write-Host "Target: $target"
Write-Host "Manifest: $manifest"
Write-Host "App data: $env:APPDATA\LanBridge"
Write-Host "TCP service port: 9527"
Write-Host "Discovery UDP: 239.10.10.10:53530"
