import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import {
  BadgeCheck,
  ClipboardList,
  Copy,
  FolderSearch,
  Github,
  History,
  Keyboard,
  Layers3,
  Play,
  Search,
  ShieldAlert,
  SplitSquareHorizontal,
  TerminalSquare,
  Wand2
} from 'lucide-react';
import { copyText } from './copy-command.js';
import './styles.css';

const quickKeys = [
  ['목록 이동', '↑ / ↓'],
  ['검색', 'Ctrl+F'],
  ['다시 열기', 'e 또는 Enter'],
  ['복제', 'c'],
  ['목록 ↔ 실행 화면', 'Ctrl+] / Ctrl+['],
  ['새 작업', 'Ctrl+N'],
  ['현재 작업 종료', 'Ctrl+K']
];

const valueMoments = [
  {
    icon: ClipboardList,
    title: '작업 세션 자체를 자산으로 남길 때',
    before:
      '멋진 코드는 남지만 그 코드를 얻기 위해 거친 대화와 판단 과정은 흩어집니다.',
    after:
      '세션 안의 대화 내용을 노하우로 보존하고, 다음 작업의 스킬셋처럼 다시 씁니다.'
  },
  {
    icon: Layers3,
    title: '여러 프로젝트의 세션을 한 화면에서 관리할 때',
    before:
      '프로젝트마다 터미널 창과 코딩 도구가 흩어져서 어느 작업이 살아 있는지 확인하기 어렵습니다.',
    after:
      '프로젝트별 세션과 실행 중인 작업을 한 화면에서 보고, 필요한 프로젝트로 바로 돌아갑니다.'
  },
  {
    icon: SplitSquareHorizontal,
    title: '여러 프로젝트를 동시에 작업할 때',
    before:
      '한 프로젝트에서 테스트가 도는 동안 다른 프로젝트를 만지려면 창을 옮겨 다니며 상태를 놓치기 쉽습니다.',
    after:
      '한 작업은 계속 돌려 두고, 다른 agent나 terminal을 열어 병렬로 확인하고 수정합니다.'
  },
  {
    icon: Copy,
    title: '세션을 복제해 다른 방향을 실험할 때',
    before:
      '원래 세션에 바로 이어서 실험하면 안정적인 수정과 큰 구조 변경이 한 흐름에 섞입니다.',
    after:
      '원본 세션을 보존하고, 기록만 복사하거나 작업 폴더까지 복사해 새 방향을 따로 시험합니다.'
  },
  {
    icon: ShieldAlert,
    title: '프로세스를 안전하게 백그라운드에 둘 때',
    before:
      '목록으로 돌아가거나 다른 일을 보려고 하다가 실행 중인 agent나 터미널을 꺼버릴까 봐 조심합니다.',
    after:
      '작업 프로세스는 살아 있게 두고 화면만 전환합니다. 종료할 때도 현재 작업과 전체 작업을 구분해 끕니다.'
  },
  {
    icon: Search,
    title: '예전 세션 맥락을 다시 찾아야 할 때',
    before:
      '어느 도구에서, 어느 폴더에서, 어떤 제목의 세션이었는지 기억을 더듬습니다.',
    after:
      '프로젝트, 시간, 도구를 기준으로 후보를 줄이고, 미리보기로 읽은 뒤 필요한 세션만 다시 엽니다.'
  }
];

const threePromises = [
  '바이브코딩으로 얻은 코드뿐 아니라 그 코드를 만든 과정까지 보존합니다.',
  '세션 안의 대화 노하우를 검색, 미리보기, 복제, 재개로 다시 씁니다.',
  '코딩에이전트를 위해 섬세하게 설계된 Multiplexer처럼 작업을 관리합니다.'
];

const heroFacts = [
  '세션 안의 대화 내용은 그 코드를 얻기 위해 사용한 노하우입니다.',
  'Claude Code, Codex, Pi, OpenCode, GJC 세션을 한눈에 관리합니다.',
  '코딩에이전트를 위해 섬세하게 설계된 Multiplexer입니다.'
];

const installCommands = [
  {
    os: 'macOS / Linux',
    command: 'curl -fsSL https://cokacmux.cokac.com/manage.sh | bash',
    note: '설치가 끝나면 새 터미널을 열고 cokacmux --version으로 확인합니다.'
  },
  {
    os: 'Windows PowerShell',
    command: 'irm https://cokacmux.cokac.com/manage.ps1 | iex',
    note: 'PowerShell에서 실행한 뒤 새 창을 열고 cokacmux --version으로 확인합니다.'
  }
];

const storySteps = [
  {
    time: '오전',
    title: '백엔드 프로젝트에서 세션이 만들어집니다',
    text:
      'Codex agent가 API 테스트를 돌리며 실패 원인과 수정 방향을 대화 안에 쌓고 있습니다.'
  },
  {
    time: '잠시 후',
    title: '프론트엔드 프로젝트도 확인해야 합니다',
    text:
      '터미널 창을 새로 뒤지는 대신 목록으로 돌아와 프론트엔드 프로젝트의 다른 세션을 엽니다.'
  },
  {
    time: 'cokacmux에서',
    title: '두 프로젝트의 세션과 프로세스를 모두 살려 둡니다',
    text:
      '백엔드 테스트는 백그라운드에서 계속 돌고, 프론트엔드 agent에는 새 지시를 보냅니다.'
  },
  {
    time: '다음 날',
    title: '코드만이 아니라 작업 맥락까지 다시 씁니다',
    text:
      '목록에서 어제의 세션을 찾아 미리보기로 확인하고, 그대로 이어 열거나 복제해 새 실험을 시작합니다.'
  }
];

const startRoutes = [
  {
    title: '아직 설치 전이라면',
    label: '설치부터',
    detail:
      '먼저 README의 설치 명령을 실행한 뒤 새 터미널을 엽니다. 설치가 끝난 뒤에는 버전 확인만 하면 됩니다.',
    key: 'cokacmux --version'
  },
  {
    title: '이미 설치했다면',
    label: '상태 확인부터',
    detail:
      '바로 앱을 열기 전에 어떤 코딩 도구의 세션을 읽을 수 있는지 확인합니다. 처음 실행 전 불안감을 줄이는 단계입니다.',
    key: 'cokacmux --check'
  },
  {
    title: '세션이 보인다면',
    label: '읽기부터',
    detail:
      '처음에는 복사나 삭제를 하지 말고, 목록 이동과 미리보기만 익힙니다. 필요한 세션인지 판단하는 감각이 먼저입니다.',
    key: '↑ / ↓, Tab'
  }
];

const primerItems = [
  {
    icon: Layers3,
    title: 'cokacmux는 무엇인가요?',
    text:
      'Claude Code, Codex, Pi, OpenCode, GJC와 터미널 작업에서 생긴 세션을 여러 프로젝트에 걸쳐 관리하는 터미널 앱입니다. 코딩에이전트를 위해 섬세하게 설계된 Multiplexer라고 이해하면 쉽습니다.'
  },
  {
    icon: FolderSearch,
    title: '언제 필요해지나요?',
    text:
      '여러 프로젝트를 동시에 수정하거나, 한 프로젝트에서 테스트를 돌려 둔 채 다른 프로젝트를 확인하거나, 예전 세션을 다시 찾아 이어가고 싶을 때 씁니다.'
  },
  {
    icon: TerminalSquare,
    title: '처음에는 무엇을 보나요?',
    text:
      '왼쪽에는 프로젝트별 세션과 실행 중인 작업이 보이고, 오른쪽에는 선택한 세션의 내용이 보입니다. 먼저 어떤 맥락과 프로세스가 살아 있는지 훑어보면 됩니다.'
  }
];

const firstRunSteps = [
  ['1', 'cokacmux 실행', '터미널에서 앱을 열면 기존 코딩 도구들이 저장한 세션 목록을 읽어 옵니다.'],
  ['2', '왼쪽 목록 확인', '도구, 제목, 시간, 작업 폴더를 보며 어떤 작업 세션인지 먼저 구분합니다.'],
  ['3', '오른쪽 미리보기 읽기', 'Tab으로 포커스를 옮겨 세션 내용을 열기 전에 확인합니다.'],
  ['4', '필요할 때만 다시 열기', 'e 또는 Enter로 원래 코딩 도구의 세션을 이어서 실행합니다.']
];

const screenParts = [
  {
    icon: TerminalSquare,
    name: '터미널 안에서 열리는 화면',
    meaning:
      'cokacmux는 웹 브라우저가 아니라 터미널에서 움직입니다. 글자와 키보드만으로 조작하는 작업 관리 화면이라고 생각하면 됩니다.',
    action:
      '처음에는 마우스를 찾지 말고 키보드의 위아래 화살표, Tab, Enter, Esc만 기억하면 됩니다.'
  },
  {
    icon: ClipboardList,
    name: '왼쪽 세션 목록',
    meaning:
      'Claude Code, Codex 같은 코딩 도구들이 만든 작업 세션을 줄 단위로 보여줍니다. 한 줄이 하나의 되돌아갈 수 있는 맥락입니다.',
    action:
      '위아래 화살표로 줄을 옮기며 제목, 시간, 작업 폴더를 훑습니다. 지금 밝게 표시된 줄이 선택된 세션입니다.'
  },
  {
    icon: FolderSearch,
    name: '목록의 칸들',
    meaning:
      '상태는 실행 중인지, 도구는 어느 AI 코딩 도구의 세션인지, 제목은 사람이 알아보기 쉬운 이름, 폴더는 그 세션이 작업하던 위치입니다.',
    action:
      '제목만 보지 말고 폴더와 시간을 같이 봅니다. 비슷한 제목이 많을 때는 폴더가 가장 좋은 단서가 됩니다.'
  },
  {
    icon: History,
    name: '오른쪽 미리보기',
    meaning:
      '선택한 세션의 내용을 읽기 전용으로 보여주는 공간입니다. 여기서 읽는다고 세션이 다시 시작되지는 않습니다.',
    action:
      'Tab을 눌러 오른쪽으로 이동한 뒤 위아래로 읽습니다. 필요한 기록이 맞는지 확인한 다음에만 다시 엽니다.'
  },
  {
    icon: Keyboard,
    name: '하단 단축키 안내',
    meaning:
      '현재 화면에서 바로 쓸 수 있는 키를 짧게 보여주는 안내줄입니다. 화면이 바뀌면 안내되는 키도 바뀝니다.',
    action:
      '모든 단축키를 외우려 하지 말고, 아래 안내줄에서 지금 필요한 키만 확인합니다. 막히면 Esc로 한 단계 빠져나옵니다.'
  },
  {
    icon: Play,
    name: '다시 열기와 실행 화면',
    meaning:
      '세션을 다시 열면 원래 코딩 도구가 실제로 실행됩니다. 이때부터는 AI에게 새 질문을 하거나 이어서 작업할 수 있습니다.',
    action:
      'e 또는 Enter로 열고, 목록으로 돌아오고 싶으면 Ctrl+] 또는 Ctrl+[를 누릅니다. 돌아와도 실행 중인 작업은 꺼지지 않습니다.'
  }
];

const beginnerSteps = [
  {
    title: '터미널을 엽니다',
    detail:
      'macOS와 Linux에서는 Terminal을, Windows에서는 PowerShell을 열면 됩니다. cokacmux는 이 창 안에서 실행됩니다.',
    key: '터미널'
  },
  {
    title: '설치가 되었는지 확인합니다',
    detail:
      'cokacmux --version을 입력했을 때 버전 번호가 나오면 준비가 된 상태입니다. 아무 반응이 없으면 설치부터 다시 확인합니다.',
    key: 'cokacmux --version'
  },
  {
    title: '세션이 읽히는지 점검합니다',
    detail:
      'cokacmux --check는 어떤 코딩 도구의 세션을 찾을 수 있는지 확인하는 명령입니다. 처음이라면 이 명령으로 상태를 먼저 보는 것이 좋습니다.',
    key: 'cokacmux --check'
  },
  {
    title: '앱을 실행합니다',
    detail:
      'cokacmux를 입력하면 세션 목록 화면이 열립니다. 이 화면이 앞으로 가장 자주 돌아오게 될 시작점입니다.',
    key: 'cokacmux'
  },
  {
    title: '가장 밝게 표시된 줄을 봅니다',
    detail:
      '밝게 표시된 줄이 현재 선택된 세션입니다. 오른쪽 미리보기는 항상 이 선택 줄을 따라 바뀝니다.',
    key: '선택 줄'
  },
  {
    title: '위아래로 천천히 움직입니다',
    detail:
      '처음에는 열려고 하지 말고 제목, 도구, 시간, 폴더가 어떻게 바뀌는지만 봅니다. 목록이 무엇을 뜻하는지 감이 생깁니다.',
    key: '↑ / ↓'
  },
  {
    title: '오른쪽 내용을 읽어 봅니다',
    detail:
      'Tab을 누르면 오른쪽 미리보기로 이동합니다. 여기서는 세션 내용만 읽는 단계라서 실수로 AI 작업이 시작되지 않습니다.',
    key: 'Tab'
  },
  {
    title: '필요한 세션이 맞는지 판단합니다',
    detail:
      '내가 찾던 파일명, 에러 메시지, 작업 내용이 보이면 맞는 세션일 가능성이 큽니다. 아니면 Tab으로 목록에 돌아가 다른 줄을 고릅니다.',
    key: 'Tab'
  },
  {
    title: '찾기 어려우면 검색합니다',
    detail:
      'Ctrl+F를 누르면 검색 방식을 고를 수 있습니다. 파일명처럼 정확한 단어가 있으면 글자 검색, 기억이 흐릿하면 AI 검색을 씁니다.',
    key: 'Ctrl+F'
  },
  {
    title: '정말 이어갈 때만 다시 엽니다',
    detail:
      'e 또는 Enter를 누르면 원래 코딩 도구로 세션을 이어서 엽니다. 단순히 읽기만 할 때는 미리보기에서 멈추면 됩니다.',
    key: 'e / Enter'
  }
];

const chapters = [
  {
    id: 'start',
    eyebrow: '실습 1',
    title: '지난 세션을 찾아 미리보기로 확인한다',
    icon: History,
    scene:
      'cokacmux가 무엇을 보여주는지 알았다면, 이제 저장된 세션 목록에서 필요한 작업 맥락을 찾는 흐름을 익힙니다.',
    steps: [
      {
        key: 'cokacmux',
        action: '터미널에서 `cokacmux`를 실행합니다.',
        why: '여러 코딩 도구가 각자 저장한 세션을 한 화면에서 모아 보기 위해서입니다.'
      },
      {
        key: '↑ / ↓',
        action: '목록을 위아래로 움직이며 제목, 폴더, 시간을 훑습니다.',
        why: '세션을 열기 전에 어느 프로젝트의 어느 기록인지 빠르게 좁힐 수 있습니다.'
      },
      {
        key: 'Tab',
        action: '오른쪽 미리보기로 포커스를 옮겨 내용을 읽습니다.',
        why: '세션을 실제로 재개하기 전에 필요한 내용인지 먼저 확인할 수 있습니다.'
      },
      {
        key: 'Home / End',
        action: '목록이나 미리보기의 처음과 끝으로 이동합니다.',
        why: '오래된 세션과 최신 세션을 빠르게 오가며 후보를 줄일 수 있습니다.'
      }
    ],
    outcome: '세션을 무작정 다시 열지 않고도, 필요한 맥락을 먼저 찾고 검토합니다.'
  },
  {
    id: 'search',
    eyebrow: '실습 2',
    title: '기억나는 단어가 있을 때 빠르게 검색한다',
    icon: Search,
    scene:
      '“auth”, “snapshot”, “resume” 같은 단어만 기억나고 정확한 세션 제목은 모르는 상황입니다.',
    steps: [
      {
        key: 'Ctrl+F',
        action: '검색 방식 선택창을 엽니다.',
        why: '글자로 직접 찾을지, AI에게 의미로 찾아 달라고 할지 고를 수 있습니다.'
      },
      {
        key: '1 또는 Enter',
        action: '글자 검색을 선택하고 검색어를 입력합니다.',
        why: '정확한 함수명, 파일명, 에러 메시지를 알고 있을 때 가장 빠릅니다.'
      },
      {
        key: '2',
        action: 'AI 검색을 선택하고 “로그인 실패를 고친 세션”처럼 문장으로 적습니다.',
        why: '정확한 단어가 기억나지 않아도 관련 세션을 찾을 수 있습니다.'
      },
      {
        key: 'Esc',
        action: '검색 결과를 지우고 전체 목록으로 돌아갑니다.',
        why: '찾은 뒤에는 다시 전체 작업 흐름을 볼 수 있어야 합니다.'
      }
    ],
    outcome: '기억나는 조각이 적어도 작업 세션을 다시 찾을 수 있습니다.'
  },
  {
    id: 'resume',
    eyebrow: '실습 3',
    title: '세션을 다시 열어 이어서 작업한다',
    icon: Play,
    scene:
      '어제 멈춘 리팩터링 작업을 오늘 이어서 하고 싶습니다. 기존 세션의 맥락을 유지하는 것이 중요합니다.',
    steps: [
      {
        key: 'e / Enter',
        action: '선택한 세션을 다시 여는 창을 엽니다.',
        why: '원래 코딩 도구의 resume 기능으로 같은 맥락을 이어가기 위해서입니다.'
      },
      {
        key: '1',
        action: 'Normal 실행을 선택합니다.',
        why: '보통은 확인 절차를 유지하는 방식이 안전합니다.'
      },
      {
        key: '2',
        action: '정말 필요한 경우에만 Skip permissions를 선택합니다.',
        why: '확인 질문을 줄이는 대신 파일 수정이나 명령 실행 위험이 커질 수 있습니다.'
      },
      {
        key: 'Ctrl+] / Ctrl+[',
        action: '작업을 켜 둔 채 세션 목록으로 돌아갑니다.',
        why: 'agent를 종료하지 않고 다른 세션을 찾아볼 수 있습니다.'
      }
    ],
    outcome: '중단했던 작업을 이어가면서도 목록 화면으로 안전하게 돌아올 수 있습니다.'
  },
  {
    id: 'parallel',
    eyebrow: '실습 4',
    title: '한 작업은 켜 두고, 다른 작업을 나란히 연다',
    icon: SplitSquareHorizontal,
    scene:
      '메인 agent가 테스트를 돌리는 동안, 오른쪽에는 명령창을 열어 로그를 보고 싶습니다.',
    steps: [
      {
        key: 'Ctrl+N',
        action: '새 terminal, cokacdir, coding agent 중 하나를 새로 엽니다.',
        why: '현재 작업을 끄지 않고 새로운 작업 공간을 만들 수 있습니다.'
      },
      {
        key: 'Ctrl+T',
        action: '오른쪽에 일반 terminal pane을 붙입니다.',
        why: '테스트, 로그, git 명령처럼 짧은 확인 작업을 옆에서 처리하기 좋습니다.'
      },
      {
        key: 'Ctrl+F',
        action: '오른쪽에 cokacdir pane을 붙입니다.',
        why: '프로젝트 폴더를 둘러보며 agent 작업과 비교할 때 유용합니다.'
      },
      {
        key: 'Ctrl+1 / Ctrl+2 / Ctrl+3',
        action: '왼쪽 목록, 메인 agent, 오른쪽 pane으로 포커스를 직접 옮깁니다.',
        why: '어느 화면에 키 입력이 들어가는지 헷갈리지 않게 합니다.'
      }
    ],
    outcome: '작업을 멈추지 않고 여러 화면을 한곳에서 관리합니다.'
  },
  {
    id: 'switch',
    eyebrow: '실습 5',
    title: '켜 둔 작업 사이를 빠르게 오간다',
    icon: Layers3,
    scene:
      '프론트엔드 수정 agent, 백엔드 테스트 terminal, 문서 정리 agent를 모두 켜 둔 상태입니다.',
    steps: [
      {
        key: 'Ctrl+B',
        action: '왼쪽 agents sidebar를 보이거나 숨깁니다.',
        why: '작업 목록을 확인할 때는 보이고, 집중할 때는 공간을 넓게 쓸 수 있습니다.'
      },
      {
        key: 'Alt+↑ / Alt+↓',
        action: '이전 또는 다음 실행 작업으로 이동합니다.',
        why: '목록 화면으로 돌아가지 않아도 빠르게 작업을 바꿀 수 있습니다.'
      },
      {
        key: 'Ctrl+PageUp / Ctrl+PageDown',
        action: '실행 화면을 순서대로 전환합니다.',
        why: '여러 agent를 탭처럼 오가고 싶을 때 편합니다.'
      },
      {
        key: 'Ctrl+. / Ctrl+/',
        action: 'sidebar, main, right pane 사이에서 포커스를 순환합니다.',
        why: '마우스 없이도 키 입력 대상 pane을 바꿀 수 있습니다.'
      }
    ],
    outcome: '여러 작업을 켜 둔 상태에서도 현재 보고 싶은 작업으로 바로 이동합니다.'
  },
  {
    id: 'clone',
    eyebrow: '실습 6',
    title: '같은 세션을 복사해 다른 방향을 실험한다',
    icon: Copy,
    scene:
      '한 세션에서는 안정적인 수정만 하고, 다른 세션에서는 더 큰 구조 변경을 실험하고 싶습니다.',
    steps: [
      {
        key: 'c',
        action: '선택한 세션의 복사 옵션창을 엽니다.',
        why: '원래 세션을 건드리지 않고 새 출발점을 만들기 위해서입니다.'
      },
      {
        key: '1',
        action: '세션 기록만 복사합니다.',
        why: '같은 맥락만 이어가고 작업 폴더는 그대로 쓰고 싶을 때 좋습니다.'
      },
      {
        key: '2',
        action: '가능하면 작업 폴더까지 함께 복사합니다.',
        why: '실험적인 변경이 원래 폴더에 섞이지 않게 할 수 있습니다.'
      },
      {
        key: 'Tab / BackTab',
        action: '복사 대상 코딩 도구를 바꿉니다.',
        why: '예를 들어 Claude에서 하던 세션을 Codex로 이어가며 비교할 수 있습니다.'
      }
    ],
    outcome: '원본을 보존하면서 새 방향의 작업을 안전하게 시도합니다.'
  },
  {
    id: 'organize',
    eyebrow: '실습 7',
    title: '제목을 정리하고 오래된 기록을 치운다',
    icon: ClipboardList,
    scene:
      '프로젝트가 끝난 뒤 나중에 찾기 쉽도록 세션 제목을 정리하고 불필요한 기록을 삭제합니다.',
    steps: [
      {
        key: 't',
        action: '선택한 세션의 제목 편집창을 엽니다.',
        why: '나중에 목록에서 바로 알아볼 수 있는 이름을 붙이기 위해서입니다.'
      },
      {
        key: 'Ctrl+T',
        action: 'AI에게 제목 초안을 만들게 합니다.',
        why: '긴 세션을 직접 읽지 않고도 핵심을 담은 제목을 얻을 수 있습니다.'
      },
      {
        key: 'Delete / d',
        action: '삭제 확인창을 엽니다.',
        why: '필요 없는 기록을 정리하되, 실수로 바로 지우지 않게 한 번 더 확인합니다.'
      },
      {
        key: 'r',
        action: '목록을 다시 읽습니다.',
        why: '밖에서 대화 파일이 바뀐 뒤 최신 상태를 확인할 수 있습니다.'
      }
    ],
    outcome: '세션 목록이 다음 작업을 시작하기 쉬운 상태로 정리됩니다.'
  },
  {
    id: 'cleanup',
    eyebrow: '실습 8',
    title: '하루가 끝나면 실행 중인 작업을 안전하게 종료한다',
    icon: ShieldAlert,
    scene:
      '여러 agent와 terminal을 켜 둔 채 퇴근하기 전에, 어떤 것은 끄고 어떤 것은 남길지 정해야 합니다.',
    steps: [
      {
        key: 'Ctrl+K',
        action: '현재 선택한 작업 하나만 종료합니다.',
        why: '끝난 작업만 조용히 정리하고 다른 작업은 계속 둘 수 있습니다.'
      },
      {
        key: 'Ctrl+Shift+K',
        action: '전체 종료 확인창을 엽니다.',
        why: '모든 agent와 terminal을 한 번에 정리할 때 사용합니다.'
      },
      {
        key: 'Esc',
        action: '확인창에서 취소합니다.',
        why: '실수로 전체 종료를 누른 경우 안전하게 빠져나옵니다.'
      },
      {
        key: 'q / Ctrl+Q',
        action: 'cokacmux 화면을 닫습니다.',
        why: '앱만 닫는 동작입니다. 실행 중인 agent를 모두 끄는 동작과는 다릅니다.'
      }
    ],
    outcome: '앱 종료와 agent 종료의 차이를 이해하고 안전하게 정리합니다.'
  }
];

const checklist = [
  '처음 실행하면 목록에서 세션을 훑고 Tab으로 미리보기를 읽는다.',
  '기억나는 단어가 있으면 Ctrl+F로 검색한다.',
  '세션을 이어갈 때는 e 또는 Enter로 다시 연다.',
  '다른 방향을 실험하기 전에는 c로 세션을 복제한다.',
  '작업을 켜 둔 채 목록으로 돌아갈 때는 Ctrl+] 또는 Ctrl+[를 쓴다.',
  '오른쪽 pane과 sidebar를 활용해 여러 작업을 동시에 본다.',
  '끝난 작업은 Ctrl+K, 전체 정리는 Ctrl+Shift+K로 확인 후 처리한다.'
];

function App() {
  return (
    <main>
      <section className="hero" aria-labelledby="hero-title">
        <div className="heroCopy">
          <p className="eyebrow">cokacmux overview</p>
          <h1 id="hero-title">코딩에이전트를 위해 섬세하게 설계된 Multiplexer, cokacmux.</h1>
          <p className="heroLead">
            바이브코딩으로 멋진 코드를 얻었다면, 그 코드를 얻기까지 거친 세션 안의
            대화도 놓치면 안 됩니다. cokacmux는 Claude Code, Codex, Pi, OpenCode, GJC
            같은 코딩 에이전트의 세션을 한눈에 관리하고 다시 사용할 수 있게 합니다.
          </p>
          <ul className="heroFacts" aria-label="cokacmux 핵심 요약">
            {heroFacts.map((fact) => (
              <li key={fact}>
                <BadgeCheck size={18} />
                <span>{fact}</span>
              </li>
            ))}
          </ul>
          <div className="heroActions" aria-label="빠른 이동">
            <a href="#why">
              <Search size={18} />
              왜 쓰는지 보기
            </a>
            <a href="#example">
              <History size={18} />
              실제 예시
            </a>
            <a href="#what-is">
              <Play size={18} />
              먼저 이해하기
            </a>
            <a href="#screen-tour">
              <Layers3 size={18} />
              첫 화면 해설
            </a>
            <a href="#first-steps">
              <Keyboard size={18} />
              10단계 따라하기
            </a>
            <a href="https://github.com/kstost/cokacmux/" target="_blank" rel="noreferrer">
              <Github size={18} />
              GitHub
            </a>
          </div>
        </div>
      </section>

      <section className="installSection" id="install" aria-labelledby="install-title">
        <div className="sectionIntro">
          <p className="eyebrow">install</p>
          <h2 id="install-title">설치는 한 줄이면 됩니다</h2>
          <p>
            내 운영체제에 맞는 명령어를 복사해서 터미널이나 PowerShell에 붙여 넣으면 됩니다.
            설치 뒤에는 새 창을 열어 버전을 확인하세요.
          </p>
        </div>

        <div className="installGrid">
          {installCommands.map((item) => (
            <article className="installCard" key={item.os}>
              <h3>{item.os}</h3>
              <div className="commandLine">
                <code>{item.command}</code>
                <CopyCommandButton command={item.command} />
              </div>
              <p>{item.note}</p>
            </article>
          ))}
        </div>
      </section>

      <section className="valueSection" id="why" aria-labelledby="why-title">
        <div className="sectionIntro">
          <p className="eyebrow">why it matters</p>
          <h2 id="why-title">좋은 점은 작업의 맥락까지 잃지 않는 것입니다</h2>
          <p>
            AI 코딩을 오래 쓰면 프로젝트마다 agent, terminal, 테스트, 세션이 동시에
            생깁니다. 중요한 것은 최종 코드만이 아니라 그 코드를 얻기 위해 거쳤던
            대화 과정입니다. cokacmux는 그 세션들을 찾고, 읽고, 복제하고, 이어 열 수
            있게 관리합니다.
          </p>
        </div>

        <div className="promiseStrip" aria-label="cokacmux가 줄여주는 일">
          {threePromises.map((promise) => (
            <div className="promiseItem" key={promise}>
              <BadgeCheck size={19} />
              <span>{promise}</span>
            </div>
          ))}
        </div>

        <div className="valueGrid">
          {valueMoments.map((moment) => {
            const Icon = moment.icon;
            return (
              <article className="valueCard" key={moment.title}>
                <Icon size={24} />
                <h3>{moment.title}</h3>
                <div className="beforeAfter">
                  <p>
                    <b>없으면</b>
                    {moment.before}
                  </p>
                  <p>
                    <b>있으면</b>
                    {moment.after}
                  </p>
                </div>
              </article>
            );
          })}
        </div>
      </section>

      <section className="storySection" id="example" aria-labelledby="example-title">
        <div className="sectionIntro">
          <p className="eyebrow">one real example</p>
          <h2 id="example-title">예를 들면, 세션을 이렇게 자산처럼 씁니다</h2>
          <p>
            cokacmux의 장점은 한 프로젝트를 멈추지 않고 다른 프로젝트로 넘어가면서도,
            각 작업의 판단 과정과 실행 상태를 다시 쓸 수 있는 형태로 남길 때 드러납니다.
          </p>
        </div>

        <ol className="storySteps">
          {storySteps.map((step, index) => (
            <li key={step.title}>
              <div className="storyIndex">{String(index + 1).padStart(2, '0')}</div>
              <div>
                <span>{step.time}</span>
                <h3>{step.title}</h3>
                <p>{step.text}</p>
              </div>
            </li>
          ))}
        </ol>
      </section>

      <section className="startChoice" id="start-choice" aria-labelledby="start-choice-title">
        <div className="sectionIntro">
          <p className="eyebrow">where to start</p>
          <h2 id="start-choice-title">내가 어디서 시작해야 하는지도 나눠 둡니다</h2>
          <p>
            처음 보는 사람이 가장 자주 막히는 지점은 “설치해야 하나, 바로 실행해야 하나,
            뭘 누르면 위험한가”입니다. 아래 순서대로 들어오면 됩니다.
          </p>
        </div>

        <div className="routeGrid">
          {startRoutes.map((route) => (
            <article className="routeCard" key={route.title}>
              <span>{route.label}</span>
              <h3>{route.title}</h3>
              <p>{route.detail}</p>
              <kbd>{route.key}</kbd>
            </article>
          ))}
        </div>
      </section>

      <section className="primer" id="what-is" aria-labelledby="what-is-title">
        <div className="sectionIntro">
          <p className="eyebrow">before scenarios</p>
          <h2 id="what-is-title">먼저, 이 앱이 하는 일을 잡고 갑니다</h2>
          <p>
            처음 보는 사람에게 중요한 것은 단축키가 아니라 화면의 역할입니다. cokacmux는
            세션 목록을 찾는 화면과 실제 agent를 실행하는 화면을 오가며 쓰는 앱입니다.
          </p>
        </div>

        <div className="primerGrid">
          {primerItems.map((item) => {
            const Icon = item.icon;
            return (
              <article className="primerCard" key={item.title}>
                <Icon size={24} />
                <h3>{item.title}</h3>
                <p>{item.text}</p>
              </article>
            );
          })}
        </div>

        <div className="firstRun">
          <div>
            <p className="eyebrow">first run</p>
            <h2>처음 켰을 때의 흐름</h2>
          </div>
          <ol>
            {firstRunSteps.map(([number, title, text]) => (
              <li key={number}>
                <strong>{number}</strong>
                <span>
                  <b>{title}</b>
                  {text}
                </span>
              </li>
            ))}
          </ol>
        </div>
      </section>

      <section className="screenTour" id="screen-tour" aria-labelledby="screen-tour-title">
        <div className="sectionIntro">
          <p className="eyebrow">first screen tour</p>
          <h2 id="screen-tour-title">처음 화면에서 보이는 것을 하나씩 해석합니다</h2>
          <p>
            처음에는 “어떤 키를 눌러야 하지?”보다 “내가 지금 어떤 세션을 보고 있지?”가
            먼저입니다. 아래 모형은 cokacmux를 켰을 때 마주치는 기본 화면을 단순화한
            것입니다.
          </p>
        </div>

        <div className="screenTourGrid">
          <div className="terminalMock" aria-label="cokacmux 첫 화면 구조 예시">
            <div className="mockTopBar">
              <span>cokacmux</span>
              <span>Sessions</span>
              <span>3 running</span>
            </div>
            <div className="mockBody">
              <div className="mockList">
                <div className="mockPanelTitle">왼쪽: 세션 목록</div>
                <div className="mockRow active">
                  <span>●</span>
                  <span>Codex</span>
                  <strong>README 정리</strong>
                </div>
                <div className="mockRow">
                  <span>○</span>
                  <span>Claude</span>
                  <strong>로그인 오류 수정</strong>
                </div>
                <div className="mockRow">
                  <span>○</span>
                  <span>OpenCode</span>
                  <strong>테스트 실패 확인</strong>
                </div>
              </div>
              <div className="mockPreview">
                <div className="mockPanelTitle">오른쪽: 선택한 세션 미리보기</div>
                <p>선택한 세션에서 어떤 작업을 했는지 읽어보는 곳입니다.</p>
                <p>여기서 읽기만 해도 원래 AI 세션이 다시 시작되지는 않습니다.</p>
              </div>
            </div>
            <div className="mockHelp">
              <span>↑↓ 이동</span>
              <span>Tab 미리보기</span>
              <span>Ctrl+F 검색</span>
              <span>c 복제</span>
              <span>e 이어 열기</span>
              <span>Esc 닫기</span>
            </div>
          </div>

          <div className="tourList">
            {screenParts.map((part) => {
              const Icon = part.icon;
              return (
                <article className="tourItem" key={part.name}>
                  <Icon size={22} />
                  <div>
                    <h3>{part.name}</h3>
                    <p>{part.meaning}</p>
                    <strong>{part.action}</strong>
                  </div>
                </article>
              );
            })}
          </div>
        </div>
      </section>

      <section className="overview" id="keys" aria-labelledby="keys-title">
        <div>
          <p className="eyebrow">first 10 minutes</p>
          <h2 id="keys-title">화면 구조를 이해한 뒤에는 기본 키만 익히면 됩니다</h2>
        </div>
        <div className="keyGrid">
          {quickKeys.map(([label, key]) => (
            <div className="keyCard" key={label}>
              <span>{label}</span>
              <kbd>{key}</kbd>
            </div>
          ))}
        </div>
      </section>

      <section className="beginnerGuide" id="first-steps" aria-labelledby="first-steps-title">
        <div className="sectionIntro">
          <p className="eyebrow">step by step</p>
          <h2 id="first-steps-title">처음부터 열 단계로 따라갑니다</h2>
          <p>
            아래 순서는 cokacmux를 처음 설치한 사람이 실제로 해볼 만한 가장 안전한
            흐름입니다. 삭제, 복사, 위험한 실행은 나중으로 미루고 먼저 읽고 찾는 방법부터
            익힙니다.
          </p>
        </div>

        <ol className="beginnerSteps">
          {beginnerSteps.map((step, index) => (
            <li key={step.title}>
              <div className="guideNumber">{String(index + 1).padStart(2, '0')}</div>
              <div className="guideBody">
                <kbd>{step.key}</kbd>
                <h3>{step.title}</h3>
                <p>{step.detail}</p>
              </div>
            </li>
          ))}
        </ol>
      </section>

      <section className="workflowShell" id="practice" aria-label="튜토리얼 구성">
        <aside className="scenarioNav">
          <p className="navTitle">실습 목차</p>
          {chapters.map((chapter) => (
            <a href={`#${chapter.id}`} key={chapter.id}>
              {chapter.eyebrow}
              <strong>{chapter.title}</strong>
            </a>
          ))}
        </aside>

        <div className="chapters">
          {chapters.map((chapter, index) => (
            <TutorialChapter chapter={chapter} index={index} key={chapter.id} />
          ))}
        </div>
      </section>

      <section className="finish" aria-labelledby="finish-title">
        <div>
          <p className="eyebrow">practice checklist</p>
          <h2 id="finish-title">이 순서대로 한 번만 따라 해보세요</h2>
        </div>
        <ol>
          {checklist.map((item) => (
            <li key={item}>
              <BadgeCheck size={20} />
              <span>{item}</span>
            </li>
          ))}
        </ol>
      </section>
    </main>
  );
}

function CopyCommandButton({ command }) {
  const [copied, setCopied] = useState(false);

  const copyCommand = async () => {
    try {
      await copyText(command);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch {
      setCopied(false);
    }
  };

  return (
    <button type="button" className="copyCommand" onClick={copyCommand}>
      <Copy size={17} />
      {copied ? '복사됨' : '복사'}
    </button>
  );
}

function TutorialChapter({ chapter, index }) {
  const Icon = chapter.icon;
  return (
    <article className="chapter" id={chapter.id}>
      <header className="chapterHeader">
        <div className="chapterIcon" aria-hidden="true">
          <Icon size={28} />
        </div>
        <div>
          <p className="eyebrow">{chapter.eyebrow}</p>
          <h2>
            <span>{String(index + 1).padStart(2, '0')}</span>
            {chapter.title}
          </h2>
          <p>{chapter.scene}</p>
        </div>
      </header>

      <div className="steps">
        {chapter.steps.map((step, stepIndex) => (
          <div className="step" key={`${chapter.id}-${step.key}`}>
            <div className="stepNumber">{stepIndex + 1}</div>
            <div className="stepBody">
              <kbd>{step.key}</kbd>
              <h3>{step.action}</h3>
              <p>{step.why}</p>
            </div>
          </div>
        ))}
      </div>

      <footer className="outcome">
        <Wand2 size={18} />
        <span>{chapter.outcome}</span>
      </footer>
    </article>
  );
}

createRoot(document.getElementById('root')).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
