/* ============================================================
 * renderer.js — 兼容垫片（v5 M1 模块化拆分后保留）
 *
 * 原约 2100 行单体已按职责拆分为 ui/js/ 下的 ES Modules，
 * 入口为 js/main.js（index.html 以 <script type="module"> 加载）：
 *
 *   js/main.js      入口：初始化顺序 + 各模块装配
 *   js/state.js     全局状态（tabs/sessions/appSettings/ver...）
 *   js/dom.js       通用 DOM 工具（$/escH/fmtSize/fmtSpeed）
 *   js/terminal.js  终端渲染与交互 + 连接/重连/密钥
 *   js/sessions.js  会话列表（分组/搜索/备注/颜色/导入导出）
 *   js/masterpw.js  主密码锁定态 + 解锁/管理模态 + 会话写盘守卫
 *   js/sftp.js      SFTP 浏览/多选/属性/传输队列/进度
 *   js/shortcuts.js 快捷命令栏与管理
 *   js/forward.js   本地端口转发
 *   js/settings.js  设置/皮肤/初始化
 *   js/updater.js   在线升级 UI 流程（含公钥指纹展示/回滚提示）
 *   js/feedback.js  使用反馈
 *
 * 本文件不再包含任何逻辑；保留仅为兼容可能的旧引用路径。
 * ============================================================ */
