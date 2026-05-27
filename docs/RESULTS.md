# cokacmux — measured results

> 2026-05-20. All numbers below come from running this binary against the
> maintainer's real Claude / Codex / OpenCode session data on this machine
> (Linux aarch64, cargo 1.95.0). Reproduce with the commands shown.

## Environment

| Component | Version |
|---|---|
| host | Linux 6.8.0-111-generic aarch64 |
| rustc / cargo | 1.95.0 |
| Claude Code | 2.1.145 |
| codex-cli | 0.131.0 |
| opencode-ai | 1.15.5 |

## Build

```text
$ cargo build --all-features
    Finished `dev` profile [unoptimized + debuginfo] target(s)

$ cargo build --no-default-features --features claude,codex
    Finished `dev` profile [unoptimized + debuginfo] target(s)
```

Feature gating verified: `claude,codex` alone does NOT pull in `rusqlite`
(no SQLite C compile).

## Test suite (`cargo test --all-features`)

```text
running 5 tests           (unit)                                   ok
running 0 tests           (bin)                                    ok
running 5 tests           (tests/install.rs)                       ok
running 3 tests           (tests/live_acceptance.rs)               3 ignored
running 3 tests           (tests/pivot.rs)                         ok
running 5 tests           (tests/roundtrip.rs)                     ok
running 1 test            (doc-test for src/lib.rs)                ok
                                                              ─────
                                                              19/19 pass + 3 ignored (live)
                                                              22/22 pass when live tests included
```

Specific tests of interest:
- `claude_same_provider_roundtrip_is_bit_identical` — `from_claude → to_claude`
  yields the source string byte-for-byte.
- `codex_same_provider_roundtrip_is_bit_identical` — likewise for Codex.
- `opencode_roundtrip_preserves_message_text` — read tempfile DB → write
  to a fresh tempfile DB → re-read, user/assistant text identical.
- `three_provider_pivot_preserves_text` — `codex → claude → opencode → codex`
  pivots preserve user/assistant text.
- Install tests confirm the on-disk layouts in tempdir homes.
- `codex_install_updates_threads_table` — install populates the codex
  `threads` index with NOT NULL columns valid against the live schema.
- `codex_install_against_live_state_5_clone` — installs into a copy of
  the user's actual `state_5.sqlite`; proves real-schema compatibility.

## Real-data smoke test

Listing what each provider has stored on this machine:

```text
$ cokacmux list --provider claude --limit 5
PROV     SESSION_ID                             MTIME      CWD
claude   a651c028-785f-447d-bd7f-a12d560e1275   1779249775 /mnt/hgfs/vmware_ubuntu_shared/cokacmux
claude   aecdfa0d-4f07-4768-a10b-118ea0aa6b23   1779245132 /home/kst/123
claude   183ed0f9-fc45-4df0-9c3b-d9932483496f   1779231583 /mnt/hgfs/vmware_ubuntu_shared/cokacmux
claude   7a34c35a-d26f-4cf8-afe4-6ab2111c8afc   1779202811 /home/kst/.cokacmux/workspace/rtm0u9ge
claude   28681082-14a6-43dd-a44b-04bde29209ef   1779199914 /mnt/hgfs/vmware_ubuntu_shared/cokacmux

$ cokacmux list --provider codex --limit 5
PROV     SESSION_ID                             MTIME      CWD
codex    019e4344-f984-7250-b925-168de132413f   1779245058 /home/kst/123
codex    019e40c1-9538-7781-9462-e9fcf559adcf   1779202892 /home/kst/4
codex    019e40c0-f44f-7652-80e0-895c302120b4   1779202850 /home/kst/4
codex    019e3e9f-1859-74b1-96f5-98487b7a239f   1779167081 /tmp
codex    019e3e9b-9c7b-7003-bf35-c66c5ed2edfa   1779166847 /tmp

$ cokacmux list --provider opencode --limit 5
PROV     SESSION_ID                             MTIME      CWD
opencode ses_1bcbac7d3ffeQ6QyJn54Ri3O5E         1779245072 /home/kst/123
opencode ses_1bf3e0c5cffeAXffUHc25Qmkea         1779202913 /home/kst/4
opencode ses_1c173bbb6ffeRt4HazX7SnL16w         1779165841 /mnt/hgfs/vmware_ubuntu_shared/cokacmux
opencode ses_1c1a27c2dffeICl63ru5odHmci         1779162778 /home/kst
opencode ses_1d110a179ffeyf4YHk3zPdflR6         1778903909 /tmp/opencode-test
```

### Inspect (one session per provider)

The three providers' "Hello / Hi" sessions for `/home/kst/123` (matched
across providers by cwd):

| Source | Session ID | Messages | Roles | Source events |
|---|---|---|---|---|
| Claude | `aecdfa0d-…` | 9 | system, user, assistant | `claude:user`, `claude:assistant`, `claude:attachment`, `claude:permission-mode`, `claude:file-history-snapshot`, `claude:last-prompt`, `claude:system` |
| Codex | `019e4344-…` | 11 | system, developer, user, assistant | `codex:session_meta`, `codex:turn_context`, `codex:event_msg.task_started`, `codex:event_msg.task_complete`, `codex:event_msg.token_count`, `codex:event_msg.user_message`, `codex:event_msg.agent_message`, `codex:response_item.message` |
| OpenCode | `ses_1bcbac7d3ffe…` | 2 | user, assistant | `opencode:message.user`, `opencode:message.assistant` |

All three parsed without errors; all user/assistant text bodies came
through cleanly:

```text
SRC claude   user/assistant texts: ['hi', 'Hi! What would you like to work on?']
SRC codex    user/assistant texts: ['<environment_context>...', 'hi', 'Hi. What do you want to work on?']
SRC opencode user/assistant texts: ['hi', 'Hi. How can I help?']
```

### Same-provider round-trip (bit-identical)

```text
$ cokacmux convert --from claude --to claude \
      --input  /home/kst/.claude/projects/-home-kst-123/aecdfa0d-...jsonl \
      --output /tmp/claude.rt.jsonl
ok: 9 messages → /tmp/claude.rt.jsonl (claude)
$ diff /home/kst/.claude/projects/-home-kst-123/aecdfa0d-...jsonl /tmp/claude.rt.jsonl
exit=0   # bit-identical

$ cokacmux convert --from codex --to codex --input <rollout>.jsonl --output /tmp/codex.rt.jsonl
ok: 11 messages → /tmp/codex.rt.jsonl (codex)
$ diff <rollout>.jsonl /tmp/codex.rt.jsonl
exit=0   # bit-identical
```

### Cross-provider round-trip (text-preserving)

```text
OK  claude->codex:    2 text body/bodies preserved
OK  claude->opencode: 2 text body/bodies preserved
OK  codex->claude:    3 text body/bodies preserved
OK  codex->opencode:  3 text body/bodies preserved
OK  opencode->claude: 2 text body/bodies preserved
OK  opencode->codex:  2 text body/bodies preserved
fail=0/6
```

### Heavy-tool-call session

The current cokacmux working session (`a651c028-…`, 707 JSONL lines,
**212 tool_use / 212 tool_result pairs**, 32 text turns) — the most
substantial real session available — converted in three directions:

```text
SRC    tool_use=212  tool_result=212  text= 32  (claude)
CODEX  tool_use=212  tool_result=212  text= 32  (claude → codex)
OPENCD tool_use=212  tool_result=212  text= 32  (claude → opencode.db)

unique call_ids preserved: 212 / 212 paired in every destination
```

Three-step pivot `claude → codex → claude` of the same session:

```text
src: texts=32  tool_use=212  tool_result=212
dst: texts=32  tool_use=212  tool_result=212
texts_match      = True
tool_uses_match  = True   (name + call_id pairs)
tool_results_match = True (call_id pairs)
```

This caught and fixed two bugs that the simple "hi" sessions could not have:

1. **Claude reuses `message.id` across streamed chunks** — the reader was
   keying `UMessage.id` off `message.id` which broke the OpenCode INSERT
   (`PRIMARY KEY message.id`). Fixed by keying off the line-unique `uuid`
   field instead; `message.id` is preserved in extras.
2. **OpenCode `tool` part read/write asymmetry** — the write path emitted
   separate parts for ToolUse and ToolResult, but the read path emitted
   both blocks for any part with `status: completed`. The result was a
   2× block inflation on roundtrip. Fixed by having the reader only emit
   ToolUse when the part carries `input`, and only emit ToolResult when
   the part carries `output`.

### Four-step pivot (`codex → claude → opencode → codex`)

```text
ok: 11 messages → /tmp/.../1.claude.jsonl (claude)
ok: 11 messages → /tmp/.../2.oc.db (opencode)
ok: 11 messages → /tmp/.../3.codex.jsonl (codex)

SRC user/assistant texts: ['<environment_context>...', 'hi', 'Hi. What do you want to work on?']
DST user/assistant texts: ['<environment_context>...', 'hi', 'Hi. What do you want to work on?']
preserved = True
```

## Live agent-acceptance tests

Run with `cargo test --all-features -- --ignored live`. These tests
write to the user's REAL agent storage directories using fresh,
non-colliding UUIDs and clean up after themselves:

```text
test live_claude_install_and_resume_path ... ok
  installed: /home/kst/.claude/projects/-home-kst-123/<NEW_UUID>.jsonl
  re-parsed and verified session_id/cwd; cleaned up.

test live_codex_install_with_threads_index ... ok
  installed rollout: /home/kst/.codex/sessions/<YYYY/MM/DD>/rollout-…-<NEW_UUID>.jsonl
  threads row in LIVE state_5.sqlite: id=<NEW_UUID> source=cli
    sandbox_policy={"type":"read-only"} approval_mode=on-request
  cleaned up rollout file + DELETE FROM threads.

test live_opencode_install_and_list ... ok
  installed to /home/kst/.local/share/opencode/opencode.db (2 messages)
  `opencode session list` shows our id: true   ← agent CLI confirms
  cleaned up session/message/part rows.
```

The opencode case is the strongest evidence: **opencode's own CLI lists
our injected session**, which means `to_opencode_db` produces rows that
satisfy every constraint and shape requirement opencode itself enforces.

### Two real-data bugs the live test caught and fixed

1. **OpenCode `session.model` must be a JSON-stringified object**
   `{"id","providerID","variant"}`. A plain `"openai/gpt-5.5"` string
   makes `opencode session list` silently drop the row. Fixed in
   `providers::opencode::write::to_db_connection`.
2. **OpenCode CLI sessions all live under `project_id='global'`** (the
   special catch-all project). Our writer was synthesizing a per-cwd
   project id, which kept rows out of the picker. Fixed by using
   `'global'` and inserting the corresponding project row.

## Other notes

- `model` field of an OpenCode session row is a JSON-stringified object;
  the reader parses it back into a proper `ModelInfo`. (Verified on a
  live `opencode.db` v1.15.5 row.)
- Cross-provider synthesis preserves *text bodies* of user/assistant
  turns. Provider-specific meta (e.g. Claude's `attachment.deferred_tools_delta`,
  Codex's `event_msg.task_complete`) round-trips perfectly when the
  destination matches the source (replay of `provenance.raw`), and is
  preserved as `ContentBlock::Other` / system-meta in cross-provider
  hops.
- Phase-3 `install` is exposed as library API only (not CLI). Tempdir
  unit tests exercise schemas; live-acceptance tests run end-to-end
  against the user's real directories with cleanup.
