# EasySSH 贡献指南

感谢你对 EasySSH 的关注！本文档说明如何参与项目贡献。EasySSH 是一款 Windows 绿色版 SSH/SFTP 终端（Electron + xterm.js + ssh2 + iconv-lite，纯开源组件），代码 100% 原创。

## 如何提交 Issue

1. 提交前请先搜索现有 Issue，避免重复。
2. 请包含以下信息：
   - EasySSH 版本号（界面标题栏可见，如 2.3.2）
   - Windows 版本与系统架构（如 Windows 11 x64）
   - 复现步骤与预期/实际行为
   - 相关错误信息（连接失败提示、`%TEMP%\easyssh-debug.log` 调试日志片段等）
3. 安全漏洞**不要**通过 Issue 公开报告，请阅读 [SECURITY.md](SECURITY.md)。

## 如何提交 Pull Request

1. Fork 仓库并创建功能分支（如 `feat/sftp-drag-upload`）。
2. 保持改动聚焦：一个 PR 只解决一个问题。
3. 提交前请本机运行验证：`npm start` 手动测试核心流程（连接、SFTP、设置）。
4. **不要修改**应用代码之外的部署产物（release/、dist/ 目录内容勿直接提交）。
5. 若改动涉及用户可见功能，**必须同步更新 README「版本更新历史记录」**（见下文"版本发布流程"）。

## 开发环境搭建

- Node.js ≥ 18
- Windows x64 环境（产物为 Windows 绿色版）

```bash
git clone <仓库地址>
cd EasySSH
npm install
npm start            # 本机调试运行
```

其他常用命令：

```bash
npm run build:exe    # 构建 release/EasySSH-<version>.exe 单文件绿色版
npm run package:win  # 备用：electron-packager 文件夹版输出到 dist/
```

## 代码规范

- JavaScript 遵循现有代码风格：2 空格缩进、单引号、分号结尾；与 main.js / preload.js / renderer.js 现状保持一致。
- 主进程（main.js）与渲染进程（renderer.js）职责清晰，IPC 通过 preload.js 暴露的受控接口进行。
- 新增第三方依赖必须是开源组件，并在 README/文档中说明用途。
- 面向用户的提示文案使用中文；代码注释使用中文。
- 不引入外部网络请求（零 CDN 依赖原则）。

## 提交信息规范

采用 `类型: 简述` 格式，简述使用中文，一句话说清改动内容。常用类型前缀：

- `feat:` 新功能（如 `feat: SFTP 支持文本转换传输模式`）
- `fix:` 缺陷修复（如 `fix: 修复 PKCS#8 私钥无法解析的问题`）
- `docs:` 文档变更
- `style:` 样式/界面调整（不影响逻辑）
- `refactor:` 重构（不新增功能、不修 bug）
- `chore:` 构建、依赖等杂项

示例：

```
fix: 旧版本生成的 PKCS#8 私钥连接时自动转换
feat: 快捷命令栏支持自定义命令按钮
```

## 版本发布流程（重要）

每次发布新版本必须完整执行以下步骤：

1. **更新版本号**：修改 `package.json` 的 `version` 字段（产物文件名由 electron-builder 按 `${version}` 生成）。
2. **更新 README「版本更新历史记录」**：在 `## 版本更新历史记录` 下新增 `### v<版本号> (<日期>)` 条目，逐条列出新增/修复内容。这是项目长期有效的维护规则，README 是版本历史的**权威源**。
3. **构建产物**：
   - `npm run build:exe` 生成 `release/EasySSH-<version>.exe`（单文件绿色版）；
   - `npm run package:win` 生成 `dist/EasySSH-<version>-win32-x64-portable.zip` 对应的文件夹版（用于 zip 解压版）。
   - 产物文件名**必须带版本号**，命名固定为 `EasySSH-<version>.exe` 与 `EasySSH-<version>-win32-x64-portable.zip`。
4. **提交与打标签**：README 历史、package.json 版本号与产物变更一并提交，建议打 `v<version>` 标签。
5. **网站自动同步**：官网 easyssh.cn 的同步机制（`tools/sync.js`）会自动解析 README 的版本历史条目，扫描 release/ 与 dist/ 中的带版本号产物，校验 sha256 后拷贝到站点 downloads/ 目录，并重新生成 `site-data.json`；站点下载页与更新日志页据此自动展示最新版本。因此**只要 README 历史与产物命名规范，网站无需手工干预即可自动同步**。
6. 生成 CHANGELOG.md 所需内容已在 README 中维护，CHANGELOG 由 README 历史生成（详见 [CHANGELOG.md](CHANGELOG.md) 顶部说明）。
