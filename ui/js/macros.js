/* ============================================================
 * macros.js — v5.2.x 第二批:宏序列(命令序列自动化)
 * 职责:宏的持久化(localStorage easyssh.macros)、管理模态
 *       (#macroModal 列表+步骤编辑器)、运行引擎(向当前活动窗格
 *       逐步发送命令,支持步骤间延时与「等待输出匹配正则」)。
 * 依赖:tauri-bridge.js 先于本模块加载(提供 window.easyssh);
 *       state.js 的 tabs/activeId(ESM live binding)。
 * 注意:独立于 shortcuts.js 的快捷命令顺序宏,不改动其任何代码;
 *       不注册新全局快捷键(shortcuts.js 属禁区)。
 * ============================================================ */
import { $ } from './dom.js';
import { tabs, activeId } from './state.js';

const LS_KEY = 'easyssh.macros';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/* ---------- 持久化 ---------- */
export function loadMacros() {
  try {
    const arr = JSON.parse(localStorage.getItem(LS_KEY) || '[]');
    return Array.isArray(arr) ? arr.filter((m) => m && m.id && m.name) : [];
  } catch { return []; }
}
function saveMacros(list) {
  try { localStorage.setItem(LS_KEY, JSON.stringify(list)); } catch {}
}

/* ---------- 运行引擎状态机 ----------
 * IDLE --runMacro(校验通过)--> RUNNING
 * RUNNING:对每个步骤 i:
 *   1) 校验窗格(pane.id===connId 且 !disconnected 且 !mirror)→ 失败 → ABORT(会话断开)
 *   2) write(connId, cmd+'\r')
 *      注:桥的 write() 是 invoke('ssh_write').catch(()=>{}),失败被吞掉,
 *          故"写失败"以步骤前/等待期间的窗格状态复查(disconnected/连接id 变化)为准。
 *   3) 无 waitFor → WAIT-DELAY(delayMs,可被停止打断) → 下一步
 *   4) 有 waitFor → LISTENING:
 *      注册 window.easyssh.onData(cb) 按连接id 过滤累计输出做正则检测。
 *      选 onData 而非包装 pane.term.write 的理由:tauri-bridge.js L172
 *      onData = (cb) => listen('ssh-data', …),Tauri event listen 每次调用
 *      注册一个新监听器并返回 unlisten 函数,可多次注册/精确注销,无副作用;
 *      而包装 term.write 需保存/还原引用,且与终端渲染路径耦合,泄漏风险更高。
 *      匹配 → finish(true) 立即下一步;超时(waitTimeoutMs,默认 10000) → ABORT。
 *   5) 任何时刻 stopReq(停止宏) → ABORT(用户停止)
 * ABORT/DONE → runState=null,注销监听/清除轮询 → IDLE。
 * 防重复:运行中同一窗格重复触发直接拒绝;其它窗格触发提示「先停止」。 */
let runState = null; // { name, connId, pane, stopReq, idx }

function status(text) { const el = $('#statusBar'); if (el) el.textContent = text; }

/* 窗格有效性复查(步骤前与等待期间):连接id 未变、未断开、非镜像 */
function paneStillValid(st) {
  const p = st.pane;
  return !!(p && p.id === st.connId && !p.disconnected && !p.mirror);
}

/* 可打断的延时(delayMs):每 50ms 检查一次 stopReq */
function sleepStoppable(ms, st) {
  return new Promise((resolve) => {
    const t0 = Date.now();
    const iv = setInterval(() => {
      if (st.stopReq || Date.now() - t0 >= ms) { clearInterval(iv); resolve(); }
    }, 50);
  });
}

/* 等待输出匹配:返回 'match' | 'timeout' | 'stop'。
 * 用 Promise.resolve(unlistenP) 统一注销(注册是异步返回 unlisten 的),
 * finally 保证 clearTimeout/clearInterval,绝不泄漏监听器。 */
function waitOutputMatch(connId, reStr, timeoutMs, st) {
  return new Promise((resolve) => {
    let buf = '';
    let done = false;
    let unlistenP = null;
    let timer = null;
    let poll = null;
    const finish = (result) => {
      if (done) return;
      done = true;
      if (timer) clearTimeout(timer);
      if (poll) clearInterval(poll);
      // 注销监听:注册可能尚未返回 unlisten,故 resolve 后再调用
      if (unlistenP) Promise.resolve(unlistenP).then((u) => { if (typeof u === 'function') u(); }).catch(() => {});
      resolve(result);
    };
    try {
      unlistenP = window.easyssh.onData((id, data) => {
        if (done || id !== connId) return;
        buf += String(data || '');
        if (buf.length > 65536) buf = buf.slice(-32768); // 防长输出撑爆内存
        try { if (new RegExp(reStr).test(buf)) finish('match'); } catch {}
      });
    } catch {
      finish('timeout');
      return;
    }
    timer = setTimeout(() => finish('timeout'), Math.max(1000, timeoutMs || 10000));
    // 停止请求轮询(onData 回调不会主动触发 stop)
    poll = setInterval(() => { if (st.stopReq || !paneStillValid(st)) finish(st.stopReq ? 'stop' : 'timeout'); }, 120);
  });
}

/* 运行宏(入口):校验目标窗格 → 逐步执行 */
export async function runMacro(m) {
  const t = tabs.get(activeId);
  const pane = (t && Array.isArray(t.panes)) ? t.panes[Math.min(t.activePane, t.panes.length - 1)] : null;
  if (!pane || !pane.id) { status('请先连接一个会话再运行宏'); return; }
  if (pane.disconnected) { status('当前连接已断开,按回车重连后再运行宏'); return; }
  if (pane.mirror) { status('只读镜像窗格不可运行宏'); return; }
  if (runState) {
    status(runState.connId === pane.id ? '已有宏在该窗格运行中,请先停止' : '已有宏正在运行,请先停止');
    return;
  }
  const steps = (m.steps || []).filter((s) => s && String(s.cmd || '').trim());
  if (!steps.length) { status('宏没有可执行的步骤'); return; }
  const st = { name: m.name, connId: pane.id, pane, stopReq: false, idx: 0 };
  runState = st;
  syncStopBtn();
  let outcome = 'done'; // done | stopped | timeout@N
  for (let i = 0; i < steps.length; i++) {
    if (st.stopReq) { outcome = 'stopped'; break; }
    const s = steps[i];
    st.idx = i + 1;
    status('宏运行中: ' + m.name + ' (步骤 ' + (i + 1) + '/' + steps.length + ')');
    if (!paneStillValid(st)) { outcome = 'disconnected'; break; }
    try {
      // 发送命令:回车执行;write 失败被桥吞掉,断开由 paneStillValid 兜底
      await window.easyssh.write(st.connId, String(s.cmd) + '\r');
    } catch {
      outcome = 'disconnected';
      break;
    }
    const waitRe = String(s.waitFor || '').trim();
    if (waitRe) {
      let reOk = true;
      try { new RegExp(waitRe); } catch { reOk = false; }
      if (!reOk) { status('宏已中止: ' + m.name + '(第 ' + (i + 1) + ' 步的正则无效)'); runState = null; syncStopBtn(); return; }
      const r = await waitOutputMatch(st.connId, waitRe, Number(s.waitTimeoutMs) > 0 ? Number(s.waitTimeoutMs) : 10000, st);
      if (r === 'timeout') {
        // 等待期间窗格已断开 → 归因为会话断开而非超时
        outcome = paneStillValid(st) ? 'timeout@' + (i + 1) : 'disconnected';
        break;
      }
      if (r === 'stop') { outcome = 'stopped'; break; }
      // 'match' → 立即下一步(无额外延时)
    } else if (Number(s.delayMs) > 0) {
      await sleepStoppable(Number(s.delayMs), st);
    }
  }
  runState = null;
  syncStopBtn();
  if (outcome === 'stopped') status('宏已停止: ' + m.name);
  else if (String(outcome).startsWith('timeout@')) status('宏在第 ' + String(outcome).slice('timeout@'.length) + ' 步等待输出超时: ' + m.name);
  else if (outcome === 'disconnected') status('宏已中止: ' + m.name + '(会话断开)');
  else status('宏执行完成: ' + m.name);
}

export function stopMacro() {
  if (runState) runState.stopReq = true;
}

/* 运行期间模态内「停止宏」按钮可见性 */
function syncStopBtn() {
  const b = $('#btnMacroStop');
  if (b) b.classList.toggle('hidden', !runState);
}

/* ---------- 管理模态 ---------- */
let editingId = null;   // null=新增,否则正在编辑的宏 id
let editorSteps = [];   // 编辑器中的步骤(深拷贝,关闭不落盘)
let editorDirty = false;

function editorOpen() { return !$('#macroEditor').classList.contains('hidden'); }

function renderMacroList() {
  const ul = $('#macroList');
  ul.innerHTML = '';
  const list = loadMacros();
  if (!list.length) {
    const li = document.createElement('li');
    li.className = 'macro-empty';
    li.textContent = '暂无宏,点「新建宏」创建';
    ul.appendChild(li);
  }
  list.forEach((m) => {
    const li = document.createElement('li');
    li.className = 'macro-item';
    const name = document.createElement('b');
    name.textContent = m.name; // textContent 防注入
    const meta = document.createElement('span');
    meta.className = 'macro-meta';
    meta.textContent = ' (' + (m.steps || []).length + ' 步)';
    const ops = document.createElement('span');
    ops.className = 'macro-ops';
    const mkBtn = (label, fn) => {
      const b = document.createElement('button');
      b.textContent = label;
      b.addEventListener('click', fn);
      return b;
    };
    ops.appendChild(mkBtn('运行', () => runMacro(m)));
    ops.appendChild(mkBtn('编辑', () => openEditor(m)));
    ops.appendChild(mkBtn('删除', () => {
      const list2 = loadMacros().filter((x) => x.id !== m.id);
      saveMacros(list2);
      renderMacroList();
    }));
    li.appendChild(name);
    li.appendChild(meta);
    li.appendChild(ops);
    ul.appendChild(li);
  });
  syncStopBtn();
}

function stepRow(s, idx) {
  const div = document.createElement('div');
  div.className = 'macro-step';
  // 头部:序号 + 上移/下移/删除
  const head = document.createElement('div');
  head.className = 'macro-step-head';
  const idxEl = document.createElement('span');
  idxEl.className = 'ms-idx';
  idxEl.textContent = String(idx + 1);
  const mkHead = (label, fn) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = label;
    b.addEventListener('click', fn);
    return b;
  };
  head.appendChild(idxEl);
  head.appendChild(mkHead('↑', () => { if (idx > 0) { [editorSteps[idx - 1], editorSteps[idx]] = [editorSteps[idx], editorSteps[idx - 1]]; rerenderSteps(); } }));
  head.appendChild(mkHead('↓', () => { if (idx < editorSteps.length - 1) { [editorSteps[idx + 1], editorSteps[idx]] = [editorSteps[idx], editorSteps[idx + 1]]; rerenderSteps(); } }));
  head.appendChild(mkHead('✕', () => { editorSteps.splice(idx, 1); rerenderSteps(); }));
  div.appendChild(head);
  // 命令 textarea
  const ta = document.createElement('textarea');
  ta.className = 'ms-cmd';
  ta.rows = 2;
  ta.placeholder = '命令,如:cd /opt/app && ./deploy.sh';
  ta.value = s.cmd || '';
  ta.addEventListener('input', () => { editorDirty = true; });
  div.appendChild(ta);
  // 选项行:延时 / 等待正则 / 超时
  const opts = document.createElement('div');
  opts.className = 'macro-step-opts';
  const mkOpt = (labelText, cls, type, ph, val) => {
    const lb = document.createElement('label');
    lb.className = 'ms-opt';
    const sp = document.createElement('span');
    sp.textContent = labelText;
    lb.appendChild(sp);
    const inp = document.createElement('input');
    inp.className = cls;
    if (type) inp.type = type;
    inp.placeholder = ph;
    if (val !== undefined && val !== null && val !== '') inp.value = String(val);
    inp.addEventListener('input', () => { editorDirty = true; });
    lb.appendChild(inp);
    return lb;
  };
  opts.appendChild(mkOpt('延时ms', 'ms-delay', 'number', '0', s.delayMs));
  opts.appendChild(mkOpt('等待输出正则', 'ms-wait', 'text', '可选,如:.*deploy done', s.waitFor));
  opts.appendChild(mkOpt('超时ms', 'ms-timeout', 'number', '10000', s.waitTimeoutMs));
  div.appendChild(opts);
  return div;
}

function rerenderSteps() {
  const box = $('#macroSteps');
  box.innerHTML = '';
  editorSteps.forEach((s, i) => box.appendChild(stepRow(s, i)));
  if (!editorSteps.length) {
    const tip = document.createElement('div');
    tip.className = 'macro-empty';
    tip.textContent = '还没有步骤,点「添加步骤」';
    box.appendChild(tip);
  }
}

function openEditor(m) {
  editingId = m ? m.id : null;
  editorSteps = m ? JSON.parse(JSON.stringify(m.steps || [])) : [{ cmd: '', delayMs: 0 }];
  editorDirty = false;
  $('#macroName').value = m ? m.name : '';
  $('#macroEditorTitle').textContent = m ? '编辑宏: ' + m.name : '新建宏';
  $('#macroListView').classList.add('hidden');
  $('#macroEditor').classList.remove('hidden');
  $('#macroMsg').textContent = '';
  rerenderSteps();
  setTimeout(() => $('#macroName').focus(), 50);
}

function closeEditorToList() {
  editingId = null;
  editorSteps = [];
  editorDirty = false;
  $('#macroEditor').classList.add('hidden');
  $('#macroListView').classList.remove('hidden');
  renderMacroList();
}

/* 读取编辑器当前步骤(从 DOM 收集,保证与界面一致) */
function collectSteps() {
  const rows = $('#macroSteps').querySelectorAll('.macro-step');
  const out = [];
  rows.forEach((row) => {
    const cmd = row.querySelector('.ms-cmd').value.trim();
    if (!cmd) return; // 空命令步骤丢弃
    const delay = Number(row.querySelector('.ms-delay').value);
    const wait = row.querySelector('.ms-wait').value.trim();
    const timeout = Number(row.querySelector('.ms-timeout').value);
    const st = { cmd, delayMs: delay > 0 ? delay : 0 };
    if (wait) { st.waitFor = wait; st.waitTimeoutMs = timeout > 0 ? timeout : 10000; }
    out.push(st);
  });
  return out;
}

$('#btnMacro').addEventListener('click', () => {
  closeEditorToList(); // 每次打开回到列表视图
  $('#macroModal').classList.remove('hidden');
});
$('#btnMacroNew').addEventListener('click', () => openEditor(null));
$('#macroCancelEdit').addEventListener('click', () => { editorDirty = false; closeEditorToList(); });
$('#macroAddStep').addEventListener('click', () => { editorSteps.push({ cmd: '', delayMs: 0 }); rerenderSteps(); });
$('#btnMacroStop').addEventListener('click', stopMacro);
$('#macroSave').addEventListener('click', () => {
  const name = $('#macroName').value.trim();
  const steps = collectSteps();
  const msg = $('#macroMsg');
  if (!name) { msg.textContent = '请输入宏名称'; return; }
  if (!steps.length) { msg.textContent = '宏没有可执行的步骤'; return; }
  const list = loadMacros();
  if (editingId) {
    const i = list.findIndex((x) => x.id === editingId);
    if (i >= 0) list[i] = { ...list[i], name, steps };
    else list.push({ id: editingId, name, steps, createdAt: Date.now() });
  } else {
    list.push({ id: 'm' + Date.now().toString(36) + Math.random().toString(36).slice(2, 6), name, steps, createdAt: Date.now() });
  }
  saveMacros(list);
  editorDirty = false;
  closeEditorToList();
});

/* 关闭保护:编辑中有未保存改动时,拦截关闭(遮罩/✕)先确认。
 * 用捕获阶段监听 main.js 的通用关闭逻辑(bubble 阶段),stopPropagation
 * 即可让通用逻辑不执行;确认后再手动 hidden。 */
$('#macroModal').addEventListener('click', async (e) => {
  const isX = !!(e.target.closest && e.target.closest('.close-x'));
  const isOverlay = e.target === $('#macroModal');
  if (!isX && !isOverlay) return;
  if (!editorOpen() || !editorDirty) return;
  e.stopPropagation();
  e.preventDefault();
  const ok = await window.easysshUi.confirm('有未保存的宏编辑内容,关闭后将丢弃,确定关闭?');
  if (ok) closeEditorToList();
}, true);
