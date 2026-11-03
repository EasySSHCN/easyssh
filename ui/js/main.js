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
import { ver, setVer, setSessions } from './state.js';
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

// 全局错误兜底:任何未捕获错误都显示在界面,绝不无声失败
window.addEventListener('error', (e) => {
  const el = $('#connMsg');
  if (el) el.textContent = '脚本错误: ' + (e.message || e.type);
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
    await mark('uiCacheStatus');
    if (need) {
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
    if (raw && raw.trim()) {
      const bak = JSON.parse(raw);
      if (bak.groups && !localStorage.getItem('easyssh.groups')) localStorage.setItem('easyssh.groups', bak.groups);
      if (bak.dlDir && !localStorage.getItem('easyssh.dlDir')) localStorage.setItem('easyssh.dlDir', bak.dlDir);
    }
  } catch {}
  setSessions(await window.easyssh.loadSessions());
  // 去重时把显示名称计入:克隆副本与原会话同 host:port:username 但名称带「 - 副本」,必须保留
  const key = (s) => (s.name || '') + '|' + s.host + ':' + (s.port || 22) + ':' + s.username;
  const seen = new Map();
  for (const s of sessions) seen.set(key(s), s);
  setSessions([...seen.values()]);
  renderSessions();
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
init();
