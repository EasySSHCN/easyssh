/* ============================================================
 * terminal.js — 终端与连接（从 renderer.js 提取）
 * 职责：xterm 终端创建/查找条/剪贴板、标签页、连接表单、
 *       connectSsh 统一连接流程（主密码校验 + 指纹确认）、
 *       断线重连/自动重连、密钥生成与安装、数据/断开事件。
 * ============================================================ */
/* global Terminal, FitAddon, SearchAddon */
import { $ } from './dom.js';
import { tabs, activeId, setActiveId, sessions, pendingSess, setPendingSess, ver, appSettings } from './state.js';
import { masterLocked, saveSessionsGuard } from './masterpw.js';
import { renderSessions } from './sessions.js';
import './palette.js'; // v5(M4) F-M4-05 全局命令面板(副作用引入:注册 Ctrl+Shift+P 全局快捷键)

/* ---------- xterm ----------
 * v5(M4) F-M4-01 终端分屏:
 * - tab 结构扩展:tab.panes = [ { id, opts, term, fit, el, search, webgl, disconnected, ... } ],
 *   tab.activePane 指向活动窗格;tab.term/fit/el/search/webgl/disconnected 均为
 *   委派到活动窗格的 getter/setter(旧代码 tab.term.write 等零改动兼容)。
 * - tab.id 始终等于「活动窗格」的连接 id:SFTP/端口转发/快捷命令等外部模块
 *   都按 tabs.get(activeId).id 工作,活动窗格切换时同步换绑。
 * - 每个 pane 独立 xterm 实例 + 独立 SSH 会话;布局变化/窗口 resize 后逐窗格 fit()。
 */
function defineTabPaneAccessors(tab) {
  const cur = () => tab.panes[Math.min(tab.activePane, tab.panes.length - 1)] || tab.panes[0];
  for (const prop of ['term', 'fit', 'search', 'webgl']) {
    Object.defineProperty(tab, prop, { get() { const p = cur(); return p ? p[prop] : undefined; }, set(v) { const p = cur(); if (p) p[prop] = v; }, configurable: true });
  }
  Object.defineProperty(tab, 'disconnected', {
    get() { const p = cur(); return p ? p.disconnected : true; },
    set(v) { const p = cur(); if (p) p.disconnected = v; },
    configurable: true,
  });
  Object.defineProperty(tab, 'el', { get() { return tab._wrap; }, configurable: true });
}

function makeTerm(tab) {
  const wrap = document.createElement('div');
  wrap.className = 'split-wrap active';
  wrap.dataset.id = tab.id;
  wrap.dataset.layout = 'single';
  $('#terminalArea').insertBefore(wrap, $('#statusBar'));
  tab._wrap = wrap;
  tab.panes = [];
  tab.activePane = 0;
  tab.layout = 'single';
  applySplitTemplate(tab); // v5(beta) 任务二:single 也统一走 JS 模板(清除可能残留的分隔条)
  defineTabPaneAccessors(tab);
  addPaneShell(tab);
  return tab;
}

/* 创建一个窗格外壳(xterm 实例 + 关闭按钮),加入 tab.panes 并返回窗格对象 */
function addPaneShell(tab) {
  const host = document.createElement('div');
  host.className = 'term-host pane';
  host.innerHTML = '<button class="pane-close" title="关闭窗格" aria-label="关闭窗格"><svg class="icon" style="width:11px;height:11px"><use href="#i-x"/></svg></button>';
  tab._wrap.appendChild(host);
  const pane = { id: null, opts: null, name: tab.name, term: null, fit: null, search: null, webgl: null, el: host, disconnected: true, userClosed: false, _autoRetries: 0, _autoTimer: null, _cdTimer: null, _restoreBuf: '', _discAt: 0 };
  makePaneTerm(tab, pane);
  tab.panes.push(pane);
  host.addEventListener('click', (e) => {
    if (e.target.closest('.pane-close')) { closePane(tab, pane); return; }
    setActivePane(tab, pane);
  });
  host.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    pasteToTerm(pane.term);
  });
  updatePaneChrome(tab);
  return pane;
}

/* 单个窗格的 xterm 初始化(原 makeTerm 主体,作用域从 tab 改为 pane) */
function makePaneTerm(tab, pane) {
  const term = new Terminal({
    fontSize: 14,
    fontFamily: 'Consolas, "Courier New", monospace',
    cursorBlink: true,
    theme: { background: '#1e1e2e', foreground: '#cdd6f4' },
    scrollback: Number(appSettings.scrollback) || 5000, // v2.11 回滚行数可配置(5000/10000/50000)
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
  // v2.11 查找插件(与 addon-fit 同样以 UMD script 方式加载,全局 SearchAddon)
  if (window.SearchAddon) {
    const search = new SearchAddon.SearchAddon();
    term.loadAddon(search);
    pane.search = search;
    search.onDidChangeResults(({ resultIndex, resultCount }) => {
      if (tabs.get(activeId) === tab) {
        $('#findCount').textContent = resultCount > 0 ? (resultIndex + 1) + '/' + resultCount : (resultIndex === -1 && resultCount > 0 ? '' : '');
      }
    });
  }
  term.open(pane.el);

  // v5 M2 F-M2-04:WebGL 渲染器(GPU 加速,xterm open 后加载)
  // 失败路径:WebglAddon 全局缺失(vendor 未加载)/构造抛错(WebGL2 不可用,如远程桌面、老驱动)
  // /运行期 onContextLoss(GPU 上下文丢失)→ 统一回退 DOM 渲染器并 console.warn 记录。
  initWebglRenderer(pane, term);

  term.onData((d) => {
    if (pane.disconnected) {
      if (/\r|\n/.test(d)) reconnectPane(tab, pane); // 断开后按回车自动重连
      return;
    }
    window.easyssh.write(pane.id, d);
  });
  term.onResize(({ cols, rows }) => {
    if (pane.id && !pane.disconnected) window.easyssh.resize(pane.id, cols, rows);
  });

  pane.term = term;
  pane.fit = fit;
  term.focus();
  // v2.11 终端快捷键:Ctrl+F 查找 / Ctrl+Shift+C 复制选中 / Ctrl+Shift+V 粘贴
  // (Ctrl+C 不拦截,正常发送中断信号;Ctrl+F 打开查找条,Shift+回车查找上一个)
  term.attachCustomKeyEventHandler((e) => {
    if (e.type !== 'keydown') return true;
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.key === 'f' || e.key === 'F')) {
      openFindBar();
      return false;
    }
    if ((e.ctrlKey || e.metaKey) && e.shiftKey) {
      const k = (e.key || '').toLowerCase();
      if (k === 'c') { copySelection(term); return false; }
      if (k === 'v') { pasteToTerm(term); return false; }
    }
    return true;
  });
  // v2.11 选中即复制(开关开启时,选中变化即写入剪贴板)
  term.onSelectionChange(() => {
    if (!appSettings.copyOnSelect) return;
    const sel = term.getSelection();
    if (sel) { try { navigator.clipboard.writeText(sel); } catch {} }
  });
  return pane;
}

/* ---------- v5(beta) 任务二:分隔条拖拽调比例 + localStorage 持久化 ----------
 * - split2v/split2h/grid 的 Grid 模板改由 JS 生成(fr 值可实时更新),single 保持纯 CSS;
 * - 分隔条 .split-sep 为独立 Grid 轨道(6px 热区),拖拽改 fr、双击恢复等分;
 * - 比例按布局类型存 localStorage('easyssh.split.ratio'),新会话/重启自动恢复;
 * - 拖拽期间 body.split-dragging 禁用文本选择;分隔条非窗格子元素,不触发活动窗格高亮。
 */
const SPLIT_SEP_PX = 6;           // 分隔条轨道宽/高(拖拽热区 ≥6px)
const SPLIT_RATIO_MIN = 0.15;     // 比例下限
const SPLIT_RATIO_MAX = 0.85;     // 比例上限
const SPLIT_LS_KEY = 'easyssh.split.ratio';
const SPLIT_RATIO_DEF = 0.5;      // 等分默认值

function clampSplitRatio(v) {
  const n = Number(v);
  if (!isFinite(n)) return SPLIT_RATIO_DEF;
  return Math.min(SPLIT_RATIO_MAX, Math.max(SPLIT_RATIO_MIN, n));
}
function loadSplitRatios() {
  try { return JSON.parse(localStorage.getItem(SPLIT_LS_KEY) || '{}') || {}; } catch { return {}; }
}
/* layout→val: split2v/split2h 存数字(首个 fr 比例);grid 存 { c, r }(列/行比例) */
function saveSplitRatio(layout, val) {
  try {
    const all = loadSplitRatios();
    all[layout] = val;
    localStorage.setItem(SPLIT_LS_KEY, JSON.stringify(all));
  } catch { /* localStorage 不可用时静默(仅失去持久化) */ }
}

/* 按布局 + 持久化比例生成分屏 Grid 模板(inline style 覆盖 style.css 基准 1fr 1fr) */
function applySplitTemplate(tab) {
  if (!tab || !tab._wrap) return;
  const wrap = tab._wrap;
  // 每次重建:清掉旧分隔条与窗格占位,避免跨布局残留
  wrap.querySelectorAll('.split-sep').forEach((s) => s.remove());
  for (const p of tab.panes) p.el.style.gridArea = '';
  const layout = tab.layout;
  if (layout === 'single') {
    wrap.style.gridTemplateColumns = '1fr';
    wrap.style.gridTemplateRows = '1fr';
    return;
  }
  const saved = loadSplitRatios();
  const place = (el, css) => { for (const k in css) el.style[k] = css[k]; };
  const getSep = (cls, area) => {
    let sep = wrap.querySelector('.split-sep[data-area="' + area + '"]');
    if (!sep) {
      sep = document.createElement('div');
      sep.className = 'split-sep ' + cls;
      sep.dataset.area = area;
      wrap.appendChild(sep);
      attachSplitDrag(tab, sep, area);
    }
    return sep;
  };
  if (layout === 'split2v') {
    const a = clampSplitRatio(saved.split2v);
    wrap.style.gridTemplateColumns = a + 'fr ' + SPLIT_SEP_PX + 'px ' + (1 - a) + 'fr';
    wrap.style.gridTemplateRows = '1fr';
    if (tab.panes[0]) place(tab.panes[0].el, { gridArea: '1 / 1' });
    if (tab.panes[1]) place(tab.panes[1].el, { gridArea: '1 / 3' });
    place(getSep('v', 'col'), { gridArea: '1 / 2' });
  } else if (layout === 'split2h') {
    const a = clampSplitRatio(saved.split2h);
    wrap.style.gridTemplateColumns = '1fr';
    wrap.style.gridTemplateRows = a + 'fr ' + SPLIT_SEP_PX + 'px ' + (1 - a) + 'fr';
    if (tab.panes[0]) place(tab.panes[0].el, { gridArea: '1 / 1' });
    if (tab.panes[1]) place(tab.panes[1].el, { gridArea: '3 / 1' });
    place(getSep('h', 'row'), { gridArea: '2 / 1' });
  } else if (layout === 'grid') {
    const g = saved.grid || {};
    const c = clampSplitRatio(g.c);
    const r = clampSplitRatio(g.r);
    wrap.style.gridTemplateColumns = c + 'fr ' + SPLIT_SEP_PX + 'px ' + (1 - c) + 'fr';
    wrap.style.gridTemplateRows = r + 'fr ' + SPLIT_SEP_PX + 'px ' + (1 - r) + 'fr';
    // 四宫格固定占位:窗格按数组顺序放 2×2,分隔条贯穿中列/中行
    const cells = ['1 / 1', '1 / 3', '3 / 1', '3 / 3'];
    tab.panes.forEach((p, i) => { if (cells[i]) place(p.el, { gridArea: cells[i] }); });
    place(getSep('v', 'col'), { gridColumn: '2', gridRow: '1 / -1' });
    place(getSep('h', 'row'), { gridRow: '2', gridColumn: '1 / -1' });
  }
}

/* 分隔条拖拽:pointer capture + pointermove 实时改 fr;持久化收敛到拖拽结束落盘 */
function attachSplitDrag(tab, sep, area) {
  let dragging = false;
  let rafPending = false;
  const ratioFromEvent = (e) => {
    const rect = tab._wrap.getBoundingClientRect();
    if (area === 'col') {
      return (e.clientX - rect.left - SPLIT_SEP_PX / 2) / Math.max(1, rect.width - SPLIT_SEP_PX);
    }
    return (e.clientY - rect.top - SPLIT_SEP_PX / 2) / Math.max(1, rect.height - SPLIT_SEP_PX);
  };
  const applyRatio = (ratio) => {
    const a = clampSplitRatio(ratio);
    const wrap = tab._wrap;
    if (tab.layout === 'split2v' || (tab.layout === 'grid' && area === 'col')) {
      wrap.style.gridTemplateColumns = a + 'fr ' + SPLIT_SEP_PX + 'px ' + (1 - a) + 'fr';
    } else {
      wrap.style.gridTemplateRows = a + 'fr ' + SPLIT_SEP_PX + 'px ' + (1 - a) + 'fr';
    }
    // xterm 尺寸随 fr 变化,合并到帧内统一 fit
    if (!rafPending) {
      rafPending = true;
      requestAnimationFrame(() => { rafPending = false; fitAllPanes(tab); });
    }
  };
  const persistRatio = () => {
    // 从当前模板反读比例落盘(与拖拽终态严格一致)
    const m = area === 'col'
      ? (tab._wrap.style.gridTemplateColumns || '').match(/^([\d.]+)fr/)
      : (tab._wrap.style.gridTemplateRows || '').match(/^([\d.]+)fr/);
    const val = clampSplitRatio(m ? m[1] : SPLIT_RATIO_DEF);
    if (tab.layout === 'grid') {
      const cur = loadSplitRatios().grid || {};
      saveSplitRatio('grid', area === 'col' ? { ...cur, c: val } : { ...cur, r: val });
    } else {
      saveSplitRatio(tab.layout, val);
    }
  };
  sep.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    e.stopPropagation(); // 不触发下方窗格的点击激活
    dragging = true;
    try { sep.setPointerCapture(e.pointerId); } catch {}
    sep.classList.add('dragging');
    document.body.classList.add('split-dragging'); // 拖拽期间禁用文本选择
  });
  sep.addEventListener('pointermove', (e) => {
    if (dragging) applyRatio(ratioFromEvent(e));
  });
  const finish = () => {
    if (!dragging) return;
    dragging = false;
    sep.classList.remove('dragging');
    document.body.classList.remove('split-dragging');
    persistRatio();
    fitAllPanes(tab);
  };
  sep.addEventListener('pointerup', finish);
  sep.addEventListener('pointercancel', finish);
  // 双击恢复等分(并按当前布局持久化)
  sep.addEventListener('dblclick', (e) => {
    e.preventDefault();
    applyRatio(SPLIT_RATIO_DEF);
    persistRatio();
  });
}

/* ---------- v5(M4) 分屏:活动窗格 / 布局 / 焦点 / fit ---------- */
/* 活动窗格高亮 + tab.id 换绑到活动窗格连接 + 状态栏 + 逐窗格 fit */
function setActivePane(tab, pane) {
  const idx = tab.panes.indexOf(pane);
  if (idx < 0) return;
  tab.activePane = idx;
  tab.panes.forEach((p) => p.el.classList.toggle('active-pane', p === pane));
  syncTabPrimaryId(tab);
  $('#statusBar').textContent = pane.disconnected
    ? pane.name + ' 已断开 — 在终端按回车重连'
    : '已连接: ' + tab.name + (tab.panes.length > 1 ? '(窗格 ' + (idx + 1) + '/' + tab.panes.length + ')' : '');
  requestAnimationFrame(() => fitAllPanes(tab));
  pane.term.focus();
}

/* tab.id 换绑到活动窗格的连接 id(SFTP/转发/快捷命令等外部模块都按 tab.id 工作) */
function syncTabPrimaryId(tab) {
  const pane = tab.panes[tab.activePane];
  if (!pane || !pane.id || pane.id === tab.id) return;
  tabs.delete(tab.id);
  tab.id = pane.id;
  tabs.set(tab.id, tab);
  tab._wrap.dataset.id = tab.id;
  if (tab.tabEl) tab.tabEl.dataset.id = tab.id;
}

/* 布局变化/窗口 resize 后对所有窗格逐个 fit(可见性由 .split-wrap.active 保证) */
function fitAllPanes(tab) {
  if (!tab || !tab.panes) return;
  for (const p of tab.panes) { try { p.fit.fit(); } catch {} }
}

/* 切换布局;不足的窗格自动补开(每格独立会话,连到同一服务器) */
function setLayout(tab, layout) {
  if (!tab) return;
  tab.layout = layout;
  tab._wrap.dataset.layout = layout;
  const need = layout === 'grid' ? 4 : (layout === 'split2v' || layout === 'split2h') ? 2 : 1;
  if (layout === 'single') {
    // 恢复单屏:仅保留活动窗格,其余窗格关闭
    while (tab.panes.length > 1) closePane(tab, tab.panes[tab.panes.length - 1]);
  } else {
    while (tab.panes.length < need) addSessionPane(tab);
  }
  applySplitTemplate(tab); // v5(beta) 任务二:布局模板由 JS 生成(含持久化比例)
  updatePaneChrome(tab);
  requestAnimationFrame(() => fitAllPanes(tab));
}

/* 新增窗格并按当前标签的会话参数开一条独立连接 */
function addSessionPane(tab) {
  if (!tab.opts) return null;
  const pane = addPaneShell(tab);
  setActivePane(tab, pane);
  pane.term.write('\x1b[33m[新窗格: 正在连接 ' + tab.opts.host + ' ...]\x1b[0m\r\n');
  connectPane(tab, pane, tab.opts).then((ok) => {
    if (ok) {
      pane.term.write('\x1b[32m[已连接 ' + tab.opts.host + ']\x1b[0m\r\n');
      $('#statusBar').textContent = '已连接: ' + tab.name;
    }
    updatePaneChrome(tab);
  });
  return pane;
}

/* 窗格关闭按钮显隐(单窗格时不显示,保留至少一个窗格) */
function updatePaneChrome(tab) {
  const multi = tab.panes.length > 1;
  for (const p of tab.panes) {
    const btn = p.el.querySelector('.pane-close');
    if (btn) btn.classList.toggle('has-panes', multi);
  }
}

function closePane(tab, pane) {
  if (tab.panes.length <= 1) {
    $('#statusBar').textContent = '分屏至少保留一个窗格(关闭标签请用标签上的 ×)';
    return;
  }
  pane.userClosed = true;
  stopRetryCountdown(pane);
  clearTimeout(pane._autoTimer);
  if (pane.id) window.easyssh.disconnect(pane.id);
  const idx = tab.panes.indexOf(pane);
  try { pane.term.dispose(); } catch {}
  pane.el.remove();
  tab.panes.splice(idx, 1);
  if (tab.activePane >= tab.panes.length) tab.activePane = tab.panes.length - 1;
  applySplitTemplate(tab); // v5(beta) 任务二:窗格数变化后重建占位与分隔条
  updatePaneChrome(tab);
  setActivePane(tab, tab.panes[tab.activePane]);
}

/* Ctrl+Shift+D 循环切换布局:单屏 → 左右 → 上下 → 四宫格 → 单屏 */
function cycleLayout() {
  const t = tabs.get(activeId);
  if (!t) return;
  const order = ['single', 'split2v', 'split2h', 'grid'];
  const next = order[(order.indexOf(t.layout || 'single') + 1) % order.length];
  setLayout(t, next);
  $('#statusBar').textContent = '分屏布局: ' + ({ single: '单屏', split2v: '左右分屏', split2h: '上下分屏', grid: '四宫格' })[next];
}
/* Ctrl+Alt+方向键 在窗格间移动焦点(顺序 = 窗格数组顺序) */
function movePaneFocus(delta) {
  const t = tabs.get(activeId);
  if (!t || t.panes.length < 2) return;
  const next = (t.activePane + delta + t.panes.length) % t.panes.length;
  setActivePane(t, t.panes[next]);
}

/* ---------- v5 M2:WebGL 渲染器接入与降级(F-M2-04) ---------- */
function initWebglRenderer(tab, term) {
  try {
    if (!window.WebglAddon || !window.WebglAddon.WebglAddon) {
      console.warn('[EasySSH] addon-webgl 未加载,回退 DOM 渲染器');
      return;
    }
    const webgl = new WebglAddon.WebglAddon();
    webgl.onContextLoss(() => fallbackToDom(tab, term, webgl, 'WebGL 上下文丢失(contextLoss)'));
    term.loadAddon(webgl);
    tab.webgl = webgl;
  } catch (err) {
    console.warn('[EasySSH] WebGL 初始化失败,已回退 DOM 渲染器:', err && err.message || err);
  }
}
function fallbackToDom(tab, term, webgl, reason) {
  try {
    if (tab.webgl === webgl) tab.webgl = null;
    webgl.dispose(); // xterm 在 dispose 时自动恢复默认 DOM 渲染器
    console.warn('[EasySSH] 已回退 DOM 渲染器:', reason);
  } catch (err) { console.warn('[EasySSH] DOM 渲染器回退异常:', err); }
}

/* ---------- v5 M2:断线重连会话恢复(scrollback 文本级恢复) ----------
 * 边界说明:
 * - 恢复内容:断线前 scrollback 的纯文本(translateToString,含换行),以分隔行标注断线时长。
 * - 未恢复:ANSI 颜色/样式、alternate screen(vim/htop 等全屏程序画面)、光标位置、
 *   cwd/env(shell 集成 OSC133,标记 P2 延后,依赖服务端 shell 钩子上报提示符/目录/环境)。
 */
function exportBufferText(term) {
  try {
    const buf = term.buffer.active;
    if (!buf || buf.type !== 'normal') return ''; // alternate screen 无法可靠导出
    const lines = [];
    for (let i = 0; i < buf.length; i++) lines.push(buf.getLine(i).translateToString(true));
    // 去掉末尾全空行,避免恢复后 scrollback 底部大量空白
    while (lines.length && !lines[lines.length - 1]) lines.pop();
    return lines.join('\n');
  } catch { return ''; }
}
function restoreSessionBuffer(pane) {
  const text = pane._restoreBuf || '';
  pane._restoreBuf = '';
  const ms = Date.now() - (pane._discAt || Date.now());
  const sec = Math.round(ms / 1000);
  const dur = sec >= 60 ? Math.floor(sec / 60) + ' 分 ' + (sec % 60) + ' 秒' : sec + ' 秒';
  if (text) {
    pane.term.write('\r\n\x1b[36m── 会话已恢复(断线 ' + dur + ') ──\x1b[0m\r\n' + text + '\r\n');
  } else {
    pane.term.write('\x1b[36m── 会话已恢复(断线 ' + dur + ') ──\x1b[0m\r\n');
  }
}

/* ================= v2.11 终端体验:查找条 / 剪贴板增强 ================= */
function copySelection(term) {
  const sel = term.getSelection();
  if (!sel) return; // 无选中时放行,不影响原按键语义
  try {
    navigator.clipboard.writeText(sel).then(() => { $('#statusBar').textContent = '已复制选中内容'; }, () => {});
  } catch {}
}
function pasteToTerm(term) {
  navigator.clipboard.readText().then((text) => {
    if (text) term.paste(text);
  }).catch(() => {
    $('#statusBar').textContent = '无法读取剪贴板,请改用 Ctrl+Shift+V 粘贴';
  });
}
function openFindBar() {
  const t = tabs.get(activeId);
  if (!t) { $('#statusBar').textContent = '请先连接一个会话再使用查找'; return; }
  $('#findBar').classList.remove('hidden');
  $('#findInput').focus();
  $('#findInput').select();
}
function findInTerm(dir) {
  const t = tabs.get(activeId);
  if (!t) return;
  if (!t.search) { $('#findCount').textContent = '查找插件不可用'; return; }
  const q = $('#findInput').value;
  if (!q) return;
  const opts = {
    // v5(M3):查找高亮跟随品牌 Emerald(xterm API 仅接受具体色值,取自 --color-* token 基准)
    decorations: {
      matchBackground: 'rgba(52, 211, 153, 0.35)',
      matchOverviewRuler: '#34D399',
      activeMatchBackground: 'rgba(251, 191, 36, 0.6)',
      activeMatchColorOverviewRuler: '#FBBF24',
      activeMatchBorder: '#FBBF24',
    },
  };
  try { dir > 0 ? t.search.findNext(q, opts) : t.search.findPrevious(q, opts); } catch {}
}
function closeFindBar() {
  $('#findBar').classList.add('hidden');
  $('#findCount').textContent = '';
  for (const t of tabs.values()) {
    try { if (t.search) t.search.clearDecorations(); } catch {}
  }
  const t = tabs.get(activeId);
  if (t) t.term.focus();
}
$('#findInput').addEventListener('keydown', (e) => {
  if (e.key === 'Enter') findInTerm(e.shiftKey ? -1 : 1); // 回车下一个,Shift+回车上一个
  else if (e.key === 'Escape') closeFindBar();
  e.stopPropagation(); // 查找条按键不透传到终端
});
$('#findNext').addEventListener('click', () => findInTerm(1));
$('#findPrev').addEventListener('click', () => findInTerm(-1));
$('#findClose').addEventListener('click', closeFindBar);
$('#btnFind').addEventListener('click', openFindBar);
// 终端区域全局 Ctrl+F(焦点不在终端时也可打开)
document.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && !e.shiftKey && (e.key === 'f' || e.key === 'F') && tabs.has(activeId)) {
    e.preventDefault();
    openFindBar();
  }
});

function activate(id) {
  setActiveId(id);
  for (const [tid, t] of tabs) t._wrap.classList.toggle('active', tid === id);
  $('#connectPanel').style.display = 'none';
  document.querySelectorAll('.tab').forEach((el) => el.classList.toggle('active', el.dataset.id === id));
  const t = tabs.get(id);
  if (t) {
    // 活动窗格换绑 tab.id 后再 fit(布局切换/标签切换后所有窗格尺寸都可能过期)
    setActivePane(t, t.panes[t.activePane]);
  }
}

function closeTab(id) {
  const t0 = tabs.get(id);
  if (t0) { t0.userClosed = true; for (const p of t0.panes) { p.userClosed = true; stopRetryCountdown(p); clearTimeout(p._autoTimer); } } // v4.3.0:标记主动关闭,阻止自动重连
  // v5(M4) 分屏:断开并释放标签下所有窗格的连接
  if (t0) for (const p of t0.panes) { if (p.id) window.easyssh.disconnect(p.id); }
  const t = tabs.get(id);
  if (t) { for (const p of t.panes) { try { p.term.dispose(); } catch {} } t._wrap.remove(); tabs.delete(id); }
  const tabEl = document.querySelector('.tab[data-id="' + id + '"]');
  if (tabEl) tabEl.remove();
  const remaining = [...tabs.keys()];
  if (remaining.length) activate(remaining[remaining.length - 1]);
  else { $('#connectPanel').style.display = 'block'; $('#statusBar').textContent = 'EasySSH ' + ver + ' — 就绪'; }
}

/* ---------- tab bar ---------- */
function addTabEl(tab) {
  const el = document.createElement('div');
  el.className = 'tab';
  el.dataset.id = tab.id;
  el.innerHTML = '<span>' + tab.name + '</span><span class="x" title="关闭标签" aria-label="关闭标签"><svg class="icon"><use href="#i-x"/></svg></span>';
  // v2.9.0:会话设置了标签颜色则应用到标签页标题
  if (tab.color) el.style.color = tab.color;
  el.addEventListener('click', (e) => {
    if (e.target.closest('.x')) closeTab(tab.id); // v5(M3):closest 兼容内嵌 SVG 图标
    else activate(tab.id);
  });
  tab.tabEl = el;
  $('#tabbar').appendChild(el);
}

/* ---------- connect / reconnect ---------- */

/* 统一 SSH 连接入口:主密码解锁校验 → 指纹查库 → 连接 → 首次信任/不匹配处理。
 * 所有连接(表单提交/断线重连)都走这里;30 秒超时围住真正的网络等待,
 * 指纹确认框等待用户操作,不占超时。 */
async function connectSsh(baseOpts) {
  if (masterLocked()) {
    $('#statusBar').textContent = '主密码未解锁,请先在启动模态中输入主密码';
    return { ok: false, info: '主密码未解锁:已保存的会话密码暂时不可用,请先解锁后再连接。' };
  }
  const host = String(baseOpts.host || '');
  const port = Number(baseOpts.port) || 22;
  const connectOnce = async (opts) => {
    const race = Promise.race([
      window.easyssh.connect(opts),
      new Promise((resolve) => setTimeout(() => resolve({ ok: false, info: '连接超时(30秒无响应)' }), 30000)),
    ]);
    try { return await race; } catch (err) { return { ok: false, info: String(err && err.message || err) }; }
  };
  // 1) 查已知主机指纹:有记录 → 作为期望指纹传给主进程严格校验
  let saved = null;
  try { saved = await window.easyssh.knownHostsGet(host, port); } catch {}
  let opts = { ...baseOpts, cols: 80, rows: 24 };
  if (saved) opts.expectedFingerprint = saved;

  // 2) 连接(始终由主进程把实际指纹带回来)
  let res = await connectOnce(opts);

  // 3) 指纹不匹配(MITM 场景):必须拒绝;用户可选「更新指纹并连接」或取消
  if (!res.ok && res.mismatch && res.fingerprint) {
    const act = await askHostKey({
      mismatch: true, host, port,
      fp: res.fingerprint, stored: saved || res.stored,
    });
    if (act === 'update') {
      await window.easyssh.knownHostsRecord(host, port, res.fingerprint);
      opts = { ...opts, expectedFingerprint: res.fingerprint };
      res = await connectOnce(opts);
    } else {
      return { ok: false, info: '已取消连接:服务器指纹与已保存指纹不一致,可能存在中间人攻击(MITM)。' };
    }
  }

  // 4) 连接成功且该 host:port 无记录(首次连接):弹确认框,信任则保存,否则立即断开
  if (res.ok && !saved && res.fingerprint) {
    const act = await askHostKey({ mismatch: false, host, port, fp: res.fingerprint });
    if (act !== 'trust') {
      window.easyssh.disconnect(res.id); // 立刻断开该连接
      return { ok: false, info: '未信任该服务器,连接已断开(指纹未保存,下次连接会再次确认)。' };
    }
    await window.easyssh.knownHostsRecord(host, port, res.fingerprint);
  }
  return res;
}

/* 指纹确认模态(Promise 化):resolve('trust'|'reject'|'update'|'cancel') */
function askHostKey({ mismatch, host, port, fp, stored }) {
  return new Promise((resolve) => {
    const m = $('#hkModal');
    const fin = (v) => { m.classList.add('hidden'); $('#hkMsg').textContent = ''; resolve(v); };
    if (mismatch) {
      $('#hkTitle').textContent = '⚠ 服务器指纹不匹配!';
      $('#hkText').textContent = '目标: ' + host + ':' + port + '\n'
        + '已保存指纹: ' + (stored || '(未知)') + '\n'
        + '实际指纹:   ' + fp + '\n\n'
        + '两次指纹不一致,可能是服务器重装/更换了密钥,也可能存在中间人攻击(MITM)。连接已被拒绝。';
      $('#hkMsg').textContent = '若你确认服务器确实更换过密钥(如重装系统),可选择「更新指纹并连接」;不确定时请勿继续。';
      $('#hkTips').textContent = '指纹 = 服务器主机密钥的 SHA256 摘要;保存后每次连接自动核对,防劫持。';
      $('#hkTrust').classList.add('hidden');
      $('#hkReject').classList.add('hidden');
      $('#hkUpdate').classList.remove('hidden');
      $('#hkCancel').classList.remove('hidden');
      $('#hkUpdate').onclick = () => fin('update');
      $('#hkCancel').onclick = () => fin('cancel');
    } else {
      $('#hkTitle').textContent = '首次连接 ' + host + ':' + port;
      $('#hkText').textContent = '服务器主机指纹:\n' + fp + '\n\n请核对无误后信任保存;保存后此服务器每次连接都会自动校验该指纹。';
      $('#hkMsg').textContent = '';
      $('#hkTips').textContent = '不信任则立即断开该连接,且不保存指纹。';
      $('#hkTrust').classList.remove('hidden');
      $('#hkReject').classList.remove('hidden');
      $('#hkUpdate').classList.add('hidden');
      $('#hkCancel').classList.add('hidden');
      $('#hkTrust').onclick = () => fin('trust');
      $('#hkReject').onclick = () => fin('reject');
    }
    m.classList.remove('hidden');
  });
}

/* v5(M4) 分屏:连接/重连以「窗格」为单位(每格独立会话) */
async function connectPane(tab, pane, opts) {
  const res = await connectSsh(opts);
  if (!res.ok) {
    const first = String(res.info || '未知错误').split('\n')[0];
    pane.disconnected = true;
    pane.term.write('\r\n\x1b[31m[连接失败: ' + first + ' — 按回车重试]\x1b[0m\r\n');
    $('#connMsg').textContent = res.info || '';
    $('#statusBar').textContent = '连接失败';
    return false;
  }
  pane.id = res.id;
  pane.opts = opts;
  pane.disconnected = false;
  if (tab.panes[tab.activePane] === pane) syncTabPrimaryId(tab); // 活动窗格才换绑 tab.id
  return true;
}

async function reconnectPane(tab, pane) {
  if (!pane.opts) return;
  clearTimeout(pane._autoTimer); // 手动重连取消挂起的自动重连
  stopRetryCountdown(pane);
  pane.userClosed = false;
  pane._autoRetries = 0;
  pane.term.write('\x1b[33m[正在重连 ' + pane.opts.host + ' ...]\x1b[0m\r\n');
  $('#statusBar').textContent = '重连中 ' + pane.opts.host + ' ...';
  const ok = await connectPane(tab, pane, pane.opts);
  if (ok) {
    restoreSessionBuffer(pane); // v5 M2:重连成功恢复断线前 scrollback(同一窗格,不重建)
    pane.term.write('\x1b[32m[已重新连接]\x1b[0m\r\n');
    $('#statusBar').textContent = '已连接: ' + tab.name;
    pane.term.focus();
  } else {
    $('#statusBar').textContent = pane.name + ' 已断开 — 按回车重连';
  }
}

/* ---------- 连接表单提交 ---------- */
$('#authtype').addEventListener('change', (e) => {
  const key = e.target.value === 'key';
  $('#pwLabel').style.display = key ? 'none' : 'flex';
  $('#keyLabel').style.display = key ? 'flex' : 'none';
  $('#ppLabel').style.display = key ? 'flex' : 'none';
  $('#keyTools').style.display = key ? 'flex' : 'none';
});
$('#btnCancel').addEventListener('click', () => { $('#connMsg').textContent = ''; });

// 「＋ 新建连接」:显示新建连接面板(已打开连接时该面板被隐藏,重新点击应恢复显示)
$('#btnNew').addEventListener('click', () => {
  setPendingSess(null); // 手动新建连接,不关联任何已存会话
  $('#connectPanel').style.display = 'block';
  $('#connMsg').textContent = '';
  const form = $('#connectForm');
  form.host.focus();
});

$('#connectForm').addEventListener('submit', async (e) => {
  e.preventDefault();
  const f = new FormData(e.target);
  const opts = Object.fromEntries(f.entries());
  $('#connMsg').textContent = '';
  const t0 = Date.now();
  const tick = setInterval(() => {
    $('#statusBar').textContent = '连接中 ' + opts.host + ' ... ' + Math.round((Date.now() - t0) / 1000) + ' 秒 (超时上限30秒)';
  }, 500);

  const ekey = (s) => s.host + ':' + (s.port || 22) + ':' + s.username;
  // v2.9.0:从列表点进来的会话优先按原条目匹配(克隆副本与原会话同 host:port:username,需区分)
  const eexisting = (pendingSess && ekey(pendingSess) === ekey(opts)) ? pendingSess : sessions.find((s) => ekey(s) === ekey(opts));
  const sessionForwards = (eexisting && Array.isArray(eexisting.forwards)) ? eexisting.forwards : []; // 随会话自动启动的端口转发
  // v3.0.0:统一走 connectSsh(主密码解锁校验 + 已知主机指纹确认流程,30 秒超时已内置)
  let res;
  try {
    res = await connectSsh({
      host: opts.host,
      port: Number(opts.port) || 22,
      username: opts.username,
      forwards: sessionForwards,
      password: opts.authtype === 'password' ? opts.password : undefined,
      privateKey: opts.authtype === 'key' ? opts.privateKey : undefined,
      passphrase: opts.authtype === 'key' ? opts.passphrase : undefined,
    });
  } catch (err) {
    res = { ok: false, info: String(err && err.message || err) };
  }
  clearInterval(tick);
  if (!res || typeof res.ok === 'undefined') res = { ok: false, info: '未知错误' };

  const name = opts.username + '@' + opts.host;

  if (!res.ok) {
    const first = String(res.info || '').split('\n')[0];
    let hint = '';
    if (/authentication methods failed/i.test(first)) {
      if (opts.authtype === 'key') {
        hint = '\n提示:私钥认证被服务器拒绝。请先用「密码」方式连上服务器,重新点一次「安装到当前已连服务器」' +
          '(新版本会自动修正家目录权限与 SELinux 上下文,并回读校验公钥是否写入成功),再改回「私钥文件」登录;' +
          '若安装时提示 PubkeyAuthentication no,需在服务器 /etc/ssh/sshd_config 里开启公钥认证并重启 sshd。';
        // 自动诊断:若这台服务器还有一条在线的密码连接,直接到服务器上查真实原因
        autoDiagnoseKey(opts);
      } else {
        hint = '\n提示:服务器拒绝了用户名/密码。请重新手动输入密码(注意大小写、末尾空格、输入法);' +
          '若勾选了保存会话,之前可能保存了错误密码并被覆盖,连接成功后会自动更新为正确密码。';
      }
    } else if (/timed out|timeout/i.test(first)) {
      hint = '\n提示:网络不通或服务器防火墙拦截,请确认 IP/端口可达。';
    } else if (/ECONNREFUSED/i.test(first)) {
      hint = '\n提示:端口未开放,确认 SSH 服务是否运行、端口是否正确。';
    }
    // 显示完整报错(含 main 追加的「本机已加载私钥」指纹与握手跟踪),不再截断成首行
    $('#connMsg').textContent = '连接失败: ' + first + hint
      + (String(res.info || '').includes('\n') ? '\n' + String(res.info).split('\n').slice(1).join('\n') : '');
    $('#statusBar').textContent = '连接失败: ' + first;
    return;
  }

  // 保存会话:密码连接存密码;密钥连接存私钥路径(私钥文件持久保存在 keys/ 目录,重启后可直接复用)
  if (opts.save) {
    const key = (s) => s.host + ':' + (s.port || 22) + ':' + s.username;
    const nk = key({ host: opts.host, port: opts.port, username: opts.username });
    // v2.9.0:优先定位点进来的那条会话(克隆副本与原会话同 key,避免改错条目)
    let existing = (pendingSess && key(pendingSess) === nk) ? sessions.indexOf(pendingSess) : -1;
    if (existing < 0) existing = sessions.findIndex((s) => key(s) === nk);
    const rec = { host: opts.host, port: opts.port, username: opts.username, authtype: opts.authtype };
    // v2.9.0:重新保存会话时保留用户已有的显示名称/分组/备注/标签颜色(旧配置没有这些字段也不受影响)
    const prevSess = existing >= 0 ? sessions[existing] : null;
    if (prevSess) for (const f of ['name', 'group', 'note', 'color']) if (typeof prevSess[f] !== 'undefined' && prevSess[f] !== '') rec[f] = prevSess[f];
    if (opts.authtype === 'password') {
      rec.password = opts.password;
    } else {
      rec.privateKey = opts.privateKey;
      rec.passphrase = opts.passphrase || '';
    }
    if (existing >= 0) sessions[existing] = rec; else sessions.push(rec);
    rec.forwards = sessionForwards; // 端口转发配置随会话持久化
    await saveSessionsGuard(sessions);
    renderSessions();
  }

  // v2.9.0:标签页标题颜色随会话的 color 字段(无则默认皮肤色)
  const tab = { id: res.id, name, color: (eexisting && eexisting.color) || '', opts: {
    host: opts.host, port: Number(opts.port) || 22, username: opts.username,
    authtype: opts.authtype, // 必须随 opts 保存:断线按回车重连时主进程要靠它选认证分支,否则密钥连接会被当成密码重连
    forwards: sessionForwards, // 重连时主进程随 ready 自动重启这些转发
    password: opts.authtype === 'password' ? opts.password : undefined,
    privateKey: opts.authtype === 'key' ? opts.privateKey : undefined,
    passphrase: opts.authtype === 'key' ? opts.passphrase : undefined,
  }, disconnected: false };
  try {
    makeTerm(tab);
    addTabEl(tab);
    tabs.set(tab.id, tab);
    activate(tab.id);
  } catch (err) {
    $('#connMsg').textContent = '终端初始化失败: ' + (err && err.message || err);
    window.easyssh.disconnect(tab.id);
  }
});

/* 密钥登录失败自动诊断:借同服务器仍在网的密码连接,到服务器上查真实拒绝原因 */
async function autoDiagnoseKey(opts) {
  const key = (t) => t && t.opts && t.opts.host + ':' + t.opts.port + ':' + t.opts.username;
  const target = opts.host + ':' + (Number(opts.port) || 22) + ':' + opts.username;
  let live = null;
  for (const t of tabs.values()) if (!t.disconnected && key(t) === target) live = t;
  if (!live) return;
  try {
    const raw = await window.easyssh.diagnoseKey(live.id, generatedPub || '');
    const sec = (name) => {
      const m = raw.match(new RegExp('@@' + name + '\\n([\\s\\S]*?)(?=@@[A-Z_]+\\n|$)'));
      return m ? m[1].trim() : '';
    };
    const findings = [];
    const who = sec('WHO');
    const user = (who.split('\n')[0] || '').trim();
    const perms = sec('PERMS').split('\n');
    for (const line of perms) {
      const m = line.match(/^([d\-][rwxstST-]{9})\s/);
      if (!m) continue;
      const p = m[1];
      const name = line.split(/\s+/).pop();
      if (p[5] === 'w' || p[8] === 'w') findings.push('✗ ' + name + ' 对组/其他用户可写(' + p + '),sshd StrictModes 会拒绝读取 authorized_keys —— 已在新版「安装公钥」中自动修复,请重新点一次「安装到当前已连服务器」');
      const owner = line.split(/\s+/)[2];
      if (/authorized_keys$/.test(name) && user && owner && owner !== user) findings.push('✗ ' + name + ' 属主是 ' + owner + ',不是登录用户 ' + user + ',sshd 会拒绝;请在服务器执行: chown ' + user + ':' + user + ' ' + name);
    }
    const sel = sec('SELINUX');
    if (/^Enforcing/im.test(sel) && /ssh_home_t/.test(sel) === false && /no-selinux/.test(sel) === false) {
      findings.push('✗ SELinux 处于 Enforcing 且 ~/.ssh 上下文不是 ssh_home_t —— 已在新版「安装公钥」中自动 restorecon,请重新点一次「安装到当前已连服务器」');
    }
    const cfg = sec('SSHD_CONFIG');
    const cfgv = (k) => { const m = cfg.match(new RegExp('^' + k + '\\s+(\\S+)', 'mi')); return m ? m[1] : ''; };
    if (/^no$/i.test(cfgv('pubkeyauthentication'))) findings.push('✗ 服务器 sshd 关闭了公钥认证(PubkeyAuthentication no):请修改 /etc/ssh/sshd_config 为「PubkeyAuthentication yes」并执行 systemctl restart sshd');
    const akf = cfgv('authorizedkeysfile');
    if (akf && !/\.ssh\/authorized_keys/.test(akf)) findings.push('✗ 服务器 sshd 的 AuthorizedKeysFile 是「' + akf + '」(不是默认的 .ssh/authorized_keys),公钥需要写到这里');
    if (/^no$/i.test(cfgv('permitrootlogin'))) findings.push('✗ 服务器 sshd 禁止 root 登录(PermitRootLogin no)');
    const am = cfgv('authenticationmethods');
    if (am && am.split(',').length > 1) findings.push('✗ 服务器要求多步认证 AuthenticationMethods=' + am + ',单一密钥无法直接登录');
    if (/dropbear-present/.test(raw) && !/sshd-binary|pubkeyauthentication/i.test(cfg)) findings.push('ℹ 服务器可能是 dropbear(非 OpenSSH),配置项名称不同,以上 sshd 配置段可能为空');
    if (sec('KEY_PRESENT') === '0') findings.push('✗ 公钥不在服务器 authorized_keys 里 —— 请点「安装到当前已连服务器」重新安装');
    else if (sec('KEY_PRESENT') === '') findings.push('? 无法确认公钥是否已写入服务器 authorized_keys(本机公钥文件读取失败)');
    const fp = sec('AUTHKEYS_FP');
    const logs = sec('SSHD_LOG');
    let diag = '\n---- 自动诊断(来自服务器) ----';
    diag += '\n' + (findings.length ? findings.join('\n') : '未发现明显配置问题;下面是服务器 authorized_keys 指纹与 sshd 日志中的拒绝记录:');
    if (fp && fp !== 'fingerprint-tool-unavailable') diag += '\n[服务器 authorized_keys 指纹]\n' + fp + '\n(与本机公钥比对:指纹不一致说明服务器装的是旧公钥,请重新点「安装到当前已连服务器」)';
    if (logs) diag += '\n[服务器日志(仅拒绝/错误记录)]\n' + (logs.trim() ? logs : '(最近日志中没有密钥拒绝记录——若刚连接失败,请再试一次密钥登录后重新查看)');
    $('#connMsg').textContent += diag;
  } catch { /* 诊断通道不可用时静默,保留基础提示 */ }
}

/* ---------- main -> renderer 数据 ----------
 * v5(M4) 分屏:数据/断线事件按「连接 id → 窗格」路由,不再只看 tab.id */
function findPaneByConnId(id) {
  for (const t of tabs.values()) {
    if (!t.panes) continue;
    const p = t.panes.find((x) => x.id === id);
    if (p) return { tab: t, pane: p };
  }
  return null;
}
function findTabByPane(pane) {
  for (const t of tabs.values()) if (t.panes && t.panes.includes(pane)) return t;
  return null;
}
window.easyssh.onData((id, data) => {
  const hit = findPaneByConnId(id);
  if (hit) hit.pane.term.write(data);
});
window.easyssh.onClosed((id) => {
  const hit = findPaneByConnId(id) || (tabs.has(id) ? { tab: tabs.get(id), pane: tabs.get(id).panes[tabs.get(id).activePane] } : null);
  if (!hit) return;
  const { tab, pane } = hit;
  if (pane && !pane.disconnected && pane.id === id) {
    pane.disconnected = true;
    // v5 M2:断线瞬间先导出 scrollback 文本,重连成功后写回恢复
    pane._restoreBuf = exportBufferText(pane.term);
    pane._discAt = Date.now();
    stopRetryCountdown(pane);
    clearTimeout(pane._autoTimer);
    // v4.3.0 健壮性:自动重连——2/4/8 秒指数退避,最多 3 次;用户主动关闭不自动重连
    if (pane.opts && !pane.userClosed) {
      pane._autoRetries = 0;
      scheduleAutoReconnect(pane);
      // 若断的是活动窗格,状态栏同步提示;其余窗格不受影响
      if (tab.panes[tab.activePane] === pane) $('#statusBar').textContent = pane.name + ' 已断开 — 自动重连中';
    } else {
      pane.term.write('\r\n\x1b[33m[连接已断开 — 按回车重连]\x1b[0m\r\n');
      if (tab.panes[tab.activePane] === pane) $('#statusBar').textContent = pane.name + ' 已断开 — 按回车重连';
    }
  }
});
// v4.3.0 自动重连调度:失败后指数退避继续,3 次失败或用户操作后停止
// v5 M2:断线期间状态栏倒计时「已断开 — 将在 X 秒后第 N 次重试」(指数退避 2/4/8s)
// v5(M4) 分屏:调度对象从 tab 改为 pane(每个窗格独立重连,互不影响)
function stopRetryCountdown(p) {
  clearInterval(p._cdTimer);
  p._cdTimer = null;
}
function startRetryCountdown(p, delayMs, attempt) {
  stopRetryCountdown(p);
  const endAt = Date.now() + delayMs;
  const tick = () => {
    if (p.userClosed || !p.disconnected) { stopRetryCountdown(p); return; }
    const left = Math.max(0, Math.ceil((endAt - Date.now()) / 1000));
    $('#statusBar').textContent = p.name + ' 已断开 — 将在 ' + left + ' 秒后第 ' + attempt + ' 次重试';
  };
  tick();
  p._cdTimer = setInterval(tick, 500);
}
function scheduleAutoReconnect(p) {
  stopRetryCountdown(p);
  if (p.userClosed || p._autoRetries >= 3) {
    if (!p.userClosed) {
      p.term.write('\r\n\x1b[31m[自动重连失败(已尝试 3 次) — 按回车手动重连]\x1b[0m\r\n');
      $('#statusBar').textContent = p.name + ' 已断开 — 按回车手动重连';
    }
    return;
  }
  const delay = 2000 * Math.pow(2, p._autoRetries);
  p._autoRetries++;
  startRetryCountdown(p, delay, p._autoRetries);
  p.term.write('\r\n\x1b[33m[连接已断开 — ' + (delay / 1000) + ' 秒后自动重连(第 ' + p._autoRetries + '/3 次),按回车立即重试]\x1b[0m\r\n');
  p._autoTimer = setTimeout(async () => {
    stopRetryCountdown(p);
    if (p.userClosed || !p.disconnected) return;
    $('#statusBar').textContent = '重连中 ' + p.opts.host + ' ...';
    const t = findTabByPane(p);
    if (!t) return; // 窗格所属标签已关闭,放弃重连
    const ok = await connectPane(t, p, p.opts);
    if (ok) {
      p._autoRetries = 0;
      restoreSessionBuffer(p); // v5 M2:重连成功恢复断线前 scrollback(同一窗格,不重建)
      p.term.write('\x1b[32m[已自动重连]\x1b[0m\r\n');
      $('#statusBar').textContent = '已连接: ' + (t ? t.name : p.name);
    } else {
      scheduleAutoReconnect(p);
    }
  }, delay);
}

window.addEventListener('resize', () => { const t = tabs.get(activeId); if (t) fitAllPanes(t); });

/* ================= v2.3 密钥登录 ================= */
let generatedPub = null;
$('#btnGenKey').addEventListener('click', async () => {
  $('#keyMsg').textContent = '';
  try {
    const k = await window.easyssh.generateKey();
    generatedPub = k.pubLine;
    $('#keyPrivPath').textContent = k.privPath;
    $('#keyPubPath').textContent = k.pubPath;
    $('#keyPubText').value = k.pubLine;
    $('#keyInfo').style.display = 'flex';
    // 自动把私钥路径填入连接表单并切到私钥认证
    $('#authtype').value = 'key';
    $('#authtype').dispatchEvent(new Event('change'));
    $('#connectForm').privateKey.value = k.privPath;
    $('#statusBar').textContent = '密钥已生成: ' + k.privPath;
  } catch (e) {
    $('#keyMsg').textContent = '生成失败: ' + (e.message || e);
  }
});
$('#btnCopyPub').addEventListener('click', async () => {
  try { await navigator.clipboard.writeText($('#keyPubText').value); $('#keyMsg').textContent = ''; $('#statusBar').textContent = '公钥已复制到剪贴板'; }
  catch { $('#keyPubText').select(); document.execCommand('copy'); $('#statusBar').textContent = '公钥已复制到剪贴板'; }
});
$('#btnInstallKey').addEventListener('click', async () => {
  if (!generatedPub) { $('#keyMsg').textContent = '请先生成密钥对'; return; }
  const t = tabs.get(activeId);
  if (!t || t.disconnected) { $('#keyMsg').textContent = '请先用密码连接一台服务器,再安装公钥'; return; }
  $('#keyMsg').textContent = '';
  try {
    const msg = await window.easyssh.installKey(t.id, generatedPub);
    $('#statusBar').textContent = msg;
    $('#keyMsg').textContent = '';
    alert(msg + '\n现在可以把认证方式切到「私钥文件」登录了(私钥路径已自动生成)。');
  } catch (e) {
    $('#keyMsg').textContent = '安装失败: ' + (e.message || e);
  }
});

/* ================= v5(M4) F-M4-01 分屏交互入口 ================= */
// 工具栏「分屏」下拉菜单
$('#btnSplit').addEventListener('click', (e) => {
  e.stopPropagation();
  const m = $('#splitMenu');
  const r = $('#btnSplit').getBoundingClientRect();
  m.style.left = r.left + 'px';
  m.style.top = (r.bottom + 4) + 'px';
  m.classList.toggle('hidden');
});
document.addEventListener('click', (e) => { if (!e.target.closest('#splitMenu')) $('#splitMenu').classList.add('hidden'); });
$('#splitMenu').addEventListener('click', (e) => {
  const item = e.target.closest('.ctx-item');
  if (!item) return;
  $('#splitMenu').classList.add('hidden');
  const t = tabs.get(activeId);
  if (!t) { $('#statusBar').textContent = '请先连接一个会话,再使用分屏'; return; }
  if (item.dataset.newpane !== undefined) { addSessionPane(t); return; }
  setLayout(t, item.dataset.layout);
});
// 全局快捷键:Ctrl+Shift+D 循环布局;Ctrl+Alt+方向键 移动窗格焦点
document.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey) {
    const k = (e.key || '').toLowerCase();
    if (k === 'd') { e.preventDefault(); cycleLayout(); return; }
  }
  if ((e.ctrlKey || e.metaKey) && e.altKey) {
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') { e.preventDefault(); movePaneFocus(1); return; }
    if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') { e.preventDefault(); movePaneFocus(-1); return; }
  }
});

/* v5(M4):分屏 API 导出(命令面板等模块复用;不与 palette.js 形成加载期循环依赖) */
export { setLayout, cycleLayout, addSessionPane };
