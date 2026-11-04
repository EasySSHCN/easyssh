# release-check.ps1 —— 需求池每日巡检(12:00,系统级定时)
# 规则:池中状态=记录 的条目数 >= 3 → 触发一次发布周期:
#   - 计算下一版本号(next-version.ps1 规则:patch+1;patch 到 10 升 minor;minor 到 10 升 major)
#   - 写出发布请求标志(release-request.json),由 dsh 会话执行开发/构建/发布
#   - 邮件通知 ad@6xm.net(经 notify-email.ps1;通道未配置时进 outbox)
param(
    [string]$PoolPath = 'P:\Program Files (x86)\EasySSH\reqpool\requirements-pool.json',
    [string]$FlagPath = 'P:\Program Files (x86)\EasySSH\reqpool\release-request.json',
    [string]$LogPath = 'P:\Program Files (x86)\EasySSH\reqpool\release-check.log',
    [string]$RepoRoot = 'B:\Users\ZhenAn\Documents\deepseek-harness\default-workspace\EasySSH-v5'
)
$ErrorActionPreference = 'Stop'
function Log($msg) { Add-Content -Path $LogPath -Value ("- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $msg") -Encoding UTF8 }
try {
    $pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $pending = @($pool.items | Where-Object { $_.status -eq '记录' })
    Log "巡检:待开发记录 $($pending.Count) 条"

    # 已有未处理的发布请求则不重复写
    if (Test-Path $FlagPath) { Log '已有未处理发布请求,跳过'; exit 0 }

    if ($pending.Count -ge 3) {
        $cur = (Get-Content (Join-Path $RepoRoot 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json).version
        $next = & (Join-Path $RepoRoot 'scripts\next-version.ps1') -Current $cur
        @{ createdAt = (Get-Date).ToString('s'); currentVersion = $cur; nextVersion = $next;
           pendingIds = @($pending | ForEach-Object { $_.id }); status = 'requested' } |
            ConvertTo-Json -Depth 5 | Set-Content $FlagPath -Encoding UTF8
        $idList = (@($pending | ForEach-Object { $_.id }) -join ', ')
        Log "达到 3 条待开发记录 → 生成发布请求 v$cur → v$next(条目: $idList)"
        & (Join-Path $RepoRoot 'scripts\notify-email.ps1') `
            -Subject "[EasySSH] 需求池触发发布周期 v$cur → v$next" `
            -Text ("需求与反馈记录池待开发记录已达 3 条,触发发布周期。`r`n当前版本: v$cur`r`n计划版本: v$next`r`n条目: " + $idList + "`r`n`r`n由 dsh 会话执行开发与发布,完成后另行通知。")
        exit 0
    }
    Log '未达 3 条,不触发发布'
} catch {
    Log "巡检异常: $($_.Exception.Message)"
    exit 1
}
