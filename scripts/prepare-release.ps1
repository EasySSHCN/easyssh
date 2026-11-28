# prepare-release.ps1 —— 发布前人工动作自动化（2026-11-28 固化）
# ① 自动生成 README「版本更新历史记录」新版本小节（数据源=reqpool 需求池「已开发未发布」条目，或 -NoteFile 补充）
# ② 自动更新 features.html + en/features.html 功能卡（六分区关键词归类 + v<版本> 标注，英文机械英译+术语表）
# ③ 自动追加 site-server site-data.en.json 英文更新日志条目（置于数组最上方）
# ④ 自动关闭 360 实时防护（无权限时诚实告警并给出手动指令，不阻塞）
# 用法: powershell -ExecutionPolicy Bypass -File scripts\prepare-release.ps1 -Version 5.2.7 [-NoteFile <md>] [-SkipFeatures] [-WhatIfStyle]
# 干跑: 加 -WhatIfStyle 只输出计划不写任何文件
param(
    [Parameter(Mandatory=$true)][string]$Version,
    [string]$NoteFile,
    [string]$Root,          # 测试可覆盖项目根；默认在脚本体内按 $PSScriptRoot 解析
    [switch]$SkipFeatures,
    [switch]$WhatIfStyle
)
$ErrorActionPreference = 'Stop'
if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
$ReqDir = Join-Path $Root 'reqpool'
$PoolPath = Join-Path $ReqDir 'requirements-pool.json'
$BackupDir = Join-Path $Root 'release-backup'
$DateStr = (Get-Date).ToString('yyyy-MM-dd')

function Log($msg) { Write-Host ("[prepare] " + $msg) }
function Fail($msg) { Write-Host ("[prepare] ✗ " + $msg) -ForegroundColor Red; exit 1 }

# ---- 文件写入统一走 .NET，保留原文件 BOM 状态（PS5.1 兼容） ----
function Read-Text($path) {
    if (-not (Test-Path $path)) { throw "文件不存在: $path" }
    return [IO.File]::ReadAllText($path, [Text.Encoding]::UTF8)
}
function Has-Bom($bytes) { return ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) }
function Write-Text($path, $text) {
    $dir = Split-Path -Parent $path
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
    $bom = $false
    if (Test-Path $path) { $bom = Has-Bom ([IO.File]::ReadAllBytes($path)[0..2]) }
    $enc = New-Object Text.UTF8Encoding($bom)
    [IO.File]::WriteAllText($path, $text, $enc)
    Log ("写入: " + $path)
}

# ---- 机械英译 + 术语表 ----
$TermMap = [ordered]@{
    '端口转发'='port forwarding'; '主密码'='master password'; '敲门'='port knocking';
    '多标签'='multi-tab'; '分屏'='split panes'; '验证码'='verification code';
    '加密'='encryption'; '密钥'='key'; '会话'='session'; '连接'='connection';
    '终端'='terminal'; '上传'='upload'; '下载'='download'; '传输'='transfer';
    '文件'='file'; '代理'='proxy'; '隧道'='tunnel'; '网关'='gateway';
    '端口'='port'; '转发'='forwarding'; '安全'='security'; '审计'='audit';
    '签名'='signature'; '凭据'='credential'; '服务器'='server'; '版本'='version';
    '发布'='release'; '构建'='build'; '配置'='configuration'; '更新'='update';
    '升级'='upgrade'; '登录'='login'; '认证'='authentication'; '验证'='verification';
    '性能'='performance'; '稳定'='stability'; '自动'='automatic'; '网络'='network';
    '反向'='reverse'; '注册'='registration'; '资源'='resource'; '邮箱'='email';
    '修复'='Fixed'; '新增'='Added'; '优化'='Improved'; '改进'='Improved';
    '兼容'='compatibility'; '支持'='support'; '界面'='UI'; '主题'='theme';
    '宏'='macro'; '广播'='broadcast'; '云'='cloud'; '用户'='user'; '脚本'='script';
    '接口'='API'; '功能'='feature'; '增强'='enhanced'; '导入'='import'; '导出'='export';
    '标签'='tab'; '指纹'='fingerprint'; '口令'='password'; '守护'='guard';
    '服务'='service'; '任务'='task'; '日志'='log'; '目录'='directory';
    'SSH'='SSH'; 'SFTP'='SFTP'; 'CI'='CI'
}
function Translate-Zh([string]$s) {
    if (-not $s) { return $s }
    $out = $s -replace '\*\*', ''
    $i = 0; $tokens = @{}
    # 先按最长词条替换为占位 token，避免互相覆盖
    foreach ($k in $TermMap.Keys) {
        $len = $k.Length
        if ($out.Contains($k)) {
            $tok = ([char]1).ToString() + $i + ([char]2).ToString()
            $tokens[$tok] = $TermMap[$k]
            $out = $out.Replace($k, $tok)
            $i++
        }
    }
    # 未覆盖的中文连续串 → "<原文> (auto)" 占位
    $out = [regex]::Replace($out, '[\u4e00-\u9fff\u3000-\u303f\uff00-\uffef]+', { param($m) $m.Value + ' (auto)' })
    foreach ($t in $tokens.Keys) { $out = $out.Replace($t, $tokens[$t]) }
    return $out
}

# ---- 六分区归类（顺序敏感，先命中先归类；都不中 → 升级与维护） ----
$Sections = @(
    @{ Key='连接与会话';   Zh='<h2># 连接与会话</h2>';      En='<h2># Connections &amp; Sessions</h2>';      Kw=@('连接','会话','导入','Xshell','PuTTY','Termius','ProxyJump','代理','标签','分组','克隆','登录') },
    @{ Key='终端体验';     Zh='<h2># 终端体验</h2>';        En='<h2># Terminal Experience</h2>';             Kw=@('终端','分屏','主题','皮肤','宏','广播','渲染','界面','UI','语言','i18n','快捷','命令面板','字体') },
    @{ Key='文件传输';     Zh='<h2># 文件传输（SFTP）</h2>'; En='<h2># File Transfer (SFTP)</h2>';            Kw=@('SFTP','上传','下载','传输','文件','断点') },
    @{ Key='网络与转发';   Zh='<h2># 网络与转发</h2>';      En='<h2># Networking &amp; Forwarding</h2>';     Kw=@('转发','端口','SOCKS','隧道','敲门','knock','SPA','网关','gateway','网络','DNS','VPN') },
    @{ Key='密钥与安全';   Zh='<h2># 密钥与安全</h2>';      En='<h2># Keys &amp; Security</h2>';             Kw=@('密钥','主密码','加密','安全','审计','签名','DPAPI','OTP','指纹','凭据','口令') }
)
$DefaultSectionEn = '<h2># Upgrades &amp; Maintenance</h2>'
$DefaultSectionZh = '<h2># 升级与维护</h2>'

function Classify([string]$text) {
    foreach ($sec in $Sections) {
        foreach ($kw in $sec.Kw) { if ($text -match [regex]::Escape($kw)) { return $sec } }
    }
    return @{ Key='升级与维护'; Zh=$DefaultSectionZh; En=$DefaultSectionEn }
}

# ---- 摘要生成：detail 里已写好面向用户文案的直接用，否则截取合理摘要 ----
function Summarize-Detail([string]$title, [string]$detail) {
    $d = ''
    if ($detail) {
        $d = ($detail -replace '\*\*', '' -replace '\\n', ' ')
        # 优先取【功能】/【需求】/【现象】/【背景】段（多为面向用户的说明）
        $seg = [regex]::Matches($d, '【(功能|需求|现象|背景|优化|修复)】([^【]{20,400})')
        if ($seg.Count -gt 0) { $d = ($seg | ForEach-Object { $_.Groups[2].Value.Trim() }) -join '；' }
        if ($d.Length -gt 260) { $d = $d.Substring(0, 260).Trim() + '…' }
    }
    if (-not $d) { $d = $title }
    return $d
}

# ---- 收集条目 ----
$entries = @()
if ($NoteFile) {
    if (-not (Test-Path $NoteFile)) { Fail "NoteFile 不存在: $NoteFile" }
    $lines = Get-Content $NoteFile -Encoding UTF8 | Where-Object { $_.Trim() }
    foreach ($ln in $lines) {
        $t = ($ln.Trim() -replace '^-\s*', '') -replace '\*\*', ''
        $id = ''
        if ($t -match '（(R-\d{8}-\d{3})）') { $id = $Matches[1] }
        $parts = $t -split '：|:', 2
        $title = $parts[0].Trim() -replace '（R-\d{8}-\d{3}）', ''
        $rest = if ($parts.Count -gt 1) { $parts[1].Trim() } else { '' }
        $entries += @{ Id=$id; Title=$title; Detail=$rest }
    }
    Log ("NoteFile 提供条目 {0} 条" -f $entries.Count)
} else {
    $pool = Get-Content $PoolPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $entries = @($pool.items | Where-Object {
        $_.status -in @('已开发未发布', '待发布') -and -not $_.PSObject.Properties['releaseVersion']
    } | ForEach-Object { @{ Id=$_.id; Title=$_.title; Detail=$_.detail } })
    Log ("需求池「已开发未发布/待发布」条目 {0} 条（口径=status∈{{已开发未发布,待发布}} 且无 releaseVersion）" -f $entries.Count)
}
if ($entries.Count -eq 0) { Log '⚠ 无待发布条目：将只生成版本小节壳（可在 -NoteFile 补充说明）' }

# ---- 生成 README 小节 ----
$sb = New-Object Text.StringBuilder
[void]$sb.AppendLine("### v$Version ($DateStr)")
foreach ($e in $entries) {
    $summary = Summarize-Detail $e.Title $e.Detail
    $idTag = if ($e.Id) { "（$($e.Id)）" } else { '' }
    [void]$sb.AppendLine("- **$($e.Title)**$idTag`：$summary")
}
$readmeSection = $sb.ToString().TrimEnd()

# ---- WhatIfStyle：只输出计划 ----
if ($WhatIfStyle) {
    Log "=== WhatIf 干跑计划（不写任何文件） ==="
    Log ("README 将插入小节：`n" + $readmeSection)
    if (-not $SkipFeatures) {
        foreach ($e in $entries) {
            $sec = Classify (($e.Title + ' ' + $e.Detail))
            Log ("features 卡片 [{0}] {1} (v{2})" -f $sec.Key, $e.Title, $Version)
            Log ("  en 卡片: " + (Translate-Zh $e.Title) + " (v$Version)")
        }
    }
    if ($entries.Count -gt 0) {
        $enNote = (Translate-Zh $entries[0].Title) + ' (v' + $Version + ')'
        Log ("site-data.en.json 将置顶条目（首条）：`n" + (Translate-Zh ($readmeSection -replace '(?m)^### [^\r\n]*\r?\n', '')))
    } else {
        Log ('site-data.en.json 将新增版本数组置顶元素 { version: ' + $Version + ', notes: [英译条目] }')
    }
    Log ('将尝试关闭 360（360rp/ZhuDongFangYu + 360实时防护服务）')
    return
}

# ---- ① README ----
$ReadmePath = Join-Path $Root 'README.md'
$readme = Read-Text $ReadmePath
if ($readme -match "### v$([regex]::Escape($Version)) ") { Fail "README 已含 v$Version 小节，疑似重复执行，中止" }
if (-not (Test-Path $BackupDir)) { New-Item -ItemType Directory -Force $BackupDir | Out-Null }
$backup = Join-Path $BackupDir ("README-{0}-{1}.md" -f $Version, (Get-Date -Format 'yyyyMMdd-HHmmss'))
Copy-Item $ReadmePath $backup -Force
Log ("README 已备份: " + $backup)
$marker = '## 版本更新历史记录'
$idx = $readme.IndexOf($marker)
if ($idx -lt 0) { Fail 'README 未找到「版本更新历史记录」锚点' }
$nl = $readme.IndexOf("`n", $idx)
if ($nl -lt 0) { $nl = $readme.Length }
$newReadme = $readme.Substring(0, $nl + 1) + "`n" + $readmeSection + "`n" + $readme.Substring($nl + 1)
try { Write-Text $ReadmePath $newReadme } catch { Fail ("README 写入失败: " + $_.Exception.Message) }

# ---- ② features.html 双语功能卡 ----
if (-not $SkipFeatures -and $entries.Count -gt 0) {
    foreach ($pair in @(@{ Path=(Join-Path $Root 'features.html'); ZhTitle=$true },
                        @{ Path=(Join-Path $Root 'en\features.html'); ZhTitle=$false })) {
        $fpath = $pair.Path
        $html = Read-Text $fpath
        if ($html -match "v$([regex]::Escape($Version))\b" -and ($html -match [regex]::Escape($entries[0].Title))) {
            Log ("⚠ " + $fpath + " 疑似已含本版卡片，跳过重复插入")
            continue
        }
        $cardsZh = @{}; $cardsEn = @{}
        foreach ($e in $entries) {
            $sec = Classify ($e.Title + ' ' + $e.Detail)
            $sumZh = Summarize-Detail $e.Title $e.Detail
            $titleZh = $e.Title; $titleEn = Translate-Zh $e.Title
            $sumEn = Translate-Zh $sumZh
            $zhCard = '<div class="card"><h3>🆕 ' + $titleZh + '（v' + $Version + '）</h3><p>' + $sumZh + '</p></div>'
            $enCard = '<div class="card"><h3>🆕 ' + $titleEn + ' (v' + $Version + ')</h3><p>' + $sumEn + '</p></div>'
            if (-not $cardsZh[$sec.Zh]) { $cardsZh[$sec.Zh] = @() }
            $cardsZh[$sec.Zh] += $zhCard
            if (-not $cardsEn[$sec.En]) { $cardsEn[$sec.En] = @() }
            $cardsEn[$sec.En] += $enCard
        }
        $anchorMap = if ($pair.ZhTitle) { $cardsZh } else { $cardsEn }
        foreach ($anchor in $anchorMap.Keys) {
            $ai = $html.IndexOf($anchor)
            if ($ai -lt 0) { Log ("⚠ " + $fpath + " 未找到分区锚点，卡片放「升级与维护」: " + $anchor); $ai = $html.IndexOf($DefaultSectionEn); $anchor = $DefaultSectionEn; if ($ai -lt 0) { Fail ("features 分区锚点缺失: " + $fpath) } }
            $gi = $html.IndexOf('<div class="grid', $ai)
            if ($gi -lt 0) { Fail ("features 分区网格缺失: " + $fpath + " " + $anchor) }
            $gle = $html.IndexOf('>', $gi) + 1
            $insert = ($anchorMap[$anchor] -join "`n          ") + "`n          "
            $html = $html.Substring(0, $gle) + $insert + $html.Substring($gle)
        }
        if (-not ($html -match "v$([regex]::Escape($Version))")) { Fail ("英文页缺 v$Version 标注（双语强校验会拦截）: " + $fpath) }
        try { Write-Text $fpath $html } catch { Fail ("features 写入失败: " + $fpath + " — " + $_.Exception.Message) }
    }
}

# ---- ③ site-data.en.json 英文更新日志置顶 ----
# 结构为按版本数组：[ {version, date, notes:[...]}, ... ]，最新版本在最前
$EnDataPath = Join-Path $Root 'site-server\site\assets\data\site-data.en.json'
# 防重护栏：解析首元素版本号
$firstVer = $null
try {
    $firstRaw = Read-Text $EnDataPath | ConvertFrom-Json
    if ($firstRaw) { $firstVer = @($firstRaw)[0].version }
} catch { $firstVer = $null }
if ($firstVer -eq $Version) {
    Log "⚠ site-data.en.json 已含 v$Version 条目，跳过重复置顶"
} else {
    $enBody = $readmeSection -replace '(?m)^### [^\r\n]*\r?\n', ''
    $enNote = Translate-Zh $enBody
    if (-not $enNote) { $enNote = "Release v$Version" }
    $newEntry = [pscustomobject]@{ version = $Version; date = $DateStr; notes = @($enNote) }
    $newEntryJson = ConvertTo-Json $newEntry -Depth 5 -Compress
    # 文本拼接插入：PS5.1 ConvertFrom-Json 顶层数组经枚举/再序列化会嵌套破坏结构，
    # 改为在首元素前字面拼接新元素，原数组内容逐字节保留
    $raw = Read-Text $EnDataPath
    $bi = $raw.IndexOf('[')
    if ($bi -lt 0) { Fail 'site-data.en.json 顶层不是数组，中止' }
    $rawNew = $raw.Substring(0, $bi + 1) + $newEntryJson + ",`r`n" + $raw.Substring($bi + 1)
    $encNoBom = New-Object Text.UTF8Encoding($false)
    try { [IO.File]::WriteAllText($EnDataPath, $rawNew, $encNoBom); Log ("写入: " + $EnDataPath + "（v$Version 置顶条目）") } catch { Fail ("site-data.en.json 写入失败: " + $_.Exception.Message) }
}

# ---- ④ 关闭 360（尽力而为，诚实报告） ----
$proc360 = Get-Process -Name '360rp','ZhuDongFangYu' -ErrorAction SilentlyContinue
$svc360 = Get-Service -ErrorAction SilentlyContinue | Where-Object { $_.Name -like '*360*' -or $_.DisplayName -like '*360实时防护*' }
$killedAny = $false
if ($proc360) {
    foreach ($p in $proc360) {
        try { taskkill /F /PID $p.Id 2>$null | Out-Null; Log ("已结束进程 " + $p.Name + " (pid " + $p.Id + ")"); $killedAny = $true }
        catch { Log ("⚠ 进程 " + $p.Name + " 结束失败: " + $_.Exception.Message) }
    }
}
if ($svc360) {
    foreach ($s in $svc360) {
        try { Stop-Service $s.Name -Force -ErrorAction Stop; Log ("已停止服务 " + $s.Name); $killedAny = $true }
        catch {
            Log ("⚠ 无管理员权限或服务停止失败: " + $s.Name + "（" + $_.Exception.Message + "）")
            Write-Host '   手动指令: 管理员 PowerShell 执行  Stop-Service "360实时防护" -Force  或右键 360 托盘退出实时防护' -ForegroundColor Yellow
        }
    }
}
if (-not $proc360 -and -not $svc360) { Log '未检测到 360 实时防护进程/服务，无需关闭' }
if ($killedAny) {
    if (-not (Test-Path $BackupDir)) { New-Item -ItemType Directory -Force $BackupDir | Out-Null }
    Set-Content (Join-Path $BackupDir "360-killed-$Version.flag") ("closed at " + (Get-Date -Format s)) -Encoding UTF8
    Write-Host '✅ 360 实时防护已尝试关闭（发布完成后 push-release 尾部将自动尝试恢复）' -ForegroundColor Green
}
Write-Host ("✅ prepare-release v$Version 完成：README 小节 + features 双语卡 + 英文更新日志 + 360 检查") -ForegroundColor Green
