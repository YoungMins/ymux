# yMux Microsoft Store 패키징

이 패키지는 기존 MSI와 별도로 만드는 x64 MSIX입니다. 소스 앱 버전은
`0.13.6`, Store 패키지 버전은 `1.13.6.0`입니다.
`-PackageVersion`을 생략하면 스크립트가 `src-tauri/tauri.conf.json`의 `version`(X.Y.Z)에서
`(X+1).Y.Z.0`으로 계산합니다.
Partner Center 예약 정보와 아래 값이 반드시 일치해야 합니다.

| 항목 | 값 |
| --- | --- |
| Identity Name | `37516KimYoungMin.yMux` |
| Publisher | `CN=FE47F4A6-CD23-402D-91CF-9296FD388F7F` |
| Publisher display name | `Kim YoungMin` |
| Package family name | `37516KimYoungMin.yMux_za4n7kzemsetr` |
| Store ID | `9N20Z9NKD2Q1` |
| 지원 OS | Windows 10 빌드 19041 이상, x64 |

## 빌드

Windows SDK 10.0.26100.0의 MakeAppx와 기존 yMux 빌드 도구가 필요합니다.
WebView2 런타임은 패키지에 포함하지 않습니다. 앱은 시스템에 설치된 Evergreen WebView2를
사용합니다(Windows 11에 기본 내장, Windows 10은 Windows Update로 배포). 따라서 런타임
보안 업데이트는 Windows가 제공하며 Store 패키지를 다시 제출할 필요가 없습니다.
드물게 WebView2가 제거된 환경(일부 LTSC/기업 환경)에서는 앱이 실행되지 않을 수 있습니다.

Tauri 아이콘 생성기로 만든 `Square44x44Logo.png`, `Square150x150Logo.png`,
`StoreLogo.png`가 있는 폴더를 준비합니다.

```powershell
pnpm tauri icon src-tauri/icons/icon.png -o target/store-icons
powershell -NoProfile -File scripts/package-msix.ps1 -AssetsDirectory target/store-icons
```

### 아이콘 변형과 resources.pri

작업 표시줄과 시작 메뉴 아이콘이 색 판 위에 작게 표시되지 않도록, 스크립트가
`src-tauri/icons/icon.png`에서 `pnpm tauri icon -p 16 -p 24 -p 32 -p 48 -p 256`로 정확한
픽셀 크기를 만들어 `AssetsSquare44x44Logo.targetsize-{16,24,32,48,256}.png`와
`_altform-unplated`(어두운 테마), `_altform-lightunplated`(밝은 테마) 변형을 staging에 추가합니다
([Microsoft 아이콘 구성 문서](https://learn.microsoft.com/en-us/windows/apps/design/style/iconography/app-icon-construction)).
manifest는 기본 파일 이름(`Square44x44Logo.png`)만 참조하며 변형은 MRT가 고릅니다.
한정자가 붙은 파일은 `resources.pri` 색인으로 해석되므로, 스크립트가 Windows SDK의
`makepri.exe`(`createconfig` 후 `new`)로 staging 루트에 `resources.pri`를 생성해 패키지에 포함합니다.

이미 Store 설정으로 새로 빌드한 실행 파일을 패키징할 때만 `-SkipBuild`를 사용합니다.

```powershell
pnpm tauri build --no-bundle --ci -c src-tauri/tauri.microsoftstore.conf.json
powershell -NoProfile -File scripts/package-msix.ps1 -SkipBuild -AssetsDirectory target/store-icons
```

스크립트는 `target/release/ymux.exe`, 로고(와 위 아이콘 변형, `resources.pri`)만 새 staging 폴더에 복사합니다. 이전 sidecar 실행 파일을 포함하지 않습니다.
MakeAppx의 기본 검증을 실행하며 `/nv`로 검증을 생략하지 않습니다.
압축 해제 후 원본과 모든 파일의 SHA256을 비교합니다.

결과:

- `target/store/ymux_1.13.6.0_x64.msix`
- 같은 파일의 `.sha256` 체크섬
- `package-files.json`: 패키지 파일별 크기와 SHA256

## Partner Center 업로드

1. yMux 앱(Store ID `9N20Z9NKD2Q1`)에서 새 제출을 엽니다.
2. Packages에 위 MSIX를 업로드하고 identity, x64, OS 최소 버전과 13개 언어를 확인합니다.
3. 제한 기능 질문에 아래 사유를 제출합니다. Store 설명, 스크린샷, 개인정보 관련
   질문과 인증 테스트 결과를 확인한 다음 제출합니다.

이 MSIX에는 `AppxSignature.p7x`가 없습니다. **Store 업로드용 unsigned 패키지**이며,
Microsoft Store가 제출 과정에서 서명합니다. 이 파일을 더블클릭하여 일반 로컬 설치할 수
있다는 의미가 아닙니다. 개발 인증서 생성이나 신뢰 저장소 변경은 하지 않습니다.
[Microsoft MSIX 서명 안내](https://learn.microsoft.com/en-us/windows/msix/package/sign-msix-package-guide)

### runFullTrust 제한 기능 사유

제출용 영문:

> yMux is a classic Windows desktop terminal multiplexer. It uses Windows
> pseudoconsole APIs to start and manage user-selected shells and command-line
> tools, accesses working directories chosen by the user, and hosts its desktop
> interface in the system-installed Microsoft Edge WebView2 runtime. These desktop process and filesystem
> operations require runFullTrust. The app runs at the current user's normal
> integrity level and does not request administrator elevation.

`ymux.exe` app execution alias는 MSI의 PATH 등록에 대응하여 터미널에서 앱을
실행할 수 있게 합니다. Windows의 앱 실행 별칭 설정에서 사용자가 관리할 수 있습니다.
MSIX는 MSI의 WiX PATH 레지스트리 변경을 사용하지 않습니다.

## 릴리스 유지보수

앱 업데이트마다 네 자리 Store 버전을 이전 제출보다 높이고 마지막 자리를 0으로 유지합니다.
Store 버전은 `tauri.conf.json`에서 자동 계산되고 manifest의 Version은 스크립트가 덮어쓰므로
별도 수정이 필요 없으며, 필요하면 `-PackageVersion`으로 직접 지정합니다.
13개 언어와 identity를 보존하고, 새로 빌드한 실행 파일을 사용합니다.
인증/실행 검증에는 Store로 설치된 앱의 창 실행, 새 터미널과 쉘 실행, 분할,
설정 저장 및 다시 실행, `ymux.exe` 별칭, WebView2 Evergreen 런타임이 없는 환경에서의 동작 확인을 포함합니다.
MakeAppx 통과는 이 실제 동작 검증이나 Store 인증 통과를 대신하지 않습니다.

[MakeAppx 문서](https://learn.microsoft.com/en-us/windows/msix/package/create-app-package-with-makeappx-tool)
