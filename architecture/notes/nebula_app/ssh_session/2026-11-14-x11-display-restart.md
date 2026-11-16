# Refresh the pooled X11 bridge when the local display restarts

## Status

Implemented; pending maintainer review.

## Context

Installing the local X server does not guarantee subsequent forwarded clients
connect. A pooled SSH connection can outlive the last shell owning a display lease.

## Evidence

Dropping the last display lease stops the local server. The next ensure creates
a new cookie, but the old bridge remains in the connection-keyed slot. Previously
request reused that slot whenever it was present, forwarding the previous cookie.
The X11 request also disabled replies, so server rejection was not observed.

## Decision

Reuse the bridge only if both local target and real authentication cookie match.
Otherwise replace its target and cookies before requesting forwarding. Ask for and
consume the SSH request reply before sending the PTY request. Existing shell-open
deadlines and the existing X11 notice handle failure.

## Rejected alternatives

- Disable authentication: unnecessary and weakens the display boundary.
- Treat successful packet submission as server acceptance: hides rejected forwarding.

## Consequences

Simultaneous shells on a live display retain the same fake cookie. Restarting the
display gives new shells a new bridge. Already-running proxies retain their own
bridge snapshot. Remote xauth/sshd configuration and Java/IGV failures still need
validation against the actual server.

## Validation

Regression tests cover a live display, a restarted display on the same port and a
changed target, alongside existing X11 setup-cookie replacement tests.

## Supersedes

None.

## Revisit when

The connection pool owns local display leases for its entire lifetime.
