# Prefer the compatibility display after incomplete IGV rendering

## Status

Implemented for local validation; pending maintainer review.

## Context

The user confirmed yserver opens IGV but leaves parts of its drawing missing.

## Evidence

The previous readiness check verifies authentication only. A server that accepts
setup never reaches startup fallback, even if an application later renders poorly.
The user's IGV result therefore invalidates using successful setup as application
compatibility evidence. The SSH proxy forwards image bytes without interpretation.

## Decision

Retain resident display ownership and authenticated setup. Prefer bundled or
installed VcXsrv before yserver. `PEBREL_X11_BACKEND=yserver` explicitly prefers
yserver; `vcxsrv`, `auto`, unset or unknown values prefer VcXsrv. The other backend
remains a startup fallback. Preference is resolved once when a display starts;
restart Pebrel to change it, and reconnect SSH to obtain the new display.

Repair the explicit VcXsrv fetch script: pin the upstream x64 archive and license,
extract it using a supplied 7-Zip executable instead of executing its NSIS installer,
retain fonts/XKB/data, license and corresponding-source link, and omit installer
helpers and unrelated client executables. No startup download or system install.

## Rejected alternatives

- Treat authenticated setup as IGV acceptance: contradicted by the user's report.
- Restart the display automatically after a rendering complaint: existing X clients
  would disconnect, and the forwarding layer cannot infer visual correctness.
- Replace or patch IGV's Java rendering preferences globally: remote application
  policy belongs to the user, and the faulty rendering path has not been isolated.

## Consequences

The local x64 validation installer carries both runtimes. VcXsrv is selected
without requiring a separate installation. Optional yserver and startup fallback
remain available. This is a compatibility workaround, not a yserver renderer fix.

## Validation

Backend-order tests cover defaults, explicit preference and stable candidate order.
The opt-in native test authenticates VcXsrv, rejects a wrong cookie, round-trips
four image pixels through PutImage/GetImage and checks owned process cleanup.
Remote IGV visual acceptance still requires the user's environment.

## Supersedes

Backend priority in [resident display ownership](2026-11-16-resident-x11-backends.md).
The lifetime, authentication and fallback rules otherwise remain applicable.

## Revisit when

The actual IGV rendering defect is isolated or yserver passes remote visual acceptance.
