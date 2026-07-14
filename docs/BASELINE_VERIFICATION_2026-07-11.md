# M1 Baseline Verification - 2026-07-11

> 정책 변경(2026-07-14): 이 문서의 GitHub Actions, hosted CI, artifact checksum/서명,
> manifest 또는 attestation 관련 미완료 항목은 더 이상 프로젝트 요구사항이 아니다.
> 로컬 검증만 사용하며 `docs/PROJECT_POLICY.md`가 이 역사적 기록보다 우선한다.

## 1. 결론

Linux aarch64의 로컬 기준선은 녹색이다. 릴리스 검증 툴체인은 Rust `1.96.1`, MSRV는
upstream Rust `1.93.0`에서 검증한 `1.93`으로 분리한다. 최종 격리 실행에서 Rust 테스트
842개가 통과했고 9개 live/환경 의존 테스트는 명시적으로 ignored 상태를 유지했다.

이 문서를 작성한 2026-07-11 당시에는 GitHub Actions 실행과 attestation도 M1 완료 조건으로
분류했지만, 2026-07-14 정책 변경으로 모두 폐기했다. 현재 남은 로컬 확인 항목은 이 환경에
없는 PowerShell parser와 별도 승인이 필요한 6-target release build뿐이다. checksum, 서명,
receipt, attestation 또는 hosted runner의 부재는 미완료 항목이나 차단 사유가 아니다.

## 2. 승인된 검증 범위

- Rust 1.93 계열과 builder-local Rust 1.96.1의 `fmt`, `check`, `test`, `clippy`
- Python builder와 Node website 단위 테스트
- Bash 구문 검사
- 실제 사용자 저장소를 쓰지 않는 격리 테스트

`python3 build.py`를 통한 release 또는 multi-target build는 이 단계의 승인 범위에 포함하지
않았다. website build도 tracked publish output을 바꾸므로 당시 실행하지 않았다. 향후 확인이
필요하면 hosted workflow가 아니라 로컬 임시 출력에서 수행한다.

## 3. 최초 기준선

### 비-Rust

| 검증 | 결과 |
| --- | --- |
| Python builder unittest | 39/39 통과 |
| Node website test | 6/6 통과 |
| `bash -n` (`manage.sh`, Windows dependency installer, EOL normalizer) | 통과 |
| PowerShell parser | 로컬 `pwsh` 부재로 미실행 |
| ShellCheck | 로컬 `shellcheck` 부재로 미실행 |

### Rust

| 툴체인 | 최초 결과 |
| --- | --- |
| system Rust 1.93.1 | check 통과, lib 113 통과, bin 625 통과/7 실패/1 ignored |
| builder Rust 1.96.1 | fmt/check 통과, 같은 7개 테스트 실패, Clippy library 24건 실패 |

실패한 7개 테스트는 개별 재실행에서도 모두 재현되었으므로 우발적 flake가 아니었다.

1. `discovery_unknown_never_prunes_and_definitive_absence_does`
2. `e_on_stale_same_cwd_new_agent_does_not_block_launch_selector`
3. `e_on_stale_same_cwd_provider_session_does_not_block_launch_selector`
4. `killall_removes_stale_agent_runtime_files`
5. `reused_pid_agent_meta_is_cleaned_and_treated_as_idle`
6. `stale_agent_meta_is_cleaned_and_treated_as_idle`
7. `stale_agent_runtime_sweep_preserves_matching_child`

## 4. 테스트 저장소 격리 사고와 처리

최초 Rust test는 `COKACMUX_TEST_ROOT`와 HOME 계열 변수를 강제하지 않았다. 그 결과 일부 bin
test가 실제 `/root/.cokacmux/agents`에 0-byte `.mutation` sidecar를 만들었다. 첫 두 suite
실행 직후 직접 관측한 신규 파일은 40개였다. 개별 재현 실행까지 끝난 뒤 처음 보존한 완전한
격리 기준점에는 총 280개가 있었으며 모두 0-byte다. 그 사이 개별 재현 실행이 추가 파일을
만들었을 수 있으므로 40개를 제외한 각 파일의 생성 provenance는 단정하지 않는다.

실제 runtime에는 당시부터 현재까지 6개의 daemon이 살아 있고 6개 socket이 LISTEN 상태다.
테스트 후에도 그대로 보존되었다. 빈 `/tmp/.tmpLwXm5w` residue도 보존했다. live runtime과
섞인 상태에서 자동 삭제하는 것이 더 위험하므로 사용자 파일이나 residue는 하나도 정리하지
않았다.

원인은 binary crate의 `cfg(test)` helper만으로 linked library와 subprocess의 production 경로를
막을 수 없다는 crate 경계였다. 이후 모든 Rust test runner에 다음을 함께 적용했다.

- `COKACMUX_TEST_ROOT`, `COKACMUX_HOME`, `COKACMUX_CONFIG_DIR`
- `HOME`, `USERPROFILE`, `TMPDIR`, `TMP`, `TEMP`
- 네 XDG directory와 `LOCALAPPDATA`, `APPDATA`
- `COKACMUX_DEBUG=0`, `COKACMUX_TRACE=0`
- PI/GJC provider override를 빈 값으로 고정
- HOME 변경 전에 기존 `CARGO_HOME`, `RUSTUP_HOME` 보존

binary/library 내부의 test-only private root는 방어 계층으로 유지하지만, runner 환경 격리를
대체하지 않는다. 실제 Codex DB를 읽는 provider layout test는 명시적 live-read gate로 ignored
처리했다.

입력 고정 검토 중 `/root/.cargo/bin/rustup --version`을 repository 안에서 실행했을 때도 새
`rust-toolchain.toml` override가 자동 동기화를 시작했다. 그 결과 `/root/.rustup`에
`1.96.1-aarch64-unknown-linux-gnu`와 5개 component가 추가되었다. 단순 조회도 외부 mutation이
될 수 있다는 직접 증거이며, 해당 toolchain은 임의 삭제하지 않고 보존했다. 이후 release
builder는 명시적 `rustup run`, offline Cargo 경계, 조회 오류의 fail-closed 처리를 사용하도록
추가 보강했다.

## 5. 기준선에서 수정한 문제

### Runtime와 discovery

- complete discovery의 definitive absence가 오래된 cached Live를 영구 보존하지 않게 했다.
- discovery 시작 시 key baseline을 저장해 scan 도중 추가된 새 daemon을 오래된 결과가
  지우지 않게 했다.
- same-CWD stale 판정 뒤 `e` 동작을 async refresh 결과에 따라 계속하도록 수정했다.
- refresh가 peer를 Live로 확인하면 deferred launch를 계속 차단하는 회귀 테스트를 추가했다.
- stale cleanup은 endpoint, child, PTY log, cwd lock을 삭제 직전 다시 검증하고 불확실하면
  보존한다.
- metadata가 없고 endpoint가 존재하는 상태를 definitive dead로 취급하지 않는다.

### 정적 품질

- `lines().flatten()` 4곳을 I/O 오류에서 무한 반복하지 않는 `map_while(Result::ok)`로 바꿨다.
- library의 Clippy 24건과 이후 드러난 bin/example/integration test 진단을 정리했다.
- production에서 사용하지 않는 코드는 플랫폼 cfg, `cfg(test)`, 실제 삭제로 분류했고
  crate-wide `dead_code` 허용은 추가하지 않았다.
- 오래된 unstable rustfmt option을 제거해 Rust 1.96.1 format 검사가 경고 없이 통과한다.

## 6. 최종 로컬 증거

| 툴체인 | all-target/all-feature check | 최소 feature check | test | 추가 증거 |
| --- | --- | --- | --- | --- |
| Rust 1.96.1 | 통과 | 통과 | 842 통과, 9 ignored | fmt 및 full Clippy `-D warnings` 통과 |
| upstream Rust 1.93.0 | 통과 | 통과 | 842 통과, 9 ignored | MSRV 근거 |
| system Rust 1.93.1 | 통과 | 통과 | 842 통과, 9 ignored | 최초 7개 실패 해소 확인 |

각 최종 full test의 실제 runtime mutation 비교는 모두 `280 -> 280`, 신규 0개였다.

Rust 1.96.1의 통과 구성은 다음과 같다.

```text
lib:             114 passed
bin:             639 passed, 1 ignored
live_acceptance:   0 passed, 3 ignored
live_readonly:     0 passed, 1 ignored
pivot:            25 passed
provider_layout:  20 passed, 1 ignored
roundtrip:         39 passed
session:           4 passed, 3 ignored
doctest:           1 passed
total:           842 passed, 9 ignored
```

주요 보존 로그와 격리 root:

```text
/tmp/cokacmux-rust196-final.gHelFw
/tmp/cokacmux-rust1930-final.eqhbXQ
/tmp/cokacmux-rust1931-final.kE9P5y
/tmp/cokacmux-clippy-deadcode.OjguF9
/tmp/cokacmux-final-contract.dGz2Zw
/tmp/cokacmux-final-msrv.XwQCS8
```

핀 파일까지 포함한 최종 Rust 1.96.1 실행에서는 fmt, all-target check, 최소 feature check,
842 test, full Clippy가 모두 통과했다. 최종 upstream Rust 1.93.0 재실행에서도 두 check와
842 test가 통과했다. Python builder test는 고정 입력 회귀가 추가된 뒤 107/107, website test는
6/6으로 통과했다.

## 7. 고정한 입력과 로컬 검증 계약

- `rust-toolchain.toml`: Rust/rustfmt/Clippy `1.96.1`
- `Cargo.toml`: `rust-version = "1.93"`
- `.node-version`: Node `24.18.0`
- `.python-version`: Python `3.14.4`
- 로컬 검증 npm: `11.17.0`
- builder: rustup `1.29.0`, Rust `1.96.1`, cargo-zigbuild/cargo-xwin `0.23.0`, Zig `0.13.0`,
  macOS SDK `14.0`
- rustup-init: exact archive URL과 Linux/macOS/Windows x86_64/aarch64 6개 host의 공식
  SHA-256 고정, digest 불일치 시 실행 전 차단
- Zig: Linux/macOS/Windows의 x86_64/aarch64 6개 host archive SHA-256 고정
- macOS SDK archive: SHA-256 고정; download/cache/existing archive 모두 추출 전 검증
- `--no-auto-setup`: 준비된 project-local rustup만
  허용하고 `rustup run 1.96.1 cargo`, `--locked --offline`, `CARGO_NET_OFFLINE=true` 적용
- noninteractive 실행 전 `rustc`/Cargo가 모두 exact `1.96.1`을 보고하는지 검증
- rustup distribution/update root를 `static.rust-lang.org`로 강제하고 inherited toolchain/host
  override 제거
- rustup toolchain/target 조회 실패는 미설치가 아닌 unknown으로 보존하고 자동 변경 차단
- Windows GNU/LLVM은 반대 architecture host toolchain 대신 native Rust `1.96.1`에
  gnullvm target std를 추가
- Zig/SDK directory 교체는 same-filesystem backup과 rollback 적용
- `Cargo.lock`과 `website/package-lock.json`: tracked input, 모든 Cargo 검증에 `--locked`
- 일반 로컬 검증은 ignored test를 활성화하는 인자를 사용하지 않음
- 로컬 build는 auto-setup을 사용할 수 있고 `--no-auto-setup`은 이를 강제로 끈다.
- `.github/workflows`와 GitHub Actions는 사용하지 않음
- noninteractive 경로에서 compiler/linker/output override 환경변수와 unreceipted Cargo
  config를 거부하고, target 분류와 artifact name을 경계에서 다시 검증

## 8. 남은 로컬 확인과 후속 이슈

### 로컬 확인

- PowerShell parser와 로컬 임시 website publish build 결과를 필요할 때 확인한다.
- Zig/SDK directory 교체는 일반 예외 rollback과 deterministic backup 복구를 모두 사용한다.
  backup rename 직후 중단되면 다음 설치 시 이전 설치를 복원하고, 새 directory 공개 직후
  중단되면 남은 backup을 정리한다.
- 별도 승인 뒤 6-target release build를 수행한다.

GitHub Actions, hosted runner, checksum, 서명, signed manifest, SBOM, receipt 및 attestation은
이 목록의 gate가 아니며 추가할 필요가 없다.

### M4로 이관할 runtime 설계

- 동일 key의 daemon generation이 discovery 도중 교체되는 race
- live metadata가 존재하지만 endpoint가 없는 상태의 UI 표현
- endpoint가 cleanup revalidation 사이에 나타나는 full-sweep injection test
- cleanup 보존 여부와 UI attach 가능성을 분리하는 명시적 Unknown 상태
