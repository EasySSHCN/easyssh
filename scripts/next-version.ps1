# next-version.ps1 —— 版本递进规则(v5 起生效;2026-11-16 补记三级规则标准表述)
# 【版本号三级递进规则(v.1.2.3 示例,全文)】
#   1) 一级版本号(major,最高级):满足 10 次二级版本号更新才进行升级(minor 满 10 → major+1,minor/patch 归零)
#   2) 二级版本号(minor,中级):满足 10 次三级版本号更新才进行升级(patch 满 10 → minor+1,patch 归零)
#   3) 三级版本号(patch):每升级 10 次就推动升级一次二级版本号
# 规则(用户 2026-11-04 指定,同上规则):
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
