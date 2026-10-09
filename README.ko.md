<h1 align="center">ymux</h1>

<p align="center">
  <a href="./README.md">English</a> &nbsp;·&nbsp; <strong>한국어</strong> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.13.7-7fdbca?style=flat-square" alt="version 0.13.7" />
</p>

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Ko-fi로 후원하기" />
  </a>
</p>

---

Windows와 macOS용 경량 tmux 스타일 터미널 멀티플렉서.

https://github.com/user-attachments/assets/705fff59-0bda-4460-a87f-d7ba6f50993a

ymux는 창 하나를 필요한 만큼의 터미널로 나누고, 레이아웃을 작업공간별로 저장하며,
셸, 스크롤백, AI 코딩 에이전트를 한곳에서 정리해 줍니다. Tauri 2 (Rust)와
xterm.js로 만들었고, Windows에서는 WebView2, macOS에서는 WKWebView 위에서
동작합니다.

## 설치

### Windows

**Microsoft Store** (스토어 등록이 완료되면 이용할 수 있습니다):
<https://apps.microsoft.com/detail/9N20Z9NKD2Q1>. 스토어 빌드는 시스템의 Evergreen
WebView2 런타임(Windows 11에 내장, Windows 10에서는 Windows Update로 제공)을
사용하며, `ymux` 앱 실행 별칭을 등록하므로 어느 터미널에서든 `ymux`를 실행할 수
있습니다.

**MSI**: [Releases](https://github.com/YoungMins/ymux/releases)에서
`ymux_*_x64_en-US.msi`를 받아 실행하세요. 인스톨러가 WebView2 부트스트래퍼를
포함하고 설치 경로를 `PATH`에 추가하므로, 어느 터미널에서든 `ymux`를 실행할 수
있습니다.

### macOS (Apple Silicon, macOS 11+)

[Releases](https://github.com/YoungMins/ymux/releases)에서
`ymux_*_macos_aarch64.dmg`를 받아 열고, ymux를 Applications로 끌어다 놓으세요.

앱은 ad-hoc 서명만 되어 있고 **공증(notarization)은 되어 있지 않습니다.** 그래서
처음 실행할 때 Gatekeeper가 "손상되었다" 또는 "확인되지 않은 개발자"라며
차단합니다. 다운로드 격리 속성을 한 번만 제거하면 됩니다:

```sh
xattr -dr com.apple.quarantine /Applications/ymux.app
```

## 기능

### 터미널과 레이아웃

- **저장되는 레이아웃**: 재귀적 가로 / 세로 분할. 각 pane은 자신의 셸, `cwd`,
  선택적 시작 명령을 기억합니다.
- **Pane 탭**: `Ctrl+Shift+T`로 포커스된 pane에 탭을 추가합니다. 숨겨진 탭도
  계속 실행됩니다.
- **현재 경로 계승**: pane을 분할하면 새 pane이 시작 디렉터리가 아니라 부모 셸이
  지금 있는 디렉터리에서 열립니다 (OSC 7 추적).
- **셸 자동 감지**: Windows에서는 `cmd.exe`, Windows PowerShell,
  PowerShell 7 (`pwsh`), Git Bash, WSL 배포판을 찾습니다. macOS에서는 로그인
  셸과 함께 발견된 zsh / bash / fish (Homebrew 설치본 포함)를 보여줍니다.
  zsh와 bash는 자동 생성된 shim을 거쳐 시작되는데, 이 shim은 사용자 본인의
  dotfile을 먼저 읽은 뒤 OSC 7 훅을 덧붙입니다.
- **영속 스크롤백**: 각 pane의 버퍼가 (색상 포함) 다음 실행 때 흐린 *"세션 복원됨"*
  구분선 아래로 복원됩니다. **설정 → 일반**에서 켜고 끌 수 있습니다.
- **Pane 확대, 이름 변경, 검색**: `Ctrl+Shift+Z`는 나머지 pane을 숨기고,
  `Ctrl+Shift+R`은 포커스된 pane의 이름을 바꾸며, `Ctrl+F`는 터미널에 검색 바를
  엽니다 (Enter / Shift+Enter로 일치 항목 사이를 이동).
- **프롬프트 하단 고정**: 출력이 pane보다 짧으면 프롬프트가 하단에 붙고 출력이
  위로 쌓입니다. 기본값은 켜짐이며 **설정 → 일반**에서 끌 수 있습니다.
- **클릭 가능한 링크와 경로**: `Ctrl+클릭`하면 `http://` / `https://` 링크는 기본
  브라우저에서 열리고, 명령이 출력한 파일 경로는 OS 기본 프로그램으로 열립니다.
  실행 파일과 스크립트는 실행되지 않고 파일 관리자에서 표시만 됩니다.
- **텍스트와 이미지 붙여넣기**: `Ctrl+V`로 텍스트를 붙여넣습니다. 클립보드에
  이미지가 있으면 (예: `Win+Shift+S` 스크린샷) 자동 정리되는 임시 파일로 저장하고
  따옴표로 감싼 경로를 대신 입력합니다. 24시간 뒤 정리됩니다
  (`paste_image_retention_hours`).
- **파일 끌어다 놓기**: 파일이나 폴더를 터미널에 끌어다 놓으면 따옴표로 감싼
  경로가 커서 위치에 입력됩니다. Enter를 누르기 전에는 아무것도 실행되지
  않습니다.
- **Pane별 설정 (⚙)**: 사용자 지정 배경색과 HotKey 버튼 (터미널 위의 이름 붙은
  버튼에 단일 줄 또는 여러 줄 명령 지정)을 설정합니다.
- **트레이로 닫기**: 창을 닫으면 ymux가 트레이(Windows 알림 영역 / macOS 메뉴
  막대)로 숨고 모든 셸과 에이전트는 계속 실행됩니다. 트레이 아이콘을 클릭하거나
  ymux를 다시 실행하면 돌아옵니다 (ymux는 한 번에 하나만 실행됩니다). 종료는
  트레이 메뉴, macOS의 `Cmd+Q`, 또는 커맨드 팔레트의 **ymux 종료**로 하며,
  저장하지 않은 편집기가 있으면 먼저 묻습니다.

### 작업공간

- **번호가 붙은 작업공간**: `Ctrl+Alt+1` .. `Ctrl+Alt+9`로 처음 9개 사이를
  전환합니다. 작업공간 패널의 `+` 버튼으로 더 추가하고, 행의 `×`로 삭제합니다
  (마지막 하나는 삭제할 수 없음). 작업공간마다 자신의 레이아웃을 저장하며, 전환해도
  pane은 살아 있어서 REPL과 tail이 죽지 않습니다.
- **작업공간 패널**: 왼쪽의 접을 수 있고 스크롤되는 목록입니다. 이름을 더블클릭해
  바꾸고, 행을 끌어 순서를 바꿉니다. 활성 작업공간에는 테두리가 표시됩니다.
- **작업공간별 노트**: 각 작업공간 번호 옆의 노트 버튼, 그리고 활성 작업공간용
  `Ctrl+Alt+N`. `localStorage`에 저장됩니다.
- **Git worktree pane**: 커맨드 팔레트의 **"새 git worktree에서 pane 열기"**가
  브랜치 이름을 묻고, worktree를 만든 뒤 (기본 위치: 옆에 있는
  `.ymux-worktrees/<branch>` 디렉터리, `worktree_base_dir`로 변경 가능) 그 안에서
  터미널을 엽니다. pane을 닫거나 작업공간을 삭제할 때 worktree 제거 여부를
  묻습니다. 브랜치와 커밋은 건드리지 않습니다.

### AI 에이전트

- **에이전트 상태 한눈에 보기**: 각 터미널 pane이 출력 활동과 벨 / OSC 9 완료
  신호로 대기 / 실행 중 / 완료 / 주의 필요 상태를 보여주며, 색상 있는 pane 테두리와
  작업공간 행의 색조로 표시됩니다. 보고 있지 않은 곳에서 CLI가 끝나면 OS 알림과
  짧은 비프음이 울립니다 (설정에서 끌 수 있음).
- **런처**: 상단 **+** 메뉴에 설치된 에이전트 CLI, 셸, GUI pane이 나열됩니다.
  **에이전트 다시 검색**을 선택하면 목록을 새로 고칩니다.
- **에이전트 트리**: 작업공간 패널에 모든 pane과 탭이, 그 안에서 실행 중인
  에이전트와 함께 표시됩니다. 훅으로 감지하는 Claude Code 세션과 서브에이전트,
  그리고 가벼운 프로세스 스캔으로 감지하는 Codex, Gemini, aider 등 다른 CLI입니다.
  행을 클릭하면 해당 pane으로 이동합니다.
- **세션 재개**: Claude Code나 Codex가 실행 중인 pane에서 ymux를 재시작하면 같은
  대화를 이어서 재개합니다: `claude --resume <id>
  --dangerously-skip-permissions` (이미 승인한 항목을 다시 승인하지 않도록 항상
  붙습니다) 또는 `codex resume <id>`. 세션이 마지막으로 활동한 뒤 24시간 이내일
  때만 대상이 됩니다. 에이전트 추적을 켜지 않아도 동작하며, CLI가 디스크에 남기는
  자체 기록을 읽어서 찾습니다. 셸만 실행 중인 pane은 계속 스크롤백을 복원합니다.
- **Claude Code 훅 추적** (기본값 꺼짐, **설정 → 일반**): `~/.claude/settings.json`에
  사용자 수준 HTTP 훅을 추가합니다. 켜져 있는 동안에는 ymux 밖의 세션을 포함해 이
  컴퓨터의 *모든* Claude Code 세션이 훅 이벤트 (프롬프트, 도구 입력과 출력)를
  `127.0.0.1`의 ymux 포트로 보냅니다. ymux는 자신의 pane에서 오지 않은 이벤트를
  무시하며, 추적을 끄면 훅이 제거됩니다. 추적이 켜진 상태에서 ymux가 실행 중이
  아니면 Claude Code가 해당 훅의 실패를 보고하므로 (예: "Stop hook error
  occurred"), ymux를 제거하기 전에 추적을 끄세요. ymux는 보조 바이너리를 함께
  설치하지 않습니다.

### 사이드 패널: 파일과 사용량

모든 작업공간이 공유하는 오른쪽 패널 하나입니다. 상단의 아이콘으로 두 화면을
전환하며, 숨겨져 있는 동안에도 각 화면은 상태를 유지합니다. 열림 상태, 화면, 너비는
저장되고, 왼쪽 가장자리를 끌어 너비를 조절합니다.

- **파일**: `Ctrl+Shift+E`로 토글합니다. `cd`로 이동할 때마다 활성 pane의 작업
  디렉터리를 따라갑니다. 파일에서 Enter를 누르면 재사용되는 편집기 탭에서 열리고,
  목록 아래에 선택한 파일이나 폴더의 미리보기가 있습니다 (`Tab`으로 표시/숨김).
- **사용량**: 패널의 사용량 아이콘으로 전환하거나 (도구 모음의 사이드 패널 버튼은 마지막 화면을 다시
  엽니다) 커맨드 팔레트에서 엽니다. 다음을 보여줍니다:
  - Claude Code와 Codex의 로컬 토큰 수. 세션 로그에서 읽으며, 최근 5시간, 최근 7일,
    전체 기록 기간을 AI 제공자, 모델, 프로젝트별로 묶어 보여줍니다.
  - 5시간 / 주간 남은 한도와 초기화까지의 카운트다운. 남은 양 또는 사용량 중
    원하는 쪽으로 표시합니다. 한도는 해당 CLI가 이미 PC에 저장해 둔 로그인 토큰으로
    Anthropic과 OpenAI에서 직접 가져옵니다 ([개인정보 보호](#개인정보-보호) 참조).

  표시되는 동안 10초마다 갱신됩니다. 로컬 합계에는 웹 채팅과 다른 기기가 포함되지
  않고 이전에 쓰던 계정이 포함될 수 있으므로, 구독 한도와 같지 않습니다. 한도를
  가져오지 못하면 기록된 스냅샷을 오래된 값이라고 표시해 보여줍니다.

### 내장 pane

터미널 옆에 ymux가 직접 그리는 화면입니다. 터미널의 오른쪽 클릭 메뉴 (해당 pane을
분할하고 작업 디렉터리를 이어받음), **+** 런처, 또는 커맨드 팔레트에서 엽니다.

| Pane | 기능 |
|------|------|
| **파일** | 탐색, 미리보기, 이름 변경, 새로 만들기, 다중 선택, 복사/이동, 덮어쓰기 확인, 휴지통으로 삭제 (확인 후) |
| **편집기** | 구문 강조 텍스트 편집기: 저장, 찾기/바꾸기, 줄 이동, CRLF/BOM 보존, 저장 안 한 변경 확인, 충돌 방지용 임시 초안, 외부 변경 감지 |
| **Git** | 커밋 그래프, 브랜치 목록과 체크아웃, worktree 추가/제거 (확인 후) |
| **브라우저** | URL 바와 뒤로 / 앞으로 / 새로고침이 있는 iframe 브라우저. URL은 전환과 재시작 후에도 유지됩니다 |

> **참고:** 브라우저 pane은 HTML `<iframe>`이라 `X-Frame-Options`나 CSP
> `frame-ancestors`로 임베드를 거부하는 사이트 (예: github.com, google.com)는
> 로드되지 않습니다. 로컬 개발 서버, Storybook, 사내 대시보드, 문서용이며 일반 웹
> 브라우징용이 아닙니다.

### 생산성

- **커맨드 팔레트**: `Ctrl+Shift+P`. 이름이나 단축키로 모든 내장 동작을 퍼지
  매칭으로 찾습니다.
- **시스템 모니터**: 하단 상태 바에 CPU / RAM / GPU / 디스크 / 네트워크를 2초마다
  표시합니다. 70%에서 주황, 90%에서 빨강으로 바뀝니다.
- **업데이트 알림**: 6시간마다 백그라운드에서 GitHub 릴리스를 확인해 닫을 수 있는
  배너로 알려줍니다. 자동으로 설치하지는 않습니다.
- **Ko-fi 후원**: `⚙` 옆의 ☕ Support 버튼이
  [ko-fi.com/youngminkim](https://ko-fi.com/youngminkim)을 엽니다.

### 사용자 지정

- **설정 (⚙)**: 일반, 구문 색상, 단축키, 설정 파일 섹션이 있습니다. 언어를 고르고,
  편집기의 구문 팔레트를 수정하고, 단축키 참조를 보거나, `theme.toml`을 바로 열 수
  있습니다.
- **13개 언어**: English, 한국어, 日本語, 中文, हिन्दी, Español, Français,
  العربية, Português, Русский, Türkçe, Deutsch, Tiếng Việt. 오른쪽 하단 상태 바의
  선택기에서 바꿉니다.
- **내장 웹 콘텐츠로부터 격리**: 모든 백엔드 명령이 호출자의 출처를 검사하므로
  브라우저 pane 안의 페이지가 ymux의 IPC를 호출할 수 없습니다. ymux는 다른 페이지에
  프레임으로 삽입되면 로드를 거부하며, 내장 브라우저는 정해진 소수의 안전한
  단축키만 전달합니다.

## 키보드 단축키

macOS에서는 아래의 모든 `Ctrl`이 `Cmd`로 바뀝니다. 그래야 `Ctrl`이 셸 본래의
용도 (`Ctrl+C`, `Ctrl+D`, `Ctrl+R`)로 남습니다. 예외는 표에 적힌 두 가지입니다.
pane 순환은 macOS가 `Cmd+Tab`을 앱 전환기로 쓰기 때문에 `Ctrl+Tab`을 유지하고,
작업공간 전환은 `Alt`를 뺍니다.

| 단축키 (Windows)            | macOS              | 동작                                 |
|-----------------------------|--------------------|--------------------------------------|
| `Ctrl+Shift+H`              | `Cmd+Shift+H`      | pane 가로 분할                       |
| `Ctrl+Shift+V`              | `Cmd+Shift+V`      | pane 세로 분할                       |
| `Ctrl+Shift+W`              | `Cmd+Shift+W`      | 포커스된 pane 닫기                   |
| `Ctrl+Shift+T`              | `Cmd+Shift+T`      | 포커스된 pane에 새 탭                |
| `Ctrl+Shift+[` / `]`        | `Cmd+Shift+[` / `]` | pane 안에서 이전 / 다음 탭          |
| `Ctrl+Shift+Z`              | `Cmd+Shift+Z`      | 포커스된 pane 확대 / 원복            |
| `Ctrl+Shift+←/→`            | `Cmd+Shift+←/→`    | 이전 / 다음 pane과 자리 바꾸기       |
| `Ctrl+Shift+R`              | `Cmd+Shift+R`      | 포커스된 pane 이름 변경              |
| `Ctrl+Shift+P`              | `Cmd+Shift+P`      | 커맨드 팔레트 열기                   |
| `Ctrl+Alt+N`                | `Cmd+Opt+N`        | 활성 작업공간의 노트 토글            |
| `Ctrl+Shift+E`              | `Cmd+Shift+E`      | 사이드 패널 토글 (파일 화면)         |
| `Ctrl+V`                    | `Cmd+V`            | 클립보드 텍스트 붙여넣기 (이미지 → 임시 파일 경로) |
| `Ctrl+F`                    | `Cmd+F`            | 터미널 스크롤백 검색 (편집기 pane에서는 찾기 / 바꾸기) |
| `Ctrl+S`                    | `Cmd+S`            | 파일 저장 (편집기 pane)              |
| `Ctrl+G`                    | `Cmd+G`            | 줄로 이동 (편집기 pane)              |
| `Ctrl++` / `Ctrl+-`         | `Cmd++` / `Cmd+-`  | 터미널 글자 크기 확대 / 축소         |
| `Ctrl+0`                    | `Cmd+0`            | 터미널 글자 크기 초기화              |
| `Ctrl+Tab`                  | `Ctrl+Tab`         | 다음 pane으로 포커스 이동            |
| `Ctrl+Shift+Tab`            | `Ctrl+Shift+Tab`   | 이전 pane으로 포커스 이동            |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9` | `Cmd+1` .. `Cmd+9` | 작업공간 전환                       |
| URL/경로 위에서 `Ctrl+클릭` | `Cmd+클릭`         | 링크 또는 출력된 파일 경로를 OS 기본 프로그램으로 열기 |
| 작업공간 이름 더블클릭      | —                  | 작업공간 이름 변경                   |
| 작업공간 행 끌기            | —                  | 작업공간 순서 변경                   |
| 터미널에서 마우스 오른쪽 클릭 | —                | 컨텍스트 메뉴: 복사/붙여넣기, 분할, 파일 / 편집기 / Git pane 열기 |
| `⚙` 버튼 (도구 모음)        | —                  | 설정 열기 (언어, 단축키, 구문 색상, 설정 파일) |

사용량 화면에는 단축키가 없습니다. 패널의 사용량 아이콘이나 커맨드
팔레트를 사용하세요.

## 설정

`config.toml`에는 작업공간, 레이아웃, 캐시된 셸 프로필이 저장됩니다. 구조가 바뀔
때마다 (디바운싱 적용) 그리고 앱을 닫을 때 다시 쓰입니다.

`theme.toml`에는 편집기의 구문 색상을 포함한 색상 팔레트가 저장됩니다.
**설정 → 구문 색상**에서 편집하거나 **설정 → 설정 파일 → 열기**로 파일을 바로 열
수 있습니다.

두 파일 모두 ymux 설정 디렉터리에 있습니다:

| 플랫폼   | 경로 |
|----------|------|
| Windows  | `%APPDATA%\ymux\` |
| macOS    | `~/Library/Application Support/ymux/` |

macOS에서는 같은 디렉터리에 자동 생성된 셸 통합 파일 (`zsh-init/`,
`bash-init.sh`)도 들어갑니다. 셸을 감지할 때마다 다시 쓰이므로 지워도 괜찮습니다.

## 개인정보 보호

ymux에는 계정, 분석, 텔레메트리가 없으며 서버도 운영하지 않습니다. 레이아웃,
스크롤백, 노트, 설정은 모두 PC에만 남습니다. ymux가 자발적으로 보내는 요청은 GitHub
업데이트 확인뿐이며, 사용량 화면을 쓰는 경우에는 Anthropic과 OpenAI에 한도를 조회하는
요청이 추가됩니다. 이 요청은 해당 CLI가 이미 로컬에 저장해 둔 토큰을 그 토큰을 발급한
제공자에게 보냅니다. 전체 [개인정보 처리방침](./packaging/microsoft-store/PRIVACY.md)을
참고하세요.

## 개발

필요한 것: Rust (stable), Node 20+, pnpm.

```sh
pnpm install
pnpm tauri dev          # run in dev mode
pnpm tauri build        # MSI on Windows, .app + .dmg on macOS
pnpm test               # fmt + tsc + clippy + tests
npx tsc --noEmit        # TypeScript type check
cargo clippy --workspace -- -D warnings
cargo test -p ytheme -p ypath
cargo test --no-default-features --lib -p ymux
```

`tauri build`는 실행한 호스트에 맞는 인스톨러를 만듭니다 (MSI 번들러는 Windows
전용, DMG 번들러는 macOS 전용이며, 릴리스 워크플로가 그렇게 동작합니다). Linux에서도
Rust 크레이트는 `cargo check --no-default-features --lib --tests -p ymux`가 깨끗하게
통과하지만, 데스크톱 앱 자체는 Linux로 배포하지 않습니다.

메인테이너용: Microsoft Store MSIX 패키지는 `scripts/package-msix.ps1`로 빌드합니다.
[packaging/microsoft-store/README.md](./packaging/microsoft-store/README.md)를
참고하세요.

## 상태

활발히 개발 중입니다. 모든 태그는 Linux 테스트, Windows MSI, macOS DMG 파이프라인을
거칩니다. 변경 내역은 [Releases](https://github.com/YoungMins/ymux/releases)를
참고하세요.

## 라이선스

[MIT](./LICENSE) © 2026 Kim YoungMin
