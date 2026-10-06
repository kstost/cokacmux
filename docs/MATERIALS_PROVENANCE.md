# 제작 자료 출처 기록

저장소에서 발견한 외부 제작 자료와 교체 상태, 자체 제작 자료의 제작 방식을 기록한다.

## 교체·제거한 항목 (2026-10-06)

| 경로 | 이전 자료 | 처리 | 제작 방식 |
|---|---|---|---|
| `website/src/icons.jsx` | `lucide-react` 0.468.0 아이콘 14종 (ISC, 외부 아이콘 팩). GitHub 로고 포함 | 의존성 제거(`website/package.json`, `website/package-lock.json`). 아이콘을 저장소 안에서 코드로 새로 그림 | 24x24 격자에 SVG 기본 도형으로 직접 작성. 외부 아이콘·로고의 path 데이터를 복사하거나 트레이싱하지 않음. GitHub 로고 자리는 일반적인 `< / >` 코드 기호로 바꿈 |
| `vendor/vte-cokac/tests/demo.vte` | 원본 vte 저장소의 테스트 픽스처. 저장소 어디서도 참조하지 않음 | 삭제. `vendor/vte-cokac/COKACMUX_PATCHES.md`에 기록 | 해당 없음 |

## 교체 대상으로 남은 항목

| 경로 | 자료 | 사용 위치 | 상태 |
|---|---|---|---|
| `assets/index-C1zrNDW8.js` | 빌드된 웹사이트 번들. 아직 `lucide-react` 아이콘 데이터가 들어 있음 | 루트 `index.html` (cokacmux.cokac.com) | 웹사이트를 다시 빌드해 번들을 교체해야 함. 재빌드 전까지 교체 완료 아님 |
| `docs/assets/cokacmux-og.png` | 홍보(OG) 이미지. 커밋 기록상 이미지 생성 도구로 만든 것으로 보임 | `index.html`, `website/index.html` | 외부 자료라는 증거는 없으나 제작 방식·프롬프트 기록이 없음. 기록 보완 필요 |
| `docs/assets/cokacmux-hero.png` | README 대표 이미지 | `README.md` | 위와 같음 |
| `website/src/styles.css`, `assets/index-B_185JAw.css` | `font-family`에 시스템 글꼴이 아닌 `Inter`를 이름으로 우선 지정. 폰트 파일은 포함하지 않음 | 웹사이트 전체 | 판정 불명. 시스템 글꼴만 쓰려면 이름을 빼야 함 |
