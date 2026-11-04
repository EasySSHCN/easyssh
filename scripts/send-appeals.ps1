# send-appeals.ps1 —— 通过 mail API 向各安全厂商发送 EasySSH v5.0.6 白名单申诉邮件
# 凭据:DPAPI 加密存于 reqpool\mail-api.cred(机密,禁止外泄);发件联系邮箱 ad@6xm.net
$ErrorActionPreference = 'Stop'
$c = Get-Content 'P:\Program Files (x86)\EasySSH\reqpool\mail-api.cred' -Raw | ConvertFrom-Json
$key = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR((ConvertTo-SecureString $c.key)))
$api = "$($c.api)/v1/send"

$body = @"
Dear Antivirus Vendor / Security Team,

We are submitting a FALSE POSITIVE / WHITELIST request for EasySSH v5.0.6.

== Product introduction ==
EasySSH is an open-source (MIT license) Windows SSH/SFTP terminal client.
Website: https://easyssh.cn
Source:  https://github.com/EasySSHCN/easyssh  |  https://gitee.com/easyssh/EasySSH
Built with Tauri 2 (Rust + system WebView2). Unsigned freeware, NOT packed (no UPX),
portable: writes nothing outside its own folder, no registry, no admin rights,
no bundleware, no silent installation. Network activity is limited to user-initiated
SSH connections (TCP 22 etc.) and HTTPS update/feedback endpoints. The v5.0 OTA
channel uses Ed25519 signature verification + AES-256-GCM encrypted payloads.

== The 5 publicly distributed files (please whitelist these SHA256 hashes) ==
1. EasySSH-5.0.6.exe            (NSIS installer)
   SHA256 ba292acc4166d1d034837f3b37f5e61bb956438c8ed70c0846f93fd255bc781b
   https://easyssh.cn/downloads/EasySSH-5.0.6.exe
2. EasySSH-5.0.6-portable.exe   (portable main executable)
   SHA256 dbed2ec0d6fce3995d30050a35ee73c6742342b4b943ede45197068c4622da14
   https://easyssh.cn/downloads/EasySSH-5.0.6-portable.exe
3. EasySSH-5.0.6-win32-x64-portable.zip (portable zip package)
   SHA256 a5b75a12878c9b88f3eadd6e17041739773bac7b36d04aba202be8c13a7c06e1
   https://easyssh.cn/downloads/EasySSH-5.0.6-win32-x64-portable.zip
4. EasySSH-5.0.6-update.exe     (OTA update payload, same bytes as #2)
   SHA256 dbed2ec0d6fce3995d30050a35ee73c6742342b4b943ede45197068c4622da14
   https://easyssh.cn/downloads/EasySSH-5.0.6-update.exe
5. EasySSH-5.0.6-source.zip     (full source code, MIT)
   SHA256 592f90c5156a7dea9f6cf19977add46cd2a974eb73407804e9eec568556bd852
   https://easyssh.cn/downloads/EasySSH-5.0.6-source.zip
Checksums: https://easyssh.cn/downloads/SHA256SUMS

== False positive behaviour ==
The portable executable is flagged/quarantined by your product on extraction or run.

Please add the hashes above to your whitelist / trusted database.
Contact: ad@6xm.net  (replies in Chinese or English are both fine)

--

中文说明:EasySSH 是开源(MIT 协议)的 Windows SSH/SFTP 终端工具,官网 easyssh.cn,
Tauri 2(Rust + WebView2)构建,绿色便携、不写系统目录与注册表、不需要管理员权限、
未加壳、无捆绑无静默安装;网络行为仅为用户主动发起的 SSH 连接与 HTTPS 升级/反馈接口。
上述 5 个文件被贵司产品误报,申请加入白名单/可信库。联系方式:ad@6xm.net
"@

$targets = @(
    @{ to = 'opensoft@360.cn';              tag = '360' },
    @{ to = 'vlab@eset.com';                tag = 'ESET' },
    @{ to = 'virus_submission@trendmicro.com'; tag = 'TrendMicro' },
    @{ to = 'virus@avast.com';              tag = 'Avast' },
    @{ to = 'falsepositive@kaspersky.com';  tag = 'Kaspersky' },
    @{ to = 'falsepositive@bitdefender.com'; tag = 'Bitdefender' },
    @{ to = 'svc@huorong.cn';               tag = 'Huorong' },
    @{ to = 'support@mcafee.com';           tag = 'McAfee' }
)
foreach ($t in $targets) {
    $payload = @{ to = @($t.to); subject = "[False Positive / Whitelist Request] EasySSH v5.0.6 (open-source SSH terminal)"; text = $body } | ConvertTo-Json
    try {
        $r = Invoke-RestMethod -Uri $api -Method Post -TimeoutSec 40 -Headers @{ Authorization = "Bearer $key" } -ContentType 'application/json' -Body $payload
        Write-Host ("OK   {0} -> {1}" -f $t.tag, ($r | ConvertTo-Json -Compress).Substring(0, [Math]::Min(120, ($r | ConvertTo-Json -Compress).Length)))
    } catch {
        $msg = ''
        try { $sr = New-Object IO.StreamReader($_.Exception.Response.GetResponseStream()); $msg = $sr.ReadToEnd() } catch { $msg = $_.Exception.Message }
        Write-Host ("FAIL {0} -> {1}" -f $t.tag, $msg)
    }
    Start-Sleep -Seconds 3
}
