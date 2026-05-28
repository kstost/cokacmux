# cokacmux Keybindings

cokacmux는 `~/.cokacmux/keybinding.json`을 읽어 단축키를 설정합니다. 파일이 없으면 기본 단축키가 모두 들어간 파일을 자동으로 만듭니다.

앱을 재시작하지 않아도 됩니다. 키 입력이 들어올 때마다 파일의 수정 시각만 확인하고, 파일이 바뀐 경우에만 다시 읽어 파싱합니다. 실행 중 파일이 삭제되어도 다음 키 입력 때 기본 파일을 다시 만듭니다. 파싱에 실패하면 기존 단축키를 유지하고 status/debug log에 실패 이유를 남깁니다.

## 설정 방식

원하는 액션만 JSON에 적으면 됩니다. 파일에 없는 액션은 기본값을 그대로 사용합니다.

```json
{
  "sessions": {
    "launch_agent": ["x"],
    "quit": ["q", "ctrl+q"]
  },
  "agent": {
    "scroll_page_up": ["shift+alt+up", "shift+alt+pageup"],
    "scroll_page_down": ["shift+alt+down", "shift+alt+pagedown"],
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

구버전에서 자동 생성된 `agent.scroll_page_up` / `agent.scroll_page_down` 값은 새 기본값으로 자동 갱신됩니다. 직접 바꾼 값은 유지됩니다.

점 표기(flat)도 사용할 수 있습니다.

```json
{
  "sessions.launch_agent": ["x"],
  "agent.scroll_page_down": ["alt+j"]
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

세션 목록/미리보기 화면에서 쓰는 액션입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `sessions.quit` | `q` | 종료 |
| `sessions.force_quit` | `ctrl+c` | 종료 |
| `sessions.toggle_agent` | `ctrl+]`, `ctrl+[`, `ctrl+3`, `ctrl+5` | 세션 화면과 agent 화면 전환 |
| `sessions.kill_agent` | `ctrl+k` | 선택한 실행 중 agent 종료 |
| `sessions.new_shell` | `ctrl+n` | 새 세션 모달 열기 |
| `sessions.toggle_focus` | `tab`, `esc` | 세션 목록과 미리보기 포커스 전환 |
| `sessions.toggle_preview` | `enter` | 미리보기 summary/full 전환 |
| `sessions.move_next` | `down`, `j` | 다음 행 선택 또는 미리보기 아래로 스크롤 |
| `sessions.move_prev` | `up`, `k` | 이전 행 선택 또는 미리보기 위로 스크롤 |
| `sessions.page_next` | `pagedown` | 10행 아래 또는 미리보기 한 페이지 아래 |
| `sessions.page_prev` | `pageup` | 10행 위 또는 미리보기 한 페이지 위 |
| `sessions.top` | `home`, `g` | 처음으로 이동 |
| `sessions.bottom` | `end`, `G` | 끝으로 이동 |
| `sessions.filter` | `/` | 검색창 열기 |
| `sessions.toggle_view` | `v` | tree/list 보기 전환 |
| `sessions.refresh` | `r` | 세션 다시 읽기 |
| `sessions.delete` | `delete`, `d` | 선택 세션 삭제 확인 열기 |
| `sessions.clone` | `c` | 선택 세션 복제 |
| `sessions.edit_title` | `t` | 선택 세션 제목 편집 |
| `sessions.launch_agent` | `e` | agent launch 모드 선택 열기 |
| `sessions.refresh_preview` | `space` | 미리보기 캐시 무시하고 다시 그리기 |
| `sessions.resize_left` | `alt+left`, `ctrl+shift+left` | 세션 패널 좁히기 |
| `sessions.resize_right` | `alt+right`, `ctrl+shift+right` | 세션 패널 넓히기 |
| `sessions.sidebar_prev` | `alt+up`, `ctrl+shift+up` | 세션 목록 선택 위로 이동 |
| `sessions.sidebar_next` | `alt+down`, `ctrl+shift+down` | 세션 목록 선택 아래로 이동 |

### agent

실행 중인 agent 화면에서 쓰는 액션입니다. 여기에 잡히지 않은 키는 active agent PTY로 전달됩니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `agent.toggle_sessions` | `ctrl+]`, `ctrl+[`, `ctrl+3`, `ctrl+5` | 세션 화면으로 전환 |
| `agent.kill` | `ctrl+k` | 현재 agent 종료 |
| `agent.new_shell` | `ctrl+n` | 현재 agent cwd를 기본값으로 새 세션 모달 열기 |
| `agent.toggle_sidebar` | `ctrl+b` | agents 사이드바 표시/숨김 |
| `agent.scroll_line_up` | `shift+up` | PTY scrollback 한 줄 위 |
| `agent.scroll_line_down` | `shift+down` | PTY scrollback 한 줄 아래 |
| `agent.scroll_page_up` | `shift+alt+up`, `shift+alt+pageup` | PTY scrollback 한 페이지 위 |
| `agent.scroll_page_down` | `shift+alt+down`, `shift+alt+pagedown` | PTY scrollback 한 페이지 아래 |
| `agent.scroll_top` | `shift+home`, `alt+home` | PTY scrollback 맨 위 |
| `agent.scroll_bottom` | `shift+end`, `alt+end` | PTY scrollback 맨 아래 |
| `agent.resize_left` | `alt+left`, `ctrl+shift+left` | agents 사이드바 좁히기 |
| `agent.resize_right` | `alt+right`, `ctrl+shift+right` | agents 사이드바 넓히기 |
| `agent.sidebar_prev` | `alt+up`, `ctrl+shift+up` | agents 사이드바 선택 위로 이동 |
| `agent.sidebar_next` | `alt+down`, `ctrl+shift+down` | agents 사이드바 선택 아래로 이동 |
| `agent.switch_prev` | `ctrl+pageup` | 이전 live agent로 전환 |
| `agent.switch_next` | `ctrl+pagedown` | 다음 live agent로 전환 |

### confirm

확인 모달에서 쓰는 액션입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `confirm.yes` | `y`, `Y` | 확인 |
| `confirm.no` | `esc`, `n`, `N` | 취소 |

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

### agent_launch

`sessions.launch_agent`가 아직 실행 중이 아닌 세션에 대해 여는 launch mode 선택 모달에서 쓰는 액션입니다. 선택 세션의 agent가 이미 살아 있으면 이 모달을 거치지 않고 바로 switch/attach 합니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `agent_launch.cancel` | `esc` | 취소 |
| `agent_launch.confirm` | `enter` | 선택한 launch mode로 start/attach |
| `agent_launch.next` | `down`, `j` | 다음 launch mode |
| `agent_launch.prev` | `up`, `k` | 이전 launch mode |
| `agent_launch.normal` | `1` | normal 선택 |
| `agent_launch.skip_permissions` | `2` | skip permissions 선택 |

### new_session

`sessions.new_shell` 또는 `agent.new_shell`로 열리는 새 세션 모달에서 쓰는 액션입니다. 액션 이름은 기존 설정과의 호환을 위해 `new_shell`로 남아 있지만, 이제는 터미널과 새 코딩 에이전트 중 하나를 고르는 모달을 엽니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `new_session.cancel` | `esc` | 취소 |
| `new_session.confirm` | `enter` | 선택한 설정으로 시작 |
| `new_session.next` | `down`, `j`, `tab` | 다음 입력 항목 |
| `new_session.prev` | `up`, `k`, `backtab` | 이전 입력 항목 |
| `new_session.choice_next` | `right`, `l`, `space` | Type / Agent / Permissions 다음 값 |
| `new_session.choice_prev` | `left`, `h` | Type / Agent / Permissions 이전 값 |
| `new_session.move_left` | `left` | 폴더 경로 커서 왼쪽 이동 |
| `new_session.move_right` | `right` | 폴더 경로 커서 오른쪽 이동 |
| `new_session.backspace` | `backspace` | 폴더 경로에서 커서 앞 글자 삭제 |
| `new_session.delete` | `delete` | 폴더 경로에서 커서 위치 글자 삭제 |
| `new_session.home` | `home` | 폴더 경로 커서 처음으로 |
| `new_session.end` | `end` | 폴더 경로 커서 끝으로 |

폴더 경로 입력 항목에서는 일반 문자 키가 경로 입력으로 우선 처리됩니다. 그래서 기본값에 `j`, `k`, `h`, `l`, `space`가 포함되어 있어도 경로 입력 중에는 문자로 들어갑니다. 입력 항목을 이동하려면 `up`, `down`, `tab`, `backtab`을 쓰면 됩니다.

### clone_target

clone target 선택 모달에서 쓰는 액션입니다.

| 액션 | 기본 키 | 설명 |
|---|---|---|
| `clone_target.cancel` | `esc` | 취소 |
| `clone_target.confirm` | `enter` | 선택한 target으로 clone |
| `clone_target.next` | `down`, `j` | 다음 target |
| `clone_target.prev` | `up`, `k` | 이전 target |

## macOS 참고

맥북 내장 키보드에는 전용 `PageUp`, `PageDown`, `Home`, `End` 키가 없습니다. 보통 다음 조합으로 입력합니다.

| 입력 | 의미 |
|---|---|
| `fn+up` | `pageup` |
| `fn+down` | `pagedown` |
| `fn+left` | `home` |
| `fn+right` | `end` |

에이전트 page scroll 기본값은 전용 Page 키가 없어도 쓸 수 있도록 `shift+alt+up/down`을 먼저 제공합니다. 외장 키보드나 `fn+up/down`을 선호하는 환경에서는 `shift+alt+pageup/pagedown`도 같은 동작입니다.

터미널이나 macOS 단축키 설정에 따라 `ctrl+fn+up/down` 같은 조합이 앱까지 전달되지 않을 수 있습니다. 그런 경우 `agent.switch_prev`, `agent.switch_next`, `agent.scroll_page_up`, `agent.scroll_page_down`을 다른 키로 지정하세요.
