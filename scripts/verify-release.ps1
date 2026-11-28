# verify-release.ps1 —— 发布后自动验证（2026-11-28 固化）
# ① https://easyssh.cn/site/data/site-data.json 的 latestVersion/version == 本次版本（fallback: 首页 HTML 含 v<版本>）
# ② changelog 页含新版本号
# ③ POST /api/download/token.php（form-urlencoded, file=EasySSH-<版本>-portable.exe 及 zip）返回 ok:true
# 输出 PASS/FAIL 汇总；任一 FAIL → exit 1
# 用法: powershell -ExecutionPolicy Bypass -File scripts\verify-release.ps1 -Version 5.2.7
param(
    [Parameter(Mandatory=$true)][string]$Version,
    [string]$BaseUrl = 'https://easyssh.cn'
)
$ErrorActionPreference = 'Continue'
try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch {}
$results = @()
function Add-Result($name, $pass, $note) {
    $script:results += @{ Name=$name; Pass=$pass; Note=$note }
    $tag = if ($pass) { 'PASS' } else { 'FAIL' }
    Write-Host ("[{0}] {1} — {2}" -f $tag, $name, $note) -ForegroundColor $(if ($pass) { 'Green' } else { 'Red' })
}
function Get-Page([string]$url, [string]$ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36') {
    try {
        $r = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 30 -Headers @{ 'Cache-Control'='no-cache'; 'User-Agent'=$ua }
        return @{ Ok=$true; Body=$r.Content }
    } catch { return @{ Ok=$false; Body='' } }
}

# ① 官网 site-data latestVersion / 首页版本
#   实测确认（2026-11-28）：数据 URL 为 https://easyssh.cn/data/site-data.json（site.js fetch siteRoot + "data/site-data.json"）；
#   需带浏览器 UA（服务器 WAF 对裸 UA 返回 404/403）
$Ua = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36'
$sd = Get-Page ($BaseUrl + '/data/site-data.json') $Ua
$homeOk = $false; $note1 = ''
if ($sd.Ok) {
    try {
        $j = $sd.Body | ConvertFrom-Json
        $latest = $null
        if ($j.PSObject.Properties['latestVersion']) { $latest = $j.latestVersion }
        elseif ($j.PSObject.Properties['version']) { $latest = $j.version }
        if ($latest -eq $Version) { $homeOk = $true; $note1 = "site-data latestVersion=$latest" }
        else { $note1 = "site-data latestVersion=$latest 期望 $Version" }
    } catch { $note1 = 'site-data.json 解析失败: ' + $_.Exception.Message }
} else { $note1 = 'site-data.json 请求失败' }
if (-not $homeOk) {
    $hp = Get-Page $BaseUrl
    if ($hp.Ok -and $hp.Body -match ("v" + [regex]::Escape($Version))) { $homeOk = $true; $note1 += '；首页 HTML 含 v' + $Version }
}
Add-Result '首页/官网版本' $homeOk $note1

# ② changelog（changelog.html 为 JS 动态渲染，静态 HTML 不含版本文本——
#    以 /data/site-data.json 的 changelog 数组为准，HTML 兜底）
$clOk = $false; $clNote = ''
if ($sd.Ok) {
    try {
        $j2 = $sd.Body | ConvertFrom-Json
        if ($j2.PSObject.Properties['changelog']) {
            $hit = @($j2.changelog | Where-Object { ($_ | Out-String) -match ("v" + [regex]::Escape($Version)) -or ($_ -is [string] -and $_ -match [regex]::Escape($Version)) })
            $top = @($j2.changelog)[0]
            $topVer = $null
            if ($top -is [string]) { if ($top -match 'v?([\d.]+)') { $topVer = $Matches[1] } }
            elseif ($top.PSObject.Properties['version']) { $topVer = $top.version }
            if ($hit.Count -gt 0 -or $topVer -eq $Version) { $clOk = $true; $clNote = "site-data changelog 含 v$Version（changelog 置顶版本: $topVer）" }
            else { $clNote = "site-data changelog 未含 v$Version（置顶: $topVer）" }
        }
    } catch { $clNote = 'site-data changelog 解析失败: ' + $_.Exception.Message }
}
if (-not $clOk) {
    $cl = Get-Page ($BaseUrl + '/changelog.html')
    if ($cl.Ok -and $cl.Body -match ("v" + [regex]::Escape($Version))) { $clOk = $true; $clNote = 'changelog.html 静态含 v' + $Version }
    elseif (-not $clNote) { $clNote = 'changelog 校验失败（动态数据与静态页均未含新版本）' }
}
Add-Result 'changelog 新版本' $clOk $clNote

# ③ 下载 token 接口（动态白名单自动生效校验）
foreach ($f in @("EasySSH-$Version-portable.exe", "EasySSH-$Version-win32-x64-portable.zip")) {
    $tokOk = $false; $tokNote = ''
    try {
        $resp = Invoke-WebRequest -Uri ($BaseUrl + '/api/download/token.php') -Method Post `
            -ContentType 'application/x-www-form-urlencoded' -Body ("file=" + [Uri]::EscapeDataString($f)) `
            -Headers @{ 'User-Agent'=$Ua; 'Referer'=($BaseUrl + '/download.html'); 'Accept'='application/json' } `
            -UseBasicParsing -TimeoutSec 30
        $tokOk = ($resp.Content -match '"ok"\s*:\s*true')
        $tokNote = if ($tokOk) { "$f 返回 ok:true" } else { "$f 返回: " + $resp.Content.Substring(0, [Math]::Min(120, $resp.Content.Length)) }
    } catch { $tokNote = "$f 请求失败: " + $_.Exception.Message }
    Add-Result ("下载白名单 " + $f) $tokOk $tokNote
}

$failCount = @($results | Where-Object { -not $_.Pass }).Count
Write-Host ''
if ($failCount -eq 0) {
    Write-Host ("✅ verify-release v$Version 全部 PASS（{0} 项）" -f $results.Count) -ForegroundColor Green
    exit 0
} else {
    Write-Host ("✗ verify-release v$Version 失败 {0}/{1} 项" -f $failCount, $results.Count) -ForegroundColor Red
    exit 1
}
