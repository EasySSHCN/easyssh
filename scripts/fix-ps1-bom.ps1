# fix-ps1-bom.ps1 —— 全仓 .ps1 BOM 守护（一次性扫描修复 + 可重复巡检）
# 背景（多次踩坑）：Windows PowerShell 5.1 读无 BOM 文件按 ANSI 解码，含中文注释/字符串的
# 脚本会解析错乱（build-v4-local、make-release-assets 两次实证）。规则：含非 ASCII 字节的
# .ps1 必须带 UTF-8 BOM；纯 ASCII 文件无需 BOM（可选补）。
# 用法: powershell -ExecutionPolicy Bypass -File scripts\fix-ps1-bom.ps1 [-Root <目录>] [-Fix]
param(
    [string]$Root = '',
    [switch]$Fix
)
$ErrorActionPreference = 'Stop'
if (-not $Root) {
    $Root = if ($PSScriptRoot) { Split-Path -Parent $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
}
$fixed = @(); $ok = @(); $clean = @()
Get-ChildItem $Root -Recurse -Filter '*.ps1' -File -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch '\\(target|node_modules|dist|src-\d|\.git|src-0312-inspect)\\' } |
    ForEach-Object {
        $b = [IO.File]::ReadAllBytes($_.FullName)
        $hasBom = ($b.Length -ge 3 -and $b[0] -eq 239 -and $b[1] -eq 187 -and $b[2] -eq 191)
        $nonAscii = $false
        foreach ($x in $b) { if ($x -gt 127) { $nonAscii = $true; break } }
        if ($hasBom) { $ok += $_.FullName; return }
        if (-not $nonAscii) { $clean += $_.FullName; return }
        # 含非 ASCII 且无 BOM：需要修复
        if ($Fix) {
            $t = [Text.Encoding]::UTF8.GetString($b)
            [IO.File]::WriteAllBytes($_.FullName, ([byte[]](239,187,191)) + [Text.Encoding]::UTF8.GetBytes($t))
            $fixed += $_.FullName
        } else { $fixed += $_.FullName }
    }
Write-Output ("扫描根: {0}" -f $Root)
Write-Output ("带 BOM 正常: {0}  纯 ASCII 无需 BOM: {1}  需修复: {2}" -f $ok.Count, $clean.Count, $fixed.Count)
if ($fixed.Count) { $fixed | ForEach-Object { Write-Output ("  {0}{1}: {2}" -f $(if($Fix){'[已补BOM] '}else{'[缺BOM] '}), $_, '') ; } }
if (-not $Fix) { Write-Output '（dry-run；加 -Fix 执行修复）' }
