# v5 M2 前端交付说明：WebGL 渲染 + 断线重连会话恢复

## 一、WebGL 终端渲染（F-M2-04，P0）

### vendor 产物
| 文件 | 来源 | 版本 | 许可 |
|---|---|---|---|
| `ui/vendor/addon-webgl.js` | npmmirror tgz `@xterm/addon-webgl/-/addon-webgl-0.18.0.tgz` 内 `package/lib/addon-webgl.js`（UMD dist 原样拷贝，未改动代码，仅文件头部补注释说明来源） | 0.18.0 | MIT（同 tgz 内 `package/LICENSE`，已另存 `ui/vendor/addon-webgl.LICENSE`） |

- 运行时零 CDN、零 npm：index.html 以 UMD `<script src="vendor/addon-webgl.js">` 本地加载，全局名 `WebglAddon`（用法 `new WebglAddon.WebglAddon()`，与现有 `FitAddon.FitAddon` 同模式）。

### xterm 版本兼容性说明
- `ui/assets/xterm.js` 为 v4 内置的 xterm UMD 包。经核对其内置 `DEFAULT_OPTIONS`（含 `rescaleOverlappingGlyphs`、`attachCustomWheelEventHandler`、`cursorInactiveStyle`），为 **xterm 5.5.x** 代际产物。
- addon-webgl 0.18.0 正是配合 @xterm/xterm 5.5 的稳定版（0.19.0/0.20.0 对应更新的 @xterm/xterm 主线，不选用）。两者内部共享同一套 WebGL 渲染管线 ABI（WebglRenderer 直接调用 `terminal._core` 的服务定位器），0.18.0 与 5.5.x 匹配。
- 兼容性验证方法：打开会话后 DevTools 控制台执行 `document.querySelector('.xterm-screen canvas')`，WebGL 生效时首个 canvas 的 context 为 `webgl2`；无 `console.warn('[EasySSH] ...回退')` 输出即加载成功。

### 降级路径（terminal.js `initWebglRenderer`）
三层防护，全部回退 DOM 渲染器（WebglAddon.dispose() 会触发 xterm 内部恢复默认渲染器并重绘）：
1. `window.WebglAddon` 缺失（vendor 文件未加载/被策略拦截）→ console.warn 后直接用 DOM。
2. `new WebglAddon.WebglAddon()` 抛错（无 WebGL2 上下文：远程桌面、老显卡驱动）→ try/catch 回退。
3. 运行期 `webgl.onContextLoss`（GPU 驱动崩溃/上下文丢失）→ dispose 回退。

### 性能验收对比（建议操作）
- 开 WebGL（默认）：执行 `dmesg` 或 `cat /var/log/syslog`（几 MB 输出），滚动流畅无卡顿掉帧；字型经 GPU 纹理图集渲染，放大窗口拖动无撕裂。
- 关 WebGL（模拟降级）：在 DevTools 中临时把 `vendor/addon-webgl.js` 从 HTML 中移除（或重命名文件）后重启，同样大输出场景 DOM 渲染器明显帧率下降、长输出时 UI 有可感知迟滞。
- 回退触发验证：DevTools → Rendering 勾选/或系统层禁用 GPU（如 `--disable-gpu` 启动 WebView2 调试）→ 启动应用，控制台应出现 `[EasySSH] WebGL 初始化失败,已回退 DOM 渲染器`，终端功能完全可用。

## 二、断线重连会话恢复（P1）

### 实现位置（ui/js/terminal.js）
- 断线瞬间（`onClosed`）：`exportBufferText()` 用 xterm buffer API 导出 scrollback 纯文本存入 `tab._restoreBuf`，并记录 `tab._discAt` 时间戳。
- 重连成功（手动回车 `reconnectTab` 或自动重连 `scheduleAutoReconnect` 内部）：
  - `restoreSessionBuffer()` 先插入分隔行 `── 会话已恢复(断线 X 分 Y 秒) ──`，随后写回断线前的缓冲区文本；**同一标签页复用，不重建**（沿用 doConnect 换绑连接 id 的既有机制）。
  - 状态栏倒计时：`startRetryCountdown()` 每 500ms 刷新「已断开 — 将在 X 秒后第 N 次重试」，接入既有 2/4/8s 指数退避；重连成功/失败/手动关闭均清理定时器。
- 手动「在终端按回车重连」路径保持不变（onData 中 `/\r|\n/` 判断原样保留）。

### 恢复边界（恢复了什么 / 没恢复什么）
| 已恢复 | 未恢复（本次边界） |
|---|---|
| scrollback 纯文本（含换行，末尾空行截除） | ANSI 颜色/样式（buffer API 只能取纯文本） |
| 断线时长标注分隔行 | alternate screen 全屏画面（vim/htop 等，无法可靠导出） |
| 同一标签页/连接选项/端口转发配置 | 光标位置、滚动位置 |
| 自动重连 3 次指数退避 + 状态栏倒计时 | **cwd / 环境变量级恢复：依赖 shell 集成（OSC 133）上报提示符/目录/env，标记 P2 延后实现**（见 terminal.js restoreSessionBuffer 注释） |

## 三、终端观感增强（P2）
- design-tokens.css 已有终端专用 token 通道（`--color-bg-terminal / --color-terminal-fg / --color-terminal-cursor / --color-terminal-selection`），但终端主题实际由 settings.js 的皮肤表（dracula/pureblack 等 term.theme）驱动，**settings.js 不在本代理文件归属内**——接入 token 需与设置模块负责人协调（皮肤表读 CSS 变量或反向注入），本次未改动，避免覆盖对方主题逻辑。
- 光标样式/闪烁：settings.js 目前无对应设置项，同样需协调后由设置侧加 UI。

## 四、遗留问题
1. 【需协调】终端主题 token 化与「渲染器(自动/WebGL/DOM)」设置项（手册 F-M2-04 交互要点）需 settings.js 侧加 UI；当前 M2 先固定「自动」策略（WebGL 优先，失败自动回退）。
2. xterm 主库仍为 v4 内置的 5.5.x UMD（`ui/assets/xterm.js`，非本代理归属）；后续如升级 @xterm/xterm 命名空间版本，addon-webgl 需同步换 0.19+。
3. 会话恢复为文本级；OSC 133 shell 集成（cwd/env/提示符定位）延后至 P2。
