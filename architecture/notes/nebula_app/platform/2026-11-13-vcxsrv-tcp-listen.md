# Bundled VcXsrv must be asked to listen on TCP

## Status

Accepted with the Windows X11 startup fix.

## Context

Placing `vcxsrv.exe` in `x11/` beside Pebrel still ended with "the built-in X11 display did not open a local port". The process was started, then killed after five seconds because nothing accepted `127.0.0.1:6000+n`.

## Evidence

VcXsrv 21 follows X.Org and does not open a TCP port unless `-listen tcp` is present. The previous command only passed `-nolisten inet6`. The success check is a TCP connect, so a running server with no TCP listener is reported as a failure and then terminated. An installer that nests `vcxsrv.exe` one directory under `x11/` was also invisible to the flat lookup.

## Decision

Pass `-listen tcp` and write the server log under the temporary authority directory. If the port never opens, or the process exits, include the last log lines in the session error. Look for `vcxsrv.exe` directly in `x11/` and in one child directory. The working directory stays the directory that contains the executable, so its fonts and libraries remain visible.

## Rejected alternatives

- Treating a live process as success without a TCP port. SSH forwarding connects to that port.
- Disabling access control with `-ac` instead of fixing the listener. The cookie check would no longer match the forwarded client.

## Consequences

A complete VcXsrv runtime can open the local display. A copy that is only the executable still fails, and the error now includes VcXsrv's own log instead of only the missing-port sentence. IPv6 stays disabled.

## Validation

`x11_server` tests check the TCP listen argument, a nested `vcxsrv.exe`, and log tailing. They do not launch the real server.

## Supersedes

None.

## Revisit when

The bundled server no longer uses the X.Org listen flag, or Windows forwarding stops using TCP.
