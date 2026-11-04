# harvest-feedback.ps1 —— 官网使用反馈采集入库(需求与反馈记录池)
# 每天 08:00 / 20:00 由 Windows 任务计划程序调用(系统级定时)。
# 功能:SSH 到发布机读取 site/data/feedback-db.json,把池中尚不存在的新反馈
#       分析后追加为「记录」状态条目;采集过程写入 harvest.log。
# 机密:池文件在仓库之外(P:\Program Files (x86)\EasySSH\reqpool\),禁止外泄/提交。
param(
    [string]$Server = 'root@la.trae.show',
    [int]$Port = 4685,
    [string]$Key = 'P:\Program Files (x86)\EasySSH\keys\id_ed25519_easyssh',
    [string]$PoolPath = 'P:\Program Files (x86)\EasySSH\reqpool\requirements-pool.json',
    [string]$LogPath = 'P:\Program Files (x86)\EasySSH\reqpool\harvest.log'
)
$ErrorActionPreference = 'Stop'
function Log($msg) {
    $line = "- $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $msg"
    Add-Content -Path $LogPath -Value $line -Encoding UTF8
}
try {
    # ---- 1. 拉取官网反馈库 ----
    $remote = 'cat /opt/dsh/workspace/easyssh-site/site/data/feedback-db.json'
    $raw = ssh -i $Key -p $Port -o BatchMode=yes -o ConnectTimeout=20 $Server $remote
    if (-not $raw) { Log '采集失败:远端反馈库为空或 SSH 失败'; exit 1 }
    $fb = $raw | ConvertFrom-Json
    $items = @($fb.items)

    # ---- 2. 读池 ----
    $pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $known = @{}
    foreach ($it in $pool.items) {
        if ($it.sourceRef) { $known[$it.sourceRef] = $true }
        if ($it.id -like 'fb-*') { $known[$it.id] = $true }
    }

    # ---- 3. 新反馈入库(状态=记录) ----
    $now = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss+08:00')
    $added = 0
    foreach ($f in $items) {
        $fid = [string]$f.id
        if (-not $fid -or $known.ContainsKey($fid)) { continue }
        $type = [string]$f.type
        if ($type -notin @('功能建议','问题反馈','其他')) { $type = '其他' }
        $content = ([string]$f.content).Trim()
        if (-not $content) { continue }
        # 摘要:截断到 60 字作为 title;完整内容放 detail
        $title = $content -replace '\s+', ' '
        if ($title.Length -gt 60) { $title = $title.Substring(0, 60) + '…' }
        $newItem = [ordered]@{
            id             = "R-$(Get-Date -Format 'yyyyMMdd')-$('{0:D3}' -f ($pool.items.Count + 1 + $added))"
            type           = $type
            title          = $title
            source         = '官网反馈'
            sourceRef      = $fid
            detail         = $content
            status         = '记录'
            createdAt      = $now
            statusHistory  = @([ordered]@{ status = '记录'; at = $now; note = "自动采集入库(feedback-db $fid,提交于 $($f.date))" })
        }
        $pool.items += New-Object PSObject -Property $newItem
        $known[$fid] = $true
        $added++
    }

    if ($added -gt 0) {
        $pool | ConvertTo-Json -Depth 10 | Set-Content $PoolPath -Encoding UTF8
        Log "采集完成:新增 $added 条(池内共 $($pool.items.Count) 条)"
    } else {
        Log '采集完成:无新增反馈'
    }
    exit 0
} catch {
    Log "采集异常: $($_.Exception.Message)"
    exit 1
}
