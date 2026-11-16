# Populate the optional Windows X display from a verified upstream portable zip.
[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64')][string] $Architecture = 'x64',
    [string] $Destination = (Join-Path $PSScriptRoot '..\packaging\windows\x11\yserver')
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$version = '1.9.0'
$pins = @{
    x64 = @{ Arch = 'x86_64'; Hash = '2f5c761109d13065d9ac9c179c26ed7ec7eb9f0653db1c5fb113950998891a52' }
    arm64 = @{ Arch = 'aarch64'; Hash = 'eb9cd0ae0ddd4306b315be627a24c42cd4a9ec9e4003d9ca647c86182cec5697' }
}
$pin = $pins[$Architecture]
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$cache = Join-Path $repo 'target\x11-cache'
New-Item -ItemType Directory -Force -Path $cache | Out-Null
$name = "yserver-$version-windows-$($pin.Arch)"
$archive = Join-Path $cache "$name.zip"
if (-not (Test-Path -LiteralPath $archive) -or
    (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $pin.Hash) {
    Invoke-WebRequest -Uri "https://github.com/joske/yserver/releases/download/v$version/$name.zip" -OutFile $archive
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $pin.Hash) {
    throw 'Pinned yserver archive SHA256 mismatch'
}
$extract = Join-Path $cache ([guid]::NewGuid().ToString('N'))
$extractPrefix = [IO.Path]::GetFullPath($extract).TrimEnd('\') + '\'
try {
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($archive)
    try {
        foreach ($entry in $zip.Entries) {
            $path = [IO.Path]::GetFullPath((Join-Path $extract $entry.FullName))
            if (-not $path.StartsWith($extractPrefix, [StringComparison]::OrdinalIgnoreCase)) {
                throw "Archive entry escapes extraction directory: $($entry.FullName)"
            }
        }
    } finally { $zip.Dispose() }
    Expand-Archive -LiteralPath $archive -DestinationPath $extract
    $runtime = Join-Path $extract $name
    foreach ($required in @('yserver.exe', 'xauth.exe', 'LICENSE.txt', 'README.txt', 'xkb\rules\evdev')) {
        if (-not (Test-Path -LiteralPath (Join-Path $runtime $required) -PathType Leaf)) {
            throw "Incomplete yserver runtime: $required"
        }
    }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Get-ChildItem -LiteralPath $runtime | Copy-Item -Destination $Destination -Recurse -Force
    Write-Output "Verified yserver $version ($Architecture) copied to $Destination"
} finally {
    if (Test-Path -LiteralPath $extract) {
        $resolved = (Resolve-Path -LiteralPath $extract).Path
        $cachePrefix = [IO.Path]::GetFullPath($cache).TrimEnd('\') + '\'
        if (-not $resolved.StartsWith($cachePrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing cleanup outside the X11 cache: $resolved"
        }
        Remove-Item -LiteralPath $resolved -Recurse -Force
    }
}
