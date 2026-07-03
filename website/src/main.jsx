import React from 'react';
import { createRoot } from 'react-dom/client';
import {
  BadgeCheck,
  ClipboardList,
  Copy,
  History,
  Keyboard,
  Layers3,
  Play,
  Search,
  ShieldAlert,
  SplitSquareHorizontal,
  Wand2
} from 'lucide-react';
import heroImage from './assets/cokacmux-hero.png';
import './styles.css';

const quickKeys = [
  ['목록 이동', '↑ / ↓'],
  ['검색', 'Ctrl+F'],
  ['다시 열기', 'e 또는 Enter'],
  ['목록 ↔ 실행 화면', 'Ctrl+] / Ctrl+['],
  ['새 작업', 'Ctrl+N'],
  ['현재 작업 종료', 'Ctrl+K']
];

const chapters = [
  {
    id: 'start',
    eyebrow: '상황 1',
    title: '월요일 아침, 지난주 대화를 다시 찾는다',
    icon: History,
    scene:
      '지난주에 Claude Code와 Codex로 여러 작업을 했는데, 어떤 대화에서 버그 원인을 찾았는지 기억이 흐릿한 상태입니다.',
    steps: [
      {
        key: 'cokacmux',
        action: '터미널에서 `cokacmux`를 실행합니다.',
        why: '여러 코딩 도구가 각자 저장한 대화를 한 화면에서 모아 보기 위해서입니다.'
      },
      {
        key: '↑ / ↓',
        action: '목록을 위아래로 움직이며 제목, 폴더, 시간을 훑습니다.',
        why: '대화를 열기 전에 어느 프로젝트의 어느 기록인지 빠르게 좁힐 수 있습니다.'
      },
      {
        key: 'Tab',
        action: '오른쪽 미리보기로 포커스를 옮겨 내용을 읽습니다.',
        why: '대화를 실제로 재개하기 전에 필요한 내용인지 먼저 확인할 수 있습니다.'
      },
      {
        key: 'Home / End',
        action: '목록이나 미리보기의 처음과 끝으로 이동합니다.',
        why: '오래된 대화와 최신 대화를 빠르게 오가며 후보를 줄일 수 있습니다.'
      }
    ],
    outcome: '대화를 무작정 다시 열지 않고도, 필요한 기록을 먼저 찾고 검토합니다.'
  },
  {
    id: 'search',
    eyebrow: '상황 2',
    title: '기억나는 단어가 있을 때 빠르게 검색한다',
    icon: Search,
    scene:
      '“auth”, “snapshot”, “resume” 같은 단어만 기억나고 정확한 대화 제목은 모르는 상황입니다.',
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
        action: 'AI 검색을 선택하고 “로그인 실패를 고친 대화”처럼 문장으로 적습니다.',
        why: '정확한 단어가 기억나지 않아도 관련 대화를 찾을 수 있습니다.'
      },
      {
        key: 'Esc',
        action: '검색 결과를 지우고 전체 목록으로 돌아갑니다.',
        why: '찾은 뒤에는 다시 전체 작업 흐름을 볼 수 있어야 합니다.'
      }
    ],
    outcome: '기억나는 조각이 적어도 대화를 다시 찾을 수 있습니다.'
  },
  {
    id: 'resume',
    eyebrow: '상황 3',
    title: '대화를 다시 열어 이어서 작업한다',
    icon: Play,
    scene:
      '어제 멈춘 리팩터링 작업을 오늘 이어서 하고 싶습니다. 기존 대화의 맥락을 유지하는 것이 중요합니다.',
    steps: [
      {
        key: 'e / Enter',
        action: '선택한 대화를 다시 여는 창을 엽니다.',
        why: '원래 코딩 도구의 resume 기능으로 같은 대화를 이어가기 위해서입니다.'
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
        action: '작업을 켜 둔 채 대화 목록으로 돌아갑니다.',
        why: 'agent를 종료하지 않고 다른 대화를 찾아볼 수 있습니다.'
      }
    ],
    outcome: '중단했던 작업을 이어가면서도 목록 화면으로 안전하게 돌아올 수 있습니다.'
  },
  {
    id: 'parallel',
    eyebrow: '상황 4',
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
    eyebrow: '상황 5',
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
    eyebrow: '상황 6',
    title: '같은 대화를 복사해 다른 방향을 실험한다',
    icon: Copy,
    scene:
      '한 대화에서는 안정적인 수정만 하고, 다른 대화에서는 더 큰 구조 변경을 실험하고 싶습니다.',
    steps: [
      {
        key: 'c',
        action: '선택한 대화의 복사 옵션창을 엽니다.',
        why: '원래 대화를 건드리지 않고 새 출발점을 만들기 위해서입니다.'
      },
      {
        key: '1',
        action: '대화 기록만 복사합니다.',
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
        why: '예를 들어 Claude에서 하던 대화를 Codex로 이어가며 비교할 수 있습니다.'
      }
    ],
    outcome: '원본을 보존하면서 새 방향의 작업을 안전하게 시도합니다.'
  },
  {
    id: 'organize',
    eyebrow: '상황 7',
    title: '제목을 정리하고 오래된 기록을 치운다',
    icon: ClipboardList,
    scene:
      '프로젝트가 끝난 뒤 나중에 찾기 쉽도록 제목을 정리하고 불필요한 기록을 삭제합니다.',
    steps: [
      {
        key: 't',
        action: '선택한 대화의 제목 편집창을 엽니다.',
        why: '나중에 목록에서 바로 알아볼 수 있는 이름을 붙이기 위해서입니다.'
      },
      {
        key: 'Ctrl+T',
        action: 'AI에게 제목 초안을 만들게 합니다.',
        why: '긴 대화를 직접 읽지 않고도 핵심을 담은 제목을 얻을 수 있습니다.'
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
    outcome: '대화 목록이 다음 작업을 시작하기 쉬운 상태로 정리됩니다.'
  },
  {
    id: 'cleanup',
    eyebrow: '상황 8',
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
  '처음 실행하면 목록에서 대화를 훑고 Tab으로 미리보기를 읽는다.',
  '기억나는 단어가 있으면 Ctrl+F로 검색한다.',
  '대화를 이어갈 때는 e 또는 Enter로 다시 연다.',
  '작업을 켜 둔 채 목록으로 돌아갈 때는 Ctrl+] 또는 Ctrl+[를 쓴다.',
  '오른쪽 pane과 sidebar를 활용해 여러 작업을 동시에 본다.',
  '끝난 작업은 Ctrl+K, 전체 정리는 Ctrl+Shift+K로 확인 후 처리한다.'
];

function App() {
  return (
    <main>
      <section className="hero" aria-labelledby="hero-title">
        <div className="heroCopy">
          <p className="eyebrow">cokacmux tutorial</p>
          <h1 id="hero-title">상황을 따라가며 배우는 cokacmux 사용법</h1>
          <p className="heroLead">
            예전 대화를 찾고, 다시 열고, 여러 코딩 도구를 켜 둔 채 오가는 흐름을
            실제 작업 상황처럼 단계별로 따라갑니다.
          </p>
          <div className="heroActions" aria-label="빠른 이동">
            <a href="#start">
              <Play size={18} />
              처음부터 보기
            </a>
            <a href="#keys">
              <Keyboard size={18} />
              핵심 키 보기
            </a>
          </div>
        </div>
        <div className="heroVisual">
          <img src={heroImage} alt="cokacmux의 세 개 pane UI를 보여주는 대표 이미지" />
        </div>
      </section>

      <section className="overview" id="keys" aria-labelledby="keys-title">
        <div>
          <p className="eyebrow">first 10 minutes</p>
          <h2 id="keys-title">처음 익힐 키는 여섯 개면 충분합니다</h2>
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

      <section className="workflowShell" aria-label="튜토리얼 구성">
        <aside className="scenarioNav">
          <p className="navTitle">상황별 목차</p>
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
