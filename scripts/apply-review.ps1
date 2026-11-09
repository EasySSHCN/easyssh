# apply-review.ps1 —— 应用 dsh 会话的 AI 复审结论
# 用法: powershell -File scripts\apply-review.ps1 -Verdicts <verdicts.json>
# verdicts.json: [ { "fbId": "...", "verdict": "real|spam|reject", "note": "判断理由(可选)" } ]
#   real  → 池条目(若有)置「记录」,官网自动回复并公开 TOP10
#   spam  → 池条目(若有)移除,官网下架(清回复、不公开)
#   reject→ 池条目置「拒绝」,官网回复不纳入说明并下架
# 同时从 review-queue.json 移除已判定条目,写 review.log。
param(
    [Parameter(Mandatory=$true)][string]$Verdicts,
    [string]$PoolPath = (Join-Path $PSScriptRoot '..\reqpool\requirements-pool.json'),
    [string]$QueuePath = (Join-Path $PSScriptRoot '..\reqpool\review-queue.json'),
    [string]$LogPath = (Join-Path $PSScriptRoot '..\reqpool\review.log'),
    [string]$Server = 'root@la.trae.show',
    [int]$Port = 4685,
    [string]$Key = 'P:\Program Files (x86)\EasySSH\keys\id_ed25519_easyssh'
)
$ErrorActionPreference = 'Stop'
function Log($m) { Add-Content $LogPath ("- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $m") -Encoding UTF8 }
$now = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss+08:00')

$vd = Get-Content $Verdicts -Raw -Encoding UTF8 | ConvertFrom-Json
$pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
$updates = @()
$applied = 0

foreach ($v in @($vd)) {
    $fbId = [string]$v.fbId
    $ver = [string]$v.verdict
    $poolItem = @($pool.items | Where-Object { $_.sourceRef -eq $fbId }) | Select-Object -First 1
    switch ($ver) {
        'real' {
            if ($poolItem) {
                if ($poolItem.status -eq '待审') {
                    $poolItem.status = '记录'
                    $poolItem.statusHistory += New-Object PSObject -Property @{ status = '记录'; at = $now; note = "AI复审判定为真实反馈$(if ($v.note) {' · ' + $v.note})" }
                }
            } else {
                # 未入池(被启发式拦截但 AI 认为真实)→ 补录
                $content = [string]$v.content
                if (-not $content) { Log "real 但无 content 且未入池,跳过 $fbId"; continue }
                $title = $content -replace '\s+', ' '
                if ($title.Length -gt 60) { $title = $title.Substring(0, 60) + '…' }
                $type = if ($v.type) { [string]$v.type } else { '其他' }
                $pool.items += New-Object PSObject -Property ([ordered]@{
                    id = "R-$(Get-Date -Format 'yyyyMMdd')-$('{0:D3}' -f ($pool.items.Count + 1))"; type = $type; title = $title
                    source = '官网反馈'; sourceRef = $fbId; detail = $content; status = '记录'; createdAt = $now
                    statusHistory = @(New-Object PSObject -Property @{ status = '记录'; at = $now; note = "AI复审补录$(if ($v.note) {' · ' + $v.note})" })
                })
            }
            $poolIds = @($pool.items | ForEach-Object { $_.id })
            $rid = ($pool.items | Where-Object { $_.sourceRef -eq $fbId } | Select-Object -First 1).id
            $updates += [ordered]@{ fbId = $fbId; reply = "感谢反馈!该内容已收录至需求与反馈记录池(编号 $rid),将按优先级评估并安排开发,处理进展会随版本发布同步,感谢对 EasySSH 的支持。(自动回复)"; public = $true }
        }
        'spam' {
            if ($poolItem) { $pool.items = @($pool.items | Where-Object { $_.sourceRef -ne $fbId }); Log "spam: 池移除 $($poolItem.id)($fbId)" }
            $updates += [ordered]@{ fbId = $fbId; reply = ''; public = $false }
        }
        'reject' {
            if ($poolItem) {
                $poolItem.status = '拒绝'
                $poolItem.statusHistory += New-Object PSObject -Property @{ status = '拒绝'; at = $now; note = "AI复审判定不纳入$(if ($v.note) {' · ' + $v.note})" }
            }
            $updates += [ordered]@{ fbId = $fbId; reply = "感谢反馈!经评估,该内容暂不纳入开发计划,已停止公开展示。感谢对 EasySSH 的支持。(自动回复)"; public = $false }
        }
        default { Log "未知 verdict '$ver'($fbId),跳过"; continue }
    }
    $applied++
}

if ($applied -gt 0) {
    $pool | ConvertTo-Json -Depth 10 | Set-Content $PoolPath -Encoding UTF8
    # 官网回写
    $updPath = "$env:TEMP\fb-updates.json"
    @{ u = $updates } | ConvertTo-Json -Depth 5 | Set-Content $updPath -Encoding UTF8
    scp -q -i $Key -P $Port -o BatchMode=yes $updPath "$Server`:/tmp/fb-updates.json"
    ssh -i $Key -p $Port -o BatchMode=yes $Server 'python3 /opt/dsh/workspace/easyssh-site/tools/fb-sync.py && bash /opt/dsh/workspace/easyssh-site/tools/deploy.sh 2>&1 | tail -1' | ForEach-Object { Log "同步回写: $_" }
    Remove-Item $updPath -Force -ErrorAction SilentlyContinue
}

# 队列:移除已判定条目
if (Test-Path $QueuePath) {
    $q = Get-Content $QueuePath -Raw -Encoding UTF8 | ConvertFrom-Json
    $qarr = @($q)
    $vids = @($vd | ForEach-Object { [string]$_.fbId })
    $qarr = @($qarr | Where-Object { $vids -notcontains [string]$_.fbId })
    @{ q = $qarr } | ConvertTo-Json -Depth 5 | Set-Content $QueuePath -Encoding UTF8
}
Log "复审结论已应用 $applied 条,队列剩余 $($qarr.Count) 条"
Write-Host "✅ 已应用 $applied 条复审结论,队列剩余 $($qarr.Count) 条"
