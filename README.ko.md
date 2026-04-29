<h1 align="center">ymux</h1>

<p align="center">
  <a href="./README.md">English</a> &nbsp;·&nbsp; <strong>한국어</strong> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.8.2-7fdbca?style=flat-square" alt="version 0.7.0" />
</p>

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Ko-fi로 후원하기" />
  </a>
</p>

---

Windows용 경량 tmux 스타일 터미널 멀티플렉서.

https://github.com/user-attachments/assets/705fff59-0bda-4460-a87f-d7ba6f50993a

Tauri 2 (Rust) + WebView2 + xterm.js 로 만들어졌습니다. Windows에서 가볍고 빠르게
네이티브로 동작하면서, 레이아웃 저장, pane별 작업 디렉터리와 실행 명령,
여러 셸 선택 (cmd / PowerShell / pwsh / Git Bash / WSL), 그리고 각자 자신만의
레이아웃을 기억하는 번호 매겨진 워크스페이스를 제공합니다.

## 기능

- **저장되는 레이아웃**: 재귀적 가로 / 세로 분할. 각 pane은 자신의 셸, `cwd`,
  선택적 시작 명령을 기억합니다.
- **현재 경로 계승**: pane을 분할하면 부모 셸이 현재 있는 경로에서 새 pane이
  열립니다. 처음 시작 경로가 아니라 실시간으로 추적된 경로를 계승합니다.
  OSC 7 이스케이프 시퀀스 추적 방식을 사용합니다.
- **셸 자동 감지**: `cmd.exe`, Windows PowerShell, PowerShell 7 (`pwsh`),
  Git Bash, WSL 배포판을 시스템에서 찾아내서 선택 가능한 프로필로 노출합니다.
- **번호 매겨진 워크스페이스**: `Ctrl+Alt+1` .. `Ctrl+Alt+9` 로 워크스페이스
  사이를 전환합니다. 모든 워크스페이스는 자신만의 레이아웃을 저장합니다. Pane은
  전환 사이에도 살아있기 때문에 (tmux 스타일) REPL 과 tail 이 죽지 않습니다.
  워크스페이스 버튼을 더블클릭하면 이름을 지정할 수 있습니다.
- **Pane별 설정 (⚙)**: 각 터미널의 `⚙` 버튼으로 **배경색** (컬러 피커) 설정과
  **HotKey 버튼** (단일 줄 또는 여러 줄 배치 명령) 관리. 배경색은 재시작 후에도
  유지됩니다.
- **브라우저 pane**: 툴바의 `+ Browser` 버튼으로 레이아웃 슬롯에 iframe 기반
  브라우저를 배치할 수 있습니다. 뒤로 / 앞으로 / 새로고침이 있는 URL 바 제공.
  URL 은 워크스페이스 전환과 앱 재시작에도 유지됩니다.
  > **참고:** 브라우저 pane은 HTML `<iframe>` 으로 구현되어, `X-Frame-Options`
  > 나 CSP `frame-ancestors` 로 임베드를 거부하는 사이트 (예: github.com,
  > google.com) 는 로드되지 않습니다. 로컬 개발 서버, Storybook, 사내
  > 대시보드, API 문서, localhost 미리보기 등 **개발용으로 적합**하며, 일반
  > 웹 브라우징 용도가 아닙니다.
- **Pane 확대**: `Ctrl+Shift+Z` 로 나머지 pane 을 숨기고 포커스된 pane 만
  집중해서 볼 수 있습니다. 다시 누르면 분할 상태로 복원됩니다.
- **스크롤백 검색**: `Ctrl+F` 로 포커스된 터미널에 검색 바를 엽니다.
  Enter / Shift+Enter 로 이전/다음 매치 이동, Esc 로 닫기.
- **Pane 이름 변경**: `Ctrl+Shift+R` 로 포커스된 pane 에 사용자 제목을
  지정합니다.
- **업데이트 알림**: 백그라운드 폴러가 6시간마다 GitHub 릴리스를 확인해서
  새 버전이 나오면 닫을 수 있는 배너로 알려줍니다. 자동 설치는 하지 않습니다.
- **시스템 모니터 상태 바**: 창 하단의 얇은 바가 CPU / RAM / GPU / 디스크 /
  네트워크 ↑↓ 를 2초마다 실시간 표시합니다. 70% 이상이면 주황, 90% 이상이면
  빨강으로 강조됩니다. 멀티 GPU / 멀티 디스크도 지원합니다 (3개 이하는 인라인,
  그 이상은 요약 + 툴팁).
- **Ko-fi 후원 버튼**: `?` 옆의 ☕ Support 버튼을 누르면
  [ko-fi.com/youngminkim](https://ko-fi.com/youngminkim) 가 시스템 브라우저에서
  열립니다.
- **클릭 가능한 URL**: 터미널 내 `http://` 또는 `https://` 링크를 `Ctrl+클릭`하면
  기본 브라우저에서 열립니다.
- **단축키 안내**: 툴바 오른쪽 상단의 `?` 버튼을 누르면 내장 단축키 팝업이
  열립니다. 한국어, English, 日本語를 지원합니다.
- **커맨드 팔레트**: `Ctrl+Shift+P` 로 VS Code 스타일 검색 가능한 명령
  오버레이를 엽니다. 이름이나 단축키로 퍼지 매칭 검색.
- **클립보드 붙여넣기**: `Ctrl+V` 로 클립보드 텍스트를 터미널에 붙여넣기.
- **13개 언어 지원**: English, 한국어, 日本語, 中文, हिन्दी, Español, Français,
  العربية, Português, Русский, Türkçe, Deutsch, Tiếng Việt. 하단 상태바의
  언어 선택기에서 변경 가능.
- **MSI 설치 시 PATH 자동 등록**: 설치 디렉터리가 시스템 PATH 에 추가되어
  `ymux`, `ymon`, `ydir`, `ycode`, `y` 를 바로 실행 가능.
- **가벼움**: Tauri 바이너리 + WebView2. 인스톨러 목표 < 10 MB.

### 동반 TUI 도구

ymux 터미널 pane 안에서 실행되는 독립 바이너리. `y` 런처로 실행 가능
(`y mon`, `y dir`, `y code`).

| 도구 | 명령어 | 설명 |
|------|--------|------|
| **ymon** | `ymon` | htop/btop 스타일 시스템 모니터 (CPU, 메모리, 디스크, 프로세스) |
| **ydir** | `ydir` | 듀얼 패널 파일 매니저 — 탐색, 복사/이동/삭제, 실행 파일 인자 다이얼로그 |
| **ycode** | `ycode <파일>` | TUI 코드 에디터 — undo/redo, 검색, goto, Esc 종료 다이얼로그, CJK/이모지 지원 |
| **y** | `y help` | 설치된 y* 도구를 목록화하고 실행하는 런처 |

## 개발

요구사항: Rust (stable), Node 20+, pnpm (또는 npm).

```sh
pnpm install
pnpm tauri dev          # 개발 모드로 실행
pnpm tauri build        # Windows MSI 인스톨러 빌드 (Windows에서 실행)
```

Windows가 아닌 호스트에서도 Rust 크레이트는 `cargo check` 가 깨끗하게 통과해서
Linux/macOS 에서 플랫폼 독립적인 로직을 작업할 수 있습니다. 하지만 전체
`tauri build` 와 엔드투엔드 PTY 검증은 반드시 Windows 에서 수행해야 합니다.

## 설정

`%APPDATA%\ymux\config.toml` 에 워크스페이스, 레이아웃, 캐시된 셸 프로필이
저장됩니다. 구조 변경이 있을 때마다 (디바운싱 적용) 그리고 앱 종료 시 다시
쓰여집니다.

## 키보드 단축키

| 단축키                         | 동작                                |
|--------------------------------|-------------------------------------|
| `Ctrl+Shift+H`                 | 현재 pane을 가로로 분할             |
| `Ctrl+Shift+V`                 | 현재 pane을 세로로 분할             |
| `Ctrl+Shift+W`                 | 포커스된 pane 닫기                  |
| `Ctrl+Shift+Z`                 | 포커스된 pane 확대 / 원복           |
| `Ctrl+Shift+R`                 | 포커스된 pane 이름 변경             |
| `Ctrl+Shift+P`                 | 커맨드 팔레트 열기                  |
| `Ctrl+V`                       | 클립보드를 터미널에 붙여넣기        |
| `Ctrl+F`                       | 터미널 스크롤백 검색                |
| `Ctrl+Tab`                     | 다음 pane으로 포커스 이동           |
| `Ctrl+Shift+Tab`               | 이전 pane으로 포커스 이동           |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9`   | 워크스페이스 전환                   |
| 워크스페이스 버튼 더블클릭       | 워크스페이스 이름 변경              |
| URL 위에서 `Ctrl+클릭`         | 기본 브라우저로 링크 열기           |
| 툴바의 `?` 버튼                | 단축키 안내 팝업 표시 / 숨기기      |

> **팁:** 툴바 오른쪽 상단의 `?` 버튼을 누르면 내장 단축키 안내 팝업이 열리며,
> 팝업 안에서 표시 언어도 변경할 수 있습니다.

## 상태

초기 MVP. 로드맵은 `docs/` (예정) 를 참고하세요.
