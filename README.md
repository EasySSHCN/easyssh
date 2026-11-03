# EasySSH v5 — Rust + Tauri 2 的 Windows 绿色版 SSH/SFTP 终端

> ## 🌐 官方网站：**https://EasySSH.CN**
> 安装包下载、在线升级、使用文档的唯一官方渠道，请认准 **https://EasySSH.CN**

[![GitHub release](https://img.shields.io/badge/version-5.0.0--alpha.1--dev-blue)](https://github.com/EasySSHCN) [![官网](https://img.shields.io/badge/官网-easyssh.cn-10B981)](https://easyssh.cn) [![License](https://img.shields.io/badge/license-MIT-green)](LICENSE)

EasySSH v5 是 [EasySSH v4](https://github.com/EasySSHCN/easyssh)（Rust + Tauri 2 + russh 内核重写版）的下一代主线仓库，围绕**安全、健壮性、执行效率、功能增强、简化流程、美化 UI** 六大方向升级。

## 版本更新历史记录

### v5.0.0-alpha.2 (2026-11-03)
- **M2 效率健壮**:SFTP 传输管道化(多 worker 并行)+断点续传(journal 跨重启)+SOCKS5/HTTP 代理+ProxyJump 跳板(≤3 级)+WebGL 终端渲染(三层降级)+断线重连会话恢复+panic 崩溃日志+升级独立日志+发版冒烟门禁+russh 集成测试地基
- **M3 UI 设计系统**:品牌主色 Emerald、style.css 全面 token 化(硬编码色 40+→0)、深浅双主题切换、内置 SVG 图标系统(34 个替换全部 emoji 图标位)、设置页卡片式重设计、无障碍(焦点环/aria/reduced-motion)
- **M4 功能增强**:终端分屏(2/4 屏 CSS Grid,设置全窗格生效)、全局命令面板(Ctrl+Shift+P)、脚本库宏(变量占位符+顺序执行)、传输队列暂停/恢复/优先级、端口转发规则持久化+一键批量应用

## v5.0 六大方向

| 方向 | 重点 |
|---|---|
| 🔐 安全 | 升级包 Ed25519 非对称签名、Argon2id 默认派生、known_hosts 管理、FIDO2 硬件密钥 |
| 🛡️ 健壮性 | 升级自回滚、断线会话状态恢复、SFTP 断点续传、panic 崩溃日志、集成测试地基 |
| ⚡ 效率 | WebGL 终端渲染、SFTP 传输管道化、大目录分页、冷启动 < 1 秒 |
| ✨ 功能 | 终端分屏、传输队列、同步浏览、脚本库宏、全局命令面板 |
| 🧹 流程 | 版本号自动派生、发版冒烟门禁、webroot 原子部署、changelog 自动化 |
| 🎨 UI | Design Tokens 设计系统、深浅双主题、内置 SVG 图标集、设置页卡片式重设计 |

## 文档

- [产品开发手册](docs/v5/产品开发手册.md)（M1-M4 功能规格与验收标准）
- [UI 设计规范](docs/v5/ui-design/design-spec.md) / [Design Tokens](docs/v5/ui-design/design-tokens.css) / [交互原型](docs/v5/ui-design/prototype.html)（浏览器直接打开）
- [发布流程](RELEASE.md) / [升级方案](UPGRADE-PLAN.md)

## 构建（Windows 10/11 x64）

前置：Rust stable (MSVC)、VS Build Tools、Node.js。

```powershell
# 安装依赖（前端为原生 HTML/JS，无 npm 依赖时跳过）
cargo build
# 完整发布构建（NSIS 安装器/便携版/绿色 zip/源码包）
powershell -ExecutionPolicy Bypass -File scripts\build-v4-local.ps1
```

`tools/`（NSIS、MinGit）与 `update-keys.json` 为本机构建私有数据，不入库。

## 与 v4 的关系

- v5 基线继承自 v4.3.10 代码库，配置/会话与 v4 完全兼容
- v4 维护仓库：https://github.com/EasySSHCN/easyssh
- v5 的 OTA 升级通道将启用 Ed25519 签名校验（M1），与 v4 通道格式向后兼容一个版本

## License

MIT © EasySSH Contributors
