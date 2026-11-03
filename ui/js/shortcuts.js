/* ============================================================
 * shortcuts.js — 快捷命令（从 renderer.js 提取）
 * 职责：快捷命令栏渲染（点击发送命令到当前终端）、
 *       快捷命令管理弹窗（增/删/改）。
 * ============================================================ */
import { $ } from './dom.js';
import { tabs, activeId, appSettings, setAppSettings } from './state.js';

function renderQuickbar() {
  const bar = $('#quickbar');
  bar.innerHTML = '';
  (appSettings.quickCommands || []).forEach((qc) => {
    const b = document.createElement('button');
    b.textContent = qc.name;
    b.title = qc.cmd;
    b.addEventListener('click', () => {
      const t = tabs.get(activeId);
      if (!t) return;
      if (t.disconnected) { $('#statusBar').textContent = '当前连接已断开,按回车重连后再用快捷命令'; return; }
      window.easyssh.write(t.id, qc.cmd + '\r');
    });
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
  $('#quickName').value = ''; $('#quickCmd').value = '';
  $('#quickAdd').textContent = '添加';
}
function renderQuickList() {
  const ul = $('#quickList');
  ul.innerHTML = '';
  (appSettings.quickCommands || []).forEach((qc, i) => {
    const li = document.createElement('li');
    li.innerHTML = '<b>' + qc.name + '</b><span class="qc">' + qc.cmd + '</span><span class="edit" title="编辑"><svg class="icon"><use href="#i-edit"/></svg></span><span class="del" title="删除"><svg class="icon"><use href="#i-x"/></svg></span>';
    li.querySelector('.edit').addEventListener('click', () => {
      editingQuickIndex = i;
      $('#quickName').value = qc.name;
      $('#quickCmd').value = qc.cmd;
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
  if (editingQuickIndex >= 0 && editingQuickIndex < appSettings.quickCommands.length) {
    appSettings.quickCommands[editingQuickIndex] = { name, cmd };
  } else {
    appSettings.quickCommands.push({ name, cmd });
  }
  await window.easyssh.saveSettings(appSettings);
  resetQuickEditor();
  renderQuickList(); renderQuickbar();
});
$('#quickName').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('#quickAdd').click(); });
$('#quickCmd').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('#quickAdd').click(); });
