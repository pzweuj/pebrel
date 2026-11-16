# Disable application self-update as an immutable distribution policy

## Status

Implemented for local validation; pending maintainer review.

## Context

The user explicitly requires permanently disabling both automatic and manual
application updates, including reminders and related functionality.

## Evidence

An automatic-check preference alone does not cover cached update notifications,
manual checks, downloads, scheduled installation or a prepared helper invocation.
The direct-distribution adapter is already the shared boundary for checks,
downloads, prepare and schedule. Runtime settings are shared by both UI shells.

## Decision

Settings core owns the fixed `APPLICATION_UPDATES_ENABLED = false` policy. Saved
preferences and script defaults cannot enable checking or downloading. Preserve
old keys as compatible inputs, but their effective values remain false.

The shared direct-update guard rejects all channels before update I/O. Startup
workers return before creating threads or reading cached downloads. The platform
does not advertise self-install support. Previously scheduled installation is
ignored; an explicit macOS update helper invocation is rejected. Update-triggered
quit and scheduling cannot stop sessions or save an update workspace.

Remove update checks, update status, switches, cached-update actions and release
links from the GPUI About page. Notification/dialog entry points also reject stale
update events. Normal application version display, session recovery, plugins,
themes and developer build/packaging commands retain their existing ownership.

## Rejected alternatives

- Save only an off preference: old settings, manual actions or cached jobs bypass it.
- Hide only the controls: leaves network and installation entry points callable.
- Delete all validation/recovery code and tests: unnecessary to deny the capability
  and risks losing recovery data from earlier installations.

## Consequences

There is no runtime preference, script default or UI action to reenable updates.
The application performs no self-update check, download or installation. Existing
cached files are not executed or destructively removed.

## Validation

Settings tests try all enabled spellings. Distribution tests cover direct, Scoop
and unknown ownership and verify rejection before update I/O or worker retention.
Handoff tests retain an old scheduled job and verify it is not consumed. GPUI
layout tests verify About renders normally with no update controls.

## Supersedes

The update-enabled defaults and direct-channel availability described by earlier
self-update implementation notes, for this user-requested distribution.

## Revisit when

Only if the user explicitly changes the permanent no-update requirement.
