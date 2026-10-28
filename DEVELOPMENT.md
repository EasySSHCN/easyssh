# EasySSH 开发维护规则（长期有效）

1. **每次版本更新必须同步更新 README.md 的「版本更新历史记录」**：
   - 新版本条目写在最上方，含版本号、日期、新增/修复内容；
   - 忘记更新视为未完成发版。
2. 产物文件名必须带版本号：`release/EasySSH-<version>.exe`、`dist/EasySSH-<version>-win32-x64-portable.zip`
   （版本号唯一来源：package.json 的 version 字段）。
3. 发版前必须通过：`node --check main.js && node --check preload.js && node --check renderer.js`。
4. 构建命令：`npm run build:exe`（单文件）/`npm run package:win`（文件夹版）。
5. **GitHub 同步发布（每次新版必须执行，可用 `bash scripts/release.sh` 一键完成本条 + 第 2、3、4 条）**：
   - git commit 本次变更 → 打 tag `v<version>` → push 到 origin（main 分支 + tag）；
   - 用 GitHub Releases API 创建同名 Release（`v<version>`），上传 `release/EasySSH-<version>.exe` 与 `dist/EasySSH-<version>-win32-x64-portable.zip` 两个产物；
   - Release 说明直接复用 README「版本更新历史记录」里该版本的条目；
   - 认证：优先 `GITHUB_TOKEN` 环境变量；也可用 `gh` CLI（若已安装）。仓库：`EasySSHCN/easyssh`。
6. 历史教训（勿重蹈）：
   - 不要用 ../node_modules 相对路径引用渲染端资源（asar 内失效）→ 用 assets/；
   - 不要自定义 ssh2 authHandler 返回固定数组（认证死循环卡死主进程）；
   - 连接成功路径不要清理连接对象（会导致无法输入）；
   - 打包必须排除 cpu-features（Linux 原生模块）。
