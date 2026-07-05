# cokacmux Keybindings

cokacmux는 config 디렉터리의 `keybinding.json`을 읽어 단축키를 설정합니다. 기본 위치는 `~/.cokacmux/keybinding.json`이고, `COKACMUX_CONFIG_DIR`을 쓰면 그 디렉터리 아래 파일을 읽습니다. 파일이 없으면 기본 단축키가 모두 들어간 파일을 자동으로 만듭니다.

앱을 재시작하지 않아도 됩니다. 백그라운드 감시 스레드가 약 2초마다 파일의 수정 시각만 확인하고, 파일이 바뀐 경우에만 다시 읽어 파싱합니다. 실행 중 파일이 삭제되어도 다음 감시 스레드 확인 때 기본 파일을 다시 만듭니다. 파싱에 실패하면 기존 단축키를 유지하고 status/debug log에 실패 이유를 남깁니다.

## 설정 방식

원하는 액션만 JSON에 적으면 됩니다. 파일에 없는 액션은 기본값을 그대로 사용합니다.

```json
{
  "sessions": {
    "launch_agent": ["x"],
    "quit": ["q", "ctrl+q"]
  },
  "agent": {
    "scroll_page_up": ["shift+alt+up", "shift+alt+pageup", "alt+pageup"],
    "scroll_page_down": ["shift+alt+down", "shift+alt+pagedown", "alt+pagedown"],
    "switch_prev": ["ctrl+,"],
    "switch_next": ["ctrl+."]
  },
  "new_session": {
    "next": ["down", "tab"],
    "prev": ["up", "backtab"]
  }
}
```

각 액션 값은 다음 형태를 지원합니다.

| 값 | 의미 |
|---|---|
| `"x"` | 단일 키로 지정 |
| `["x", "ctrl+x"]` | 여러 키 중 하나로 지정 |
| `[]` | 해당 액션 비활성화 |
| `null` | 해당 액션 비활성화 |

액션을 설정하면 기본값에 추가되는 것이 아니라 그 액션의 기본 단축키 전체를 대체합니다. 예를 들어 `"sessions.quit": ["ctrl+q"]`만 쓰면 `q` 종료는 꺼지고 `ctrl+q`만 남습니다.

현재 기본값에서 `sessions.launch_agent`는 `["e", "enter"]`, `sessions.toggle_focus`는 `["tab"]`, `sessions.filter`는 `["ctrl+f"]`, `sessions.ai_title_settings`는 `["comma"]`이고, `sessions.toggle_preview`와 `sessions.ai_search`는 빈 배열입니다. Agent 화면에서는 `agent.toggle_cokacdir_panel`이 `["ctrl+f"]`, `agent.toggle_terminal_panel`이 `["ctrl+t"]`, `agent.focus_sidebar`/`agent.focus_main`/`agent.focus_auxiliary`가 각각 `["ctrl+1"]`/`["ctrl+2"]`/`["ctrl+3"]`입니다.

구버전에서 자동 생성된 `sessions.launch_agent: ["e"]` + `sessions.toggle_preview: ["enter"]` 조합은 새 기본값으로 자동 갱신됩니다. 직접 바꾼 값은 유지됩니다.

구버전에서 자동 생성된 `sessions.filter: ["/"]`와 `sessions.ai_search: ["ctrl+s"]` 값도 새 검색 선택창 기본값으로 자동 갱신됩니다. 직접 바꾼 값은 유지됩니다.

구버전에서 자동 생성된 `sessions.ai_title_settings: ["ctrl+t"]` 또는 `["comma", "ctrl+t"]` 값도 `["comma"]` 기본값으로 자동 갱신됩니다. 직접 바꾼 값은 유지됩니다.

구버전에서 자동 생성된 `sessions.toggle_focus: ["tab", "esc"]` 값도 `["tab"]` 기본값으로 자동 갱신됩니다. 직접 바꾼 값은 유지됩니다.

구버전에서 자동 생성된 이동 계열 기본값의 `h`, `j`, `k`, `l` 별칭도 새 기본값으로 자동 갱신됩니다. 직접 바꾼 값은 유지됩니다.

구버전에서 자동 생성된 `agent.scroll_page_up` / `agent.scroll_page_down` 값도 새 기본값으로 자동 갱신됩니다. 직접 바꾼 값은 유지됩니다.

점 표기(flat)도 사용할 수 있습니다.

```json
{
  "sessions.launch_agent": ["x"],
  "agent.scroll_page_down": ["alt+d"]
}
```

## 키 이름

기본 형식은 `modifier+modifier+key`입니다.

```text
ctrl+q
alt+up
shift+alt+up
ctrl+shift+left
```

키 이름은 대소문자를 구분하지 않습니다. 키 이름 안의 `_`, `-`, 공백은 무시되므로 `pageup`, `page-up`, `page_up`은 같은 키입니다.

### Modifier

| 이름 | 별칭 |
|---|---|
| `ctrl` | `control` |
| `alt` | `option` |
| `shift` | |
| `super` | `cmd`, `command` |
| `meta` | |
| `hyper` | |

### Special Keys

| 이름 | 별칭 |
|---|---|
| `backspace` | `bksp` |
| `enter` | `return` |
| `left` | |
| `right` | |
| `up` | |
| `down` | |
| `home` | |
| `end` | |
| `pageup` | `pgup` |
| `pagedown` | `pgdn` |
| `tab` | |
| `backtab` | |
| `delete` | `del` |
| `insert` | `ins` |
| `esc` | `escape` |
| `space` | |
| `f1` ... `f12` | |

### Symbol Keys

일반 문자 하나는 그대로 쓸 수 있습니다.

```text
a
G
1
/
]
,
.
```

다음 기호는 이름으로도 쓸 수 있습니다.

| 이름 | 키 |
|---|---|
| `slash` | `/` |
| `backslash` | `\` |
| `comma` | `,` |
| `dot`, `period` | `.` |
| `plus` | `+` |
| `minus`, `dash` | `-` |
| `semicolon` | `;` |
| `colon` | `:` |
| `quote` | `'` |
| `doublequote` | `"` |
| `backtick`, `grave` | `` ` `` |
| `openbracket`, `lbracket` | `[` |
| `closebracket`, `rbracket` | `]` |

`+` 키 자체를 설정할 때는 `plus`를 사용하세요. `+` 문자는 modifier 구분자로 쓰입니다.

## 기본 액션

### global

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `global.quit` | `ctrl+q` | 어디서든 종료 |

### sessions

세션 목록/미리보기 화면에서 쓰는 액션입니다. `Esc`는 `sessions.toggle_focus` 기본값이 아닙니다. normal 세션 화면에서는 적용된 검색 결과를 먼저 해제하고, 검색 결과가 없으면 종료합니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `sessions.quit` | `q` | 종료 |
| `sessions.force_quit` | `ctrl+c` | 종료 |
| `sessions.toggle_agent` | `ctrl+]`, `ctrl+[` | 세션 화면과 agent 화면 전환 |
| `sessions.kill_agent` | `ctrl+k` | 선택한 실행 중 대상 종료 |
| `sessions.new_shell` | `ctrl+n` | 새 세션 모달 열기 |
| `sessions.toggle_focus` | `tab` | 세션 목록과 미리보기 포커스 전환 |
| `sessions.toggle_preview` | 없음 | 미리보기를 summary 모드로 되돌림. 기본값에서는 비활성화 |
| `sessions.move_next` | `down` | 다음 행 선택 또는 미리보기 아래로 스크롤 |
| `sessions.move_prev` | `up` | 이전 행 선택 또는 미리보기 위로 스크롤 |
| `sessions.page_next` | `pagedown` | 10행 아래 또는 미리보기 한 페이지 아래 |
| `sessions.page_prev` | `pageup` | 10행 위 또는 미리보기 한 페이지 위 |
| `sessions.top` | `home`, `g` | 처음으로 이동 |
| `sessions.bottom` | `end`, `G` | 끝으로 이동 |
| `sessions.filter` | `ctrl+f` | 검색 방식 선택창 열기 |
| `sessions.ai_search` | 없음 | AI 검색 프롬프트 직접 열기. 기본값에서는 검색 선택창을 사용 |
| `sessions.toggle_view` | `v` | tree/list 보기 전환 |
| `sessions.refresh` | `r` | 세션 다시 읽기 |
| `sessions.delete` | `delete`, `d` | 선택 세션 삭제 확인 열기 |
| `sessions.clone` | `c` | 선택 세션 복제 |
| `sessions.edit_title` | `t` | 선택 세션 제목 편집 |
| `sessions.ai_title_settings` | `comma` | 설정 화면 열기 |
| `sessions.launch_agent` | `e`, `enter` | agent launch 모드 선택 열기 또는 live agent 연결 |
| `sessions.refresh_preview` | `space` | 미리보기 캐시 무시하고 다시 그리기 |
| `sessions.resize_left` | `alt+left`, `ctrl+shift+left` | 세션 패널 좁히기 |
| `sessions.resize_right` | `alt+right`, `ctrl+shift+right` | 세션 패널 넓히기 |
| `sessions.sidebar_prev` | `alt+up`, `ctrl+shift+up` | 세션 목록 선택 위로 이동 |
| `sessions.sidebar_next` | `alt+down`, `ctrl+shift+down` | 세션 목록 선택 아래로 이동 |

### search

`sessions.filter`로 열리는 검색 방식 선택 모달에서 쓰는 액션입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `search.cancel` | `esc` | 취소 |
| `search.confirm` | `enter` | 선택한 검색 방식 열기 |
| `search.next` | `down`, `tab` | 다음 검색 방식 |
| `search.prev` | `up`, `backtab` | 이전 검색 방식 |
| `search.text` | `1` | 일반 검색 선택 |
| `search.ai` | `2` | AI 검색 선택 |

### agent

실행 중인 agent 화면에서 쓰는 액션입니다. 여기에 잡히지 않은 키는 active agent PTY로 전달됩니다. Codex는 line/page/top/bottom 스크롤을 transcript/pager 입력으로 보냅니다. Claude Code는 page/top/bottom을 fullscreen scroll 키로 바꾸고, 한 줄 스크롤은 원래 키를 자식 CLI에 전달합니다. OpenCode는 page up/down만 전용 키로 바꾸고, line/top/bottom은 원래 키를 자식 CLI에 전달합니다. Pi와 GJC는 현재 원래 스크롤 키를 자식 CLI에 전달합니다. 일반 터미널은 cokacmux PTY scrollback을 움직입니다. `cokacdir`에서는 Shift가 포함된 단축키를 자식 앱에 우선 전달하므로 기본 `Shift+...` 스크롤 키는 `cokacdir`로 들어가고, Shift가 없는 `Alt+Home` / `Alt+End`는 parent PTY scrollback을 움직입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `agent.toggle_sessions` | `ctrl+]`, `ctrl+[` | 세션 화면으로 전환 |
| `agent.kill` | `ctrl+k` | 현재 코딩 agent/일반 터미널 종료. `cokacdir` 화면에서는 자식 앱에 전달 |
| `agent.new_shell` | `ctrl+n` | 현재 agent cwd를 기본값으로 새 세션 모달 열기 |
| `agent.toggle_sidebar` | `ctrl+b` | agents 사이드바 표시/숨김 |
| `agent.toggle_cokacdir_panel` | `ctrl+f` | 현재 코딩 에이전트의 cwd로 오른쪽 cokacdir 패널 표시/숨김. 숨김 중에도 자식 앱은 계속 실행 |
| `agent.toggle_terminal_panel` | `ctrl+t` | 현재 코딩 에이전트의 cwd로 오른쪽 terminal 패널 표시/숨김. 숨김 중에도 자식 앱은 계속 실행 |
| `agent.focus_sidebar` | `ctrl+1` | 왼쪽 agents 패널로 포커스 이동. 숨겨져 있으면 표시 |
| `agent.focus_main` | `ctrl+2` | 중앙 agent 패널로 포커스 이동 |
| `agent.focus_auxiliary` | `ctrl+3` | 오른쪽 보조 패널로 포커스 이동 |
| `agent.scroll_line_up` | `shift+up` | transcript/scrollback 한 줄 위 |
| `agent.scroll_line_down` | `shift+down` | transcript/scrollback 한 줄 아래 |
| `agent.scroll_page_up` | `shift+alt+up`, `shift+alt+pageup`, `alt+pageup` | transcript/scrollback 한 페이지 위 |
| `agent.scroll_page_down` | `shift+alt+down`, `shift+alt+pagedown`, `alt+pagedown` | transcript/scrollback 한 페이지 아래 |
| `agent.scroll_top` | `shift+home`, `alt+home` | transcript/scrollback 맨 위 |
| `agent.scroll_bottom` | `shift+end`, `alt+end` | transcript/scrollback 맨 아래 |
| `agent.resize_left` | `alt+left`, `ctrl+shift+left` | 포커스된 side panel 경계 왼쪽 이동. 중앙 포커스에서 양쪽이 모두 열려 있으면 동작하지 않음 |
| `agent.resize_right` | `alt+right`, `ctrl+shift+right` | 포커스된 side panel 경계 오른쪽 이동. 왼쪽 패널 숨김 상태에서 왼쪽 resize 대상이면 한 단계 표시 |
| `agent.sidebar_prev` | `alt+up`, `ctrl+shift+up` | agents 사이드바 선택 위로 이동 |
| `agent.sidebar_next` | `alt+down`, `ctrl+shift+down` | agents 사이드바 선택 아래로 이동 |
| `agent.switch_prev` | `ctrl+pageup` | 이전 live agent로 전환 |
| `agent.switch_next` | `ctrl+pagedown` | 다음 live agent로 전환 |

### delete_confirm

세션 삭제 확인 모달에서 쓰는 액션입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `delete_confirm.cancel` | `esc`, `n`, `N` | 삭제 취소 |
| `delete_confirm.confirm` | `enter` | 선택한 버튼 실행 |
| `delete_confirm.next` | `right`, `down`, `tab` | 다음 버튼 선택 |
| `delete_confirm.prev` | `left`, `up`, `backtab` | 이전 버튼 선택 |
| `delete_confirm.delete` | `1`, `y`, `Y` | Delete session 버튼 실행 |
| `delete_confirm.cancel_choice` | `2` | Cancel 버튼 실행 |

### create_folder

세션 실행 폴더가 없을 때 표시되는 생성 확인 모달에서 쓰는 액션입니다. 기본 선택은 `Cancel`입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `create_folder.cancel` | `esc`, `n`, `N` | 생성하지 않고 취소 |
| `create_folder.confirm` | `enter` | 선택한 버튼 실행 |
| `create_folder.next` | `right`, `down`, `tab` | 다음 버튼 선택 |
| `create_folder.prev` | `left`, `up`, `backtab` | 이전 버튼 선택 |
| `create_folder.create` | `1`, `y`, `Y` | Create/start 버튼 실행 |
| `create_folder.cancel_choice` | `2` | Cancel 버튼 실행 |

### restore_data

clone으로 저장된 폴더 데이터가 있을 때 표시되는 복원 확인 모달에서 쓰는 액션입니다. 기본 선택은 `Start without restore`입니다. 같은 cwd에 live 코딩에이전트가 있으면 restore/start는 차단됩니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `restore_data.skip` | `esc`, `n`, `N` | 복원 없이 시작 |
| `restore_data.confirm` | `enter` | 선택한 버튼 실행 |
| `restore_data.next` | `right`, `down`, `tab` | 다음 버튼 선택 |
| `restore_data.prev` | `left`, `up`, `backtab` | 이전 버튼 선택 |
| `restore_data.restore` | `1`, `y`, `Y` | Restore/start 버튼 실행 |
| `restore_data.skip_choice` | `2` | Start without restore 버튼 실행 |

### filter

검색창에서 쓰는 액션입니다. 검색 적용 시 세션 ID, 작업 폴더, 타이틀과 세션 본문 전체를 대상으로 찾습니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `filter.cancel` | `esc` | 검색창 닫기 (미적용) |
| `filter.apply` | `enter` | Search 버튼 실행 |
| `filter.move_left` | `left` | 커서 왼쪽 이동 |
| `filter.move_right` | `right` | 커서 오른쪽 이동 |
| `filter.home` | `home` | 입력 처음으로 이동 |
| `filter.end` | `end` | 입력 끝으로 이동 |
| `filter.backspace` | `backspace` | 검색어 한 글자 삭제 |
| `filter.delete` | `delete` | 커서 위치 글자 삭제 |

### title

제목 편집 모드에서 쓰는 액션입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `title.cancel` | `esc` | 제목 편집 취소 |
| `title.save` | `enter` | 제목 저장 |
| `title.move_left` | `left` | 커서 왼쪽 이동 |
| `title.move_right` | `right` | 커서 오른쪽 이동 |
| `title.home` | `home` | 커서 처음으로 |
| `title.end` | `end` | 커서 끝으로 |
| `title.backspace` | `backspace` | 커서 앞 글자 삭제 |
| `title.delete` | `delete` | 커서 위치 글자 삭제 |
| `title.ai_generate` | `ctrl+t` | 선택한 AI agent로 세션 제목 자동 생성 |

### ai_title_settings

설정 화면에서 쓰는 액션입니다. 액션 이름은 기존 설정과의 호환을 위해 `ai_title_settings`로 남아 있습니다. 기본 AI agent 설정은 없음입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `ai_title_settings.cancel` | `esc` | 변경하지 않고 닫기. 실행 파일 경로 편집 중에는 편집 내용을 버리고 편집 종료 |
| `ai_title_settings.save` | `enter` | 설정 저장. 실행 파일 경로 행에서는 편집 시작/완료. 편집 완료 직후 다시 누르면 저장 |
| `ai_title_settings.next` | `down`, `tab` | 다음 행 |
| `ai_title_settings.prev` | `up`, `backtab` | 이전 행 |
| `ai_title_settings.none` | `1` | 설정 없음 선택 |
| `ai_title_settings.claude` | `2` | Claude 선택 |
| `ai_title_settings.codex` | `3` | Codex 선택 |
| `ai_title_settings.opencode` | `4` | OpenCode 선택 |
| `ai_title_settings.pi` | `5` | Pi 선택 |

설정 화면의 섹션 이동은 `←` / `→`입니다. `Space`는 현재 행의 값을 바꾸거나 AI provider를 선택합니다. Keybindings와 Data 섹션은 읽기 전용이며, 저장 전 변경사항은 설정 화면 상단에 표시됩니다.

### agent_launch

`sessions.launch_agent`가 아직 실행 중이 아닌 세션에 대해 여는 launch mode 선택 모달에서 쓰는 액션입니다. 선택 세션의 agent가 이미 살아 있으면 이 모달을 거치지 않고 바로 switch/attach 합니다. 같은 cwd를 쓰는 다른 live 코딩에이전트가 있으면 새 agent 시작은 차단됩니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `agent_launch.cancel` | `esc` | 취소 |
| `agent_launch.confirm` | `enter` | 선택한 launch mode로 start/attach |
| `agent_launch.next` | `down` | 다음 launch mode |
| `agent_launch.prev` | `up` | 이전 launch mode |
| `agent_launch.normal` | `1` | normal 선택 |
| `agent_launch.skip_permissions` | `2` | skip permissions 선택 |

### new_session

`sessions.new_shell` 또는 `agent.new_shell`로 열리는 새 세션 모달에서 쓰는 액션입니다. 액션 이름은 기존 설정과의 호환을 위해 `new_shell`로 남아 있지만, 이제는 터미널, `cokacdir`, 새 코딩 에이전트 중 하나를 고르는 모달을 엽니다. 터미널/`cokacdir`은 같은 cwd에 여러 개 열 수 있지만, 코딩에이전트는 같은 cwd에서 동시에 둘 이상 시작하지 않습니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `new_session.cancel` | `esc` | 취소 |
| `new_session.confirm` | `enter` | 선택한 설정으로 시작 |
| `new_session.next` | `down`, `tab` | 다음 입력 항목. Folder 항목에서는 `tab`이 경로 자동완성으로 우선 동작 |
| `new_session.prev` | `up`, `backtab` | 이전 입력 항목. Folder 자동완성 목록이 열려 있으면 `up`/`backtab`은 후보 선택 |
| `new_session.choice_next` | `right`, `space` | Type / Agent / Permissions 다음 값 |
| `new_session.choice_prev` | `left` | Type / Agent / Permissions 이전 값 |
| `new_session.move_left` | `left` | 폴더 경로 커서 왼쪽 이동 |
| `new_session.move_right` | `right` | 폴더 경로 커서 오른쪽 이동 |
| `new_session.backspace` | `backspace` | 폴더 경로에서 커서 앞 글자 삭제 |
| `new_session.delete` | `delete` | 폴더 경로에서 커서 위치 글자 삭제 |
| `new_session.home` | `home` | 폴더 경로 커서 처음으로 |
| `new_session.end` | `end` | 폴더 경로 커서 끝으로 |

Folder 항목의 자동완성은 로컬 디렉터리만 제안합니다. 후보는 exact, 대소문자 무시 exact, prefix, substring, subsequence 순서로 정렬되며, 숨김 디렉터리는 `.`를 직접 입력하기 전까지 뒤로 밀립니다.

폴더 경로 입력 항목에서는 일반 문자 키가 경로 입력으로 우선 처리됩니다. 입력 항목을 이동하려면 `up`, `down`, `tab`, `backtab`을 쓰면 됩니다.

### clone_options

clone 실행 전에 세션만 복제할지, 저장 가능한 폴더 데이터도 함께 복제할지 고르는 버튼 모달에서 쓰는 액션입니다. 폴더 데이터까지 복제하면 원본 cwd와 겹치지 않는 새 전용 cwd를 만들고 새 세션 기록도 그 cwd로 패치하며, 복제본 내용 해시가 원본과 맞을 때만 성공 처리합니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `clone_options.cancel` | `esc` | clone 취소 |
| `clone_options.confirm` | `enter` | 선택한 버튼 실행 |
| `clone_options.next` | `right`, `down` | 다음 버튼 선택 |
| `clone_options.prev` | `left`, `up` | 이전 버튼 선택 |
| `clone_options.target_next` | `tab` | 다음 대상 provider 선택 |
| `clone_options.target_prev` | `backtab` | 이전 대상 provider 선택 |
| `clone_options.session_only` | `1` | Session only 버튼 실행 |
| `clone_options.folder_data` | `2` | Folder data too 버튼 실행 |
| `clone_options.cancel_choice` | `3` | Cancel 버튼 실행 |

## macOS 참고

맥북 내장 키보드에는 전용 `PageUp`, `PageDown`, `Home`, `End` 키가 없습니다. 보통 다음 조합으로 입력합니다.

| 입력 | 의미 |
|---|---|
| `fn+up` | `pageup` |
| `fn+down` | `pagedown` |
| `fn+left` | `home` |
| `fn+right` | `end` |

에이전트 page scroll 기본값은 전용 Page 키가 없어도 쓸 수 있도록 `shift+alt+up/down`을 먼저 제공합니다. 외장 키보드나 `fn+up/down`을 선호하는 환경에서는 `shift+alt+pageup/pagedown`도 같은 동작입니다. Windows Terminal에서 `shift+alt+up/down`이 pane resize로 잡히면 `alt+pageup/pagedown`을 사용하세요.

터미널이나 macOS 단축키 설정에 따라 `ctrl+fn+up/down` 같은 조합이 앱까지 전달되지 않을 수 있습니다. 그런 경우 `agent.switch_prev`, `agent.switch_next`, `agent.scroll_page_up`, `agent.scroll_page_down`을 다른 키로 지정하세요.
