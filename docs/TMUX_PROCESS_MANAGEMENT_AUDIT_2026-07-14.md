# tmux process-management audit (2026-07-14)

## Scope

This audit compares cokacmux commit
`d3308e80ecd13fe5e7f7801f7f9300f3dc40623d` with upstream tmux commit
`dba85ee32e2941c96608602ce010bb03faaa57c8` (2026-07-13). It focuses on
process and PTY ownership, client detach, input ordering, child reaping, and
final-output handling. Checksums, signatures, signed manifests, and attestations
are explicitly not required by project policy and their absence is not a
defect. GitHub Actions and `.github/workflows` are likewise intentionally not
used. See `docs/PROJECT_POLICY.md`.

Relevant upstream sources:

- `server-client.c`: client key events enter the same command queue as commands,
  so a key after `switch-client` resolves its pane after the switch.
- `window.c`: input is written through an explicit `window_pane *`; pane
  destruction waits for both child exit and pending PTY/pipe output to drain.
- `server.c`: SIGCHLD is reaped with `waitpid`, while losing a client only
  removes that client and leaves sessions and panes owned by the server.
- `job.c`: process death and fd closure are tracked separately, and I/O is
  driven by nonblocking buffers.

Upstream links:

- <https://github.com/tmux/tmux/blob/dba85ee32e2941c96608602ce010bb03faaa57c8/server-client.c>
- <https://github.com/tmux/tmux/blob/dba85ee32e2941c96608602ce010bb03faaa57c8/window.c>
- <https://github.com/tmux/tmux/blob/dba85ee32e2941c96608602ce010bb03faaa57c8/server.c>
- <https://github.com/tmux/tmux/blob/dba85ee32e2941c96608602ce010bb03faaa57c8/job.c>

## Existing alignment

cokacmux already follows the most important tmux ownership rules:

- An agent daemon, not the TUI client, owns the child process and PTY. Client
  detach and socket failure preserve running work.
- PTY input, output, and disk writes use dedicated workers and bounded queues,
  keeping the daemon/UI loops responsive under backpressure.
- Explicit termination revalidates process identity with PID start tokens and
  preserves runtime state when liveness is uncertain.
- Child exit already triggers a final output drain before the exit event and
  runtime cleanup.

## Defects found and changes applied

### 1. Input could cross an asynchronous focused-process transition

The old `active_agent` stayed installed while a switch worker connected to the
new daemon. A normal key or paste arriving in that interval fell through to the
old agent. Opening a right panel that would receive focus had the analogous
risk: input could reach the main process before the auxiliary PTY was ready.
tmux avoids this class of bug by serializing switch commands and key callbacks
in one command queue and resolving the pane when the callback runs.

Applied behavior:

- Terminal-bound keys and pastes are retained while a main-agent attach or a
  focus-taking auxiliary attach is in flight. Background auxiliary restores
  that do not take focus do not pause main-agent input. UI commands such as
  another switch or quit remain immediately responsive.
- Every retained event records the requested `AgentKey`. When the attach chain
  settles, input is replayed only if that exact key is active.
- The completed attach re-resolves both pane sizes from the current terminal
  dimensions before replay, so a resize that occurred while connecting cannot
  leave the new PTY on stale geometry.
- Input for a failed or superseded target is discarded with an explicit status
  instead of falling back to the old or newest unrelated agent.
- The queue is bounded to 4,096 events and 8 MiB. Replay uses the existing
  in-memory client writer queue and completes before the next UI event, keeping
  terminal input ordered ahead of any later switch command.

This provides the same essential ordering guarantee as tmux's command queue:
the switch is resolved before later terminal input chooses its process.

### 2. Input acknowledgement preceded the actual PTY write

The daemon previously sent `InputAccepted` after a frame entered its in-memory
writer FIFO. If `write_all` or `flush` then failed, the client had already
dropped its recovery copy and the input was lost.

Applied behavior:

- Sequenced input jobs carry their client instance and sequence into the PTY
  writer.
- The writer reports a completion only after `write_all` plus `flush` succeeds.
- Only that completion records the dedupe watermark and queues
  `InputAccepted`. A failed write remains unacknowledged and disconnects the
  client while preserving the managed process.
- Retries received while the original sequence is still in flight are deduped
  without being acknowledged early. A retry of an already completed sequence
  is acknowledged immediately without writing it twice.

The acknowledgement means “accepted by the PTY write boundary,” not “the child
application consumed or interpreted the bytes,” which is the strongest
guarantee a byte-stream PTY can provide.

### 3. Final output drain used an idle-time guess instead of reader EOF

After `try_wait` reported child exit, the daemon stopped its final drain after
250 ms without a chunk. A delayed PTY reader could therefore lose the child's
last output. tmux keeps a pane until child status is ready and readable output
has drained.

Applied behavior:

- `drain_output_chunks` now distinguishes an empty channel from a disconnected
  PTY reader.
- The exit path drains until the reader reports channel closure (PTY EOF), with
  the existing two-second bound retained only as protection against a broken
  platform reader.
- The exit notice remains ordered after this final drain.

### 4. Runtime endpoint repair could remain permanently in flight

The daemon repairs a deleted socket/meta/auth endpoint on a worker so a stalled
filesystem cannot block PTY pumping. The worker previously had no watchdog;
one stuck repair left `runtime_repair_in_flight` set forever. It also held a
newly rebound listener until meta/auth file work completed, delaying the point
at which the daemon could accept clients again.

Applied behavior:

- Socket rebinding is now a separate first stage. Its listener returns to the
  accept loop before any meta/auth file repair begins, matching tmux's
  recreate-and-resume-accept ordering.
- Meta/auth repairs run on the following observation and clear only flags they
  actually attempted; a late result cannot falsely mask a file deleted after
  its snapshot.
- A 60-second watchdog abandons a stalled observation without treating the
  managed child as dead. At most one replacement runs beside one abandoned
  worker, preventing retry storms; late results reclaim capacity and successful
  listeners can still be adopted.

## Regression coverage added

- Input is not sent to the old agent while a main switch is pending.
- Deferred input is replayed only after its exact target becomes active.
- Failed attaches never fall back to the old agent for deferred input.
- Input waits for a focus-taking auxiliary PTY rather than leaking into its
  parent process.
- Queue admission alone emits no `InputAccepted` and does not advance the
  accepted-sequence watermark.
- A successful PTY write produces a completion eligible for acknowledgement.
- A failed PTY write produces only a failure completion and can never be
  reported as accepted.
- The PTY output reader reports channel closure after child exit, which is the
  final-drain completion signal.
- Runtime repair prioritizes a live listener over file repair, and its watchdog
  admits at most one bounded replacement.

## Validation status

`cargo fmt --all -- --check` and `git diff --check` pass. Project policy
requires separate approval before build or Rust test execution, so compilation
and the Rust test suite have not been run as part of this audit yet.
