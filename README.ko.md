<h1 align="center">ymux</h1>

<p align="center">
  <a href="./README.md">English</a> &nbsp;·&nbsp; <strong>한국어</strong> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.11.2-7fdbca?style=flat-square" alt="version 0.11.2" />
</p>

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Ko-fi로 후원하기" />
  </a>
</p>

---

Windows와 macOS용 경량 tmux 스타일 터미널 멀티플렉서.

https://github.com/user-attachments/assets/705fff59-0bda-4460-a87f-d7ba6f50993a

Tauri 2 (Rust) + xterm.js 로 만들어졌고, Windows에서는 WebView2, macOS에서는
WKWebView 위에서 동작합니다. 가볍고 빠르게 네이티브로 동작하면서, 레이아웃 저장,
pane별 작업 디렉터리와 실행 명령, 여러 셸 선택 (Windows: cmd / PowerShell /
pwsh / Git Bash / WSL, macOS: zsh / bash / fish), 그리고 각자 자신만의 레이아웃을
기억하는 번호 매겨진 워크스페이스를 제공합니다.

## 설치

### Windows

[Releases](https://github.com/YoungMins/ymux/releases) 에서 `ymux_*_x64_en-US.msi`
를 받아 실행하세요. 인스톨러가 WebView2 부트스트래퍼를 포함하고 설치 경로를
`PATH` 에 등록하므로, 설치 직후부터 아무 터미널에서나 `ymux` 를 실행할 수
있습니다.

### macOS (Apple Silicon, macOS 11+)

[Releases](https://github.com/YoungMins/ymux/releases) 에서
`ymux_*_macos_aarch64.dmg` 를 받아 열고, ymux 를 Applications 로 끌어다 놓으세요.

앱은 ad-hoc 서명만 되어 있고 **공증(notarization)은 되어 있지 않습니다.** 그래서
처음 실행할 때 Gatekeeper 가 "손상되었다" 또는 "확인되지 않은 개발자" 라며
차단합니다. 다운로드 격리 속성을 한 번만 제거하면 됩니다:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

## 기능

- **저장되는 레이아웃**: 재귀적 가로 / 세로 분할. 각 pane은 자신의 셸, `cwd`,
  선택적 시작 명령을 기억합니다.
- **현재 경로 계승**: pane을 분할하면 부모 셸이 현재 있는 경로에서 새 pane이
  열립니다. 처음 시작 경로가 아니라 실시간으로 추적된 경로를 계승합니다.
  OSC 7 이스케이프 시퀀스 추적 방식을 사용합니다.
- **셸 자동 감지**: Windows에서는 `cmd.exe`, Windows PowerShell,
  PowerShell 7 (`pwsh`), Git Bash, WSL 배포판을 찾아냅니다. macOS에서는 로그인
  셸과 함께 발견된 zsh / bash / fish (Homebrew 설치본 포함) 를 노출합니다.
  zsh 와 bash 는 자동 생성된 셸 통합 shim 을 거쳐 실행되는데, 이 shim 은 사용자
  본인의 dotfile 을 먼저 읽은 뒤 OSC 7 훅을 덧붙입니다. 덕분에 설정 파일을
  직접 건드리지 않아도 실시간 `cwd` 계승이 동작합니다.
- **번호 매겨진 워크스페이스**: `Ctrl+Alt+1` .. `Ctrl+Alt+9` 로 처음 9개
  워크스페이스 사이를 전환합니다. 워크스페이스 패널의 `+` 버튼으로 개수 제한
  없이 언제든 추가할 수 있고, 워크스페이스 행에 마우스를 올리면 나타나는 `×`
  로 삭제할 수 있습니다 (마지막 하나는 삭제 불가). 모든 워크스페이스는 자신만의
  레이아웃을 저장합니다. Pane은 전환 사이에도 살아있기 때문에 (tmux 스타일)
  REPL 과 tail 이 죽지 않습니다. 워크스페이스 이름을 더블클릭하면 이름을 바꿀
  수 있고, 활성 워크스페이스는 블록 전체에 테두리가 표시되어 한눈에 구분됩니다.
- **Git worktree pane**: 커맨드 팔레트의 **"새 git worktree에서 pane 열기"** 로
  브랜치 이름을 입력하면 git worktree 를 만들고 (기본 위치: 리포지토리 옆의
  `.ymux-worktrees/<브랜치>` 디렉터리, `worktree_base_dir` 로 변경 가능) 그
  안으로 이동한 터미널 pane 을 엽니다 — AI 에이전트나 실험마다 격리된
  체크아웃을 하나씩 줄 수 있습니다. pane 을 닫거나 워크스페이스 전체를
  삭제할 때 worktree 제거를 물어봅니다 (브랜치와 커밋은 건드리지 않습니다).
- **영속 스크롤백**: 터미널 출력이 앱 재시작 후에도 유지됩니다. 각 pane 의
  버퍼가 (색상 포함) 다음 실행 때 흐린 *"세션 복원됨"* 구분선 아래로
  복원됩니다. **설정 → 일반** 에서 켜고 끌 수 있습니다.
- **에이전트 상태 한눈에 보기**: 각 터미널 pane 이 출력 활동과 벨 / OSC 9
  완료 신호로 프로세스 상태 — 대기 / 실행 중 / 완료 / 주의 필요 — 를
  추적해서, 색상 있는 pane 테두리와 워크스페이스 패널의 행 색조로 표시합니다.
  보고 있지 않은 pane 의 CLI 가 끝나면 OS 알림과 짧은 비프음도 울립니다
  (설정에서 끌 수 있음).
- **에이전트 세션 재개**: Claude Code 나 Codex 가 실행 중인 pane 에서 ymux 를
  재시작하면, 예전 스크롤백을 그대로 보여주는 대신 같은 대화를 이어서
  재개합니다 — `claude --resume <id> --dangerously-skip-permissions`
  (이미 승인했던 항목을 다시 승인하지 않도록 항상 붙습니다) 또는
  `codex resume <id>`. 세션이 마지막으로 활동한 뒤 24시간 이내여야 재개
  대상이 됩니다. 에이전트 트리 추적을 켜지 않아도 동작합니다 — CLI가 디스크에
  남기는 자체 세션 기록에서 세션 id를 찾아냅니다. 셸만 실행 중인 pane은
  영향 없이 지금처럼 스크롤백을 복원합니다.
- **에이전트 트리**: 워크스페이스 패널에 각 워크스페이스의 pane(과 탭)이,
  그 안에서 실행 중인 코딩 에이전트와 함께 표시됩니다 — 훅으로 감지하는
  Claude Code 세션과 서브에이전트, 그리고 가벼운 프로세스 스캔으로 감지하는
  Codex, Gemini, aider 등 다른 CLI. 행을 클릭하면 바로 해당 pane으로
  이동합니다. Claude Code 훅 추적은 기본적으로 꺼져 있으며
  **설정 → 일반** 에서 켤 수 있습니다. 켜면 `~/.claude/settings.json` 에 사용자
  수준 HTTP 훅이 추가되므로, 켜져 있는 동안에는 ymux 밖의 세션을 포함한 이
  컴퓨터의 *모든* Claude Code 세션이 훅 이벤트(프롬프트, 도구 입력과 출력)를
  `127.0.0.1` 의 ymux 포트로 보냅니다. ymux 는 자신의 pane 에서 오지 않은 이벤트는
  무시하며, 추적을 끄면 훅이 제거됩니다.
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
- **파일 도크**: `Ctrl+Shift+E` 로 오른쪽 파일 패널을 여닫습니다. 이
  패널은 `cd` 로 이동할 때마다 활성 pane의 작업 디렉터리를 따라갑니다.
  파일에서 Enter 를 누르면 도크에 머무르지 않고 재사용되는 편집기 탭에서
  파일이 열립니다. 파일 목록 아래에 선택한 파일·폴더의 미리보기가 있고,
  `Tab` 으로 켜고 끌 수 있습니다. 도크의 왼쪽 가장자리를 끌어 너비를
  조절합니다.
- **Pane 확대**: `Ctrl+Shift+Z` 로 나머지 pane 을 숨기고 포커스된 pane 만
  집중해서 볼 수 있습니다. 다시 누르면 분할 상태로 복원됩니다.
- **스크롤백 검색**: `Ctrl+F` 로 포커스된 터미널에 검색 바를 엽니다.
  Enter / Shift+Enter 로 이전/다음 매치 이동, Esc 로 닫기.
- **Pane 이름 변경**: `Ctrl+Shift+R` 로 포커스된 pane 에 사용자 제목을
  지정합니다.
- **프롬프트 하단 고정**: 터미널 출력이 pane 보다 짧으면 프롬프트가 위쪽이
  아니라 하단에 붙어서 출력이 위로 쌓입니다. 기본적으로 켜져 있으며
  **설정 → 일반** 에서 끌 수 있습니다.
- **업데이트 알림**: 백그라운드 폴러가 6시간마다 GitHub 릴리스를 확인해서
  새 버전이 나오면 닫을 수 있는 배너로 알려줍니다. 자동 설치는 하지 않습니다.
- **시스템 모니터 상태 바**: 창 하단의 얇은 바가 CPU / RAM / GPU / 디스크 /
  네트워크 ↑↓ 를 2초마다 실시간 표시합니다. 70% 이상이면 주황, 90% 이상이면
  빨강으로 강조됩니다. 멀티 GPU / 멀티 디스크도 지원합니다 (3개 이하는 인라인,
  그 이상은 요약 + 툴팁).
- **Ko-fi 후원 버튼**: `⚙` 옆의 ☕ Support 버튼을 누르면
  [ko-fi.com/youngminkim](https://ko-fi.com/youngminkim) 가 시스템 브라우저에서
  열립니다.
- **클릭 가능한 링크와 경로**: 터미널 내 `http://` 또는 `https://` 링크를
  `Ctrl+클릭`하면 기본 브라우저에서 열립니다. 명령 출력에 찍힌 파일 경로
  (`cat`, `git log`, 에이전트 답변 등)도 `Ctrl+클릭`으로 OS 기본 프로그램으로
  열 수 있습니다. 실행 파일과 스크립트는 실행되지 않고 파일 관리자에서
  드러나기만 합니다.
- **설정 패널 (⚙)**: WinUI 3 스타일 모달로 왼쪽에 메뉴(일반, 구문 색상,
  단축키, 설정 파일), 오른쪽에 콘텐츠가 표시됩니다. 표시 언어 변경,
  편집기 창의 구문 강조 색상을 컬러 피커로 편집, 내장 단축키 참조 확인,
  또는 `theme.toml` 을 기본 편집기로 한 번에 열 수 있습니다.
- **커맨드 팔레트**: `Ctrl+Shift+P` 로 VS Code 스타일 검색 가능한 명령
  오버레이를 엽니다. 이름이나 단축키로 퍼지 매칭 검색.
- **워크스페이스별 노트**: 각 워크스페이스 번호 옆에 노트 아이콘이 있어
  해당 워크스페이스 전용 노트를 열 수 있습니다. `Ctrl+Alt+N` 으로 현재
  활성 워크스페이스의 노트를 토글합니다. 노트는 `localStorage` 에
  자동 저장되어 세션 간 유지되며, 내용이 있는 워크스페이스의 아이콘은
  액센트 색상으로 강조됩니다.
- **클립보드 붙여넣기 — 텍스트와 이미지**: `Ctrl+V` 로 클립보드 텍스트를
  터미널에 붙여넣습니다. 클립보드에 **이미지**가 있으면 (예: `Win+Shift+S`
  스크린샷) 자동 정리되는 임시 파일로 저장하고 따옴표로 감싼 경로를 대신
  입력합니다 — 스크린샷을 pane 안의 Claude Code 같은 AI CLI 에 바로 넘길 수
  있습니다. 붙여넣은 이미지는 24시간 후 정리됩니다
  (`paste_image_retention_hours` 로 조정 가능).
- **파일 드래그로 경로 입력**: 파일이나 폴더를 터미널 pane 에 끌어다 놓으면
  따옴표로 감싼 경로가 커서 위치에 입력됩니다. 포커스된 pane 이 아니라
  **떨어뜨린 그 pane** 에 들어갑니다. Enter 는 보내지 않습니다.
- **13개 언어 지원**: English, 한국어, 日本語, 中文, हिन्दी, Español, Français,
  العربية, Português, Русский, Türkçe, Deutsch, Tiếng Việt. 하단 상태바의
  언어 선택기에서 변경 가능.
- **MSI 설치 시 PATH 자동 등록**: 설치 디렉터리가 시스템 PATH 에 추가되어
  `ymux` 를 바로 실행 가능.
- **가벼움**: Tauri 바이너리 + WebView2. 인스톨러 목표 < 10 MB.
- **내장 웹 콘텐츠로부터 격리**: 모든 백엔드 명령이 호출자의 출처를 검사하므로
  브라우저 pane 안의 페이지는 ymux 자체 IPC 를 호출할 수 없습니다. ymux 는
  다른 페이지에 프레임으로 삽입되면 로드를 거부하며, 내장 브라우저는 프로세스
  실행이나 pane 닫기 같은 위험한 동작 없이 정해진 소수의 안전한 단축키만
  전달합니다.

### 도구 창

파일, 편집기, Git 화면은 터미널·브라우저와 나란히 ymux 가 직접 그리는
pane 입니다. 터미널의 오른쪽 클릭 메뉴(그 pane 을 분할하고 작업 디렉터리를
이어받음)나 커맨드 팔레트에서 엽니다.

| 창 | 여는 방법 | 기능 |
|----|-----------|------|
| **파일** | 오른쪽 클릭 → *여기서 파일 보기*, 파일 도크 | 탐색, 미리보기, 이름 변경, 새로 만들기, 다중 선택, 복사/이동, 덮어쓰기 확인, 휴지통으로 삭제 (확인 후) |
| **편집기** | 오른쪽 클릭 → *분할: 편집기 창*, 파일에서 Enter | 구문 강조 텍스트 편집기 — 저장, 찾기/바꾸기, 줄 이동, CRLF/BOM 보존, 저장 안 한 변경 확인, 충돌 방지용 임시 초안, 외부 변경 감지 |
| **Git** | 오른쪽 클릭 → *여기서 Git* | 커밋 그래프, 브랜치 목록과 체크아웃, worktree 추가/제거 (확인 후) |

독립 실행 명령 `ydir` / `ycode` / `ygit` / `ymon`, `y` 런처와 `y` 훅 중계기는
더 이상 없으며, ymux 는 보조 바이너리를 함께 설치하지 않습니다. 에이전트 추적은
Claude Code 의 HTTP 훅을 사용해 `127.0.0.1` 의 ymux 로 이벤트를 보냅니다 — 업그레이드
후 처음 실행할 때 예전 `y` 훅 항목은 자동으로 바뀝니다. 추적이 켜진 상태에서 ymux 가
실행 중이 아니면 Claude Code 세션에 훅 실패가 표시되므로 (예: "Stop hook error
occurred"), ymux 를 제거하기 전에 **설정 → 일반** 에서 추적을 꺼 두세요.

## 개발

요구사항: Rust (stable), Node 20+, pnpm (또는 npm).

```sh
pnpm install
pnpm tauri dev          # 개발 모드로 실행
pnpm tauri build        # Windows에서는 MSI, macOS에서는 .app + .dmg
```

`tauri build` 는 실행한 호스트에 맞는 인스톨러를 만듭니다. MSI 번들러는
Windows 전용이고 DMG 번들러는 macOS 전용이라, 각 인스톨러는 해당 플랫폼에서
빌드해야 합니다 (릴리스 워크플로가 그렇게 동작합니다).

Linux 에서도 Rust 크레이트는 `cargo check` 가 깨끗하게 통과하므로 플랫폼 독립적인
로직은 거기서 작업할 수 있습니다. 다만 데스크톱 앱 자체는 Linux 로 배포하지
않습니다.

## 설정

`config.toml` 에 워크스페이스, 레이아웃, 캐시된 셸 프로필이 저장됩니다. 구조
변경이 있을 때마다 (디바운싱 적용) 그리고 앱 종료 시 다시 쓰여집니다.

`theme.toml` 에는 편집기 창의 구문 강조 색상을 포함한 색상 팔레트가
저장됩니다. **설정 → 구문 색상** 의 컬러 피커로 편집하거나
**설정 → 설정 파일 → 열기** 로 파일을 바로 열 수 있습니다.

두 파일 모두 ymux 설정 디렉터리에 있습니다:

| 플랫폼   | 경로 |
|----------|------|
| Windows  | `%APPDATA%\ymux\` |
| macOS    | `~/Library/Application Support/ymux/` |

macOS 에서는 같은 디렉터리에 자동 생성된 셸 통합 파일(`zsh-init/`,
`bash-init.sh`)도 함께 들어갑니다. 셸을 감지할 때마다 새로 쓰이므로 지워도
괜찮습니다.

## 키보드 단축키

macOS 에서는 아래의 모든 `Ctrl` 이 `Cmd` 로 바뀝니다. 그래야 `Ctrl` 이 셸
본래의 용도(`Ctrl+C`, `Ctrl+D`, `Ctrl+R`)로 그대로 남습니다. 예외는 표에 적힌
두 가지입니다. pane 순환은 macOS 가 `Cmd+Tab` 을 앱 전환기로 쓰기 때문에
`Ctrl+Tab` 을 유지하고, 워크스페이스 전환은 `Alt` 를 뺍니다.

| 단축키 (Windows)               | macOS              | 동작                                |
|--------------------------------|--------------------|-------------------------------------|
| `Ctrl+Shift+H`                 | `Cmd+Shift+H`      | 현재 pane을 가로로 분할             |
| `Ctrl+Shift+V`                 | `Cmd+Shift+V`      | 현재 pane을 세로로 분할             |
| `Ctrl+Shift+W`                 | `Cmd+Shift+W`      | 포커스된 pane 닫기                  |
| `Ctrl+Shift+T`                 | `Cmd+Shift+T`      | 포커스된 pane에 새 탭               |
| `Ctrl+Shift+[` / `]`           | `Cmd+Shift+[` / `]` | pane 안에서 이전 / 다음 탭         |
| `Ctrl+Shift+Z`                 | `Cmd+Shift+Z`      | 포커스된 pane 확대 / 원복           |
| `Ctrl+Shift+←/→`               | `Cmd+Shift+←/→`    | 이전 / 다음 pane과 자리 바꾸기      |
| `Ctrl+Shift+R`                 | `Cmd+Shift+R`      | 포커스된 pane 이름 변경             |
| `Ctrl+Shift+P`                 | `Cmd+Shift+P`      | 커맨드 팔레트 열기                  |
| `Ctrl+Alt+N`                   | `Cmd+Opt+N`        | 현재 워크스페이스 노트 토글         |
| `Ctrl+Shift+E`                 | `Cmd+Shift+E`      | 파일 도크 토글                      |
| `Ctrl+V`                       | `Cmd+V`            | 클립보드 텍스트 붙여넣기 (이미지 → 임시 파일 경로) |
| `Ctrl+F`                       | `Cmd+F`            | 터미널 스크롤백 검색 (편집기 창에서는 찾기 / 바꾸기) |
| `Ctrl+S`                       | `Cmd+S`            | 파일 저장 (편집기 창)               |
| `Ctrl+G`                       | `Cmd+G`            | 줄로 이동 (편집기 창)               |
| `Ctrl++` / `Ctrl+-`            | `Cmd++` / `Cmd+-`  | 터미널 글자 크기 확대 / 축소        |
| `Ctrl+0`                       | `Cmd+0`            | 터미널 글자 크기 초기화             |
| `Ctrl+Tab`                     | `Ctrl+Tab`         | 다음 pane으로 포커스 이동           |
| `Ctrl+Shift+Tab`               | `Ctrl+Shift+Tab`   | 이전 pane으로 포커스 이동           |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9`   | `Cmd+1` .. `Cmd+9` | 워크스페이스 전환                   |
| URL/경로 위에서 `Ctrl+클릭`    | `Cmd+클릭`         | 링크 또는 출력된 파일 경로를 OS 기본 프로그램으로 열기 |
| 워크스페이스 이름 더블클릭       | —                  | 워크스페이스 이름 변경              |
| 워크스페이스 행 드래그           | —                  | 워크스페이스 순서 변경              |
| 터미널에서 마우스 오른쪽 클릭    | —                  | 컨텍스트 메뉴 — 복사/붙여넣기, 분할, 파일/편집기/Git 창 열기 |
| 툴바의 `⚙` 버튼                | —                  | 설정 열기 (언어, 단축키, 구문 색상, 설정 파일) |

> **팁:** 툴바 오른쪽 상단의 `⚙` 버튼을 누르면 WinUI 3 스타일의 설정 모달이
> 열립니다. 왼쪽 사이드바에서 표시 언어 변경, 편집기 창의 구문 강조 팔레트 편집,
> `theme.toml` 등 설정 파일 한 번에 열기까지 가능합니다.

## 상태

활발히 개발 중 — 모든 태그마다 Linux 테스트 + Windows MSI + macOS DMG 빌드 CI 파이프라인을
거쳐 정기적으로 릴리스됩니다. 변경 내역은
[Releases](https://github.com/YoungMins/ymux/releases) 를 참고하세요.
