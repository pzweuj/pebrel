# Ship complete optional X display trees in both Windows formats

## Status

Implemented for local validation; pending maintainer review.

## Context

The user approved an optional yserver backend. Its portable Windows runtime has
XKB and license subdirectories. The previous ZIP rule copied only flat files;
the installer had no X11 payload rule at all.

## Evidence

The official yserver v1.9.0 release publishes x86_64 and aarch64 portable archives
and SHA256 asset digests. Its README requires XKB data; the archive also contains
the executable, xauth, README and dependency licenses.

## Decision

The explicit `fetch-yserver.ps1` script pins version, architecture and archive
digest, validates archive paths and required runtime inputs, then populates the
ignored `packaging/windows/x11/yserver/` build input. It installs no system service.
ZIP and Inno retain the full optional X11 tree. Architecture validation includes
all executable/DLL payloads under it. User startup performs no downloads.

Material file icon SVGs are a small source-controlled subset from one fixed
upstream commit. Their MIT license is installed with both Windows formats, and
the source commit is recorded alongside the assets.

## Rejected alternatives

- Copy executable files alone: loses keyboard data and dependency licenses.
- Fetch at application startup: makes startup depend on remote executable downloads.
- Ignore X display machine types: permits an ARM64 package with an x64 display.
- Commit the portable X server binaries: these remain reproducible build inputs.

## Consequences

An explicit fetch is required to include yserver. A build without optional runtimes
remains possible. Runtime presence does not remove the backend's GPU requirements.
Existing VcXsrv files remain usable and are not removed by the fetch script.

## Validation

The fetch checks the official pinned digest and required files. PE fixtures accept
matching optional runtimes and reject mismatched ones. Archive manifests and the
installer are checked through the existing package entry points.

## Supersedes

The flat optional-runtime payload limitation in the 2026-11-12 packaging note.

## Revisit when

The pinned upstream release or either Windows packaging layout changes.
