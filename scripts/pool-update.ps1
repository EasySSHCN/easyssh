# pool-update.ps1 - 按条目 id 更新机器池 detail（追加防坑备注），带校验与写后读回
# 用法: .\pool-update.ps1 -Id R-20261119-005 -DetailAppend "备注文本" [-Note "statusHistory note"]
# PS5.1 兼容：仅用 cmdlet，不用 .NET 静态方法；Set-Content -Encoding UTF8 在 PS5.1 下写带 BOM UTF-8
param(
    [Parameter(Mandatory=$true)][string]$Id,
    [Parameter(Mandatory=$true)][string]$DetailAppend,
    [string]$Note = ''
)
$ScriptRoot = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
$PoolFile = Join-Path (Split-Path -Parent $ScriptRoot) 'reqpool\requirements-pool.json'
$LOG  = Join-Path (Split-Path -Parent $ScriptRoot) 'reqpool\pool-add.log'

$pool = Get-Content $PoolFile -Raw -Encoding UTF8 | ConvertFrom-Json
$item = $pool.items | Where-Object { $_.id -eq $Id } | Select-Object -First 1
if (-not $item) { Write-Output "ERR 未找到条目 $Id"; exit 1 }
if (@($item.PSObject.Properties.Name) -notcontains 'detail') { Write-Output "ERR 条目无 detail 字段"; exit 1 }

$item.detail = $item.detail + "`n【防坑备注 $((Get-Date).ToString('yyyy-MM-dd'))】$DetailAppend"
if ($Note -and @($item.PSObject.Properties.Name) -contains 'statusHistory') {
    $newHist = @($item.statusHistory) + @([pscustomobject]@{
        status = $item.status
        at     = (Get-Date).ToString('yyyy-MM-ddTHH:mm:sszzz')
        note   = $Note
    })
    $item.statusHistory = $newHist
}

$json = ConvertTo-Json $pool -Depth 12
ConvertTo-Json $pool -Depth 12 | Set-Content $PoolFile -Encoding UTF8

# 写后读回复核
$check = Get-Content $PoolFile -Raw -Encoding UTF8 | ConvertFrom-Json
$checkItem = $check.items | Where-Object { $_.id -eq $Id } | Select-Object -First 1
$probe = $DetailAppend.Substring(0, [Math]::Min(40, $DetailAppend.Length))
if ($checkItem -and $checkItem.detail.Contains($probe)) {
    Add-Content -Path $LOG -Value "$(Get-Date -Format s) [pool-update] $Id detail 追加成功 ($($DetailAppend.Length) 字)" -Encoding UTF8
    Write-Output "OK $Id detail 已追加并复核通过"
} else {
    Write-Output "ERR 读回复核失败，请检查 $PoolFile"
    exit 1
}


