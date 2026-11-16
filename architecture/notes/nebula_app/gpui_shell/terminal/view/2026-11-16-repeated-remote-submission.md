# Reconcile a returned remote prompt before the next Enter

## Status

Implemented for local validation; pending maintainer review.

## Context

The user could follow the first pwd but later invocations did not follow.

## Evidence

Enter captures a shell command only if the previous command is not still marked
running. Silent remote shells have no authoritative completion marker, and their
periodic observer may not see an idle prompt before the user types again. The
next Enter is then treated as stdin for the previous command and never arms pwd.

## Decision

Before that busy-input guard, a remote pane can reconcile its verified current
prompt prefix with the saved submission prompt using the same matcher as the
existing activity observer. This includes the typed input snapshot, allowing a
changed remote cwd in a prompt with the same login anchor. Local process evidence,
alternate screens, native program status and hook-owned activity retain priority.
Bracketed login/host prompts keep their identity across cwd changes. Themed arrow
prompts require an explicit terminal input boundary; the generic activity matcher
continues to reject bare Agent-style arrows and questions. Known Agents remain
excluded from submission reconciliation even if an input boundary is present.
Completion then follows the existing command-end path before capturing the next
submission. Command/prompt matching remains implemented once in input_state.

The legacy display's independent log-replay command classifier moves into its
existing command-classification module rather than growing the oversized facade.
Its public entry and behavior tests remain unchanged.

## Rejected alternatives

- Only reset after reading pwd: a subsequent silent cd would block the next pwd.
- Clear running state on every Enter: can submit shell interpretations of program input.
- Wait for the periodic observer: it can miss the idle prompt once typing resumes.

## Consequences

Repeated pwd and intervening cd commands establish fresh capture lifetimes without
waiting for a watchdog tick. This remains bounded screen evidence for silent SSH
shells, not proof of arbitrary remote process completion.

## Validation

A terminal regression submits pwd, cd, pwd and pwd without an observer tick or
OSC completion, and checks every directory capture and resulting pane cwd.
Existing foreground-input and workspace SFTP routing tests remain applicable.

## Supersedes

None; repairs the submission lifetime of the explicit-pwd fallback.

## Revisit when

Every remote shell provides an authoritative prompt/completion boundary.
