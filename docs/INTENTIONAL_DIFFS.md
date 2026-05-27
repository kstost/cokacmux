# Intentional Diffs vs Native Artifacts

> 작성일: 2026-05-22
> 짝 문서: [SESSION_CLONE_NATIVE_FIDELITY_STRATEGY.md](./SESSION_CLONE_NATIVE_FIDELITY_STRATEGY.md) (§3, §5, §8, §11 #13)
>
> 이 문서는 변환·clone 결과가 native artifact와 **의도적으로** 다른 지점을
> 한곳에 모은다. 각 항목은 "왜 의도적으로 남겼는지"를 명시한다. 새 diff가
> 발견되면 PASS(여기 추가) 또는 FAIL(코드 수정)로 분류한다.

---

## 1. 외부 provider runtime prelude는 그대로 재기록하지 않는다 (§3.5, §8.4)

cross-provider 변환에서 다음 카테고리는 **target visible 대화에 다시 emit하지
않는다**. 원본은 `provenance.raw` 에 보존되어 같은 provider 라운드트립 시
복원 가능.

| 필터되는 항목 | 위치 (`should_skip_foreign_runtime_context`) | 이유 |
|---|---|---|
| Codex가 만든 `<environment_context>` / `<permissions instructions>` / `<skills_instructions>` 가 prefix인 user message | `providers/claude/write.rs:495-498`, `providers/codex/write.rs:716-719` | Codex가 실행 환경에서 주입한 prelude. 다른 provider로 갈 때 진짜 user 발화처럼 재기록하면 안 됨. |
| meta flag (`flags.is_meta`) 가 있는 모든 메시지 | 동일 함수, lines 475, 695 | parser가 분류한 "대화 아님" 표식. visible 대화에서 제외. |
| 외부 provider 출처의 System/Developer 메시지 (Claude 측에서 Codex/Claude 모든 시스템 메시지, Codex 측에서 claude/codex 출처) | 동일 함수 | runtime/system 텍스트가 target native 대화로 흘러들어가는 것 차단. |

cross-provider 경로에서 **새로 만들어 넣지 않는 것** (= 0에서 1을 만들지 않음):

- Claude `attachment.deferred_tools_delta` / `skill_listing` / `permission-mode` 변경 이벤트
- Codex developer prelude (`<environment_context>` 등) — 위 필터의 대칭
- 임의의 permissions / tool budget / skill listing payload

같은 provider raw replay (`Claude→Claude`, `Codex→Codex`, `OpenCode→OpenCode`)
에서는 위 데이터가 `provenance.raw` 에서 그대로 재기록되어 bit-identical.

## 1.1 합성하되 "실값 또는 명시적 기본값"으로 표시 (§3.5 보완)

resume/list 가 동작하기 위해 구조적으로 필요한 line은 합성한다. 단,
모든 값은 (a) UniversalSession에서 가져온 실값, 또는 (b) 의도적으로
0/null/하드코드 기본값임을 식별 가능하게 한다.

| 합성되는 line | 실값 출처 | 기본값 (없을 때) |
|---|---|---|
| Codex `synth_task_started` | `session.session_id` 기반 turn_id | — |
| Codex `synth_token_count` | `session.usage_total.{input,output,reasoning,total}_tokens` | 모두 `0`. `rate_limits.primary/secondary.used_percent=0.0`, `model_context_window=258400` 은 하드코드. |
| Codex `synth_task_complete` | `last_assistant_text(session)` | `duration_ms=0`, `time_to_first_token_ms=0` 하드코드. `completed_at` 은 변환 시각. |
| OpenCode `agent-switched` / `model-switched` floor row | `session.extras["opencode_agent"]`, `session.model` | 없을 때만 1쌍 emit. 원본에 이미 있으면 합성 안 함. |

이 값들이 "실제 측정값처럼 보이지만 0/기본값" 인 점은 본 문서 §6 의 합성
상수 정책과 같은 의미다. native validation은 통과하되, 통계/budget 분석에는
신뢰 마커로 사용 가능하다.

## 2. ID 재발급 정책 (§5.3, §11 #6, #7)

| Provider | clone 시 항상 새로 발급 | 원본 그대로 유지 가능 |
|---|---|---|
| Claude | `sessionId` (파일 stem), 각 line `uuid` | `parentUuid` 체인은 새 uuid로 재매핑됨 |
| Codex | `session_meta.payload.id` (UUID v7), rollout 파일명 | `turn_context`, `response_item.id` 등 메타 식별자는 원본 유지 가능 |
| OpenCode | `session.id`(`ses_…`), `message.id`(`msg_…`), `part.id`(`prt_…`), **`session_message.id`(`evt_…`)** | `parent_id` 체인은 새 id로 재매핑 |

특히 OpenCode `evt_…`는 globally primary-keyed 이므로 원본 재사용 시
`INSERT OR REPLACE` 가 원본 row를 덮어쓴다. clone 경로는 반드시
`crate::ids::opencode_event_id()` 로 새 id를 발급한다. (강제 회귀 테스트:
`tests/install.rs`)

## 3. Claude resume-safe content block 필터 (§5.1, §11 #8)

`src/session/clone.rs::is_claude_api_content_type` 의 allowlist 와
`sanitize_claude_raw_content_for_resume` 의 필터에 걸리지 않는 content
block type 은 **visible conversation**에 내보내지 않는다. 대표적으로 다음이 걸러진다:

- OpenCode `step-start` / `step-finish`
- OpenCode 내부 control parts (`tool` 의 raw `state` 구조 등)
- 기타 `ContentBlock::Other` 중 Claude API에서 reject되는 type tag

원본 데이터는 `provenance.raw` 에 보존된다 — 다시 OpenCode로 돌아갈 때
복원 가능.

## 4. OpenCode default variant 생략 (§5.3 write strategy)

OpenCode 네이티브는 `message.data` JSON에서 `model.variant` 가 `"default"`
인 경우 해당 키 자체를 생략한다. 우리도 동일하게 생략한다.

- 구현: `providers/opencode/write.rs:511-513` (`if model.variant != "default" { value["variant"] = ... }`)
- 라운드트립 fixture에서 `variant:"default"` 케이스 사용: `tests/pivot.rs:620`

## 5. Codex `thread_source` (§5.2)

`codex exec` 가 native로 만드는 rollout에는 `thread_source` 컬럼이 NULL
이다. 우리 변환기는 source가 알려진 경우(`session_meta.payload.thread_source`)
에만 값을 채우고, 모르면 NULL로 둔다. 잘못된 추정값 (`"cli"`, `"web"` 등)
은 채우지 않는다.

- `providers/codex/install.rs` 의 threads 인서트

## 6. Claude write의 합성 상수 (의도적, 식별 가능)

cross-provider Claude write는 일부 필드를 합성한다. 이 값은 **의도적**이며,
"이 line은 cokacmux에서 합성됐다" 를 식별 가능하게 한다.

| 필드 | 합성 상수 | 위치 |
|---|---|---|
| `version` (per-line) | `SYNTHETIC_CLAUDE_VERSION` (현재 `"2.1.147"`) | `providers/claude/write.rs:57` |
| `entrypoint` | `"sdk-cli"` | `providers/claude/write.rs:418` |
| `userType` | `"external"` | `providers/claude/write.rs:417` |
| `gitBranch` (없을 때) | `"HEAD"` | `providers/claude/write.rs:437` |

이 값은 Claude resume이 허용하는 범위 안의 일반값이다. native Claude 본인이
생성한 line의 version은 `origin.cli_version` 에 보존되며, 같은 provider
raw replay 시 원본 그대로 재기록된다.

## 7. cross-provider session 메타 손실 (§5.1, §5.2, §5.3)

다음 메타는 cross-provider 경로에서 **target native shape에 1:1 매핑이
없으므로** 손실된다. UniversalSession에는 `provenance.raw` / `extras` 로
보존되어 같은 provider 라운드트립 시 복원된다.

| 원본 → 대상 | 손실되는 메타 |
|---|---|
| Codex → Claude | `turn_context.model_reasoning_effort`, `event_msg.token_count.info` 세부 |
| Codex → OpenCode | `task_started/complete` 마커 (OpenCode에 1:1 대응 이벤트 없음) |
| Claude → Codex | `ai-title` / `custom-title` 구분 (Codex는 단일 title 슬롯) |
| Claude → OpenCode | `permission-mode` 이벤트 시퀀스 (control event로 변환되지 않음) |
| OpenCode → Claude | `session_message.agent-switched` (Claude attachment로 합성하지 않음 — §1 참조) |
| OpenCode → Codex | `session_message.model-switched` 의 중간 분기 (UniversalSession.model 최종값만 반영) |

## 8. 시간 정밀도 / timestamp 단위 (§3.4)

provider별 timestamp 단위가 다르다:

- Claude: RFC3339 ms 정밀도
- Codex: RFC3339 ms 정밀도
- OpenCode: epoch ms (`time_created`, `time_updated`)

cross-provider 변환 시 단위 변환에서 ±1ms 의 정밀도 손실이 발생할 수 있다.
이는 native validation을 깨지 않으며, semantic profile 비교는 round-trip
preserving이다. 의도적 손실로 분류한다.

---

## 9. 의도적 diff가 **아닌** 항목 (= 발견 시 버그)

다음은 diff가 발견되면 코드 버그로 분류하고 즉시 수정한다 (회귀 테스트 추가):

- session_id / 파일명 stem 불일치
- `session_meta.payload.id` 와 session_id 불일치 (Codex)
- OpenCode `session.directory` 또는 `path` 가 비어 있음
- OpenCode `model` 컬럼이 plain string (JSON object 가 아님)
- OpenCode `project_id` 가 `'global'` 아님 (top-level session의 경우)
- assistant message 가 두 번 row로 들어감
- tool_use ↔ tool_result `call_id` 페어링이 깨짐
- `INSERT OR REPLACE` 가 원본 row를 덮어씀

위 항목 중 §9의 모든 케이스는 `src/session/native_validate.rs` 와
`tests/install.rs`, `tests/live_acceptance.rs` 의 회귀 게이트에 잠겨 있다.

---

## 10. 변경 이력

| 날짜 | 변경 |
|---|---|
| 2026-05-22 | 초안 (§11 #13 충족용) |
