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
├── features.html                            # 可选：本版功能有变化时提供新版功能总览页
└── easyssh-gateway-<ver>/                   # 可选：网关部署套件（R-20261120-009，本版网关有更新时提供）
    ├── easyssh-gateway                      # 服务端二进制（Linux 交叉编译产物，无后缀；流程验证可为 .exe）
    ├── config.toml.template                 # 配置模板（现网 config.toml 不被覆盖）
    ├── deploy.sh                            # 运维手工更新辅助脚本
    ├── keys.example.php                     # 审计密钥模板（真件 keys.php 只放服务器，永不交付）
    └── SHA256SUMS                           # 逐件 SHA256 清单（intake 强校验）
    ├── install-webconsole.sh                # Web 服务台安装器（页面/接口/数据目录/审核 cron，R-20261121-002）
    ├── gateway-apply-audit.php              # 每天 10:00/16:00 人工或 AI 定时审核任务（双结果邮件）
    ├── README.md                            # 自建部署总说明（网关 + Web 服务台一站式）
    └── webconsole/                          # 服务台站点文件（api/gateway/*.php、gw-admin.html、apply.html）
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
- `easyssh4/` 源码树（开发机向本机交付源码的通道；**不含** target/、dist 构建产物）
- `/opt/dsh/home/secrets/.gittoken`（GitHub 令牌，Release API 校验用）
- `.cache/electron-builder-nsis-manual/` 的 makensis（打包兜底工具，~8MB）

## 已知约定

- 发布口径为 **MSVC 构建**（`cargo xwin`），mingw 产物不作为发布物
- 官网下载列表固定展示最近 **3 个**版本（sync.js KEEP_LATEST_VERSIONS），旧产物自动清理
- `/api/update/check` 为动态服务，实时读 site-data.json 封包，无需写 webroot 静态文件
- features.html 分区：连接与会话 / 终端体验 / 文件传输 / 网络与转发 / 密钥与安全 / 升级与维护；
  新功能写入对应分区卡片并带版本号标注
- 参考清单：`easyssh4/docs/v4-发布清单.md`（其中"发布机侧"步骤已由 intake 脚本固化）

## 网关部署/更新/回滚契约（R-20261120-009，2026-11-21 起）

网关 `easyssh-gateway` 部署在本机（127.0.0.1:8443 回环，nginx 仅代理 `=/api/auth/login`、`=/api/resources`、`^~ /tunnel/` 三条精确路由）。目录布局：

```
/opt/dsh/workspace/easyssh-gateway/
├── current/                 # 现网运行目录（config.toml / 密钥 / easyssh-gateway 二进制）——intake 永不触碰
├── releases/v<ver>/         # intake 落位的各版本套件（二进制+模板，累积保留）
├── backup/                  # intake 落位前自动快照的现行二进制，保留最近 5 份
└── version                  # 现行版本标记（version=<ver>，运维更新时维护）
```

**更新口径（重要）**：`intake-release.sh` 对网关套件**只做「落位 + SHA256 校验 + 快照」**——
- SHA256 校验失败即拒收（`!! 网关部署套件 SHA256 校验失败`）；
- 新二进制写入 `releases/v<ver>/`，`current/`、`config.toml`、密钥、nginx 路由一律不动；
- **不自动重启网关服务（systemctl restart easyssh-gateway）**——避免误伤线上 SSH 隧道会话。

**更新生效（运维手工执行）**：
```bash
cd /opt/dsh/workspace/easyssh-gateway
cp -f releases/v<新ver>/easyssh-gateway current/easyssh-gateway   # 或切换软链
echo "version=<新ver>" > version
systemctl restart easyssh-gateway          # 会在隧道会话低峰执行；重启前确认无活跃隧道
curl -s https://easyssh.cn/api/resources | head -c 200      # 健康验证
```

**回滚（网关二进制单件，独立于官网整版回滚）**：
```bash
bash /opt/dsh/workspace/easyssh-site/tools/rollback-release.sh gateway --list    # 列 backup/
bash /opt/dsh/workspace/easyssh-site/tools/rollback-release.sh gateway <旧ver> --check   # 预检
bash /opt/dsh/workspace/easyssh-site/tools/rollback-release.sh gateway <旧ver>           # 替换 current 二进制，不重启
systemctl restart easyssh-gateway         # 运维手工执行生效
```

官网整版回滚（`rollback-release.sh <旧版本号>`）不涉及网关；网关与官网版本回滚相互独立、按需分别执行。
