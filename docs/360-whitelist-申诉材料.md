# EasySSH v4 — 360 白名单申诉材料包

> 申诉入口：360 开放平台 **http://open.soft.360.cn/** → 「误报申诉」→ 软件提交
> （也可用「木马查杀 → 信任区」临时自救，见文末用户指引）

## 申诉文件清单

| 文件 | SHA256 |
|---|---|
| EasySSH.exe（解压后主程序） | `25490772a9c85e5ae0681e0a2d607deeb30bf244eeea7c2fadad1f17a73cb2d4` |
| EasySSH-v4-alpha-win64.zip（分发包） | `3a060e57eee6f288c5d4bbc1246707572f6e83edc29bc11b6d1ff9595f483922` |

下载地址：
- GitHub: https://github.com/EasySSHCN/easyssh/releases/tag/v4.0.0-alpha.1
- Gitee: https://gitee.com/easyssh/EasySSH/releases/tag/v4.0.0-alpha.1
- 官网: https://easyssh.cn/download.html

## 申诉文案（可直接粘贴）

EasySSH 是一款开源（MIT 协议）的 Windows SSH 终端工具，官网 easyssh.cn。
程序使用 Tauri 2 框架（Rust + 系统 WebView2）构建，为未签名的开源免费软件。

被误报的表现：运行/解压 EasySSH.exe 时被 360「文件防护」拦截或直接查杀。

技术说明：
1. 程序为绿色便携软件，运行时不向系统目录写入任何文件，不写注册表，
   不需要管理员权限，所有数据保存在程序目录内；
2. 程序不包含加壳（UPX 等均未使用），资源段携带完整图标与版本信息；
3. SSH 功能基于开源库 russh（Rust 实现），网络行为仅为用户主动发起的
   SSH 连接（TCP 22 等端口）；
4. 唯一外部依赖 WebView2Loader.dll 为微软官方 WebView2 加载器（随包分发）；
5. 项目开源可查：https://github.com/EasySSHCN/easyssh

申请将上述 SHA256 哈希加入 360 白名单/可信数据库，谢谢。

## 用户侧自救指引（官网 FAQ 同步文案）

1. 360 弹窗时点「允许本次操作」；
2. 一劳永逸：360 → 木马查杀 → 信任区 → 添加目录 → 选择 EasySSH 所在文件夹；
3. 被误删文件后请从官网重新下载解压。

## 附注（研发侧）

- **不建议 UPX 加壳**：壳是恶意软件标配特征，360 对加壳程序查杀率显著上升，体积收益（32→15MB 左右）远不抵误报代价。
- **根治方案仍是代码签名**：OV 证书 ~800-2000 元/年即可显著改善；EV ~2000-4000 元/年可秒过 SmartScreen 与国产杀软。证书接入构建链的工作（signtool / osslsigncode 跨平台签名）已具备条件，采购后一天内可接入。
