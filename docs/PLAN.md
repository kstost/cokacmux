# cokacmux 작업계획서

> 작성일: 2026-05-20 (rev. 2)
> 대상: cokacmux(코딩 에이전트 설치/서비스 매니저) → **cokacmux**(에이전트 세션 데이터 상호변환기) 전환.

---

## 0. 한 줄 요약

세 코딩 에이전트(**Claude Code / Codex / OpenCode**)의 세션 데이터를 무손실로 담을 수 있는 공통 데이터 모델 `UniversalType`을 정의하고, 각 provider 포맷과 `UniversalType` 사이의 양방향 변환기를 **라이브러리 crate**로 제공하여, 다른 앱(예: cokacmux)이 의존성으로 끼워 쓸 수 있게 한다. CLI 바이너리는 라이브러리를 얇게 감싼 frontend.

```
codex   ──from_codex──▶   UniversalType   ──to_claude──▶   claude
                              ▲   │
                              │   ▼
                         opencode ↔ …
```

---

## 1. 배경 — 전환에 앞서 수행한 확인 작업

이 섹션은 **계획 수립에 앞서 사용자에게 요청한 확인 사항과 그 확인 결과(조사 결과 포함)** 를 모두 기록한다.

### 1.1 cokacmux → cokacmux 전환 자체에 대한 확인

| 질문 | 답 (확정) |
|---|---|
| 새 기능의 도메인 | "에이전트 세션 데이터 상호변환기" (본 미션) |
| 기존 `src/` 코드 처리 | **build 인프라만 참고. `src/`는 전부 폐기**. |
| MVP 범위 | **라이브러리 crate 중심 모듈화 + CLI**. 다른 앱에 이식 수월한 형태가 1차 목표. |
| OpenCode 입출력 | **rusqlite로 직접** (feature gate 처리). |

### 1.2 옵션 답을 위한 사전 조사 — 세 에이전트의 세션 저장소 위치

본 미션의 입출력 양 끝단을 정의하려면 각 에이전트가 세션을 디스크 어디에·어떤 포맷으로 저장하는지부터 확정해야 했다. 직접 실행·DB 조회·파일시스템 검사로 확인한 결과를 아래에 옮긴다 (현 시스템 기준).

#### 1.2.1 OpenCode (`opencode-ai 1.15.5`)

- **루트**: `~/.local/share/opencode/` (XDG `data` 슬롯)
- **세션 본문 = SQLite**: `~/.local/share/opencode/opencode.db` (Drizzle ORM)
- 주요 테이블: `project`, `session`, `message`, `part`, `todo`, `event`, `workspace`, `session_message`, `session_share`, `permission`
  - `session(id, project_id, slug, directory, title, agent, model, cost, tokens_*, time_created, time_updated, …)`
  - `message(id, session_id, time_*, data JSON)` 한 메시지 = 한 행, 본문은 JSON 컬럼
  - `part(id, message_id, session_id, time_*, data JSON)` 메시지 안의 부분(텍스트/툴콜 청크)
- **파일 스냅샷**: `~/.local/share/opencode/snapshot/<project_id>/<worktree_sha1>/` — bare git repo 형태 (변환 대상은 아님)
- **레거시 JSON 잔재**: `~/.local/share/opencode/storage/session_diff/ses_*.json` (대부분 `{}`)
- **CLI**: `opencode session list|delete`, `opencode export <sessionID>`, `opencode import <file>`, `opencode db "<SQL>"`

#### 1.2.2 Codex (`codex-cli 0.131.0`)

- **루트**: `~/.codex/` (XDG 미사용·통합 디렉터리)
- **세션 본문 = JSONL 롤아웃**: `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO_TS>-<UUID>.jsonl`
  - 파일명 UUID = 세션 ID. 한 줄 = 한 이벤트.
  - 이벤트 타입: `session_meta`, `turn_context`, `event_msg`(내부 `user_message`/`agent_message`/`token_count`/`task_started`/`task_complete` 등), `response_item`(내부 `message`/`function_call`/`function_call_output`/`custom_tool_call*`/`reasoning`)
- **세션 인덱스 = SQLite**: `~/.codex/state_5.sqlite`
  - `threads(id, rollout_path, cwd, model, agent_role, first_user_message, tokens_used, git_*, archived, archived_at, …)`
  - 보조 테이블: `thread_goals`, `thread_spawn_edges`(fork), `thread_dynamic_tools`, `stage1_outputs`, `agent_jobs(_items)`, `jobs`, `backfill_state`, `remote_control_enrollments`
- **디버그 로그(세션 본문 아님)**: `~/.codex/logs_2.sqlite`(`logs`), `~/.codex/log/codex-tui.log`
- **전역 입력 히스토리**: `~/.codex/history.jsonl`
- **CLI**: `codex resume [<UUID>|--last]`, `codex fork`, 공식 export/import 명령 없음

#### 1.2.3 Claude Code (`2.1.145`)

- **루트**: `~/.claude/` (XDG 미사용)
- **세션 본문 = JSONL**: `~/.claude/projects/<인코딩-cwd>/<session-uuid>.jsonl` (모드 0600)
  - cwd 인코딩: `/`, `.`, `_` → 모두 `-` 치환
    - `/mnt/hgfs/vmware_ubuntu_shared/cokacmux` → `-mnt-hgfs-vmware-ubuntu-shared-cokacmux`
    - `/home/kst/.cokacmux-workspace-280AE0F2` → `-home-kst--cokacmux-workspace-280AE0F2` (점 → `--`)
  - 이벤트 타입(실측): `message`, `user`, `ai-title`, `permission-mode`, `last-prompt`, `file-history-snapshot`, `task_reminder`, `system`, `skill_listing`, `deferred_tools_delta`, `attachment`, `queue-operation` 등
- **세션 사이드카 디렉터리**: `~/.claude/projects/<인코딩-cwd>/<session-uuid>/`
  - `tool-results/<랜덤>.txt` — 큰 툴 결과의 외화 저장
  - `memory/`는 인코딩-cwd의 형제 디렉터리에 별도 존재
- **실행 중 세션 레지스트리**: `~/.claude/sessions/<PID>.json` (모드 0700)
- **보조**: `todos/`, `tasks/<uuid>/`, `file-history/<dir-hash>/<file-id>@vN`, `shell-snapshots/`, `history.jsonl`
- **CLI**: `claude -c|--continue`, `claude -r|--resume`, `--session-id <uuid>`, `--fork-session`, `--no-session-persistence`. 공식 export/import 없음.

#### 1.2.4 세 도구 비교 (확인 결과 요약)

| 항목 | OpenCode | Codex | Claude Code |
|---|---|---|---|
| 루트 | XDG 분산 (`~/.local/share/opencode` 외) | `~/.codex/` 통합 | `~/.claude/` 통합 |
| **세션 본문** | **SQLite** (`opencode.db`) | **JSONL** | **JSONL** |
| **인덱스** | DB 자체 | **SQLite** (`state_5.sqlite::threads`) | 없음(JSONL+PID 라이브파일) |
| 사이드카 | 없음 (DB blob) | 없음 | `<uuid>/tool-results/` 외화 |
| Export/Import | `opencode export/import` | 없음 | 없음 |
| 변환 시 읽기 비용 | rusqlite로 SELECT | JSONL line-by-line | JSONL line-by-line + 사이드카 |
| 변환 시 쓰기 비용 | DB row INSERT | JSONL write + `state_5.sqlite` INSERT | JSONL write (+ 선택적 사이드카) |

### 1.3 레퍼런스 앱 조사 — `cokacmux`

본 미션과 유사한 일을 단방향으로 하는 선행 작업. 구조(`src/services/session_archive.rs`):

- 정규화 스키마 `FullSession { session_id, provider, cwd, model, git, session_meta, messages: Vec<Message> }`
- `Message { role, source: "provider:ev-type", content: Vec<ContentBlock>, raw: Value }` — `raw`로 원본 1줄 보존
- `ContentBlock { kind: text|thinking|tool_use|tool_result|patch|…, text, tool_name, tool_id, tool_input, tool_output, is_error, extra }`
- provider별 파서: `parse_claude`, `parse_codex`, `parse_opencode`(rusqlite), `parse_gemini`
- 출력: `~/.cokacmux/ai_sessions_full/<sid>.json` (atomic write, source mtime ≤ target mtime이면 skip)

**우리 미션과의 차이**: cokacmux의 `FullSession`은 **단방향(에이전트 → 정규화)** 만 다룬다. 본 미션은 **양방향**이며, 정규화 결과로부터 원본 포맷을 다시 합성하는 `to_X` 함수가 추가 분량의 핵심. 또한 **라이브러리 crate로 빠져 있어 cokacmux 자체가 의존성으로 흡수할 수 있어야 한다** — 이 점이 본 사이클의 핵심 비-기능 요구사항(non-functional requirement).

---

## 2. 미션 정의

### 2.1 목표
1. `UniversalType` 정의: 세 provider의 모든 세션 정보를 의미적 손실 없이 담는 데이터 구조.
2. 변환 함수 6종 구현:
   - `from_codex(...)    → UniversalType`
   - `from_claude(...)   → UniversalType`
   - `from_opencode(...) → UniversalType`
   - `to_codex(UniversalType, ...)    → codex artifact`
   - `to_claude(UniversalType, ...)   → claude artifact`
   - `to_opencode(UniversalType, ...) → opencode artifact`
3. 임의 provider X → Y 변환은 `from_X → to_Y` 합성으로 표현된다(피벗 경유 강제).
4. **라이브러리 crate로 제공한다.** 다른 Rust 앱이 `cokacmux = "0.1"` 한 줄로 의존 가능. CLI 바이너리는 라이브러리를 얇게 감싼 frontend.
5. **provider별·CLI 의존성을 Cargo features로 분리.** 라이브러리 사용자가 필요 없는 provider는 빌드에서 제외 가능.

### 2.2 비목표 (이번 사이클에서 다루지 않음)
- LLM 호출, 에이전트 자체 동작, 실시간 스트리밍 처리
- Gemini 지원 (참고만, MVP 범위 밖 — 추후 plugin 형태로 추가 가능)
- 파일 스냅샷·git snapshot·shell snapshot 등 세션 **주변** 아티팩트 변환 (대화 내용·툴 사용 기록만)
- 변환된 결과를 에이전트의 **라이브 저장소에 자동 설치**하는 기본 CLI 명령 (라이브러리 API로는 노출 — §5 Phase 3 참고)

### 2.3 정합성 기준
- **A → U → A** (왕복): 시각적 식별자(자동 생성 ts, 자동 부여 id)를 제외하면 의미적으로 동일.
- **A → U → B → U → A** (피벗 왕복): provider 고유 필드는 유실되어도 좋으나, 대화 흐름(role, 텍스트 본문, 툴 호출/응답 쌍, 사고 과정의 존재 여부)은 보존.
- 어떤 provider의 어떤 이벤트 타입도 **silently dropped** 되어서는 안 된다 → 미지 항목은 `extras` 슬롯에 격리 보존.

---

## 3. `UniversalType` 설계

### 3.1 설계 원칙
- **Provider-agnostic 표면 + Provider-specific 확장**: 1급 필드는 `role: User|Assistant|Tool|System|Developer` 처럼 추상화, 각 provider 고유 필드는 `extras: BTreeMap<String, Value>`.
- **무손실 원칙**: 메시지 단위로 원본 1줄(또는 1행)을 `provenance.raw`에 보관 (cokacmux의 `raw: Value` 트릭 계승).
- **Tool call 짝짓기**: 모든 `ToolUse`는 `call_id`를 갖고, 대응 `ToolResult`는 같은 `call_id` 참조. provider가 안 주면 합성.
- **Discriminated union으로 콘텐츠 블록 표현**: serde의 `#[serde(tag = "kind")]` JSON serde 친화적.
- **외부 의존성 0**: `UniversalType` 자체는 `serde` + `serde_json` + `chrono`만 의존. rusqlite·uuid 같은 무거운 dep은 provider 어댑터 쪽으로.

### 3.2 스키마 (Rust 의사코드)

```rust
pub struct UniversalSession {
    pub schema_version: String,        // "ut/1.0"
    pub session_id: String,            // UUID v7 권장. 원본 ID가 UUID면 그대로.
    pub origin: ProviderOrigin,
    pub cwd: String,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub title: Option<String>,         // claude customTitle / codex first_user_message 등
    pub model: Option<ModelInfo>,
    pub git: Option<GitInfo>,
    pub usage_total: Option<Usage>,
    pub session_meta: Option<Value>,   // session_meta 페이로드 통째
    pub messages: Vec<UMessage>,
    pub extras: BTreeMap<String, Value>,
}

pub struct ProviderOrigin {
    pub provider: Provider,
    pub cli_version: Option<String>,
    pub source_path: Option<String>,   // JSONL 경로 또는 "opencode.db#session_id"
    pub source_mtime: Option<DateTime<Utc>>,
}

pub enum Provider { Codex, Claude, OpenCode }

pub struct UMessage {
    pub id: String,
    pub parent_id: Option<String>,
    pub index: u32,
    pub timestamp: Option<DateTime<Utc>>,
    pub role: Role,
    pub model: Option<ModelInfo>,
    pub usage: Option<Usage>,
    pub stop_reason: Option<String>,
    pub content: Vec<ContentBlock>,
    pub flags: MessageFlags,
    pub provenance: Provenance,
    pub extras: BTreeMap<String, Value>,
}

pub struct Provenance {
    pub source_event_type: String,     // e.g. "codex:response_item.function_call"
    pub raw: Value,                    // 원본 한 줄/한 행 통째 (무손실)
}

#[serde(tag = "kind")]
pub enum ContentBlock {
    Text       { text: String, extras: BTreeMap<String, Value> },
    Thinking   { text: String, encrypted: Option<String>, extras: ... },
    ToolUse    { call_id: String, name: String, input: Value, extras: ... },
    ToolResult { call_id: String, output: Value, is_error: bool, extras: ... },
    Image      { mime: String, source: ImageSource, extras: ... },
    Attachment { name: Option<String>, path: Option<String>, mime: Option<String>, extras: ... },
    Patch      { unified_diff: String, extras: ... },
    Other      { type_tag: String, payload: Value },  // 알 수 없는 콘텐츠 catch-all
}

pub enum ImageSource { LocalPath(String), Base64 { data: String }, Url(String) }
pub struct ModelInfo { pub provider_id: Option<String>, pub model_id: String, pub variant: Option<String> }
pub struct Usage { /* input/output/cached/reasoning/total tokens + cost_usd */ }
pub struct GitInfo { pub branch: Option<String>, pub commit: Option<String>, pub origin_url: Option<String> }
pub struct MessageFlags { pub is_sidechain: bool, pub is_meta: bool, pub is_compaction: bool, pub skipped: bool }
```

### 3.3 Provider별 매핑 규칙 (요약)

핵심 부분만. 풀 매핑 표는 `docs/mapping/<provider>.md`로 구현 단계에서 작성.

#### Claude → Universal
| Claude line.type | Universal |
|---|---|
| `user` (string content) | `UMessage{role:User, content:[Text]}` |
| `user` (tool_result content) | `UMessage{role:Tool, content:[ToolResult{call_id from tool_use_id}]}` |
| `assistant`/`message` (content array) | `UMessage{role:Assistant, content:[Text|ToolUse|Thinking|…]}` |
| `session_meta`/`permission-mode`/`file-history-snapshot`/`ai-title`/`last-prompt`/`skill_listing`/`task_reminder`/`deferred_tools_delta`/`system` | `UMessage{role:System, flags.is_meta=true}` + `provenance.raw` 보존 |
| `attachment` | `UMessage{role:User, content:[Attachment]}` |
| `isSidechain:true` | `flags.is_sidechain=true` (드롭 X) |
| **사이드카 `tool-results/<hash>.txt`** | 해당 `ToolResult.output`을 외부 파일에서 인라인 hydrate (옵션) |

#### Codex → Universal
| Codex outer.type | inner | Universal |
|---|---|---|
| `session_meta` | — | top-level `session_meta`, `git`, `created_at` |
| `turn_context` | — | `UMessage{role:System, flags.is_meta=true}` + `model` 추출 (이후 기본값) |
| `event_msg` | `user_message` | `UMessage{role:User}` (image 첨부 있으면 `Image` block) |
| `event_msg` | `agent_message` | `UMessage{role:Assistant, content:[Text]}` (response_item.message와 dedupe) |
| `event_msg` | `token_count` | `UMessage{role:System, flags.is_meta=true, usage=…}` |
| `event_msg` | `task_started`/`task_complete` | meta system message |
| `response_item` | `message` | role 그대로, content array → `Text` blocks |
| `response_item` | `function_call`/`custom_tool_call` | `ToolUse` (`arguments` JSON 문자열 → 재파싱) |
| `response_item` | `function_call_output`/`custom_tool_call_output` | `ToolResult` |
| `response_item` | `reasoning` | `Thinking` (`encrypted_content`는 `extras.encrypted`) |

#### OpenCode → Universal
| 소스 | Universal |
|---|---|
| `session` 행 | top-level 메타 (id, title, model, cost, tokens_*) |
| `message` 행, `data.role="user"` | `UMessage{role:User}` |
| `message` 행, `data.role="assistant"` | `UMessage{role:Assistant, model, usage}` |
| `part` 행 (메시지 내부 청크) | `data.type`에 따라 `Text`/`ToolUse`/`ToolResult`/`Thinking` ContentBlock 생성 |
| `todo`/`event`/`permission` | top-level `extras.opencode_*` 또는 meta message |

### 3.4 Universal → Provider 합성 규칙

각 `to_X`는 위 매핑의 **역방향**을 수행하되, **provider 외 출처에서 만들어진 메시지**(예: claude → universal → codex 변환에서 claude의 `permission-mode` 메시지를 codex로 보낼 때)는:

1. `provenance.source_event_type`이 동일 provider면 `provenance.raw`를 그대로 재방출.
2. 다른 provider면 → 의미가 가장 가까운 시스템/메타 이벤트로 변환. 손실 필드는 `extras.<provider>_lost: [...]`에 기록.
3. 매핑 불가하면 → 단순 `system` 메시지 + 본문에 "(from <provider> X)" prefix 부착.

---

## 4. 변환기 아키텍처

### 4.1 입출력 단위
- **Input "session reference"**: (a) JSONL 파일 경로, (b) `opencode.db` 경로 + `session_id`, (c) `--from-cwd` + provider (가장 최근 세션 자동 선택)
- **Output (라이브러리 사용자에게는 양쪽 다 제공)**:
  - Phase 1: `UniversalSession` (in-memory) 또는 직렬화된 JSON
  - Phase 2: provider 포맷 in-memory (예: `Vec<JsonlLine>`) 또는 파일
  - Phase 3 (라이브러리 API만, CLI는 별도): provider 실저장소에 install (라이브러리 사용자가 자기 책임으로 호출)

### 4.2 모듈 / 파일 구조

```
cokacmux/
├── Cargo.toml              # [lib] + [[bin]] 둘 다. features로 dep 분리
├── src/
│   ├── lib.rs              # 라이브러리 public API. re-export.
│   ├── universal/
│   │   ├── mod.rs
│   │   ├── schema.rs       # UniversalSession 등 (§3.2)
│   │   ├── validate.rs     # call_id 짝, role 일관성 검증
│   │   └── json.rs         # canonical JSON serialize/deserialize
│   ├── providers/
│   │   ├── mod.rs          # `pub trait ProviderAdapter`
│   │   ├── claude/                    # (always-on)
│   │   │   ├── mod.rs
│   │   │   ├── read.rs                # JSONL → Vec<RawLine>
│   │   │   ├── write.rs               # Vec<RawLine> → JSONL
│   │   │   ├── from_universal.rs
│   │   │   ├── to_universal.rs
│   │   │   ├── path.rs                # cwd-encoding, 사이드카 경로
│   │   │   └── sidecar.rs             # tool-results/ hydrate/dehydrate
│   │   ├── codex/                     # (always-on)
│   │   │   ├── mod.rs
│   │   │   ├── read.rs
│   │   │   ├── write.rs
│   │   │   ├── from_universal.rs
│   │   │   ├── to_universal.rs
│   │   │   └── threads_db.rs          # state_5.sqlite 인덱싱 (feature "codex-index")
│   │   └── opencode/                  # (feature = "opencode")
│   │       ├── mod.rs
│   │       ├── read.rs                # rusqlite SELECT
│   │       ├── write.rs               # rusqlite INSERT
│   │       ├── from_universal.rs
│   │       └── to_universal.rs
│   ├── pivot.rs                       # X → U → Y 합성, 양방향 헬퍼
│   ├── error.rs                       # `ConvertError`, `Result` alias
│   ├── ids.rs                         # UUID v7 발급, 안정 ID 합성
│   ├── time.rs                        # ISO/epoch 변환
│   ├── jsonl.rs                       # 줄 단위 read/write, atomic write
│   └── bin/
│       └── cokacmux.rs          # CLI 진입점 (feature = "cli")
├── tests/
│   ├── data/
│   │   ├── claude/*.jsonl             # fixture 세션
│   │   ├── codex/*.jsonl
│   │   └── opencode/*.sql             # CREATE INSERT 스크립트
│   ├── roundtrip.rs
│   └── pivot.rs
├── docs/
│   ├── PLAN.md  (this file)
│   └── mapping/{claude,codex,opencode}.md
└── (build 인프라: build.py, manage.sh, manage.ps1, builder/, install_*.sh — 기존 그대로 유지)
```

### 4.3 라이브러리 Public API

`src/lib.rs` 가 export하는 최소 표면:

```rust
// 핵심 타입
pub use universal::{UniversalSession, UMessage, ContentBlock, Role,
                    Provider, ProviderOrigin, ModelInfo, Usage, GitInfo,
                    Provenance, MessageFlags, ImageSource};

// 에러
pub use error::{ConvertError, Result};

// 1) 파일 경로 기반 — 가장 흔한 사용
pub fn read_session(provider: Provider, src: &SessionSource) -> Result<UniversalSession>;
pub fn write_session(provider: Provider, session: &UniversalSession, dst: &SessionTarget) -> Result<()>;
pub fn convert_file(from: Provider, to: Provider, src: &SessionSource, dst: &SessionTarget) -> Result<()>;

// 2) In-memory 변환 — 호스트 앱이 직접 IO 관리
pub mod claude {
    pub fn from_jsonl_str(jsonl: &str, ctx: &ClaudeReadCtx) -> Result<UniversalSession>;
    pub fn to_jsonl_string(session: &UniversalSession, opts: &ClaudeWriteOpts) -> Result<String>;
}
pub mod codex {
    pub fn from_jsonl_str(jsonl: &str, ctx: &CodexReadCtx) -> Result<UniversalSession>;
    pub fn to_jsonl_string(session: &UniversalSession, opts: &CodexWriteOpts) -> Result<String>;
}
#[cfg(feature = "opencode")]
pub mod opencode {
    pub fn from_db_session(conn: &rusqlite::Connection, session_id: &str) -> Result<UniversalSession>;
    pub fn to_db_session(conn: &mut rusqlite::Connection, session: &UniversalSession) -> Result<()>;
}

// 3) Discovery (옵션)
pub fn list_sessions(provider: Provider, scope: &ListScope) -> Result<Vec<SessionInfo>>;

pub enum SessionSource {
    Path(PathBuf),                                                   // claude/codex JSONL
    OpenCodeDb { db_path: PathBuf, session_id: String },            // opencode
    LatestInCwd { provider: Provider, cwd: PathBuf },
}

pub enum SessionTarget {
    Path(PathBuf),
    OpenCodeDb { db_path: PathBuf },
    Stdout,
}
```

핵심 설계 의도:
- 호스트 앱(예: cokacmux)이 `rusqlite::Connection`을 이미 들고 있으면 `opencode::from_db_session(&conn, ...)`에 직접 넘김 — DB 핸들 중복 생성 방지.
- 파일 IO를 우리가 하지 않아도 되는 호스트는 `claude::from_jsonl_str(...)`로 메모리 스트링만 주고받음 — 라이브러리에서 파일시스템 의존 제거.
- `Provider` enum과 `SessionSource`/`SessionTarget` enum을 통해 호출 표면을 균일화.

### 4.4 Cargo Features

```toml
[features]
default     = ["claude", "codex", "opencode", "cli"]
claude      = []                          # JSONL only, always lightweight
codex       = []                          # 기본 변환만. state_5.sqlite 인덱싱은 별도
codex-index = ["dep:rusqlite", "codex"]   # codex install 시 threads 테이블 갱신
opencode    = ["dep:rusqlite"]            # opencode.db read/write
cli         = ["dep:clap", "dep:anyhow"]  # bin/cokacmux.rs 빌드
discovery   = ["dep:dirs"]                # ~/.codex, ~/.claude 등 자동 탐색
```

라이브러리 사용자가 claude/codex만 쓰고 싶다면:
```toml
cokacmux = { version = "0.1", default-features = false, features = ["claude", "codex"] }
```
이러면 rusqlite·clap 빌드 비용을 0으로 만든다 — "이식 수월한 모듈화"의 핵심.

### 4.5 CLI 표면 (얇은 wrapper)

```
cokacmux convert
    --from   <codex|claude|opencode|auto>
    --to     <codex|claude|opencode|universal>
    --input  <PATH | OPENCODE_DB#SID>
    --output <PATH>
    [--cwd <DIR>]                       # provider가 cwd를 필요로 할 때
    [--inline-tool-results]             # claude tool-results/ 사이드카 흡수 (기본 ON)
    [--strict]                          # 미지 이벤트 발견 시 에러
    [--dry-run]

cokacmux inspect
    --from auto --input <PATH | OPENCODE_DB#SID>     # UniversalType JSON → stdout

cokacmux list --provider <codex|claude|opencode> [--cwd <DIR>]
```

`--from auto`는 파일 sniff:
- `*.db` → opencode
- JSONL 첫 줄에 `"timestamp"`+`"payload"` → codex
- JSONL 첫 줄에 `"sessionId"`+`"type"` → claude

`install`은 CLI 노출 X (라이브러리 API로만). 사용자가 자기 도구로 명시적으로 사용.

---

## 5. 구현 단계 (Phases)

### Phase 0 — 코드베이스 정리
- **유지 (build 인프라)**: `build.py`, `buildbin.sh`, `buildweb.sh`, `builtclean.sh`, `manage.sh`, `manage.ps1`, `install_windows_build_deps.sh`, `builder/`, `rustfmt.toml`, `.editorconfig`, `.gitignore`
- **삭제 (cokacmux 본체)**: `src/` 전부 (cli, core, dashboard, main.rs, service, tui)
- **삭제 또는 재작성**: `README.md`, `CLAUDE.md`(프로젝트 룰), `dist_beta/`, `docs/cokacmux-guide.md`
- **신규**: `src/lib.rs`(빈 placeholder), `src/bin/cokacmux.rs`(빈 placeholder)
- **`Cargo.toml` 슬림화**:
  - `name = "cokacmux"`, `version = "0.1.0"`, `description`, `repository` 갱신
  - **제거**: `ratatui`, `crossterm`, `reqwest`, `rcgen`, `tokio-rustls`, `rustls`, `rustls-pemfile`, `rustls-pki-types`, `if-addrs`, `libc`(unix), `tokio`(풀 features)
  - **유지**: `serde`, `serde_json`, `clap`, `chrono`, `sha2`, `getrandom`
  - **추가**: `uuid = { version = "1", features = ["v7", "serde"] }`, `thiserror`, optional `rusqlite = { version = "0.32", features = ["bundled"], optional = true }`, optional `anyhow = { version = "1", optional = true }`, optional `dirs = { version = "5", optional = true }`
  - `[lib]` + `[[bin]] path = "src/bin/cokacmux.rs"` 둘 다
  - `[features]` (§4.4)
- 빌드 통과만 확인 (실행/기능 없음)
- build.py가 새 바이너리명을 인식하도록 필요 시 손봄

### Phase 1 — `UniversalType` 정의 + 모든 `from_X` + In-memory API
1. `src/universal/schema.rs` 작성 (§3.2). serde derive 포함.
2. `src/universal/validate.rs`: call_id 짝 검사, 빈 메시지 제거, role 일관성
3. `src/error.rs`: `ConvertError` (thiserror)
4. `src/providers/claude/`: `from_jsonl_str` + 사이드카 hydrate 옵션
5. `src/providers/codex/`: `from_jsonl_str` — 외부/내부 type 양쪽 매핑
6. `src/providers/opencode/`: `from_db_session` (rusqlite read-only)
7. `src/lib.rs`: 모든 public API export
8. CLI: `cokacmux inspect`, `cokacmux convert --to universal`
9. **테스트**:
   - 각 provider 작은 fixture로 단위 테스트
   - 현 시스템 `~/.codex`, `~/.claude`, `~/.local/share/opencode`에서 실제 세션을 골라 `inspect` → 결과 JSON을 사람이 검사
   - `--strict`로 alarmingly 새로운 이벤트 타입이 있는지 확인

### Phase 2 — `to_X` 합성 (파일 출력)
1. `to_claude::to_jsonl_string`: UniversalSession → claude JSONL 라인 시퀀스. cwd 인코딩으로 파일명 결정. 사이드카는 `--with-sidecar` 옵션.
2. `to_codex::to_jsonl_string`: UniversalSession → 롤아웃 JSONL. `state_5.sqlite` 갱신은 `codex-index` feature.
3. `to_opencode::to_db_session`: rusqlite INSERT. `opencode export` 실행 결과의 정확한 row 구조를 캡처해 역엔지니어링 필요 — Phase 2 시작 시 한 번 조사.
4. CLI: `cokacmux convert --to <claude|codex|opencode>`
5. **테스트(왕복)**: 같은 세션을 `from_X → to_X` 해서 원본과 의미적 동일성 검사 (canonical 정규화 후 diff).
6. **테스트(피벗)**: `codex → universal → claude → universal → codex` 4단계 왕복에서 텍스트 본문이 보존되는지.

### Phase 3 (라이브러리 API만, CLI 미노출) — 실저장소 install 헬퍼
1. `claude::install_to_user_dir(session, opts)`: `~/.claude/projects/<encoded-cwd>/<sid>.jsonl`에 atomic 쓰기.
2. `codex::install_to_user_dir(session, opts)`: JSONL을 날짜 디렉터리에 쓰고, `codex-index` feature ON이면 `state_5.sqlite::threads` 행 INSERT.
3. `opencode::install_to_default_db(session, opts)`: 기본 `opencode.db`에 INSERT. opencode가 돌고 있으면 거부 (lock probe).
4. CLI는 노출하지 않음. 호스트 앱이 자기 책임으로 호출.

### Phase 4 (옵션) — 품질/유틸
- `list` 명령: provider별 세션 인덱스 (cokacmux의 resolver 로직 참고)
- `--from auto` sniff 정확도 개선
- 변환 통계 출력(`--stats`)
- streaming 처리 (수십 MB 이상 세션 대응 — 현 단계 불요)

---

## 6. 테스트 전략

### 6.1 단위 테스트 (라이브러리 crate 내부)
- `from_X`: fixture(`tests/data/<provider>/<id>.jsonl`) → 기대 `UniversalSession` JSON과 비교
- `to_X`: `UniversalSession` fixture → 기대 provider artifact 와 비교
- 매핑 규칙별 1개 이상 (turn_context, function_call, sidechain, tool-results 외화 등)

### 6.2 왕복 테스트 (property-style)
- `tests/data/<provider>/*` 의 모든 fixture에 대해:
  - `from_X → to_X` 결과의 canonical JSON이 원본과 같음
  - 텍스트 본문 100% 보존
  - 모든 ToolUse가 짝지어진 ToolResult를 가짐
- 피벗 왕복은 `text-body-equal` 기준만 통과하면 OK (provider 고유 필드는 손실 허용)

### 6.3 라이브러리 통합 테스트
- `tests/lib_smoke.rs`: 외부 crate 흉내 — `default-features = false, features = ["claude"]`로 빌드해서 rusqlite 빌드되지 않는지 검증
- `tests/lib_api.rs`: in-memory API(`from_jsonl_str`, `to_jsonl_string`)로 한 사이클

### 6.4 실데이터 스모크 테스트 (CI에서는 skip, 로컬 only)
- `~/.codex/sessions`, `~/.claude/projects`, `~/.local/share/opencode/opencode.db`에서 무작위 N개 → `inspect` 에러 없이 통과
- 환경변수 `COKACCONVERTER_SMOKE=1` 일 때만 실행

### 6.5 Strict 모드
- `--strict`는 미지의 이벤트 타입이나 짝없는 `tool_use_id`를 발견하면 에러. 새 버전 에이전트가 추가한 필드를 빨리 발견하기 위한 안전망.

---

## 7. 위험 / 트레이드오프

| 위험 | 영향 | 완화 |
|---|---|---|
| OpenCode·Codex 스키마가 향후 변경 | 변환 실패 / 무손실 깨짐 | `provenance.raw` 보존, `--strict` 모드, 통합 테스트 |
| Claude tool-results 사이드카 누락 | ToolResult 본문 truncated | 인라인 hydrate 기본 ON |
| `opencode.db` 동시 접근 | DB lock 충돌 | read는 `OPEN_READ_ONLY`. write는 lock probe + 사용자에게 명시 |
| Codex `reasoning.encrypted_content` 재방출 | 다른 provider에 보낼 때 무의미 | extras 보존하되 to_X에서는 평문 summary만 사용 |
| UUID 충돌 (다른 provider의 다른 세션이 같은 UUID) | install 시 위험 | `--force` 명시 요구. 라이브러리 API는 명시적 `overwrite: bool` 인자 |
| 큰 JSONL (수 MB) | 메모리 사용 | 1차 구현은 메모리에 다 올림. Phase 4에서 streaming |
| **API 표면이 너무 크면 이식 어려움** | "수월한 모듈화" 목표 위반 | public API 최소화. internal은 `pub(crate)`. 신중하게 add |
| **rusqlite C 의존성** | 라이브러리 사용자가 SQLite 빌드 강제 | `opencode` feature gate. 디폴트는 ON이지만 OFF로 끄기 쉽게 |

---

## 8. 산출물 체크리스트

### Phase 0
- [ ] `src/` 전체 삭제
- [ ] build 인프라 파일들 유지 확인 (`build.py`, `manage.{sh,ps1}`, `builder/` 등)
- [ ] `Cargo.toml` 슬림화 + `name = "cokacmux"` + `[lib]` + `[[bin]]` + `[features]`
- [ ] `src/lib.rs` + `src/bin/cokacmux.rs` (빈) — `cargo build` 통과
- [ ] `README.md` 새로 작성 (cokacmux 소개)

### Phase 1
- [ ] `src/universal/schema.rs` — UniversalType 정의
- [ ] `src/universal/validate.rs`
- [ ] `src/error.rs`
- [ ] `src/providers/claude/{read,from_universal,sidecar,path}.rs`
- [ ] `src/providers/codex/{read,from_universal}.rs`
- [ ] `src/providers/opencode/{read,from_universal,db}.rs` (feature gated)
- [ ] `src/lib.rs` — public API export
- [ ] `src/bin/cokacmux.rs` — `convert --to universal`, `inspect`
- [ ] `tests/data/<provider>/*` — fixture 세션 1~3개씩

### Phase 2
- [ ] `src/providers/<X>/{write,to_universal}.rs`
- [ ] `src/pivot.rs`
- [ ] `tests/roundtrip.rs`, `tests/pivot.rs`
- [ ] `src/bin/cokacmux.rs` — `convert --to <provider>`
- [ ] `docs/mapping/{claude,codex,opencode}.md` 완성

### Phase 3 (옵션)
- [ ] `src/providers/<X>/install.rs`
- [ ] `tests/install.rs` (격리된 tempdir 사용)

---

## 9. 결정 사항

### 9.1 확정 (사용자 답변 완료)

| # | 결정 | 답 |
|---|---|---|
| Q1 | 기존 cokacmux `src/` 코드 처리 | **`src/` 전부 폐기. build 인프라(build.py, manage.*, builder/ 등)만 유지** |
| Q2 | MVP 범위 | **다른 앱 이식이 수월한 라이브러리 crate 중심 모듈화. CLI는 얇은 wrapper. Phase 1+2가 핵심 MVP, Phase 3은 라이브러리 API로만 노출** |
| Q3 | OpenCode 입출력 | **rusqlite 직접. `opencode` feature gate로 외부 사용자에게는 옵션화** |

### 9.2 권장값으로 진행 (사용자 미답, 변경 원하면 알려주세요)

| # | 결정 | 권장 |
|---|---|---|
| Q4 | Claude tool-results 사이드카 | 인라인 hydrate **기본 ON** — 무손실 보장. 라이브러리 API에서 `ClaudeReadCtx { inline_tool_results: bool }`로 끌 수 있음 |
| Q5 | 버전 표기 | **`0.1.0`** — 완전히 새 프로젝트라는 신호. 1.0은 Phase 2 완료 + 호스트 앱 1개(예: cokacmux) 통합 검증 후 |
| Q6 | 라이센스/저자 | 기존 `MIT` 유지, 저자 `cokac <monogatree@gmail.com>` 유지 (Cargo.toml의 license/authors 그대로) |

### 9.3 Phase 2 시작 시 추가 조사 필요

- **`opencode export` 출력 포맷**: opencode가 내보내는 JSON의 정확한 스키마. `to_opencode` 합성 시 이 포맷이 row INSERT를 안 해도 되는 대안 경로가 될 가능성. 시작 시점에 `opencode export <sid> > /tmp/X.json` 으로 한 번 캡처.
- **`state_5.sqlite::threads` 행 형태**: `codex-index` feature 구현 시 어느 컬럼이 NOT NULL이라 반드시 채워야 하는지 확인 (이미 §1.2.2에서 일부 파악됨, 실제 INSERT 시점에 마저).

---

이상의 결정이 모두 반영된 상태에서 Phase 0부터 진행한다.
