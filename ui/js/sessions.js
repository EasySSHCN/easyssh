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
  // v5.0.7 兼容:旧桥接可能直接返回文件字符串(无 ok/data 包装)
  if (typeof res === 'string') {
    try { res = { ok: true, data: JSON.parse(res.replace(/^\uFEFF/, '')) }; }
    catch (e) { $('#statusBar').textContent = '导入失败:文件不是有效的 JSON'; return; }
  }
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

/* ---------- v5.0.8/R-003 外部会话导入(Xshell / PuTTY / Termius / OpenSSH config) ---------- */
const EXT_GROUP = { xshell: 'Xshell', putty: 'PuTTY', termius: 'Termius', sshconfig: 'SSH config' };

$('#btnSessImportExt').addEventListener('click', () => {
  $('#importExtModal').classList.remove('hidden');
});
$('#importExtModal').addEventListener('click', (e) => {
  const item = e.target.closest('[data-extsrc]');
  if (!item) return;
  $('#importExtModal').classList.add('hidden');
  importExternal(item.dataset.extsrc);
});

// 把解析出的条目并入会话列表(与 JSON 导入同一套去重/脱敏规则)
async function mergeExternal(list, groupLabel) {
  const exist = new Set(sessions.map(sessKey));
  let added = 0, skipped = 0;
  for (const it of list) {
    if (!it || !it.host) { skipped++; continue; }
    const rec = {
      name: it.name || it.username + '@' + it.host,
      host: String(it.host), port: Number(it.port) || 22,
      username: it.username || 'root', authtype: 'password',
      ...(groupLabel ? { group: groupLabel } : {}),
    };
    if (exist.has(sessKey(rec))) { skipped++; continue; }
    sessions.push(rec);
    exist.add(sessKey(rec));
    added++;
  }
  await saveSessionsGuard(sessions);
  renderSessions();
  $('#statusBar').textContent = '导入完成(' + groupLabel + '):新增 ' + added + ' 个,跳过重复/无效 ' + skipped + ' 个(不含密码,连接时需重新输入)';
  if (!added && skipped) $('#statusBar').textContent = '导入完成(' + groupLabel + '):未发现新会话(全部为重复或无效条目)';
}

// OpenSSH config 解析:Host 块 → HostName/User/Port(通配符与 Host* 行跳过)
function parseSshConfig(text) {
  const out = [];
  let cur = null;
  let curName = null;
  const flush = () => {
    if (cur && cur.host && curName && !/[*?!]/.test(curName)) out.push({ ...cur, name: curName });
    cur = null; curName = null;
  };
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const m = line.match(/^(\S+)\s*(.*)$/);
    if (!m) continue;
    const key = m[1].toLowerCase(), val = m[2].trim();
    if (key === 'host') {
      flush();
      curName = val.split(/\s+/)[0];
      cur = {};
    } else if (!cur) { continue; }
    else if (key === 'hostname') cur.host = val.split(/\s+/)[0];
    else if (key === 'user') cur.username = val.split(/\s+/)[0];
    else if (key === 'port') cur.port = Number(val) || 22;
  }
  flush();
  return out;
}

// Xshell .xsh(XML)解析:收集所有元素上 Name="X" Value="Y" 属性对,
// 同时兜底收集裸属性(host/port/user 等)与同名子元素文本,兼容不同 Xshell 版本布局
function parseXshell(text) {
  let doc;
  try { doc = new DOMParser().parseFromString(text, 'text/xml'); } catch { return []; }
  if (doc.querySelector('parsererror')) return [];
  const vals = {};
  const put = (k, v) => { if (v && !vals[k]) vals[k] = v; };
  const walk = (el) => {
    // 形如 <Param Name="Host" Value="1.2.3.4"/>
    const n = el.getAttribute && el.getAttribute('Name');
    const v = el.getAttribute && el.getAttribute('Value');
    if (n && v != null) put(n.toLowerCase(), v);
    for (const attr of el.attributes || []) {
      const k = attr.name.toLowerCase();
      if (['host', 'hostname', 'port', 'portnumber', 'user', 'username', 'loginuser'].includes(k)) put(k === 'loginuser' ? 'username' : k, attr.value);
    }
    for (const child of el.children || []) {
      const k = (child.tagName || '').toLowerCase();
      if (['host', 'hostname', 'port', 'portnumber', 'user', 'username'].includes(k) && child.textContent) put(k === 'portnumber' ? 'port' : k, child.textContent.trim());
      walk(child);
    }
  };
  walk(doc.documentElement);
  const host = vals.host || vals.hostname;
  if (!host) return [];
  return [{
    host,
    port: Number(vals.port) || 22,
    username: vals.username || vals.user || 'root',
    name: vals.name || doc.documentElement.getAttribute('name') || '',
  }];
}

// Termius 导出 JSON:root.data.hosts(常见)或直接 hosts 数组;凭据按 id 解析用户名
function parseTermius(data) {
  const root = (data && data.data && Array.isArray(data.data.hosts)) ? data.data : (Array.isArray(data && data.hosts) ? data : null);
  if (!root) return [];
  const creds = {};
  const credList = root.credentials || (data && data.credentials) || [];
  for (const c of credList) if (c && c.id) creds[c.id] = c.username || '';
  const out = [];
  for (const h of root.hosts) {
    if (!h) continue;
    const host = h.address || h.host || h.hostname;
    if (!host) continue;
    let username = h.username || '';
    const cids = h.credential_ids || h.credentials || [];
    if (!username && Array.isArray(cids) && cids.length) username = creds[cids[0]] || '';
    out.push({ name: h.label || h.name || '', host, port: Number(h.port) || 22, username: username || 'root' });
  }
  return out;
}

async function importExternal(src) {
  const label = EXT_GROUP[src] || src;
  try {
    if (src === 'putty' || src === 'xshell') {
      const res = await window.easyssh.externalSessionsRead(src);
      if (!res || !res.ok) { $('#statusBar').textContent = label + ' 读取失败: ' + (res && res.message || '未知错误'); return; }
      let list = [];
      if (src === 'putty') list = res.entries || [];
      else for (const f of res.files || []) { const p = parseXshell(f.text || ''); if (p[0]) p[0].name = p[0].name || f.name; list = list.concat(p); }
      await mergeExternal(list, label);
      return;
    }
    // sshconfig / termius:文件对话框选择
    const paths = await window.easyssh.pickOpen();
    const path = Array.isArray(paths) ? paths[0] : paths;
    if (!path) return;
    const text = (await window.easyssh.readTextFile(path)).replace(/^\uFEFF/, '');
    if (src === 'sshconfig') await mergeExternal(parseSshConfig(text), label);
    else {
      let data;
      try { data = JSON.parse(text); } catch { $('#statusBar').textContent = '导入失败:文件不是有效的 JSON(Termius 需导出 JSON 格式)'; return; }
      await mergeExternal(parseTermius(data), label);
    }
  } catch (e) {
    $('#statusBar').textContent = label + ' 导入失败: ' + (e && e.message || e);
  }
}
