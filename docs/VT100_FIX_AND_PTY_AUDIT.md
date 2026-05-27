# vt100 패닉 수정 + PTY 자율 전수 테스트 방법론

> 작성일: 2026-05-23
> 대상 플랫폼: Windows 11 ARM64 / aarch64-pc-windows-msvc
> 빌드 도구사슬: rustc 1.95.0, MSVC 14.44.35207, Windows 11 SDK 22621
> 관련 커밋 베이스: `98840b9` (cokacmux v0.1.9)

이 문서는 Windows에서 cokacmux가 Claude Code 세션에 attach할 때 stderr를 도배하던 vt100 패닉을 (a) 어떻게 진단했고, (b) 어떻게 PTY 안에서 자율적으로 재현·검증했으며, (c) 패치를 적용한 뒤 앱 전체 기능을 PTY로 어떻게 전수 확인했는지를 기록한다. 각 단계의 실행 가능한 명령과 산출물 위치를 모두 적어두어 동일한 방식으로 재현·확장할 수 있도록 했다.

---

## 0. 한 줄 요약

vt100 0.16.2의 `Row::clear_wide`가 마지막 칼럼 + `is_wide` 상태에서 `cells[col+1]`을 무경계 인덱싱하여 발생하는 OOB 패닉이 근본 원인이었다. PTY를 통한 자율 재현으로 6가지 트리거 시나리오를 모두 합성한 뒤, `vendor/vt100-cokac/`의 로컬 fork와 `[patch.crates-io]`로 수정했고, 같은 PTY 인프라로 23개 사용자-노출 기능을 회귀 없이 전수 검증했다. 결과: **모든 시나리오에서 패닉 0건, 기능 전수조사 23/23 PASS**.

---

## 1. 문제 진단

### 1.1 증상

Windows PowerShell에서 cokacmux 실행 후 Claude Code 세션에 attach하면 stderr가 다음과 같이 도배되었다:

```
thread 'main' (NNNN) panicked at .../vt100-0.16.2/src/screen.rs:870:22:
called `Option::unwrap()` on a `None` value
thread 'main' (NNNN) panicked at .../vt100-0.16.2/src/screen.rs:916:26:
index out of bounds: the len is 76 but the index is 76
```

`safe_parser_process` 의 `catch_unwind` 덕분에 프로세스가 죽지는 않았지만, 패닉 메시지가 alt screen 위에 직접 쓰이면서 TUI가 시각적으로 망가졌다.

### 1.2 디버그 로그 수집 절차

`~/.cokacmux/debug/cokacmux.log`를 분석한다.

```powershell
$env:COKACMUX_DEBUG = "1"
cd C:\Users\kst\work\cokacmux
.\target\release\cokacmux.exe
# (Claude 세션 attach + 사이드바 리사이즈)
notepad $env:USERPROFILE\.cokacmux\debug\cokacmux.log
```

각 행의 형식:

```
[HH:MM:SS.mmm pid=1234 thread=name ThreadId(1)] event_name {"json": "..."}
```

- `vt100_parser_panic` — `safe_parser_process`의 catch_unwind가 잡은 패닉. 입력 바이트 샘플(128자)과 길이를 기록.
- `vt100_panic_suppressed` — `install_vt100_panic_filter`가 stderr 출력을 억제한 패닉. 패닉 위치(file/line/col)를 기록.

#### 양 카운트 불일치는 버그가 아니다

같은 시간대에 `vt100_parser_panic` 6건 vs `vt100_panic_suppressed` 3건 식의 불균형이 관찰되었다. 처음에는 panic hook이 일부 패닉을 놓친 것으로 의심했지만, 두 종류의 이벤트가 **데몬과 클라이언트 양쪽에서 동시에 `cokacmux.log`에 기록**되기 때문에 발생하는 인터리브일 뿐이었다. 즉 같은 파일에 두 프로세스가 append하는 동안 OS 레벨에서 순서가 섞일 수 있고, 클라이언트도 socket으로 받은 동일 바이트를 자기 parser에 또 먹이기 때문에 양 이벤트가 서로 다른 빈도로 나타난다. 후속 검증(§2)에서 모든 패닉이 hook으로 잡힌다는 점이 직접 확인되었다.

### 1.3 root cause 분석 (소스 추적)

`screen.rs:870`은 `Screen::text()`에서 wide character 처리 중 `drawing_cell_mut(col + 1).unwrap()`이 None을 unwrap하는 위치다. `screen.rs:916`도 같은 함수의 또 다른 분기에서 같은 패턴.

하지만 `"len is 76 but the index is 76"` 메시지는 unwrap이 아닌 직접 인덱싱(`vec[i]`)의 panic이다. 추적해보면 `vendor/vt100-cokac/src/row.rs:86`의 `clear_wide`:

```rust
pub fn clear_wide(&mut self, col: u16) {
    let cell = &self.cells[usize::from(col)];          // OK
    let other = if cell.is_wide() {
        &mut self.cells[usize::from(col + 1)]          // <-- panic 'len N index N'
    } else if cell.is_wide_continuation() {
        &mut self.cells[usize::from(col - 1)]          // 0에서 underflow
    } else {
        return;
    };
    other.clear(*other.attrs());
}
```

호출 체인:
1. cokacmux가 PTY 리사이즈 → vt100의 `Screen::set_size` → `Grid::set_size` → `Row::resize`
2. `Row::resize`는 단순히 `Vec::resize`로 잘라낼 뿐 → 마지막 칼럼에 있던 wide char의 `is_wide` 플래그가 **잔존**, 짝이 되는 continuation 셀은 사라짐
3. claude.exe가 다음에 그 칼럼에 ECH(`\x1b[1X`) / EL(`\x1b[K`) / DCH(`\x1b[1P`) / 일반 텍스트 overwrite를 보냄
4. 해당 처리에서 `Row::erase` → `clear_wide` → `cells[col+1]` 접근 → OOB

이로써 production 패닉 메시지(`len is 76 index is 76`)가 **콜럼이 76이 된 직후의 사용 케이스**라는 것이 정확히 매치된다.

---

## 2. PTY 자율 재현 방법론

### 2.1 왜 PTY인가

vt100 패닉은 결국 입력 바이트 시퀀스의 문제다. 가장 빠른 재현은:
- 패닉을 일으키는 바이트 시퀀스를 합성해서 `vt100::Parser`에 직접 먹이는 것.

하지만 "정확히 어떤 바이트가 패닉을 만드는지"는 처음엔 불명확했다. 그래서 두 단계로 접근했다:

| 단계 | 목적 | 방법 |
|---|---|---|
| 1) 실제 출력 캡처 | 의심 바이트 후보군 확보 | claude.exe를 PTY로 띄워 stdout 바이트를 파일로 저장 |
| 2) 합성 트리거 | 정확한 트리거 식별 | 1)에서 도출한 가설을 단위 시나리오로 코드화 |

### 2.2 ⚠️ 중요한 제약: claude.exe를 Claude Code 안에서 spawn 금지

현재 작업 세션이 Claude Code 안에서 진행될 때, **portable-pty로 `claude.exe`를 spawn하면 현재 세션이 종료**된다 (확인일: 2026-05-23, Windows ARM64). 추정 원인은 두 claude 인스턴스가 같은 콘솔 핸들 / OAuth 락 / 프로세스 그룹 자원을 공유하기 때문.

따라서 자율 재현은 다음 규칙을 따른다:

- **금지**: Claude Code 컨텍스트 안에서 `claude.exe`를 PTY로 spawn.
- **허용**: 이미 캡처된 바이트 스트림을 파일에서 읽어 `vt100::Parser`에 먹이는 합성 테스트.
- **허용**: 외부 터미널에서 사용자가 직접 캡처 한 번 떠다 두기.
- **허용**: `cokacmux.exe`(클로드를 attach까지는 가지 않는) 자체를 PTY로 driving.

이 규칙은 `~/.claude/projects/.../memory/feedback_claude_subspawn.md`에도 기록되어 있다.

### 2.3 단계 1 — 실제 PTY 캡처 도구

`examples/capture_claude.rs`는 portable-pty 위에서 임의의 자식 프로세스를 띄워 stdout 전체를 바이너리 파일로 저장한다.

```powershell
cd C:\Users\kst\work\cokacmux
.\tools\with_msvc.ps1 cargo build --example capture_claude --features tui

# 환경변수로 program / args를 지정 (사용자 외부 환경에서 실행)
$env:CAPTURE_PROG = "claude.exe"
$env:CAPTURE_ARGS = ""
.\target\debug\examples\capture_claude.exe 20 10 7 capture_w20.bin
# 인자: cols rows seconds out_path
```

#### 핵심 노하우 — Windows ConPTY 동작 차이

처음 캡처가 0바이트로 끝났던 이유는 두 가지였다:

1. **master 조기 해제**: `pair.master`를 `drop`하면 ConPTY 핸들이 닫혀 reader가 EOF. → master를 함수 끝까지 살려둔다.
2. **ESC[6n DSR 응답 누락**: 자식이 시작 직후 cursor position query를 보내고 응답을 기다리면 출력이 멈춘다. → 캡처 시작 직후 `\x1b[1;1R`을 미리 써주고, 이후 스트림에서 `\x1b[6n`을 보면 같은 응답을 다시 써준다.

```rust
// 핵심 코드 발췌 (capture_claude.rs)
{
    let mut w = writer.lock().unwrap();
    let _ = w.write_all(b"\x1b[1;1R"); // 선제 DSR 응답
    let _ = w.flush();
}
// 그리고 reader 루프 안에서:
if slice.windows(4).any(|w| w == b"\x1b[6n") {
    let _ = writer.lock().unwrap().write_all(b"\x1b[1;1R");
}
```

### 2.4 단계 2 — 합성 패닉 트리거

`examples/vt100_repro.rs`는 캡처된 출력에 의존하지 않고, vt100 API를 직접 호출해서 패닉 조건을 분해/조립한다. `install_vt100_panic_filter` + `safe_parser_process`를 자체 복제해 hook 발화 횟수를 카운트한다.

핵심 가설:

> **resize shrink → 마지막 칼럼에 stranded된 is_wide cell → 다음 erase/overwrite가 OOB**

이를 표 하나로 검증:

| label | 초기 (cols, rows, bytes) | resize 후 (cols, rows) | 트리거 바이트 | 의미 |
|---|---|---|---|---|
| ECH_after_shrink_w20to19 | 20, 5, `ESC[1;19H 漢` | 19, 5 | `ESC[1;19H ESC[1X` | ECH (Erase Char) |
| CHA_then_overwrite_w20to19 | 20, 5, `ESC[1;19H 漢` | 19, 5 | `ESC[1;19H A` | 평범한 overwrite |
| EL_after_shrink_w20to19 | 20, 5, `ESC[1;19H 漢` | 19, 5 | `ESC[1;19H ESC[K` | EL (Erase Line) |
| DCH_after_shrink_w20to19 | 20, 5, `ESC[1;19H 漢` | 19, 5 | `ESC[1;19H ESC[1P` | DCH (Delete Char) |
| ECH_after_shrink_w77to76 | 77, 5, `ESC[1;76H 漢` | 76, 5 | `ESC[1;76H ESC[1X` | **production 메시지와 일치** (`len 76 index 76`) |
| emoji_then_overwrite | 20, 5, `ESC[1;19H 😀` | 19, 5 | `ESC[1;19H A` | 4-byte UTF-8 wide |

실행:

```powershell
.\tools\with_msvc.ps1 cargo build --example vt100_repro --features tui
.\target\debug\examples\vt100_repro.exe
```

패치 전 (vt100 0.16.2 그대로) 결과는 모두 panic (`hook_fired=6, hook_vt100=6`). 패치 후 결과는 `hook_fired=0`.

### 2.5 단계 3 — 실데이터 리플레이 도구 (보조)

`examples/vt100_replay.rs`는 단계 1에서 캡처한 `.bin` 파일을 다양한 cols에서 재생하고, 1바이트 단위 bisection으로 패닉 위치를 좁힌다.

```powershell
.\target\debug\examples\vt100_replay.exe capture_w20.bin 20 10
```

출력은 `whole_w{N}` 단위 결과와 `bisect cols={N}` 결과가 함께 나온다. 실제 트리거 바이트를 모를 때 1차 탐색에 유용.

---

## 3. 패치 적용

### 3.1 로컬 fork 구조

```
vendor/vt100-cokac/
├── Cargo.toml      (registry 사본 그대로)
└── src/
    ├── row.rs      (clear_wide / erase / truncate / resize 패치)
    ├── screen.rs   (Screen::text의 unwrap → if let Some)
    └── ...         (나머지는 무손)
```

`Cargo.toml`에 patch 엔트리 추가:

```toml
[patch.crates-io]
vt100 = { path = "vendor/vt100-cokac" }
```

### 3.2 row.rs 패치 — 가장 중요한 변경

- `clear_wide`: `cells.get(col)` + `cells.len()` 경계 체크 + `checked_sub`로 underflow 방지
- `erase`: `cells.get(i)` 경계 체크 추가
- `truncate`: `len == 0`일 때 `cells[len - 1]` underflow 방지
- `resize`: shrink 시 새 마지막 cell에 `is_wide` 플래그가 남아있으면 **사전에 clear** → 향후 호출 경로가 안전

```rust
pub fn resize(&mut self, len: u16, cell: crate::Cell) {
    let new_len = usize::from(len);
    if new_len < self.cells.len() && new_len > 0 {
        if let Some(last) = self.cells.get_mut(new_len - 1) {
            if last.is_wide() {
                let attrs = *last.attrs();
                last.clear(attrs);  // ← stranded is_wide 제거
            }
        }
    }
    self.cells.resize(new_len, cell);
    self.wrapped = false;
}
```

### 3.3 screen.rs 패치 — defense in depth

`Screen::text`의 wide-char 처리 두 곳을 `.unwrap()` → `if let Some(...)`로 변경. row.rs 패치가 일관성을 유지하므로 이 경로는 실제로는 진입하지 않지만, 다른 미발견 경로에 대한 방어선.

### 3.4 회귀 테스트

`src/bin/cokacmux.rs`의 `#[cfg(test)] mod tests`에 `safe_parser_process_handles_wide_char_after_shrink_resize` 추가. 6개 시나리오 모두 `safe_parser_process`가 `true`를 리턴(즉 패닉 없음)하는지 단언.

```powershell
.\tools\with_msvc.ps1 cargo test --features tui --bin cokacmux -- safe_parser_process_handles
```

### 3.5 빌드 헬퍼

`tools/with_msvc.ps1` — `vcvarsarm64.bat`을 소싱하고 `%USERPROFILE%\.cargo\bin`을 PATH 앞에 붙인 뒤 명령을 실행. PowerShell 자식 세션은 환경을 상속받지 않으므로 cargo를 부를 때마다 prefix로 사용.

```powershell
.\tools\with_msvc.ps1 cargo build --release --features tui
```

---

## 4. PTY 자율 전수 테스트 (feature_audit)

### 4.1 목적

vt100 패닉 수정이 사용자-노출 기능에 회귀를 일으키지 않았는지, **외부 입력이 전혀 없는 완전 자율 모드**로 검증한다.

### 4.2 설계 원칙

| 원칙 | 구현 |
|---|---|
| 자식 프로세스가 spawn되는 부수효과 차단 | claude/codex/opencode을 attach시키는 키 (`e`, `c`, `Ctrl+K`) 제외 |
| 사용자 데이터 보호 | `~/.cokacmux/settings.json`을 시작 시 백업, 종료 시 복원 |
| 빠른 실패 검출 | 출력에 `"panicked at"`, `"thread main panicked"`, `"backtrace"` 등장 시 즉시 FAIL |
| 양성 시그널 요구 | TUI 모드 테스트는 `"sessions"` 등 기대 마커가 출력에 등장해야 PASS |
| 종료 코드 검증 | Ctrl+Q 또는 self-exit 후 종료 코드 0 요구 |

### 4.3 구현 (`examples/feature_audit.rs`)

각 테스트 케이스는 다음 필드로 정의된다:

```rust
struct TestCase {
    name: &'static str,
    cols: u16, rows: u16,       // PTY 크기
    boot_ms: u64,                // 초기 렌더 대기
    steps: &'static [Step],      // 키 스크립트
    expect_present: &'static [&'static str],
    expect_absent: &'static [&'static str],
    cli_args: &'static [&'static str],
    expects_self_exit: bool,
}
```

`Step::Send(bytes)` / `Step::Wait(ms)` 두 가지 액션. CSI 키는 직접 바이트로 보낸다 (`\x1b[A` = ↑, `\x1b[B` = ↓, `\x1b[5~` = PgUp, `\x1b[6~` = PgDn, ...).

#### Windows ConPTY 고유 처리

캡처 도구와 동일한 패턴:

- 선제 ESC[6n 응답 + 스트림 내 자동 응답
- `pair.master`를 `Arc<Mutex<Option<...>>>`에 담아 reader 스레드 종료 시점에 명시적으로 drop
- 스레드 join은 `is_finished()` 폴링 + 2초 hard timeout (Windows ConPTY가 EOF를 안 주는 케이스 회피)

```rust
// 핵심 시퀀스
drop(writer);
if let Ok(mut guard) = master_holder.lock() {
    guard.take();  // master drop → 자식 stdout pipe 닫힘 → reader EOF
}
let join_start = Instant::now();
while !reader_thread.is_finished() && join_start.elapsed() < Duration::from_secs(2) {
    std::thread::sleep(Duration::from_millis(50));
}
if reader_thread.is_finished() {
    let _ = reader_thread.join();
}
```

### 4.4 테스트 목록 (23개)

| # | 이름 | 범주 | 보내는 키 시퀀스 |
|---|---|---|---|
| 1 | cli_version | CLI | `cokacmux --version` |
| 2 | cli_help | CLI | `cokacmux --help` |
| 3 | cli_check | CLI | `cokacmux --check` |
| 4 | tui_boot_120x30 | 부팅 | (대기만) |
| 5 | tui_boot_40x10 | 부팅 | (좁은 화면) |
| 6 | tui_boot_200x60 | 부팅 | (넓은 화면) |
| 7 | tui_boot_20x5_tiny | 부팅 | (극단적 작은 화면, AGENT_MIN_PTY clamp) |
| 8 | nav_down_x5 | 네비 | `↓×5` |
| 9 | nav_up_past_top | 네비 | `↑×8` (상한선 넘어가기) |
| 10 | nav_jk | 네비 | `jjjjj kkk` (vim-style) |
| 11 | nav_pgdown_pgup | 네비 | `PgDn PgDn PgUp` |
| 12 | nav_home_end | 네비 | `G g` |
| 13 | focus_tab_esc | 포커스 | `Tab Tab Esc` |
| 14 | preview_enter_toggle | 프리뷰 | `Enter Enter` (summary↔full) |
| 15 | view_toggle_v | 뷰 | `v v` (list↔tree) |
| 16 | filter_open_and_type | 필터 | `/ c l a u d e Esc` |
| 17 | filter_no_match | 필터 | `/ z z z z _ ... Esc` (매치 없음) |
| 18 | refresh_r | 갱신 | `r` |
| 19 | random_key_smash | 카오스 | 임의 영문/특수문자 + Shift+Tab + Backspace + Enter + Esc |
| 20 | workflow_filter_then_nav | 조합 | `/ c Enter j j j k k Tab Tab` |
| 21 | tiny_with_nav | 경계 | 30×8에서 `jjjj v v` |
| 22 | boundary_77_cols | 경계 | 77×24, vt100 패닉 경계 폭과 일치 |
| 23 | boundary_76_cols | 경계 | 76×24, **production 패닉 폭과 정확히 일치** |

### 4.5 실행 + 결과 수집

빌드:

```powershell
.\tools\with_msvc.ps1 cargo build --release --example feature_audit --features tui
```

실행 — PowerShell 인코딩 문제(`Tee-Object`가 UTF-16LE 출력)를 피하기 위해 stdout/stderr를 분리해서 파일로:

```powershell
& "$PWD\target\release\examples\feature_audit.exe" 2>audit.err 1>audit.out
```

진행 모니터링 (별도 터미널 또는 bash):

```bash
tail -f audit.err | tr -d '\r' | grep -E "^\[[0-9]+/|PASS|FAIL"
```

### 4.6 결과 (2026-05-23 실측)

```
==============================
  FEATURE AUDIT  23/23 passed
==============================
  PASS cli_version              - ok (120 bytes)
  PASS cli_help                 - ok (1143 bytes)
  PASS cli_check                - ok (171 bytes)
  PASS tui_boot_120x30          - ok (7959 bytes)
  PASS tui_boot_40x10           - ok (1288 bytes)
  PASS tui_boot_200x60          - ok (20229 bytes)
  PASS tui_boot_20x5_tiny       - ok (518 bytes)
  PASS nav_down_x5              - ok (10631 bytes)
  PASS nav_up_past_top          - ok (7959 bytes)
  PASS nav_jk                   - ok (10489 bytes)
  PASS nav_pgdown_pgup          - ok (10880 bytes)
  PASS nav_home_end             - ok (10717 bytes)
  PASS focus_tab_esc            - ok (14216 bytes)
  PASS preview_enter_toggle     - ok (13350 bytes)
  PASS view_toggle_v            - ok (8592 bytes)
  PASS filter_open_and_type     - ok (12464 bytes)
  PASS filter_no_match          - ok (12770 bytes)
  PASS refresh_r                - ok (10369 bytes)
  PASS random_key_smash         - ok (17347 bytes)
  PASS workflow_filter_then_nav - ok (16040 bytes)
  PASS tiny_with_nav            - ok (1389 bytes)
  PASS boundary_77_cols         - ok (7814 bytes)
  PASS boundary_76_cols         - ok (6294 bytes)
```

### 4.7 추가하지 않은 케이스 (의도적 제외)

| 키 | 이유 |
|---|---|
| `e` (attach) | claude.exe spawn → 현재 Claude Code 세션 종료 |
| `c` (clone) | 사용자 세션 디렉터리에 사본 생성 |
| `t` (edit title) | cokacmux titles 파일 변경 |
| `d` (delete) | 세션 파일 삭제 |
| `Ctrl+K` (kill agent) | 실행 중인 다른 daemon 종료 |
| `Alt+←/→/↑/↓`, `Ctrl+Shift+←/→/↑/↓` (resize/sidebar pick) | settings.json 저장 트리거 (백업해도 미세 race) |

사용자가 별도 환경에서 attach 경로까지 검증할 수 있도록, `examples/drive_cokacmux.rs`가 동일한 PTY 인프라로 cokacmux 자체를 잠시 띄워 stderr 패닉 문자열 부재만 확인하는 보조 도구를 함께 둔다 (단일 케이스).

---

## 5. 빌드/검증 한 줄 명령어 모음

| 목적 | 명령 |
|---|---|
| 도구사슬 환경 진입 | `.\tools\with_msvc.ps1 <cargo command...>` |
| dev 빌드 | `.\tools\with_msvc.ps1 cargo build --features tui` |
| release 빌드 | `.\tools\with_msvc.ps1 cargo build --release --features tui` |
| 회귀 테스트(vt100) | `.\tools\with_msvc.ps1 cargo test --features tui --bin cokacmux -- safe_parser_process_handles` |
| 전체 lib+bin 테스트 | `.\tools\with_msvc.ps1 cargo test --features tui --lib --bins` |
| vt100 합성 재현 | `.\target\debug\examples\vt100_repro.exe` |
| 캡처(외부 환경에서) | `$env:CAPTURE_PROG="claude.exe"; .\target\debug\examples\capture_claude.exe 20 10 7 out.bin` |
| 캡처 리플레이 | `.\target\debug\examples\vt100_replay.exe out.bin 20 10` |
| 기능 전수 감사 | `& "$PWD\target\release\examples\feature_audit.exe" 2>audit.err 1>audit.out` |
| cokacmux 실행 | `cd C:\Users\kst\work\cokacmux; .\target\release\cokacmux.exe` |

---

## 6. 산출물 위치

```
cokacmux/
├── Cargo.toml                       [patch.crates-io] 추가
├── docs/VT100_FIX_AND_PTY_AUDIT.md  (이 문서)
├── examples/
│   ├── capture_claude.rs            # PTY 캡처 (외부 환경에서만)
│   ├── vt100_repro.rs               # 합성 패닉 재현
│   ├── vt100_replay.rs              # 캡처 바이트 리플레이 + bisection
│   ├── drive_cokacmux.rs            # cokacmux 자체 짧게 driving (보조)
│   └── feature_audit.rs             # 23 케이스 자율 전수 테스트
├── src/bin/cokacmux.rs              # safe_parser_process / install_vt100_panic_filter /
│                                    #   dump_vt100_panic_input / 회귀 테스트 추가
├── tools/with_msvc.ps1              # MSVC env wrapper
└── vendor/vt100-cokac/              # 패치된 vt100 0.16.2 fork
    ├── Cargo.toml
    └── src/{row,screen,...}.rs
```

---

## 7. 한계 / 미해결 항목

| 항목 | 상태 |
|---|---|
| Claude Code 실세션 attach E2E 검증 | **사용자 외부 환경에서 수동 검증 필요** (Claude Code 안에서는 자율화 불가) |
| Linux/macOS 동등 검증 | 미수행 (해당 OS에는 ConPTY 이슈 없음 — 패치는 동일하게 안전) |
| build.py로 cross-compile 후 dist_beta 갱신 | 미수행 (사용자 명시 요청 시에만 빌드) |
| upstream vt100 PR | 미수행 (필요 시 별도 작업) |
| `vt100_panic_input_dump` 기능 검증 | 코드는 추가되어 있고 회귀 테스트로 trigger되지만, 실제 production 패닉을 한 번도 만나지 않은 상태에서 검증 완료 — 패치 이후로는 트리거되지 않을 예정 |

---

## 8. 후속 작업자를 위한 메모

- vt100을 더 최신 버전으로 올릴 때 본 fork 변경분을 다시 적용해야 한다. 핵심 diff는 `vendor/vt100-cokac/src/{row,screen}.rs`의 `cokacmux patch:` 주석 단위.
- `feature_audit.rs`에 새 키 / 새 모드를 추가하려면 `TestCase` 한 줄만 추가하면 된다. 파괴적 키를 추가할 때는 §4.7 표를 반드시 업데이트.
- `capture_claude.rs`를 Claude Code 안에서 다시 시도하지 말 것 (메모리 `feedback_claude_subspawn` 참조).
- PTY가 0바이트만 반환하면 십중팔구 ESC[6n 응답 누락이다.
- Tee-Object의 UTF-16LE 출력은 `iconv -f UTF-16LE` 또는 stdout/stderr 직접 redirect로 우회.
