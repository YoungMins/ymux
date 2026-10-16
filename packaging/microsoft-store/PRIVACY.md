# yMux Privacy Policy / 개인정보 처리방침

Effective date / 시행일: 2026-10-16

---

## English

yMux is a desktop terminal multiplexer developed by Kim YoungMin ("we"). This
policy explains what information yMux handles and where it goes.

### Summary

- yMux has **no accounts, no analytics, no telemetry, and no advertising**.
- We operate **no servers** and **do not receive any of your data**.
- Everything yMux stores stays **on your device**. The only network requests
  yMux makes on its own are listed below.

### Information stored on your device

yMux saves the following in your user profile (for example
`%APPDATA%\ymux`) so the app can restore your work:

- Settings, workspace layouts, shell profiles and keyboard shortcuts
- Terminal scrollback of your panes
- Workspace notes and unsaved editor drafts
- Images you paste into a terminal (deleted automatically after a while)
- A cache of the AI usage figures described below
- If you turn on the optional activity log: daily log files of what your AI
  agents did (`activity/` in the same folder; kept 14 days by default, and
  prompt text only if you opt in)
- If you add webhook destinations: a retry queue of messages that could not be
  delivered yet (`webhooks/`; at most 200 messages or 5 MiB per destination)
- Webhook URLs, signing secrets and model API keys you enter, stored encrypted
  (Windows DPAPI; a file readable only by you on macOS) in `secrets.json`,
  never in the settings file, and never shown again

This data is never sent to us. Uninstalling yMux and deleting this folder
removes it.

### Network requests made by yMux

1. **Update check.** yMux periodically asks the GitHub API
   (`api.github.com`) for the latest release version. GitHub receives
   ordinary request information such as your IP address and the app's
   user agent. No personal data is included. GitHub's privacy statement
   applies.

2. **AI token usage panel (Claude Code / Codex).** If you use Claude Code
   or the OpenAI Codex CLI on this computer, yMux:
   - reads their local session logs (`~/.claude`, `~/.codex`) to count
     tokens on your device;
   - reads the sign-in token those tools already stored locally and sends
     it **directly** to the tool's own provider to retrieve your remaining
     usage quota: Anthropic (`api.anthropic.com`) for Claude Code, and
     OpenAI (`chatgpt.com`) for Codex.

   The token is sent only to the provider that issued it, never to us or to
   any third party, and yMux does not store a copy of it. Anthropic's and
   OpenAI's privacy policies apply to those requests. If you do not use
   these tools, nothing is read or sent.

3. **Browser panes.** Web pages you open in a browser pane are loaded
   directly from those websites, which may collect data under their own
   policies. yMux's default start page is Bing.

4. **Programs you run.** Shells and command-line programs you start in yMux
   run as you, and their network activity is governed by those programs,
   not by yMux.

5. **Links.** The GitHub and Ko-fi buttons open those websites only when
   you click them.

6. **Destinations you configure (optional, off by default).** If you turn on
   the activity log, you may also add:
   - *Webhook destinations* (a generic JSON endpoint, Slack, Discord or
     Microsoft Teams): yMux sends a short report when an AI agent's turn ends
     or it needs you, to the URL you entered.
   - *A summarizer endpoint* (a local Ollama or an OpenAI-compatible server):
     yMux sends the facts of a finished turn to it to write a summary. It must
     be on your own computer unless you explicitly allow a remote server.

   Data goes only to the addresses you enter and nowhere else; **we receive
   nothing**. Reports are built from clipped data with secrets redacted
   (best-effort pattern matching). Your prompts and the assistant's replies are
   off by default and are included only if you opt in (recording them, and
   including them per destination). Files are attached only if you enable
   attachments for a destination, only of the types you allow, within size
   limits, and never files with sensitive names (such as `.env` or private
   keys). The receiving service (Slack, Discord, Microsoft, your own server)
   handles the data under its own policy. A webhook URL works like a password:
   anyone who has it can post to that channel, so keep it private. Remove a
   destination and its saved URL and secret are deleted.

yMux's web interface runs on Microsoft Edge WebView2, a component of
Windows. Microsoft may collect diagnostic data from WebView2 according to
your Windows settings and the Microsoft Privacy Statement.

### AI agent tracking (optional)

If you turn on agent tracking, yMux adds HTTP hooks to your Claude Code
settings file (`~/.claude/settings.json`). Claude Code then reports session
events to yMux over the loopback address `127.0.0.1` only, so the events
never leave your computer. Turning tracking off removes the hooks. The optional
activity log (above) is built from these events; it stays on your device unless
you add a destination.

### Children

yMux is a developer tool and is not directed at children. We do not
knowingly collect information from anyone, including children.

### Changes

If this policy changes, we will update this page and its effective date.

### Contact

Questions about this policy: [CONTACT EMAIL] or
https://github.com/YoungMins/ymux/issues

---

## 한국어

yMux는 김영민(이하 "개발자")이 만든 데스크톱 터미널 멀티플렉서입니다. 이
방침은 yMux가 어떤 정보를 다루고, 그 정보가 어디로 전달되는지 설명합니다.

### 요약

- yMux에는 **계정, 분석 도구, 텔레메트리, 광고가 없습니다**.
- 개발자는 **서버를 운영하지 않으며, 사용자의 데이터를 받지 않습니다**.
- yMux가 저장하는 정보는 모두 **사용자의 기기 안에만** 있습니다. yMux가
  스스로 보내는 네트워크 요청은 아래에 적힌 것이 전부입니다.

### 기기에 저장되는 정보

yMux는 작업 상태를 복원하기 위해 다음 정보를 사용자 프로필
폴더(예: `%APPDATA%\ymux`)에 저장합니다.

- 설정, 워크스페이스 레이아웃, 셸 프로필, 단축키
- 각 터미널 창의 스크롤백 내용
- 워크스페이스 메모, 저장하지 않은 편집기 초안
- 터미널에 붙여넣은 이미지(일정 시간이 지나면 자동 삭제)
- 아래에서 설명하는 AI 사용량 수치의 캐시
- 선택 기능인 활동 로그를 켠 경우: AI 에이전트가 한 일의 일별 로그 파일
  (같은 폴더의 `activity/`, 기본 14일 보관, 프롬프트 내용은 직접 켠 경우에만
  기록)
- 웹훅 목적지를 추가한 경우: 아직 전달되지 못한 메시지의 재시도 대기열
  (`webhooks/`, 목적지당 최대 200개 또는 5 MiB)
- 입력한 웹훅 URL, 서명 비밀값, 모델 API 키: `secrets.json`에 암호화되어 저장됩니다
  (Windows는 DPAPI, macOS는 본인만 읽을 수 있는 파일). 설정 파일에는 저장되지
  않으며 다시 표시되지 않습니다.

이 정보는 개발자에게 전송되지 않습니다. yMux를 제거하고 이 폴더를 지우면
함께 삭제됩니다.

### yMux가 보내는 네트워크 요청

1. **업데이트 확인.** yMux는 주기적으로 GitHub API(`api.github.com`)에 최신
   릴리스 버전을 조회합니다. 이 과정에서 GitHub는 IP 주소, 앱의 User-Agent
   같은 일반적인 요청 정보를 받습니다. 개인정보는 포함되지 않으며, GitHub의
   개인정보 처리방침이 적용됩니다.

2. **AI 토큰 사용량 패널(Claude Code / Codex).** 이 컴퓨터에서 Claude Code나
   OpenAI Codex CLI를 사용하는 경우 yMux는 다음을 수행합니다.
   - 해당 도구의 로컬 세션 기록(`~/.claude`, `~/.codex`)을 읽어 기기 안에서
     토큰 사용량을 계산합니다.
   - 해당 도구가 이미 로컬에 저장해 둔 로그인 토큰을 읽어, 남은 사용 한도를
     확인하기 위해 그 도구의 제공사에 **직접** 보냅니다. Claude Code는
     Anthropic(`api.anthropic.com`), Codex는 OpenAI(`chatgpt.com`)입니다.

   토큰은 그것을 발급한 제공사에만 전송되며, 개발자나 다른 제3자에게는
   전송되지 않습니다. yMux는 토큰 사본을 따로 저장하지 않습니다. 이 요청에는
   Anthropic과 OpenAI의 개인정보 처리방침이 적용됩니다. 해당 도구를 사용하지
   않으면 아무것도 읽거나 보내지 않습니다.

3. **브라우저 창.** 브라우저 창에서 연 웹 페이지는 해당 웹사이트에서 직접
   불러오며, 그 웹사이트가 자체 방침에 따라 정보를 수집할 수 있습니다.
   기본 시작 페이지는 Bing입니다.

4. **사용자가 실행하는 프로그램.** yMux에서 실행한 셸과 명령줄 프로그램은
   사용자 권한으로 실행되며, 그 프로그램의 네트워크 활동은 yMux가 아니라 해당
   프로그램의 정책을 따릅니다.

5. **링크.** GitHub, Ko-fi 버튼은 사용자가 누를 때만 해당 웹사이트를 엽니다.

6. **사용자가 설정하는 목적지(선택 기능, 기본값 꺼짐).** 활동 로그를 켠 경우
   다음을 추가할 수 있습니다.
   - *웹훅 목적지*(일반 JSON 엔드포인트, Slack, Discord, Microsoft Teams): AI
     에이전트의 턴이 끝나거나 사용자가 필요할 때, 입력한 URL로 짧은 보고를
     보냅니다.
   - *요약기 엔드포인트*(로컬 Ollama 또는 OpenAI 호환 서버): 끝난 턴의 사실
     정보를 보내 요약을 만듭니다. 원격 서버를 명시적으로 허용하지 않는 한 이
     컴퓨터에 있는 서버여야 합니다.

   데이터는 사용자가 입력한 주소로만 전송되며 그 외에는 어디로도 가지 않습니다.
   **개발자는 아무것도 받지 않습니다.** 보고는 잘라낸 데이터에서 비밀값을 마스킹해
   만들어집니다(최선을 다하는 패턴 매칭). 프롬프트와 어시스턴트의 답변은 기본값이
   꺼짐이며, 사용자가 직접 켠 경우(기록 및 목적지별 포함 설정)에만 들어갑니다.
   파일은 목적지별로 첨부를 켠 경우에만, 허용한 형식만, 크기 제한 안에서
   첨부되며 `.env`나 개인 키처럼 민감한 이름의 파일은 보내지 않습니다. 받는
   서비스(Slack, Discord, Microsoft, 사용자의 서버)는 자체 방침에 따라 데이터를
   처리합니다. 웹훅 URL은 비밀번호와 같아서 가진 사람은 누구나 그 채널에 글을 올릴
   수 있으니 공개하지 마세요. 목적지를 삭제하면 저장된 URL과 비밀값도 삭제됩니다.

yMux의 화면은 Windows 구성 요소인 Microsoft Edge WebView2 위에서 동작합니다.
Microsoft는 Windows 설정과 Microsoft 개인정보 처리방침에 따라 WebView2의 진단
데이터를 수집할 수 있습니다.

### AI 에이전트 추적(선택 기능)

에이전트 추적을 켜면 yMux는 Claude Code 설정 파일(`~/.claude/settings.json`)에
HTTP 훅을 추가합니다. Claude Code는 세션 이벤트를 루프백 주소 `127.0.0.1`로만
yMux에 보내므로, 이 이벤트는 컴퓨터 밖으로 나가지 않습니다. 추적을 끄면 훅이
제거됩니다. 선택 기능인 활동 로그(위)는 이 이벤트로 만들어지며, 목적지를
추가하지 않는 한 기기 안에만 남습니다.

### 아동

yMux는 개발자용 도구이며 아동을 대상으로 하지 않습니다. 개발자는 아동을
포함한 누구의 정보도 수집하지 않습니다.

### 변경

이 방침이 바뀌면 이 페이지와 시행일을 갱신합니다.

### 문의

이 방침에 관한 문의: [연락처 이메일] 또는
https://github.com/YoungMins/ymux/issues
