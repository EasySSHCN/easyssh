# smoke-gate.ps1 —— v5(M2)/F-P-02 发版冒烟门禁（push-release.ps1 上传前强制执行）
# 门禁项:
#   1) 产物存在性（便携版 exe 必须在 dist 目录）
#   2) 版本一致性（exe FileVersion == 配置版本）
#   3) 启动冒烟（分离启动便携版 → 8 秒内进程存活 → 强制结束）
# 任一失败 → 中止发布并写入 release-log.md（与 v5(M1) 对接发布执行记录日志规范一致）。
# 应急绕过：-SkipSmoke（仅限应急发布，须在执行记录中留痕）。
# 文档：docs/v5/冒烟门禁.md
param(
    [Parameter(Mandatory=$true)][string]$Version,
    [string]$Dist,   # 产物目录（默认 <repo>\dist-v4）
    [string]$Root,   # 仓库根（默认脚本上级目录）
    [switch]$SkipSmoke
)
$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
if (-not $Dist) { $Dist = Join-Path $Root 'dist-v4' }

# ---- 执行记录日志（与 push-release.ps1 的 Write-ReleaseLog 格式一致） ----
function Write-GateLog {
    param([string]$Stage, [string]$Result, [string]$Reason = '')
    $stamp = (Get-Date).ToString('yyyy-MM-dd HH:mm:ss')
    $r = ($Reason -replace '\r?\n', ' / ').Trim()
    if ($r.Length -gt 200) { $r = $r.Substring(0, 200) + '…' }
    $line = "- $stamp [dev-machine(dsh)] v$Version $Stage`: $Result" + $(if ($r) { " — 原因: $r" } else { '' })
    $logFile = Join-Path $Root 'release-log.md'
    Add-Content $logFile $line -Encoding UTF8
}

function Fail {
    param([string]$Reason)
    Write-Host "✗ 冒烟门禁失败: $Reason" -ForegroundColor Red
    Write-GateLog '冒烟门禁' '失败' $Reason
    exit 1
}

# ---- 应急绕过 ----
if ($SkipSmoke) {
    Write-Host '⚠ --skip-smoke：已跳过冒烟门禁（仅限应急发布，已在执行记录留痕）' -ForegroundColor Yellow
    Write-GateLog '冒烟门禁' '已跳过' '调用方显式指定 skip-smoke（应急发布）'
    return
}

Write-Host "==> 冒烟门禁(v$Version)..."

# ---- 门禁 1：产物存在性 ----
$portable = Join-Path $Dist "EasySSH-$Version-portable.exe"
if (-not (Test-Path $portable)) {
    Fail "缺少便携版产物 $portable —— 请先跑构建脚本"
}

# ---- 门禁 2：版本一致性（FileVersion == 配置版本） ----
$fv = (Get-Item $portable).VersionInfo.FileVersion
if ($fv -ne $Version) {
    Fail "便携版 FileVersion($fv) 与配置版本($Version) 不一致"
}
Write-Host "  [gate-1/2] 产物存在且 FileVersion=$fv ✅"

# ---- 门禁 3：启动冒烟（分离启动 → 8 秒存活 → taskkill） ----
$p = Start-Process -FilePath $portable -PassThru
Start-Sleep -Seconds 8
$alive = -not $p.HasExited
if ($alive) {
    try { Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue } catch {}
}
if (-not $alive) {
    $code = $null
    try { $code = $p.ExitCode } catch {}
    $reason = "启动冒烟失败：进程 8 秒内退出(ExitCode=$code)。若本机已运行 360 安全卫士，请确认已将 dist-v4 目录加入白名单后重试 —— 360 疑似拦截。"
    Fail $reason
}
Write-Host '  [gate-3]   启动冒烟通过(8 秒存活→已结束) ✅'

Write-Host "✅ 冒烟门禁通过(v$Version)" -ForegroundColor Green
Write-GateLog '冒烟门禁' '通过'
exit 0
