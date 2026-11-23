# build-v4-local.ps1 —— EasySSH v4 本机(Windows/MSVC)发布构建
# 用法: powershell -ExecutionPolicy Bypass -File scripts\build-v4-local.ps1 [-OpenDir]
# 产物: dist\EasySSH-<ver>-{portable.exe,win32-x64-portable.zip,update.exe,source.zip}
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
# v5.0.0-alpha:预发布版本(含 -alpha.N 后缀)的 NSIS 版本字段须为纯数字 x.x.x.x,取基础版本
$VerBase = ($Version -replace '-.*$','')

# ---- 2. UI 语法检查(v5:ui/js/ 模块 + 桥接与静态页脚本) ----
foreach ($f in (Get-ChildItem (Join-Path $UI 'js') -Filter '*.js' -ErrorAction SilentlyContinue)) {
    node --check $f.FullName
    if ($LASTEXITCODE -ne 0) { Write-Host "✗ $($f.Name) 语法检查失败" -ForegroundColor Red; exit 1 }
}
foreach ($f in @('tauri-bridge.js','assets\i18n.js')) {
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
$stage = Join-Path $Root "dist\EasySSH-$Version"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe (Join-Path $stage 'EasySSH.exe')

# ---- 5. 打 zip + 单文件 portable exe + 官网同步产物 + SHA256 ----
# 单文件版:裸 exe 改名即可(MSVC 构建已静态链接 WebView2Loader,无需任何伴随文件;
# 运行时在 exe 同目录自动生成 keys/、config.json、config.key 等,机制不变)
$portable = Join-Path $Root "dist\EasySSH-$Version-portable.exe"
Copy-Item (Join-Path $stage 'EasySSH.exe') $portable -Force
# 绿色便携 zip:官网 sync.js 识别的官方命名(主发布物)
# v4.3.7 修正:不再额外生成 -win64.zip 冗余副本(与官方命名 zip 内容完全相同)
#   - EasySSH-<ver>-update.exe:在线升级载荷(裸 app exe,升级器免解压直换)
$zip = Join-Path $Root "dist\EasySSH-$Version-win32-x64-portable.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
# v4.3.6:先复制到临时目录再压缩——运行中的 exe/安全软件扫描会让 ZipArchive 直读失败
$tmpZipStage = Join-Path $env:TEMP ("ezstage-" + $Version)
Remove-Item $tmpZipStage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $tmpZipStage | Out-Null
Copy-Item (Join-Path $stage '*') $tmpZipStage -Recurse -Force
# 360 实时扫描会短暂锁定刚复制的 exe,重试 5 次(每次间隔 5 秒)
$zipped = $false
for ($zi = 1; $zi -le 5; $zi++) {
  try { Compress-Archive -Path (Join-Path $tmpZipStage '*') -DestinationPath $zip -Force -ErrorAction Stop; $zipped = $true; break }
  catch { Write-Host ("  zip 第 " + $zi + " 次失败(文件被占用),5 秒后重试…"); Start-Sleep 5 }
}
if (-not $zipped) { Write-Host 'zip 打包反复失败' -ForegroundColor Red; exit 1 }
Copy-Item (Join-Path $stage 'EasySSH.exe') (Join-Path $Root "dist\EasySSH-$Version-update.exe") -Force

# ---- 5.5 源码包（本机构建，替代发布机 sync.js 打包） ----
# git archive 仅导出已跟踪文件：.gitignore 命中的 target/dist/tools(含密钥)/update-keys.json 自动排除
# R-20261120-009: easyssh-gateway/ 独立源码目录已跟踪(.gitignore 不排除该目录,仅排除其密钥真件),
# 随 git add -A 入库后自动进入本源码包；打包前先过下方 5.6 前置脱敏门禁
$gitExe = Join-Path $Root 'tools\MinGit\cmd\git.exe'
if (Test-Path $gitExe) {
    $prevEap2 = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    Push-Location $Root
    & $gitExe add -A 2>$null
    $tree = & $gitExe write-tree
    & $gitExe archive --format=zip -o (Join-Path $Root "dist\EasySSH-$Version-source.zip") $tree 2>$null
    Pop-Location
    $ErrorActionPreference = $prevEap2
    if (Test-Path (Join-Path $Root "dist\EasySSH-$Version-source.zip")) { Write-Host '  source.zip 已生成(本机构建)' }
    else { Write-Host '⚠ 源码包生成失败(不影响其余产物)' -ForegroundColor Yellow }
} else {
    Write-Host '⚠ 未找到 MinGit，跳过源码包' -ForegroundColor Yellow
}

# ---- 5.6 easyssh-gateway 网关构建与部署套件（R-20261120-009） ----
# 源码目录 EasySSH\easyssh-gateway\（独立源码库，无独立版本号，随主版本发布）。
# 目录尚未落地时跳过（网关源码落地前保持流程兼容，不阻塞客户端构建）。
# 产物: dist\easyssh-gateway-<ver>\{easyssh-gateway[.exe], config.toml.template, deploy.sh, keys.example.php, SHA256SUMS}
#       dist\EasySSH-<ver>-gateway-deploy.zip（push-release 随版交付 → intake 落位不重启）
$GwDir = Join-Path $Root 'easyssh-gateway'
if (Test-Path (Join-Path $GwDir 'Cargo.toml')) {
    # 脱敏门禁: 网关目录内禁止出现私钥真件/keys.php 真件(源码包与部署套件都不得携带真密钥)
    $leaks = @()
    Get-ChildItem $GwDir -Recurse -File | Where-Object { $_.FullName -notmatch '\\target(\\|$)' } | ForEach-Object {
        if (Select-String -Path $_.FullName -Pattern 'BEGIN (RSA |EC |OPENSSH |DSA |PGP )?PRIVATE KEY' -Quiet) { $leaks += $_.FullName }
    }
    $phpKeys = Get-ChildItem $GwDir -Recurse -File -Filter 'keys.php' -ErrorAction SilentlyContinue
    if ($phpKeys) { $leaks += ($phpKeys.FullName) }
    if ($leaks) {
        Write-Host '✗ 网关脱敏门禁: 检出疑似密钥真件,禁止入源码包/部署套件:' -ForegroundColor Red
        $leaks | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
        exit 1
    }
    Write-Host '==> 网关脱敏门禁通过(无私钥真件/keys.php)'
    # cargo build --release(网关为独立 workspace,用自身 manifest)
    $prevEapG = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
    Push-Location $GwDir
    & $cargo build --release 2>&1 | Tee-Object -Variable gwLog | Select-Object -Last 3
    $gcode = $LASTEXITCODE
    Pop-Location
    $ErrorActionPreference = $prevEapG
    if ($gcode -ne 0) { Write-Host '✗ easyssh-gateway release 构建失败' -ForegroundColor Red; exit 1 }
    # 二进制: 网关自身 workspace 的 target\release(Windows 后缀 .exe;Linux 交叉编译产物无后缀)
    $gbin = Get-ChildItem (Join-Path $GwDir 'target\release') -Filter 'easyssh-gateway*' -File |
        Where-Object { $_.Extension -in @('', '.exe') } | Select-Object -First 1
    if (-not $gbin) { Write-Host '✗ 未找到 easyssh-gateway 二进制' -ForegroundColor Red; exit 1 }
    # 部署套件组装
    $gstage = Join-Path $Root "dist\easyssh-gateway-$Version"
    Remove-Item $gstage -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force $gstage | Out-Null
    Copy-Item $gbin.FullName (Join-Path $gstage $gbin.Name) -Force
    # Web 服务台套件（R-20261121-002）：从仓库同步申请/服务台页面与接口到 webconsole/，随套件交付
    $wcDir = Join-Path $GwDir 'deploy-gateway\webconsole'
    New-Item -ItemType Directory -Force $wcDir | Out-Null
    New-Item -ItemType Directory -Force (Join-Path $wcDir 'api\gateway') | Out-Null
    foreach ($wf in @('apply.php','audit.php','issue.php','keys.example.php')) { Copy-Item (Join-Path (Join-Path $Root 'site-server\site\api\gateway') $wf) (Join-Path $wcDir ('api\gateway\' + $wf)) -Force }
    foreach ($wf in @('gw-admin.html','apply.html')) { Copy-Item (Join-Path $Root ('site-server\site\' + $wf)) (Join-Path $wcDir $wf) -Force }
    foreach ($xa in @('install-webconsole.sh','README.md')) { Copy-Item (Join-Path $GwDir ('deploy-gateway\' + $xa)) (Join-Path $gstage $xa) -Force }
    Copy-Item (Join-Path $Root 'site-server\tools\gateway-apply-audit.php') (Join-Path $gstage 'gateway-apply-audit.php') -Force
    foreach ($ga in @('config.toml.template', 'deploy.sh', 'keys.example.php')) {
        # 模板优先取 easyssh-gateway\deploy-gateway\ 部署套件源目录，回退网关根目录
        $gasrc = Join-Path (Join-Path $GwDir 'deploy-gateway') $ga
        if (-not (Test-Path $gasrc)) { $gasrc = Join-Path $GwDir $ga }
        if (Test-Path $gasrc) { Copy-Item $gasrc (Join-Path $gstage $ga) -Force }
        else { Write-Host "⚠ 网关部署套件缺少 $ga(模板未就绪,跳过该件)" -ForegroundColor Yellow }
    }
    $gsums = Join-Path $gstage 'SHA256SUMS'
    Get-ChildItem $gstage -File | ForEach-Object {
        Add-Content $gsums ("{0}  {1}" -f ((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLower()), $_.Name) -Encoding ASCII
    }
    $gzip = Join-Path $Root "dist\EasySSH-$Version-gateway-deploy.zip"
    Remove-Item $gzip -Force -ErrorAction SilentlyContinue
    Compress-Archive -Path (Join-Path $gstage '*') -DestinationPath $gzip -Force
    Write-Host '✅ easyssh-gateway 部署套件已生成' -ForegroundColor Green
    Write-Host ("  套件目录: {0}" -f $gstage)
    Write-Host ("  套件 zip : {0}  SHA256 : {1}" -f $gzip, ((Get-FileHash $gzip -Algorithm SHA256).Hash.ToLower()))
} else {
    Write-Host 'ℹ easyssh-gateway/ 源码不存在,跳过网关构建(R-20261120-009 流程已就绪)'
}
$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
$hashP = (Get-FileHash $portable -Algorithm SHA256).Hash.ToLower()

# ---- 6. NSIS 单文件引导安装器（官网对接契约要求的 EasySSH-<ver>.exe） ----
$makensis = Join-Path $Root 'tools\NSIS\nsis-3.10\Bin\makensis.exe'
if (Test-Path $makensis) {
    $wv = Get-ChildItem (Join-Path $Root 'target\release\build\webview2-com-sys-*\out\x64\WebView2Loader.dll') -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($wv) {
        $nsiOut = Join-Path $Root "dist\EasySSH-$Version.exe"
        $appExe = Join-Path $stage 'EasySSH.exe'
        & $makensis `
            "-DAPP_EXE=$appExe" `
            "-DLOADER_DLL=$($wv.FullName)" `
            "-DOUTFILE=$nsiOut" `
            "-DICON=$(Join-Path $Root 'src-tauri\icons\icon.ico')" `
            "-DVERSION=$VerBase" `
            "-DVER4=$VerBase.0" `
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
