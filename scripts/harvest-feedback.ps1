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
    # ---- 0. 内容质量审核(2026-11-04 用户要求:垃圾/测试内容不得入池、不得回复、一律下架) ----
    # 启发式:1) 单字符连跑≥10  2) 任意6字符片段重复≥3次(复读机式灌水)  3) 平台测试占位标记  4) 去空白后唯一字符占比<0.2
    function Test-Junk([string]$c) {
        if (-not $c) { return $true }
        $t = ($c -replace '\s+', '')
        if ($t.Length -lt 6) { return $true }
        $run = 1
        for ($i = 1; $i -lt $t.Length; $i++) {
            if ($t[$i] -eq $t[$i-1]) { $run++; if ($run -ge 10) { return $true } } else { $run = 1 }
        }
        if ($t -match '可能含有违规信息') { return $true }
        $grams = @{}
        for ($i = 0; $i + 6 -le $t.Length; $i++) {
            $g = $t.Substring($i, 6)
            $grams[$g] = [int]$grams[$g] + 1
            if ($grams[$g] -ge 3) { return $true }
        }
        $uniq = ($t.ToCharArray() | Select-Object -Unique).Count
        if ($t.Length -ge 20 -and ($uniq / $t.Length) -lt 0.2) { return $true }
        return $false
    }

    # ---- 1. 拉取官网反馈库(webroot 实时库——用户提交直接写入此处;site/data 仅是种子,deploy 不同步) ----
    $remote = 'cat /www/wwwroot/easyssh.cn/data/feedback-db.json'
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

    # ---- 3. 新反馈审核入库(三路分流) ----
    # 垃圾(启发式命中)→ 下架不入池;高置信真实 → 入池「记录」+自动回复公开;
    # 不确定 → 入池「待审」+写入 AI 复审队列(不回复不公开),由 dsh 会话判定后回填。
    $now = (Get-Date).ToString('yyyy-MM-ddTHH:mm:ss+08:00')
    $added = 0
    $junked = 0
    $pending = 0
    $updates = @()   # 下架更新(垃圾反馈)
    $QueuePath = 'P:\Program Files (x86)\EasySSH\reqpool\review-queue.json'
    $queue = @()
    if (Test-Path $QueuePath) { try { $q = Get-Content $QueuePath -Raw -Encoding UTF8 | ConvertFrom-Json; $queue = @($q) } catch {} }
    $inQueue = @{}; foreach ($qit in $queue) { $inQueue[$qit.fbId] = $true }
    foreach ($f in $items) {
        $fid = [string]$f.id
        if (-not $fid -or $known.ContainsKey($fid)) { continue }
        $content = ([string]$f.content).Trim()
        if (-not $content) { continue }
        if (Test-Junk $content) {
            # 垃圾/测试内容:不入池,直接从官网下架(清回复、不公开)
            $updates += [ordered]@{ fbId = $fid; reply = ''; public = $false }
            $junked++
            continue
        }
        $type = [string]$f.type
        if ($type -notin @('功能建议','问题反馈','其他')) { $type = '其他' }
        # 置信判定:含明确诉求词且长度足够 → 高置信真实(快速通道);否则 → 待审(AI 复审)
        $fast = ($content.Length -ge 20) -and ($content -match '希望|建议|支持|增加|添加|优化|改进|能不能|可不可以|方便|问题|错误|异常|失败|崩溃|卡顿|闪退|很慢|bug|BUG|兼容|导入|导出|连接|下载|上传|传输|界面|字体|主题|快捷键|分屏')
        $status = if ($fast) { '记录' } else { '待审' }
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
            status         = $status
            createdAt      = $now
            statusHistory  = @([ordered]@{ status = $status; at = $now; note = "自动采集入库(feedback-db $fid,提交于 $($f.date))$(if (-not $fast) {' · 待AI复审'})" })
        }
        $pool.items += New-Object PSObject -Property $newItem
        $known[$fid] = $true
        $added++
        if (-not $fast) {
            $pending++
            if (-not $inQueue.ContainsKey($fid)) {
                $queue += [ordered]@{ fbId = $fid; poolId = ([string]$newItem.id); type = $type; content = $content; date = [string]$f.date; queuedAt = $now }
            }
        }
    }
    if ($queue.Count -gt 0) {
        @{ q = $queue } | ConvertTo-Json -Depth 5 | Set-Content $QueuePath -Encoding UTF8
    }

    if ($added -gt 0) {
        $pool | ConvertTo-Json -Depth 10 | Set-Content $PoolPath -Encoding UTF8
        Log "采集完成:新增 $added 条(快速通道 $(($added - $pending)) / 待AI复审 $pending),拦截垃圾 $junked 条(池内共 $($pool.items.Count) 条)"
    } elseif ($junked -gt 0) {
        Log "采集完成:无新增真实反馈,拦截垃圾 $junked 条"
    } else {
        Log '采集完成:无新增反馈'
    }

    # ---- 4. 双向同步:凡来源为官网反馈且已入池的条目,自动回复并按状态公开/下架 ----
    # 规则(用户 2026-11-04 指定):
    #   入池(记录/开发/完成发布) → 自动回复官方进展并公开到 TOP10;
    #   拒绝/暂缓                → 回复不纳入说明并从 TOP10 下架(public=false);
    #   垃圾/测试内容            → 不入池,清回复并下架;
    #   未入池                   → 不回复、不公开。
    # 合并采集阶段产生的垃圾下架更新($updates),后续池内条目更新追加到同一数组
    # 合并采集阶段产生的垃圾下架更新($updates),后续池内条目更新追加到同一数组
    foreach ($it in $pool.items) {
        if ($it.source -ne '官网反馈' -or -not $it.sourceRef) { continue }
        $st = [string]$it.status
        if ($st -in @('拒绝','暂缓')) {
            $reply = "感谢反馈!经评估,该内容暂不纳入开发计划(记录编号 $($it.id)),已停止公开展示。感谢对 EasySSH 的支持。(自动回复)"
            $pub = $false
        } elseif ($st -eq '待审') {
            # AI 复审中:不回复、不公开
            $reply = ''
            $pub = $false
        } elseif ($st -eq '开发') {
            $reply = "感谢反馈!该内容已收录至需求与反馈记录池(编号 $($it.id)),目前正在开发中,将随下个版本发布。(自动回复)"
            $pub = $true
        } elseif ($st -eq '完成发布') {
            $ver = if ($it.releaseVersion) { "已随 v$($it.releaseVersion) 正式发布" } else { '已开发完成并随新版本发布' }
            $reply = "感谢反馈!该内容已收录至需求与反馈记录池(编号 $($it.id)),$ver,可在官网下载最新版本体验。(自动回复)"
            $pub = $true
        } else {
            $reply = "感谢反馈!该内容已收录至需求与反馈记录池(编号 $($it.id)),将按优先级评估并安排开发,处理进展会随版本发布同步,感谢对 EasySSH 的支持。(自动回复)"
            $pub = $true
        }
        $updates += [ordered]@{ fbId = [string]$it.sourceRef; reply = $reply; public = $pub }
    }
    if ($updates.Count -gt 0) {
        $updPath = "$env:TEMP\fb-updates.json"
        @{ u = $updates } | ConvertTo-Json -Depth 5 | Set-Content $updPath -Encoding UTF8
        # fb-sync.py 自身兼容 {"u":[...]} 包装与裸数组
        scp -q -i $Key -P $Port -o BatchMode=yes $updPath "$Server`:/tmp/fb-updates.json"
        $sync = ssh -i $Key -p $Port -o BatchMode=yes $Server 'python3 /opt/dsh/workspace/easyssh-site/tools/fb-sync.py && bash /opt/dsh/workspace/easyssh-site/tools/deploy.sh 2>&1 | tail -1'
        $sync | ForEach-Object { Log "同步回写: $_" }
        Remove-Item $updPath -Force -ErrorAction SilentlyContinue
    }
    exit 0
} catch {
    Log "采集异常: $($_.Exception.Message)"
    exit 1
}
