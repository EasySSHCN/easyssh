# EasySSH 发布数据交接契约（开发机 → la 服务器）

> 分工（2026-11-02 起）：**项目开发在开发机**（本服务器不再保留开发仓库/.git/node_modules/target）；
> 本服务器只保留 **官网站点(easyssh-site) + 发布流程所需最小数据**。
> 官网站点与发布流程仍以本服务器为主，更新由开发机推送数据过来。

## 开发机每个版本需提供（"发布四件套"）

| # | 内容 | 服务器落位 | 用途 |
|---|---|---|---|
| 1 | 合并后的 v4 源码树 tar（排除 target/dist-v4/.git/update-keys.json） | 解压覆盖 `/opt/dsh/workspace/easyssh4/`（**勿动**，无 .git 可保） | 官网"源码包"下载位打包（sync.js 按 SOURCE_EXCLUDES 打包） |
| 2 | 主仓 README.md（含最新 `### v<ver>` 版本历史小节） | 覆盖 `/opt/dsh/workspace/EasySSH/README.md` | **官网全部页面数据的权威源**（index/changelog/features/docs 由 sync.js 生成 site-data.json） |
| 3 | 构建产物三件套（官方命名） | `/opt/dsh/workspace/EasySSH/dist/` | 官网下载位 + 在线升级：`EasySSH-<ver>-portable.exe` / `EasySSH-<ver>-win32-x64-portable.zip` / `EasySSH-<ver>-update.exe` |
| 4 | 功能变更说明（供 features.html 功能卡与版本号标注更新） | 服务器侧人工/由 dsh 更新 `site/features.html` | 官方发布强校验项 |

可选：`release/EasySSH-<ver>.exe`（NSIS 安装版，未构建则跳过——sync.js 只要 portable/zip/update 任意其一即可收录该版本）。

## 发布执行（服务器端）

```bash
bash /opt/dsh/workspace/easyssh-site/tools/release-v4.sh <版本号>     # 官网+在线升级+GitHub Release
bash /opt/dsh/workspace/easyssh-site/tools/release-v4.sh <版本号> --no-github   # 仅官网
```

脚本自动完成：版本一致性校验（tauri.conf/README/features.html）→ 产物三件套校验 → `sync.js`（生成 site-data.json + downloads/ + SHA256SUMS + AES 加密在线升级包 .exe.enc）→ `deploy.sh` 上线 → `healthcheck.sh` 自检 → GitHub Release（git push 由开发机负责）。

## 本服务器保留的数据（2026-11-02 清理后）

| 路径 | 大小 | 用途 |
|---|---|---|
| `/opt/dsh/workspace/easyssh-site/` | 163M | 官网站点（页面/downloads/api/发布工具+真实升级主密钥） |
| `/opt/dsh/workspace/EasySSH/` | 116M | 发布数据：README.md（权威源）+ dist/release（最近 3 版产物接收位） |
| `/opt/dsh/workspace/easyssh4/` | 1.5M | v4 源码树（官网源码包打包源；tar 覆盖时不要带入 .git/target/dist-v4） |

已清除：easyssh4/.git(2.3G)、dist-v4、主仓 node_modules、release 旧版本(4.1.x)、/tmp 全部同步临时文件（约 6.5G）。git 完整历史在 GitHub（EasySSHCN/easyssh，main + v4-dev 分支）。
