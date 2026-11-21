/* ============================================================
 * main.js — 渲染端入口（由 renderer.js 拆分而来）
 * 职责：模块装配与初始化顺序（与原 renderer.js 顶部到底部的
 *       执行顺序一致）；全局错误兜底；弹窗通用关闭行为；
 *       版本号预取；WebView2 旧缓存迁移；启动画面淡出。
 * 依赖：tauri-bridge.js / win-controls.js / i18n.js 以经典脚本
 *       方式在 index.html 中先于本模块加载（提供 window.easyssh
 *       等全局 API），故保留全局加载、不改为模块引入——
 *       理由见仓库 v5(M1) 提交说明。
 * ============================================================ */
import { $ } from './dom.js';
// v5(beta) 修复:sessions 必须显式导入(此前去重循环引用了未导入的绑定 → init 中断 → 会话列表空白)
import { ver, setVer, sessions, setSessions, appSettings } from './state.js';
import { initMasterPw } from './masterpw.js';
import { renderSessions } from './sessions.js';
import { initSettings } from './settings.js';
import { initUpdate } from './updater.js';
import { checkCrashLogs } from './crashlog.js'; // v5(beta) 任务五:崩溃日志查看入口
// 副作用引入:各功能模块在求值期注册事件监听(与原单体脚本自上而下注册等价)
import './terminal.js';
import './sftp.js';
import './forward.js';
import './feedback.js';
import './macros.js'; // v5.2.x 宏序列(命令序列自动化)
import { initManualTooltips } from './tooltip.js'; // v5.2.x/F-R10-21 功能按钮悬停说明+手册外链
import { initGatewayForm } from './gateway.js'; // R-20261120-005 ③ 网关折叠区表单逻辑

/* 版本号从主进程实时读取(app.getVersion),避免改版时状态栏还显示旧版本号
 * v4:版本号异步预取完成后回填状态栏(getVersionSync 同步语义在 Tauri 下不可用)
 * v5(M1) 修复:预取与 init() 状态栏回填存在竞态(模块化后 init 链更快,回填时 ver
 * 仍为空,且后续正则条件匹配不上残缺文本导致版本号永久丢失)——统一走 setReadyBar
 * 幂等回填:仅在状态栏仍处于"就绪"态时重写 */
function setReadyBar() {
  const el = document.getElementById('statusBar');
  if (el && el.textContent.includes('就绪') && !el.textContent.includes('已连接') && !el.textContent.includes('断开')) {
    el.textContent = 'EasySSH ' + ver + ' — 就绪,点击「新建连接」开始';
  }
}
(async () => {
  try {
    const v = await window.easyssh.getVersion();
    if (v) { setVer('v' + v); setReadyBar(); }
  } catch {}
})();

// 全局错误兜底:任何未捕获错误都显示在界面 + 落盘 ui-error.log(绝不无声失败)
// v5(beta) 增强:补捕获 unhandledrejection(Promise 异常此前不经过 window.error,静默丢失)
window.__uiErrMarks = [];
async function __logUiError(kind, text) {
  try {
    window.__uiErrMarks.push(kind + '=' + Math.round(performance.now()) + ' ' + text);
    const dir = await window.easyssh.cfgDirPath();
    const sep = dir.includes('\\') ? '\\' : '/';
    await window.easyssh.saveTextFile(dir + sep + 'ui-error.log',
      JSON.stringify({ wall: Date.now(), errs: window.__uiErrMarks }, null, 1));
  } catch {}
}
window.addEventListener('error', (e) => {
  const el = $('#connMsg');
  if (el) el.textContent = '脚本错误: ' + (e.message || e.type);
  __logUiError('error', (e.message || e.type) + ' @' + (e.filename || '') + ':' + (e.lineno || 0));
});
window.addEventListener('unhandledrejection', (e) => {
  const st = e.reason && e.reason.stack ? String(e.reason.stack).split('\n').slice(0, 4).join(' | ') : '';
  __logUiError('promise', String(e.reason && (e.reason.message || e.reason)).slice(0, 200) + (st ? ' @stack: ' + st : ''));
});

/* ---------- 弹窗关闭 ---------- */
document.querySelectorAll('.close-x').forEach((b) => b.addEventListener('click', () => {
  $('#' + b.dataset.close).classList.add('hidden');
}));
document.querySelectorAll('.modal').forEach((m) => m.addEventListener('click', (e) => {
  // v3.0.0:.mp-block 模态(主密码解锁/指纹确认)必须显式操作,点遮罩不关闭
  if (e.target === m && !m.classList.contains('mp-block')) m.classList.add('hidden');
}));

async function init() {
  // v5(beta) 临时性能埋点(定位启动耗时,排查后移除)——每步增量落盘
  const marks = [];
  const dirSep = (() => { try { return null; } catch { return null; } })();
  let cfgDirForMark = '';
  const mark = async (n) => {
    marks.push(n + '=' + Math.round(performance.now()));
    try { await window.easyssh.saveTextFile(cfgDirForMark + 'ui-timing.json', JSON.stringify({ wall: Date.now(), marks })); } catch {}
  };
  mark('init-start');
  try { cfgDirForMark = (await window.easyssh.cfgDirPath()).replace(/[\\/]+$/, '') + '/'; await mark('cfgDir'); } catch {}
  // v4.2.3:WebView2 旧缓存清理(版本变更触发;自建分组/下载目录先备份后恢复)
  try {
    // v5(beta):uiCacheStatus 清缓存+重载路径已移除——WebView2 缓存清理完全由 Rust
    // 启动层在窗口创建前完成(标记 mismatch → 旧数据目录改名后台删除)。该 JS 路径
    // 在 360 下曾引发"清缓存+整页重载"循环卡死/启动 10 秒+ 问题,且属死代码(恒 false)。
    const need = false;
    await mark('uiCacheStatus-skipped');
    if (false) {
      const dump = JSON.stringify({
        groups: localStorage.getItem('easyssh.groups'),
        dlDir: localStorage.getItem('easyssh.dlDir'),
      });
      const dir = await window.easyssh.cfgDirPath();
      const sep = dir.includes('\\') ? '\\' : '/';
      await window.easyssh.saveTextFile(dir + sep + 'ui-backup.json', dump);
      await window.easyssh.uiCacheClear();
      return; // 清理后页面将自动重载
    }
    const dir = await window.easyssh.cfgDirPath();
    const sep = dir.includes('\\') ? '\\' : '/';
    const raw = await window.easyssh.readTextFile(dir + sep + 'ui-backup.json');
    await mark('readBackup-done');
    if (raw && raw.trim()) {
      const bak = JSON.parse(raw);
      if (bak.groups && !localStorage.getItem('easyssh.groups')) localStorage.setItem('easyssh.groups', bak.groups);
      if (bak.dlDir && !localStorage.getItem('easyssh.dlDir')) localStorage.setItem('easyssh.dlDir', bak.dlDir);
    }
  } catch {}
  await mark('loadSessions-before');
  try {
    setSessions(await window.easyssh.loadSessions());
    await __logUiError('ok', 'loadSessions 完成');
  } catch (e) {
    await __logUiError('loadSessions', String(e && (e.message || e)).slice(0, 200));
  }
  await mark('loadSessions-after');
  // 去重时把显示名称计入:克隆副本与原会话同 host:port:username 但名称带「 - 副本」,必须保留
  const key = (s) => (s.name || '') + '|' + s.host + ':' + (s.port || 22) + ':' + s.username;
  const seen = new Map();
  for (const s of sessions) seen.set(key(s), s);
  setSessions([...seen.values()]);
  renderSessions();
  await mark('rendered='+sessions.length);
  await mark('sessions-rendered');
  setReadyBar();
  // v5(beta) 任务五:启动时检测崩溃日志(桥接命令就绪才生效;状态栏提示 + 设置-关于列表)
  try { await checkCrashLogs(); } catch {}
  await mark('crashLogs');
  // v4.1.1:前端就绪 → 淡出启动画面 + 显示窗口（启动期窗口隐藏,消除白屏）
  try {
    const sp = document.getElementById('bootSplash');
    if (sp) { sp.classList.add('hide'); setTimeout(() => sp.remove(), 350); }
    if (window.__TAURI__ && window.__TAURI__.window) window.__TAURI__.window.getCurrentWindow().show();
    await mark('window-shown');
    // 临时:落盘计时数据(排查后移除)
    try {
      const dir = await window.easyssh.cfgDirPath();
      const sep = dir.includes('\\') ? '\\' : '/';
      await window.easyssh.saveTextFile(dir + sep + 'ui-timing.json',
        JSON.stringify({ wall: Date.now(), marks }));
    } catch {}
  } catch {}
}

/* ---------- 初始化顺序(与原 renderer.js 一致) ---------- */
// 1) 主密码状态检测(启用时弹全屏解锁模态)
// 2) 设置/皮肤/快捷命令加载
// 3) 升级横幅回放最近一次检测结果
// 4) 会话加载 + 启动画面淡出
initMasterPw();
initSettings();
initUpdate();
initManualTooltips(); // v5.2.x/F-R10-21:静态 DOM 按钮tooltip 绑定(幂等)
initGatewayForm(); // R-20261120-005 ③:网关折叠区事件绑定(幂等)
init();
// v5.2.2/R-20261119-005:portable 免安装版任务栏固定引导(初始化完成后延迟 5 秒检查)
setTimeout(() => { try { initTaskbarPin(); } catch {} }, 5000);

/* ============================================================
 * v5.2.2/R-20261119-005:portable 免安装版任务栏固定引导
 * 流程:延迟 5 秒 → settings.taskbarPinDismissed 未置 true 且
 *       invoke('taskbar_pin_status') 返回 pinned=false → 弹一次性引导弹窗。
 *       「固定到任务栏」调 taskbar_pin_do;「不再提示」置 dismissed 并落盘;
 *       「稍后」仅关闭(下次启动再提示)。
 * 弹窗 DOM 动态注入(样式复用 index.html 既有 .modal/.modal-box 体系),不改 index.html。
 * ============================================================ */
function __tbPinT(zh) {
  // i18n:复用全局字典(以简体中文原文为键);zh-CN 原样返回
  const loc = (appSettings && appSettings.lang) || 'zh-CN';
  return (window.EasySSH_I18N && window.EasySSH_I18N.t) ? window.EasySSH_I18N.t(loc, zh) : zh;
}

async function initTaskbarPin() {
  if (!window.__TAURI__ || !window.__TAURI__.core) return; // 非 Tauri 环境(如浏览器直开)跳过
  if (appSettings && appSettings.taskbarPinDismissed === true) return; // 用户已选「不再提示」
  try {
    const r = await window.__TAURI__.core.invoke('taskbar_pin_status');
    if (r && r.pinned) return; // 已固定,无需引导
  } catch { return; } // 命令不可用(旧版后端)静默跳过
  showTaskbarPinModal();
}

function showTaskbarPinModal() {
  if (document.getElementById('tbPinModal')) { document.getElementById('tbPinModal').classList.remove('hidden'); return; }
  document.body.insertAdjacentHTML('beforeend', [
    '<div id="tbPinModal" class="modal hidden">',
    '  <div class="modal-box" style="max-width:460px;">',
    '    <div class="modal-head"><b id="tbPinTitle"></b><button class="close-x" id="tbPinX"><svg class="icon"><use href="#i-x"/></svg></button></div>',
    '    <div class="form-col">',
    '      <div id="tbPinMsg" style="font-size:13px;color:var(--color-text-primary);white-space:pre-line;"></div>',
    '      <div class="tips" id="tbPinPath" style="word-break:break-all;"></div>',
    '      <div id="tbPinStatus" style="color:var(--color-accent);font-size:12px;min-height:16px;"></div>',
    '      <div class="row"><button class="primary" id="tbPinOk"></button><button id="tbPinLater"></button><button id="tbPinNever"></button></div>',
    '    </div>',
    '  </div>',
    '</div>',
  ].join(''));
  const m = document.getElementById('tbPinModal');
  document.getElementById('tbPinTitle').textContent = __tbPinT('把 EasySSH 固定到任务栏?');
  document.getElementById('tbPinMsg').textContent = __tbPinT('检测到 EasySSH 以免安装(portable)方式运行,尚未固定到任务栏。固定后可从任务栏一键快速启动。');
  document.getElementById('tbPinOk').textContent = __tbPinT('固定到任务栏');
  document.getElementById('tbPinLater').textContent = __tbPinT('稍后');
  document.getElementById('tbPinNever').textContent = __tbPinT('不再提示');
  m.classList.remove('hidden');
  // 关闭/点遮罩 = 稍后(仅隐藏,下次启动再提示)
  document.getElementById('tbPinX').addEventListener('click', () => m.classList.add('hidden'));
  document.getElementById('tbPinLater').addEventListener('click', () => m.classList.add('hidden'));
  m.addEventListener('click', (e) => { if (e.target === m) m.classList.add('hidden'); });
  // 「不再提示」:置 dismissed 并按现有 saveSettings 流程落盘
  document.getElementById('tbPinNever').addEventListener('click', async () => {
    try { appSettings.taskbarPinDismissed = true; await window.easyssh.saveSettings(appSettings); } catch {}
    m.classList.add('hidden');
  });
  // 「固定到任务栏」:调 Rust taskbar_pin_do → 成功提示 / need_manual 引导右键手动固定
  document.getElementById('tbPinOk').addEventListener('click', async () => {
    const okBtn = document.getElementById('tbPinOk');
    const statusEl = document.getElementById('tbPinStatus');
    const pathEl = document.getElementById('tbPinPath');
    okBtn.disabled = true;
    statusEl.textContent = __tbPinT('正在固定…');
    try {
      const r = await window.__TAURI__.core.invoke('taskbar_pin_do');
      if (r && r.ok && !r.needManual) {
        statusEl.textContent = __tbPinT('✅ 已固定到任务栏');
        setTimeout(() => m.classList.add('hidden'), 1500);
      } else if (r && r.ok && r.needManual) {
        statusEl.textContent = __tbPinT('未能自动固定(系统限制)。可在开始菜单中右键点击 EasySSH,选择「固定到任务栏」。');
        // 快捷方式路径说明(路径本身不做翻译,单独节点展示)
        pathEl.textContent = __tbPinT('快捷方式位置:') + (r.message ? r.message.replace(/^.*位置: /, ' ') : '');
      } else {
        statusEl.textContent = __tbPinT('固定失败: ') + (r && r.message || __tbPinT('未知错误'));
      }
    } catch (e) {
      statusEl.textContent = __tbPinT('固定失败: ') + String(e && e.message || e);
    }
    okBtn.disabled = false;
  });
}
