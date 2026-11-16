# Built-in X11 forwarding

## Status

Implemented for new SSH shells. Windows starts a separate VcXsrv process when the host option is on and `x11/vcxsrv.exe` is beside the application.

## Context

Remote graphical programs need an X display on the machine running Pebrel. Asking Windows users to install VcXsrv, Xming, or X410 leaves the feature unfinished. The SSH connection is already implemented with russh, which can send `x11-req` and receive `x11` channels.

## Evidence

- `SshConnectionOptions::x11_forwarding` is off unless a saved host turns it on. Old profile JSON has no field and stays off.
- `ssh_session/x11.rs` checks the fake MIT-MAGIC-COOKIE-1 before replacing it with the local display cookie.
- `platform/x11_server.rs` owns the Windows process with the existing job-object helper. Linux and macOS use `DISPLAY` and do not ship VcXsrv.

## Decision

Keep the X server out of `pebrel.exe`. Windows packaging can place the VcXsrv runtime in `x11/`. A missing runtime does not fail the shell; the session reports that X11 did not start. Forwarding is trusted (`single_connection = false`), equivalent to `ssh -Y`, and applies only to shells opened after the host option is saved.

## Rejected alternatives

- Telling the user to install an X server.
- Implementing an X server inside the Pebrel process.
- Enabling forwarding for every host by default.

## Consequences

The Windows package grows by the VcXsrv runtime when that directory is present. Remote programs can use the local display only while a forwarded shell is connected. Closing the last such shell stops the bundled server and its windows.

## Validation

`ssh_session/x11.rs` replaces a matching setup cookie and rejects a different one, for both byte orders. It does not launch VcXsrv or connect to a real display.

## Supersedes

None.

## Revisit when

The bundled server is replaced, or Wayland-native remote display becomes a separate feature.
