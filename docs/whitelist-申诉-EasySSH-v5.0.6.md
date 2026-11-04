# EasySSH v5.0.6 安全软件白名单申诉材料包

> 联系人邮箱:**ad@6xm.net**(各厂商申诉统一联系方式)
> 申诉日期:2026-11-04
> 适用版本:EasySSH v5.0.6(2026-11-04 发布,最新正式版)

## 一、软件介绍

EasySSH 是一款开源(MIT 协议)的 Windows SSH/SFTP 终端工具,官网 https://easyssh.cn ,源码托管于 GitHub/Gitee(GitHub: https://github.com/EasySSHCN/easyssh ,Gitee: https://gitee.com/easyssh/EasySSH )。

- 技术栈:Tauri 2(Rust 原生后端 + 系统 WebView2 前端),未加壳(未使用 UPX 等),资源段携带完整图标与版本信息;
- 免费开源,无广告、无捆绑、无静默安装,绿色便携:不写系统目录、不写注册表、不需要管理员权限,所有数据保存在程序目录内;
- SSH 功能基于开源库 russh(Rust 实现),网络行为仅为用户主动发起的 SSH 连接(TCP 22 等端口)与官网在线升级/反馈接口(HTTPS);
- v5.0 内置升级通道采用 Ed25519 签名强制验签 + AES-256-GCM 加密载荷,升级异常自动回滚;
- 因暂未购买代码签名证书,绿色包被部分安全软件启发式误报,特此申请白名单。

## 二、公开提供的 5 个文件(官网下载位,全部公开可下载)

官网下载页:https://easyssh.cn/download.html
SHA256SUMS:https://easyssh.cn/downloads/SHA256SUMS

| # | 文件 | 用途 | SHA256 | 下载地址 |
|---|------|------|--------|----------|
| 1 | EasySSH-5.0.6.exe | NSIS 标准安装引导器(带安装界面,解压释放 EasySSH.exe 到同目录,供普通用户安装使用) | `ba292acc4166d1d034837f3b37f5e61bb956438c8ed70c0846f93fd255bc781b` | https://easyssh.cn/downloads/EasySSH-5.0.6.exe |
| 2 | EasySSH-5.0.6-portable.exe | 绿色便携单文件(即主程序 EasySSH.exe 本体,解压即用;最常被用户直接运行的文件,误报主诉对象) | `dbed2ec0d6fce3995d30050a35ee73c6742342b4b943ede45197068c4622da14` | https://easyssh.cn/downloads/EasySSH-5.0.6-portable.exe |
| 3 | EasySSH-5.0.6-win32-x64-portable.zip | 官网主发布物绿色 zip 包(内含单个 EasySSH.exe,解压即用;GitHub/Gitee Release 同步分发) | `a5b75a12878c9b88f3eadd6e17041739773bac7b36d04aba202be8c13a7c06e1` | https://easyssh.cn/downloads/EasySSH-5.0.6-win32-x64-portable.zip |
| 4 | EasySSH-5.0.6-update.exe | 在线升级通道载荷(裸应用 exe,与 portable 同字节;软件内「检查更新」经加密通道下载并原地替换) | `dbed2ec0d6fce3995d30050a35ee73c6742342b4b943ede45197068c4622da14` | https://easyssh.cn/downloads/EasySSH-5.0.6-update.exe |
| 5 | EasySSH-5.0.6-source.zip | 完整源代码包(MIT 开源,供审计;无可执行内容) | `592f90c5156a7dea9f6cf19977add46cd2a974eb73407804e9eec568556bd852` | https://easyssh.cn/downloads/EasySSH-5.0.6-source.zip |

注:#2 与 #4 为同一 exe 的两种分发命名(官网下载位 / 升级载荷),SHA256 相同。
镜像分发:GitHub Release https://github.com/EasySSHCN/easyssh/releases/tag/v5.0.6 、Gitee Release https://gitee.com/easyssh/EasySSH/releases/tag/v5.0.6 (附件同哈希)。

## 三、误报表现(通用申诉文案,可直接粘贴)

EasySSH(开源 SSH 终端,v5.0.6)被贵司产品误报/拦截,请将上表 5 个文件的 SHA256 加入白名单/可信库。程序为 Tauri 2(Rust + WebView2)未签名开源免费软件:绿色便携、不写系统目录与注册表、不要求管理员权限、未加壳、无捆绑无静默安装;网络行为仅为用户主动发起的 SSH 连接与 HTTPS 升级/反馈接口。源码与官网均可公开审计,联系方式 ad@6xm.net 。

(English) EasySSH (open-source SSH terminal, v5.0.6) is falsely flagged by your product. Please whitelist the SHA256 hashes of the 5 files listed above. Tauri 2 (Rust + WebView2) unsigned freeware: portable, writes nothing outside its own folder, no registry, no admin rights, not packed, no bundleware; network activity is limited to user-initiated SSH connections and HTTPS update/feedback endpoints. Source and website are publicly auditable. Contact: ad@6xm.net

## 四、各厂商提交渠道清单

| 厂商 | 渠道 | 说明 |
|------|------|------|
| 360 | http://open.soft.360.cn/ 「误报申诉」 | 需开发者账号登录提交 |
| 腾讯电脑管家/TP | https://mcm.qq.com/ 误报申诉 | 需 QQ 登录 |
| 火绒 | 官网「误报反馈」 https://www.huorong.cn/ 或论坛发帖;邮箱 svc@huorong.cn | 邮件可直接发送,附材料包链接 |
| 金山毒霸 | http://www.ijinshan.com/ 客服/误报反馈 | 需登录 |
| 微软 Defender | https://www.microsoft.com/en-us/wdsi/filesubmission | 需 Microsoft 账号 |
| 卡巴斯基 | https://opentip.kaspersky.com/ (False Positive) | 免登录表单,含验证码 |
| Avast/AVG | https://www.avast.com/false-positive-file-submission.php | 免登录表单,含验证码 |
| ESET | https://www.eset.com/int/support/false-positive/ | 免登录表单,含验证码 |
| Bitdefender | https://www.bitdefender.com/submit/ | 免登录表单 |
| Norton/LifeLock | https://submit.norton.com/ | 需账号 |
| McAfee | https://www.mcafee.com/enterprise/en-us/forms/false-positive.html | 表单 |
| Dr.Web | https://vms.drweb.com/send_virus/ | 免登录 |
| Sophos | https://support.sophos.com/ (sample submission) | 需账号 |

## 五、提交记录(2026-11-04 自动提交,截图见 appeal-screens/)

### 国产厂商组

| 厂商 | 状态 | 细节 / 后续 |
|------|------|-------------|
| 360 | ❌ 需登录 | open.soft.360.cn 申诉入口在「提交软件检测」,右上角需登录/注册;客服邮箱 **opensoft@360.cn** 可作人工申诉备用渠道 |
| 腾讯电脑管家 | ❌ 入口不可达 | mcm.qq.com 自动导航失败且需 QQ 登录,需人工浏览器访问 |
| 火绒 | ❌ 需登录 | 误报反馈走论坛 bbs.huorong.cn/forum-44「病毒查杀问题反馈→样本误报」,发帖需论坛账号 |
| 金山毒霸 | ❌ 无网页表单 | 官网无误报入口;需客户端内反馈或微信公众号在线客服 |
| 瑞星 | ❌ 入口失效 | 样本上传系统 up.rising.com.cn 超时;仅剩论坛(需登录) |

### 国际厂商组

| 厂商 | 状态 | 细节 / 后续 |
|------|------|-------------|
| Kaspersky | ❌ 需登录 | opentip.kaspersky.com 提交入口 "Requests" 需 Kaspersky 账号登录 |
| Avast/AVG | ❌ 入口下线 | false-positive-file-submission.php 已 404,需另寻现渠道 |
| ESET | ❌ 入口下线 | /support/false-positive/ 已 404;可用邮件渠道 **vlab@eset.com**(样本提交) |
| Bitdefender | ❌ 入口下线 | /submit/ 重定向到帮助文档;需账号或 GravityZone 企业渠道 |
| Dr.Web | ⏸ 到验证码止 | 正确入口 https://vms.drweb.com/sendvirus/?lng=en ;表单(link/email/描述)可预填,图片验证码需人工。自动化已填 link+ad@6xm.net+英文文案(含 5 个 SHA256),未提交 |

> 自动提交总结论(2026-11-04):10 家厂商 0 家可全自动提交成功——全部卡在登录墙、入口下线或图片验证码,未绕过任何验证机制。
> 人工完成优先级:① Dr.Web sendvirus 表单(仅差验证码);② ESET 邮件 vlab@eset.com;③ 360 opensoft@360.cn 邮件或注册开发者账号;④ 火绒论坛 bbs.huorong.cn/forum-44;⑤ 腾讯 QQ 登录 mcm.qq.com;⑥ 金山/瑞星走客户端或微信公众号渠道。统一材料见本文档第二、三节。
