# Windows ships VcXsrv as its sole X display

## Status

Implemented at the maintainer's request.

## Context

VcXsrv already serves the Windows X11 forwarding workflow. Maintaining a second
server also put its executable, keyboard data and licenses in Windows packages.

## Evidence

The former packaging rules recursively copied the entire prepared X11 directory.
Removing a download script alone left previously fetched binaries in new builds.
The pinned VcXsrv 21.1.16 accepts the per-screen `-notrayicon` option.

## Decision

The ZIP and Inno manifests take their optional X11 input from the VcXsrv
subdirectory used by [the fetch script](../../../scripts/fetch-vcxsrv.ps1).
Windows [display discovery](../../../nebula_app/src/platform/x11_server.rs)
tries bundled VcXsrv first, followed by installed VcXsrv. Pebrel supplies
`-notrayicon` to the process it starts.

## Rejected alternatives

Hiding icons through Windows notification-area preferences would also affect
independently launched servers. Retaining automatic yserver fallback would keep
the discontinued backend reachable on upgraded installations.

## Consequences

Previously extracted yserver files and the old backend preference no longer
participate in discovery. Existing flat and nested VcXsrv installations remain
discoverable. The resident lease still keeps a display ready for SSH clients.

## Validation

The package manifest tests check the VcXsrv payload. The display tests cover
stale yserver files, installed VcXsrv fallback, command arguments and
authenticated image transfer against the real server.

## Supersedes

The yserver payload in [optional Windows X displays](2026-11-16-optional-yserver-runtime.md)
and the backend selection in [rendering preference](../nebula_app/platform/2026-11-16-x11-rendering-backend-preference.md).

## Revisit when

A supported Windows workflow requires another display server.
