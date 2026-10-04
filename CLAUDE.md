<!-- BEGIN USER POLICY: NO EXTERNAL MATERIALS 2026-10-04 -->
# 필수 지침: 외부 제작 자료 사용 절대 금지

2026-10-04 사용자 지시: **“앱을 만들때 절대로 외부자료를 사용해서는 안된다.”**
이 지침은 신규 개발, 기존 앱 수정, 샘플 제작, 테스트 배포, 스토어 제출 준비에 모두 적용한다. 이전 프로젝트 문서에서 외부 자료 사용을 허용했더라도 이 지침을 따른다.

## 금지 범위

- 외부에서 가져온 사진, 그림, 아이콘, 로고, 영상, 애니메이션, 3D 모델, 텍스처, 폰트 파일, 음악, 효과음, 녹음, 문구, 기사, 카드, 템플릿, 예제 콘텐츠, 데이터셋을 제품 제작 자료로 사용하지 않는다.
- 웹 검색, 이미지 검색, 스톡 사이트, 공개 저장소, 다른 앱, 디자인 시안, 패키지 예제에서 자료를 다운로드·복사·캡처·추출해 사용하지 않는다. 원격 URL로 불러오는 것도 금지한다.
- **무료, 유료 구매, 상업적 이용 가능, CC0, 공개 도메인, 오픈 라이선스, 출처 표기, 사용 허락은 예외 사유가 아니다.** 사용권이 있어도 외부 제작 자료는 금지한다.
- 샘플·데모·온보딩·빈 화면·테스트 픽스처·임시 자리표시자에도 같은 규칙을 적용한다. 앱 번들뿐 아니라 스크린샷, 홍보 이미지, 스토어 콘텐츠, 지원 웹사이트에도 적용한다.
- 외부 자료를 자르거나 색을 바꾸고, 필터·트레이싱·AI 변환·일부 재작성으로 가공해 자체 제작이라고 부르지 않는다. 외부 자료를 생성 도구의 입력이나 참고 이미지로 넣어 우회하지 않는다.

## 제작 방법

- 필요한 콘텐츠는 직접 촬영·작성·녹음·디자인하거나 코드로 직접 그린다. 외부 원본을 입력하지 않고 새로 생성한 독자적인 이미지·음원·문구는 자체 제작 자료로 관리한다. 특정 외부 작품의 복제를 지시하지 않는다.
- 자체 제작 자료의 제작 방식과 원본을 기록한다. 생성 도구를 썼다면 프롬프트와 생성 결과를 보관한다. 생성했다는 사실만으로 권리 문제가 없다고 단정하지 않는다.
- 폰트 파일을 외부에서 가져오지 않는다. 플랫폼이 제공하는 시스템 글꼴 API 또는 직접 제작한 서체를 사용한다. 아이콘도 직접 제작하거나 코드로 그린다.
- 자체 제작 자료가 없으면 직접 만들거나 해당 샘플을 생략한다. 일정이나 시각적 완성도를 이유로 외부 자료를 임시로 넣지 않는다.

## 개발 도구와 사용자 자료의 구분

- SDK, 컴파일러, 프레임워크, 기능 구현 라이브러리와 공식 기술 문서는 개발 도구다. 이를 이용한다는 이유로 그 안의 예제 사진·폰트·아이콘·문구 등 제작 자료를 가져올 수는 없다. 의존성이 함께 묶는 제작 자료도 확인한다.
- 사용자가 앱 기능을 통해 선택하거나 작성한 개인 자료는 사용자 입력으로 처리한다. 이를 개발자가 제공하는 샘플, 홍보물 또는 다른 사용자의 콘텐츠로 재사용하지 않는다.

## 기존 자료와 작업 완료 기준

- 기존 외부 자료를 발견하면 파일 경로·출처·사용 위치를 기록하고 교체 대상으로 표시한다. 기존 라이선스·출처 기록을 지워 자체 제작으로 위장하지 않는다.
- 외부 제작 자료를 제거하거나 자체 제작 자료로 교체하고, 앱 번들·테스트 자료·스토어 자료에 남지 않았는지 확인한 뒤 새 테스트 배포 또는 출시를 진행한다.
- 지침을 작성한 것과 실제 자료를 교체한 것은 별개다. 교체·검증하지 않은 항목을 완료라고 보고하지 않는다.
- 새 프로젝트를 만들거나 다른 위치로 복사할 때 이 지침을 해당 프로젝트의 `AGENTS.md`에도 포함한다. 하위 작업 지침이나 자동화 도구가 이 규칙을 완화해서는 안 된다.
<!-- END USER POLICY: NO EXTERNAL MATERIALS 2026-10-04 -->

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

## Distribution and Repository Automation Policy

- `docs/PROJECT_POLICY.md` is authoritative for distribution and automation.
- `cokacmux`/`cokacdir` distribution does not require checksums, signatures,
  signed manifests, SBOMs, or attestations. Their absence is intentional and
  is not a defect or release blocker.
- GitHub Actions is not used. Keep `.github/workflows` absent and do not make
  any validation, release, or website task depend on hosted workflows.
- Existing third-party tool archive hashes are local builder implementation
  details, not a release-artifact authenticity requirement.

## Test Storage Safety

- Rust tests require the same explicit user approval as build commands.
- Never run Rust tests against the real home or app storage. Set an isolated
  `COKACMUX_TEST_ROOT`, `COKACMUX_HOME`, `COKACMUX_CONFIG_DIR`, HOME,
  USERPROFILE, temp, XDG, LOCALAPPDATA, and APPDATA tree first.
- Preserve the original `CARGO_HOME` and `RUSTUP_HOME` before replacing HOME,
  or a rustup proxy can silently resolve a different toolchain.
- Set `RUSTUP_AUTO_INSTALL=0` for read-only verification after the required
  toolchain is prepared. Even `rustup --version` can provision a repository
  override when auto-install remains enabled.
- Keep `COKACMUX_DEBUG=0`, `COKACMUX_TRACE=0`, and provider directory
  overrides empty in the normal test gate.
- Never add `--ignored` to the normal test command. Tests marked as live-read
  or live-acceptance require a separate, explicit user-approved gate.

## Version Management

- Version is defined in `Cargo.toml` (line 3: `version = "x.x.x"`)
- All version displays use `env!("CARGO_PKG_VERSION")` macro to read from Cargo.toml
- To update version: modify `Cargo.toml` and the matching `cokacmux` package version in `Cargo.lock` together; builds use `--locked`. Version displays reflect `Cargo.toml` automatically.
- Never hardcode version strings in source code
