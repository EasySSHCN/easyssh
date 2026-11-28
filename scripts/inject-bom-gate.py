# -*- coding: utf-8 -*-
"""向 push-release.ps1 注入 BOM-GATE（幂等）"""
import io, os
f = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'scripts', 'push-release.ps1')
t = io.open(f, encoding='utf-8-sig').read()
if 'BOM-GATE' in t:
    print('already injected'); raise SystemExit
anchor = 'rollback-release.sh)\n)'
if anchor not in t:
    anchor = 'rollback-release.sh)\r\n)'
assert anchor in t, 'anchor not found'
inject = anchor + '''

# BOM-GATE 20261126：发布链所有含中文 .ps1 必须带 UTF-8 BOM（PS5.1 无 BOM 按 ANSI 解析会错乱，多次实证）
& (Join-Path $PSScriptRoot 'fix-ps1-bom.ps1') -Root (Split-Path -Parent $PSScriptRoot) -Fix | Out-Null
$left = (& (Join-Path $PSScriptRoot 'fix-ps1-bom.ps1') -Root (Split-Path -Parent $PSScriptRoot) | Select-String '\\u9700\\u4fee\\u590d: (\\d+)').Matches.Groups[1].Value
if ($left -ne '0') { throw ("BOM-GATE failed: {0} .ps1 files missing UTF-8 BOM (run scripts\\fix-ps1-bom.ps1 -Fix)" -f $left) }
Write-Host '    BOM-GATE passed' '''
io.open(f, 'w', encoding='utf-8-sig', newline='').write(t.replace(anchor, inject))
print('injected')
