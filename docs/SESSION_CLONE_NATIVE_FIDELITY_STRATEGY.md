# Session Clone Native Fidelity Mission

> 작성일: 2026-05-22  
> 대상: Claude Code, Codex, OpenCode 세션 clone/변환 기능  
> 핵심 키워드: 실측 우선, 네이티브 포맷 충실도, 안전한 clone, 검증 가능한 변환

---

## 1. 가장 중요한 미션

우리의 가장 중요한 미션은 단순히 세션 데이터를 “대충 읽고 다른 포맷으로 저장하는 것”이 아니다.

가장 중요한 미션은 다음과 같다.

> Claude Code, Codex, OpenCode 중 어느 에이전트에서 만들어진 세션이든, clone 또는 cross-provider 변환 후 대상 에이전트가 직접 생성한 네이티브 세션 데이터와 최대한 동일한 형태로 저장되게 만들고, 그 결과를 대상 에이전트가 자연스럽게 목록화·표시·resume·계층 관리할 수 있게 한다.

이 미션에서 “성공”은 파일이나 DB row가 하나 만들어졌다는 뜻이 아니다.

성공은 다음 조건을 모두 만족해야 한다.

- 대상 에이전트의 실제 저장소에 들어갔을 때 깨지지 않아야 한다.
- 대상 에이전트의 session list, resume, UI, tree view, clone hierarchy에서 자연스럽게 다뤄져야 한다.
- 원본 대화의 의미 정보가 보존되어야 한다.
- 대상 포맷의 필수 필드, ID 형태, row 관계, JSON shape, timestamp, index row가 네이티브와 맞아야 한다.
- 변환기가 임의로 거짓 runtime context를 만들어 넣어서는 안 된다.
- 같은 provider 안에서 clone할 때 원본 세션을 손상시키면 안 된다.
- 변환 결과가 “우리 validator에서만 통과”하는 것이 아니라, 실제 에이전트가 생성한 네이티브 데이터와 비교했을 때 구조적으로 설명 가능해야 한다.

즉, 이 프로젝트의 핵심 목표는 “포맷 변환기”보다 더 엄격하다.

우리는 사실상 세 에이전트의 세션 저장 레이어를 관측하고, 그 저장 관습을 재현하며, 그 위에 안전한 clone 계층 기능을 구축하고 있다.

---

## 2. 왜 이 미션이 중요한가

### 2.1 clone은 단순 복사가 아니다

clone 기능은 원본 세션을 복제해서 새 세션을 만드는 기능이다.

하지만 이 앱에서 clone은 단순한 파일 복사가 아니다.

clone 후에는 다음 개념이 생긴다.

- 원본 세션은 parent가 된다.
- 새 세션은 child가 된다.
- 같은 원본에서 여러 child가 파생될 수 있다.
- child가 다시 clone되면 더 깊은 계층 구조가 생긴다.
- 사용자는 이 계층을 tree view로 보고, parent/child 관계를 따라 세션을 탐색해야 한다.

따라서 clone된 세션이 대상 에이전트에서 제대로 보이지 않거나 resume되지 않으면, tree 구조 자체가 사용자에게 신뢰를 잃는다.

### 2.2 cross-provider clone은 더 어렵다

Claude → Claude clone은 원본 포맷을 상당 부분 그대로 재사용할 수 있다.

하지만 Claude → Codex, Codex → OpenCode, OpenCode → Claude 같은 변환은 완전히 다르다.

각 provider는 다음이 다르다.

- 저장 위치
- 세션 ID 형태
- 메시지 ID 형태
- parent 관계 표현 방식
- timestamp 단위
- JSONL line type
- SQLite table 관계
- assistant reasoning 표현
- tool call / tool result 표현
- runtime context 표현
- session list용 index 저장 방식
- title, preview, first user message 계산 방식
- resume 시 허용되는 content block type

따라서 cross-provider clone은 “텍스트만 복사”해서는 안 된다.

대상 에이전트가 직접 만든 것과 같은 native artifact를 만들어야 한다.

### 2.3 잘못된 변환은 조용히 위험하다

세션 변환 오류는 항상 즉시 터지지 않는다.

특히 위험한 오류는 다음과 같다.

- 목록에는 보이지만 resume하면 깨진다.
- resume은 되지만 conversation order가 어긋난다.
- DB primary key가 충돌해서 원본 row를 덮어쓴다.
- assistant message가 두 번 보인다.
- tool result가 중복된다.
- default field 하나 때문에 대상 CLI가 row를 조용히 무시한다.
- runtime context를 잘못 주입해서 다음 turn의 모델 행동이 바뀐다.

이런 문제는 단순 unit test로 잡기 어렵다.

그래서 실측과 native comparison이 필수다.

---

## 3. 미션 수행 원칙

## 3.1 실측 우선

추측으로 포맷을 만들지 않는다.

각 provider에 대해 실제 에이전트를 실행하고, 그 결과 생긴 세션 파일 또는 DB row를 직접 분석한다.

필수 관측 대상은 다음이다.

- 새 세션 생성 직후의 artifact
- 최소 대화 1턴의 artifact
- user message shape
- assistant message shape
- reasoning shape
- tool call / tool result shape
- session metadata shape
- session list/index shape
- clone 또는 fork 관련 shape
- timestamp와 ID ordering
- nullable field와 optional field의 실제 사용 방식

## 3.2 소스가 있으면 소스를 본다

OpenCode와 Codex는 소스가 있다.

소스가 있는 provider는 실측만 믿지 않고, 다음을 함께 확인한다.

- schema 정의
- ID 생성 함수
- projector 또는 recorder 코드
- session list 코드
- resume 후보 선택 코드
- SQLite insert/upsert 코드
- JSON serialization 코드
- test fixture

실측은 “실제로 현재 버전이 만든 결과”를 알려준다.

소스는 “왜 그렇게 만들어지는지”와 “어떤 필드가 불변식인지”를 알려준다.

둘을 함께 봐야 한다.

## 3.3 소스가 없으면 더 많이 측정한다

Claude Code는 소스가 없다.

따라서 Claude는 다음 방식으로 접근한다.

- 여러 prompt로 native JSONL을 생성한다.
- `--session-id`, `--name`, `--output-format`, budget 제한 등 옵션별 차이를 본다.
- 생성된 line type의 top-level key를 fingerprint한다.
- `user`, `assistant`, `attachment`, `custom-title`, `agent-name`, `queue-operation`, `last-prompt` 같은 line을 분류한다.
- Claude resume에서 허용되는 content block type과 거부되는 content block type을 구분한다.
- 알 수 없는 runtime attachment는 임의 생성하지 않는다.

Claude는 특히 “관측 기반 보수적 합성”이 중요하다.

## 3.4 raw 보존과 synthetic 합성을 분리한다

같은 provider 왕복에서는 raw를 최대한 보존한다.

예:

- Claude → Universal → Claude
- Codex → Universal → Codex
- OpenCode → Universal → OpenCode

이때 가능한 한 원본 raw를 그대로 replay한다.

하지만 cross-provider 변환에서는 raw를 그대로 넣을 수 없다.

예:

- Codex의 `response_item.message`를 Claude JSONL에 그대로 넣으면 Claude native format이 아니다.
- OpenCode의 `part.data`를 Codex rollout에 그대로 넣으면 Codex가 모르는 payload가 된다.

따라서 cross-provider에서는 UniversalSession의 의미 정보를 바탕으로 대상 provider native shape를 synthetic으로 만든다.

중요한 점은 synthetic이라고 해서 임의로 만들어도 된다는 뜻이 아니다.

synthetic 결과도 대상 provider의 native artifact 실측과 맞아야 한다.

## 3.5 거짓 runtime context는 만들지 않는다

Codex native session에는 developer/runtime prelude가 들어갈 수 있다.

Claude native session에는 attachment 형태의 runtime context가 들어갈 수 있다.

하지만 이런 정보는 “그 에이전트가 그 순간 실행되며 실제로 주입한 context”다.

cross-provider 변환기가 이를 임의로 만들어 넣으면 다음 문제가 생긴다.

- 실제 권한과 다른 permissions instruction이 기록될 수 있다.
- 실제 skill 목록과 다른 skill listing이 들어갈 수 있다.
- 실제 budget 또는 tool state와 다른 attachment가 생길 수 있다.
- 다음 resume turn에서 모델에게 잘못된 맥락을 줄 수 있다.

따라서 원본 provider의 runtime context는 같은 provider raw replay에서는 보존하되, cross-provider synthetic output에서는 보수적으로 생략한다.

이 원칙은 “네이티브와 100% 같은 line count”보다 더 중요하다.

우리의 목표는 “겉보기 가짜 네이티브”가 아니라 “대상 에이전트가 안전하게 받아들이는 진짜 의미의 네이티브 호환 데이터”다.

## 3.6 변환은 반드시 검증 gate를 통과해야 한다

clone 후에는 즉시 native validation을 수행한다.

검증 실패 artifact는 성공으로 보고하면 안 된다.

검증 항목은 provider별로 다르다.

Claude:

- JSONL 파일 존재
- 파일명 stem과 session id 일치
- JSON parse 가능
- sessionId 일관성
- conversation row 존재
- conversation uuid 존재
- uuid가 실제 UUID 형태
- Claude API content block으로 안전한 type만 포함

Codex:

- JSONL 파일 존재
- `session_meta` 존재
- `session_meta.payload.id`가 session id와 일치
- cwd 존재
- event row 존재
- `state_5.sqlite::threads` row 존재
- threads.rollout_path가 실제 파일과 일치

OpenCode:

- DB 존재
- `session` row 존재
- directory, slug, version, path non-empty
- `message` row 존재
- `part` row가 message row를 참조
- session id가 `ses_` native shape
- message id가 `msg_` native shape
- part id가 `prt_` native shape
- session_message id가 `evt_` native shape

---

## 4. 전체 전략

## 4.1 큰 흐름

이 미션은 다음 루프를 반복해서 수행한다.

1. native 실측
2. source code 분석
3. 현재 변환 결과 생성
4. native artifact와 변환 artifact 비교
5. 차이 분류
6. 코드 수정
7. validation 강화
8. regression test 추가
9. 실제 clone 재실측
10. 결과 문서화

이 루프가 핵심이다.

한 번 구현하고 끝나는 방식이 아니다.

provider 버전이 바뀔 때마다 native format이 바뀔 수 있으므로, measurement tool과 validation이 계속 필요하다.

## 4.2 native 실측 단계

각 provider에 대해 실제 세션을 생성한다.

Codex:

- `codex exec`로 최소 prompt 실행
- `~/.codex/sessions/YYYY/MM/DD/rollout-...jsonl` 확인
- `~/.codex/state_5.sqlite::threads` row 확인
- `session_meta`, `turn_context`, `event_msg`, `response_item` shape fingerprint

Claude:

- `claude -p --session-id ...`로 최소 prompt 실행
- `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl` 확인
- line type별 top-level key 확인
- `custom-title`, `agent-name`, `queue-operation`, `user`, `attachment`, `assistant`, `last-prompt` 구조 확인

OpenCode:

- `opencode run --dir ... --format json` 실행
- `~/.local/share/opencode/opencode.db` 확인
- `session`, `message`, `part`, `session_message` row 확인
- ID prefix와 ordering 확인
- `part.data`의 `step-start`, `reasoning`, `text`, `step-finish` 구조 확인

## 4.3 source code 분석 단계

Codex:

- rollout recorder
- rollout policy
- protocol type
- state extraction
- threads index insert/upsert
- resume candidate selection

OpenCode:

- `Identifier.create`
- `SessionID`, `MessageID`, `PartID`, `EventID`
- SQLite schema
- projectors
- message-v2 schema
- session-message schema
- session list/render code

Claude:

- 소스가 없으므로 실측 기반
- CLI option별 생성물 비교
- resume 실패 케이스 관측

## 4.4 UniversalSession 피벗 전략

provider 간 직접 변환을 6개씩 따로 만들지 않는다.

항상 다음 형태를 따른다.

```text
source provider artifact
  → from_source()
  → UniversalSession
  → to_target()
  → target provider artifact
```

이렇게 하는 이유:

- 변환 조합이 늘어나도 복잡도가 폭증하지 않는다.
- semantic preservation test를 한 곳에서 만들 수 있다.
- provider별 read/write 책임이 분리된다.
- unknown provider-specific raw를 UniversalSession 안에 보존할 수 있다.
- target writer는 target native shape만 고민하면 된다.

## 4.5 clone 적용 전략

clone은 단순 convert보다 더 엄격하다.

clone 시 수행할 일:

- 새 session id 발급
- 대상 provider에 맞는 session id shape 사용
- message id 재발급
- parent id 재매핑
- provenance raw 내부 id 재작성
- cwd override 반영
- origin에 clone-of 표시
- target provider live storage에 install
- native validation 수행
- clone tree store에 parent-child edge 기록

provider별 ID 원칙:

- Claude: UUID 형태 line uuid
- Codex: UUID v7 session id, rollout filename에 포함
- OpenCode: native Identifier 기반
  - session: `ses_` + descending identifier
  - message: `msg_` + ascending identifier
  - part: `prt_` + ascending identifier
  - session_message event: `evt_` + ascending identifier

특히 OpenCode에서는 `session_message.id`가 globally primary-keyed이므로, 원본 `evt_...`를 재사용하면 원본 row를 `INSERT OR REPLACE`로 빼앗을 수 있다.

따라서 clone 시 반드시 새 `evt_...`를 발급한다.

---

## 5. Provider별 전략

## 5.1 Claude 전략

Claude는 JSONL 기반이다.

기본 저장 위치:

```text
~/.claude/projects/<encoded-cwd>/<session-id>.jsonl
```

핵심 native line:

- `custom-title`
- `agent-name`
- `queue-operation`
- `user`
- `assistant`
- `attachment`
- `last-prompt`

write 전략:

- 같은 provider replay 가능 시 raw를 최대한 보존한다.
- cross-provider synthetic 시 native에서 관측한 기본 line sequence를 만든다.
- `custom-title`, `agent-name`, `queue-operation`, `last-prompt`를 생성한다.
- conversation line에는 `uuid`, `parentUuid`, `sessionId`, `cwd`, `timestamp`, `version`, `entrypoint`, `userType`, `gitBranch`를 넣는다.
- assistant line에는 `requestId`를 넣는다.
- user line에는 `permissionMode`, `promptId`를 넣는다.
- Claude가 resume에서 받아들이기 어려운 OpenCode control part 등은 visible content로 내보내지 않는다.

주의:

- Claude attachment는 runtime-generated 성격이 강하다.
- cross-provider synthetic에서 fake attachment를 만들지 않는다.
- 같은 provider raw replay에서는 원본 attachment를 보존한다.

## 5.2 Codex 전략

Codex는 JSONL rollout과 SQLite index를 함께 쓴다.

본문:

```text
~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<session-id>.jsonl
```

index:

```text
~/.codex/state_5.sqlite
threads(...)
```

핵심 rollout line:

- `session_meta`
- `event_msg.task_started`
- `turn_context`
- `response_item.message`
- `response_item.reasoning`
- `event_msg.user_message`
- `event_msg.agent_message`
- `event_msg.token_count`
- `event_msg.task_complete`

write 전략:

- `session_meta.payload.id`는 session id와 일치해야 한다.
- `source`는 `exec` 기준으로 맞춘다.
- `thread_source`는 native `codex exec`에서 없는 경우 `NULL`로 둔다.
- `turn_context`에 model, effort, cwd, approval policy, sandbox policy를 넣는다.
- visible user/assistant text는 `response_item.message`로 표현한다.
- user display event는 `event_msg.user_message`로 표현한다.
- assistant display event는 `event_msg.agent_message`로 표현한다.
- reasoning이 없더라도 native shape에 맞춰 빈 reasoning item을 넣을 수 있다.
- token count와 task complete event를 넣는다.
- install 시 `state_5.sqlite::threads` row를 함께 갱신한다.

주의:

- Codex native에는 runtime developer/env prelude가 있을 수 있다.
- 이 prelude는 실제 실행 환경에서 Codex가 주입한 것이다.
- cross-provider synthetic에서 임의로 만들면 거짓 runtime context가 된다.
- 따라서 resume/list에 필요한 핵심 line은 만들되, fake runtime prelude는 만들지 않는다.

## 5.3 OpenCode 전략

OpenCode는 SQLite 기반이다.

본문:

```text
~/.local/share/opencode/opencode.db
```

핵심 table:

- `session`
- `message`
- `part`
- `session_message`
- `project`

핵심 native shape:

- `session.id`: `ses_...`
- `message.id`: `msg_...`
- `part.id`: `prt_...`
- `session_message.id`: `evt_...`
- user message row + text part
- assistant message row + step parts
  - `step-start`
  - `reasoning`
  - `text`
  - `step-finish`
- 기본 session_message
  - `agent-switched`
  - `model-switched`

write 전략:

- `project_id='global'` row를 보장한다.
- `session` row에는 directory, title, agent, model, token totals, slug, version, path를 채운다.
- `session.model`은 JSON object string 형태로 저장한다.
- `message.data` user에는 role, time, agent, model, summary를 넣는다.
- `message.data` assistant에는 parentID, role, time, mode, agent, path, cost, tokens, modelID, providerID, finish를 넣는다.
- default variant는 `message.data`에서 생략한다.
- non-default variant는 보존한다.
- assistant part에는 native wrapper를 만든다.
- 원본에 이미 `step-start` 또는 `step-finish`가 있으면 중복 생성하지 않는다.
- text/reasoning assistant part에는 `time`과 `metadata`를 붙인다.
- tool use/result는 가능한 한 하나의 replayable tool part로 fuse한다.

주의:

- OpenCode same-provider clone에서 원본 `evt_...`를 재사용하면 원본 `session_message` row를 손상시킬 수 있다.
- 반드시 새 event id를 만든다.

---

## 6. 비교와 검증 방법

## 6.1 fingerprint 비교

native artifact와 converted artifact를 그대로 diff하면 timestamp, id, token 수 때문에 항상 차이가 난다.

따라서 먼저 shape fingerprint를 비교한다.

Codex/Claude JSONL fingerprint:

- line count
- outer type count
- payload type count
- top-level key set
- content block type count
- first session id
- first cwd
- parse error count

OpenCode DB fingerprint:

- table schema
- row count
- message count
- part count
- session_message count
- `data` JSON key count
- ID prefix/shape
- orphan part 여부

## 6.2 semantic comparison

shape가 맞아도 의미가 깨질 수 있다.

따라서 UniversalSession으로 다시 읽어서 semantic profile을 비교한다.

비교 대상:

- cwd
- user text
- assistant text
- thinking 존재 여부와 텍스트
- tool use name/call_id/input
- tool result call_id/output/is_error
- image block
- attachment block
- message order
- parent relation

## 6.3 live validation

실제 저장소에 install한 뒤 검증한다.

검증은 다음 순서다.

1. clone 생성
2. 대상 저장소에 artifact 작성
3. provider-specific native_validate 실행
4. 가능하면 대상 CLI로 list 확인
5. fingerprint 생성
6. native fingerprint와 비교
7. 문제가 있으면 코드 수정
8. regression test 추가

## 6.4 regression test 원칙

실측으로 발견한 모든 버그는 test로 고정한다.

중요 test 유형:

- same-provider raw roundtrip
- cross-provider semantic preservation
- OpenCode ID shape
- OpenCode session_message event rekey
- OpenCode step part duplication 방지
- OpenCode default variant omission
- Codex state_5 index row compatibility
- Claude resume-safe content block filtering
- clone parent chain repair
- clone tree rendering

---

## 7. Debug logging 전략

런타임 문제는 사용자가 본 화면만으로는 파악하기 어렵다.

따라서 다음 지점에 debug log를 남긴다.

- provider read start/end/error
- provider write start/end/error
- clone start
- load success/failure
- mutation start/end
- install start/end/failure
- native validation report
- search/list/remove/title 등 session manager operation
- OpenCode tool fusion count
- row count / message count / part count
- validation failure summary

환경 변수:

```text
COKACMUX_DEBUG=1
COKACCONVERT_DEBUG=1
```

로그 위치:

```text
~/.cokacmux/debug/cokacmux.log
```

로그 원칙:

- 사용자 prompt 원문은 필요 이상으로 길게 남기지 않는다.
- path, id, count, provider, stage, error는 남긴다.
- 변환 실패 시 어느 provider 어느 단계에서 실패했는지 바로 알 수 있어야 한다.

---

## 8. 안전 원칙

## 8.1 원본 손상 금지

clone은 원본을 읽기만 해야 한다.

원본에 write하면 안 된다.

특히 OpenCode SQLite에서 primary key 충돌로 원본 row가 `REPLACE`되지 않게 해야 한다.

## 8.2 live storage write는 검증과 함께

대상 에이전트의 실제 저장소에 쓰는 작업은 항상 validation과 붙어야 한다.

검증 실패 시 성공 메시지를 내지 않는다.

## 8.3 알 수 없는 데이터는 삭제하지 말고 격리

모르는 line type 또는 content block은 버리지 않는다.

UniversalSession의 `Other` 또는 `provenance.raw`에 보존한다.

다만 대상 provider가 받아들이지 못하는 raw를 visible conversation으로 내보내지는 않는다.

## 8.4 fake native 금지

네이티브처럼 보이기 위해 실제로 존재하지 않은 runtime context를 만들어 넣지 않는다.

특히 다음은 조심한다.

- permissions instruction
- skill listing
- tool budget
- environment context
- generated attachment
- agent-specific hidden control event

## 8.5 실패는 조용히 숨기지 않는다

변환 중 일부를 drop해야 한다면 debug log에 남긴다.

validation failure는 사용자에게 요약해서 알린다.

---

## 9. 현재 달성 상태

현재까지 확인된 상태:

- Claude native artifact 실측 완료
- Codex native artifact 실측 완료
- OpenCode native artifact 실측 완료
- Codex source 분석 완료
- OpenCode source 분석 완료
- Claude는 source 없음, 실측 기반으로 분석
- live clone 6방향 이상 실측 완료
- OpenCode same-provider clone 원본 row 보존 확인
- native validation 도입
- OpenCode native ID shape validation 강화
- Codex `state_5.sqlite` index write 확인
- OpenCode target fingerprint가 native row/part count와 일치
- 전체 test suite 통과

중요하게 수정된 실제 버그:

- OpenCode session_message 원본 event id 재사용 위험 수정
- OpenCode same-provider clone의 `step-start`/`step-finish` 중복 수정
- OpenCode default variant message row 차이 수정
- Codex `thread_source` 관련 예전 가정 수정
- Claude resume-safe content block filtering 강화

---

## 10. 앞으로의 작업 전략

## 10.1 더 많은 native scenario 확보

최소 prompt뿐 아니라 다음 scenario를 provider별로 실측해야 한다.

- tool call 성공
- tool call 실패
- 여러 turn 대화
- image input
- file attachment
- long output sidecar
- interrupted/cancelled turn
- compaction
- fork/clone/native child session
- permission prompt
- model switch
- agent switch

## 10.2 native fixture archive 구축

실측 artifact를 그대로 보존할 fixture archive가 필요하다.

권장 구조:

```text
fixtures/native/
  codex/
    simple_text/
    tool_success/
    tool_error/
  claude/
    simple_text/
    tool_success/
    attachment/
  opencode/
    simple_text/
    tool_success/
    session_message/
```

각 fixture에는 다음을 함께 저장한다.

- raw artifact
- fingerprint
- provider version
- command used
- cwd
- expected semantic profile

## 10.3 acceptance test 강화

단순 validation을 넘어서 실제 CLI acceptance를 늘린다.

Codex:

- `codex resume <id>` dry-run 또는 최소 prompt로 resume 가능성 확인
- list에서 title/preview가 정상인지 확인

Claude:

- `claude --resume <id>` 또는 session file discovery 확인
- invalid content block이 resume을 깨지 않는지 확인

OpenCode:

- `opencode session list`에서 row가 보이는지 확인
- session detail/export에서 message/part가 정상인지 확인

## 10.4 clone tree UX와 결합

포맷 변환은 backend mission이고, 사용자 경험은 tree view와 연결된다.

clone tree UX 원칙:

- 기본 session view는 tree mode
- parent/child 관계가 한눈에 보여야 한다
- child prefix는 `└ ` 형태
- depth indentation은 2 spaces
- filter/search 시 matching child의 ancestor context를 유지
- clone된 세션이 어느 provider에 있는지 명확히 표시
- 원본이 삭제되어도 orphan child를 안전하게 표시

## 10.5 provider version drift 감지

에이전트는 계속 업데이트된다.

따라서 버전 drift 감지가 필요하다.

- native_probe 결과에 provider version 저장
- validation 실패 시 version 출력
- source-inspection 기반 invariant와 실측 invariant를 분리 기록
- 새 버전에서 fingerprint가 달라지면 문서와 writer를 업데이트

---

## 11. Definition of Done

이 미션의 완료 기준은 다음이다.

1. 세 provider 모두 native simple text session을 생성하고 fingerprint를 문서화했다.
2. 세 provider 모두 source 또는 실측 기반 저장 불변식을 정리했다.
3. 모든 provider pair에 대해 cross-provider clone이 가능하다.
4. clone 후 native validation이 자동으로 수행된다.
5. OpenCode, Codex처럼 index DB가 있는 provider는 index row까지 맞게 쓴다.
6. 같은 provider clone은 원본 artifact를 손상하지 않는다.
7. OpenCode ID shape와 row 관계가 native와 맞다.
8. Claude resume-unsafe content block은 걸러진다.
9. Codex state row가 native `exec` 기준과 맞다.
10. semantic profile이 cross-provider pivot 후에도 보존된다.
11. 실제 live storage에 넣은 결과가 대상 CLI에서 인식된다.
12. 실측으로 발견한 모든 bug가 regression test로 고정된다.
13. 남은 차이는 “왜 의도적으로 남겼는지” 문서화되어 있다.

---

## 12. 한 줄 전략 요약

우리는 provider 포맷을 추측하지 않는다.

실제 에이전트가 만든 세션을 측정하고, 소스가 있으면 소스를 읽고, 변환 결과를 실제 저장소에 넣고, native artifact와 비교하고, 차이를 코드와 validation과 regression test에 반영한다.

이 루프를 반복해서 clone된 세션이 대상 에이전트 안에서 “외부에서 변환된 가짜”가 아니라 “그 에이전트가 자연스럽게 다룰 수 있는 native-compatible child session”이 되게 만든다.
