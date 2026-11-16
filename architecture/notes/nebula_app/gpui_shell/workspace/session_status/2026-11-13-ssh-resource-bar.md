# SSH resource strip samples the focused pane on its own exec channel

## Status

Accepted with the bottom session strip.

## Context

The terminal card showed the focused SSH session but not who was logged in or
how busy the host was. MobaXterm keeps that on a bottom strip: login identity,
CPU, memory, swap, disk, and network rates. Those numbers are not in OSC 7 or
the shell title.

## Evidence

`ssh_session::exec_capture` already runs a script on an authenticated exec
channel without printing it into the terminal. `/proc` and `df -P` expose the
counters on Linux hosts. CPU and interface rates need two samples; a single
reading cannot produce them.

## Decision

While the terminal tab is drawing a ready SSH pane, the workspace owns one
status probe. It starts immediately, then at most once every two seconds, and
never overlaps itself. The script prints a marked key-value block. The strip
shows the destination user and host at once, then replaces them with `id` and
`hostname` when the sample arrives. Memory, swap, disk, and uptime come from
one sample. CPU and byte rates come from the delta to the previous sample.
Switching panes or leaving SSH drops the task, bumps the generation, and clears
the reading so a late sample cannot paint the next session.

## Rejected alternatives

- Sleeping inside the remote script to compute CPU in one call. The exec then
  stays open for the sleep, and a pane switch still waits on it.
- Parsing resource text out of the interactive shell. Prompt banners and full
  screen programs make that unreliable, and the command would be visible.
- Polling on a timer even when the terminal tab is not drawn. A hidden document
  tab should not keep asking the host.

## Consequences

Linux hosts with `/proc` fill the strip. Hosts without it still show the login
parsed from the destination, with dashes for the missing meters. Two silent
samples are required before CPU and network move. The probe uses the shared SSH
connection and does not open a second authentication.

## Validation

`session_status` tests parse a noisy sample, check memory, swap, disk, uptime,
a 50% CPU delta, a download rate, and wrapped counters. They do not connect to
a host. The strip itself is drawn by the terminal tab; hover tooltips use the
existing tooltip path.

## Supersedes

None.

## Revisit when

The focused pane can read host counters without a separate exec, or a non-Linux
session needs its own sampler.
