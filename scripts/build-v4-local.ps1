# build-v4-local.ps1 —— EasySSH v4 本机(Windows/MSVC)发布构建
# 用法: powershell -ExecutionPolicy Bypass -File scripts\build-v4-local.ps1 [-OpenDir]
# 产物: dist-v4\EasySSH-<ver>-win64.zip (EasySSH.exe + WebView2Loader.dll)
param([switch]$OpenDir)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Src  = Join-Path $Root 'src-tauri'
$UI   = Join-Path $Root 'ui'

# ---- 0. 360 实时防护提醒 ----
$av = Get-Process | Where-Object { $_.ProcessName -in @('360rp','360sd','ZhuDongFangYu','360tray') }
if ($av) {
    Write-Host '⚠ 检测到 360 进程在运行:' ($av.ProcessName -join ',') -ForegroundColor Yellow
    Write-Host '  未签名的新编译 exe/DLL 可能被实时删除导致 LNK1104。建议先关闭 360 实时防护再继续。' -ForegroundColor Yellow
    $go = Read-Host '仍要继续构建吗? (y/N)'
    if ($go -ne 'y') { exit 1 }
}

# ---- 1. 版本号(唯一来源 tauri.conf.json,并校验与 Cargo.toml 一致) ----
$conf = Get-Content (Join-Path $Src 'tauri.conf.json') -Raw | ConvertFrom-Json
$Version = $conf.version
$cargoVer = (Select-String -Path (Join-Path $Src 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
if ($Version -ne $cargoVer) {
    Write-Host "✗ 版本号不一致: tauri.conf.json=$Version vs Cargo.toml=$cargoVer,先同步两处!" -ForegroundColor Red
    exit 1
}
Write-Host "==> 版本: v$Version"

# ---- 2. UI 语法检查 ----
foreach ($f in @('renderer.js','tauri-bridge.js','assets\i18n.js')) {
    node --check (Join-Path $UI $f)
    if ($LASTEXITCODE -ne 0) { Write-Host "✗ $f 语法检查失败" -ForegroundColor Red; exit 1 }
}
Write-Host '==> UI 语法检查通过'

# ---- 3. cargo build --release (原生 MSVC) ----
$cargo = "$env:USERPROFILE\.cargo\bin\cargo.exe"
$prevEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'   # cargo 的 stderr 警告不应中断构建
Push-Location $Src
& $cargo build --release 2>&1 | Tee-Object -Variable buildLog | Select-Object -Last 3
$code = $LASTEXITCODE
Pop-Location
$ErrorActionPreference = $prevEap
if ($code -ne 0) { Write-Host '✗ release 构建失败' -ForegroundColor Red; exit 1 }

# Cargo workspace:产物在项目根 target\(不是 src-tauri\target)
$exe = Join-Path $Root 'target\release\easyssh.exe'
if (-not (Test-Path $exe)) { Write-Host "✗ 缺少 $exe" -ForegroundColor Red; exit 1 }

# ---- 4. 收集产物 ----
# v4.2.3(本机):MSVC 构建已静态链接 WebView2Loader,裸 exe 可独立运行(已实测),
# zip 与 portable 均为单文件,不再携带 WebView2Loader.dll
$stage = Join-Path $Root "dist-v4\EasySSH-$Version"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe (Join-Path $stage 'EasySSH.exe')

# ---- 5. 打 zip + 单文件 portable exe + 官网同步产物 + SHA256 ----
# 单文件版:裸 exe 改名即可(MSVC 构建已静态链接 WebView2Loader,无需任何伴随文件;
# 运行时在 exe 同目录自动生成 keys/、config.json、config.key 等,机制不变)
$portable = Join-Path $Root "dist-v4\EasySSH-$Version-portable.exe"
Copy-Item (Join-Path $stage 'EasySSH.exe') $portable -Force
# v4.3.1:官网 sync.js 识别的官方命名产物(上传主仓 dist/release 用)
#   - EasySSH-<ver>-win32-x64-portable.zip:官网下载位命名
#   - EasySSH-<ver>-update.exe:在线升级载荷(裸 app exe,升级器免解压直换)
$zip = Join-Path $Root "dist-v4\EasySSH-$Version-win64.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
Copy-Item $zip (Join-Path $Root "dist-v4\EasySSH-$Version-win32-x64-portable.zip") -Force
Copy-Item (Join-Path $stage 'EasySSH.exe') (Join-Path $Root "dist-v4\EasySSH-$Version-update.exe") -Force
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
$hashP = (Get-FileHash $portable -Algorithm SHA256).Hash.ToLower()

# ---- 6. NSIS 单文件引导安装器（官网对接契约要求的 EasySSH-<ver>.exe） ----
$makensis = Join-Path $Root 'tools\NSIS\nsis-3.10\Bin\makensis.exe'
if (Test-Path $makensis) {
    $wv = Get-ChildItem (Join-Path $Root 'target\release\build\webview2-com-sys-*\out\x64\WebView2Loader.dll') -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($wv) {
        $nsiOut = Join-Path $Root "dist-v4\EasySSH-$Version.exe"
        $appExe = Join-Path $stage 'EasySSH.exe'
        & $makensis `
            "-DAPP_EXE=$appExe" `
            "-DLOADER_DLL=$($wv.FullName)" `
            "-DOUTFILE=$nsiOut" `
            "-DICON=$(Join-Path $Root 'src-tauri\icons\icon.ico')" `
            "-DVERSION=$Version" `
            "-DVER4=$Version.0" `
            (Join-Path $Root 'nsis\bootstrap-v4.nsi') 2>&1 | Select-Object -Last 2
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path $nsiOut)) { Write-Host '✗ NSIS 打包失败(不影响其余产物,但官网 intake 会拒收)' -ForegroundColor Red; exit 1 }
    } else {
        Write-Host '⚠ 未找到 WebView2Loader.dll,跳过 NSIS 打包' -ForegroundColor Yellow
    }
} else {
    Write-Host '⚠ 未安装 NSIS(tools\NSIS),跳过引导安装器打包——官网 intake 将拒收' -ForegroundColor Yellow
}

Write-Host ''
Write-Host '✅ 构建完成(zip 与 portable 均为单文件 EasySSH.exe)' -ForegroundColor Green
Write-Host ("  portable: {0}  ({1:N1} MB)" -f $portable, ((Get-Item $portable).Length/1MB))
Write-Host ("    SHA256 : {0}" -f $hashP)
Write-Host ("  zip     : {0}  ({1:N1} MB)" -f $zip, ((Get-Item $zip).Length/1MB))
Write-Host ("    SHA256 : {0}" -f $hash)
Write-Host ("  发布说明请同步 README「版本更新历史记录」中 v$Version 条目")

if ($OpenDir) { explorer $stage } else { explorer (Split-Path -Parent $stage) }
