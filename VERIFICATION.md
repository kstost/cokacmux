# 검증 체크리스트

이 프로젝트에서 중점적으로 확인해야 할 항목들. 빌드 후 순서대로 점검하고,
"상시 감시" 항목은 평소 운용 중 주기적으로 확인한다.
(근거 원칙: CLAUDE.md의 Core Goal / Invariants)

## 0. 빌드·테스트 기본

- [ ] `python3 build.py --linux-arm64` 성공
- [ ] `cargo test` 통과 — 특히 이번에 수정된 테스트:
      keybinding 3종(워처 방식), `runtime_refresh_without_discovery_preserves_cached_live_shell`(확정 사망 의미론),
      `live_pty_output_round_trips_*` 2종(disk writer 드레인), `tests/roundtrip.rs`(불변식 6)
- [ ] 빌드 직후 **`cokacmux killall` → 재시작** (데몬 프로토콜·디스크 라이터가 새 바이너리 필요)

## 1. attach/전환 (비동기화 + 핸드셰이크 검증)

- [ ] sessions에서 Enter로 에이전트 열기 — "attaching ..." 표시 후 정상 진입
- [ ] 새 에이전트 생성(n), 셸 열기, 복원 프롬프트(저장 데이터 있는 세션), 폴더 생성 프롬프트 각각 동작
- [ ] Alt+↑/↓ 연타 — 누른 횟수만큼 정확히 이동(붕괴/건너뜀 없음), 최종 대상에 안착
- [ ] 전환 직후 키 입력이 새 에이전트에 정확히 들어감 (불변식 2)
- [ ] detach(Ctrl+]) 직후 늦은 attach 결과가 화면을 다시 끌고 가지 않음
- [ ] 로그에 `daemon_client_attach_snapshot_failed` / `daemon_client_output_failed` 가 **0건**

## 2. agents/sessions 목록 (순서·동기화·유령)

- [ ] 에이전트들이 활동 중일 때 사이드바 **순서가 절대 바뀌지 않음** (first-seen 고정)
- [ ] 새 에이전트는 항상 맨 아래 추가, 죽은 항목은 슬롯 반납
- [ ] 에이전트 내부에서 `exit` → 마지막 출력 표시 → "exited" 상태 → **1초 내 목록에서 제거**
- [ ] Ctrl+K kill → 즉시 제거 (기존 동작 유지)
- [ ] 죽은 항목에 포커스 시도 → 즉시 목록에서 치유 제거, 다음 키가 그 아래 항목으로
- [ ] codex 새 에이전트 띄운 뒤 5–10초 내 sessions 목록의 실세션이 **live로 표시**
      (`new_agent_backing_session_linked` 이벤트 확인)
- [ ] 그 실세션 행에서 Enter → 새로 launch하지 않고 **기존 에이전트로 복귀** (이중 실행 방지, 불변식 1)

## 3. 디스크 스톨 내성 (Core Goal 직접 검증)

스톨 유도: 대형 I/O 부하(예: `dd if=/dev/zero of=/tmp/x bs=1M count=8000 oflag=direct`) 또는 자연 발생 대기.

- [ ] 스톨 중 **키 입력·화면·목록 네비게이션이 멈추지 않음** (원칙 1)
- [ ] 스톨 중 **에이전트 화면이 계속 갱신됨** ← 이번 disk-writer 변경의 핵심 검증
- [ ] 스톨 중 상태가 quiet로 정직하게 표시, 해제 후 자동 복귀 (원칙 3)
- [ ] 해제 후 attach/detach 폭주·유령·고착 없음 (원칙 2)
- [ ] `~/.cokacmux/debug/cokacmux-stalls.log` 에 워치독 기록이 남는지 (debug 꺼도 기록됨)
- [ ] 스톨 중 쌓인 스크롤백 공백은 `daemon_disk_job_dropped` 로 계수되는지

## 4. 로깅 규율 (원칙 4)

- [ ] `--debug` 없이 실행 + 모든 데몬에 한 번씩 attach → `cokacmux.log` 가 **더 이상 자라지 않음**
      (attach 시 데몬이 클라이언트 모드 채택; 예외는 stalls 파일뿐)
- [ ] 셸에 `COKACMUX_DEBUG=1` export된 상태에서 --debug 없이 실행 → 새 데몬이 로그를 만들지 않음
- [ ] `--debug` 운용 시 로그 증가량이 시간당 수 MB 수준 (이전: 7분에 90MB)
- [ ] `debug_log_lines_dropped` 가 보이면 = 스톨 중 드롭 발생 — 정상 동작 확인용

## 5. 프로세스 수명주기 (불변식 1)

- [ ] TUI 종료 후 데몬·에이전트 생존, 재시작 시 재발견·재접속
- [ ] Ctrl+K 후 `ps`로 고아 프로세스 없음 (자식 → 프로세스 그룹 순 종료)
- [ ] killall 후 `~/.cokacmux/agents/`, `~/.cokacmux/debug/` 가 삭제되고 설정/키바인딩/데이터는 남음
- [ ] 데몬 재시작 후 attach 시 스크롤백 리하이드레이트 정상 (pty 로그 무결성)

## 6. 상시 감시 (운용 중 주기 점검)

- [ ] `cokacmux-stalls.log` 가 비어있는지 — 내용이 있으면 UI 스톨이 있었다는 증거, 즉시 분석
- [ ] 목록에 "포커스 안 되는 항목"이 보이면 → 유령 회귀, 로그 확보
- [ ] 같은 세션에 에이전트 2개가 뜨는 일이 없는지 (backing 연결 회귀)
- [ ] 키 입력이 다른 에이전트로 가는 일이 절대 없는지 (불변식 2 — 최우선)

## 6.5 런타임 판정표 — 시나리오별 기대 로그 시그니처

`--debug`로 실행 후 각 시나리오를 수행하고 로그 순서를 대조한다.
시그니처가 어긋나면 해당 전이가 회귀한 것이다.

| 시나리오 | 기대 시그니처 (순서대로) | 판정 기준 |
|---|---|---|
| 에이전트 자체 exit | `daemon_child_exit` → (최종 드레인 출력 전달) → 클라이언트에 Exited → `daemon_pty_log_cleanup` → `agent_runtime_file_remove` ×2 | exit 직전 출력이 화면에 보임; 목록에서 ≤1s 제거; `agent_meta_stale_removed` 불필요(정상 정리됨) |
| Ctrl+K (active) | `kill_agent_switch_selected` → `attach_request_queued`/`attach_ready` | 다음 에이전트로 자동 전환; 고아 프로세스 0 (`ps` 확인) |
| 데몬 kill -9 (크래시 모사) | TUI: `agent_reader_thread_exit` → exited 설정 → 다음 워커 사이클에 `agent_meta_stale_removed` → 목록 제거 | 자식 CLI도 수 초 내 종료(PTY EOF); 목록 잔존 ≤10s |
| 에이전트 전환 (Alt+↓) | `agent_switch_selected` → `daemon_attach_request`(새 데몬) → `daemon_client_connected` → `agent_switch_ready` | `daemon_client_attach_snapshot_failed` 0건; 이전 데몬에 `daemon_client_disconnected` 또는 detach |
| 죽은 항목 포커스 | `agent_switch_failed` (`daemon_gone_removed: true`) | 항목 즉시 소멸; 다음 키는 그 아래 항목으로 |
| codex 새 에이전트 → 실세션 연결 | ≤10s 내 `new_agent_backing_session_linked` | sessions 목록에서 해당 실세션 live 표시; 그 행 Enter → `restore_live_agent_ready`(신규 launch 아님) |
| 디스커버리 순서 변화 | (이벤트 없음) | 사이드바 순서 불변 — 변하면 레지스트리 회귀 |
| TUI 재시작 | `main_start` → 디스커버리로 기존 데몬 전원 재발견 | 직전 세션과 사이드바 순서가 다를 수 있음(첫 빌드 순서) — 세션 내에서만 고정 보장 |

## 7. 알려진 잔여 한계 (이상이 아님)

- 제목 저장·세션 삭제·설정 저장은 메인 스레드 동기 fs — 스톨 중 해당 동작만 잠시 지연
- 자식 CLI 자체가 디스크에 묶여 출력을 멈추는 경우는 표시 불가(어떤 멀티플렉서도 동일)
- 장시간 스톨 시 스크롤백 로그에 공백 가능 (라이브 화면은 무손실)
- sessions 뷰는 프레임마다 전체 행 구성 — 세션 수천 개 규모가 되면 뷰포트 가상화 필요
