# Explicit pwd output supplies pane-owned remote directory metadata

## Status

Implemented; pending maintainer review.

## Context

SFTP following cannot rely on a connection-wide cwd probe when several silent
shells share a connection. The user requested an explicit `pwd` fallback.

## Evidence

The terminal already captures a verified submitted command from its grid.
Its bounded reading API provides epoch-qualified absolute rows, including wrapped
output. Workspace cwd following already rejects background-pane navigation.

## Decision

Arm a pane-owned capture only for a submitted `pwd`, `pwd -L`, or `pwd -P` in
built-in SSH. Read the first complete output line after that submission, joining
soft wraps. Reject errors, relative paths, alternate screens, stale epochs,
evicted output, over-budget output and captures older than ten seconds.

Apply a path through the existing cwd event, then emit a directory-query event.
The focused workspace can snap a manual SFTP listing back even when this explicit
query reports the same cwd as before. The follow preference still gates the snap.
New submissions and shell metadata replace the capture. No command is injected.

## Rejected alternatives

- Scanning arbitrary output for absolute paths: ordinary file contents are not cwd.
- Guessing among the shells sharing one SSH connection: it can navigate the wrong pane.
- Requiring shell integration: this fallback exists for shells that do not report cwd.

## Consequences

Ordinary terminal output pays only an optional-state check. Captured output has
a fixed cell/byte budget. Screen text cannot distinguish path-ending spaces from
terminal padding; authoritative shell metadata remains the lossless path source.
Aliases that replace pwd with unrelated output are outside this fallback contract.

## Validation

Parser tests cover chunked, wrapped Unicode paths, old output and rejected commands.
GPUI tests exercise submission through the terminal and focused SFTP routing,
including returning from a manual listing and ignoring background-pane queries.

## Supersedes

None. Complements the connection-wide fallback in
[the unreported-cwd decision](../../workspace/remote_files/2026-11-13-unreported-cwd-probe.md).

## Revisit when

Every pane exposes an authoritative cwd independently of shell integration.
