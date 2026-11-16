# EasySSH v5.1.5 误报申诉记录（2026-11-16）

## 背景
- 2026-11-16 00:39–00:44 Windows Defender 将 `EasySSH-5.1.5-portable.exe` 误报为 `Trojan:Win32/Bearfoos.A!ml`（严重）并隔离，SmartScreen 同步拦截。
- 本地文件 SHA-256 与 GitHub Release v5.1.5 官方 digest 完全一致（`e1a4f53f…6613096`），确认为 `!ml` 机器学习启发式误报。
- 误报记录：1 条「已阻止威胁」+ 2 条「已删除或已还原威胁」，均指向 portable exe。

## 申诉文件与 SHA256（三件官方发布物）
| 文件 | SHA256 |
|------|--------|
| EasySSH-5.1.5-portable.exe | `e1a4f53fc2f1ffb7541e67fb7a5271cff7d87dfd3f622bf88b1be38fb6613096` |
| EasySSH-5.1.5.exe | `2b8e4fd160c2cab13848ed48ca2a45f4d17dc987d7df25238bb1880d00f250ce` |
| EasySSH-5.1.5-win32-x64-portable.zip | `64e30944fa807afddd99820c9b2b6c6a30e2717ee42ac6dcae9baf93decab228` |

## 提交记录
- 邮件申诉（`scripts/send-appeals-v515.ps1`，mail-api 通道，发件 ad@6xm.net，2026-11-16 01:02）：
  | 厂商 | 收件 | 状态 |
  |------|------|------|
  | 火绒 | svc@huorong.cn | ✅ 已发送 |
  | 360 | opensoft@360.cn | ✅ 已发送 |
  | ESET | vlab@eset.com | ✅ 已发送 |
  | TrendMicro | virus_submission@trendmicro.com | ✅ 已发送 |
  | Avast | virus@avast.com | ✅ 已发送 |
  | Kaspersky | falsepositive@kaspersky.com | ✅ 已发送 |
  | Bitdefender | falsepositive@bitdefender.com | ✅ 已发送 |
  | McAfee | support@mcafee.com | ✅ 已发送 |

## 微软渠道（未走邮件，官方明确拒绝）
- 微软官方 FAQ：不接受邮件样本提交，唯一通道是 https://www.microsoft.com/wdsi/filesubmission （需登录，以 software developer 身份提交并选误报争议）。
- 提交时填上述三个 SHA256 + 下载链接（GitHub Release 与官网），检测名 `Trojan:Win32/Bearfoos.A!ml`，联系人 ad@6xm.net。
- 提交后在 https://www.microsoft.com/wdsi/submissionhistory 跟踪判定；判定为误报后随定义更新自动放行。
- 用户侧临时措施：保护历史记录中还原文件 + 对 `P:\Program Files (x86)\EasySSH\` 目录加排除（申诉通过后移除）。

## 后续建议
- 长期方案：购买 Authenticode 代码签名证书（v5.0.6 申诉记录中已列为根治手段），或加入微软 MAPS 计划。
- 关注其他厂商回复（ad@6xm.net），如厂商要求补交样本再另行跟进。
