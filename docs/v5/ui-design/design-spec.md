# EasySSH v5.0 UI 设计规范（Design Spec）

> 版本：v5.0-draft-1 ｜ 适用：Windows 桌面（Tauri 2 + WebView2）｜ 配套文件：`design-tokens.css`、`prototype.html`

---

## 1. 设计原则

1. **内容优先（Content First）**：终端输出与文件列表是主角，UI 骨架（侧栏/工具条/状态栏）必须安静，使用中性色与细边框，不与终端内容争夺注意力。
2. **克制装饰（Restraint）**：无渐变滥用、无大阴影堆叠、无装饰性插画；动效只用于传达状态变化（视图切换、加载、确认），不做纯装饰动画。
3. **一致性与可预测性（Consistency）**：所有组件基于同一套 Design Tokens；同类操作（打开/取消/删除）在任何页面行为与外观一致，用户永远能预测点击结果。
4. **键盘优先（Keyboard First）**：面向专业用户，一切高频操作有快捷键；`Ctrl+Shift+P` 命令面板是键盘用户的总入口；Tab 顺序遵循视觉顺序。
5. **无障碍（Accessible by Default）**：文本对比度 ≥ 4.5:1，交互元素 ≥ 3:1；全部图标带 `aria-label` 或可见文字；焦点环永远清晰可见。
6. **绿色轻量（Lean by Design）**：零外部依赖（无 CDN、无 Web 字体），图标为内联 SVG；动画全部使用 `transform/opacity` 合成层属性。

---

## 2. 色彩系统

### 2.1 品牌主色：青绿色（Emerald / Terminal Green）

选择理由：延续产品"绿色轻量"定位，绿色在深色终端背景上辨识度高、与成功语义天然一致，且区别于 SecureCRT（灰）、Xshell（蓝）的主流用色。

| 色阶 | HEX | 用途 |
|---|---|---|
| brand-50 | `#ECFDF5` | 浅色主题强调背景 |
| brand-100 | `#D1FAE5` | 选中行背景（浅色） |
| brand-200 | `#A7F3D0` | 悬停高亮（浅色） |
| brand-300 | `#6EE7B7` | 禁用主按钮 |
| brand-400 | `#34D399` | 图形装饰/状态点（浅色主题） |
| brand-500 | `#10B981` | 品牌基准色、Logo、状态点 |
| **brand-600** | `#059669` | **主按钮、链接、激活标签** |
| brand-700 | `#047857` | 主按钮悬停 |
| brand-800 | `#065F46` | 深色强调文本 |
| brand-900 | `#064E3B` | 浅色主题选中侧栏项底色 |

### 2.2 语义色

| 语义 | 基准 | 深色主题值 | 浅色主题值 |
|---|---|---|---|
| 成功 success | `#10B981` | `#34D399` | `#059669` |
| 警告 warning | `#F59E0B` | `#FBBF24` | `#B45309` |
| 危险 danger | `#EF4444` | `#F87171` | `#DC2626` |
| 信息 info | `#3B82F6` | `#60A5FA` | `#2563EB` |

### 2.3 中性色阶（Slate 系）

| 色阶 | HEX | 用途 |
|---|---|---|
| neutral-0 | `#FFFFFF` | 纯白（浅色卡片） |
| neutral-50 | `#F8FAFC` | 浅色页面底 |
| neutral-100 | `#F1F5F9` | 浅色侧栏底 |
| neutral-200 | `#E2E8F0` | 浅色边框 |
| neutral-300 | `#CBD5E1` | 浅色分隔线/禁用边框 |
| neutral-400 | `#94A3B8` | 占位符文字 |
| neutral-500 | `#64748B` | 次要文字（浅色） |
| neutral-600 | `#475569` | 正文（浅色） |
| neutral-700 | `#334155` | 悬停底（深色） |
| neutral-800 | `#1E293B` | 深色面板底 |
| neutral-900 | `#0F172A` | 深色侧栏底 |
| neutral-950 | `#0B1120` | 深色终端底 |

### 2.4 深浅主题对照表（核心）

| Token | 深色（默认） | 浅色 |
|---|---|---|
| `--color-bg-primary` | `#0B1120` | `#F8FAFC` |
| `--color-bg-secondary`（侧栏/面板） | `#0F172A` | `#F1F5F9` |
| `--color-bg-elevated`（卡片/浮层） | `#1E293B` | `#FFFFFF` |
| `--color-bg-terminal` | `#0B1120` | `#0F172A`（浅色下终端仍保持深色，见 §3） |
| `--color-text-primary` | `#E2E8F0` | `#0F172A` |
| `--color-text-secondary` | `#94A3B8` | `#475569` |
| `--color-text-muted` | `#64748B` | `#94A3B8` |
| `--color-border-subtle` | `#1E293B` | `#E2E8F0` |
| `--color-border-strong` | `#334155` | `#CBD5E1` |
| `--color-accent` | `#10B981` | `#059669` |

---

## 3. 字体系统

| 用途 | 字体栈（全部系统字体，零下载） |
|---|---|
| UI 字体 | `-apple-system, "Segoe UI Variable", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, sans-serif` |
| 等宽（终端/代码） | `"Cascadia Code", "Cascadia Mono", Consolas, "JetBrains Mono", "Sarasa Mono SC", "Noto Sans Mono CJK SC", monospace` |

- **字体连字（Ligatures）**：终端字体优先使用 `Cascadia Code`（Windows 10+ 内置），自带 `=>`、`!=`、`>=` 等编程连字；CSS 层通过 `font-variant-ligatures: contextual` 开启、`none` 关闭，设置页提供"终端连字"开关。检测不到 Cascadia 时回退 Consolas（无连字，功能不受影响）。
- **终端区主题联动**：终端背景/前景取自 `--color-bg-terminal` / `--color-ansi-*`，切换 `data-theme` 时同步更新 xterm.js 的 `theme` 选项；浅色主题下终端仍为深色底（终端用户习惯深色读输出），通过独立的 terminal token 组实现。

### 字号 / 行高 / 字重阶梯

| Token | 值 | 用途 |
|---|---|---|
| `--font-size-xs` | 11px / 1.45 | 徽章、状态栏 |
| `--font-size-sm` | 12px / 1.5 | 次要文字、表格副列 |
| `--font-size-base` | 13px / 1.6 | 正文（桌面工具标准） |
| `--font-size-md` | 14px / 1.6 | 输入框、列表项 |
| `--font-size-lg` | 16px / 1.5 | 卡片标题 |
| `--font-size-xl` | 18px / 1.4 | 页面标题 |
| 终端字号 | 13px / 1.35，`--font-mono` | 可在设置中 10–24px 调节 |
| 字重 | 400 正文 / 500 强调 / 600 标题 / 700 页面标题 | 不使用 300 以下细体 |

---

## 4. 间距系统

4px 基准网格，全部间距为 4 的倍数：

`--space-1: 4px ｜ --space-2: 8px ｜ --space-3: 12px ｜ --space-4: 16px ｜ --space-5: 20px ｜ --space-6: 24px ｜ --space-8: 32px ｜ --space-10: 40px`

常用组合：组件内边距 `--space-3`；卡片内边距 `--space-4`；区块间距 `--space-6`；侧栏项内边距 `--space-2 --space-3`。

---

## 5. 圆角 / 阴影 / 边框

| Token | 值 | 用途 |
|---|---|---|
| `--radius-sm` | 4px | 徽章、状态点、内联标签 |
| `--radius-md` | 6px | 按钮、输入框、列表项 |
| `--radius-lg` | 10px | 卡片、模态框、Toast |
| `--radius-full` | 999px | 胶囊标签、头像点 |

阴影（深色主题阴影更重、浅色更柔）：

| Token | 深色 | 浅色 |
|---|---|---|
| `--shadow-sm` | `0 1px 2px rgb(0 0 0 / .4)` | `0 1px 2px rgb(15 23 42 / .08)` |
| `--shadow-md` | `0 4px 12px rgb(0 0 0 / .5)` | `0 4px 12px rgb(15 23 42 / .12)` |
| `--shadow-lg` | `0 12px 32px rgb(0 0 0 / .6)` | `0 12px 32px rgb(15 23 42 / .16)` |

边框：统一 1px 实线；深色主题用低对比色 `--color-border-subtle`，避免栅格感过强；仅输入框聚焦与拖拽目标使用 2px。

---

## 6. 组件清单与规范

| 组件 | 规范要点 |
|---|---|
| **按钮-主** | brand-600 底、白字、`--radius-md`、高 32px、内边距 `0 16px`；悬停 brand-700；禁用 40% 透明 |
| **按钮-次** | 透明底 + 1px `--color-border-strong` 边框；悬停 `--color-bg-hover` |
| **按钮-危险** | danger 色系；二次确认后执行；文本按钮变体用 danger 前景 |
| **按钮-图标** | 32×32 方形，仅图标 + `aria-label` + `title`；悬停出现 `--color-bg-hover` 底 |
| **输入框** | 高 32px、`--radius-md`、1px 边框；聚焦时边框变 accent + 2px 外发光环 `box-shadow: 0 0 0 3px --color-focus-ring` |
| **下拉选择** | 原生 `<select>` 样式化即可；右侧 12px chevron 图标 |
| **标签页** | 顶部标签栏：激活标签 `--color-bg-elevated` 底 + 顶部 2px accent 指示条 + 关闭按钮悬停出现；宽度自适应，最小 120px，省略号截断 |
| **侧栏会话项** | 高 36px；左侧 8px 状态点（绿=在线/灰=离线/黄=连接中/红=失败）；名称 + 主机次行 `--font-size-xs`；右键菜单支持 |
| **终端工具条** | 会话名 + 连接时长左对齐；右侧：搜索、清屏、分屏、下载、断开（危险色图标）；高 40px |
| **表格（SFTP 文件列表）** | 表头吸顶 `--color-bg-secondary`；行高 32px；悬停行高亮；选中行 brand-100/800（按主题）；斑马纹不使用（保持克制） |
| **模态框** | 居中、`--radius-lg`、`--shadow-lg`、遮罩 `rgb(0 0 0/.5)`；Esc 关闭；焦点困在框内（focus trap）；标题 + 内容 + 右对齐按钮排 |
| **Toast** | 右下角堆叠，`--radius-lg`，左侧 3px 语义色条，5s 自动消失，可关闭；`role="status"` |
| **徽章** | `--radius-sm`、`--font-size-xs`、语义色 10% 底 + 语义色文字；用于协议/标签/权限 |
| **空状态** | 居中：48px 线性图标（muted 色）+ 一句说明 + 一个次级按钮；不放插画 |
| **加载态** | 骨架屏：`--color-bg-hover` 底的圆角块，1.2s 呼吸动效；列表加载用 3 行骨架，不用 spinner 遮罩 |
| **命令面板** | 屏幕上方 15% 处居中浮层，宽 560px；输入即过滤；↑↓ 选择、Enter 执行、Esc 关闭；结果按"会话/命令/设置"分组 |

---

## 7. 图标规范

- **网格**：24×24 viewBox，实际绘制内容 20×20 居中；导出使用 `currentColor` 描边。
- **线宽**：1.5px，圆头（`stroke-linecap="round"`）圆角连接（`stroke-linejoin="round"`）；禁用实心填充风格。
- **最小命中区**：32×32（含内边距），视觉可小于命中区。
- **命名清单（核心 32 个）**：

| 名称 | 用途 | 名称 | 用途 |
|---|---|---|---|
| `terminal` | 终端标签/新建终端 | `folder` | 文件夹 |
| `sftp` | SFTP 视图 | `folder-open` | 展开目录 |
| `settings` | 设置入口 | `file` | 普通文件 |
| `search` | 搜索框 | `upload` | 上传 |
| `plus` | 新建连接/标签 | `download` | 下载 |
| `x` | 关闭标签/弹窗 | `refresh` | 刷新列表 |
| `plug` | 连接 | `trash` | 删除 |
| `plug-off` | 断开连接 | `info` | 属性/详情 |
| `chevron-right` | 分组展开 | `pause` | 暂停传输 |
| `chevron-down` | 分组收起 | `play` | 恢复传输 |
| `chevron-up` | 下拉箭头 | `copy` | 复制 |
| `check` | 成功/确认 | `edit` | 编辑 |
| `alert` | 警告 | `keyboard` | 快捷键设置 |
| `sun` | 浅色主题 | `palette` | 外观设置 |
| `moon` | 深色主题 | `arrow-up-circle` | 升级 |
| `command` | 命令面板 | `circle-help` | 关于/帮助 |
| `clock` | 连接时长 | `split` | 分屏终端 |

---

## 8. 动效规范

| Token | 值 | 用途 |
|---|---|---|
| `--duration-fast` | 120ms | 悬停、按下、焦点环 |
| `--duration-base` | 200ms | 下拉展开、Toast 进出、骨架屏 |
| `--duration-slow` | 320ms | 视图切换（View Transitions）、模态框 |
| `--ease-standard` | `cubic-bezier(0.2, 0, 0, 1)` | 默认缓动（快出缓停） |
| `--ease-emphasized` | `cubic-bezier(0.3, 0, 0.1, 1)` | View Transition |
| 骨架屏呼吸 | `opacity .5→1→.5, 1.2s infinite` | 加载占位 |

规则：
- **View Transitions**：主视图切换（终端 ⇄ SFTP ⇄ 设置）使用 `document.startViewTransition()`，旧视图不存在时直接切换；全部动画仅 `transform/opacity`。
- **微交互**：按钮悬停只变底色不做位移；按下 `scale(0.98)`；仅此一处使用 transform 缩放。
- **尊重系统设置**：`@media (prefers-reduced-motion: reduce)` 下全部动画时长归零。
- 终端内滚动、SFTP 列表滚动均使用原生滚动，不加自定义动画。

---

## 9. 无障碍要求

1. **对比度**：正文文字 ≥ 4.5:1，大号文字与图形/边框 ≥ 3:1；深浅两套主题逐 token 校验（已在校色时用 WCAG 公式验证）。
2. **焦点样式**：`:focus-visible` 统一 `box-shadow: 0 0 0 2px --color-bg-primary, 0 0 0 4px --color-focus-ring` 双环，永不用 `outline: none` 裸删。
3. **键盘导航**：
   - 侧栏会话列表：↑↓ 移动、Enter 连接、F2 重命名、Delete 删除（带确认）；
   - 标签栏：`Ctrl+Tab` / `Ctrl+Shift+Tab` 切换、`Ctrl+W` 关闭；
   - 命令面板：`Ctrl+Shift+P` 呼出、↑↓ 选择、Enter 执行、Esc 关闭；
   - 全站 Tab 顺序 = 视觉顺序；模态框内 focus trap。
4. **ARIA**：
   - 侧栏 `role="navigation" aria-label="会话列表"`；会话项 `role="treeitem" aria-selected aria-expanded`；
   - 状态点配 `aria-label="已连接"` 等文本说明，不只靠颜色；
   - 图标按钮必须 `aria-label`；Toast `role="status" aria-live="polite"`；命令面板 `role="dialog" aria-modal`，结果列表 `role="listbox"` + `aria-activedescendant`；
   - 主题切换按钮 `aria-pressed` 表达当前状态。
5. **缩放**：支持 Windows 显示缩放 100–200%，布局在 1280×720 下无横向滚动；侧栏可折叠至 48px 图标模式。
