[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [string]$PackageVersion,
    [string]$AssetsDirectory,
    [string]$MakeAppx = (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin\10.0.26100.0\x64\makeappx.exe')
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$output = Join-Path $repo 'target\store'
$stage = Join-Path $output 'stage'
$unpacked = Join-Path $output 'unpacked'
$release = Join-Path $repo 'target\release'
$manifestSource = Join-Path $repo 'packaging\microsoft-store\AppxManifest.xml'
if (-not (Test-Path -LiteralPath $MakeAppx -PathType Leaf)) {
    throw "Windows SDK MakeAppx not found: $MakeAppx"
}
# Default Store version from tauri.conf.json: app X.Y.Z -> (X+1).Y.Z.0.
if (-not $PackageVersion) {
    $appVersion = (Get-Content -LiteralPath (Join-Path $repo 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version
    if ($appVersion -notmatch '^(\d+)\.(\d+)\.(\d+)$') { throw "Cannot derive Store version from app version '$appVersion'." }
    $PackageVersion = '{0}.{1}.{2}.0' -f ([uint64]$Matches[1] + 1), $Matches[2], $Matches[3]
}
if ($PackageVersion -notmatch '^\d+\.\d+\.\d+\.0$') {
    throw 'Store package version must be four numbers with the final component 0.'
}
$versionParts = $PackageVersion.Split('.') | ForEach-Object { [uint64]$_ }
if ($versionParts[0] -lt 1 -or ($versionParts | Where-Object { $_ -gt 65535 })) {
    throw 'Store version major must be at least 1 and every component must be at most 65535.'
}
if (-not $SkipBuild) {
    Push-Location $repo
    try {
        & pnpm tauri build --no-bundle --ci -c src-tauri/tauri.microsoftstore.conf.json
        if ($LASTEXITCODE -ne 0) { throw 'Tauri Store build failed.' }
    } finally { Pop-Location }
}
$exe = Join-Path $release 'ymux.exe'
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "Missing build input: $exe" }
# The package relies on the system Evergreen WebView2; drop the stale provenance file of older fixed-runtime builds.
$staleProvenance = Join-Path $output 'runtime-source.json'
if (Test-Path -LiteralPath $staleProvenance) { Remove-Item -LiteralPath $staleProvenance -Force }
if (-not $AssetsDirectory) {
    $AssetsDirectory = Join-Path $repo 'target\store-icons'
    Push-Location $repo
    try {
        & pnpm tauri icon src-tauri/icons/icon.png -o $AssetsDirectory
        if ($LASTEXITCODE -ne 0) { throw 'Tauri Store logo generation failed.' }
    } finally { Pop-Location }
}
foreach ($logo in @('Square44x44Logo.png', 'Square150x150Logo.png', 'StoreLogo.png')) {
    if (-not (Test-Path -LiteralPath (Join-Path $AssetsDirectory $logo) -PathType Leaf)) {
        throw "Missing logo: $logo in $AssetsDirectory"
    }
}
# Delete only fixed descendants of the verified repository output directory.
foreach ($directory in @($stage, $unpacked)) {
    $absolute = [IO.Path]::GetFullPath($directory)
    $expectedParent = [IO.Path]::GetFullPath($output).TrimEnd('\') + '\'
    if (-not $absolute.StartsWith($expectedParent, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Unsafe output path: $absolute"
    }
    if (Test-Path -LiteralPath $absolute) { Remove-Item -LiteralPath $absolute -Recurse -Force }
}
$null = New-Item -ItemType Directory -Path (Join-Path $stage 'Assets') -Force
Copy-Item -LiteralPath (Join-Path $release 'ymux.exe') -Destination $stage
foreach ($logo in @('Square44x44Logo.png', 'Square150x150Logo.png', 'StoreLogo.png')) {
    Copy-Item -LiteralPath (Join-Path $AssetsDirectory $logo) -Destination (Join-Path $stage 'Assets')
}
# Taskbar/Start icons: exact-pixel target sizes, plain + unplated (dark) + lightunplated (light).
# Without these Windows shrinks Square44x44Logo onto a system icon plate.
$sizesDir = Join-Path $output 'icon-sizes'
if (Test-Path -LiteralPath $sizesDir) { Remove-Item -LiteralPath $sizesDir -Recurse -Force }
Push-Location $repo
try {
    & pnpm tauri icon src-tauri/icons/icon.png -o $sizesDir -p 16 -p 24 -p 32 -p 48 -p 256
    if ($LASTEXITCODE -ne 0) { throw 'Tauri target-size icon generation failed.' }
} finally { Pop-Location }
foreach ($size in 16, 24, 32, 48, 256) {
    $png = Join-Path $sizesDir "${size}x${size}.png"
    if (-not (Test-Path -LiteralPath $png -PathType Leaf)) { throw "Missing generated icon: $png" }
    foreach ($suffix in '', '_altform-unplated', '_altform-lightunplated') {
        Copy-Item -LiteralPath $png -Destination (Join-Path $stage "Assets\Square44x44Logo.targetsize-$size$suffix.png")
    }
}
if (Get-ChildItem -LiteralPath $stage -Directory -Recurse -Filter 'WebView2') {
    throw 'A WebView2 directory is in the stage; the Store package must use the system Evergreen runtime.'
}
[xml]$manifest = Get-Content -LiteralPath $manifestSource -Raw
$manifest.Package.Identity.Version = $PackageVersion
$manifest.Save((Join-Path $stage 'AppxManifest.xml'))
# Qualified assets are only resolved through a resources.pri index, so build one.
$makePri = Join-Path (Split-Path $MakeAppx -Parent) 'makepri.exe'
if (-not (Test-Path -LiteralPath $makePri -PathType Leaf)) { throw "MakePri not found: $makePri" }
$priConfig = Join-Path $output 'priconfig.xml'
& $makePri createconfig /cf $priConfig /dq en-US /pv 10.0.0 /o
if ($LASTEXITCODE -ne 0) { throw 'MakePri createconfig failed.' }
& $makePri new /pr $stage /cf $priConfig /mn (Join-Path $stage 'AppxManifest.xml') /of (Join-Path $stage 'resources.pri') /o
if ($LASTEXITCODE -ne 0) { throw 'MakePri new failed.' }
$package = Join-Path $output "ymux_${PackageVersion}_x64.msix"
& $MakeAppx pack /d $stage /p $package /o
if ($LASTEXITCODE -ne 0) { throw 'MakeAppx pack validation failed.' }
& $MakeAppx unpack /p $package /d $unpacked /o
if ($LASTEXITCODE -ne 0) { throw 'MakeAppx unpack failed.' }
foreach ($file in Get-ChildItem -LiteralPath $stage -File -Recurse) {
    $relative = $file.FullName.Substring($stage.Length + 1)
    $copy = Join-Path $unpacked $relative
    if (-not (Test-Path -LiteralPath $copy -PathType Leaf) -or
        (Get-FileHash -LiteralPath $file.FullName).Hash -ne (Get-FileHash -LiteralPath $copy).Hash) {
        throw "Unpacked byte verification failed: $relative"
    }
}
if (Test-Path -LiteralPath (Join-Path $unpacked 'AppxSignature.p7x')) {
    throw 'Expected an unsigned Store submission package.'
}
$hash = (Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash.ToLowerInvariant()
"$hash  $([IO.Path]::GetFileName($package))" | Set-Content -LiteralPath "$package.sha256" -Encoding ascii
$inventory = Get-ChildItem -LiteralPath $stage -File -Recurse | ForEach-Object {
    [ordered]@{
        path = $_.FullName.Substring($stage.Length + 1)
        bytes = $_.Length
        sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}
$inventory | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath (Join-Path $output 'package-files.json') -Encoding utf8
Write-Output "Validated unsigned Store package: $package"
Write-Output "SHA256: $hash"
