# EasySSH 发布对接说明（开发机 ↔ 发布机）

> 本机 = **发布机**：只保留官网（easyssh-site）与发布流程；构建、开发、git 推送均在**开发机**完成。
> 本文档是两侧 dsh 的对接契约，变更须双侧同步。

## 角色分工

| 事项 | 开发机（dsh） | 发布机（本机） |
|---|---|---|
| 源码开发 / 构建（MSVC+NSIS）/ git 推送 GitHub、Gitee | ✅ 负责 | ❌ 不再承担（本机工具链已清理） |
| 功能总览页 features.html 更新 | ✅ 负责（随交付提供） | 落地部署 + 校验 |
| README 更新历史小节（权威源） | ✅ 负责（随交付提供） | 校验 + 落位 |
| 官网同步 / 加密升级接口 / SHA256SUMS | ❌ | ✅ `tools/intake-release.sh` 一键完成 |
| webroot 部署 | ❌ | 自动（root 侧定时任务 ~5 分钟） |

## 交付方式（开发机 → 发布机）

放到本机固定接收目录（文件同步/任意方式均可，dsh 有写入权限）：

```
/opt/dsh/workspace/incoming/v<版本号>/
├── EasySSH-<ver>.exe                        # NSIS 单文件引导安装器（build-v4-setup.sh 产物）
├── EasySSH-<ver>-update.exe                 # 裸 MSVC exe（在线升级加密载荷）
├── EasySSH-<ver>-portable.exe               # 裸 MSVC exe（下载页便携版）
├── EasySSH-<ver>-win32-x64-portable.zip     # 便携 zip（内含 EasySSH.exe）
├── README.md                                # EasySSH 仓库完整 README（已含 ### v<ver> 小节）
└── features.html                            # 可选：本版功能有变化时提供新版功能总览页
```

命名必须严格一致（intake 脚本按文件名识别版本号与用途）。

### ⚠ NSIS 引导安装器（对接对齐点，待开发机确认）

开发机当前 `RELEASE.md`/`build-v4-local.ps1` 只产出 `EasySSH.exe + WebView2Loader.dll +
EasySSH-<ver>-win64.zip`，**不含**契约要求的 `EasySSH-<ver>.exe`（NSIS 单文件引导器）。
补齐方式任选其一：
- 开发机 Windows 上装 NSIS 3，用 `nsis/bootstrap-v4.nsi` 打包（defines 见 easyssh4 仓库
  `scripts/build-v4-setup.sh` 中的 makensis 调用）；
- 或双方约定改契约（例如下载页改主推 zip），intake 脚本同步放宽校验。
在确认前，intake 会因缺少该文件而拒绝发布（这是有意的强校验）。

## 发布机执行（一行命令）

```bash
bash /opt/dsh/workspace/easyssh-site/tools/intake-release.sh /opt/dsh/workspace/incoming/v<版本号>
```

脚本自动完成：交付完整性校验（缺文件/README 缺更新小节即拒绝）→ 产物落位
（EasySSH/release + dist + 官网 downloads）→ README 换新 → features.html 检查
（交付了就用交付版；没交付则校验现页已含新版本号，否则拒绝）→ `sync.js`
（生成加密升级包 .enc、api/update/check、site-data、SHA256SUMS）→ 加密协议自测 →
healthcheck → 校验 `latestVersion`。

全部通过即发布完成；webroot 页面由 root 侧定时任务约 5 分钟内自动生效。

## 发布机保留的数据（勿删）

- `easyssh-site/` 整个目录（站点、tools、`tools/update-keys.json` 升级主密钥、logs）
- `EasySSH/` 仓库：README.md、release/、dist/（最近 3 个版本的产物）——sync.js 的数据源
- `easyssh4/` 源码树（开发机向本机交付源码的通道；**不含** target/、dist-v4 构建产物）
- `/opt/dsh/home/secrets/.gittoken`（GitHub 令牌，Release API 校验用）
- `.cache/electron-builder-nsis-manual/` 的 makensis（打包兜底工具，~8MB）

## 已知约定

- 发布口径为 **MSVC 构建**（`cargo xwin`），mingw 产物不作为发布物
- 官网下载列表固定展示最近 **3 个**版本（sync.js KEEP_LATEST_VERSIONS），旧产物自动清理
- `/api/update/check` 为动态服务，实时读 site-data.json 封包，无需写 webroot 静态文件
- features.html 分区：连接与会话 / 终端体验 / 文件传输 / 网络与转发 / 密钥与安全 / 升级与维护；
  新功能写入对应分区卡片并带版本号标注
- 参考清单：`easyssh4/docs/v4-发布清单.md`（其中"发布机侧"步骤已由 intake 脚本固化）
