# send-appeals-v515.ps1 —— 通过 mail API 向各安全厂商发送 EasySSH v5.1.5 白名单申诉邮件
# 背景:2026-11-16 Windows Defender 将 EasySSH-5.1.5-portable.exe 误报为 Trojan:Win32/Bearfoos.A!ml 并隔离
#       (本地文件 SHA-256 与 GitHub Release 官方 digest 一致,确认为误报)
# 凭据:DPAPI 加密存于 reqpool\mail-api.cred(机密,禁止外泄);发件联系邮箱 ad@6xm.net
# 微软官方明确不接受邮件样本提交(filesubmission 门户是唯一通道),故收件清单不含微软。
$ErrorActionPreference = 'Stop'
$credPath = Join-Path $PSScriptRoot '..\reqpool\mail-api.cred'
$c = Get-Content $credPath -Raw | ConvertFrom-Json
$key = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR((ConvertTo-SecureString $c.key)))
$api = "$($c.api)/v1/send"

$body = @"
Dear Antivirus Vendor / Security Team,

We are submitting a FALSE POSITIVE / WHITELIST request for EasySSH v5.1.5.
(2026-11-16, Microsoft Defender detected Trojan:Win32/Bearfoos.A!ml on our
portable executable and quarantined it; local SHA256 matches our official
GitHub Release digest exactly, confirming a false positive.)

== Product introduction ==
EasySSH is an open-source (MIT license) Windows SSH/SFTP terminal client.
Website: https://easyssh.cn
Source:  https://github.com/EasySSHCN/easyssh  |  https://gitee.com/easyssh/EasySSH
Built with Tauri 2 (Rust + system WebView2). Unsigned freeware, NOT packed (no UPX),
portable: writes nothing outside its own folder, no registry, no admin rights,
no bundleware, no silent installation. Network activity is limited to user-initiated
SSH connections (TCP 22 etc.) and HTTPS update/feedback endpoints. The OTA channel
uses Ed25519 signature verification + AES-256-GCM encrypted payloads.

== The 3 publicly distributed files (please whitelist these SHA256 hashes) ==
1. EasySSH-5.1.5-portable.exe   (portable single-file executable, main false-positive target)
   SHA256 e1a4f53fc2f1ffb7541e67fb7a5271cff7d87dfd3f622bf88b1be38fb6613096
   https://easyssh.cn/downloads/EasySSH-5.1.5-portable.exe
   (GitHub Release mirror: https://github.com/EasySSHCN/easyssh/releases/download/v5.1.5/EasySSH-5.1.5-portable.exe)
2. EasySSH-5.1.5.exe            (NSIS installer)
   SHA256 2b8e4fd160c2cab13848ed48ca2a45f4d17dc987d7df25238bb1880d00f250ce
   https://easyssh.cn/downloads/EasySSH-5.1.5.exe
3. EasySSH-5.1.5-win32-x64-portable.zip (portable zip package)
   SHA256 64e30944fa807afddd99820c9b2b6c6a30e2717ee42ac6dcae9baf93decab228
   https://easyssh.cn/downloads/EasySSH-5.1.5-win32-x64-portable.zip
Checksums: https://easyssh.cn/downloads/SHA256SUMS

== False positive behaviour ==
The portable executable is flagged/quarantined by Windows Defender as
Trojan:Win32/Bearfoos.A!ml (an ML-heuristic verdict) on extraction or run.

Please add the hashes above to your whitelist / trusted database.
Contact: ad@6xm.net  (replies in Chinese or English are both fine)

--

中文说明:EasySSH 是开源(MIT 协议)的 Windows SSH/SFTP 终端工具,官网 easyssh.cn,
Tauri 2(Rust + WebView2)构建,绿色便携、不写系统目录与注册表、不需要管理员权限、
未加壳、无捆绑无静默安装;网络行为仅为用户主动发起的 SSH 连接与 HTTPS 升级/反馈接口。
2026-11-16 Windows Defender 将 EasySSH-5.1.5-portable.exe 误报为
Trojan:Win32/Bearfoos.A!ml 并隔离(本地 SHA256 与官方 GitHub Release 摘要一致,确认为误报)。
上述 3 个文件被误报,申请加入白名单/可信库。联系方式:ad@6xm.net
"@

$targets = @(
    @{ to = 'svc@huorong.cn';                  tag = 'Huorong' },
    @{ to = 'opensoft@360.cn';                 tag = '360' },
    @{ to = 'vlab@eset.com';                   tag = 'ESET' },
    @{ to = 'virus_submission@trendmicro.com'; tag = 'TrendMicro' },
    @{ to = 'virus@avast.com';                 tag = 'Avast' },
    @{ to = 'falsepositive@kaspersky.com';     tag = 'Kaspersky' },
    @{ to = 'falsepositive@bitdefender.com';   tag = 'Bitdefender' },
    @{ to = 'support@mcafee.com';              tag = 'McAfee' }
)
foreach ($t in $targets) {
    $payload = @{ to = @($t.to); subject = "[False Positive / Whitelist Request] EasySSH v5.1.5 (open-source SSH terminal, Defender Bearfoos.A!ml FP)"; text = $body } | ConvertTo-Json
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
    try {
        $r = Invoke-RestMethod -Uri $api -Method Post -TimeoutSec 40 -Headers @{ Authorization = "Bearer $key" } -ContentType 'application/json; charset=utf-8' -Body $bytes
        $json = ($r | ConvertTo-Json -Compress)
        Write-Host ("OK   {0} -> {1}" -f $t.tag, $json.Substring(0, [Math]::Min(120, $json.Length)))
    } catch {
        $msg = $_.Exception.Message
        try { $sr = New-Object IO.StreamReader($_.Exception.Response.GetResponseStream()); $msg = $sr.ReadToEnd() } catch {}
        Write-Host ("FAIL {0} -> {1}" -f $t.tag, $msg)
    }
    Start-Sleep -Seconds 3
}
