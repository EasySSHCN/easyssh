/* ============================================================
 * shortcuts.js — 快捷命令 / 脚本库宏(从 renderer.js 提取)
 * 职责：快捷命令栏渲染(点击发送命令到当前终端)、
 *       快捷命令管理弹窗(增/删/改)、
 *       v5(M4) F-M4-04 脚本库宏:
 *       - 变量占位符:{{host}}/{{port}}/{{user}} 取当前会话连接参数;
 *         {{input:提示文字}} 运行时弹输入框(记住上次值,localStorage);
 *       - 顺序宏:cmd 多行(每行一条命令)按序执行,间隔可配(qc.interval,
 *         默认 300ms);旧单命令数据完全兼容(单行 = 长度为 1 的宏)。
 * ============================================================ */
import { $ } from './dom.js';
import { tabs, activeId, appSettings, setAppSettings, DEFAULT_QUICK_CMDS } from './state.js';

/* v5.1.4:快捷命令栏在 i18n 跳过区(#quickbar)内,MutationObserver 不翻译。
 * 对「未经用户修改的默认快捷命令」按当前界面语言显示译名;
 * 用户自定义/改名的命令(含中文)原样显示,绝不误伤用户数据。 */
function qcDisplayName(name) {
  const lang = appSettings.lang || 'zh-CN';
  if (lang === 'zh-CN' || !window.EasySSH_I18N) return name;
  const isDefault = (DEFAULT_QUICK_CMDS || []).some((d) => d.name === name);
  if (!isDefault) return name;
  const t = window.EasySSH_I18N.t(lang, name);
  return t === name ? name : t;
}

/* ---------- 宏解析与执行 ---------- */
const MACRO_VALS_KEY = 'easyssh.macroVals'; // {{input:xxx}} 记住上次值(轻量 localStorage)

function loadMacroVals() {
  try { const o = JSON.parse(localStorage.getItem(MACRO_VALS_KEY) || '{}'); return o && typeof o === 'object' ? o : {}; } catch { return {}; }
}
function saveMacroVals(o) { try { localStorage.setItem(MACRO_VALS_KEY, JSON.stringify(o)); } catch {} }
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/* 解析一行命令中的占位符。用户取消输入返回 null(整个宏中止)。
 * {{host}}/{{port}}/{{user}} 来自活动窗格的连接参数(tab.opts);
 * 未知占位符原样保留,不做替换。 */
async function resolveMacroLine(line, tab) {
  const o = tab.opts || {};
  const vals = loadMacroVals();
  const re = /\{\{\s*([^}]+?)\s*\}\}/g;
  let out = '';
  let last = 0;
  let m;
  while ((m = re.exec(line))) {
    const raw = m[1];
    let v;
    if (raw === 'host') v = o.host || '';
    else if (raw === 'port') v = String(o.port || 22);
    else if (raw === 'user') v = o.username || '';
    else if (raw.startsWith('input:')) {
      const key = raw.slice(6).trim();
      v = await window.easysshUi.prompt(key, vals[key] || '');
      if (v === null) return null; // 用户取消 → 中止整条宏
      vals[key] = v;
      saveMacroVals(vals);
    } else {
      v = m[0]; // 未知占位符原样保留
    }
    out += line.slice(last, m.index) + v;
    last = m.index + m[0].length;
  }
  return out + line.slice(last);
}

/* 执行一条快捷命令/宏:多行 cmd 按序发送,行间隔 qc.interval(默认 300ms)。
 * 导出给命令面板(palette.js)复用。 */
export async function runQuickCommand(qc) {
  const t = tabs.get(activeId);
  if (!t) { $('#statusBar').textContent = '请先连接一个会话再使用快捷命令'; return; }
  if (t.disconnected) { $('#statusBar').textContent = '当前连接已断开,按回车重连后再用快捷命令'; return; }
  const lines = String(qc.cmd || '').split('\n').map((s) => s.trim()).filter(Boolean);
  if (!lines.length) return;
  const interval = Number(qc.interval) > 0 ? Number(qc.interval) : 300;
  for (let i = 0; i < lines.length; i++) {
    const resolved = await resolveMacroLine(lines[i], t);
    if (resolved === null) { $('#statusBar').textContent = '宏已取消(参数输入被取消)'; return; }
    window.easyssh.write(t.id, resolved + '\r');
    if (lines.length > 1) {
      $('#statusBar').textContent = '宏执行中: 第 ' + (i + 1) + '/' + lines.length + ' 条';
      if (i < lines.length - 1 && interval > 0) await sleep(interval);
    }
  }
  if (lines.length > 1) $('#statusBar').textContent = '宏执行完成(' + lines.length + ' 条)';
}

function renderQuickbar() {
  const bar = $('#quickbar');
  bar.innerHTML = '';
  (appSettings.quickCommands || []).forEach((qc) => {
    const b = document.createElement('button');
    const lines = String(qc.cmd || '').split('\n').filter((s) => s.trim());
    const suffix = lines.length > 1
      ? ((appSettings.lang || 'zh-CN') === 'zh-CN' ? ' (' + lines.length + '条)' : ' (' + lines.length + ')')
      : '';
    b.textContent = qcDisplayName(qc.name) + suffix;
    b.title = qc.cmd;
    b.addEventListener('click', () => runQuickCommand(qc));
    bar.appendChild(b);
  });
}
export { renderQuickbar };

/* ---------- 快捷命令管理 ---------- */
$('#btnQuick').addEventListener('click', async () => {
  // 与原实现一致:每次打开管理弹窗先从磁盘重载设置(可能被其它入口更新过)
  setAppSettings(await window.easyssh.loadSettings());
  resetQuickEditor();
  renderQuickList();
  $('#quickModal').classList.remove('hidden');
});
let editingQuickIndex = -1; // -1 表示新增模式,>=0 表示正在编辑的下标
function resetQuickEditor() {
  editingQuickIndex = -1;
  $('#quickName').value = ''; $('#quickCmd').value = ''; $('#quickInterval').value = '';
  $('#quickAdd').textContent = '添加';
}
function renderQuickList() {
  const ul = $('#quickList');
  ul.innerHTML = '';
  (appSettings.quickCommands || []).forEach((qc, i) => {
    const li = document.createElement('li');
    const lines = String(qc.cmd || '').split('\n').filter((s) => s.trim());
    const preview = lines.length > 1
      ? lines[0] + ' ⏎ …共 ' + lines.length + ' 条,间隔 ' + (Number(qc.interval) > 0 ? Number(qc.interval) : 300) + 'ms'
      : qc.cmd;
    li.innerHTML = '<b>' + qc.name + '</b><span class="qc"></span><span class="edit" title="编辑"><svg class="icon"><use href="#i-edit"/></svg></span><span class="del" title="删除"><svg class="icon"><use href="#i-x"/></svg></span>';
    li.querySelector('.qc').textContent = preview; // textContent 防注入(命令含 {{}}/< 等)
    li.querySelector('.edit').addEventListener('click', () => {
      editingQuickIndex = i;
      $('#quickName').value = qc.name;
      $('#quickCmd').value = qc.cmd;
      $('#quickInterval').value = Number(qc.interval) > 0 ? Number(qc.interval) : '';
      $('#quickAdd').textContent = '保存';
      $('#quickName').focus();
    });
    li.querySelector('.del').addEventListener('click', async () => {
      if (editingQuickIndex === i) resetQuickEditor();
      appSettings.quickCommands.splice(i, 1);
      await window.easyssh.saveSettings(appSettings);
      renderQuickList(); renderQuickbar();
    });
    ul.appendChild(li);
  });
}
$('#quickAdd').addEventListener('click', async () => {
  const name = $('#quickName').value.trim(), cmd = $('#quickCmd').value.trim();
  if (!name || !cmd) return;
  const interval = Number($('#quickInterval').value) > 0 ? Number($('#quickInterval').value) : undefined;
  const rec = interval ? { name, cmd, interval } : { name, cmd }; // interval 缺省时不写字段(兼容旧数据)
  if (editingQuickIndex >= 0 && editingQuickIndex < appSettings.quickCommands.length) {
    appSettings.quickCommands[editingQuickIndex] = rec;
  } else {
    appSettings.quickCommands.push(rec);
  }
  await window.easyssh.saveSettings(appSettings);
  resetQuickEditor();
  renderQuickList(); renderQuickbar();
});
$('#quickName').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('#quickCmd').focus(); });
// v5(M4):quickCmd 改为 textarea 多行编辑(每行一条命令构成顺序宏),不再回车提交
