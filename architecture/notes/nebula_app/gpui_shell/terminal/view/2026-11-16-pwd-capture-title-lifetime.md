# Keep explicit pwd output discovery independent of activity and title metadata

## Status

Implemented for local validation; pending maintainer review.

## Context

The user reported that directory following completely stopped in the previous build.

## Evidence

A regression with `NEBULA|/old||pwd` before the output Wakeup reproduces a concrete
failure: the title handler cancels the armed capture, leaving cwd at `/old`.
History recall also invalidates the key mirror, so ordinary command classification
cannot reliably arm a capture for `$ pwd`. Existing tests assigned the mirror and
did not insert preexec title events; they missed both lifetimes. A queued
CommandStart can also be drained after the shared grid has advanced through the
whole output batch; reclassification must preserve the original reading span.

## Decision

At Enter, independently recognize only `pwd`, `pwd -L` and `pwd -P` using the
existing shared grid prompt reader. A key mirror is not required for this narrow
screen check. When SSH echo is incomplete, accept only a complete supported
input mirror with a verified echoed prefix at a shell prompt; account for the
remaining ASCII echo and delayed wrapping when locating output.
Reuse the bounded reading-span capture; do not reset foreground
activity merely to discover cwd. Known Agents, hook ownership, native program
status, alternate screens and VI input retain their existing boundaries.

A new submission replaces the old capture. CommandStart preserves an existing
Enter capture while still classifying other command state. Titles and cwd reports update metadata
but do not cancel pending explicit output. Complete path output, invalid output,
command completion, ten-second expiry and terminal epoch changes bound its lifetime.
The existing pane-scoped RemoteDirectoryQueried event drives SFTP navigation.

## Rejected alternatives

- Depend on generic activity completion or the key mirror: both can be absent
  in a silent remote shell or after Up/Down history navigation.
- Cancel on every title: preexec titles arrive before the command output.
- Interpret all absolute output paths as cwd: unrelated program output must not
  redirect the file browser.

## Consequences

Explicit queries work with uninstrumented arrow and ordinary shell prompts,
including repeated queries of the same directory after manual SFTP browsing.
This narrow discovery does not claim arbitrary remote command completion.

## Validation

The new title/history regression fails against the previous implementation and
passes after the change. Workspace tests exercise preexec titles, repeated pwd,
actual SFTP navigation and background-pane isolation. Delayed-echo fixtures press
Enter before zero, one or two echoed letters, including at the right margin. Negative tests cover Agent
prompts, foreground stdin, unrelated commands and invalid path output.

## Supersedes

Capture eligibility/lifetime assumptions in
[remote submission reconciliation](2026-11-16-repeated-remote-submission.md).
The conservative generic activity prompt matcher remains unchanged.

## Revisit when

Every remote shell provides trustworthy pane-scoped cwd reports.
