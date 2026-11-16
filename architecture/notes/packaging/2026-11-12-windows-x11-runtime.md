# Windows package can carry the X11 display

## Status

The application looks for `x11/vcxsrv.exe` beside itself. The release layout includes that directory only after `scripts/fetch-vcxsrv.ps1` has populated `packaging/windows/x11`.

## Context

X11 forwarding on Windows needs a display server. The server is GPL-compatible with Pebrel and stays a separate executable. Committing the binary would add a large unreviewed blob to git.

## Evidence

- `scripts/fetch-vcxsrv.ps1` downloads one pinned installer, checks SHA256, and copies the runtime into `packaging/windows/x11`.
- `scripts/package-release.ps1` appends whatever files are already in that directory to the package manifest.

## Decision

Do not commit VcXsrv. A Windows package built without the fetched runtime remains valid and simply has no built-in display. A package built after the fetch script includes those files under `x11/`.

## Rejected alternatives

- Downloading VcXsrv on the user's first SSH connection.
- Vendoring the executable in the git tree.

## Consequences

Release builders who want built-in X11 run the fetch script before `package-release.ps1`. The SHA256 in the script is the pin; a changed upstream file fails the script.

## Validation

The fetch script refuses a hash mismatch. Packaging still checks that the zip listing matches the manifest, including any added `x11/` files.

## Supersedes

None.

## Revisit when

The pinned VcXsrv version changes or the Windows package stops being a loose file layout.
