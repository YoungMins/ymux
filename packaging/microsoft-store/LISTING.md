# yMux — Microsoft Store listing

Store ID `9N20Z9NKD2Q1` · Package `37516KimYoungMin.yMux` · Partner Center 입력값.
글자 수 한도는 Partner Center 기준이며, 아래 문구는 모두 한도 안에 들어갑니다.

---

## 1. Properties (속성)

| 항목 | 값 |
| --- | --- |
| Category | **Developer tools** |
| Subcategory | (없음) |
| Privacy policy URL | `https://github.com/YoungMins/ymux/blob/main/packaging/microsoft-store/PRIVACY.md` (커밋·푸시 후 유효) |
| Website | `https://github.com/YoungMins/ymux` |
| Support contact | `https://github.com/YoungMins/ymux/issues` (또는 공개 이메일) |
| Pricing | Free |
| Product declarations | "This app depends on non-Microsoft drivers or NT services" → **체크 안 함** · "Customers can install to alternate drives" → 체크 유지 · "Accessibility guidelines tested" → **체크 안 함**(정식 검증 전) |

### System requirements (시스템 요구 사항)

- OS: Windows 10 version 2004 (build 19041) or later, x64
- Memory / DirectX / camera 등: 해당 없음
- Additional requirements (English, ≤200자):
  `Requires Microsoft Edge WebView2 Runtime (included with Windows 11 and delivered to Windows 10 via Windows Update).`

### Age rating (IARC 설문 요령)

- 유형: **App** (게임 아님)
- 폭력·성·약물·도박·욕설: 모두 **No**
- 사용자 간 소통/채팅: **No** · 위치 공유: **No** · 개인정보 공유: **No**
- 디지털 구매: **No**
- **Unrestricted internet access: Yes** — 브라우저 창에서 임의의 웹사이트를 열 수 있으므로
  정직하게 Yes로 답합니다. 보통 등급은 크게 오르지 않고 "Unrestricted Internet" 표시만 붙습니다.

---

## 2. Store listing — English (en-us)

### Product name
yMux

### Short title (≤50)
yMux – Terminal Multiplexer

### Short description (≤1,000)
A lightweight, tmux-style terminal multiplexer for Windows. Split terminals into saved layouts, switch between numbered workspaces, and keep shells, scrollback and AI coding agents organized in one fast native window.

### Description (≤10,000)
yMux is a lightweight terminal multiplexer for Windows, inspired by tmux. It lets you split one window into as many terminals as you need, save those layouts, and come back to exactly where you left off.

LAYOUTS THAT PERSIST
Split any pane horizontally or vertically, as deep as you like. Every pane remembers its shell, working directory and an optional startup command, and every workspace keeps its own layout across restarts. Terminal scrollback is saved too, so your output is still there after you reopen the app.

ALL YOUR SHELLS
yMux detects Command Prompt, Windows PowerShell, PowerShell 7, Git Bash and your WSL distributions automatically. When you split a pane, the new one opens in the directory your shell is in right now, not the one it started in.

NUMBERED WORKSPACES
Switch workspaces with Ctrl+Alt+1 to 9 and add as many as you need. Panes keep running while you switch away, so REPLs, servers and log tails never die.

BUILT FOR AI CODING AGENTS
- See at a glance which pane's agent is running, finished or waiting for you.
- The agent tree lists every pane and agent in each workspace.
- Resume Claude Code and Codex sessions after restarting yMux.
- Open a pane in a new git worktree to give each agent an isolated checkout.
- The side panel shows your Claude Code and Codex token usage and remaining quota.

MORE THAN A TERMINAL
- Side panel with a file browser and AI usage view
- Built-in text editor and git log / branch / worktree view
- Browser panes next to your terminals
- Command palette (Ctrl+Shift+P), pane tabs, pane zoom and scrollback search
- Clickable links and file paths in terminal output
- Paste text and images straight into the terminal
- Per-workspace notes and a live CPU / RAM status bar
- Close to tray and keep everything running
- Customizable theme, syntax colors and keyboard shortcuts
- Available in 13 languages

PRIVATE BY DESIGN
yMux has no accounts, no analytics and no telemetry. Your layouts, scrollback and settings stay on your PC.

yMux is free and open source: https://github.com/YoungMins/ymux

### What's new in this version (≤1,500)
First Microsoft Store release (version 0.13.7).
- One side panel for the file browser and AI token usage. Switch views with the icons at the top of the panel; the toolbar button reopens the last view.
- Compact AI usage view: remaining 5-hour and weekly quota for Claude Code and Codex, with a short reset countdown.
- Fixed Claude Code quota sometimes showing "—".
- New app icon.

### Product features (≤20개, 각 ≤200자)
1. Split terminals horizontally and vertically into layouts that are saved automatically
2. Numbered workspaces (Ctrl+Alt+1–9) that keep their panes running in the background
3. Auto-detects Command Prompt, PowerShell, PowerShell 7, Git Bash and WSL
4. New panes open in the current working directory of the shell you split
5. Terminal scrollback is restored after restarting the app
6. Live status of AI coding agents (Claude Code, Codex) in every pane
7. Resume AI agent sessions after a restart
8. Open a pane in a new git worktree for isolated agent work
9. AI token usage and remaining quota in the side panel
10. File browser side panel that follows your terminal's directory
11. Built-in text editor and git view
12. Browser panes alongside your terminals
13. Command palette, pane tabs, pane zoom and scrollback search
14. Clickable links and file paths in terminal output
15. Paste images directly into the terminal
16. Per-workspace notes and a CPU / RAM status bar
17. Customizable themes and keyboard shortcuts
18. No accounts, no analytics, no telemetry
19. Available in 13 languages

### Search terms (≤7개, 각 ≤30자, 합계 ≤21단어)
terminal · tmux · multiplexer · powershell · wsl · claude code · developer tools

### Copyright and trademark info (≤200)
© 2026 Kim YoungMin

---

## 3. 스토어 목록 — 한국어 (ko-kr)

### 제품 이름
yMux

### 짧은 제목
yMux – 터미널 멀티플렉서

### 짧은 설명
Windows용 가볍고 빠른 tmux 스타일 터미널 멀티플렉서입니다. 터미널을 나눠 레이아웃으로 저장하고, 번호가 매겨진 워크스페이스를 오가며, 셸과 스크롤백, AI 코딩 에이전트를 창 하나에서 정리하세요.

### 설명
yMux는 tmux에서 영감을 받은 Windows용 터미널 멀티플렉서입니다. 창 하나를 필요한 만큼 여러 터미널로 나누고, 그 배치를 저장해 두었다가 다음에 그대로 이어서 작업할 수 있습니다.

저장되는 레이아웃
어떤 창이든 가로·세로로 원하는 만큼 나눌 수 있습니다. 각 창은 셸, 작업 폴더, 시작 명령을 기억하고, 워크스페이스마다 레이아웃이 따로 저장됩니다. 터미널 스크롤백도 저장되어 앱을 다시 열어도 출력 내용이 남아 있습니다.

모든 셸을 한곳에서
명령 프롬프트, Windows PowerShell, PowerShell 7, Git Bash, WSL 배포판을 자동으로 찾아 줍니다. 창을 나누면 새 창은 셸이 처음 시작한 폴더가 아니라 지금 있는 폴더에서 열립니다.

번호가 매겨진 워크스페이스
Ctrl+Alt+1~9로 워크스페이스를 전환하고, 필요한 만큼 추가할 수 있습니다. 다른 워크스페이스로 옮겨 가도 창은 계속 실행되므로 REPL, 서버, 로그가 끊기지 않습니다.

AI 코딩 에이전트를 위한 기능
- 각 창의 에이전트가 작업 중인지, 끝났는지, 입력을 기다리는지 한눈에 보입니다.
- 에이전트 트리에서 워크스페이스의 모든 창과 에이전트를 볼 수 있습니다.
- yMux를 다시 시작해도 Claude Code와 Codex 세션을 이어서 열 수 있습니다.
- 새 git worktree에서 창을 열어 에이전트마다 독립된 작업 공간을 줄 수 있습니다.
- 사이드 패널에서 Claude Code와 Codex의 토큰 사용량과 남은 한도를 확인할 수 있습니다.

터미널 그 이상
- 파일 브라우저와 AI 사용량을 오가는 사이드 패널
- 내장 텍스트 편집기와 git 로그·브랜치·worktree 보기
- 터미널 옆에 띄우는 브라우저 창
- 명령 팔레트(Ctrl+Shift+P), 창 탭, 창 확대, 스크롤백 검색
- 터미널 출력의 링크와 파일 경로 클릭
- 텍스트와 이미지를 터미널에 바로 붙여넣기
- 워크스페이스별 메모와 CPU·RAM 상태 표시줄
- 창을 닫아도 트레이에서 계속 실행
- 테마, 구문 색상, 단축키 사용자 설정
- 13개 언어 지원

개인정보를 먼저 생각한 설계
yMux에는 계정, 분석 도구, 텔레메트리가 없습니다. 레이아웃, 스크롤백, 설정은 모두 내 PC에만 저장됩니다.

yMux는 무료 오픈 소스입니다: https://github.com/YoungMins/ymux

### 이 버전의 새로운 기능
Microsoft Store 첫 출시입니다(버전 0.13.7).
- 파일 브라우저와 AI 토큰 사용량을 하나의 사이드 패널에서 봅니다. 패널 위쪽 아이콘으로 전환하고, 툴바 버튼은 마지막으로 보던 화면을 다시 엽니다.
- 간결해진 AI 사용량 화면: Claude Code와 Codex의 5시간·주간 남은 한도와 초기화까지 남은 시간을 보여 줍니다.
- Claude Code 한도가 가끔 "—"로 표시되던 문제를 고쳤습니다.
- 새 앱 아이콘

### 제품 기능
1. 터미널을 가로·세로로 나누고 레이아웃을 자동 저장
2. 백그라운드에서도 창이 계속 실행되는 번호 워크스페이스(Ctrl+Alt+1~9)
3. 명령 프롬프트, PowerShell, PowerShell 7, Git Bash, WSL 자동 감지
4. 나눈 창이 현재 셸의 작업 폴더에서 열림
5. 앱을 다시 시작해도 터미널 스크롤백 복원
6. 창마다 AI 코딩 에이전트(Claude Code, Codex) 상태 표시
7. 다시 시작한 뒤에도 AI 에이전트 세션 이어서 열기
8. 새 git worktree에서 창을 열어 에이전트별 독립 작업
9. 사이드 패널에서 AI 토큰 사용량과 남은 한도 확인
10. 터미널 폴더를 따라가는 파일 브라우저 사이드 패널
11. 내장 텍스트 편집기와 git 보기
12. 터미널 옆에 띄우는 브라우저 창
13. 명령 팔레트, 창 탭, 창 확대, 스크롤백 검색
14. 터미널 출력의 링크와 파일 경로 클릭
15. 터미널에 이미지 바로 붙여넣기
16. 워크스페이스별 메모와 CPU·RAM 상태 표시줄
17. 테마와 단축키 사용자 설정
18. 계정·분석·텔레메트리 없음
19. 13개 언어 지원

### 검색어
터미널 · tmux · 멀티플렉서 · 파워셸 · wsl · claude code · 개발자 도구

### 저작권 및 상표 정보
© 2026 Kim YoungMin

---

## 4. 스크린샷 · 이미지

- 데스크톱 스크린샷: 최소 1장, 최대 10장. **1366×768 이상**(권장 1920×1080 또는 3840×2160), PNG.
- 권장 구성:
  1. 여러 창으로 나눈 레이아웃 + 왼쪽 워크스페이스 패널
  2. AI 에이전트 상태가 보이는 워크스페이스(에이전트 트리)
  3. 사이드 패널 — 파일 브라우저
  4. 사이드 패널 — AI 토큰 사용량
  5. 명령 팔레트
  6. 편집기 또는 git 보기
- 스크린샷에 실제 토큰, 계정 이메일, 회사 코드, 개인 경로가 보이지 않게 정리합니다.
- 스토어 로고(선택): 300×300 PNG. 생략하면 패키지 로고를 씁니다.

## 5. 인증 참고 사항 (Notes for certification)

```
yMux is a desktop terminal multiplexer. It needs runFullTrust to start user-selected
shells (cmd, PowerShell, Git Bash, WSL) through the Windows pseudoconsole API.
No account or sign-in is needed to test it: launch the app and a PowerShell or
Command Prompt pane opens automatically. Split with Ctrl+Shift+H / Ctrl+Shift+V,
open the command palette with Ctrl+Shift+P. The AI usage panel only shows data if
Claude Code or the Codex CLI is installed; otherwise it shows an empty state.
The app uses the system Microsoft Edge WebView2 runtime.
```

## 6. 언어별 목록에 관한 주의

패키지 매니페스트에 13개 언어(en, ko, ja, zh, hi, es, fr, ar, pt, ru, tr, de, vi)가
선언되어 있어 Partner Center가 13개 언어 모두의 스토어 목록을 요구합니다.
선택지는 두 가지입니다.

1. 나머지 11개 언어의 목록도 작성합니다. 위 영어 문구를 번역합니다.
2. 매니페스트 `Resources`를 실제로 목록을 낼 언어(예: en-US, ko-KR)로 줄입니다.
   앱 화면은 여전히 13개 언어를 지원하며, 스토어에 표시되는 지원 언어만 줄어듭니다.
