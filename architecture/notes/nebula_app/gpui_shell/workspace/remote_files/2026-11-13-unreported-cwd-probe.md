# Unreported remote shells keep following through a throttled cwd probe

## Status

Accepted with the file-panel follow fix.

## Context

SFTP follow moves when the focused pane's OSC 7 or `NEBULA|` title changes.
Many remote shells never emit either report. Opening the drawer still probes
once, then `shell_cwd` stays empty and later `cd` leaves the listing where it
was. Turning "follow the session" on calls the same reported-cwd snap, so the
switch does nothing for those shells.

## Evidence

`TerminalView::remote_cwd` returns a path only after a shell report. The drawer
poll reads that value and does not ask the host again. `remote_cwd::probe`
already finds the unique foreground shell on one connection and returns
`Ambiguous` when several PTYs share it.

## Decision

While follow is on, the drawer is showing a listing, and the focused pane has
no shell report, probe that connection at most once every two seconds and never
overlap a probe. A `Located` path is applied through the existing follow rule:
an unchanged shell directory keeps manual browsing, a changed one lists the new
directory. `Ambiguous` and `Unavailable` do not move the listing. A shell report
that arrives later is authoritative and stops the probe.

The task lives on the browser. Rebinding, detaching, forgetting the pane, or
turning follow off drops it, which closes the oneshot and cancels the exec.

## Rejected alternatives

- Probing on every drawer frame. `ps` on the remote host is too expensive, and
  the render path would start a new exec per frame.
- Treating a repeated probe of the same directory as a fresh navigation. That
  discards manual browsing the follow switch is documented to keep.
- Guessing among pooled PTYs when the probe is `Ambiguous`. The wrong directory
  is worse than staying put; those panes still follow when they report a cwd.

## Consequences

A session with no shell integration spends one remote exec every two seconds
while its file drawer is open and follow is on. Sessions that report a cwd do
not. The probe remains connection-wide, so two silent panes on one SSH
connection stay where the user put them.

## Validation

`remote_files/follow_tests.rs` queues probe answers for one destination and
checks that an unchanged directory keeps a manual listing, a later unique
directory is listed, and an ambiguous answer does not move it. The test does
not authenticate or read host credentials.

## Supersedes

The consequence in
[2026-10-10-pane-cwd-following.md](2026-10-10-pane-cwd-following.md)
that shells without cwd reports are not followed after the initial probe.

## Revisit when

Each SSH pane can ask for its own PTY cwd, so a shared connection no longer has
to return `Ambiguous`.
