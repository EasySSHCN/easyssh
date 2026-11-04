# notify-email.ps1 —— 发布通知邮件(发布完成后调用,收件人 ad@6xm.net)
# 发送通道(按优先级):
#   1) HTTP 邮件 API:设置环境变量 EASYSSH_MAIL_API_URL(POST JSON: {to,subject,text})
#      与 EASYSSH_MAIL_API_KEY(Bearer Token),适配 SendGrid/Brevo/Mailgun 等中转网关
#   2) SMTP:设置 EASYSSH_SMTP_HOST / EASYSSH_SMTP_PORT / EASYSSH_SMTP_USER / EASYSSH_SMTP_PASS
#   3) 均未配置:写入待发邮箱 outbox,待配置后由 -FlushOutbox 补发(绝不静默丢失)
param(
    [Parameter(Mandatory=$true)][string]$Subject,
    [Parameter(Mandatory=$true)][string]$Text,
    [string]$To = 'ad@6xm.net',
    [switch]$FlushOutbox
)
$ErrorActionPreference = 'Stop'
$Outbox = 'P:\Program Files (x86)\EasySSH\reqpool\outbox'
New-Item -ItemType Directory -Force $Outbox | Out-Null

function Save-Outbox($s, $t) {
    $f = Join-Path $Outbox ("pending-{0:yyyyMMdd-HHmmss}.json" -f (Get-Date))
    @{ to = $To; subject = $s; text = $t; createdAt = (Get-Date).ToString('s') } |
        ConvertTo-Json | Set-Content $f -Encoding UTF8
    Write-Host "⚠ 邮件通道未配置,已写入待发: $f"
}

function Send-Api($s, $t) {
    $url = $env:EASYSSH_MAIL_API_URL; $key = $env:EASYSSH_MAIL_API_KEY
    # 默认通道:DPAPI 加密凭据(reqpool\mail-api.cred,机密禁止外泄)→ https://gz.trae.show/mail-api/v1/send
    $credFile = 'P:\Program Files (x86)\EasySSH\reqpool\mail-api.cred'
    if (-not $url -and (Test-Path $credFile)) {
        try {
            $c = Get-Content $credFile -Raw -Encoding UTF8 | ConvertFrom-Json
            $url = $c.api
            $key = [Runtime.InteropServices.Marshal]::PtrToStringAuto([Runtime.InteropServices.Marshal]::SecureStringToBSTR((ConvertTo-SecureString $c.key)))
        } catch { }
    }
    if (-not $url -or -not $key) { return $false }
    if ($url -notmatch '/v1/send$') { $url = $url.TrimEnd('/') + '/v1/send' }
    try {
        $body = @{ to = @($To); subject = $s; text = $t } | ConvertTo-Json
        Invoke-RestMethod -Uri $url -Method Post -ContentType 'application/json' `
            -Headers @{ Authorization = "Bearer $key" } -Body $body -TimeoutSec 30 | Out-Null
        Write-Host "✅ 通知邮件已发送(API): $s"
        return $true
    } catch { Write-Host "✗ 邮件 API 发送失败: $($_.Exception.Message)"; return $false }
}

function Send-Smtp($s, $t) {
    if (-not $env:EASYSSH_SMTP_HOST) { return $false }
    try {
        $sec = ConvertTo-SecureString $env:EASYSSH_SMTP_PASS -AsPlainText -Force
        $cred = New-Object System.Management.Automation.PSCredential($env:EASYSSH_SMTP_USER, $sec)
        $smtpPort = 587
        if ($env:EASYSSH_SMTP_PORT) { $smtpPort = [int]$env:EASYSSH_SMTP_PORT }
        Send-MailMessage -SmtpServer $env:EASYSSH_SMTP_HOST -Port $smtpPort `
            -UseSsl -Credential $cred -From $env:EASYSSH_SMTP_USER -To $To `
            -Subject $s -Body $t -Encoding UTF8
        Write-Host "✅ 通知邮件已发送(SMTP): $s"
        return $true
    } catch { Write-Host "✗ SMTP 发送失败: $($_.Exception.Message)"; return $false }
}

if ($FlushOutbox) {
    Get-ChildItem $Outbox -Filter 'pending-*.json' | Sort-Object Name | ForEach-Object {
        $m = Get-Content $_.FullName -Raw -Encoding UTF8 | ConvertFrom-Json
        if ((Send-Api $m.subject $m.text) -or (Send-Smtp $m.subject $m.text)) { Remove-Item $_.FullName -Force }
    }
    exit 0
}

if ((-not (Send-Api $Subject $Text)) -and (-not (Send-Smtp $Subject $Text))) {
    Save-Outbox $Subject $Text; exit 2   # 待发(outbox),不算硬失败
}
