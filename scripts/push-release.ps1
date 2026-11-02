# push-release.ps1 —— 开发机 → 发布机(la.trae.show) 发布对接脚本
# 组装契约交付目录(见服务器 easyssh-site/HANDOFF.md) → 上传 incoming/v<ver>/ →
# 远程执行 intake-release.sh → 回写双向状态(服务器 DEV-STATUS.md + 本地 release-log.md)
#
# 用法:
#   powershell -ExecutionPolicy Bypass -File scripts\push-release.ps1 -Version 4.3.1 [-FeaturesHtml <路径>]
# 前置: 已跑过 scripts\build-v4-local.ps1(dist-v4 下有全套产物),README 已含 v<版本> 小节
param(
    [Parameter(Mandatory=$true)][string]$Version,
    [string]$FeaturesHtml,
    [string]$Server = 'root@la.trae.show',
    [int]$Port = 4685,
    [string]$Key = 'P:\Program Files (x86)\EasySSH\keys\id_ed25519_easyssh',
    [switch]$StageOnly   # 仅上传交付目录到 incoming/,不执行 intake(不触发上线)
)
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Dist = Join-Path $Root 'dist-v4'

# ---- 1. 收集交付六件套 ----
$incoming = Join-Path $Root "dist-v4\incoming-v$Version"
Remove-Item $incoming -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $incoming | Out-Null
$files = @(
    "EasySSH-$Version.exe",
    "EasySSH-$Version-update.exe",
    "EasySSH-$Version-portable.exe",
    "EasySSH-$Version-win32-x64-portable.zip"
)
foreach ($f in $files) {
    $src = Join-Path $Dist $f
    if (-not (Test-Path $src)) { Write-Host "✗ 缺少产物 $src —— 请先跑 build-v4-local.ps1" -ForegroundColor Red; exit 1 }
    Copy-Item $src (Join-Path $incoming $f)
}
$readme = Join-Path $Root 'README.md'
Copy-Item $readme (Join-Path $incoming 'README.md') -Force
# features.html:默认交付仓库根副本(每次发版随功能变更更新);-FeaturesHtml 可显式指定其他路径
$featSrc = if ($FeaturesHtml) { $FeaturesHtml } else { Join-Path $Root 'features.html' }
if (Test-Path $featSrc) {
    Copy-Item $featSrc (Join-Path $incoming 'features.html') -Force
    Write-Host '    已包含 features.html(功能页交付)'
} else {
    Write-Host '⚠ 仓库根无 features.html——intake 将校验服务器现有页已含新版本号' -ForegroundColor Yellow
}
Select-String -Path (Join-Path $incoming 'README.md') -Pattern ("^### v" + $Version + ' ') -Quiet | ForEach-Object {
    if (-not $_) { Write-Host "✗ README 缺少 ### v$Version 更新历史小节" -ForegroundColor Red; exit 1 }
}
Write-Host "==> 交付目录就绪: $incoming"

# ---- 2. 上传到发布机 incoming ----
$remoteIncoming = "/opt/dsh/workspace/incoming/v$Version"
ssh -i $key -p $Port -o BatchMode=yes $Server ("rm -rf " + $remoteIncoming + " && mkdir -p " + $remoteIncoming)
scp -q -i $key -P $Port -o BatchMode=yes (Join-Path $incoming '*') ("$Server`:$remoteIncoming/")
Write-Host "==> 已上传到 $remoteIncoming"
if ($StageOnly) {
    $remote = ssh -i $key -p $Port -o BatchMode=yes $Server ("ls -la " + $remoteIncoming + " && grep -c '^### v" + $Version + " ' " + $remoteIncoming + "/README.md")
    $remote | ForEach-Object { Write-Host "  [remote] $_" }
    Write-Host "✅ StageOnly: 交付目录已就绪,未执行 intake。确认无误后去掉 -StageOnly 正式发布。" -ForegroundColor Green
    exit 0
}

# ---- 3. 远程执行 intake(官网发布唯一入口) ----
$intakeLog = ssh -i $key -p $Port -o BatchMode=yes $Server ("bash /opt/dsh/workspace/easyssh-site/tools/intake-release.sh " + $remoteIncoming + " 2>&1")
$intakeLog | ForEach-Object { Write-Host "  [intake] $_" }
$ok = ($intakeLog -join "`n") -match '发布完成'

# ---- 4. 双向状态回写 ----
$stamp = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
$actor = 'dev-machine(dsh)'
$line = "- $stamp [$actor] v$Version 交付+intake: $(if ($ok) {'SUCCESS'} else {'FAILED —— 见上方日志'})"
# 4.1 服务器侧 DEV-STATUS.md(发布机 dsh 可读)
$remoteStatus = "/opt/dsh/workspace/incoming/DEV-STATUS.md"
ssh -i $key -p $Port -o BatchMode=yes $Server ("echo '" + $line + "' >> " + $remoteStatus + " 2>/dev/null || echo '# DEV-STATUS 开发机状态日志' > " + $remoteStatus + " && echo '" + $line + "' >> " + $remoteStatus)
# 4.2 本地日志
Add-Content (Join-Path $Root 'release-log.md') $line

if ($ok) {
    Write-Host "✅ v$Version 已通过 intake 发布上线(webroot 约 5 分钟内生效)" -ForegroundColor Green
    # ---- 4.5 接收区清理：dist/release 只保留最新 3 版(历史版本只在本地归档与 GitHub/Gitee) ----
    ssh -i $key -p $Port -o BatchMode=yes $Server ("bash /opt/dsh/workspace/easyssh-site/tools/prune-v4-intake.sh 3 2>&1 | tail -4")
    # ---- 5. 仓库同步(GitHub + Gitee)：本机 commit → bundle 上传 → 服务器执行 push ----
    $git = Join-Path $Root 'tools\MinGit\cmd\git.exe'
    if (Test-Path $git) {
        $prevEap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
        Push-Location $Root   # git/bundle 必须在项目根执行
        & $git add -A 2>$null
        & $git commit -q -m "release: v$Version" 2>$null
        $bundle = Join-Path $env:TEMP ("easyssh-v4-$Version.bundle")
        & $git bundle create $bundle --all 2>&1 | Out-Null
Pop-Location
        $ErrorActionPreference = $prevEap
                scp -q -i $key -P $Port -o BatchMode=yes $bundle ("$Server`:/tmp/easyssh-v4-$Version.bundle")
        ssh -i $key -p $Port -o BatchMode=yes $Server ("bash /opt/dsh/workspace/easyssh-site/tools/repo-sync.sh /tmp/easyssh-v4-$Version.bundle " + $Version + " 2>&1 | tail -8; rm -f /tmp/easyssh-v4-$Version.bundle")
        Remove-Item $bundle -Force -ErrorAction SilentlyContinue
    } else {
        Write-Host '⚠ 未找到本机 git(tools\MinGit),跳过仓库同步' -ForegroundColor Yellow
    }
    Write-Host '   官网 healthcheck 可再跑一次确认'
} else {
    Write-Host '✗ intake 失败——根据上方日志修正后重跑本脚本' -ForegroundColor Red
    exit 1
}
