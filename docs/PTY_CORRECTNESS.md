# PTY correctness boundaries

The daemon owns the PTY and authoritative terminal parser. A client socket
ending, a failed resize, and a failed output reader do not prove that the
managed process exited.

## Snapshots and compatibility

New clients advertise terminal_checkpoints on Attach. The daemon then sends
a Snapshot containing a versioned parser checkpoint and screen-history state.
The checkpoint includes both grids, their history and saved cursors, modes,
scroll regions, attributes, and the incremental vte decoder. A socket worker
validates the checkpoint before queueing it in output order; the UI installs
the prepared state without replaying a large ANSI history. Resize and
backpressure checkpoints also preserve history instead of replacing it with
an empty history.

Clients which do not advertise this capability receive the existing ANSI
snapshot instead. The history replay explicitly scrolls every history row
off the visible grid before clearing it. New clients can also consume legacy
snapshots from an older daemon, but those older daemons cannot supply a
lossless decoder checkpoint.

Checkpoint queues account for decoded state memory, not only ANSI byte
length. Partially consumed byte segments use an offset and do not repeatedly
copy their remaining tail.

When output was discarded under backpressure, the shutdown path queues an
authoritative final snapshot before Exited. This includes the case where
queueing the exit notice would itself cross the discard threshold. Socket
flushing remains time-bounded.

## Input and geometry

New clients also advertise input_replay_epochs. The bounded per-client
deduplication ledger assigns a fresh epoch when admitting a new replay window.
Input frames carry that epoch, and the client retains it with unacknowledged
bytes. An evicted window cannot silently become eligible for replay merely
because the daemon PID still matches. Expired or unverifiable retained input
stays paused; it is not automatically sent or silently discarded.

Clients without epoch support use the non-replaying input path. An updated
client connected to an older daemon cannot safely replay uncertain input if
the older daemon cannot establish a replay epoch.

Keys are encoded using the current terminal input mode. Device reports are
generated at their parser position, preserving query count and order
independently of PTY read chunking. Historical log replay does not generate
new device replies.

The PTY reader retries Interrupted, reports permanent read failures explicitly,
and distinguishes an actual EOF from its channel disappearing. Reader errors
do not trigger child termination or runtime-file cleanup.

The daemon commits parser geometry only after the OS resize succeeds.
Attached reports the applied dimensions. A client tracks pending resize
requests separately from applied dimensions. A failed candidate attach
restores the saved screen, not just its previous dimensions; if OS rollback
fails, the surviving connection is resynchronized to the actual dimensions.

## Regression gate

Focused tests are in src/bin/cokacmux_pty_tests.rs, with additional existing
PTY integration cases in src/bin/cokacmux.rs. They cover checkpoint continuation
at every byte boundary, full history preservation, device-report ordering,
key encodings, read interruption, resize failure, expired replay windows,
output-buffer allocation, and final snapshot/exit ordering.

Rust builds and tests require explicit user approval and isolated test
storage as specified by CLAUDE.md and PROJECT_POLICY.md. Formatting and
read-only syntax/manifest checks are not substitutes for that runtime gate.
