# Process Lifetime Goals

> 작성일: 2026-07-04
> 대상: `cokacmux`의 에이전트, 터미널, `cokacdir` 등 백그라운드 프로세스 처리

## 한 줄 목표

`cokacmux`는 사용자가 명시적으로 종료하지 않은 백그라운드 프로세스를 `cokacmux` 자신의 판단, 정리, 복원, refresh 결함 때문에 죽이거나 잃어버리면 안 된다.

## 사용자가 원하는 것

사용자는 `cokacmux`를 일시적으로 종료했다가 나중에 다시 켜더라도, 이전에 백그라운드로 띄워 둔 에이전트나 터미널이 그대로 살아 있고 다시 접근 가능하기를 원한다.

이 요구사항의 핵심은 "죽었으면 다시 살린다"가 아니다. 사용자가 원하는 것은 외부 요인이 아닌 `cokacmux`의 이유로 프로세스가 죽거나, 살아 있는데도 `cokacmux`가 놓쳐서 접근 불가능해지는 일이 없어야 한다는 것이다.

## 범위

이 문서에서 말하는 백그라운드 프로세스는 다음을 포함한다.

- 코딩 에이전트 프로세스
- 일반 터미널 프로세스
- `cokacdir` 관련 프로세스
- 오른쪽 패널이나 auxiliary pane에서 시작된 보조 프로세스
- `cokacmux` 종료 이후에도 살아 있어야 하는 detached 또는 hidden 상태의 프로세스

## 핵심 요구사항

### 1. 명시적 종료와 자동 정리를 분리한다

`cokacmux`가 프로세스를 종료해도 되는 경우는 사용자가 명시적으로 종료 의사를 표현한 경우여야 한다.

예:

- 사용자가 해당 에이전트 또는 터미널을 kill/close/delete한 경우
- 전체 종료 명령이 명확히 "자식 프로세스까지 종료"를 의미하는 경우
- 사용자가 확인 가능한 UI 동작으로 프로세스 종료를 요청한 경우

반대로 다음 동작은 프로세스를 죽이면 안 된다.

- `cokacmux` UI 종료
- 레이아웃 전환
- sidebar 갱신
- runtime refresh
- stale entry 정리
- 부모 pane이 사라진 경우의 자동 정리
- startup restore 실패
- 소켓 또는 metadata 불일치만으로 수행되는 cleanup

### 2. 살아있는 프로세스를 놓치면 안 된다

프로세스가 실제로 살아 있다면, `cokacmux`는 그 프로세스를 UI 상태에서 잃어버리면 안 된다. 특히 `cokacmux`를 다시 시작한 직후에는 저장된 metadata만 신뢰하지 말고 실제 런타임 상태를 확인해야 한다.

살아있는 프로세스가 다음 상태로 남는 것은 결함이다.

- sidebar에는 보이지만 focus 이동이 불가능함
- `Ctrl+]`로 복원되지 않음
- 새 터미널을 열었을 때만 뒤늦게 보임
- 내부 registry에는 있으나 attach 경로가 없음
- metadata 정리 과정에서 더 이상 발견되지 않음

### 3. startup restore는 결정적이어야 한다

`cokacmux`를 켜자마자 `Ctrl+]`를 눌렀을 때, 이전 에이전트가 살아 있다면 사용자는 애매한 시간을 기다렸다가 다시 눌러야 해서는 안 된다.

다음 UX는 허용되지 않는다.

- 너무 빨리 누르면 실패하고, 잠시 후 다시 누르면 성공함
- refresh가 아직 끝나지 않았다는 사실을 사용자가 알 수 없음
- 사용자 입력이 runtime discovery보다 먼저 도착했다는 이유로 restore 요청이 소비됨
- 임의 시간 뒤에 다시 시도하면 우연히 동작함

올바른 동작은 다음 중 하나여야 한다.

- 이미 확인된 live runtime 상태로 즉시 복원한다.
- live runtime 확인이 진행 중이면, 사용자의 restore 요청을 보존한 뒤 확인 결과가 도착했을 때 이어서 처리한다.
- 복원할 수 없다면 "살아있는 대상이 없음" 또는 "확인 실패" 같은 결정적인 상태를 남긴다.

### 4. 임의 delay로 race를 덮지 않는다

프로세스 생명주기 문제를 해결하기 위해 근거 없는 고정 시간 sleep, timeout, debounce, retry delay를 넣는 방식은 목표에 맞지 않는다.

허용되는 시간 기반 처리는 다음 정도로 제한한다.

- 사용자가 볼 수 있는 상태 표시
- diagnostics 또는 slow-path logging
- OS kill 절차에서 graceful termination 후 강제 종료로 넘어가기 위한 짧은 wait
- 외부 API나 시스템 호출의 명세상 필요한 timeout
- daemon socket connect, identity query, subprocess command 같은 I/O 작업의 bounded timeout. 이 timeout은 "준비됐을 것"을 추정하는 delay가 아니라, 응답하지 않는 외부 I/O가 cleanup/refresh/attach 경로를 무한히 붙잡지 못하게 하는 상한이어야 한다.

허용되지 않는 처리는 다음과 같다.

- "500ms 정도 기다리면 refresh가 끝났을 것" 같은 추정
- 일정 시간이 지나면 pending restore 요청을 포기함
- startup race를 sleep으로 숨김
- 테스트에서만 맞는 임의 지연값에 의존함

### 5. metadata는 보조 정보일 뿐 최종 진실이 아니다

metadata, socket path, pid file, sidebar entry는 실제 프로세스 생존 여부와 attach 가능성을 판단하기 위한 단서다. 이 정보가 stale하거나 불완전할 수 있으므로, `cokacmux`는 가능한 경우 실제 프로세스와 런타임 응답을 함께 확인해야 한다.

특히 다음 판단은 보수적으로 해야 한다.

- pid가 재사용되었는지 확인
- pid의 시작 시각 또는 start tick이 일치하는지 확인
- daemon socket이 응답하는지 확인
- child process가 아직 살아 있는지 확인
- metadata가 가리키는 프로세스가 정말 `cokacmux`가 관리하던 대상인지 확인

확실하지 않은 상태에서 프로세스를 죽이는 쪽으로 판단하면 안 된다.

### 6. 자동 cleanup은 release 우선이어야 한다

부모 pane, right panel, auxiliary registry, stale runtime entry를 정리할 때는 프로세스를 종료하기 전에 먼저 소유 관계를 해제해서 standalone live process로 보존할 수 있는지 검토해야 한다.

자동 cleanup의 기본 방향은 다음이어야 한다.

- UI 소유권 해제
- registry entry 정리
- attach 가능한 live process로 보존
- 실제 종료는 명시적 kill 경로에서만 수행

### 7. 복구보다 비파괴가 우선이다

프로세스가 죽었을 때 다시 실행하는 기능은 이 목표의 핵심이 아니다. 재시작은 동일 세션, PTY, 터미널 상태, 에이전트 내부 상태를 보존하지 못할 수 있다.

따라서 우선순위는 다음과 같다.

1. `cokacmux` 결함으로 프로세스를 죽이지 않는다.
2. 살아있는 프로세스를 놓치지 않는다.
3. attach 또는 restore 상태를 결정적으로 관리한다.
4. 외부 요인으로 이미 죽은 경우에만 별도 복구 정책을 논의한다.

## 외부 요인과 내부 결함의 구분

다음은 `cokacmux`가 완전히 통제할 수 없는 외부 요인이다.

- OS reboot
- 사용자의 직접 kill
- parent shell, terminal emulator, tmux, systemd, login session 정책
- OOM killer
- 에이전트 바이너리 자체 crash
- 권한 변경 또는 파일시스템 손상

하지만 외부 요인이 있을 수 있다는 사실은 `cokacmux` 내부 결함을 정당화하지 않는다. `cokacmux`가 해야 할 일은 내부 원인으로 프로세스를 죽이거나 잃는 경로를 제거하고, 외부 요인과 내부 판단 실패를 구분할 수 있는 로그와 상태를 남기는 것이다.

## 결함으로 간주할 증상

다음 증상은 프로세스 처리 목표 관점에서 결함 후보로 다룬다.

- 자기 전에 실행해 둔 에이전트가 아침에 `Ctrl+]`로 복원되지 않음
- 새 터미널을 열면 sidebar에는 이전 에이전트가 보이지만 focus 이동이 되지 않음
- `cokacmux` 재시작 직후에는 복원이 안 되고 잠시 후 다시 누르면 됨
- refresh 완료 전 사용자 입력이 무시되거나 소비됨
- stale cleanup이 살아있는 child process를 종료함
- metadata 불일치만 보고 실제 live process를 제거함
- right panel 또는 auxiliary 정리 과정에서 프로세스가 같이 죽음
- setup 실패 중 이미 spawn된 child process가 방치되거나 반대로 잘못된 대상이 종료됨

## 수용 기준

프로세스 생명주기 관련 수정은 최소한 다음 기준을 만족해야 한다.

- 명시적 kill 경로와 자동 cleanup 경로가 코드상 분리되어 있다.
- 자동 cleanup 경로는 살아있는 child process를 종료하지 않는다.
- startup 직후 `Ctrl+]` restore 요청은 live discovery 완료 전에도 보존된다.
- live discovery 결과가 도착하면 pending restore가 이어서 처리된다.
- pending restore는 임의 timeout 때문에 소비되지 않는다.
- 같은 프로세스인지 판단할 때 pid 단독 비교에 의존하지 않는다.
- pid 재사용 방지를 위한 start token은 플랫폼이 제공하는 충분한 해상도의 값을 우선 사용한다. Linux는 `/proc/<pid>/stat` start ticks, macOS는 `proc_pidinfo(PROC_PIDTBSDINFO)`의 `pbi_start_tvsec/tvusec`, Windows는 process creation FILETIME을 사용한다.
- daemon argv 식별은 가능한 한 OS가 보존한 argv 원본을 사용한다. Linux는 `/proc/<pid>/cmdline`, macOS는 `sysctl(KERN_PROCARGS2)`, Windows는 `CommandLineToArgvW`로 파싱한 command line을 사용하며, `ps -o command`처럼 사람이 읽는 문자열을 재구성한 값을 destructive 또는 replacement 판단의 주 근거로 삼지 않는다.
- stale metadata 정리는 실제 live runtime 확인 없이 파괴적으로 동작하지 않는다.
- 관련 동작에는 회귀 테스트가 있다.
- race 해결이 근거 없는 sleep이나 magic number에 의존하지 않는다.

## 구현 원칙

- 프로세스 종료는 명시적 사용자 의도가 있는 함수 이름과 call path에서만 일어나야 한다.
- 함수 이름에 cleanup, refresh, normalize, discover, restore, release가 들어간 경로는 기본적으로 비파괴적이어야 한다.
- 비파괴적으로 확신할 수 없는 경우에는 상태를 보존하고 진단 로그를 남긴다.
- "알 수 없음"은 "죽여도 됨"이 아니라 "더 확인해야 함"으로 취급한다.
- pid start token을 읽지 못한 상태는 token mismatch가 아니라 unknown으로 취급한다. 실제 종료 대상인지 판단하는 경로에서는 unknown을 일치로 보지 않고, 자동 cleanup 경로에서는 unknown을 이유로 파괴적으로 정리하지 않는다.
- stale metadata를 정리하기 전에 같은 runtime stem의 daemon socket이 identity를 응답할 수 있는지 확인한다. identity가 복구되면 그 정보를 live state로 사용하고, identity가 없더라도 socket reachability가 확인되면 meta/socket/pty log를 unlink하지 않는다.
- daemon/child 종료 뒤 runtime 파일을 지울 때는 종료 요청 전에 읽은 metadata 바이트나 endpoint inode를 그대로 요구하지 않는다. identity 응답이나 daemon의 마지막 write/unlink가 metadata와 endpoint를 정상적으로 갱신할 수 있으므로, 대상 generation이 inactive임을 확인한 뒤 둘을 다시 관측하고 key, pid/start token, endpoint generation, start lock, socket reachability를 mutation lock 안에서 재검증한다. 삭제는 attach endpoint의 부재를 먼저 확정한 뒤 metadata를 제거하고, 두 identity 파일이 실제로 사라진 경우에만 cleanup 성공으로 취급한다.
- direct child가 살아 있는 동안 같은 runtime에 속한 descendant의 `pid + start token`을 metadata에 기록하고, daemon이 exact direct child의 종료를 관측한 뒤에도 group/tree snapshot을 주기적으로 다시 확인하여 종료 순간의 조회 실패나 늦게 발견된 descendant를 공유 상태와 metadata write queue에 보존한다. bounded witness set에서는 최신 재검증 관측과 이미 기록된 Windows 단절 subtree identity를 PID 정렬보다 우선한다. leader가 먼저 종료된 process group/tree를 `lost`로 확정하거나 명시적 kill로 정리할 때는 현재도 살아 있는 기록된 descendant identity를 continuity witness로 확인해야 하며, 숫자 PGID/root PID만으로 표시하거나 신호를 보내면 안 된다. Windows에서 종료된 중간 부모 때문에 Toolhelp parent chain이 끊겨도 이미 기록된 exact witness를 daemon 생존과 종료 완료 판정에 포함한다.
- witness가 없는 이전 버전 metadata 또는 witness를 재확인할 수 없는 상태는 보존한다. 자동 stale cleanup은 witness 유무와 관계없이 살아 있는 process group/tree를 종료하지 않는다.
- 새 daemon을 시작하기 전에 metadata/socket으로 기존 daemon을 확인할 수 없더라도, OS process table에서 같은 `--agent-daemon <provider> <session_id>`를 가진 daemon이 발견되면 새 프로세스로 덮어쓰지 않는다.
- 새 daemon 시작은 같은 agent key 단위로 직렬화한다. 기존 daemon 확인과 새 daemon spawn 사이에는 per-agent start lock을 잡고, lock 획득 후 반드시 daemon connect/live check를 다시 수행한다. 두 `cokacmux` 인스턴스가 동시에 같은 agent를 시작하더라도 둘 다 "없다"고 판단하고 중복 spawn하면 안 된다.
- daemon socket reachability probe와 attach connect는 platform별 raw blocking connect가 아니라 bounded connect 경로를 사용한다.
- startup, refresh, restore는 서로 독립된 timer가 아니라 상태 전이로 연결한다.
- 새 daemon을 시작한 뒤 attach할 때는 "소켓이 곧 생길 것"이라고 추정하는 polling delay에 의존하지 않는다. daemon이 socket bind와 child spawn을 끝낸 뒤 readiness 신호를 보내고, 부모는 그 신호를 검증한 다음 attach한다.
- attach 단계 실패는 daemon death의 증거가 아니다. 특히 새 daemon start 이후 readiness 또는 socket connect가 실패했다면, 살아 있는 daemon을 놓치지 않도록 live discovery refresh를 요청해야 한다.
- 명시적 kill 경로에서 `SIGTERM` 뒤 `SIGKILL`로 넘어가기 전의 유예 시간은 startup/restore/liveness 판단에 쓰지 않는다. 이 유예는 사용자 의도가 이미 확인된 종료 경로에서만 쓰는 상한이며, pid/process-group 상태를 관찰해 이미 종료된 경우 강제 종료를 생략한다.
- 테스트는 빠른 성공 경로뿐 아니라 느린 discovery, queued refresh, stale metadata, pid 재사용, parent pane 소멸 경로를 포함해야 한다.

## tmux에서 참고할 원칙

`tmux`는 오랫동안 같은 문제를 다뤄 온 프로젝트이므로, `cokacmux`의 프로세스 생명주기 모델은 다음 원칙을 참고한다.

조사 기준은 `tmux/tmux`의 `31b0b0c99e39ced9a42fe3674b80f9eb0e009da7` 커밋이다. 2026-07-04에 확인한 `origin/master`가 이 커밋과 일치한다.

확인한 주요 소스 지점은 다음과 같다.

- [`client.c:72-164`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/client.c#L72-L164): server socket 연결 실패를 임의 sleep으로 처리하지 않고, lock 획득과 retry를 상태 전이로 처리한다.
- [`client.c:627-664`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/client.c#L627-L664): client는 server에 연결된 사실만으로 attached 상태가 되지 않고, server의 `MSG_READY`를 받은 뒤 attached 상태로 전환한다.
- [`server.c:174-260`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/server.c#L174-L260): server fork 중 signal을 막고, socket 생성과 client 등록을 마친 뒤 server start lock을 해제한다. lock은 readiness를 추정하는 delay가 아니라 중복 server start를 막는 소유권 전이 장치다.
- [`server.c:281-305`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/server.c#L281-L305): server exit 판단은 attached client/session뿐 아니라 `job_still_running()` 상태도 확인한다. 살아 있는 job이 있으면 빈 UI 상태만으로 즉시 종료하지 않는다.
- [`server.c:461-510`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/server.c#L461-L510): `SIGCHLD`를 받아 `waitpid(WAIT_ANY, WNOHANG|WUNTRACED)`로 child 종료를 확정하고 pane 상태에 반영한다.
- [`job.c:109-231`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/job.c#L109-L231): fork 또는 pty fork 전 signal을 막고, 성공한 child의 `pid`, fd, event callback을 중앙 job table에 기록한다.
- [`job.c:356-418`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/job.c#L356-L418): `waitpid`로 확인된 job death만 상태 전이에 반영하고, 전체 job 종료는 `job_kill_all` 같은 명시 경로로 분리한다.
- [`proc.c:230-295`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/proc.c#L230-L295): process signal handler 등록과 child exec 전 signal cleanup을 중앙화한다.
- [`tmux.h:1263-1313`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/tmux.h#L1263-L1313): pane은 `pid`, exit `status`, `PANE_EXITED`, `PANE_STATUSREADY`를 명시적으로 들고 있다.
- [`window.c:381-402`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/window.c#L381-L402): pane destroy는 출력 drain, fd 상태, exited/status-ready 상태를 확인한 뒤 수행한다.
- [`cmd-detach-client.c:25-115`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/cmd-detach-client.c#L25-L115)와 [`cmd-kill-pane.c:25-69`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/cmd-kill-pane.c#L25-L69): client detach와 pane kill은 서로 다른 명령과 코드 경로다.
- [`spawn.c:408-525`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/spawn.c#L408-L525): spawn 실패 정리는 방금 만들던 pane과 fd로 범위를 좁힌다.
- [`server.c:281-328`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/server.c#L281-L328)와 [`server-fn.c:437-486`](https://github.com/tmux/tmux/blob/31b0b0c99e39ced9a42fe3674b80f9eb0e009da7/server-fn.c#L437-L486): `exit-empty`, `exit-unattached`, session/window destroy는 tmux의 제품 정책이며, cokacmux의 "사용자가 죽이지 않은 백그라운드 프로세스 보존" 정책과는 분리해서 봐야 한다.

추가로 `sleep`, `usleep`, `nanosleep` 사용을 검색했을 때 프로세스 수명, server startup, attach retry 경로에서 고정 지연으로 race를 덮는 코드는 확인되지 않았다. 검색된 `tty.c`의 `usleep(100)`은 tty 출력 처리 경로이며 server readiness나 child lifetime 판단 경로가 아니다.

### 1. 서버가 child process의 소유자다

`tmux`에서 pane process는 client가 아니라 server가 관리한다. client는 server에 붙었다가 떨어질 수 있는 관찰자이자 입력 전달자이며, client detach는 pane child 종료를 의미하지 않는다.

`cokacmux`에서도 이 구분을 유지해야 한다.

- daemon은 PTY child의 실제 소유자다.
- TUI client 또는 right panel client를 drop/detach/release하는 것은 daemon child를 종료하는 의미가 아니다.
- child 종료는 명시적 kill, pane/session/server 종료처럼 별도 의도가 있는 경로에만 있어야 한다.

### 2. detach와 kill은 프로토콜과 코드 경로에서 분리한다

`tmux`는 `detach-client`와 `kill-pane`, `kill-session`, `kill-server`를 별도 명령과 별도 상태 전이로 다룬다. detach는 client exit 상태를 만들 뿐 pane process를 직접 죽이지 않는다.

`cokacmux`도 다음을 보장해야 한다.

- `Detach`, hide, release, restore 실패는 프로세스 종료와 연결되지 않는다.
- 명시적 kill 경로는 함수명, UI 확인, 로그, 테스트에서 드러나야 한다.
- "부모 pane이 없어졌다"는 사실만으로 보조 프로세스를 kill하지 않고, 먼저 standalone live process로 release한다.

### 3. child death는 관측된 이벤트로 처리한다

`tmux`는 `SIGCHLD`와 `waitpid` 결과로 child exit를 확정하고, exit status를 pane 상태에 반영한다. 단순히 파일, fd, UI entry, metadata가 사라졌다는 이유만으로 임의 프로세스를 죽이지 않는다.

`cokacmux`에서는 플랫폼별 차이 때문에 동일한 구현을 그대로 쓸 수는 없지만, 원칙은 같다.

- metadata는 단서이고 최종 진실이 아니다.
- 가능하면 socket reachability, process identity, pid start token, child pid start token을 함께 확인한다.
- 확인할 수 없는 상태는 "죽여도 됨"이 아니라 "보존하고 더 확인해야 함"으로 둔다.
- 종료 여부를 추정하는 주체와 실제 종료를 수행하는 주체를 분리하지 않는다. 가능한 한 daemon/runtime worker가 관측한 사실을 상태로 올리고, UI refresh나 stale cleanup은 그 상태를 소비만 한다.

### 4. 반쯤 만들어진 리소스는 좁은 범위에서 정리한다

`tmux`의 spawn 실패 경로는 아직 완성되지 않은 pane/cell/fd만 정리한다. 이 정리는 이미 성공적으로 실행 중인 다른 pane process를 건드리는 작업이 아니다.

`cokacmux`에서도 setup 실패 정리는 다음 범위로 제한한다.

- 이미 spawn된 바로 그 child만 정리한다.
- key, pid, start token 등으로 대상이 특정되지 않은 프로세스는 종료하지 않는다.
- 기존 live daemon이나 stale metadata의 child를 자동 cleanup에서 종료하지 않는다.

### 5. 종료 전 graceful wait는 kill 절차에만 둔다

`tmux`도 명시적 server/job 종료 경로에서는 `SIGTERM` 같은 종료 신호를 사용하지만, startup restore race를 고정 sleep으로 해결하지 않는다.

`cokacmux`의 시간 기반 처리는 다음처럼 제한한다.

- pending restore를 포기하는 timeout으로 쓰지 않는다.
- discovery가 느릴 때는 사용자 요청을 상태로 보존하고 slow log만 남긴다.
- 짧은 wait는 명시적 kill 이후 OS 리소스 정리나 파일 핸들 해제를 위한 절차적 대기일 때만 허용한다.

### 6. startup attach race는 lock/retry/state로 다룬다

`tmux` client는 server socket 연결이 실패했을 때 임의 sleep으로 server 준비를 추정하지 않는다. socket connect 실패, lock 획득, 재시도, server start를 상태 전이로 다루며, 다른 client가 server를 시작 중이면 lock 해제를 기다린 뒤 다시 connect를 시도한다.

`cokacmux`도 startup 직후 `Ctrl+]` restore를 다음 방식으로 처리해야 한다.

- runtime discovery가 진행 중이면 사용자 요청을 pending state로 보존한다.
- discovery 완료 이벤트가 도착하면 pending restore를 이어서 처리한다.
- "충분히 기다렸을 것"이라는 고정 delay로 restore 성공 여부를 결정하지 않는다.
- 동시에 여러 refresh가 발생하더라도 최신 요청 의도와 live discovery 필요 여부를 잃지 않는다.
- daemon start 경로는 tmux처럼 lock을 잡은 뒤 connect를 다시 시도해야 한다. lock은 준비 시간을 추정하는 delay가 아니라, 같은 key의 start 시도를 하나의 상태 전이로 직렬화하는 장치다.
- daemon start 이후 attach는 tmux의 `MSG_READY`처럼 명시적 readiness 신호를 기준으로 해야 한다. socket 파일이 생겼거나 일정 시간이 지났다는 사실만으로 attach 준비를 추정하지 않는다.

### 7. tmux의 종료 정책은 그대로 가져오지 않는다

`tmux` server는 `exit-empty`, `exit-unattached`, `kill-server`, session/window/pane destroy 같은 명시 정책에 따라 child process를 종료할 수 있다. 이것은 `tmux`의 제품 정책이지, `cokacmux`의 백그라운드 보존 요구사항과 동일하지 않다.

`cokacmux`가 참고해야 할 부분은 "server가 child를 소유한다"와 "detach와 kill을 분리한다"는 구조이지, unattached 상태를 자동 종료 조건으로 삼는 정책이 아니다.

### 8. tmux에서 그대로 답을 얻을 수 없는 부분

`tmux`의 프로세스 모델은 Unix server 중심이고, Windows Job Object, macOS의 낮은 해상도 process start time fallback, Linux pid 재사용 방지를 위한 `/proc` start ticks 보존 같은 cross-platform daemon identity 문제를 직접 해결해 주지는 않는다.

따라서 cokacmux는 tmux의 구조 원칙을 참고하되 다음 보강을 자체 요구사항으로 유지해야 한다.

- daemon pid와 child pid를 pid 단독으로 신뢰하지 않고 start token과 함께 검증한다.
- destructive kill/reset/killall 경로는 대상 pid를 발견한 시점의 검증만으로 충분하지 않다. 실제 종료 직전에도 같은 start token인지 다시 확인해야 하며, token이 없거나 달라진 경우에는 종료하지 않고 unverified로 보존한다.
- socket이 없거나 metadata가 어긋난 상태를 곧바로 dead로 확정하지 않는다.
- Windows에서는 Job Object에 묶인 부모에서 daemon이 떨어져 나올 수 없으면 백그라운드 보존을 보장했다고 말하지 않는다.
- macOS에서는 `proc_pidinfo(PROC_PIDTBSDINFO)`의 시작 시각과 process status, `sysctl(KERN_PROCARGS2)` argv를 우선 사용하고, 낮은 해상도 fallback이나 `ps`가 재구성한 command 문자열은 destructive/replacement 판단에 쓰지 않는다.
- tmux의 `exit-empty` 또는 `exit-unattached`에 해당하는 자동 종료 정책은 도입하지 않는다.

## 완료 전 남은 작업

아래 항목이 현재 목표를 완료했다고 말하기 전에 남아 있는 작업이다. 각 항목은 구현 여부가 아니라, 현재 코드와 검증 증거로 이 문서의 계약을 입증할 수 있는지를 기준으로 판단한다.

### 1. Windows 프로세스 식별 경로 검증과 보강

Windows에서는 runtime metadata/socket 파일이 없거나 손상된 상태에서도 기존 daemon을 놓치면 안 된다. 새 daemon 시작 전 OS process table에서 같은 `--agent-daemon <provider> <session_id>`를 가진 프로세스를 찾아 기존 백그라운드 프로세스 교체를 막을 수 있어야 한다.

완료 조건:

- PowerShell, WMI, 사람이 읽는 command string 재구성에만 의존하지 않는지 확인한다.
- Windows에서 process creation FILETIME start token을 destructive/replacement 판단 직전까지 유지하고 재확인한다.
- 부모 프로세스가 Windows Job Object에 묶여 있을 때 daemon breakaway가 실패하면 백그라운드 보존을 보장했다고 취급하지 않는다.
- runtime 파일이 사라진 live daemon을 새 daemon으로 덮어쓰지 않는 회귀 테스트 또는 동등한 검증을 갖춘다.

### 2. macOS 프로세스 식별 경로 검증

macOS에서는 `ps`가 재구성한 문자열이나 낮은 해상도 시작 시각 fallback을 destructive/replacement 판단의 근거로 삼으면 안 된다.

완료 조건:

- daemon argv 식별은 `sysctl(KERN_PROCARGS2)` 기반 경로가 우선 사용됨을 확인한다.
- pid start token과 zombie/process status 판단은 `proc_pidinfo(PROC_PIDTBSDINFO)` 기반 경로가 우선 사용됨을 확인한다.
- macOS target 또는 실제 macOS 환경에서 빌드와 관련 테스트를 통과시킨다.
- macOS에서 token unknown 또는 argv unknown은 kill/delete/replace 허가가 아니라 preserve/diagnose 상태로 이어진다.

### 3. cross-platform 빌드와 테스트 증거 확보

이 문서의 목표는 Linux만의 목표가 아니다. Linux, macOS, Windows에서 같은 생명주기 계약이 유지되어야 한다.

완료 조건:

- Linux native `cargo check --bin cokacmux`와 `cargo test --bin cokacmux`를 통과한다.
- macOS target 또는 실제 macOS에서 `cargo check --bin cokacmux`를 통과한다.
- Windows target 또는 실제 Windows에서 `cargo check --bin cokacmux`를 통과한다.
- platform-specific `cfg` 경로가 테스트 없이 컴파일만 되는 상태에 머물지 않도록, 최소한 parser/identity/termination guard 단위 테스트를 각 플랫폼별로 둔다.

### 4. 실제 사용자 시나리오 회귀 테스트

사용자가 제기한 증상은 단순 단위 함수 문제가 아니라 startup, discovery, attach, UI focus 상태가 이어진 시나리오 문제다.

완료 조건:

- `cokacmux` 시작 직후 live discovery가 끝나기 전에 `Ctrl+]`가 들어와도 pending restore가 보존되고 discovery 완료 후 복원된다.
- 장시간 detached 상태 뒤 재실행했을 때 살아 있는 daemon이 sidebar와 attach/focus 경로 양쪽에서 일관되게 보인다.
- 새 터미널을 열어야만 과거 agent가 뒤늦게 보이는 상태가 재현되지 않는다.
- attach 단계 실패가 daemon death로 단정되지 않고 live discovery refresh로 이어진다.
- parent pane 또는 auxiliary pane 정리 과정에서 child process가 kill되지 않고 release/preserve된다.

### 5. destructive 경로 전수 감사

프로세스를 실제로 종료하거나 runtime 파일을 삭제하는 모든 경로는 별도로 감사해야 한다. 함수 이름이 cleanup/reset/refresh라 해도 내부에서 kill/delete가 일어나면 이 계약의 대상이다.

완료 조건:

- `kill`, `reset`, `killall`, stale cleanup, cwd lock cleanup, orphan runtime cleanup에서 process identity와 start token을 종료 직전에 재확인한다.
- start token을 읽지 못한 상태는 destructive 작업 허가가 아니라 unverified preserve로 이어진다.
- socket 또는 metadata 누락만으로 daemon/child를 dead로 확정하지 않는다.
- runtime 파일 삭제 전에 같은 stem의 reachable daemon 또는 identity 응답을 확인한다.
- 자동 cleanup은 child process 종료 대신 release/preserve를 우선한다.

### 6. 시간차 처리 감사

startup/restore/liveness/readiness 문제를 임의 지연으로 숨기면 목표를 만족하지 못한다.

완료 조건:

- restore 요청은 timeout 때문에 소비되지 않는다.
- readiness는 daemon이 보내는 명시 신호 또는 상태 전이로 판단한다.
- liveness는 process identity, socket reachability, start token, daemon identity 같은 관측 가능한 사실로 판단한다.
- `sleep`, `timeout`, `poll interval`, `debounce` 값은 readiness/liveness 추정값이 아니라 I/O 상한, 명시 kill 유예, diagnostics, housekeeping cadence 중 하나로 설명 가능해야 한다.
- 근거 없는 magic number delay가 발견되면 제거하거나 상태 기반 전이로 바꾼다.

## 비목표

이 문서는 다음을 요구하지 않는다.

- OS reboot 이후에도 프로세스가 살아 있어야 함
- 사용자가 직접 kill한 프로세스를 되살려야 함
- 에이전트 바이너리 자체 crash를 `cokacmux`가 완전히 방지해야 함
- 모든 외부 터미널 정책을 우회해야 함
- 죽은 프로세스를 자동으로 새 세션으로 재생성해야 함

## 요약

사용자가 원하는 보장은 단순한 편의 기능이 아니라 `cokacmux`의 프로세스 생명주기 계약이다.

`cokacmux`가 백그라운드 실행을 제공한다면, 사용자가 명시적으로 종료하지 않은 프로세스는 `cokacmux`의 내부 상태 갱신, 정리, 복원, 재시작 과정 때문에 죽거나 접근 불가능해져서는 안 된다.
