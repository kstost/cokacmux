# Toolchain Baseline - 2026-07-11

## 1. 목적

M1에서 toolchain을 고정하기 전에 현재 host, 기존 debug artifact, local cross-builder가 실제로
어떤 버전을 사용하고 있는지 기록한다. 이 단계에서는 build/test를 실행하지 않았다.

## 2. Host

```text
OS: Linux 6.12.76-linuxkit aarch64
glibc: 2.43
Node: 24.18.0
npm: 11.17.0
Python: 3.14.4
zsh: 5.9
bash: 5.3.9
```

`pwsh`와 `shellcheck`는 현재 host PATH에 없다. 따라서 PowerShell parser 검증과
ShellCheck는 현재 host만으로 M1 gate를 충족할 수 없다.

## 3. System Rust

```text
rustc: 1.93.1 (01f6ddf75 2026-02-11)
cargo: 1.93.1 (083ac5135 2025-12-15)
host: aarch64-unknown-linux-gnu
LLVM: 21.1.8
rustfmt: 설치되지 않음
```

`target/.rustc_info.json`과 `target/debug/cokacmux` 안의 source path는 기존 debug artifact가
system Rust 1.93.1로 만들어졌음을 나타낸다. 그러나 현재 `src/bin/cokacmux.rs`가 이
artifact보다 나중에 수정되었으므로 1.93.1은 현재 source의 검증 완료 toolchain이 아니다.

### PATH 밖의 rustup toolchain

추가 조사에서 `/root/.cargo/bin`은 현재 PATH에 없지만 다음 upstream toolchain을 보유하고
있음이 확인되었다.

```text
1.93.0-aarch64-unknown-linux-gnu
1.95.0-aarch64-unknown-linux-gnu
stable-aarch64-unknown-linux-gnu -> 1.95.0
```

이 1.93.0 설치본은 system distribution의 patched 1.93.1과 별개다. 최종 MSRV 검증에는
upstream 1.93.0을 명시적으로 사용했다.

이 초기 목록을 기록한 뒤, repository의 exact toolchain override 아래에서 실행한 rustup
버전 조회가 자동 동기화를 일으켜 `1.96.1-aarch64-unknown-linux-gnu`도 추가되었다. 이는 최초
inventory 입력이 아니라 이번 검증 중 생긴 외부 mutation이며 삭제하지 않았다.

## 4. Local cross-builder Rust

`build.py`는 local cargo/rustup pair가 존재하면 다음 environment를 사용한다.

```text
CARGO_HOME=/shared/cokacmux/builder/tools/cargo
RUSTUP_HOME=/shared/cokacmux/builder/tools/rustup
```

이 environment에서 실측한 값은 다음과 같다.

```text
toolchain: stable-aarch64-unknown-linux-gnu (default)
rustc: 1.96.1 (31fca3adb 2026-06-26)
cargo: 1.96.1 (356927216 2026-06-26)
rustfmt: 1.9.0-stable (31fca3adb2 2026-06-26)
clippy: 0.1.96 (31fca3adb2 2026-06-26)
```

현재 local builder에는 `stable`이라는 rolling alias만 저장되어 있어 이 directory 자체만으로는
동일 환경을 나중에 재현할 수 없다.
local rustup proxy를 위 environment 없이 직접 실행하면 다른 toolchain 정보가 관측될 수
있으므로, 모든 builder 진입점과 CI는 두 environment 변수를 명시해야 한다.

설치된 target:

```text
aarch64-apple-darwin
aarch64-pc-windows-msvc
aarch64-unknown-linux-gnu
x86_64-apple-darwin
x86_64-pc-windows-msvc
x86_64-unknown-linux-gnu
```

## 5. Cross-build tools

```text
Zig: 0.13.0
cargo-zigbuild: 0.23.0
cargo-xwin: 0.23.0
macOS SDK: 14.0
clang: 21.1.8
lld: 21.1.8
Linux compatibility target configured by builder: glibc 2.17
```

현재 `python3 build.py --status --no-color`는 Rust, Zig, cargo-zigbuild, macOS SDK,
cargo-xwin, clang, lld, llvm-lib, clang-cl을 모두 발견했다. 이는 존재 확인일 뿐 build 성공
증거가 아니다.

M1 구현에서는 builder의 입력 계약을 다음처럼 강화했다.

- rustup-init `1.29.0`의 exact archive URL과 지원 host 6개 공식 SHA-256 고정
- noninteractive는 rustup-init archive digest와 일치하는 project-local rustup만 허용
- 모든 Cargo build variant를 `rustup run 1.96.1 cargo`로 실행하고 rustc/Cargo
  reported version도 exact 검증
- release/CI/`--no-auto-setup`에 `--locked --offline`과 `CARGO_NET_OFFLINE=true` 적용
- cargo-zigbuild/cargo-xwin `0.23.0` exact install과 reported version 검증
- Zig 0.13.0의 지원 host 6개 archive SHA-256 검증
- macOS SDK 14.0 archive SHA-256 검증
- download, local cache copy, 기존 archive의 checksum mismatch를 extraction 전에 차단
- rustup toolchain/target 조회 실패를 unknown으로 전파해 자동 설치 차단
- rustup dist/update root를 공식 static server로 강제하고 inherited override 제거
- Windows gnullvm을 native Rust `1.96.1` toolchain의 target std로 준비
- Zig/SDK staged directory 교체 실패 시 기존 설치 rollback
- release/CI의 implicit setup 금지
- content-addressed receipt가 없는 모든 non-native release/CI 경로를 clean/tool probe
  전에 fail-closed 처리
- noninteractive compiler/linker/output override와 unreceipted Cargo config, forged/unknown target 거부

Python builder 회귀 테스트 107개가 이 계약을 검증한다. 실제 cross-build는 실행하지 않았다.

## 6. 현재 입력 digest

```text
49fc5db1f43e57f0bb03f564bdcdfca6573bd530f1354203697e215a145c0034  Cargo.lock
de689b5c835db773be45660dc0fa3e49793f88e05fc83599972679e59befde06  website/package-lock.json

9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792  rustup-init-1.29.0-linux-aarch64
4acc9acc76d5079515b46346a485974457b5a79893cfb01112423c89aeb5aa10  rustup-init-1.29.0-linux-x86_64
aeb4105778ca1bd3c6b0e75768f581c656633cd51368fa61289b6a71696ac7e1  rustup-init-1.29.0-macos-aarch64
33cf85df9142bc6d29cbc62fa5ca1d4c29622cddb55213a4c1a43c457fb9b2d7  rustup-init-1.29.0-macos-x86_64
3af309e6c3062aa11df0e932954f69d13b734d8a431e593812f3ecd9ff9e6ef6  rustup-init-1.29.0-windows-aarch64.exe
86478e53f769379d7f0ebfa7c9aa97cb76ca92233f79aa2cc0dbee2efaac73c7  rustup-init-1.29.0-windows-x86_64.exe

041ac42323837eb5624068acd8b00cd5777dac4cf91179e8dad7a7e90dd0c556  zig-linux-aarch64-0.13.0.tar.xz
5e4d3be6b445f0eacc0333ff2117e93e4433d8c4fe44053a14f735033a98aaa9  MacOSX14.0.sdk.tar.xz
5c3cb1279642301ace6a5a58e77c5d354bc67069029b1bec08a19340b71a35de  zig-0.13.0/zig
46ffe8eb0e17a1df6e9915887497aa60aafa1abc33f012ea9c666c052b50c05d  cargo-zigbuild
9878f298b6e29eebc4adb416d75cd2ed39e57dc8e364aeaec292fb23752c9778  cargo-xwin

6d4e18f1fa6662db8ab0ae1708c776c0dbbd6ca87202d5521e4dcada82a9611e  local rustc 1.96.1
45ca815482af36fff0a57bd34360f35aa6cd41dc17c18ca7c11d28231bcc60e4  local cargo 1.96.1
f7e584150087c87de7bdc4caf2a7ae6b136f96c4beaf25aa0d3fcb923d74d190  local rustfmt 1.96.1
d8ad812bc5e1501e1a1704238643f588e84d75078b4bd285a0ecf88f5d2f42db  local clippy-driver 1.96.1
```

rustup-init digest는 `static.rust-lang.org`의 `1.29.0` archive checksum에서 대조했다.
Zig archive digest는 지원 host 별로 코드에 고정했다. macOS SDK digest는 현재
local cache payload을 고정한 값이며, 발행자가 독립적으로 제공한 checksum은 아니다.
download, cache copy, 기존 archive는 모두 추출 전에 이 값과 대조한다.

## 7. Pin 결정 결과

같은 working tree와 격리 storage에서 다음을 검증했다.

1. upstream 1.93.0: all-target/all-feature check, 최소 feature check, 842 test 통과
2. system 1.93.1: all-target/all-feature check, 최소 feature check, 842 test 통과
3. builder 1.96.1: fmt, all-target/all-feature check, 842 test, full Clippy 통과
4. 세 full test 모두 실제 runtime mutation 신규 0개

따라서 release/CI toolchain은 `1.96.1`, Cargo MSRV는 `1.93`으로 결정했다. repository에는
`rust-toolchain.toml`과 `Cargo.toml`의 `rust-version`을 별도 계약으로 기록한다.

6-target compile은 아직 실행하지 않았다. 또한 zigbuild, Windows GNU/LLVM,
cargo-xwin SDK/CRT payload receipt가 없으므로 cross-release 재현성 gate는 열린
상태이며 해당 release 경로는 clean/tool probe 전에 의도적으로 차단된다. Cargo
외 build script 네트워크를 완전히 차단하려면 추가 OS-level sandbox도 필요하다.
native macOS/Windows system compiler/linker/SDK는 현재 compatibility prerequisite로만 취급하며,
content receipt 또는 immutable runner attestation 전에는 bit-reproducible release 증거가 아니다.
Zig/SDK 교체는 예외 발생 시 기존 directory를 복원하지만, process interruption 사이의
orphan `.backup`을 자동 복구하는 crash journal은 후속 과제다.
상세 결과는
`docs/BASELINE_VERIFICATION_2026-07-11.md`에 기록했다.
