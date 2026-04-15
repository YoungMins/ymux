<h1 align="center">ymux</h1>

<p align="center">
  <a href="./README.md">English</a> &nbsp;·&nbsp; <strong>한국어</strong> &nbsp;·&nbsp; <a href="./README.ja.md">日本語</a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/version-0.2.1-7fdbca?style=flat-square" alt="version 0.2.1" />
</p>

<p align="center">
  <a href="https://ko-fi.com/youngminkim">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Ko-fi로 후원하기" />
  </a>
</p>

---

Windows용 경량 tmux 스타일 터미널 멀티플렉서.

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
- **클릭 가능한 URL**: 터미널 내 `http://` 또는 `https://` 링크를 `Ctrl+클릭`하면
  기본 브라우저에서 열립니다.
- **단축키 안내**: 툴바 오른쪽 상단의 `?` 버튼을 누르면 내장 단축키 팝업이
  열립니다. 한국어, English, 日本語를 지원합니다.
- **가벼움**: Tauri 바이너리 + WebView2. 인스톨러 목표 < 10 MB.

## 개발

요구사항: Rust (stable), Node 20+, pnpm (또는 npm).

```sh
pnpm install
pnpm tauri dev          # 개발 모드로 실행
pnpm tauri build        # Windows 인스톨러 빌드 (Windows에서 실행)
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
| `Ctrl+Tab`                     | 다음 pane으로 포커스 이동           |
| `Ctrl+Shift+Tab`               | 이전 pane으로 포커스 이동           |
| `Ctrl+Alt+1` .. `Ctrl+Alt+9`   | 워크스페이스 전환                   |
| URL 위에서 `Ctrl+클릭`         | 기본 브라우저로 링크 열기           |
| 툴바의 `?` 버튼                | 단축키 안내 팝업 표시 / 숨기기      |

> **팁:** 툴바 오른쪽 상단의 `?` 버튼을 누르면 내장 단축키 안내 팝업이 열리며,
> 팝업 안에서 표시 언어도 변경할 수 있습니다.

## 상태

초기 MVP. 로드맵은 `docs/` (예정) 를 참고하세요.
