# Provenance Audit - 2026-07-11

## 1. 목적과 범위

현재 `/shared/cokacmux`에는 `.git` metadata가 없었다. 이 문서는 canonical 저장소와 현재
snapshot의 관계를 확인하고, 기존 source와 배포 artifact를 덮어쓰기 전에 보존해야 할
기준점을 기록한다.

이 감사에서는 Rust build/test와 배포를 실행하지 않았다. 원격 조회, content diff, digest,
기존 바이너리의 `--version`만 사용했다.

## 2. Canonical 기준

- Repository: `https://github.com/kstost/cokacmux`
- Default/only branch: `main`
- HEAD: `9f1ad54b60b36a7e6f354a5079ffa1842b4b0b15`
- Commit time: `2026-07-09T00:15:27+09:00`
- Subject: `Fix new session folder completion overlay`
- Tags: 없음
- Git tree: `56d9161687c6cdf170f8a4b840d8ab08c35b68d5`

## 3. 현재 snapshot과의 차이

Canonical index를 현재 작업공간에 연결해 content 기준으로 비교했다.

- canonical tracked files: 135
- 동일 tracked files: 99
- 수정 tracked files: 36
- 누락/삭제 tracked files: 0
- nonignored untracked files: 9
- 전체 차이: 45 files, `+17,845 / -2,346`
- pre-documentation local tree: `c2dc3561d4aac12eab5a228286c2da05be6f9779`
- snapshot audit commit: `87b5e59f69fb87865fc4bd6d05a96c1a42a81131`

`src/bin/cokacmux.rs` 하나의 차이는 `+14,161 / -1,852`이고, 64,077줄에서 76,386줄로
늘었다. Rust `#[test]` 선언은 canonical 722개에서 local 854개로 증가했다. 이는 정적
선언 개수이며 현재 source의 실행 통과를 의미하지 않는다.

양쪽 모두 Cargo version은 `0.2.39`이다. `Cargo.lock`은 canonical과 동일하다.
`Cargo.toml`의 유일한 내용 차이는 `uuid` dependency에 `v4` feature가 추가된 것이다.

다음 실행 파일은 local에서 executable mode가 추가되었다.

- `build.py`
- `install_windows_build_deps.sh`
- `manage.sh`

## 4. Nonignored 신규 파일

```text
builder/tests/__init__.py
builder/tests/test_build_script.py
builder/tests/test_executor.py
builder/tests/test_manage_sh.py
builder/tests/test_targets.py
builder/tests/test_tools.py
website/scripts/copy-build.test.mjs
website/src/copy-command.js
website/src/copy-command.test.js
```

## 5. 변경 묶음 판정

### Runtime/process lifecycle

- authenticated attach와 local peer 검증
- CSPRNG auth token과 private runtime file
- bounded frame, queue, read/write와 backpressure
- PTY input acknowledgement, sequence deduplication, replay
- runtime symlink/FIFO/reparse 방어와 atomic publication
- worker panic/watchdog/restart
- `Unknown = preserve` 종료/정리 race 방어
- auxiliary 관계와 termination 복구

### Session 및 adapter

- provider session-id path traversal 방어
- install identity/cwd rewrite
- same-id self-clone 거부
- private context/session/debug file
- atomic JSONL replace
- OpenCode transactional removal과 native row 보존
- terminal control sanitization
- Pi/GJC native id와 tool metadata 보존 강화

### Builder 및 installer

- `--no-auto-setup`의 비변경 계약 강화
- archive traversal과 partial install 방어
- private staging과 atomic publish/rollback
- multi-target 일부 성공 결과의 publish 거부
- 두 프로그램 다운로드 후 실행/이름 검증과 atomic install

Installer의 mutable `main` URL 사용과 checksum/signature 부재는 의도적인 프로젝트
정책이다. 둘 다 해결 대상이나 릴리스 차단 사유가 아니며, 배포 진위용 checksum, 디지털
서명, signed manifest 및 attestation은 요구하지 않는다. 자세한 정책은
`docs/PROJECT_POLICY.md`에 있다.

### Website

- build output/reference 사전 검증
- symlink/special file와 hashed asset collision 방어
- asset-first, atomic index publish
- clipboard fallback 분리와 Node test

## 6. Artifact 관계

현재 `dist_beta` 6개 파일은 canonical HEAD에 tracked된 파일과 SHA-256이 모두 동일하다.
따라서 현재 local source 변경을 반영한 artifact가 아니다.

```text
284deeb66442e1f5df7eec72f4cd257428a025e6319581bea4f2df718539ce49  cokacmux-linux-aarch64
58fa45e74be84b0b40949f26bb5b77e3532b48f1b3045c11942c780fef7db750  cokacmux-linux-x86_64
17e50bbf1fe5c823c11cd0263189e1ff781b6d6748b9ad20df023c4c1f1c9d29  cokacmux-macos-aarch64
ee57690f1a3ff4571c4925909d56028391177a77959bc3632057929accb13116  cokacmux-macos-x86_64
1f31a38639ba57c0c45f46381f7f74d3806b06e82da5577ba57b5f56947a8fd9  cokacmux-windows-aarch64.exe
fff9d3dc9ad3691f3c2adbad01558f9e03894e50a0ebf27ad368b9cff5ce9ad6  cokacmux-windows-x86_64.exe
```

Linux aarch64 release artifact와 기존 `target/debug/cokacmux`는 모두 `cokacmux 0.2.39`를
출력했다. 그러나 debug binary는 `2026-07-11 05:05:55+09:00`, 현재
`src/bin/cokacmux.rs`는 `2026-07-11 06:11:32+09:00`이므로 debug binary도 최신 source를
증명하지 않는다.

## 7. 보존 증거

감사 과정에서 source를 수정하지 않은 상태로 다음 증거를 `/tmp/cokacmux-canonical.1dE7pl`
아래에 만들었다.

```text
evidence/local-vs-main.patch
  sha256 a1765f56257097aaeb2a639321a4a02de845612ed1a388ada7c8508e07998e86

evidence/source-sha256.txt
  sha256 1d91f8a4cff6ac392ef1db2769ff8c0321464f93ed362ab1518d71dfa1ab240c

evidence/name-status.txt
  sha256 3637878b96a14bd5cb29a4700047208a8c9bab7d9484d23b30f3c4df766c9a3c

evidence/upstream-main.bundle
  sha256 b3c6b73a3f007e56f3d721060bd514bd05d1667b75b06ad0331e31aa3a330a74

evidence/local-snapshot.bundle
  sha256 45696bb5051c87f5ad64565fad85d5a78c8fbfa023ebc302257870b39c832b3d
```

`local-snapshot.bundle`의 `audit/local-snapshot-20260711` branch는 문서 추가 전 local
snapshot과 content 기준으로 일치한다. `/tmp`는 영구 보관소가 아니므로 실제 baseline
채택 시 이 branch를 canonical Git 저장소 또는 별도 durable backup으로 옮겨야 한다.

감사 문서 작성 후 `local-snapshot-with-docs.bundle`도 생성했고, 이를 이용해 실제
작업공간의 Git metadata를 복구했다.

```text
branch: stabilization/baseline-20260711
upstream: origin/main
base: 9f1ad54b60b36a7e6f354a5079ffa1842b4b0b15
snapshot commit: 87b5e59f69fb87865fc4bd6d05a96c1a42a81131
initial documentation commit: 8577f11e0c9a987231decae8a46f6a28adb2caff

evidence/local-snapshot-with-docs.bundle
  sha256 bd981a265f1d4fa9fe24ab3daaef4899d5ce3eca7b936328350638e5ed0d33e6
```

두 commit은 잃어버린 원래 작성 순서를 복원한 것이 아니다. 첫 commit은 발견된 source
snapshot을 content 기준으로 보존한 synthetic audit commit이고, 두 번째는 이 감사와
로드맵 문서를 추가한 commit이다. branch는 원격에 push하지 않았다.

## 8. 판정과 다음 gate

1. 현재 local snapshot은 canonical HEAD 이후의 실질적인 대규모 안정화 작업이다.
2. canonical HEAD로 현재 작업공간을 덮거나 reset해서는 안 된다.
3. 변경의 원 commit, 작성 순서, review 이력은 `.git` 부재 때문에 복구할 수 없다.
4. 현재 45개 파일 차이는 provenance baseline commit으로 채택해 로컬 branch에 보존했다.
5. 기존 0.2.39 artifact는 legacy로 동결하고 새 source artifact로 교체하지 않는다.
6. 새 릴리스 후보는 baseline과 검증 gate를 통과한 뒤 새 patch version을 사용한다.
7. 다음 작업은 M1 baseline 검증과 고정할 toolchain의 실측이다.
