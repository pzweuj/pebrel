# Hide VcXsrv keyboard compilation without changing interactive consoles

## Status

Implemented for local validation; pending maintainer review.

## Context

The user reported many CMD windows flashing during startup after VcXsrv became
the preferred bundled display.

## Evidence

An isolated startup trace shows VcXsrv launching CMD and xkbcomp repeatedly.
[Its keyboard compiler invocation](https://github.com/marchaesen/vcxsrv/blob/21.1.16/xorg-server/xkb/ddxLoad.c)
uses CRT system(), so hiding only Pebrel's direct child does not control that CMD.
The display previously reset after the last setup probe disconnected. A stale
server-N.xkm directory in the shared Windows temporary root also prevents
keyboard output from being written.

## Decision

Scope COMSPEC to Pebrel's GUI executable only in the VcXsrv child environment.
An internal environment marker activates an early command bridge before console
attachment, CLI parsing, hooks or GUI initialization. Preserve the original /c
command tail, including quotes, Unicode and redirections. Forward it to the native
System32 cmd.exe with CREATE_NO_WINDOW and inherited standard handles. Remove the
marker and restore COMSPEC in that child to prevent recursion. The existing
VcXsrv job owns these descendants; no system environment or interactive PTY changes.

Use -noreset for the resident display. Give each launch a distinct temporary
keyboard directory alongside its authority/log files, preventing stale or
concurrent server-N.xkm collisions without deleting unrelated temporary data.

Separately, the Codex capability probe can use a recognized npm launcher's own
native binary. Custom launchers retain the existing fallback; interactive Agent
launch behavior is unchanged.

## Rejected alternatives

- Reapply CREATE_NO_WINDOW to VcXsrv only: it already uses the hidden process adapter.
- Disable XKB: breaks keyboard input.
- Disable X11 or fall back to yserver silently: does not address IGV compatibility.
- Change COMSPEC globally: would affect user terminals and unrelated programs.

## Consequences

The bridge adds no executable payload or installed service. It exits with the
keyboard command's result. Display authentication, owned job cleanup and VcXsrv
image rendering still require native validation after launch changes.

## Validation

Tests preserve command tails and redirections with Unicode paths and verify exit
codes through the actual GUI helper. Native display tests authenticate and round-
trip image pixels. Startup traces compare descendant command/console hosts.

## Supersedes

The assumption that hiding the direct display process also hides its CRT commands.
Resident display ownership and backend priority remain unchanged.

## Revisit when

The bundled VcXsrv launches its keyboard compiler with a native hidden-process API.
