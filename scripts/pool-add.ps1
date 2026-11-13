# pool-add.ps1 —— 需求池条目登记(通用流程化入口)
# 用途:把一条需求(含「已开发未发布」补录)追加进 reqpool\requirements-pool.json。
# 流程:条目 JSON 先写到 reqpool\pending-items\<id>.json,再执行本脚本:
#   powershell -ExecutionPolicy Bypass -File scripts\pool-add.ps1 -ItemFile reqpool\pending-items\R-<yyyymmdd>-<nnn>.json
# 校验(全部通过才写入,防止脏数据破坏 release-check.ps1 的触发计数口径):
#   - 条目 JSON 可解析;必填字段齐全(id/type/title/source/sourceRef/detail/createdAt/status)
#   - id 格式 R-<yyyymmdd>-<流水> 且池内唯一
#   - status 仅允许 记录/待开发(池的触发计数只认这两种;完成发布等状态流转由发布会话改)
#   - status=记录 时禁止携带 releaseVersion(防 release-check 防过期护栏告警)
#   - statusHistory 非空且首条 status 与条目 status 一致(留痕)
#   - JSON 文件名与条目 id 一致
# 写入成功后:条目源文件移入 pending-items\done\,日志记 reqpool\pool-add.log。
param(
    [Parameter(Mandatory)][string]$ItemFile,
    [string]$PoolPath,
    [string]$LogPath
)
# PS5.1 的 $PSScriptRoot 在参数默认值求值阶段不可用 → 在脚本体内解析默认路径
$ScriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
if (-not $PoolPath) { $PoolPath = Join-Path $ScriptRoot '..\reqpool\requirements-pool.json' }
if (-not $LogPath) { $LogPath = Join-Path $ScriptRoot '..\reqpool\pool-add.log' }
$ErrorActionPreference = 'Stop'
function Log($msg) { Add-Content -Path $LogPath -Value ("- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $msg") -Encoding UTF8 }
try {
    if (-not (Test-Path $ItemFile)) { throw "条目文件不存在: $ItemFile" }
    $item = Get-Content $ItemFile -Raw -Encoding UTF8 | ConvertFrom-Json
    $required = 'id','type','title','source','sourceRef','detail','createdAt','status'
    foreach ($f in $required) {
        if (-not $item.PSObject.Properties[$f] -or [string]::IsNullOrWhiteSpace([string]$item.$f)) { throw "缺少必填字段: $f" }
    }
    if ($item.id -notmatch '^R-\d{8}-\d{3,4}$') { throw "id 格式不符(应为 R-<yyyymmdd>-<流水>): $($item.id)" }
    $fileNameId = [IO.Path]::GetFileNameWithoutExtension($ItemFile)
    if ($fileNameId -ne $item.id) { throw "文件名($fileNameId)与条目 id($($item.id))不一致" }
    if ($item.status -notin @('记录','待开发')) { throw "status 仅允许 记录/待开发,当前: $($item.status)" }
    if ($item.status -eq '记录' -and $item.PSObject.Properties['releaseVersion']) { throw "status=记录 的条目不得携带 releaseVersion(触发计数护栏)" }
    if (-not $item.statusHistory -or $item.statusHistory.Count -lt 1) { throw "statusHistory 不能为空(必须留痕)" }
    if ($item.statusHistory[0].status -ne $item.status) { throw "statusHistory 首条($($item.statusHistory[0].status))与条目 status($($item.status))不一致" }
    if ($item.statusHistory[0].at -ne $item.createdAt) { throw "statusHistory 首条 at 应等于 createdAt" }

    $pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ($pool.items | Where-Object { $_.id -eq $item.id }) { throw "池内已存在同 id 条目: $($item.id)" }

    $pool.items = @($pool.items) + $item
    $pool | ConvertTo-Json -Depth 12 | Set-Content $PoolPath -Encoding UTF8

    # 复核(写后读回验证,保证文件仍可解析且条目在池)
    $chk = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not ($chk.items | Where-Object { $_.id -eq $item.id })) { throw "写入复核失败:条目不在池中" }
    $pending = @($chk.items | Where-Object { $_.status -eq '记录' -and -not $_.PSObject.Properties['releaseVersion'] })
    Log "登记成功 $($item.id)「$($item.title.Substring(0, [Math]::Min(40, $item.title.Length)))」;池共 $($chk.items.Count) 条,待开发 $($pending.Count) 条($(($pending | ForEach-Object { $_.id }) -join ', '))"

    $doneDir = Join-Path (Split-Path -Parent $ItemFile) 'done'
    New-Item -ItemType Directory -Force $doneDir | Out-Null
    Move-Item $ItemFile (Join-Path $doneDir (Split-Path -Leaf $ItemFile)) -Force

    Write-Host "✅ $($item.id) 已登记;当前待开发 $($pending.Count) 条(≥3 触发发布周期)" -ForegroundColor Green
    exit 0
} catch {
    Log "登记失败: $($_.Exception.Message)"
    Write-Host "✗ 登记失败: $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
