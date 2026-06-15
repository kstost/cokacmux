## Core Goal: Low-Spec Robustness

cokacmux must stay usable and correct on low-spec machines — when the disk
stalls for tens of seconds, the CPU is saturated, and memory is tight. Do not
frame slowness as an environment problem to fix outside the app; design the
app's behavior for the moment the environment is bad. Quality is judged by
what the user experiences during a 30-second disk stall, not on a fast machine.

Four principles, each grounded in a past incident:

1. **The UI thread never blocks.** No file I/O, no process spawn, no
   sleep/retry loop on the main thread. Slow work runs on workers and returns
   via `MainEvent`. (Incident: synchronous attach path froze the UI for 15.9s
   during a disk stall.)
2. **External delays must not cascade.** Show slowness honestly in the status
   line, but make second-order damage structurally impossible — e.g. queued
   key bursts must not replay as an attach/detach storm. (Incident: stalled
   input queue burst stole daemon client connections.)
3. **Recover without user intervention.** No stuck states: dead daemons leave
   the list, failed attaches clear themselves, a worker panic still produces a
   result. Removal/cleanup requires definitive evidence of death (socket file
   gone, pid dead), never a guess. (Incident: ghost agents stayed listed and
   unfocusable forever.)
4. **Diagnostics must not become the load.** Keep event logging, but heavy
   payloads (screen dumps, large samples) belong behind `--trace`. Logging is
   asynchronous and drops-with-accounting under pressure instead of blocking.
   (Incident: 90MB of debug logs in 7 minutes; log writes blocked the UI 27s.)

## Invariants (in priority order)

When changes conflict, the lower number wins.

1. **Running agents and their work are never harmed.** Kill/delete happens
   only on explicit user command. No automatic cleanup (stale sweep, rotation,
   ghost removal) may touch a live daemon's files or session data.
2. **Input goes to exactly the intended agent, in order, once.** Attach/switch
   is asynchronous; the `reader_id` routing that ties keystrokes to the
   intended target must be preserved by any refactor.
3. **The display tells the truth.** List entries, busy/quiet states, and the
   agent pane must reflect reality. An entry that cannot take focus, or a
   live connection that receives no output, is a correctness bug. Showing
   "unknown/slow" honestly beats faking a good state.
4. **The runtime-file contract is the single source of truth for daemon
   liveness.** `~/.cokacmux/agents/` socket + meta files: discovery, state
   reads, cleanup, and attach all depend on this contract. Never add a code
   path that assumes liveness without the files or death despite them.
5. **The UI always responds** (see Core Goal).
6. **Conversion contracts are explicit.** Same-provider adapter round-trips
   preserve native data; cross-provider `convert()` creates a continuation
   context wrapper, not a lossless native transcript.

Example of applying the order: never sacrifice display truth (3) for
responsiveness (5); never risk a live agent (1) to clean up the display (3).

## CRITICAL: Do Not Change Design Without Permission

- **NEVER change product design/UX without explicit user request**
- Bug fix and design change are completely different things
- If you identify a "potential improvement" or "UX issue", only REPORT it - do NOT implement
- When user says "fix it", fix only the BUGS, not your suggestions
- If you think design change is needed, ASK FIRST before implementing
- Violating this rule wastes user's time and breaks trust

## Build Guidelines

- **IMPORTANT: Only build when the user explicitly requests it**
- Never run build commands automatically after code changes
- Never run build commands to "verify" or "check" code
- Do not use `cargo build`, `python3 build.py`, or any build commands unless user asks
- Focus only on code modifications; user handles all builds manually

## Version Management

- Version is defined in `Cargo.toml` (line 3: `version = "x.x.x"`)
- All version displays use `env!("CARGO_PKG_VERSION")` macro to read from Cargo.toml
- To update version: only modify `Cargo.toml`, all other locations reflect automatically
- Never hardcode version strings in source code
