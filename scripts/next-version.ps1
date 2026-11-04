# next-version.ps1 —— 版本递进规则(v5 起生效)
# 规则(用户 2026-11-04 指定):
#   1) 常规发布:patch + 1(如 5.0.6 → 5.0.7)
#   2) patch 累积到 10(如 v5.0.10)后,下一次发布升大版本 minor+1、patch 归零(5.0.10 → 5.1.0)
#   3) minor 累积到 10(如 v5.10.0)后,下一次发布升分支版本 major+1、patch 归零(5.10.0 → 6.0.0)
# 用法: powershell -File scripts\next-version.ps1 -Current 5.0.6   →  输出 5.0.7
param([Parameter(Mandatory=$true)][string]$Current)
$v = [version]$Current
$maj = $v.Major
$min = $v.Minor
$pat = if ($v.Build -lt 0) { 0 } else { $v.Build }
# minor 已累积到 10 → 本次发布直接升分支版本(major+1,minor/patch 归零)
if ($min -ge 10) { "$($maj + 1).0.0"; exit }
$pat++
if ($pat -gt 10) { $pat = 0; $min++ }
"$maj.$min.$pat"
