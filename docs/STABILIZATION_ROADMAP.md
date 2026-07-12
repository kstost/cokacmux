# cokacmux 안정화 로드맵

## 1. 목표

이번 안정화 사이클의 목표는 현재 사용자 경험과 런타임 계약을 유지하면서 다음 질문에
근거로 답할 수 있게 만드는 것이다.

> 어떤 Git 커밋을 어떤 고정된 입력으로 빌드했고, 어떤 검증을 거쳐 어떤 바이너리를
> 배포했는가?

안정화가 끝날 때까지 신규 provider, 신규 사용자 기능, UI/키바인딩 변경, 변환 계약 변경,
대규모 구조 개편은 기본 범위에 포함하지 않는다.

## 2. 변경할 수 없는 불변식

모든 작업은 `CLAUDE.md`의 우선순위를 따른다.

1. 명시적 사용자 명령이 없으면 실행 중인 agent와 그 데이터를 해치지 않는다.
2. 입력은 사용자가 의도한 agent에 순서대로 정확히 한 번 전달한다.
3. 화면과 상태는 실제 런타임 상태를 정직하게 표시한다.
4. 확인되지 않은 상태는 사망이나 삭제 허가가 아니라 `preserve`로 처리한다.
5. UI thread에 새 blocking I/O를 추가하지 않는다.
6. same-provider 복제는 native 보존 계약을, cross-provider 복제는 context handoff 계약을
   유지한다.

## 3. 단계와 선행 관계

```text
M0 provenance 기준점
  -> M1 고정된 입력과 자동 검증
       +-> M2 private filesystem / 개인정보
       +-> M3 세션 데이터 무결성
       +-> M4 프로세스 수명주기 / 파괴 경로
              -> M5 검증 가능한 릴리스
                    -> M6 점진적 모듈화
```

M2, M3, M4는 M1이 녹색이 된 뒤 병렬 진행할 수 있다. M5는 세 작업이 모두 완료되어야
승인한다. M6는 첫 검증 릴리스 이후에만 시작한다.

## 4. M0 - provenance 기준점

### 작업

- canonical 저장소와 현재 snapshot의 차이를 파일 단위로 확인한다.
- 현재 snapshot을 재구성할 수 있는 patch, source manifest, baseline commit을 보존한다.
- 기존 0.2.39 배포 파일은 `legacy/unverified`로 동결한다.
- 다음 릴리스는 기존 버전을 덮어쓰지 않고 새 patch 버전을 사용한다.
- `cokacdir`의 호환 버전과 digest도 릴리스 입력으로 취급한다.

### 완료 기준

- 릴리스 후보를 가리키는 단일 Git SHA가 있다.
- snapshot과 canonical의 모든 차이가 설명된다.
- 기존 artifact는 변경되지 않는다.
- 후속 변경을 독립적으로 되돌릴 수 있다.

### 현재 상태

2026-07-11 대조 결과는 `docs/PROVENANCE_AUDIT_2026-07-11.md`에 기록했다. 차이 보존은
완료했다. 실제 작업공간은 canonical `main`을 부모로 하는
`stabilization/baseline-20260711` branch로 복구했으며, 원래 45-file 차이는 별도의 audit
snapshot commit으로 보존했다. 이 branch는 아직 원격에 push하지 않았다.

M0는 완료 상태다. M1 검증 승인을 받아 로컬 baseline과 입력 고정을 진행했으며, 결과는
`docs/BASELINE_VERIFICATION_2026-07-11.md`에 기록했다.

## 5. M1 - 고정된 입력과 자동 검증

### 작업

- `Cargo.lock`을 릴리스 입력으로 추적하고 모든 Cargo 검증에 `--locked`를 사용한다.
- Rust/rustfmt/clippy 버전을 `rust-toolchain.toml`에 고정한다.
- MSRV는 `Cargo.toml`의 `rust-version`으로 별도 명시한다.
- Node와 CI Python 버전을 고정한다.
- Zig, macOS SDK, cargo-zigbuild, cargo-xwin의 버전과 archive digest를 고정한다.
- CI 릴리스 경로에서는 builder의 자동 setup을 금지한다.
- Linux 빠른 gate와 Linux/macOS/Windows native matrix를 추가한다.
- Rust, Python builder, Node, Bash, PowerShell 검증을 자동화한다.
- 실제 사용자 저장소를 쓰는 ignored live test는 일반 CI에서 실행하지 않는다.

### 기본 검증 계약

```text
cargo fmt --all -- --check
cargo check --locked --all-targets --all-features
cargo test --locked --all-features
cargo check --locked --no-default-features --features claude,codex
cargo clippy --locked --all-targets --all-features -- -D warnings
python3 -m unittest discover -s builder/tests -p 'test_*.py' -v
npm ci && npm test && npm run build
bash / PowerShell 정적 검사
```

명령은 저장소 정책에 따라 사용자 승인을 받은 검증 단계에서 실행한다.

### 완료 기준

- required gate가 실패하면 main에 병합할 수 없다.
- fresh checkout에서 lockfile 변경 없이 검증된다.
- 세 OS의 platform-specific 경로가 native runner에서 컴파일되고 테스트된다.
- 릴리스 경로에 unpinned tool download가 없다.

### 진행 상태 - 2026-07-11

로컬 구현과 Linux aarch64 검증은 녹색이다.

- release Rust 1.96.1: fmt, 두 check, 842 test, full Clippy 통과
- upstream MSRV Rust 1.93.0: 두 check와 842 test 통과
- system Rust 1.93.1: 두 check와 842 test 통과
- Python builder 107개, Node 6개, Bash 구문 검사 통과
- Rust/Node/Python과 rustup/cross-builder 버전, Cargo lockfile, rustup/Zig/SDK SHA 고정
- release/CI implicit setup 금지, receipted local rustup + exact Rust Cargo offline 실행,
  rustup 조회 실패 fail-closed
- receipt 없는 모든 non-native release 경로를 clean/tool probe 전에 fail-closed
- noninteractive compiler/output override, Cargo config, forged/unknown target fail-closed
- SHA-pinned official action과 격리 storage를 사용하는 CI workflow 구현

M1은 아직 완료 상태가 아니다. branch를 push하지 않아 native Linux/macOS/Windows workflow,
PowerShell parser, ephemeral website publish build가 실제 runner에서 실행되지 않았다. 또한
zigbuild, Windows GNU/LLVM, cargo-xwin SDK/CRT payload receipt, native system tool attestation,
OS-level release network sandbox, 별도 승인 대상인 6-target release build가 남아 있다.
이 gate가 닫히기 전에는 M2/M3/M4 구현을 시작하지 않는다.

## 6. M2 - private filesystem과 개인정보

### 작업

- AI `searchdata`를 기밀 transcript 파생 데이터로 분류한다.
- Unix directory `0700`, file `0600`; Windows protected DACL을 생성 시점부터 강제한다.
- 기존 cache 권한을 안전하게 교정하고, 교정 실패 시 AI 검색만 fail-closed 한다.
- GJC transcript prompt의 공용 `/tmp` 직접 쓰기를 private ephemeral file과 RAII cleanup으로
  교체한다.
- `ensure_private_dir`, `atomic_replace_private`, `create_private_ephemeral` primitive를
  검증된 JSONL writer를 기반으로 공용화한다.
- sensitive overwrite에서 목적 파일을 먼저 삭제하지 않는다.

### 완료 기준

- umask와 관계없이 transcript 파생 데이터를 다른 로컬 사용자가 읽을 수 없다.
- 성공, 오류, 취소, timeout 뒤에 prompt temp가 남지 않는다.
- symlink, FIFO, reparse point, 동시 writer, write fault 테스트가 통과한다.
- 권한이 불확실한 데이터는 AI agent에 전달되지 않는다.

## 7. M3 - 세션 데이터 무결성

### 권장 정책

- TUI preview: `BestEffort`, 가능한 내용을 보여주되 손상을 숨기지 않는다.
- AI index 및 복제/변환/설치: `LosslessRequired`, 손실 가능성이 있으면 중단한다.
- 명시적 schema 감사: `SchemaAudit`, 미지 event type과 구조 위반까지 검사한다.

기존 public reader API를 깨지 않고 `ReadPolicy`, `ReadOutcome`, bounded diagnostics를
추가한다. 진단에는 transcript 원문이 아니라 line number와 오류 종류만 기록한다.

활성 JSONL은 읽기 전후 identity, size, mtime을 비교한다. 마지막 partial line은 작성 중인
상태일 수 있으므로 preview에서는 경고하고, mutation 경로에서는 안정된 snapshot을 요구한다.
임의 sleep으로 readiness를 추정하지 않는다.

### 완료 기준

- malformed input이 정상 데이터로 간주되어 새 artifact를 만들지 않는다.
- 실패한 복제/변환은 부분 target을 남기지 않는다.
- 유효하지만 미지인 event는 provenance에 보존된다.
- partial recovery는 명시적 승인과 영구 diagnostics 없이 사용할 수 없다.

## 8. M4 - 프로세스 수명주기와 파괴 경로

모든 종료/삭제 경로를 다음 네 종류로 분류한다.

1. `ExplicitUserKill`
2. `OwnedSetupRollback`
3. `NonDestructiveRelease`
4. `AutomaticHousekeeping`

kill, killall, reset, stale cleanup, runtime file 제거, session delete, clone rollback, folder
snapshot 삭제, parent/auxiliary 정리를 모두 감사한다. 각 경로는 권한 근거, 대상 identity,
실행 직전 재검증, 실패 시 preserve, idempotent retry, 사용자 표시를 가져야 한다.

삭제는 `prepare -> confirm -> identity revalidate -> execute`로 분리한다. 확인 뒤 대상이
교체되었거나 live/unverified 상태이면 아무것도 삭제하지 않는다.

### 플랫폼별 증거

- Linux: `/proc` argv, start ticks, process group
- macOS: `KERN_PROCARGS2`, `PROC_PIDTBSDINFO`, zombie 판정
- Windows: direct command-line 조회, creation FILETIME, Job Object breakaway, runtime metadata가
  사라진 live daemon 재발견

### 완료 기준

- `Unknown -> dead/kill/delete` 전이가 없다.
- 모든 파괴 primitive는 직전 identity 재검증을 거친다.
- 세 OS에서 detach/종료 후 보존, restore, 중복 start 방지가 입증된다.
- partial deletion을 성공으로 표시하지 않는다.

## 9. M5 - 검증 가능한 릴리스

- 새 버전과 `vX.Y.Z` tag를 정확히 일치시킨다.
- tag의 단일 SHA에서 6개 artifact를 clean build한다.
- architecture, ABI, `--version`, `--help`, 격리 HOME의 `--check`를 검증한다.
- SHA-256, size, target, Git SHA, toolchain을 담은 manifest와 SBOM을 생성한다.
- draft release에서 전 target을 확인한 후 한 번에 publish한다.
- 일부 target만 성공한 partial release는 허용하지 않는다.
- installer는 mutable branch URL 대신 immutable release manifest를 사용한다.
- 하나의 manifest가 cokacmux와 호환 cokacdir의 version, URL, size, digest를 함께 고정한다.
- 두 프로그램을 모두 검증한 뒤 pair 단위로 교체하고 실패 시 둘 다 복원한다.
- checksum을 발행자 인증으로 오해하지 않도록 signed manifest 또는 CI attestation을 추가한다.

공개된 tag와 asset은 교체하지 않는다. 수정은 새 patch release로만 배포한다.

## 10. M6 - 점진적 모듈화

동작 변경과 파일 이동을 한 변경에 섞지 않는다. 권장 추출 순서는 다음과 같다.

1. settings, keybindings, UI geometry/help 등 비파괴 leaf
2. IPC wire type과 순수 lifecycle model
3. runtime path와 private read/write
4. lock, discovery, liveness 합성
5. daemon과 PTY ownership
6. client, attach, input acknowledgement
7. OS termination
8. App coordinator와 UI

각 단계 전에 IPC JSON, CLI help, config, UI, runtime metadata golden fixture를 고정한다.
wire field, 사용자 동작, timeout, 상태 전이를 바꿔야 추출할 수 있다면 중단하고 별도 설계
승인을 받는다.

## 11. 공통 중단 조건

- 실행 중인 agent 또는 사용자 데이터에 위해가 발생한다.
- 입력 중복, 오배달, 손실이 발생한다.
- `Unknown`을 사망이나 삭제 허가로 축약한다.
- 테스트를 통과시키기 위해 timeout이나 sleep을 늘려야 한다.
- 기존 format, IPC wire, CLI, config, 키, UI를 의도치 않게 변경한다.
- 실패를 로그만 남기고 성공으로 보고한다.

이 중 하나라도 발생하면 다음 단계로 진행하지 않고 직전 녹색 기준점으로 돌아간다.
