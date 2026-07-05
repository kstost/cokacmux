# cokacmux CLI

이 문서는 TUI 밖에서 `cokacmux`를 실행할 때의 명령을 정리합니다.

## 기본 명령

| 명령 | 용도 |
|---|---|
| `cokacmux` | TUI를 엽니다. |
| `cokacmux --check` | 세션 탐색이 되는지 headless로 확인합니다. |
| `cokacmux killall` | 실행 중인 cokacmux daemon과 runtime 파일을 정리합니다. |
| `cokacmux agents killall` | `killall`과 같은 agent runtime 정리 명령입니다. |
| `cokacmux reset` | `~/.cokacmux` 설정과 runtime을 함께 정리합니다. |
| `cokacmux start <name> -- <command...>` | 명령을 managed terminal로 background에서 시작합니다. |

`--debug`와 `--trace`는 전역 옵션입니다. 예를 들어 `cokacmux --debug start web -- node server.js`처럼 쓸 수 있습니다.

## 용어를 쉽게 말하면

| 용어 | 쉬운 뜻 |
|---|---|
| terminal | 명령을 실행하는 화면입니다. |
| background terminal | 화면을 계속 보고 있지 않아도 뒤에서 살아 있는 terminal입니다. |
| daemon | terminal을 계속 붙잡고 있는 작은 cokacmux 프로세스입니다. |
| agents 목록 | 지금 다시 붙을 수 있는 실행 중 화면 목록입니다. |
| runtime 파일 | cokacmux가 실행 중인 terminal을 다시 찾기 위해 잠깐 저장하는 상태 파일입니다. |

## `start`

`start`는 TUI에서 `Ctrl+N`으로 terminal을 여는 것과 같은 계열의 기능입니다. 다만 사용자가 직접 shell을 열어 명령을 입력하는 대신, CLI가 명령을 바로 실행한 terminal daemon을 만듭니다.

쉽게 말하면, 개발 서버나 watch 작업을 cokacmux 안에 켜 두는 명령입니다. 명령을 시작한 뒤에는 shell로 바로 돌아오고, 실행 중인 화면은 나중에 TUI에서 다시 볼 수 있습니다.

여기서 shell로 돌아온다는 말은 명령이 끝났다는 뜻이 아닙니다. 명령은 cokacmux terminal 안에서 계속 돌고 있고, 사용자는 원래 shell에서 다른 일을 할 수 있다는 뜻입니다.

```bash
cokacmux start web --cwd /path/to/project -- node server.js
```

형식은 다음과 같습니다.

```text
cokacmux start [--cwd <PATH>] <NAME> -- <COMMAND> [ARGS...]
```

| 항목 | 설명 |
|---|---|
| `<NAME>` | agents 목록에 표시할 terminal 이름입니다. 예: `web`, `api`, `vite` |
| `--cwd <PATH>` | 명령을 실행할 폴더입니다. 생략하면 현재 폴더입니다. |
| `--` | cokacmux 옵션과 실행할 명령을 나누는 구분자입니다. 필수입니다. |
| `<COMMAND> [ARGS...]` | 실제 실행할 프로그램과 인자입니다. |

실행 흐름은 다음과 같습니다.

1. `cwd` 폴더가 있는지 확인합니다.
2. `COMMAND`가 실행 가능한 프로그램인지 확인합니다.
3. cokacmux daemon을 백그라운드로 띄웁니다.
4. daemon이 terminal을 만들고 그 안에서 명령을 실행합니다.
5. 준비가 끝나면 `cokacmux start` 명령은 바로 끝납니다.
6. 실행 중인 terminal은 TUI의 agents 목록에서 다시 열 수 있습니다.

예시:

```bash
cokacmux start web -- npm run dev
cokacmux start api --cwd ~/work/app -- node server.js
cokacmux start worker --cwd /srv/app -- bash -lc 'source .env && npm run worker'
```

`start`로 만든 항목은 기존 live agents 목록에 `terminal`로 들어갑니다. TUI에서 `Ctrl+]` 또는 `Ctrl+[`로 agents 목록에 돌아오면 이름으로 확인하고 다시 붙을 수 있습니다.

### `--`가 필요한 이유

`--` 앞은 cokacmux가 읽는 옵션입니다. `--` 뒤는 실제로 실행할 명령입니다.

```bash
cokacmux start web --cwd ~/app -- npm run dev
```

위 명령에서 `--cwd ~/app`은 cokacmux 옵션이고, `npm run dev`는 terminal 안에서 실행할 명령입니다.

`--`를 빼면 cokacmux가 어디까지 자기 옵션이고 어디부터 실행할 명령인지 알 수 없어서 거부합니다.

## Windows에서 쓰기

Windows에서도 같은 형식으로 씁니다.

```powershell
cokacmux start web --cwd C:\work\app -- npm run dev
cokacmux start api --cwd C:\work\app -- node server.js
cokacmux start worker -- powershell.exe -NoLogo -NoProfile -Command "npm run worker"
```

Windows에서는 `PATH`와 `PATHEXT` 규칙을 따릅니다. 그래서 `npm`, `pnpm`, `yarn`처럼 실제 파일이 `npm.cmd`인 명령도 그대로 쓸 수 있습니다.

```powershell
cokacmux start web -- npm run dev
```

프로그램이 공백이 들어간 폴더에 있어도 실행할 수 있습니다. 예를 들어 `C:\Tools With Spaces\npm.cmd` 같은 경로도 Windows 규칙에 맞게 처리합니다.

PowerShell 문법이 필요한 명령은 `powershell.exe -Command`로 감싸세요.

```powershell
cokacmux start logs -- powershell.exe -NoLogo -NoProfile -Command "Get-Content .\server.log -Wait"
```

`dir`, `cd`, `set`처럼 shell 안에서만 의미가 있는 명령은 직접 실행 파일이 아닙니다. 이런 명령은 `cmd.exe /C` 또는 `powershell.exe -Command`로 감싸야 합니다.

```powershell
cokacmux start list -- cmd.exe /C dir
cokacmux start setup -- powershell.exe -NoProfile -Command "cd C:\work\app; npm run dev"
```

## 실행 뒤 확인하기

실행이 잘 되었는지 확인하는 방법은 세 가지입니다.

| 방법 | 설명 |
|---|---|
| TUI 열기 | `cokacmux`를 실행한 뒤 agents 목록에서 이름을 확인합니다. |
| `--check` | `cokacmux --check`로 세션 탐색이 되는지 빠르게 확인합니다. |
| `killall` | 테스트로 띄운 terminal을 모두 정리할 때 `cokacmux killall`을 씁니다. |

`cokacmux start`가 성공하면 보통 다음처럼 한 줄을 출력합니다.

```text
started terminal web: session=... cwd=... command=...
```

이 줄이 보이면 terminal daemon이 준비되었다는 뜻입니다. 명령 출력은 이 shell에 계속 찍히지 않고, TUI로 다시 붙었을 때 볼 수 있습니다.

## 안전 규칙

`start`는 실행 전에 아래를 확인합니다.

| 검사 | 동작 |
|---|---|
| name | 비어 있거나 control character가 있으면 거부합니다. |
| cwd | 존재하는 폴더여야 합니다. CLI에서는 없는 폴더를 자동 생성하지 않습니다. |
| command | `--` 뒤에 최소 1개 인자가 있어야 합니다. |
| program | 실행 가능한 프로그램이어야 합니다. PATH 또는 platform별 실행 파일 규칙을 따릅니다. |
| metadata | command metadata가 너무 길거나 깨졌을 때 provider resume으로 잘못 떨어지지 않고 terminal error로 종료합니다. |

이 검사는 사용자가 입력한 명령 자체의 의미를 검증하지 않습니다. 예를 들어 `rm -rf ...` 같은 명령은 사용자가 직접 실행한 것과 같은 권한으로 실행됩니다.

## 오류 메시지 읽기

자주 볼 수 있는 오류는 다음과 같습니다.

| 메시지 | 뜻 | 해결 |
|---|---|---|
| `command program is not runnable or not found` | 실행할 프로그램을 찾지 못했거나 실행할 수 없습니다. | 프로그램 이름을 확인하고, PATH에 들어 있는지 확인합니다. Windows에서는 `.cmd`나 `.exe`가 있는지도 확인합니다. |
| `launch folder does not exist` | `--cwd`로 준 폴더가 없습니다. | 폴더를 만들거나, 올바른 경로로 다시 실행합니다. |
| `--` 관련 오류 | 실행할 명령을 구분하지 못했습니다. | `cokacmux start 이름 -- 실제명령` 형태로 다시 씁니다. |
| name 관련 오류 | terminal 이름이 비었거나 제어 문자가 들어 있습니다. | `web`, `api`, `worker`처럼 짧고 단순한 이름을 씁니다. |

## 용도와 제한

적합한 용도:

| 용도 | 예시 |
|---|---|
| 개발 서버 유지 | `node server.js`, `npm run dev`, `vite --host` |
| watch 작업 유지 | `cargo watch -x test`, `npm run watch` |
| worker 유지 | `bash -lc 'source .env && node worker.js'` |

덜 적합한 용도:

| 용도 | 이유 |
|---|---|
| 짧은 일회성 명령 | 실행 직후 daemon이 종료되어 TUI에서 붙을 시간이 거의 없습니다. |
| 출력만 보고 끝나는 명령 | 일반 shell에서 직접 실행하는 편이 단순합니다. |
| shell builtin 직접 실행 | `cd`, `source` 같은 builtin은 `bash -lc '...'` 형태로 감싸야 합니다. |

짧게 끝나는 명령을 실행해야 한다면 일반 shell에서 직접 실행하는 편이 낫습니다.

```bash
# cokacmux start보다 일반 shell에 더 어울립니다.
npm test
cargo test
echo hello
```

## 프로세스 수명

`cokacmux start`는 명령을 직접 background로 버리는 것이 아니라, cokacmux daemon이 PTY를 만들고 그 안에서 명령을 실행합니다. 그래서 TUI를 꺼도 명령은 계속 살아 있을 수 있고, 나중에 agents 목록에서 다시 붙을 수 있습니다.

명령이 끝나면 daemon은 runtime metadata와 socket을 정리합니다. 일반 macOS/Linux/Windows 환경에서는 종료된 daemon 프로세스도 OS의 init/system service가 회수합니다.

다만 일부 container나 sandbox처럼 PID 1이 child process를 회수하지 않는 환경에서는, 아주 짧게 끝나는 command daemon이 `defunct` 상태로 보일 수 있습니다. 이 상태는 이미 종료된 프로세스라 CPU를 쓰거나 명령이 계속 실행되는 것은 아니지만, process table entry는 남을 수 있습니다. 이런 환경에서는 `start`를 `echo`, `sleep 1` 같은 일회성 명령에 반복 사용하지 말고, 오래 유지할 서버/worker에 쓰는 편이 안전합니다.

## 아직 없는 CLI 관리 명령

현재 구현된 새 CLI 표면은 `start`입니다. TUI에서 가능한 조작을 CLI로 모두 노출한 상태는 아닙니다.

아직 별도 명령으로 제공하지 않는 항목:

| 기능 | 현재 방법 |
|---|---|
| live terminal 목록 조회 | TUI의 agents 목록에서 확인 |
| 개별 terminal stop | TUI에서 해당 terminal에 붙은 뒤 `Ctrl+K` |
| terminal log 조회 | TUI로 attach해서 확인 |
| 이름 변경 | TUI의 title edit 흐름 사용 |

전체 runtime을 정리해야 하면 `cokacmux killall`을 사용합니다.

## 빠른 점검 예시

아래 순서로 실행하면 `start`가 내 환경에서 잘 되는지 빠르게 볼 수 있습니다.

macOS/Linux:

```bash
cokacmux start demo -- sh -c 'while true; do date; sleep 5; done'
cokacmux --check
cokacmux killall
```

Windows PowerShell:

```powershell
cokacmux start demo -- powershell.exe -NoLogo -NoProfile -Command "while ($true) { Get-Date; Start-Sleep -Seconds 5 }"
cokacmux --check
cokacmux killall
```

첫 번째 명령은 계속 살아 있는 terminal을 만듭니다. 두 번째 명령은 cokacmux가 그 terminal을 찾을 수 있는지 확인합니다. 마지막 명령은 테스트로 띄운 것을 정리합니다.
