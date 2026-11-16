# Resident Windows X display with optional yserver and VcXsrv fallback

## Status

Implemented for local validation; pending maintainer review.

## Context

The user reported that IGV still did not open and requested an X display that
starts with Pebrel. They approved optional yserver while retaining VcXsrv.

## Evidence

The existing display lease starts only for a host with X11 forwarding enabled
and ends after the last shell closes. It checks a TCP listener, not authentication.
The yserver 1.9.0 Windows package accepts `:N --rootless -auth FILE`, uses TCP,
and includes XKB data and library licenses. Its Windows rendering requires an
appropriate Vulkan driver, so successful startup cannot be assumed.

## Decision

After resident-instance ownership is resolved, a named background worker acquires
one display lease. The first GUI frame does not wait for it. The worker retains
the lease until its owning guard is dropped and reacquires it after a child exit.
The existing process group owns and terminates the child tree.

Try an optional yserver beside Pebrel under `x11/`, then bundled or installed
VcXsrv. Each launch must accept an authenticated X11 setup before it is ready.
Retain logs on startup failure and try the next candidate. Authentication files
are random, process-specific files, removed after failure or normal shutdown.
Leases carry the display cookie so an old lease cannot decrement a replacement
server's user count.

The per-host forwarding switch remains authoritative. Starting a local server
does not itself create the remote DISPLAY or prove an application can render.

## Rejected alternatives

- Replace the mature fallback unconditionally: yserver can fail on GPU/driver support.
- Load yserver into Pebrel as a Rust crate: a separately owned process already fits
  its lifecycle and avoids importing its rendering dependency graph.
- Disable X authentication or accept an open port as readiness: neither proves
  the forwarded client can authenticate.
- Start synchronously on the GUI thread: display startup can wait on I/O and drivers.

## Consequences

The local X process survives closing the last SSH pane and ends with Pebrel.
Normal shell requests share the same display; SSH still replaces its fake cookie
with the local cookie. Files are discovered locally; startup never downloads code.
Real IGV acceptance still requires the user's remote environment.

## Validation

Tests cover candidate fallback, backend arguments and stale lease releases.
An explicitly enabled Windows test launches the pinned real yserver, checks good
and bad authentication cookies, then verifies process/authority cleanup.

## Supersedes

The shell-only display lifetime described in the original X11 forwarding note.
The TCP-listen fix remains applicable to VcXsrv.

## Revisit when

Backend selection needs a persisted user preference or yserver's CLI changes.
