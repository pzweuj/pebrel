# Handle ownership during segmented SFTP download recovery

## Status

Implemented.

## Context

A remote READ can fail after other segments have already reached disk. Sequential recovery reduces concurrent request pressure while preserving completed local writes.

## Evidence

A server can atomically replace a pathname while existing handles still refer to its previous contents. Equal file sizes allow a mixed download to pass a byte-count check. In russh-sftp 2.3.0, dropping a file sends CLOSE without updating the client-side negotiated handle count.

## Decision

The download owns all successful opens. Workers borrow their segment handles, and each failed range gets one sequential retry on its original handle starting at the first uncommitted byte. The owner awaits CLOSE acknowledgements even after a read failure or cooperative cancellation.

## Rejected alternatives

Reopening the path for the remaining ranges can combine different versions. Relying on file drop can exhaust the client quota on a reused session.

## Consequences

Segment handles remain open during sequential recovery. Cleanup can take a CLOSE round trip before the caller observes completion or cancellation. Existing handles preserve identity across pathname replacement but cannot prevent in-place modifications by a remote writer.

## Validation

The [codec tests](../../../../nebula_app/src/ssh_sftp/transfer_io_tests.rs) replace a same-size file during recovery and exercise negotiated handle limits after success, early EOF and cancellation.

## Supersedes

None.

## Revisit when

The SFTP library supports concurrent offset reads on a single handle or changes its acknowledged-close accounting.
