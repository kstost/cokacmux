# cokacmux

> Claude Code, Codex, OpenCode 세션을 한 화면에서 보고, 찾고, 복제하고, 다시 실행하는 터미널 TUI 도구

Claude Code, Codex, OpenCode는 모두 훌륭한 코딩 에이전트이지만 세션 저장 위치와 형식이 서로 다릅니다. 그래서 며칠 전에 어떤 에이전트로 어떤 작업을 했는지 찾거나, 저장된 대화를 이어 실행하거나, 같은 대화에서 다른 방향으로 실험하기가 번거롭습니다.

`cokacmux`는 이 세 에이전트의 대화 기록을 한곳에 모아 보여주고, 키보드만으로 미리보기, 검색, 재실행, 새 에이전트 시작, 복제, 삭제, 백그라운드 전환을 할 수 있게 해주는 앱입니다.

별도 서버나 API를 쓰지 않습니다. 세션 데이터는 내 컴퓨터 안에서 읽고 쓰며, 실제 코딩 작업은 사용자가 이미 설치해 둔 Claude Code, Codex, OpenCode CLI를 실행해서 처리합니다.

---

## 목차

1. [먼저 알아둘 개념](#1-먼저-알아둘-개념)
2. [이 앱이 해주는 일](#2-이-앱이-해주는-일)
3. [빠른 시작](#3-빠른-시작)
4. [설치하기](#4-설치하기)
5. [처음 실행해 보기](#5-처음-실행해-보기)
6. [기본 사용 흐름](#6-기본-사용-흐름)
7. [키보드 단축키](#7-키보드-단축키)
8. [CLI 명령어](#8-cli-명령어)
9. [설정 파일](#9-설정-파일)
10. [단축키 설정](#10-단축키-설정)
11. [데이터 저장 위치](#11-데이터-저장-위치)
12. [자주 묻는 질문](#12-자주-묻는-질문)
13. [문제가 생겼을 때](#13-문제가-생겼을-때)
14. [업데이트와 제거](#14-업데이트와-제거)
15. [소스에서 빌드하기](#15-소스에서-빌드하기)
16. [작동 원리](#16-작동-원리)

---

## 1. 먼저 알아둘 개념

처음 쓰는 분은 아래 용어만 이해하면 됩니다.

| 용어 | 뜻 |
|---|---|
| 세션 | Claude Code, Codex, OpenCode가 저장한 대화 기록입니다. 예전 작업 내용입니다. |
| 미리보기 | 저장된 세션 파일을 읽어서 오른쪽 창에 보여주는 기능입니다. 에이전트를 실행하지 않습니다. |
| 에이전트 | 실제로 실행 중인 Claude Code, Codex, OpenCode CLI 프로세스입니다. |
| 백그라운드 에이전트 | `Ctrl+]`로 목록 화면에 돌아와도 종료되지 않고 계속 살아있는 에이전트입니다. |
| 작업 폴더 | 에이전트를 시작할 폴더입니다. 코딩 작업의 기준 디렉터리입니다. |
| PATH | 터미널에서 `codex`, `claude`, `opencode` 같은 명령을 찾는 운영체제의 검색 경로입니다. |
| launch mode | 에이전트를 일반 모드로 시작할지, 권한 확인 우회 옵션을 붙일지 고르는 모드입니다. |
| Skip permissions | 권한 확인을 우회하는 위험한 실행 모드입니다. 신뢰하는 폴더에서만 사용하세요. |

중요한 차이가 하나 있습니다.

- `Delete` 또는 `d`: 저장된 세션 기록을 삭제합니다.
- `Ctrl+K`: 실행 중인 백그라운드 에이전트 프로세스를 종료합니다.

기록을 지우는 것과 실행 중인 프로세스를 끄는 것은 다릅니다.

---

## 2. 이 앱이 해주는 일

### 세션을 한 화면에 모아 보기

Claude Code, Codex, OpenCode가 각자 다른 위치에 저장한 대화를 하나의 세션 목록으로 보여줍니다.

### 대화 미리보기

세션을 선택하면 오른쪽 창에서 내용을 바로 볼 수 있습니다. 단순히 저장 파일을 읽는 동작이라 에이전트 CLI가 실행되지 않습니다.

### 전체 검색

`/` 키로 검색창을 열 수 있습니다. 검색 대상은 제목뿐 아니라 세션 ID, 작업 폴더, 타이틀, 세션 본문 전체입니다. 검색 중에는 UI가 멈춘 것처럼 보이지 않도록 검색 상태가 표시됩니다.

### 저장된 세션 이어 실행

세션을 고르고 `e`를 누르면 해당 에이전트를 다시 실행해 대화를 이어갈 수 있습니다. 이미 cokacmux가 띄운 같은 에이전트가 살아 있으면 새로 실행하지 않고 다시 붙습니다.

### 새 터미널 또는 새 코딩 에이전트 시작

`Ctrl+N`을 누르면 새 세션 모달이 열립니다. 여기서 그냥 터미널을 열지, Claude Code/Codex/OpenCode를 새 작업 폴더에서 시작할지 고를 수 있습니다. 입력한 폴더가 없으면 시작 전에 자동으로 만듭니다.

### 백그라운드 유지와 전환

에이전트를 실행한 뒤 `Ctrl+]` 또는 `Ctrl+[`를 누르면 세션 목록으로 돌아옵니다. 에이전트는 종료되지 않고 백그라운드에서 계속 살아 있습니다. 다시 `Ctrl+]` 또는 같은 세션에서 `e`를 누르면 다시 연결됩니다.

### 세션 복제와 변환

`c`를 누르면 선택한 세션을 복제합니다. 복제 대상은 Claude, Codex, OpenCode 중에서 고를 수 있습니다. 같은 에이전트 안에서 복제할 수도 있고, 다른 에이전트 형식으로 변환해 새 세션으로 만들 수도 있습니다.

### 세션 삭제

`Delete` 또는 `d`를 누르면 확인창을 띄운 뒤 세션 기록을 삭제합니다. 바로 삭제하지 않습니다.

### 추가 요금 없음

cokacmux는 어떤 AI API도 직접 호출하지 않습니다. 이미 설치해 둔 에이전트 CLI를 실행할 뿐입니다. 비용과 사용량은 각 에이전트의 기존 구독/정책을 따릅니다.

---

## 3. 빠른 시작

이미 Claude Code, Codex, OpenCode 중 하나를 사용하고 있다면 다음 순서만 따라 하면 됩니다.

### 1단계: 설치

macOS/Linux:

```bash
curl -fsSL https://cokacmux.cokac.com/manage.sh | bash
```

Windows PowerShell:

```powershell
irm https://cokacmux.cokac.com/manage.ps1 | iex
```

### 2단계: 설치 확인

```bash
cokacmux --version
```

### 3단계: 사용하는 에이전트 CLI 확인

사용하는 것만 확인하면 됩니다. 셋을 모두 설치할 필요는 없습니다.

```bash
claude --version
codex --version
opencode --version
```

### 4단계: 실행

```bash
cokacmux
```

### 5단계: 기본 조작

| 하고 싶은 일 | 키 |
|---|---|
| 세션 선택 이동 | `↑`/`↓` 또는 `j`/`k` |
| 오른쪽 미리보기로 포커스 이동 | `Tab` |
| 미리보기 요약/전체 전환 | `Enter` |
| 검색 | `/` |
| 선택한 세션 이어 실행 | `e` |
| 새 터미널/새 코딩 에이전트 시작 | `Ctrl+N` |
| 에이전트 화면에서 목록으로 돌아오기 | `Ctrl+]` 또는 `Ctrl+[` |
| 세션 복제 | `c` |
| 세션 삭제 | `Delete` 또는 `d` |
| 종료 | `q` 또는 `Ctrl+Q` |

---

## 4. 설치하기

### macOS / Linux

터미널에 다음 명령을 붙여넣으세요.

```bash
curl -fsSL https://cokacmux.cokac.com/manage.sh | bash
```

설치 스크립트는 운영체제와 CPU 종류에 맞는 바이너리를 내려받아 `/usr/local/bin` 또는 `~/.local/bin`에 설치합니다. 설치 후 새 터미널에서 `cokacmux` 명령을 사용할 수 있습니다.

스크립트를 먼저 보고 싶다면 이렇게 확인한 뒤 실행하세요.

```bash
curl -fsSL https://cokacmux.cokac.com/manage.sh -o manage.sh
less manage.sh
bash manage.sh
```

### Windows

PowerShell을 열고 다음 명령을 실행하세요. 관리자 권한은 필요 없습니다.

```powershell
irm https://cokacmux.cokac.com/manage.ps1 | iex
```

기본 설치 위치는 `%LOCALAPPDATA%\cokacmux\`입니다. PATH가 갱신되므로 설치 후 PowerShell을 한 번 닫았다가 다시 여는 편이 안전합니다.

스크립트를 먼저 보고 싶다면 이렇게 확인한 뒤 실행하세요.

```powershell
iwr https://cokacmux.cokac.com/manage.ps1 -OutFile manage.ps1
notepad .\manage.ps1
powershell -ExecutionPolicy Bypass -File .\manage.ps1
```

### 잘 설치됐는지 확인

```bash
cokacmux --version
```

버전 번호가 출력되면 설치된 것입니다.

### 에이전트 CLI 준비

cokacmux는 저장된 세션을 미리보기할 수 있지만, `e`나 `Ctrl+N`으로 실제 에이전트를 실행하려면 해당 CLI가 설치되어 있고 로그인도 되어 있어야 합니다.

```bash
claude --version
codex --version
opencode --version
```

셋을 모두 설치할 필요는 없습니다. 본인이 쓰는 에이전트만 실행 가능하면 됩니다.

### 실행 파일 위치를 직접 지정해야 할 때

기본적으로 cokacmux는 `claude`, `codex`, `opencode`라는 명령 이름을 그대로 실행합니다. 실제 위치는 운영체제의 PATH 해석에 맡깁니다.

macOS/Linux에서 현재 잡힌 위치 확인:

```bash
command -v claude
command -v codex
command -v opencode
```

Windows PowerShell에서 현재 잡힌 위치 확인:

```powershell
Get-Command claude
Get-Command codex
Get-Command opencode
```

PATH에 여러 버전이 잡혀 있거나 특정 설치본을 꼭 쓰고 싶다면 `~/.cokacmux/settings.json`의 `agent_programs`에 직접 적을 수 있습니다.

```json
{
  "cokacmux": {
    "agent_programs": {
      "claude": "~/.local/bin/claude",
      "codex": "/usr/bin/codex",
      "opencode": "~/.opencode/bin/opencode"
    }
  }
}
```

Windows 경로는 JSON 규칙에 맞게 `\\`로 쓰거나 `/`를 사용할 수 있습니다.

```json
{
  "cokacmux": {
    "agent_programs": {
      "claude": "C:\\Users\\me\\.local\\bin\\claude.exe",
      "codex": "C:/Users/me/AppData/Roaming/npm/codex.cmd",
      "opencode": "C:/Users/me/.opencode/bin/opencode.exe"
    }
  }
}
```

주의할 점은 여기에 실행 옵션을 넣지 않는 것입니다. 아래처럼 쓰면 안 됩니다.

```json
{
  "cokacmux": {
    "agent_programs": {
      "codex": "/usr/bin/codex --yolo"
    }
  }
}
```

올바른 값은 실행 파일 경로 또는 명령 이름까지만입니다.

```json
{
  "cokacmux": {
    "agent_programs": {
      "codex": "/usr/bin/codex"
    }
  }
}
```

`--yolo`, `--dangerously-skip-permissions`, OpenCode 권한 환경변수 같은 옵션은 `e` 또는 `Ctrl+N`에서 Skip permissions를 선택하면 cokacmux가 자동으로 붙입니다.

---

## 5. 처음 실행해 보기

터미널에서 실행합니다.

```bash
cokacmux
```

처음 실행하면 `~/.cokacmux/` 디렉터리와 기본 설정 파일이 자동으로 만들어집니다. 그 다음 cokacmux가 각 에이전트의 기본 저장 위치를 찾아 세션 목록을 만듭니다.

| 에이전트 | 기본 저장 위치 |
|---|---|
| Claude Code | `~/.claude/projects/...` |
| Codex | `~/.codex/sessions/...` |
| OpenCode | macOS/Linux: `~/.local/share/opencode/opencode.db` |
| OpenCode | Windows: `%LOCALAPPDATA%\opencode\opencode.db` |

한 번도 쓰지 않은 에이전트가 있어도 괜찮습니다. 해당 에이전트 세션만 비어 있을 뿐입니다.

### 화면 구성

```text
┌──────────────────────────────┬──────────────────────────────┐
│ 세션 목록                    │ 선택한 세션 미리보기         │
│                              │                              │
│  > 오늘 작업한 Codex 세션    │ user: 이 문제를 고쳐줘       │
│    어제 Claude 세션          │ assistant: 우선 ...          │
│    OpenCode 실험 세션        │ user: 좋아요                 │
│                              │                              │
└──────────────────────────────┴──────────────────────────────┘
```

왼쪽은 세션 목록이고, 오른쪽은 선택한 세션의 미리보기입니다. 화면 아래에는 현재 쓸 수 있는 주요 단축키가 짧게 표시됩니다.

에이전트를 실행하면 화면이 에이전트 터미널로 바뀝니다. 이때 왼쪽에는 실행 중인 에이전트/셸 사이드바가 보일 수 있습니다. `Ctrl+B`로 숨기거나 다시 보일 수 있습니다.

---

## 6. 기본 사용 흐름

### 6-1. 세션 둘러보기

`↑`/`↓` 또는 `j`/`k`로 세션을 고릅니다. 미리보기는 선택이 바뀔 때 자동으로 갱신됩니다.

빠르게 이동하려면:

| 이동 | 키 |
|---|---|
| 10칸 위/아래 | `PageUp` / `PageDown` |
| 맨 위 | `Home` 또는 `g` |
| 맨 아래 | `End` 또는 `G` |

맥북에는 전용 `PageUp`, `PageDown`, `Home`, `End` 키가 없는 경우가 많습니다. 보통은 다음 조합을 씁니다.

| 맥북 입력 | 의미 |
|---|---|
| `fn+↑` | `PageUp` |
| `fn+↓` | `PageDown` |
| `fn+←` | `Home` |
| `fn+→` | `End` |

터미널 설정에 따라 일부 조합이 앱까지 전달되지 않을 수 있습니다. 그런 경우 [단축키 설정](#10-단축키-설정)으로 다른 키를 지정하세요.

### 6-2. 미리보기 읽기

기본 포커스는 세션 목록에 있습니다. 오른쪽 미리보기를 스크롤하려면 `Tab` 또는 `Esc`로 포커스를 옮깁니다.

미리보기 포커스에서:

| 동작 | 키 |
|---|---|
| 위/아래 스크롤 | `↑`/`↓` 또는 `j`/`k` |
| 한 페이지 위/아래 | `PageUp` / `PageDown` |
| 처음/끝 | `Home` / `End` 또는 `g` / `G` |
| 요약/전체 보기 전환 | `Enter` |
| 목록으로 돌아가기 | `Tab` 또는 `Esc` |

`Space`를 누르면 미리보기 캐시를 무시하고 다시 읽습니다.

### 6-3. 검색하기

`/`를 누르면 검색창이 열립니다. 검색어를 입력하고 `Enter`를 누르면 Search가 실행됩니다.

검색 대상:

- 세션 ID
- 작업 폴더 경로
- 세션 타이틀
- 대화 본문 전체

검색 중에는 버튼이 Searching 상태로 바뀌며 스피너가 표시됩니다. 긴 세션이 많아도 검색 작업은 백그라운드에서 진행됩니다. 취소하려면 `Esc`를 누르세요.

검색 결과를 지우고 전체 목록으로 돌아가려면 빈 검색어로 Search를 실행하면 됩니다.

### 6-4. 세션 이어 실행하기

세션을 선택하고 `e`를 누르면 Agent launch 모달이 열립니다.

| 모드 | 의미 |
|---|---|
| Normal | 일반 실행입니다. 에이전트가 평소처럼 권한 확인을 요청할 수 있습니다. |
| Skip permissions (danger) | 권한 확인을 우회하는 옵션을 붙입니다. 신뢰하는 폴더에서만 사용하세요. |

모달에서 `↑`/`↓` 또는 `j`/`k`로 선택하고 `Enter`로 시작합니다. `1`은 Normal, `2`는 Skip permissions를 바로 선택합니다. `Esc`는 취소입니다.

이미 같은 세션의 백그라운드 에이전트가 살아 있으면 새로 실행하지 않고 다시 연결합니다. 이 경우 launch mode는 새 프로세스를 시작하지 않으므로 실질적으로 영향을 주지 않습니다.

Skip permissions를 선택하면 이어 실행할 때 다음 형태가 됩니다.

| 에이전트 | 실행 형태 |
|---|---|
| Claude Code | `claude --dangerously-skip-permissions --resume <session-id>` |
| Codex | `codex --yolo resume -C <cwd> <session-id>` |
| OpenCode | `OPENCODE_PERMISSION='{"*":"allow"}' opencode <cwd> --session <session-id>` |

### 6-5. 에이전트 화면에서 목록으로 돌아오기

에이전트가 실행된 화면에서 `Ctrl+]` 또는 `Ctrl+[`를 누르면 세션 목록 화면으로 돌아옵니다. 에이전트는 종료되지 않습니다.

다시 에이전트로 돌아가려면:

- `Ctrl+]` 또는 `Ctrl+[`를 다시 누릅니다.
- 또는 세션 목록에서 같은 세션을 선택하고 `e`를 누릅니다.

세션 목록으로 돌아올 때 cokacmux는 디스크의 세션 목록과 실행 상태를 다시 읽어 최신 상태로 맞춥니다.

### 6-6. 새 터미널 또는 새 코딩 에이전트 시작

`Ctrl+N`을 누르면 New session 모달이 열립니다.

설정할 수 있는 값:

| 항목 | 설명 |
|---|---|
| Type | `Terminal` 또는 `Coding agent` |
| Folder | 시작할 작업 폴더 |
| Agent | Coding agent일 때 `claude`, `codex`, `opencode` 중 선택 |
| Permissions | Coding agent일 때 Normal 또는 Skip permissions 선택 |

`Folder`에 없는 경로를 입력하면 시작 전에 자동으로 생성합니다.

Terminal을 선택하면 해당 폴더에서 일반 셸을 엽니다. Coding agent를 선택하면 해당 폴더에서 새 Claude/Codex/OpenCode 세션을 시작합니다.

새 코딩 에이전트를 Skip permissions로 시작하면 다음 형태가 됩니다.

| 에이전트 | 실행 형태 |
|---|---|
| Claude Code | `claude --dangerously-skip-permissions` |
| Codex | `codex --yolo -C <cwd>` |
| OpenCode | `OPENCODE_PERMISSION='{"*":"allow"}' opencode <cwd>` |

### 6-7. 실행 중인 에이전트 전환과 종료

에이전트 화면에서:

| 동작 | 키 |
|---|---|
| 세션 목록으로 돌아가기 | `Ctrl+]` 또는 `Ctrl+[` |
| 현재 에이전트/셸 종료 | `Ctrl+K` |
| 이전/다음 실행 중인 대상 전환 | `Ctrl+PageUp` / `Ctrl+PageDown` |
| 사이드바 보이기/숨기기 | `Ctrl+B` |
| 사이드바 선택 이동 | `Alt+↑` / `Alt+↓` 또는 `Ctrl+Shift+↑` / `Ctrl+Shift+↓` |
| 사이드바 폭 조절 | `Alt+←` / `Alt+→` 또는 `Ctrl+Shift+←` / `Ctrl+Shift+→` |

세션 목록 화면에서도 실행 중인 세션을 선택한 뒤 `Ctrl+K`를 누르면 해당 백그라운드 에이전트를 종료할 수 있습니다.

### 6-8. 에이전트 화면 스크롤

에이전트 터미널 화면은 일반 키 입력을 에이전트에 그대로 전달합니다. 그래서 스크롤용 키는 일반 방향키와 분리되어 있습니다.

| 동작 | 키 |
|---|---|
| 한 줄 위/아래 | `Shift+↑` / `Shift+↓` |
| 한 화면 위/아래 | `Shift+PageUp` / `Shift+PageDown` 또는 `Alt+PageUp` / `Alt+PageDown` |
| 맨 위/아래 | `Shift+Home` / `Shift+End` 또는 `Alt+Home` / `Alt+End` |

### 6-9. 세션 복제하기

세션을 선택하고 `c`를 누르면 Clone target 모달이 열립니다.

복제 대상:

- Claude
- Codex
- OpenCode

기본 선택은 원본과 같은 에이전트입니다. `↑`/`↓` 또는 `j`/`k`로 대상 에이전트를 고르고 `Enter`로 복제합니다. `Esc`는 취소입니다.

복제는 원본 세션을 수정하지 않고 새 세션 ID를 가진 복제본을 만듭니다. 다른 에이전트를 대상으로 고르면 가능한 범위에서 세션 데이터를 해당 에이전트 형식으로 변환해 설치합니다.

### 6-10. 세션 타이틀 바꾸기

세션을 선택하고 `t`를 누르면 제목 편집창이 열립니다. 사람이 알아보기 쉬운 이름을 붙일 수 있습니다.

저장 위치는 `~/.cokacmux/titles.json`입니다. 원본 에이전트의 대화 파일을 직접 수정하지 않고, cokacmux가 보여줄 표시 이름만 저장합니다.

### 6-11. 세션 삭제하기

세션을 선택하고 `Delete` 또는 `d`를 누르면 확인창이 열립니다. `y`로 삭제하고, `n` 또는 `Esc`로 취소합니다.

삭제는 실제 저장소의 세션 기록을 지우는 동작입니다.

| 에이전트 | 삭제 방식 |
|---|---|
| Claude Code | 해당 JSONL 세션 파일 삭제 |
| Codex | 해당 세션 파일과 관련 인덱스 정보 삭제 |
| OpenCode | SQLite DB의 해당 세션/메시지 행 삭제 |

중요한 기록은 삭제 전에 백업하세요.

### 6-12. 목록 새로고침과 종료

| 동작 | 키 |
|---|---|
| 디스크에서 세션 목록 다시 읽기 | `r` |
| 종료 | `q`, `Ctrl+Q`, `Ctrl+C` |

`q`나 `Ctrl+Q`로 TUI를 종료해도 백그라운드 에이전트는 사용자가 명시적으로 종료하지 않는 한 계속 살아 있습니다. 모두 정리하고 싶으면 `Ctrl+K` 또는 `cokacmux killall`을 사용하세요.

---

## 7. 키보드 단축키

기본 단축키입니다. 모든 단축키는 `~/.cokacmux/keybinding.json`에서 바꿀 수 있습니다.

### 세션 목록 / 미리보기 화면

| 키 | 동작 |
|---|---|
| `q` | 종료 |
| `Ctrl+Q` | 어디서든 종료 |
| `Ctrl+C` | 세션 화면에서 종료 |
| `↑`/`↓`, `j`/`k` | 선택 이동. 미리보기 포커스에서는 미리보기 스크롤 |
| `PageUp` / `PageDown` | 10칸 이동. 미리보기 포커스에서는 페이지 스크롤 |
| `Home`/`g`, `End`/`G` | 맨 위 / 맨 아래 |
| `Tab` / `Esc` | 세션 목록과 미리보기 포커스 전환 |
| `Enter` | 미리보기 요약/전체 전환 |
| `Space` | 미리보기 강제 새로고침 |
| `/` | 검색창 열기 |
| `v` | 트리 보기 / 목록 보기 전환 |
| `t` | 제목 편집 |
| `r` | 세션 목록 새로고침 |
| `c` | 복제 대상 선택 후 세션 복제 |
| `Delete` / `d` | 세션 삭제 확인창 열기 |
| `e` | Agent launch 모달 열기 |
| `Ctrl+N` | 새 터미널/새 코딩 에이전트 모달 열기 |
| `Ctrl+K` | 선택한 실행 중 에이전트 종료 |
| `Ctrl+]` / `Ctrl+[` | 활성 에이전트 화면으로 전환 또는 다시 연결 |
| `Ctrl+3` / `Ctrl+5` | `Ctrl+]` / `Ctrl+[` 대체 입력 |
| `Alt+↑` / `Alt+↓` | 세션 사이드바 선택 이동 |
| `Ctrl+Shift+↑` / `Ctrl+Shift+↓` | 세션 사이드바 선택 이동 |
| `Alt+←` / `Alt+→` | 세션 패널 크기 조절 |
| `Ctrl+Shift+←` / `Ctrl+Shift+→` | 세션 패널 크기 조절 |

### Agent launch 모달

`e` 키로 열리는 모달입니다.

| 키 | 동작 |
|---|---|
| `Enter` | 선택한 모드로 시작/연결 |
| `↑`/`↓`, `j`/`k` | 선택 이동 |
| `1` | Normal 선택 |
| `2` | Skip permissions 선택 |
| `Esc` | 취소 |

### New session 모달

`Ctrl+N`으로 열리는 모달입니다.

| 키 | 동작 |
|---|---|
| `Enter` | 선택한 설정으로 시작 |
| `↑`/`↓`, `j`/`k`, `Tab` / `BackTab` | 입력 항목 이동 |
| `←`/`→`, `h`/`l`, `Space` | Type / Agent / Permissions 값 변경 |
| 폴더 입력 중 문자 키 | 경로 입력 |
| 폴더 입력 중 `←`/`→` | 커서 이동 |
| 폴더 입력 중 `Home` / `End` | 처음 / 끝으로 이동 |
| 폴더 입력 중 `Backspace` / `Delete` | 글자 삭제 |
| `Esc` | 취소 |

폴더 입력 중에는 `j`, `k`, `h`, `l`, `Space`가 문자 입력으로 우선 처리됩니다. 입력 항목을 옮기려면 화살표, `Tab`, `BackTab`을 쓰세요.

### 에이전트 화면

| 키 | 동작 |
|---|---|
| `Ctrl+]` / `Ctrl+[` | 세션 목록으로 돌아가기 |
| `Ctrl+3` / `Ctrl+5` | `Ctrl+]` / `Ctrl+[` 대체 입력 |
| `Ctrl+Q` | TUI 종료 |
| `Ctrl+K` | 현재 에이전트/셸 종료 |
| `Ctrl+N` | 현재 작업 폴더를 기본값으로 새 세션 모달 열기 |
| `Ctrl+B` | 에이전트 사이드바 표시/숨김 |
| `Ctrl+PageUp` / `Ctrl+PageDown` | 이전/다음 실행 중 대상 전환 |
| `Shift+↑` / `Shift+↓` | PTY scrollback 한 줄 위/아래 |
| `Shift+PageUp` / `Shift+PageDown` | PTY scrollback 한 화면 위/아래 |
| `Alt+PageUp` / `Alt+PageDown` | PTY scrollback 한 화면 위/아래 |
| `Shift+Home` / `Shift+End` | PTY scrollback 맨 위/아래 |
| `Alt+Home` / `Alt+End` | PTY scrollback 맨 위/아래 |
| `Alt+↑` / `Alt+↓` | 에이전트 사이드바 선택 이동 |
| `Ctrl+Shift+↑` / `Ctrl+Shift+↓` | 에이전트 사이드바 선택 이동 |
| `Alt+←` / `Alt+→` | 에이전트 사이드바 폭 조절 |
| `Ctrl+Shift+←` / `Ctrl+Shift+→` | 에이전트 사이드바 폭 조절 |
| 그 외 키 | 현재 에이전트에 그대로 전달 |

### 검색창

| 키 | 동작 |
|---|---|
| `Enter` | Search 실행 |
| `Esc` | 닫기 |
| `←` / `→` | 커서 이동 |
| `Home` / `End` | 처음 / 끝 |
| `Backspace` / `Delete` | 글자 삭제 |

### 제목 편집창

| 키 | 동작 |
|---|---|
| `Enter` | 저장 |
| `Esc` | 취소 |
| `←` / `→` | 커서 이동 |
| `Home` / `End` | 처음 / 끝 |
| `Backspace` / `Delete` | 글자 삭제 |

### 복제 대상 선택 모달

| 키 | 동작 |
|---|---|
| `Enter` | 선택한 에이전트로 복제 |
| `↑`/`↓`, `j`/`k` | 대상 이동 |
| `Esc` | 취소 |

### 삭제 확인창

| 키 | 동작 |
|---|---|
| `y` / `Y` | 삭제 |
| `n` / `N` / `Esc` | 취소 |

---

## 8. CLI 명령어

TUI를 띄우지 않고 쓸 수 있는 명령도 있습니다.

| 명령 | 설명 |
|---|---|
| `cokacmux` | TUI 실행 |
| `cokacmux --check` | TUI 없이 세션 탐색이 되는지 확인 |
| `cokacmux --debug` | 디버그 로그를 켜고 TUI 실행 |
| `cokacmux killall` | cokacmux가 띄운 백그라운드 에이전트/셸 데몬 종료 |
| `cokacmux agents killall` | `cokacmux killall`과 같은 별칭 |
| `cokacmux --version` 또는 `cokacmux -V` | 버전 출력 |
| `cokacmux --help` 또는 `cokacmux -h` | 도움말 출력 |

### `--check`

터미널 화면을 바꾸지 않고 세션 탐색만 수행합니다.

```bash
cokacmux --check
```

예상 출력:

```text
cokacmux --check ok: 12 sessions discovered (status: 12 sessions)
```

세션이 안 보일 때 먼저 이 명령으로 확인하면 좋습니다.

### `killall`

cokacmux가 띄운 백그라운드 에이전트/셸 데몬을 한 번에 종료합니다.

```bash
cokacmux killall
```

출력 예:

```text
killed 2 agent daemon(s); stale=0 skipped_self=0 errors=0 pty_logs_deleted=2
```

`killall`은 cokacmux가 관리하는 런타임 메타데이터를 확인한 뒤 대상만 종료합니다. 일반적으로 사용자가 별도 터미널에서 직접 실행한 `claude`, `codex`, `opencode`까지 무작정 죽이는 용도가 아닙니다.

---

## 9. 설정 파일

설정 파일은 `~/.cokacmux/settings.json`입니다. 처음 실행할 때 파일이 없으면 기본 파일이 자동으로 생성됩니다.

기본 생성 예:

```json
{
  "cokacmux": {
    "sessions_pane_percent": 45,
    "sessions_pane_width": null,
    "agent_sidebar_width": 30,
    "agent_sidebar_visible": true,
    "session_view": "tree",
    "agent_programs": {
      "codex": "",
      "claude": "",
      "opencode": ""
    }
  }
}
```

각 항목의 의미:

| 키 | 의미 |
|---|---|
| `sessions_pane_percent` | 세션 목록 패널의 기본 너비 비율입니다. 기본값은 45입니다. |
| `sessions_pane_width` | 세션 목록 패널의 고정 너비입니다. `null`이면 percent 값을 씁니다. 패널 크기를 조절하면 숫자로 저장됩니다. |
| `agent_sidebar_width` | 에이전트 화면의 사이드바 너비입니다. |
| `agent_sidebar_visible` | 에이전트 사이드바를 처음에 보일지 정합니다. |
| `session_view` | `"tree"` 또는 `"list"`입니다. `v` 키로도 바뀝니다. |
| `agent_programs` | Claude/Codex/OpenCode 실행 파일 경로를 직접 지정하는 곳입니다. |

`agent_programs`의 빈 문자열은 placeholder입니다. 비워 두면 기존처럼 PATH에서 `codex`, `claude`, `opencode`를 찾습니다.

### `agent_programs` 자세히

| 키 | 비워 두면 실행하는 명령 | 쓰는 곳 |
|---|---|---|
| `agent_programs.claude` | `claude` | Claude Code 세션 이어가기, 새 Claude Code 시작 |
| `agent_programs.codex` | `codex` | Codex 세션 이어가기, 새 Codex 시작 |
| `agent_programs.opencode` | `opencode` | OpenCode 세션 이어가기, 새 OpenCode 시작 |

동작 규칙:

- 키를 생략하면 기본 명령 이름을 씁니다.
- `null`, 빈 문자열, 공백뿐인 문자열은 설정하지 않은 것으로 보고 기본 명령 이름을 씁니다.
- `~/bin/codex`처럼 `~/`로 시작하면 사용자 홈 디렉터리로 확장합니다.
- `/usr/bin/codex`처럼 경로이면 그 파일을 실행합니다.
- `codex-beta`처럼 명령 이름이면 PATH에서 다시 찾습니다.
- Windows에서는 `.exe`, `.cmd`, `.bat`, `.ps1` 경로를 사용할 수 있습니다.
- 이미 실행 중인 백그라운드 에이전트에는 변경된 경로가 적용되지 않습니다. 새 경로로 실행하려면 기존 에이전트를 `Ctrl+K` 또는 `cokacmux killall`로 종료한 뒤 다시 시작하세요.

`settings.json`은 앱 시작 시 읽습니다. 파일을 직접 고친 뒤에는 cokacmux를 재시작하는 편이 가장 명확합니다. 단축키 파일인 `keybinding.json`만 다음 키 입력 때 자동으로 다시 읽습니다.

---

## 10. 단축키 설정

단축키 파일은 `~/.cokacmux/keybinding.json`입니다. 파일이 없으면 기본 단축키가 모두 들어간 파일을 자동으로 만듭니다.

앱을 재시작할 필요는 없습니다. 키를 누를 때마다 파일 수정 시각을 확인하고, 바뀐 경우에만 다시 읽습니다. 파일을 잘못 수정해서 파싱에 실패하면 기존 단축키를 유지하고 상태줄/디버그 로그에 실패 이유를 남깁니다.

원하는 액션만 적으면 됩니다.

```json
{
  "sessions": {
    "launch_agent": ["x"],
    "delete": ["delete", "d"],
    "quit": ["q", "ctrl+q"]
  },
  "agent_launch": {
    "skip_permissions": ["2", "s"]
  },
  "new_session": {
    "next": ["down", "tab"],
    "prev": ["up", "backtab"]
  },
  "agent": {
    "scroll_page_up": ["shift+up", "alt+k"],
    "scroll_page_down": ["shift+down", "alt+j"],
    "switch_prev": ["ctrl+,"],
    "switch_next": ["ctrl+."]
  }
}
```

주의할 점:

- 액션을 설정하면 기본값에 추가되는 것이 아니라 그 액션의 기본 단축키 전체를 대체합니다.
- 파일에 없는 액션은 기본값을 그대로 씁니다.
- 빈 배열 `[]` 또는 `null`을 넣으면 해당 액션이 비활성화됩니다.
- 점 표기(`"sessions.launch_agent": ["x"]`)도 사용할 수 있습니다.

키 표기 예:

```text
ctrl+q
alt+up
shift+pageup
ctrl+shift+left
enter
esc
space
delete
f1
```

지원하는 키 이름과 전체 액션 목록은 [`docs/KEYBINDINGS.md`](docs/KEYBINDINGS.md)에 정리되어 있습니다.

기본 단축키 파일을 다시 만들고 싶다면 `~/.cokacmux/keybinding.json`을 삭제하세요. 다음 키 입력 때 기본 파일이 다시 생성됩니다.

---

## 11. 데이터 저장 위치

cokacmux가 직접 만드는 파일은 홈 폴더 안의 `.cokacmux/` 디렉터리에 모입니다.

| 위치 | 용도 |
|---|---|
| `~/.cokacmux/settings.json` | UI 설정과 에이전트 실행 파일 경로 설정 |
| `~/.cokacmux/keybinding.json` | 단축키 설정 |
| `~/.cokacmux/titles.json` | 사용자가 붙인 세션 표시 이름 |
| `~/.cokacmux/agents/` | 실행 중인 백그라운드 에이전트/셸 메타데이터와 통신용 소켓 |
| `~/.cokacmux/debug/cokacmux.log` | `--debug`로 실행했을 때 기록되는 단일 런타임 로그 |

Windows에서 `~`는 보통 `C:\Users\사용자이름\`입니다. 따라서 `~/.cokacmux/`는 보통 `C:\Users\사용자이름\.cokacmux\`입니다.

원본 에이전트 데이터는 각 에이전트의 저장소에 그대로 있습니다.

| 에이전트 | 원본 데이터 |
|---|---|
| Claude Code | `~/.claude/projects/...` |
| Codex | `~/.codex/sessions/...` |
| OpenCode | macOS/Linux: `~/.local/share/opencode/opencode.db` |
| OpenCode | Windows: `%LOCALAPPDATA%\opencode\opencode.db` |

---

## 12. 자주 묻는 질문

### AI 비용이 더 나가나요?

아니요. cokacmux는 AI API를 직접 호출하지 않습니다. 이미 쓰는 Claude Code, Codex, OpenCode CLI를 실행하고, 로컬에 저장된 세션 파일을 읽고 쓸 뿐입니다.

### 제 대화 내용이 외부로 전송되나요?

cokacmux 자체는 세션 데이터를 외부 서버로 전송하지 않습니다. 모든 처리는 로컬 파일과 로컬 프로세스를 대상으로 합니다. 단, 실제 에이전트 CLI가 동작하면서 각 서비스와 통신하는 것은 해당 에이전트의 일반 동작입니다.

### 세션이 하나도 안 보여요.

다음을 확인하세요.

- Claude Code, Codex, OpenCode 중 하나를 한 번이라도 사용해 세션이 생겼는지 확인합니다.
- 각 에이전트가 기본 위치에 데이터를 저장하는지 확인합니다.
- `cokacmux --check`를 실행해 몇 개의 세션이 발견되는지 확인합니다.
- 문제가 계속되면 `cokacmux --debug`로 실행한 뒤 `~/.cokacmux/debug/` 로그를 확인합니다.

### 미리보기는 되는데 `e`로 실행이 안 돼요.

미리보기는 저장 파일만 읽으면 되지만, 실행은 실제 에이전트 CLI가 필요합니다. 해당 명령이 PATH에서 실행되는지 확인하세요.

```bash
command -v claude
command -v codex
command -v opencode
```

Windows PowerShell:

```powershell
Get-Command claude
Get-Command codex
Get-Command opencode
```

로그인/인증이 끝나 있는지도 확인하세요. PATH가 애매하면 `settings.json`의 `agent_programs`에 직접 경로를 지정할 수 있습니다.

### `e`와 `Ctrl+N`은 뭐가 다른가요?

`e`는 선택한 저장 세션을 이어 실행하거나 이미 실행 중인 같은 세션에 다시 붙습니다.

`Ctrl+N`은 새 세션 모달을 열어 새 터미널 또는 새 코딩 에이전트를 시작합니다.

### Skip permissions는 언제 쓰나요?

보통은 Normal을 쓰면 됩니다. Skip permissions는 에이전트가 명령 실행이나 파일 접근 전에 묻는 확인 절차를 우회하는 위험한 모드입니다.

각 에이전트에서는 다음과 비슷한 효과가 납니다.

| 에이전트 | 권한 우회 방식 |
|---|---|
| Claude Code | `--dangerously-skip-permissions` |
| Codex | `--yolo` |
| OpenCode | `OPENCODE_PERMISSION='{"*":"allow"}'` |

신뢰하지 않는 저장소, 외부에서 받은 코드, 중요한 파일이 많은 폴더에서는 사용하지 마세요.

### `Ctrl+]` / `Ctrl+[`가 안 먹혀요.

일부 터미널은 `Ctrl+[`를 `Esc`로 보내거나, 특정 Ctrl 조합을 가로챌 수 있습니다. 이 경우 `Ctrl+]`를 먼저 써 보세요. 그래도 어렵다면 `keybinding.json`에서 `sessions.toggle_agent`와 `agent.toggle_sessions`를 다른 키로 바꾸면 됩니다.

기본값에는 대체 입력으로 `Ctrl+3`, `Ctrl+5`도 들어 있습니다.

### 백그라운드 에이전트는 언제 종료되나요?

사용자가 명시적으로 종료하기 전까지 계속 살아 있습니다.

종료 방법:

- 현재 에이전트 화면에서 `Ctrl+K`
- 세션 목록에서 실행 중인 세션을 선택하고 `Ctrl+K`
- 전체 정리: `cokacmux killall`

`q`나 `Ctrl+Q`로 TUI를 종료해도 백그라운드 에이전트는 자동 종료하지 않습니다.

### `Delete`와 `Ctrl+K`는 왜 둘 다 있나요?

서로 대상이 다릅니다.

| 키 | 대상 | 결과 |
|---|---|---|
| `Delete` / `d` | 저장된 세션 기록 | 실제 세션 데이터 삭제 |
| `Ctrl+K` | 실행 중인 백그라운드 프로세스 | 에이전트/셸 종료 |

### 검색이 느릴 수 있나요?

세션이 많고 본문이 길면 시간이 걸릴 수 있습니다. 검색은 백그라운드에서 실행되고, 검색 중에는 검색창에 Searching 상태가 표시됩니다.

### 맥북에 PageUp/PageDown 키가 없어요.

보통 `fn+↑`, `fn+↓`가 PageUp/PageDown으로 동작합니다. 터미널이 이 키를 전달하지 않으면 `keybinding.json`에서 다른 키를 지정하세요. 예를 들어 에이전트 스크롤을 `alt+k`, `alt+j`로 바꿀 수 있습니다.

### Windows에서 한글이나 박스 문자가 깨져요.

Windows Terminal 사용을 권장합니다. 글꼴은 한글과 박스 문자를 지원하는 등폭 글꼴을 선택하세요. 예를 들면 Cascadia Mono, D2Coding, JetBrains Mono 계열이 무난합니다.

### 화면이 깨지거나 커서 위치가 이상해요.

터미널 폭을 80칸 이상으로 넓혀 보세요. 그래도 문제가 있으면 `r`로 새로고침하거나 앱을 다시 실행하세요. 문제가 반복되면 `cokacmux --debug`로 로그를 남겨 확인하세요.

---

## 13. 문제가 생겼을 때

### 빠른 점검 순서

1. 버전 확인:

```bash
cokacmux --version
```

2. 세션 탐색 확인:

```bash
cokacmux --check
```

3. 에이전트 CLI 확인:

```bash
claude --version
codex --version
opencode --version
```

4. 디버그 모드로 실행:

```bash
cokacmux --debug
```

### 디버그 로그

`--debug`를 붙여 실행한 경우에만 `~/.cokacmux/debug/cokacmux.log`에 로그가 기록됩니다. TUI, 세션 목록, 검색, 미리보기, 에이전트 시작/연결, 백그라운드 데몬, provider 처리, 변환/복제 흐름이 모두 이 단일 파일에 모입니다.

각 줄에는 시간, 프로세스 ID, 스레드 정보, 이벤트 이름, 세부 JSON이 함께 들어갑니다. 백그라운드 에이전트처럼 별도 프로세스에서 발생한 로그도 같은 파일에 append됩니다.

활성 로그 파일은 `cokacmux.log` 하나입니다. 파일이 5 MiB를 넘으면 기존 파일은 `cokacmux.log.1`로 회전되고 새 `cokacmux.log`에 이어서 기록됩니다.

---

## 14. 업데이트와 제거

### 업데이트

설치 명령을 다시 실행하면 새 버전으로 덮어씁니다.

macOS/Linux:

```bash
curl -fsSL https://cokacmux.cokac.com/manage.sh | bash
```

Windows PowerShell:

```powershell
irm https://cokacmux.cokac.com/manage.ps1 | iex
```

### 제거

macOS/Linux:

- 실행 파일: `/usr/local/bin/cokacmux` 또는 `~/.local/bin/cokacmux`
- 설정까지 지우려면: `~/.cokacmux/`

Windows:

- 실행 파일과 설치 폴더: `%LOCALAPPDATA%\cokacmux\`
- 설정까지 지우려면: `C:\Users\사용자이름\.cokacmux\`

원본 Claude/Codex/OpenCode 세션 데이터는 별도 위치에 있으므로, 위 파일을 지워도 원본 에이전트 데이터가 자동으로 삭제되지는 않습니다.

---

## 15. 소스에서 빌드하기

일반 사용자는 설치 스크립트를 쓰면 됩니다. 직접 빌드하려면 Rust가 필요합니다.

필요한 것:

- Rust 안정 버전: https://rustup.rs
- C 컴파일러: OpenCode 지원을 위해 SQLite를 함께 빌드합니다.
- macOS: Xcode Command Line Tools
- Linux: `build-essential` 또는 `gcc`
- Windows: MSVC 빌드 도구
- 여러 OS용으로 한 번에 빌드하려면 Python 3과 `zig`

빌드:

```bash
git clone https://github.com/kstost/cokacmux
cd cokacmux
cargo build --release --bin cokacmux
./target/release/cokacmux
```

개발 중 검증:

```bash
cargo test
cargo fmt --check
cargo clippy --all-targets --all-features
```

여러 운영체제용 빌드:

```bash
python build.py --setup
python build.py --all
python build.py --windows
python build.py --status
```

산출물은 `dist_beta/cokacmux-<OS>-<CPU>[.exe]` 형태로 만들어집니다.

---

## 16. 작동 원리

세 에이전트는 세션 데이터를 서로 다른 방식으로 저장합니다.

| 에이전트 | 저장 형식 |
|---|---|
| Claude Code | 작업 폴더별 JSONL 파일 |
| Codex | 날짜별 JSONL rollout 파일과 SQLite 인덱스 |
| OpenCode | SQLite 데이터베이스 |

cokacmux는 이 데이터를 읽어 공통 모델로 표현합니다. 그래서 한 화면에서 세 에이전트 세션을 같이 보여주고, 검색하고, 미리보기하고, 복제할 수 있습니다.

에이전트를 실행할 때는 자체 AI 엔진을 쓰지 않습니다. 사용자의 시스템에 설치된 `claude`, `codex`, `opencode` CLI를 PTY 안에서 실행하고, TUI가 그 화면에 붙었다 떨어졌다 하는 방식입니다.

백그라운드 에이전트 정보는 `~/.cokacmux/agents/`에 저장됩니다. 이 정보 덕분에 세션 목록으로 돌아와도 에이전트가 계속 살아 있고, 나중에 다시 연결할 수 있습니다.

---

## 더 알고 싶다면

- [`docs/KEYBINDINGS.md`](docs/KEYBINDINGS.md): 단축키 파일 형식, 키 이름, 전체 액션 목록
- [`docs/PLAN.md`](docs/PLAN.md): 전체 설계 배경과 에이전트별 저장 형식 분석
- [`docs/RESULTS.md`](docs/RESULTS.md): 실제 세션 데이터로 검증한 결과

---

## 라이선스 / 만든 사람

- 라이선스: MIT
- 만든 사람: cokac <monogatree@gmail.com>
- 저장소: https://github.com/kstost/cokacmux
- 공식 페이지: https://cokacmux.cokac.com

문제가 생기거나 새 기능 제안이 있으면 GitHub Issues로 알려주세요.

---

## 면책조항

이 소프트웨어는 있는 그대로 제공되며, 명시적이든 묵시적이든 어떠한 종류의 보증도 제공하지 않습니다. 여기에는 상품성, 특정 목적 적합성, 비침해성에 대한 보증이 포함되지만 이에 한정되지 않습니다.

어떠한 경우에도 작성자, 저작권자 또는 기여자는 이 소프트웨어의 사용 또는 사용 불능과 관련해 발생하는 어떠한 청구, 손해 또는 기타 책임에 대해 책임지지 않습니다. 여기에는 데이터 손실 또는 손상, 시스템 오작동, 보안 문제, 금전적 손실, 직접적/간접적/부수적/특별/징벌적/결과적 손해가 포함되지만 이에 한정되지 않습니다.

이 소프트웨어를 사용하는 데 따른 모든 위험과 책임은 사용자 본인에게 있습니다.
