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
$script:FailReason = ''   # v4.3.8:各阶段失败原因采集,写入对接发布执行记录日志

# ---- 对接发布执行记录(双向回写,各保留最新 181 天) ----
# 本机权威源: release-log.md；服务器镜像: incoming/DEV-STATUS.md（发布机 dsh 可读）
function Write-ReleaseLog {
    param([string]$Stage, [string]$Result, [string]$Reason = '')
    $stamp = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
    $r = ($Reason -replace '\r?\n', ' / ').Trim()
    if ($r.Length -gt 200) { $r = $r.Substring(0, 200) + '…' }
    $line = "- $stamp [dev-machine(dsh)] v$Version $Stage`: $Result" + $(if ($r) { " — 原因: $r" } else { '' })
    $logFile = Join-Path $Root 'release-log.md'
    Add-Content $logFile $line -Encoding UTF8
    # 保留最新 181 天:清理过期记录行(表头/非记录行保留)
    $logKeep = (Get-Date).AddDays(-181)
    $all = Get-Content $logFile -Encoding UTF8 -ErrorAction SilentlyContinue
    $kept = $all | Where-Object {
        $l = $_
        if ($l -match '^- (\d{4}-\d{2}-\d{2}) ') {
            try { [datetime]::ParseExact($Matches[1], 'yyyy-MM-dd', $null) -ge $logKeep } catch { $true }
        } else { $true }
    }
    $kept | Set-Content $logFile -Encoding UTF8
    # 服务器镜像:整文件覆盖上传
    scp -q -i $Key -P $Port -o BatchMode=yes $logFile "$Server`:/opt/dsh/workspace/incoming/DEV-STATUS.md"
}

# ---- 1. 收集交付六件套 ----
$incoming = Join-Path $Root "dist-v4\incoming-v$Version"
Remove-Item $incoming -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $incoming | Out-Null
$files = @(
    "EasySSH-$Version.exe",
    "EasySSH-$Version-update.exe",
    "EasySSH-$Version-portable.exe",
    "EasySSH-$Version-win32-x64-portable.zip",
    "EasySSH-$Version-source.zip"
)
foreach ($f in $files) {
    $src = Join-Path $Dist $f
    if (-not (Test-Path $src)) {
        $script:FailReason = "缺少产物 $f —— 请先跑 build-v4-local.ps1"
        Write-Host "✗ $script:FailReason" -ForegroundColor Red
        Write-ReleaseLog -Stage '交付组装' -Result '失败' -Reason $script:FailReason
        exit 1
    }
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
    if (-not $_) {
        $script:FailReason = "README.md 缺少 ### v$Version 更新历史小节（官网 changelog 权威源）"
        Write-Host "✗ $script:FailReason" -ForegroundColor Red
        Write-ReleaseLog -Stage '交付组装' -Result '失败' -Reason $script:FailReason
        exit 1
    }
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

# ---- 4. 对接发布执行记录日志（双向回写，各保留最新 181 天） ----
# 本机权威源: release-log.md；服务器镜像: incoming/DEV-STATUS.md（发布机 dsh 可读）
if ($ok) {
    Write-ReleaseLog -Stage '交付+intake' -Result '成功'
} else {
    # v4.3.8:失败记录携带具体原因——优先取 intake 日志的 '!!' 错误行,否则取含 失败/错误 的行
    $reason = (($intakeLog | Where-Object { $_ -match '^\s*!!\s*(.+)$' } | ForEach-Object { $Matches[1] }) -join ' / ')
    if (-not $reason) { $reason = (($intakeLog | Where-Object { $_ -match '失败|错误' }) -join ' / ') }
    if (-not $reason) { $reason = 'intake 执行失败(详见上传日志)' }
    Write-ReleaseLog -Stage '交付+intake' -Result '失败' -Reason $reason
}

if ($ok) {
    Write-Host "✅ v$Version 已通过 intake 发布上线(webroot 约 5 分钟内生效)" -ForegroundColor Green
    # ---- 4.5 接收区清理：dist/release/incoming 只保留最新 3 版(历史版本只在本地归档与 GitHub/Gitee) ----
    ssh -i $key -p $Port -o BatchMode=yes $Server ("bash /opt/dsh/workspace/easyssh-site/tools/prune-v4-intake.sh 3 2>&1 | tail -4")
    # ---- 5. 仓库同步(GitHub + Gitee)：本机 commit → bundle 上传 → 服务器执行 push ----
    # v4.3.8:仓库同步结果同样写入执行记录(SUCCESS/FAILED+原因)
    $git = Join-Path $Root 'tools\MinGit\cmd\git.exe'
    if (Test-Path $git) {
        $prevEap = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
        Push-Location $Root   # git/bundle 必须在项目根执行
        & $git add -A 2>$null
        & $git commit -q -m "release: v$Version" 2>$null
        $bundle = Join-Path $env:TEMP ("easyssh-v4-$Version.bundle")
        & $git bundle create $bundle --all 2>&1 | Out-Null
        Pop-Location
        $ErrorActionPreference = $prevEap
        scp -q -i $key -P $Port -o BatchMode=yes $bundle ("$Server`:/tmp/easyssh-v4-$Version.bundle")
        $syncOut = ssh -i $key -p $Port -o BatchMode=yes $Server ("bash /opt/dsh/workspace/easyssh-site/tools/repo-sync.sh /tmp/easyssh-v4-$Version.bundle " + $Version + " 2>&1; rm -f /tmp/easyssh-v4-$Version.bundle")
        $syncOut | ForEach-Object { Write-Host "  [repo-sync] $_" }
        $syncOk = ($syncOut -join "`n") -match '仓库同步\+Release 完成'
        if ($syncOk) {
            Write-ReleaseLog -Stage '仓库同步(GitHub+Gitee)' -Result '成功'
        } else {
            $sr = (($syncOut | Where-Object { $_ -match '失败|fatal|error|!!' }) -join ' / ')
            if (-not $sr) { $sr = '输出无完成标记' }
            Write-ReleaseLog -Stage '仓库同步(GitHub+Gitee)' -Result '失败' -Reason ('输出报错(原文): ' + $sr)
        }
    } else {
        Write-Host '⚠ 未找到本机 git(tools\MinGit),跳过仓库同步' -ForegroundColor Yellow
        Write-ReleaseLog -Stage '仓库同步(GitHub+Gitee)' -Result '已跳过' -Reason '本机未找到 git'
    }
    Write-Host '   官网 healthcheck 可再跑一次确认'
} else {
    Write-Host '✗ intake 失败——失败原因已记录到 release-log.md 与服务器 DEV-STATUS.md' -ForegroundColor Red
    exit 1
}
