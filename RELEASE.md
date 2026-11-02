# EasySSH v4 本机发布流程（Windows / MSVC）

> 适用环境：开发主库在本机（Windows + rustup stable + VS Build Tools MSVC 14.44 + Node.js）。
> **分工（2026-11-02 起）**：开发在本机；la 服务器（la.trae.show）只保留官网站点 + 发布流程数据，负责上线。
> 官网数据交接契约见 `docs/RELEASE-HANDOFF.md`（服务器同路径副本 `/opt/dsh/workspace/RELEASE-HANDOFF.md`）。

## 一、发版前检查清单（按顺序）

1. **改版本号**（唯一来源，两处必须一致）：
   - `src-tauri/tauri.conf.json` → `"version": "4.x.y"`
   - `src-tauri/Cargo.toml` → `version = "4.x.y"`
2. **更新 README.md 的「版本更新历史记录」**：新条目写在最上方（版本号/日期/新增修复内容）。官方规则：忘记更新视为未完成发版。
   ⚠️ 此 README 是**官网全部页面数据的权威源**——发版时必须同步一份到服务器（见第五节）。
3. **语法检查**（改过 UI/桥接必做）：
   ```powershell
   node --check ui/renderer.js
   node --check ui/tauri-bridge.js
   node --check ui/assets/i18n.js
   ```
4. **本机功能验收**：`cargo build`（debug）→ 连接真实服务器 → SSH / SFTP（上传下载、右键删除/重命名/新建目录）/ 端口转发 / 主密码 / 设置 各过一遍。
5. **官网静态板块检查**（download.html 等人工维护内容，服务器 `/opt/dsh/workspace/easyssh-site/site/`）：
   - `download.html` 的「v4 全新内核」等功能介绍板块是否与最新版状态一致（板块内**不要写死版本号/文件直链**，动态下载卡会自动更新）
   - `features.html` 功能卡版本号标注（intake 强校验）
   - `docs-src/*.md` 文档如有变更同步更新
6. **⚠️ 关闭 360 实时防护**（木马查杀 → 实时防护，或右键托盘退出并确认 `360rp/ZhuDongFangYu` 已停）——编译产物未签名，实时扫描会锁住/删除刚生成的 exe/DLL 导致 LNK1104。

## 二、一键构建（本机）

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-v4-local.ps1
```

脚本自动完成：`cargo build --release` → 收集单文件产物 → 打 zip → 输出 SHA256。

产物（`dist-v4\` 下，官网同步用**官方命名副本**会一并生成）：
| 产物 | 用途 |
|---|---|
| `EasySSH-<版本>-portable.exe` | **单文件版（主发布物）**；运行时在 exe 同目录自动生成 `keys/`、`config.json`、`config.key` |
| `EasySSH-<版本>-win32-x64-portable.zip` | **绿色 zip（官网命名，主发布物之一）**：内含单个 EasySSH.exe（解压即用；WebView2Loader 已静态链接，不携带 dll） |
| `EasySSH-<版本>-update.exe` | **官网命名副本**（在线升级载荷：裸 app exe，升级器免解压直换） |
| （可选）`EasySSH-<版本>-setup.exe` | NSIS 安装版（未纳入本机流程） |

## 三、官网内容更新（发版必做，页面在 la 服务器）

官网 8 个页面的数据来源与更新方式：

| 页面 | 数据来源 | 发版时要做什么 |
|---|---|---|
| `index.html` | 动态读 `site/data/site-data.json`（由主仓 README 生成） | 无需手改（README 同步后自动更新） |
| `changelog.html` | 同上（版本历史全动态） | 无需手改 |
| `download.html` | 动态（下载位/SHA256 来自 sync.js）+ 一处静态 v4 预览版卡片（低频） | 无需手改（预览版卡片仅在发预览版时更新） |
| `features.html` | **静态功能卡 + 版本号标注** | ✅ 必须人工更新：新增/变更功能写入对应分区卡片（连接与会话/终端体验/文件传输/网络与转发/密钥与安全/升级与维护），并含 `v<版本>` 标注——**官方发布强校验，缺了直接拦截** |
| `docs.html` | `docs-src/*.md`（develop/faq/user-guide） | 文档有变更时更新对应 md |
| `faq.html` | 同上 | 同上 |
| `contributing.html` / `license.html` / `security.html` / `code-of-conduct.html` | 静态 | 极少变更，发版检查一遍即可 |
| 在线升级 | `sync.js` 用真实主密钥生成 `EasySSH-<ver>-update.exe.enc` + 清单 | 自动（依赖站点 `tools/update-keys.json`，已在服务器） |

## 三、源码压缩包与官网静态板块

**源码压缩包（v4.3.8 起改由本机构建）**：`build-v4-local.ps1` 用 `git archive` 从本地仓库打包（仅已跟踪文件，自动排除 target/dist-v4/tools 密钥/update-keys.json），生成 `dist-v4\EasySSH-<版本>-source.zip`，随交付上传；服务器 `intake-release.sh` 优先落位使用，`sync.js` 见已存在源码包即直接复用（不再从服务器旧树重新打包）。
- 服务器路径：`/opt/dsh/workspace/easyssh-site/site/downloads/EasySSH-<版本>-source.zip`
- 公开下载：`https://easyssh.cn/downloads/EasySSH-<版本>-source.zip`（仅最新版生成，随版本滚动）

**官网静态板块**（人工内容，发布后检查）：`download.html` 的功能介绍板块已改为无版本号硬编码的正式版说明（2026-11 修订），以后不再随版本过时；其他页面数据均由 site-data.json 动态渲染。

## 四、发布到 la 服务器 + 官网上线（对接流程）

**对接契约**：服务器 `easyssh-site/HANDOFF.md`（本地存档 `docs/HANDOFF-server.md`）；接收工具 `tools/intake-release.sh`（存档 `docs/intake-release-server.sh`）。
**双向状态**：开发机写 `/opt/dsh/workspace/incoming/DEV-STATUS.md`；发布机 intake 结果在 `easyssh-site/logs/`。

**一行完成（推荐）**：
```powershell
powershell -ExecutionPolicy Bypass -File scripts\push-release.ps1 -Version <版本号> [-FeaturesHtml <更新后的功能页>]
```

脚本自动：组装契约交付目录（NSIS 安装器/update.exe/portable.exe/官方命名 zip/README.md/features.html）→ 上传 `/opt/dsh/workspace/incoming/v<版本>/` → 远程执行 `intake-release.sh`（完整性强校验 → 产物落位 → features 检查 → sync.js 生成 site-data+SHA256SUMS+加密升级包 → 加密自检 → healthcheck → latestVersion 校验）→ 回写 `DEV-STATUS.md` 与本地 `release-log.md` → **仓库同步**（本机 commit 打 git bundle 上传 → 服务器 `tools/repo-sync.sh` 推送 GitHub main+v4-dev+tag、Gitee main+v4-dev+tag）。
加 `-StageOnly` 只上传暂存不发布（预检模式）。

**git 推送执行方式**：本机只做 commit 并打 git bundle 上传；实际推送由服务器 `tools/repo-sync.sh` 执行（凭据都在服务器：GitHub=`/opt/dsh/home/secrets/.gittoken`，Gitee=`.giteetoken`，Gitee 账号 easyssh）。

**分步手动**（等价）：
1. 上传交付目录到 `/opt/dsh/workspace/incoming/v<版本>/`
2. 服务器执行 `bash /opt/dsh/workspace/easyssh-site/tools/intake-release.sh /opt/dsh/workspace/incoming/v<版本>`
3. webroot 由 root 侧定时任务约 5 分钟自动同步；GitHub git push 由本机负责

**features.html 更新责任（开发机）**：新版本功能写入对应分区卡片（连接与会话/终端体验/文件传输/网络与转发/密钥与安全/升级与维护）并带版本号标注；本地维护 `features.html` 副本，经 `-FeaturesHtml` 交付；不交付则要求服务器现有页已含新版本号标注。

## 五、发布后

1. **服务器接收区自动清理**：push-release 在 intake 成功后自动执行 `tools/prune-v4-intake.sh`——`EasySSH/dist`、`release` 与 `incoming` 均只保留最新 3 个版本（`incoming/DEV-STATUS.md` 状态日志永远保留）。历史版本文件只保留在：本机 `dist-v4\` 归档 + GitHub/Gitee 已发布的 Release。
2. **对接发布执行记录日志（181 天保留）**：每次发布自动在两侧写入执行记录——本机 `release-log.md`（权威源）与服务器 `incoming/DEV-STATUS.md`（镜像，发布机 dsh 可读）；`prune-v4-intake.sh` 与 push-release 均自动清理超过 **181 天（6 个月）** 的旧记录，非记录行（表头等）保留。
3. 浏览器验证 https://easyssh.cn 首页版本号、download 下载位、changelog 条目。
4. **发布完再开启 360**。误报申诉材料：`docs/360-whitelist-申诉材料.md`；用户指引 `docs/360-用户处理指南.md`。

## 六、已知注意事项（勿重蹈）

- 版本号唯一来源是 `tauri.conf.json`/`Cargo.toml`，状态栏经 `get_version` 实时读取。
- `ui/` 前端资源**编译时嵌入 exe**，改完前端必须重新 `cargo build`。
- WebView2 缓存已由自定义 `tauri` 协议 no-cache + 启动层缓存标记根治；升级后 UI 不生效先确认走的是新 exe。
- 本地 debug：`cargo build` + 运行 `C:\temp\e4t3\debug\easyssh.exe`（CARGO_TARGET_DIR=C:\temp\e4t3）。
- 服务器侧交接契约：`docs/RELEASE-HANDOFF.md`（服务器副本 `/opt/dsh/workspace/RELEASE-HANDOFF.md`）。
