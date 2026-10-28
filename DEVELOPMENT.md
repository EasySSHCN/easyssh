# EasySSH 开发维护规则（长期有效）

1. **每次版本更新必须同步更新 README.md 的「版本更新历史记录」**：
   - 新版本条目写在最上方，含版本号、日期、新增/修复内容；
   - 忘记更新视为未完成发版。
2. 产物文件名必须带版本号：`release/EasySSH-<version>.exe`、`dist/EasySSH-<version>-win32-x64-portable.zip`
   （版本号唯一来源：package.json 的 version 字段）。
3. 发版前必须通过：`node --check main.js && node --check preload.js && node --check renderer.js`。
4. 构建命令：`npm run build:exe`（单文件）/`npm run package:win`（文件夹版）。
5. 历史教训（勿重蹈）：
   - 不要用 ../node_modules 相对路径引用渲染端资源（asar 内失效）→ 用 assets/；
   - 不要自定义 ssh2 authHandler 返回固定数组（认证死循环卡死主进程）；
   - 连接成功路径不要清理连接对象（会导致无法输入）；
   - 打包必须排除 cpu-features（Linux 原生模块）。
