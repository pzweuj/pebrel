# Validate the machine type of every executable payload before naming a Windows package.
function Assert-WindowsPackageArchitecture {
    param(
        [Parameter(Mandatory = $true)][string] $Root,
        [ValidateSet('x64', 'arm64')][string] $Architecture = 'x64',
        [string] $RuntimeDirectory = $Root,
        [ValidateSet('pebrel-cli.exe', 'pebrel.com')][string] $CliExecutable = 'pebrel-cli.exe',
        [string] $X11Directory = (Join-Path $Root 'x11')
    )
    $machine = if ($Architecture -eq 'arm64') { 0xAA64 } else { 0x8664 }
    $payloads = @('pebrel.exe', $CliExecutable, 'pebrel-hook.exe', 'conpty.dll', 'OpenConsole.exe') | ForEach-Object {
        $name = $_
        $directory = if ($name -in @('pebrel.exe', $CliExecutable)) { $Root } else { $RuntimeDirectory }
        Join-Path $directory $name
    }
    if (Test-Path -LiteralPath $X11Directory) {
        $payloads += @(Get-ChildItem -LiteralPath $X11Directory -Recurse -File | Where-Object {
            $_.Extension -in '.exe', '.dll'
        } | ForEach-Object FullName)
    }
    foreach ($path in $payloads) {
        $stream = [IO.File]::OpenRead($path)
        $reader = [IO.BinaryReader]::new($stream)
        try {
            if ($stream.Length -lt 64 -or $reader.ReadUInt16() -ne 0x5A4D) {
                throw "Invalid executable header: $path"
            }
            $stream.Position = 0x3C
            $offset = $reader.ReadUInt32()
            if ($offset -lt 64 -or $offset -gt $stream.Length - 6) {
                throw "Invalid executable header offset: $path"
            }
            $stream.Position = $offset
            if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne $machine) {
                throw "Executable architecture does not match $Architecture`: $path"
            }
        } finally {
            $reader.Dispose()
            $stream.Dispose()
        }
    }
}
