# Extract the pinned upstream runtime; never run its installer on the build host.
[CmdletBinding()]
param(
    [string] $Destination = (Join-Path $PSScriptRoot '..\packaging\windows\x11\vcxsrv'),
    [string] $SevenZip = '7z.exe'
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$version = '21.1.16'
$name = "vcxsrv-64.$version.0.installer.noadmin.exe"
$hash = '25d541101716aee898ddad2af95bc0d7d3791cb639e16455b4156efa67c8fb67'
$licenseHash = '0b383d5a63da644f628d99c33976ea6487ed89aaa59f0b3257992deac1171e6b'
$extractor = (Get-Command $SevenZip -ErrorAction Stop).Source
$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$cache = Join-Path $repo 'target\x11-cache'
New-Item -ItemType Directory -Force -Path $cache | Out-Null
$archive = Join-Path $cache $name
$license = Join-Path $cache "vcxsrv-$version-COPYING"
foreach ($artifact in @(
    @{ Path = $archive; Hash = $hash; Url = "https://github.com/marchaesen/vcxsrv/releases/download/$version/$name" },
    @{ Path = $license; Hash = $licenseHash; Url = "https://raw.githubusercontent.com/marchaesen/vcxsrv/$version/COPYING" }
)) {
    if (-not (Test-Path -LiteralPath $artifact.Path) -or
        (Get-FileHash -LiteralPath $artifact.Path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $artifact.Hash) {
        Invoke-WebRequest -Uri $artifact.Url -OutFile $artifact.Path
    }
    if ((Get-FileHash -LiteralPath $artifact.Path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $artifact.Hash) {
        throw 'Pinned VcXsrv input SHA256 mismatch'
    }
}
$extract = Join-Path $cache ([guid]::NewGuid().ToString('N'))
try {
    & $extractor x $archive "-o$extract" -y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'VcXsrv archive extraction failed' }
    foreach ($required in @('vcxsrv.exe', 'xauth.exe', 'xkbcomp.exe', 'xkbdata\rules\base', 'fonts')) {
        if (-not (Test-Path -LiteralPath (Join-Path $extract $required))) {
            throw "Incomplete VcXsrv runtime: $required"
        }
    }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    # NSIS helpers, uninstallers and unrelated client apps are not runtime inputs.
    Get-ChildItem -LiteralPath $extract | Where-Object {
        $_.Name -ne '$PLUGINSDIR' -and
        ($_.Extension -ne '.exe' -or $_.Name -in @('vcxsrv.exe', 'xauth.exe', 'xkbcomp.exe'))
    } | Copy-Item -Destination $Destination -Recurse -Force
    Copy-Item -LiteralPath $license -Destination (Join-Path $Destination 'COPYING') -Force
    $source = "VcXsrv $version, unmodified upstream binaries. License: COPYING.`n" +
        "Upstream: https://github.com/marchaesen/vcxsrv/tree/$version`n" +
        "Corresponding source (including dependencies and build scripts): https://github.com/marchaesen/vcxsrv/archive/refs/tags/$version.zip`n"
    [IO.File]::WriteAllText((Join-Path $Destination 'SOURCE.txt'), $source, [Text.UTF8Encoding]::new($false))
    Write-Output "Verified VcXsrv $version (x64) copied to $Destination"
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
