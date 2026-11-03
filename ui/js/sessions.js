/* ============================================================
 * sessions.js — 会话列表（从 renderer.js 提取）
 * 职责：会话列表渲染、分组/搜索/备注/颜色、右键菜单、克隆、
 *       编辑弹窗、新建分组、导入导出、标签颜色联动。
 * ============================================================ */
import { $, escH } from './dom.js';
import { tabs, sessions, pendingSess, setPendingSess, sessKey, sessSecretFields } from './state.js';
import { saveSessionsGuard } from './masterpw.js';

/* ---------- v2.9.0 会话分组 / 搜索 / 备注 / 颜色 / 导入导出 / 克隆 ---------- */
// 空分组持久保存(localStorage,轻量便携;随会话保存的分组名不需要额外存储)
function loadExtraGroups() {
  try { const a = JSON.parse(localStorage.getItem('easyssh.groups') || '[]'); return Array.isArray(a) ? a.filter((g) => typeof g === 'string' && g.trim()) : []; } catch { return []; }
}
function saveExtraGroups(list) { try { localStorage.setItem('easyssh.groups', JSON.stringify(list)); } catch {} }
export function getGroups() {
  const out = [];
  for (const g of loadExtraGroups()) if (!out.includes(g)) out.push(g);
  for (const s of sessions) { const g = (s.group || '').trim(); if (g && !out.includes(g)) out.push(g); }
  return out;
}

let sessionFilter = ''; // 搜索过滤词(纯前端实时过滤)
const collapsedGroups = new Set(); // 已收起的分组(空字符串代表「默认」)
let ctxIndex = -1; // 右键菜单当前指向的会话下标
let editingSessIndex = -1; // 编辑弹窗当前指向的会话下标
let seColor = ''; // 编辑弹窗当前选中的颜色

export function renderSessions() {
  const ul = $('#sessionList');
  ul.innerHTML = '';
  const q = sessionFilter.trim().toLowerCase();
  const match = (s) => !q || [s.host, s.username, s.name, s.note]
    .some((v) => typeof v === 'string' && v.toLowerCase().includes(q));
  // 分组顺序:「默认(未分组)」在最前,其余按自定义分组在前、会话分组随出现的顺序排列
  const groups = [{ id: '', label: '默认' }].concat(getGroups().map((g) => ({ id: g, label: g })));
  for (const grp of groups) {
    const items = sessions.map((s, i) => ({ s, i })).filter(({ s }) => (s.group || '').trim() === grp.id && match(s));
    if (!items.length) continue;
    const collapsed = !q && collapsedGroups.has(grp.id); // 搜索时忽略收起状态,保证结果可见
    const head = document.createElement('li');
    head.className = 'group-head';
    head.style.display = 'flex';
    head.innerHTML = '<span>' + (collapsed ? '▸ ' : '▾ ') + escH(grp.label) + '</span><span class="cnt">(' + items.length + ')</span>';
    // v5(beta) 新增:分组改名/删除操作(内联按钮,点击不触发折叠)
    const acts = document.createElement('span');
    acts.style.cssText = 'margin-left:auto;display:inline-flex;gap:8px;opacity:.0;transition:opacity .15s;cursor:pointer;';
    acts.innerHTML = '<span class="gp-ren" title="改名" style="font-size:12px;">✎</span>'
      + (grp.id ? '<span class="gp-del" title="删除分组(会话移回默认)" style="font-size:12px;">🗑</span>' : '');
    head.appendChild(acts);
    head.addEventListener('mouseenter', () => { acts.style.opacity = '.75'; });
    head.addEventListener('mouseleave', () => { acts.style.opacity = '0'; });
    const renEl = acts.querySelector('.gp-ren');
    if (renEl) renEl.addEventListener('click', (e) => { e.stopPropagation(); openGroupModal('rename', grp.id || '默认'); });
    const delEl = acts.querySelector('.gp-del');
    if (delEl) delEl.addEventListener('click', (e) => { e.stopPropagation(); deleteGroupApply(grp.id); });
    head.addEventListener('click', () => {
      if (collapsedGroups.has(grp.id)) collapsedGroups.delete(grp.id); else collapsedGroups.add(grp.id);
      renderSessions();
    });
    ul.appendChild(head);
    if (collapsed) continue;
    for (const { s, i } of items) ul.appendChild(makeSessionLi(s, i));
  }
  if (!ul.children.length) {
    const li = document.createElement('li');
    li.textContent = q ? '(无匹配的会话)' : '(暂无会话,点击「新建连接」创建)';
    ul.appendChild(li);
  }
}

function makeSessionLi(s, i) {
  const li = document.createElement('li');
  const displayName = s.name || (s.username + '@' + s.host);
  li.innerHTML = '<span class="sess-main">'
    + '<span class="sess-name">' + (s.color ? '<span class="dot" style="background:' + escH(s.color) + '"></span>' : '') + escH(displayName) + '</span>'
    + (s.note ? '<span class="sess-note">' + escH(s.note) + '</span>' : '')
    + '</span><span class="del" title="删除" aria-label="删除会话"><svg class="icon"><use href="#i-trash"/></svg></span>';
  if (s.note) li.title = s.note; // 备注悬停提示
  li.addEventListener('click', (e) => {
    if (e.target.closest('.del')) { // v5(M3):closest 兼容内嵌 SVG 图标
      sessions.splice(i, 1);
      saveSessionsGuard(sessions);
      renderSessions();
      return;
    }
    fillConnectForm(s);
  });
  // 右键菜单:克隆 / 编辑(分组/备注/颜色) / 快速设色 / 删除
  li.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    openCtxMenu(e.clientX, e.clientY, i);
  });
  return li;
}

// 把会话配置回填到连接表单(原 renderSessions 内联逻辑提取)
function fillConnectForm(s) {
  setPendingSess(s); // 记录来源条目,提交时优先按它匹配(克隆副本区分)
  $('#connectPanel').style.display = 'block';
  const form = $('#connectForm');
  form.host.value = s.host; form.port.value = s.port || 22;
  form.username.value = s.username;
  // 按保存时的认证方式恢复表单(默认兼容旧记录:password)
  const at = s.authtype === 'key' && s.privateKey ? 'key' : 'password';
  form.authtype.value = at;
  form.authtype.dispatchEvent(new Event('change'));
  form.password.value = at === 'password' ? (s.password || '') : '';
  form.privateKey.value = at === 'key' ? (s.privateKey || '') : '';
  form.passphrase.value = at === 'key' ? (s.passphrase || '') : '';
  (at === 'password' ? form.password : form.privateKey).focus();
}

/* ---------- 会话右键菜单 ---------- */
export function hideCtxMenu() { $('#ctxMenu').classList.add('hidden'); }
function openCtxMenu(x, y, i) {
  ctxIndex = i;
  const m = $('#ctxMenu');
  m.classList.remove('hidden'); // 先显示才能量取尺寸,用于贴边防溢出
  const r = m.getBoundingClientRect();
  m.style.left = Math.max(4, Math.min(x, window.innerWidth - r.width - 8)) + 'px';
  m.style.top = Math.max(4, Math.min(y, window.innerHeight - r.height - 8)) + 'px';
}
document.addEventListener('click', (e) => { if (!e.target.closest('#ctxMenu')) hideCtxMenu(); });
document.addEventListener('contextmenu', (e) => { if (!e.target.closest('#sessionList li')) hideCtxMenu(); });
$('#ctxMenu').addEventListener('click', (e) => {
  const sw = e.target.closest('.ctx-swatch');
  if (sw && ctxIndex >= 0 && ctxIndex < sessions.length) { // 快速设置标签颜色
    const s = sessions[ctxIndex];
    if (sw.dataset.color) s.color = sw.dataset.color; else delete s.color;
    saveSessionsGuard(sessions);
    renderSessions(); applyTabColors(sessKey(s));
    hideCtxMenu();
    return;
  }
  const item = e.target.closest('.ctx-item');
  if (!item || ctxIndex < 0 || ctxIndex >= sessions.length) return;
  const act = item.dataset.act;
  if (act === 'clone') cloneSession(ctxIndex);
  else if (act === 'edit') openSessEdit(ctxIndex);
  else if (act === 'delete') { sessions.splice(ctxIndex, 1); saveSessionsGuard(sessions); renderSessions(); }
  hideCtxMenu();
});

// v2.9.0 克隆会话:复制完整配置,显示名称加「 - 副本」后缀(密码等配置一并复制)
function cloneSession(i) {
  const s = sessions[i];
  const c = { ...s };
  c.name = (s.name || s.username + '@' + s.host) + ' - 副本';
  sessions.splice(i + 1, 0, c);
  saveSessionsGuard(sessions);
  renderSessions();
  $('#statusBar').textContent = '已克隆会话: ' + c.name;
}

// 把某会话的颜色同步到已打开的同配置标签页(编辑/右键改色后立即生效)
export function applyTabColors(key) {
  for (const t of tabs.values()) {
    if (!t.opts) continue;
    if (t.opts.host + ':' + (t.opts.port || 22) + ':' + t.opts.username !== key) continue;
    const s = sessions.find((x) => sessKey(x) === key);
    if (t.tabEl) { t.tabEl.style.color = (s && s.color) || ''; }
    if (s && s.color && !t.color) t.color = s.color;
  }
}

/* ---------- 会话编辑弹窗(分组 / 备注 / 颜色) ---------- */
function markSeColors() {
  document.querySelectorAll('#seColors .ctx-swatch').forEach((el) => {
    el.style.outline = ((el.dataset.color || '') === seColor) ? '2px solid var(--color-text-primary)' : 'none';
  });
}
function openSessEdit(i) {
  editingSessIndex = i;
  const s = sessions[i];
  $('#sessEditTitle').textContent = s.name || (s.username + '@' + s.host);
  const sel = $('#seGroup');
  const opts = ['', ...getGroups()];
  const cur = (s.group || '').trim();
  if (cur && !opts.includes(cur)) opts.push(cur);
  sel.innerHTML = '';
  opts.forEach((g) => {
    const o = document.createElement('option');
    o.value = g; o.textContent = g || '默认(未分组)';
    sel.appendChild(o);
  });
  sel.value = cur;
  $('#seNote').value = s.note || '';
  seColor = s.color || '';
  markSeColors();
  $('#sessEditModal').classList.remove('hidden');
}
$('#seColors').addEventListener('click', (e) => {
  const sw = e.target.closest('.ctx-swatch');
  if (!sw) return;
  seColor = sw.dataset.color || '';
  markSeColors();
});
$('#seSave').addEventListener('click', () => {
  if (editingSessIndex < 0 || editingSessIndex >= sessions.length) { $('#sessEditModal').classList.add('hidden'); return; }
  const s = sessions[editingSessIndex];
  const g = $('#seGroup').value.trim();
  const note = $('#seNote').value.trim();
  if (g) s.group = g; else delete s.group;
  if (note) s.note = note; else delete s.note;
  if (seColor) s.color = seColor; else delete s.color;
  saveSessionsGuard(sessions);
  renderSessions();
  applyTabColors(sessKey(s));
  $('#sessEditModal').classList.add('hidden');
  $('#statusBar').textContent = '会话「' + (s.name || s.username + '@' + s.host) + '」已更新';
});
$('#seCancel').addEventListener('click', () => $('#sessEditModal').classList.add('hidden'));

/* ---------- 分组管理(新建/改名/删除) ---------- */
// v5(beta) 新增:分组改名与删除。存储:自定义分组在 localStorage,会话分组名在 sessions[].group。
let groupModalMode = 'create';   // 'create' | 'rename'
let groupModalFrom = '';         // 改名模式的原分组名('默认' = 未分组哨兵)
function openGroupModal(mode, fromName) {
  groupModalMode = mode;
  groupModalFrom = fromName || '';
  $('#groupName').value = mode === 'rename' ? (fromName === '默认' ? '' : fromName) : '';
  $('#groupModal').classList.remove('hidden');
  $('#groupName').focus();
}
function renameGroupApply(fromName, newName) {
  // 默认(未分组哨兵)改名 = 把所有未分组会话移入新组并将新组登记为自定义分组
  let moved = 0;
  for (const s of sessions) { if ((s.group || '').trim() === (fromName === '默认' ? '' : fromName)) { s.group = newName === '默认' ? '' : newName; moved++; } }
  const extras = loadExtraGroups();
  const ei = extras.indexOf(fromName);
  if (fromName !== '默认' && ei >= 0) {
    if (newName === '默认') extras.splice(ei, 1); else extras[ei] = newName;
    saveExtraGroups(extras);
  } else if (fromName === '默认' && newName !== '默认' && !extras.includes(newName)) {
    extras.push(newName);
    saveExtraGroups(extras);
  }
  renderSessions();
  saveSessionsGuard(sessions);
  $('#statusBar').textContent = '分组已改名「' + fromName + '」→「' + newName + '」' + (moved ? '(含 ' + moved + ' 个会话)' : '');
}
function deleteGroupApply(name) {
  let moved = 0;
  for (const s of sessions) { if ((s.group || '').trim() === name) { s.group = ''; moved++; } }
  const extras = loadExtraGroups();
  const ei = extras.indexOf(name);
  if (ei >= 0) { extras.splice(ei, 1); saveExtraGroups(extras); }
  renderSessions();
  saveSessionsGuard(sessions);
  $('#statusBar').textContent = '分组「' + name + '」已删除' + (moved ? ',其中 ' + moved + ' 个会话已移回默认' : '');
}
$('#btnNewGroup').addEventListener('click', () => openGroupModal('create'));
$('#groupSave').addEventListener('click', () => {
  const name = $('#groupName').value.trim();
  if (!name) { $('#statusBar').textContent = '请输入分组名称'; return; }
  if (groupModalMode === 'rename') {
    const from = groupModalFrom;
    if (name === from) { $('#groupModal').classList.add('hidden'); return; }
    if (from !== '默认' && getGroups().includes(name) && name !== '默认') { $('#statusBar').textContent = '分组「' + name + '」已存在'; return; }
    $('#groupModal').classList.add('hidden');
    renameGroupApply(from, name);
    return;
  }
  if (getGroups().includes(name)) { $('#statusBar').textContent = '分组「' + name + '」已存在'; return; }
  const extras = loadExtraGroups();
  extras.push(name);
  saveExtraGroups(extras);
  $('#groupModal').classList.add('hidden');
  renderSessions();
  $('#statusBar').textContent = '分组「' + name + '」已创建,右键会话「编辑备注/分组」即可移入';
});
$('#groupName').addEventListener('keydown', (e) => { if (e.key === 'Enter') $('#groupSave').click(); });

/* ---------- 会话搜索(纯前端实时过滤) ---------- */
$('#sessionSearch').addEventListener('input', (e) => {
  sessionFilter = e.target.value;
  renderSessions();
});

/* ---------- 会话导出 / 导入 ---------- */
$('#btnSessExport').addEventListener('click', async () => {
  if (!sessions.length) { $('#statusBar').textContent = '暂无可导出的会话'; return; }
  // 脱敏:密码/口令(含密文字段)一律不写入导出文件
  const data = {
    app: 'EasySSH',
    kind: 'sessions',
    exportedAt: new Date().toISOString(),
    sessions: sessions.map((s) => { const o = { ...s }; for (const f of sessSecretFields) delete o[f]; return o; }),
  };
  try {
    const p = await window.easyssh.sessionsExport('easyssh-sessions.json', data);
    if (p) $('#statusBar').textContent = '已导出 ' + data.sessions.length + ' 个会话(不含密码): ' + p;
  } catch (e) { $('#statusBar').textContent = '导出失败: ' + (e.message || e); }
});
$('#btnSessImport').addEventListener('click', async () => {
  let res;
  try { res = await window.easyssh.sessionsImport(); } catch (e) { $('#statusBar').textContent = '导入失败: ' + (e.message || e); return; }
  if (!res) return; // 用户取消选择
  if (!res.ok) { $('#statusBar').textContent = res.message || '导入失败'; return; }
  const data = res.data;
  // 兼容两种格式:EasySSH 导出 JSON(顶层 sessions 数组)或纯会话数组
  const list = Array.isArray(data) ? data : (data && Array.isArray(data.sessions) ? data.sessions : null);
  if (!list) { $('#statusBar').textContent = '导入失败:文件格式不正确(需要 EasySSH 导出的会话 JSON)'; return; }
  const exist = new Set(sessions.map(sessKey));
  let added = 0, skipped = 0;
  for (const it of list) {
    if (!it || typeof it !== 'object' || !it.host || !it.username) { skipped++; continue; }
    const k = sessKey(it); // host+port+username 相同视为重复
    if (exist.has(k)) { skipped++; continue; }
    const rec = { ...it };
    for (const f of sessSecretFields) delete rec[f]; // 导入的会话一律不含密码,连接时重新输入
    sessions.push(rec);
    exist.add(k);
    added++;
  }
  await saveSessionsGuard(sessions);
  renderSessions();
  $('#statusBar').textContent = '导入完成:新增 ' + added + ' 个,跳过重复/无效 ' + skipped + ' 个(导入的会话不含密码,连接时需重新输入)';
});
