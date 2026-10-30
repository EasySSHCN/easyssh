/* global Terminal, FitAddon, SearchAddon */
const $ = (s) => document.querySelector(s);
// tab: { id, term, fit, el, name, opts, disconnected }
const tabs = new Map();
let activeId = null;
let sessions = [];
let pendingSess = null; // v2.9.0:列表点进来的原会话条目(克隆副本区分用)
// 版本号从主进程实时读取(app.getVersion),避免改版时状态栏还显示旧版本号
let ver = 'v2.6';
try { ver = 'v' + window.easyssh.getVersionSync(); } catch {}
function sftpTextMode() {
  const m = document.querySelector('input[name="sftpmode"]:checked');
  return m && m.value === 'text';
}

// 全局错误兜底:任何未捕获错误都显示在界面,绝不无声失败
window.addEventListener('error', (e) => {
  const el = $('#connMsg');
  if (el) el.textContent = '脚本错误: ' + (e.message || e.type);
});

/* ---------- xterm ---------- */
function makeTerm(tab) {
  const host = document.createElement('div');
  host.className = 'term-host';
  host.dataset.id = tab.id;
  $('#terminalArea').insertBefore(host, $('#statusBar'));

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
    tab.search = search;
    search.onDidChangeResults(({ resultIndex, resultCount }) => {
      if (tabs.get(activeId) === tab) {
        $('#findCount').textContent = resultCount > 0 ? (resultIndex + 1) + '/' + resultCount : (resultIndex === -1 && resultCount > 0 ? '' : '');
      }
    });
  }
  term.open(host);

  term.onData((d) => {
    if (tab.disconnected) {
      if (/\r|\n/.test(d)) reconnectTab(tab); // 断开后按回车自动重连
      return;
    }
    window.easyssh.write(tab.id, d);
  });
  term.onResize(({ cols, rows }) => {
    if (!tab.disconnected) window.easyssh.resize(tab.id, cols, rows);
  });

  tab.term = term;
  tab.fit = fit;
  tab.el = host;
  host.addEventListener('click', () => term.focus());
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
  // v2.11 右键粘贴(终端区域,不干扰 SFTP 列表/会话列表自身的 contextmenu)
  host.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    pasteToTerm(term);
  });
  return tab;
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
    decorations: {
      matchBackground: 'rgba(137, 180, 250, 0.35)',
      matchOverviewRuler: '#89b4fa',
      activeMatchBackground: 'rgba(250, 179, 135, 0.6)',
      activeMatchColorOverviewRuler: '#fab387',
      activeMatchBorder: '#fab387',
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
  activeId = id;
  for (const [tid, t] of tabs) t.el.classList.toggle('active', tid === id);
  $('#connectPanel').style.display = 'none';
  document.querySelectorAll('.tab').forEach((el) => el.classList.toggle('active', el.dataset.id === id));
  const t = tabs.get(id);
  if (t) {
    t.fit.fit();
    t.term.focus();
    $('#statusBar').textContent = t.disconnected ? t.name + ' 已断开 — 在终端按回车重连' : '已连接: ' + t.name;
  }
}

function closeTab(id) {
  window.easyssh.disconnect(id);
  const t = tabs.get(id);
  if (t) { t.term.dispose(); t.el.remove(); tabs.delete(id); }
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
  el.innerHTML = '<span>' + tab.name + '</span><span class="x">✕</span>';
  // v2.9.0:会话设置了标签颜色则应用到标签页标题
  if (tab.color) el.style.color = tab.color;
  el.addEventListener('click', (e) => {
    if (e.target.classList.contains('x')) closeTab(tab.id);
    else activate(tab.id);
  });
  tab.tabEl = el;
  $('#tabbar').appendChild(el);
}

/* ---------- connect / reconnect ---------- */

/* ================= v3.0.0 主密码 / 已知主机指纹(渲染端流程) ================= */
// masterState:enabled=config.key.enc 存在;unlocked=本次运行已解包 DEK。
// 未启用主密码时 unlocked 恒为 true,行为与旧版一致(零迁移成本)。
const masterState = { enabled: false, unlocked: true };
function masterLocked() { return masterState.enabled && !masterState.unlocked; }
// 锁定态下渲染端内存中的会话密码全是空串,直接保存会把所有已存密码覆盖掉——
// 主密码未解锁时一律拒绝会话写盘(只读浏览不受影响),解锁后自动恢复可用。
async function saveSessionsGuard(list) {
  if (masterLocked()) {
    $('#statusBar').textContent = '⚠ 主密码未解锁:为保护已存密码,本次会话修改未保存。请先在启动弹窗输入主密码解锁';
    return;
  }
  await window.easyssh.saveSessions(list);
}

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

/* ---------- 主密码解锁模态(启动早期出现,全屏阻断) ---------- */
async function showMpUnlock() {
  $('#mpUnlockModal').classList.remove('hidden');
  $('#mpUnlockMsg').textContent = '';
  $('#mpUnlockPw').value = '';
  $('#mpResetRow').classList.add('hidden');
  $('#mpUnlockPw').focus();
}
async function tryMpUnlock() {
  const pw = $('#mpUnlockPw').value;
  if (!pw) { $('#mpUnlockMsg').textContent = '请输入主密码'; return; }
  $('#mpUnlockBtn').disabled = true;
  let r;
  try { r = await window.easyssh.masterpwUnlock(pw); }
  catch (e) { r = { ok: false, message: String(e && e.message || e) }; }
  $('#mpUnlockBtn').disabled = false;
  if (r && r.ok) {
    masterState.unlocked = true;
    $('#mpUnlockModal').classList.add('hidden');
    $('#mpUnlockPw').value = '';
    // 解锁成功后重新加载会话:此前的加载因 DEK 未就绪解不出密码
    try { sessions = await window.easyssh.loadSessions(); renderSessions(); } catch {}
    $('#statusBar').textContent = '✅ 已解锁:会话密码可用';
  } else if (r && r.locked) {
    $('#mpUnlockMsg').textContent = r.message || '错误次数过多,已锁定';
    $('#mpResetRow').classList.remove('hidden'); // 逃生路径:重置密码字段
  } else {
    $('#mpUnlockMsg').textContent = (r && r.remaining != null)
      ? '主密码错误,还可尝试 ' + r.remaining + ' 次'
      : ((r && r.message) || '解锁失败');
    $('#mpUnlockPw').value = '';
    $('#mpUnlockPw').focus();
  }
}
$('#mpUnlockBtn').addEventListener('click', tryMpUnlock);
$('#mpUnlockPw').addEventListener('keydown', (e) => { if (e.key === 'Enter') tryMpUnlock(); });
$('#mpResetBtn').addEventListener('click', async () => {
  if (!confirm('确认重置?将删除主密码文件并清空所有已保存的会话密码(会话列表保留,连接时需重新录入密码)。此操作不可撤销。')) return;
  try { await window.easyssh.masterpwReset(); } catch {}
  masterState.enabled = false;
  masterState.unlocked = true;
  $('#mpUnlockModal').classList.add('hidden');
  try { sessions = await window.easyssh.loadSessions(); renderSessions(); } catch {}
  $('#statusBar').textContent = '已重置:主密码已清除,会话密码已清空,请重新录入';
});

/* ---------- 主密码管理(设置弹窗内) ---------- */
let mpMode = 'enable'; // enable | change | disable
function refreshMpStatus() {
  window.easyssh.masterpwStatus().then((st) => {
    masterState.enabled = !!st.enabled;
    if (!st.enabled) masterState.unlocked = true;
    $('#mpStatus').textContent = st.enabled ? '已启用(本机已启用主密码保护)' : '未启用(仅本机密钥文件保护)';
    $('#btnMpEnable').style.display = st.enabled ? 'none' : '';
    $('#btnMpChange').style.display = st.enabled ? '' : 'none';
    $('#btnMpDisable').style.display = st.enabled ? '' : 'none';
  }).catch(() => {});
}
function openMpModal(mode) {
  mpMode = mode;
  $('#mpMsg').textContent = '';
  $('#mpOld').value = ''; $('#mpNew').value = ''; $('#mpNew2').value = '';
  $('#mpOldRow').style.display = (mode === 'enable') ? 'none' : 'flex';
  $('#mpNewRow').style.display = (mode === 'disable') ? 'none' : 'flex';
  $('#mpNew2Row').style.display = (mode === 'disable') ? 'none' : 'flex';
  const titles = { enable: '启用主密码', change: '更改主密码', disable: '关闭主密码' };
  $('#mpModeTitle').textContent = titles[mode] || '';
  $('#mpTips').textContent = mode === 'enable'
    ? '启用后:现有会话密钥(config.key)被主密码包裹为 config.key.enc,原文件改名为 config.key.bak(首次成功解锁后自动删除)。主密码遗忘无法找回。'
    : (mode === 'change'
      ? '输入当前主密码验证后,用新主密码重新包裹会话密钥。'
      : '输入主密码验证后,还原 config.key 明文文件并删除 config.key.enc(恢复到无主密码状态,保护强度回到仅本机密钥)。');
  $('#mpModal').classList.remove('hidden');
  (mode === 'enable' ? $('#mpNew') : $('#mpOld')).focus();
}
$('#btnMpEnable').addEventListener('click', () => openMpModal('enable'));
$('#btnMpChange').addEventListener('click', () => openMpModal('change'));
$('#btnMpDisable').addEventListener('click', () => openMpModal('disable'));
$('#mpCancel2').addEventListener('click', () => $('#mpModal').classList.add('hidden'));
$('#mpGo').addEventListener('click', async () => {
  const oldPw = $('#mpOld').value, newPw = $('#mpNew').value, newPw2 = $('#mpNew2').value;
  const fail = (msg) => { $('#mpMsg').textContent = msg; };
  let r;
  try {
    if (mpMode === 'enable') {
      if (!newPw || newPw.length < 6) return fail('主密码至少需要 6 位');
      if (newPw !== newPw2) return fail('两次输入的主密码不一致');
      r = await window.easyssh.masterpwEnable(newPw);
    } else if (mpMode === 'change') {
      if (!oldPw) return fail('请输入当前主密码');
      if (!newPw || newPw.length < 6) return fail('新主密码至少需要 6 位');
      if (newPw !== newPw2) return fail('两次输入的新主密码不一致');
      r = await window.easyssh.masterpwChange(oldPw, newPw);
    } else {
      if (!oldPw) return fail('请输入主密码');
      r = await window.easyssh.masterpwDisable(oldPw);
    }
  } catch (e) { r = { ok: false, message: String(e && e.message || e) }; }
  if (!r || !r.ok) return fail((r && r.message) || '操作失败');
  masterState.unlocked = true;
  $('#mpModal').classList.add('hidden');
  $('#statusBar').textContent = {
    enable: '✅ 主密码已启用:config.key 已包裹为 config.key.enc',
    change: '✅ 主密码已更改:会话密钥已用新口令重新包裹',
    disable: '✅ 主密码已关闭:已还原 config.key,删除 config.key.enc',
  }[mpMode] || '已更新';
  refreshMpStatus();
});

async function doConnect(tab, opts) {
  const res = await connectSsh(opts);
  if (!res.ok) {
    // 提取错误首行(调试信息另存界面)
    const first = String(res.info || '未知错误').split('\n')[0];
    if (tab.term) {
      tab.disconnected = true;
      tab.term.write('\r\n\x1b[31m[连接失败: ' + first + ' — 按回车重试]\x1b[0m\r\n');
    }
    $('#connMsg').textContent = res.info || '';
    $('#statusBar').textContent = '连接失败';
    return false;
  }
  // 重连场景:换绑到新连接 id
  if (tab.id !== res.id) {
    tabs.delete(tab.id);
    tab.id = res.id;
    tabs.set(tab.id, tab);
    tab.el.dataset.id = tab.id;
    tab.tabEl.dataset.id = tab.id;
  }
  tab.disconnected = false;
  return true;
}

async function reconnectTab(tab) {
  if (!tab.opts) return;
  tab.term.write('\x1b[33m[正在重连 ' + tab.opts.host + ' ...]\x1b[0m\r\n');
  $('#statusBar').textContent = '重连中 ' + tab.opts.host + ' ...';
  const ok = await doConnect(tab, tab.opts);
  if (ok) {
    tab.term.write('\x1b[32m[已重新连接]\x1b[0m\r\n');
    $('#statusBar').textContent = '已连接: ' + tab.name;
    tab.term.focus();
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
  pendingSess = null; // 手动新建连接,不关联任何已存会话
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

/* ---------- session list ---------- */

/* ---------- v2.9.0 会话分组 / 搜索 / 备注 / 颜色 / 导入导出 / 克隆 ---------- */
const sessKey = (s) => s.host + ':' + (s.port || 22) + ':' + s.username;
const sessSecretFields = ['password', 'passwordEnc', 'passphrase', 'passphraseEnc']; // 导出/导入一律剔除的敏感字段
function escH(v) {
  return String(v === null || v === undefined ? '' : v).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}
// 空分组持久保存(localStorage,轻量便携;随会话保存的分组名不需要额外存储)
function loadExtraGroups() {
  try { const a = JSON.parse(localStorage.getItem('easyssh.groups') || '[]'); return Array.isArray(a) ? a.filter((g) => typeof g === 'string' && g.trim()) : []; } catch { return []; }
}
function saveExtraGroups(list) { try { localStorage.setItem('easyssh.groups', JSON.stringify(list)); } catch {} }
function getGroups() {
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

function renderSessions() {
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
    head.innerHTML = '<span>' + (collapsed ? '▸ ' : '▾ ') + escH(grp.label) + '</span><span class="cnt">(' + items.length + ')</span>';
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
    + '</span><span class="del" title="删除">🗑</span>';
  if (s.note) li.title = s.note; // 备注悬停提示
  li.addEventListener('click', (e) => {
    if (e.target.classList.contains('del')) {
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
  pendingSess = s; // 记录来源条目,提交时优先按它匹配(克隆副本区分)
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
function hideCtxMenu() { $('#ctxMenu').classList.add('hidden'); }
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
function applyTabColors(key) {
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
    el.style.outline = ((el.dataset.color || '') === seColor) ? '2px solid var(--fg)' : 'none';
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

/* ---------- 新建分组 ---------- */
$('#btnNewGroup').addEventListener('click', () => {
  $('#groupName').value = '';
  $('#groupModal').classList.remove('hidden');
  $('#groupName').focus();
});
$('#groupSave').addEventListener('click', () => {
  const name = $('#groupName').value.trim();
  if (!name) { $('#statusBar').textContent = '请输入分组名称'; return; }
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

/* ---------- main -> renderer 数据 ---------- */
window.easyssh.onData((id, data) => { const t = tabs.get(id); if (t) t.term.write(data); });
window.easyssh.onClosed((id) => {
  const t = tabs.get(id);
  if (t && !t.disconnected) {
    t.disconnected = true;
    t.term.write('\r\n\x1b[33m[连接已断开 — 按回车重连]\x1b[0m\r\n');
    $('#statusBar').textContent = t.name + ' 已断开 — 按回车重连';
  }
});

window.addEventListener('resize', () => { const t = tabs.get(activeId); if (t) t.fit.fit(); });


/* ================= v2.0 设置 / 皮肤 / 快捷命令 / SFTP ================= */
const THEMES = {
  dracula: {
    term: { background: '#1e1e2e', foreground: '#cdd6f4', cursor: '#f8f8f2', cursorAccent: '#1e1e2e', selectionBackground: '#44475a' },
    ui: { bg: '#1e1e2e', bg2: '#181825', panel: '#27273a', fg: '#cdd6f4', accent: '#89b4fa', border: '#313244', redbtn: '#f38ba8' },
  },
  pureblack: {
    term: { background: '#000000', foreground: '#e0e0e0', cursor: '#ffffff', cursorAccent: '#000000', selectionBackground: '#444444' },
    ui: { bg: '#0d0d0d', bg2: '#000000', panel: '#1a1a1a', fg: '#e0e0e0', accent: '#4fc3f7', border: '#242424', redbtn: '#ff6b6b' },
  },
  solarized: {
    term: { background: '#002b36', foreground: '#839496', cursor: '#93a1a1', cursorAccent: '#002b36', selectionBackground: '#073642' },
    ui: { bg: '#002b36', bg2: '#001e26', panel: '#073642', fg: '#839496', accent: '#268bd2', border: '#0a4552', redbtn: '#dc322f' },
  },
  light: {
    term: { background: '#fdf6e3', foreground: '#586e75', cursor: '#657b83', cursorAccent: '#fdf6e3', selectionBackground: '#eee8d5' },
    ui: { bg: '#fdf6e3', bg2: '#eee8d5', panel: '#f5efdc', fg: '#586e75', accent: '#268bd2', border: '#d5cdb8', redbtn: '#dc322f' },
  },
  matrix: {
    term: { background: '#001100', foreground: '#00ff41', cursor: '#00ff41', cursorAccent: '#001100', selectionBackground: '#005f00' },
    ui: { bg: '#04140a', bg2: '#001100', panel: '#0a2410', fg: '#39ff6e', accent: '#00ff41', border: '#124a1e', redbtn: '#ff5555' },
  },
};
let appSettings = { fontFamily: 'Consolas', fontSize: 14, encoding: 'utf8', theme: 'dracula', quickCommands: [], scrollback: 5000, copyOnSelect: false, sessionLogEnabled: false, sessionLogDir: '' };

function applyTermSettings() {
  const t = THEMES[appSettings.theme] || THEMES.dracula;
  for (const tab of tabs.values()) {
    tab.term.options.fontFamily = appSettings.fontFamily + ', monospace';
    tab.term.options.fontSize = Number(appSettings.fontSize) || 14;
    tab.term.options.theme = t.term;
    tab.term.options.scrollback = Number(appSettings.scrollback) || 5000; // v2.11 回滚行数运行时生效
  }
  // 整个应用界面 + 标题栏跟随皮肤
  const root = document.documentElement;
  for (const [k, v] of Object.entries(t.ui)) root.style.setProperty('--' + k, v);
  window.easyssh.setWinTheme(t.ui.bg2, t.ui.fg);
}
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

/* ---------- 设置弹窗 ---------- */
$('#btnSettings').addEventListener('click', async () => {
  appSettings = await window.easyssh.loadSettings();
  $('#setFont').value = appSettings.fontFamily;
  $('#setSize').value = appSettings.fontSize;
  $('#setEnc').value = appSettings.encoding;
  $('#setTheme').value = appSettings.theme;
  // v2.11 新增设置项(旧配置文件缺字段时 loadSettings 已补默认值)
  $('#setScrollback').value = String(Number(appSettings.scrollback) || 5000);
  $('#setCopyOnSelect').checked = !!appSettings.copyOnSelect;
  $('#setSessLog').checked = !!appSettings.sessionLogEnabled;
  $('#setLogDir').value = appSettings.sessionLogDir || '';
  refreshMpStatus(); // v3.0.0 主密码状态与启用/更改/关闭入口
  $('#settingsModal').classList.remove('hidden');
});
$('#setSave').addEventListener('click', async () => {
  appSettings.fontFamily = $('#setFont').value;
  appSettings.fontSize = Number($('#setSize').value) || 14;
  appSettings.encoding = $('#setEnc').value;
  appSettings.theme = $('#setTheme').value;
  // v2.11 新增设置项:回滚行数 / 选中即复制 / 会话日志
  appSettings.scrollback = Number($('#setScrollback').value) || 5000;
  appSettings.copyOnSelect = $('#setCopyOnSelect').checked;
  appSettings.sessionLogEnabled = $('#setSessLog').checked;
  appSettings.sessionLogDir = $('#setLogDir').value.trim();
  await window.easyssh.saveSettings(appSettings);
  applyTermSettings();
  $('#settingsModal').classList.add('hidden');
  $('#statusBar').textContent = '设置已保存' + (appSettings.encoding !== 'utf8' ? '(编码对新连接生效)' : '')
    + (appSettings.sessionLogEnabled ? ';会话日志对新连接生效' : '');
});

/* ---------- 快捷命令管理 ---------- */
$('#btnQuick').addEventListener('click', async () => {
  appSettings = await window.easyssh.loadSettings();
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
    li.innerHTML = '<b>' + qc.name + '</b><span class="qc">' + qc.cmd + '</span><span class="edit">✎</span><span class="del">✕</span>';
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

/* ---------- 本地端口转发(SecureCRT 会话选项→端口转发 同款业务) ---------- */
$('#btnPf').addEventListener('click', () => {
  const t = tabs.get(activeId);
  if (!t || t.disconnected) { $('#statusBar').textContent = '当前连接已断开,先连上服务器再配置端口转发'; return; }
  $('#pfTitle').textContent = ' — ' + t.name;
  resetPfEditor();
  renderPfList();
  $('#pfModal').classList.remove('hidden');
});
let editingPfPort = null; // 正在编辑的已保存转发的 localPort(null=新增模式)

// 把当前会话的转发配置写回已保存会话(重连时自动重启用)
async function savePfForwards(t) {
  const nk = t.opts.host + ':' + (t.opts.port || 22) + ':' + t.opts.username;
  const rec = sessions.find((s) => s.host + ':' + (s.port || 22) + ':' + s.username === nk);
  if (rec) { rec.forwards = t.opts.forwards; await saveSessionsGuard(sessions); }
}

function resetPfEditor() {
  editingPfPort = null;
  $('#pfAdd').textContent = '启动';
  $('#pfName').value = ''; $('#pfLocalPort').value = ''; $('#pfRemoteHost').value = ''; $('#pfRemotePort').value = '';
  if ($('#pfSocks').checked) { $('#pfSocks').checked = false; $('#pfSocks').dispatchEvent(new Event('change')); }
}

// 把一条转发配置填回表单进入编辑模式(保存后即可再次启动使用)
function editPfConfig(f) {
  editingPfPort = Number(f.localPort);
  $('#pfAdd').textContent = '保存';
  $('#pfSocks').checked = !!f.dynamic;
  $('#pfSocks').dispatchEvent(new Event('change'));
  $('#pfName').value = f.name || '';
  $('#pfLocalPort').value = f.localPort;
  $('#pfRemoteHost').value = f.dynamic ? '' : (f.remoteHost || '');
  $('#pfRemotePort').value = f.dynamic ? '' : (f.remotePort || '');
  $('#pfLocalPort').focus();
}

async function renderPfList() {
  const ul = $('#pfList');
  ul.innerHTML = '';
  const t = tabs.get(activeId);
  const running = await window.easyssh.pfList(activeId);
  const saved = (t && Array.isArray(t.opts.forwards)) ? t.opts.forwards : [];
  const runningPorts = new Set(running.map((f) => Number(f.localPort)));
  const savedOnly = saved.filter((f) => !runningPorts.has(Number(f.localPort)));
  if (!running.length && !savedOnly.length) { const li = document.createElement('li'); li.textContent = '(暂无转发,在下方填写后点「启动」;配置会随会话保存,下次可一键复用)'; ul.appendChild(li); return; }
  // 正在运行的转发:停止 + 编辑(编辑会以新配置重启)
  running.forEach((f) => {
    const li = document.createElement('li');
    li.innerHTML = '<b>' + f.name + '</b><span class="qc">' + (f.dynamic
      ? 'SOCKS4/5 动态代理 本机 127.0.0.1:' + f.localPort
      : '本机 127.0.0.1:' + f.localPort + ' → ' + f.remoteHost + ':' + f.remotePort) + ' (运行中)</span>'
      + '<span class="edit" title="编辑此转发">✎</span><span class="del" title="停止">✕</span>';
    li.querySelector('.edit').addEventListener('click', () => editPfConfig(f));
    li.querySelector('.del').addEventListener('click', async () => {
      if (editingPfPort === Number(f.localPort)) resetPfEditor();
      await window.easyssh.pfStop(activeId, f.localPort);
      renderPfList();
    });
    ul.appendChild(li);
  });
  // 已保存但未运行的转发记录:▶ 一键重新使用 / ✎ 编辑 / ✕ 删除
  savedOnly.forEach((f) => {
    const li = document.createElement('li');
    li.innerHTML = '<b>' + f.name + '</b><span class="qc">' + (f.dynamic
      ? 'SOCKS4/5 动态代理 本机 127.0.0.1:' + f.localPort
      : '本机 127.0.0.1:' + f.localPort + ' → ' + (f.remoteHost || '127.0.0.1') + ':' + f.remotePort) + ' (未启动)</span>'
      + '<span class="use" title="启动此转发">▶</span><span class="edit" title="编辑此转发">✎</span><span class="del" title="删除此记录">✕</span>';
    li.querySelector('.use').addEventListener('click', async () => {
      const res = await window.easyssh.pfStart(activeId, f);
      if (!res.ok) { $('#statusBar').textContent = '端口转发失败: ' + (res.message || '未知错误'); return; }
      t.term.write('\x1b[36m[' + (f.dynamic
        ? 'SOCKS4/5 动态转发已启动: 本机 127.0.0.1:' + f.localPort
        : '端口转发已启动: 本机 127.0.0.1:' + f.localPort + ' → ' + (f.remoteHost || '127.0.0.1') + ':' + f.remotePort) + ']\x1b[0m\r\n');
      renderPfList();
    });
    li.querySelector('.edit').addEventListener('click', () => editPfConfig(f));
    li.querySelector('.del').addEventListener('click', async () => {
      if (editingPfPort === Number(f.localPort)) resetPfEditor();
      t.opts.forwards = (t.opts.forwards || []).filter((x) => Number(x.localPort) !== Number(f.localPort));
      await savePfForwards(t);
      renderPfList();
    });
    ul.appendChild(li);
  });
}
$('#pfAdd').addEventListener('click', async () => {
  const t = tabs.get(activeId);
  if (!t || t.disconnected) return;
  const dynamic = $('#pfSocks').checked; // SecureCRT 式:勾选后为 SOCKS4/5 动态转发,无需远程端口
  const cfg = {
    name: $('#pfName').value.trim() || ('PF' + $('#pfLocalPort').value.trim()),
    localPort: Number($('#pfLocalPort').value),
    remoteHost: $('#pfRemoteHost').value.trim(),
    remotePort: dynamic ? 0 : Number($('#pfRemotePort').value),
    dynamic,
  };
  // 编辑模式:更新已保存的记录;若该端口正在运行,先停掉旧监听再按新配置重启
  if (editingPfPort !== null) {
    t.opts.forwards = t.opts.forwards || [];
    const si = t.opts.forwards.findIndex((x) => Number(x.localPort) === Number(editingPfPort));
    if (si >= 0) t.opts.forwards[si] = cfg; else t.opts.forwards.push(cfg);
    await savePfForwards(t);
    const wasRunning = await window.easyssh.pfStop(activeId, editingPfPort);
    resetPfEditor();
    const res = await window.easyssh.pfStart(activeId, cfg);
    if (!res.ok) { $('#statusBar').textContent = '端口转发失败: ' + (res.message || '未知错误'); renderPfList(); return; }
    t.term.write('\x1b[36m[' + (dynamic
      ? 'SOCKS4/5 动态转发已启动: 本机 127.0.0.1:' + cfg.localPort + ' (把浏览器/程序的 SOCKS 代理指向此端口)'
      : '端口转发已启动: 本机 127.0.0.1:' + cfg.localPort + ' → ' + (cfg.remoteHost || '127.0.0.1') + ':' + cfg.remotePort) + ']\x1b[0m\r\n');
    renderPfList();
    return;
  }
  const res = await window.easyssh.pfStart(activeId, cfg);
  if (!res.ok) { $('#statusBar').textContent = '端口转发失败: ' + (res.message || '未知错误'); return; }
  // 配置随会话持久化:重连时自动重启,并写回已保存会话(同端口已存在则覆盖)
  t.opts.forwards = t.opts.forwards || [];
  const si = t.opts.forwards.findIndex((x) => Number(x.localPort) === cfg.localPort);
  if (si >= 0) t.opts.forwards[si] = cfg; else t.opts.forwards.push(cfg);
  await savePfForwards(t);
  resetPfEditor();
  t.term.write('\x1b[36m[' + (dynamic
    ? 'SOCKS4/5 动态转发已启动: 本机 127.0.0.1:' + cfg.localPort + ' (把浏览器/程序的 SOCKS 代理指向此端口)'
    : '端口转发已启动: 本机 127.0.0.1:' + cfg.localPort + ' → ' + (cfg.remoteHost || '127.0.0.1') + ':' + cfg.remotePort) + ']\x1b[0m\r\n');
  renderPfList();
});
// 勾选 SOCKS 动态转发时,远程主机/端口无需填写(SecureCRT 同款交互)
$('#pfSocks').addEventListener('change', () => {
  const on = $('#pfSocks').checked;
  $('#pfRemoteHost').disabled = on;
  $('#pfRemotePort').disabled = on;
  $('#pfRemoteHost').placeholder = on ? '动态转发无需设置' : '目标主机,留空=SSH服务器';
  $('#pfRemotePort').placeholder = on ? '无需设置' : '目标端口';
  $('#pfTips').textContent = on
    ? '已选 SOCKS 4/5 动态转发:本机 127.0.0.1:本地端口 将成为一个经 SSH 加密隧道的 SOCKS 代理,目标由使用它的程序(浏览器、curl --socks5 等)自行指定,无需设置远程主机/端口。会话断开转发自动停止。'
    : '连接建立后,访问本机 127.0.0.1:本地端口 的流量将经加密隧道由 SSH 服务器转发到 目标主机:目标端口(可用于安全访问内网数据库/管理台)。会话断开转发自动停止。';
});
window.easyssh.onPfEvent(({ id, message }) => {
  const t = tabs.get(id);
  if (t) t.term.write('\x1b[33m[端口转发] ' + message + '\x1b[0m\r\n');
});

/* ---------- SFTP 面板 ---------- */
let sftpSel = null; // 选中的远程文件 { name, path, type, size }
function sftpAbs(dir, name) { return (dir === '/' ? '' : dir) + '/' + name; }
function fmtSize(n) {
  if (n < 1024) return n + ' B';
  if (n < 1048576) return (n / 1024).toFixed(1) + ' KB';
  if (n < 1073741824) return (n / 1048576).toFixed(1) + ' MB';
  return (n / 1073741824).toFixed(2) + ' GB';
}
async function sftpBrowse(dir) {
  const t = tabs.get(activeId);
  if (!t) { $('#sftpMsg').textContent = '请先连接一个会话'; return; }
  if (t.disconnected) { $('#sftpMsg').textContent = '当前连接已断开,按回车重连后再用 SFTP'; return; }
  $('#sftpMsg').textContent = '';
  $('#sftpList').innerHTML = '<div class="frow"><span class="fname">加载中...</span></div>';
  try {
    const items = await window.easyssh.sftpList(t.id, dir);
    $('#sftpPath').value = dir;
    $('#sftpTitle').textContent = t.name;
    sftpSel = null;
    const list = $('#sftpList');
    list.innerHTML = '';
    if (dir !== '/') {
      const up = document.createElement('div');
      up.className = 'frow';
      up.innerHTML = '<span class="fname">📁 ..</span>';
      up.addEventListener('click', () => sftpBrowse(dir.split('/').slice(0, -1).join('/') || '/'));
      list.appendChild(up);
    }
    for (const it of items) {
      const row = document.createElement('div');
      row.className = 'frow';
      row.innerHTML = '<span class="fname">' + (it.type === 'dir' ? '📁 ' : '📄 ') + it.name + '</span>'
        + '<span class="fsize">' + (it.type === 'dir' ? '-' : fmtSize(it.size)) + '</span>';
      row.addEventListener('click', () => {
        list.querySelectorAll('.frow').forEach((r) => r.classList.remove('sel'));
        row.classList.add('sel');
        sftpSel = { ...it, path: sftpAbs(dir, it.name) };
      });
      row.addEventListener('dblclick', () => { if (it.type === 'dir') sftpBrowse(sftpAbs(dir, it.name)); });
      // v2.10.0 远端行右键菜单(下载/重命名/删除/新建目录/刷新)
      row.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        e.stopPropagation();
        list.querySelectorAll('.frow').forEach((r) => r.classList.remove('sel'));
        row.classList.add('sel');
        sftpSel = { ...it, path: sftpAbs(dir, it.name) };
        openSftpCtx(e.clientX, e.clientY);
      });
      list.appendChild(row);
    }
  } catch (e) {
    $('#sftpList').innerHTML = '';
    $('#sftpMsg').textContent = 'SFTP 错误: ' + (e.message || e);
  }
}
$('#btnSftp').addEventListener('click', () => {
  const t = tabs.get(activeId);
  if (!t) { $('#statusBar').textContent = '请先连接一个会话再打开 SFTP'; return; }
  $('#sftpModal').classList.remove('hidden');
  sftpBrowse($('#sftpPath').value || '/root');
});
$('#sftpGo').addEventListener('click', () => sftpBrowse($('#sftpPath').value.trim() || '/'));
$('#sftpPath').addEventListener('keydown', (e) => { if (e.key === 'Enter') sftpBrowse($('#sftpPath').value.trim() || '/'); });
$('#sftpUp').addEventListener('click', () => {
  const cur = $('#sftpPath').value.trim() || '/';
  sftpBrowse(cur.split('/').slice(0, -1).join('/') || '/');
});
// v2.10.0:上传改走传输队列(xferStart),不再阻塞等待单个文件传完
$('#sftpUpload').addEventListener('click', async () => {
  const t = tabs.get(activeId);
  if (!t || t.disconnected) return;
  const files = await window.easyssh.pickOpen();
  if (!files.length) return;
  const dir = $('#sftpPath').value.trim() || '/root';
  let queued = 0;
  for (const f of files) {
    const remote = sftpAbs(dir, f.split('\\').pop().split('/').pop());
    const res = await startUploadTask(t.id, f, remote, false);
    if (res.ok) queued++; else $('#sftpMsg').textContent = '上传失败: ' + (res.message || '未知错误');
  }
  if (queued) {
    $('#sftpMsg').textContent = '';
    $('#statusBar').textContent = '已加入上传队列 ' + queued + ' 个任务,点「⇅ 队列」查看进度';
    sftpBrowse(dir);
  }
});
// v2.10.0:双击文件 = 下载(保留「选择保存位置」交互,但改走传输队列;目录双击进入,目录下载走右键菜单)
document.addEventListener('dblclick', async (e) => {
  const row = e.target.closest('#sftpList .frow');
  if (!row || !sftpSel || sftpSel.type !== 'file') return;
  const t = tabs.get(activeId);
  if (!t || t.disconnected) return;
  const local = await window.easyssh.pickSave(sftpSel.name);
  if (!local) return;
  const res = await window.easyssh.xferStart(t.id, {
    direction: 'download', localPath: local, remotePath: sftpSel.path, ...xferOpts(),
  });
  if (!res.ok) { $('#sftpMsg').textContent = '下载失败: ' + (res.message || '未知错误'); return; }
  $('#sftpMsg').textContent = '';
  $('#statusBar').textContent = '已加入下载队列: ' + sftpSel.name;
});

/* ================= v2.10.0 SFTP 强化:右键菜单 / 传输队列 / 拖拽 ================= */

/* ---------- 传输配置(文本模式/编码沿用面板设置) ---------- */
function xferOpts() {
  const o = { textMode: sftpTextMode() };
  if (o.textMode && appSettings && appSettings.encoding) o.encoding = appSettings.encoding;
  return o;
}
// 发起上传任务(返回 xfer:start 的结果,便于统计入队数量)
async function startUploadTask(sessionId, localPath, remotePath, recursive) {
  return window.easyssh.xferStart(sessionId, {
    direction: 'upload', localPath, remotePath, recursive: !!recursive, ...xferOpts(),
  });
}

/* ---------- SFTP 远端文件右键菜单 ---------- */
let sftpCtxTarget = null; // 右键菜单当前指向的远端条目 { name, path, type }
function hideSftpCtx() { $('#sftpCtx').classList.add('hidden'); }
function openSftpCtx(x, y) {
  sftpCtxTarget = sftpSel;
  hideCtxMenu();
  const m = $('#sftpCtx');
  m.classList.remove('hidden'); // 先显示才能量取尺寸,用于贴边防溢出
  const r = m.getBoundingClientRect();
  m.style.left = Math.max(4, Math.min(x, window.innerWidth - r.width - 8)) + 'px';
  m.style.top = Math.max(4, Math.min(y, window.innerHeight - r.height - 8)) + 'px';
}
document.addEventListener('click', (e) => { if (!e.target.closest('#sftpCtx')) hideSftpCtx(); });
document.addEventListener('contextmenu', (e) => { if (!e.target.closest('#sftpList .frow')) hideSftpCtx(); });

// v2.10 远端操作改走 SFTP 通道(sftp:rename / sftp:remove / sftp:mkdir),不再写终端 shell 命令
async function sftpRemoteOp(id, fn, label) {
  const res = await fn(id);
  if (!res.ok) $('#sftpMsg').textContent = label + '失败: ' + (res.message || '未知错误');
  return res.ok;
}
$('#sftpCtx').addEventListener('click', async (e) => {
  const item = e.target.closest('.ctx-item');
  hideSftpCtx();
  if (!item || !sftpCtxTarget) return;
  const t = tabs.get(activeId);
  if (!t || t.disconnected) { $('#statusBar').textContent = '当前连接已断开,无法执行远端操作'; return; }
  const act = item.dataset.act;
  const cur = $('#sftpPath').value.trim() || '/';
  if (act === 'refresh') { sftpBrowse(cur); return; }
  if (act === 'download') {
    if (sftpCtxTarget.type === 'dir') {
      // 目录下载:先确定本地落盘目录(exe 目录渲染端拿不到,用一次「选择保存位置」取目录并记住,之后不再询问)
      const base = await ensureDlDir();
      if (!base) { $('#statusBar').textContent = '已取消目录下载'; return; }
      const localDir = joinLocal(base, sftpCtxTarget.name);
      const res = await window.easyssh.xferStart(t.id, {
        direction: 'download', localPath: localDir, remotePath: sftpCtxTarget.path, recursive: true, ...xferOpts(),
      });
      if (!res.ok) { $('#sftpMsg').textContent = '下载失败: ' + (res.message || '未知错误'); return; }
      $('#statusBar').textContent = '目录「' + sftpCtxTarget.name + '」已加入下载队列,落盘到 ' + localDir;
    } else {
      // 单文件下载:保留原「选择保存位置」交互,但改走传输队列
      const local = await window.easyssh.pickSave(sftpCtxTarget.name);
      if (!local) return;
      const res = await window.easyssh.xferStart(t.id, {
        direction: 'download', localPath: local, remotePath: sftpCtxTarget.path, ...xferOpts(),
      });
      if (!res.ok) { $('#sftpMsg').textContent = '下载失败: ' + (res.message || '未知错误'); return; }
      $('#statusBar').textContent = '已加入下载队列: ' + sftpCtxTarget.name;
    }
    return;
  }
  if (act === 'rename') {
    const nn = prompt('重命名为:', sftpCtxTarget.name);
    if (!nn || nn === sftpCtxTarget.name) return;
    const ok = await sftpRemoteOp(t.id, (id) => window.easyssh.sftpRename(id, sftpCtxTarget.path, sftpAbs(cur, nn)), '重命名');
    if (ok) { $('#statusBar').textContent = '重命名完成'; sftpBrowse(cur); }
    return;
  }
  if (act === 'mkdir') {
    const nn = prompt('新建目录名称:', '');
    if (!nn) return;
    const ok = await sftpRemoteOp(t.id, (id) => window.easyssh.sftpMkdir(id, sftpAbs(cur, nn)), '新建目录');
    if (ok) { $('#statusBar').textContent = '目录已创建'; sftpBrowse(cur); }
    return;
  }
  if (act === 'delete') {
    if (!confirm((sftpCtxTarget.type === 'dir' ? '删除目录(含全部内容)' : '删除文件') + ': ' + sftpCtxTarget.path + ' ?')) return;
    const ok = await sftpRemoteOp(t.id, (id) => window.easyssh.sftpRemove(id, sftpCtxTarget.path), '删除');
    if (ok) { $('#statusBar').textContent = '已删除'; sftpBrowse(cur); }
  }
});

// 目录下载落盘目录:渲染端拿不到 exe 同目录,首次用「选择保存位置」对话框取一个目录并记住(localStorage)
async function ensureDlDir() {
  const saved = localStorage.getItem('easyssh.dlDir');
  if (saved) return saved;
  const picked = await window.easyssh.pickSave('选择任意位置以确定目录(此文件名会被忽略)');
  if (!picked) return null;
  const dir = picked.replace(/[\\/][^\\/]+$/, '');
  if (!dir) return null;
  localStorage.setItem('easyssh.dlDir', dir);
  return dir;
}
// 本地路径拼接(沿用 picked 路径中的分隔符风格)
function joinLocal(base, name) {
  const sep = base.includes('\\') ? '\\' : '/';
  return base.replace(/[\\/]+$/, '') + sep + name;
}

/* ---------- 拖拽上传 ---------- */
// Electron 32+ 渲染端拿不到 File.path,优先经 preload 透传的 webUtils.getPathForFile 还原真实
// 本地路径;拿不到 path(异常环境)才提示改用「上传文件…」按钮。
(function initSftpDnd() {
  const list = $('#sftpList');
  list.addEventListener('dragover', (e) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
    $('#sftpDropHint').classList.remove('hidden');
  });
  list.addEventListener('dragleave', () => $('#sftpDropHint').classList.add('hidden'));
  list.addEventListener('drop', async (e) => {
    e.preventDefault();
    $('#sftpDropHint').classList.add('hidden');
    const t = tabs.get(activeId);
    if (!t || t.disconnected) { $('#statusBar').textContent = '当前连接已断开,无法上传'; return; }
    const files = [...(e.dataTransfer.files || [])];
    if (!files.length) return;
    const dir = $('#sftpPath').value.trim() || '/root';
    let queued = 0, noPath = 0;
    for (const f of files) {
      let p = f.path || '';
      if (!p) { try { p = window.easyssh.getPathForFile(f) || ''; } catch {} }
      if (!p) { noPath++; continue; }
      const res = await startUploadTask(t.id, p, sftpAbs(dir, f.name), f.type === '' || /\/$/.test(p));
      if (res.ok) queued++;
    }
    if (noPath) $('#sftpMsg').textContent = '提示:无法获取拖入文件的本地路径,请用「上传文件…」按钮选择后上传。';
    if (queued) { $('#statusBar').textContent = '已加入上传队列 ' + queued + ' 个任务'; sftpBrowse(dir); }
  });
})();

/* ---------- 传输队列弹窗 ---------- */
const xferTasks = new Map(); // taskId -> task(进度事件实时更新)
const xferCleared = new Set(); // 「清空已完成」只从视图移除,主进程历史不动
const XFER_PHASE_TXT = { queued: '排队', running: '传输中', done: '完成', failed: '失败', cancelled: '已取消' };
function fmtSpeed(bps) {
  if (!bps || bps <= 0) return '';
  if (bps < 1024) return Math.round(bps) + ' B/s';
  if (bps < 1048576) return (bps / 1024).toFixed(1) + ' KB/s';
  return (bps / 1048576).toFixed(1) + ' MB/s';
}
function xferActiveCount() {
  let n = 0;
  for (const t of xferTasks.values()) if (t.phase === 'queued' || t.phase === 'running') n++;
  return n;
}
function renderXferBadge() { $('#xferCount').textContent = xferActiveCount(); }

function renderXferList() {
  const ul = $('#xferList');
  const all = [...xferTasks.values()].filter((t) => !xferCleared.has(t.taskId));
  ul.innerHTML = '';
  if (!all.length) {
    const d = document.createElement('div');
    d.className = 'xempty';
    d.textContent = '(暂无传输任务;在 SFTP 面板右键文件下载,或用「上传文件…」发起传输)';
    ul.appendChild(d);
    return;
  }
  for (const t of all) {
    const pct = t.bytesTotal > 0 ? Math.min(100, Math.round(t.bytesDone / t.bytesTotal * 100)) : (t.phase === 'done' ? 100 : 0);
    const row = document.createElement('div');
    row.className = 'xrow phase-' + t.phase;
    row.innerHTML = '<span class="xdir">' + (t.direction === 'upload' ? '⬆' : '⬇') + '</span>'
      + '<div class="xmain">'
      + '<div class="xtitle"><b>' + (t.direction === 'upload' ? '上传' : '下载') + '</b> '
      + escH(t.direction === 'upload' ? t.localPath : t.remotePath)
      + (t.recursive ? ' <span class="tips">(目录递归)</span>' : '')
      + (t.textMode ? ' <span class="tips">(文本' + escH(t.encoding || '') + ')</span>' : '')
      + '</div>'
      + '<div class="xbar"><div class="xbar-in" style="width:' + pct + '%"></div></div>'
      + '<div class="xmeta">' + fmtSize(t.bytesDone) + ' / ' + fmtSize(t.bytesTotal)
      + (t.speedBps > 0 ? ' · ' + fmtSpeed(t.speedBps) : '')
      + (t.filesTotal > 0 ? ' · 文件 ' + t.filesDone + '/' + t.filesTotal : '')
      + (t.currentFile ? ' · ' + escH(t.currentFile) : '')
      + (t.phase === 'failed' && t.error ? '<br><span class="xerr">失败原因: ' + escH(t.error) + '</span>' : '')
      + '</div></div>'
      + '<span class="xbadge b-' + t.phase + '">' + (XFER_PHASE_TXT[t.phase] || t.phase) + '</span>'
      + ((t.phase === 'queued' || t.phase === 'running') ? '<span class="xcancel" title="取消此任务">✕</span>' : '');
    const c = row.querySelector('.xcancel');
    if (c) c.addEventListener('click', async () => {
      const res = await window.easyssh.xferCancel(t.taskId);
      if (!res.ok) $('#statusBar').textContent = '取消失败: ' + (res.message || '未知错误');
    });
    ul.appendChild(row);
  }
}

$('#btnXferQueue').addEventListener('click', async () => {
  $('#xferTitle').textContent = ' — 最近 50 条';
  try {
    const list = await window.easyssh.xferList();
    for (const t of list) xferTasks.set(t.taskId, t);
  } catch { /* 拉取失败就用缓存渲染 */ }
  renderXferBadge();
  renderXferList();
  $('#xferModal').classList.remove('hidden');
});
$('#btnXferClear').addEventListener('click', () => {
  for (const t of xferTasks.values()) {
    if (t.phase !== 'queued' && t.phase !== 'running') xferCleared.add(t.taskId);
  }
  renderXferList();
});
// 进度事件实时刷新(约 200ms 节流,phase 变化必达)
window.easyssh.onXferProgress((t) => {
  if (!t || !t.taskId) return;
  xferTasks.set(t.taskId, t);
  renderXferBadge();
  if (!$('#xferModal').classList.contains('hidden')) renderXferList();
});

/* ---------- 使用反馈(加密提交到官网) ---------- */
$('#btnFeedback').addEventListener('click', () => {
  $('#fbMsg').textContent = '';
  $('#fbModal').classList.remove('hidden');
  $('#fbContent').focus();
});
$('#fbOpenSite').addEventListener('click', (e) => {
  e.preventDefault();
  window.open('https://easyssh.cn/download.html', '_blank');
});
$('#fbSubmit').addEventListener('click', async () => {
  const btn = $('#fbSubmit');
  const content = $('#fbContent').value.trim();
  if (content.length < 5) { $('#fbMsg').textContent = '反馈内容至少 5 个字'; $('#fbMsg').style.color = 'var(--red)'; return; }
  btn.disabled = true;
  $('#fbMsg').textContent = '提交中…'; $('#fbMsg').style.color = '';
  try {
    const res = await window.easyssh.fbSubmit({
      type: $('#fbType').value,
      contact: $('#fbContact').value.trim(),
      content,
    });
    $('#fbMsg').textContent = res.message || (res.ok ? '已提交' : '提交失败');
    $('#fbMsg').style.color = res.ok ? 'var(--green, #4caf50)' : 'var(--red)';
    if (res.ok) { $('#fbContent').value = ''; $('#fbContact').value = ''; }
  } catch (e) {
    $('#fbMsg').textContent = '提交失败: ' + (e.message || e);
    $('#fbMsg').style.color = 'var(--red)';
  } finally { btn.disabled = false; }
});

/* ---------- 弹窗关闭 ---------- */
document.querySelectorAll('.close-x').forEach((b) => b.addEventListener('click', () => {
  $('#' + b.dataset.close).classList.add('hidden');
}));
document.querySelectorAll('.modal').forEach((m) => m.addEventListener('click', (e) => {
  // v3.0.0:.mp-block 模态(主密码解锁/指纹确认)必须显式操作,点遮罩不关闭
  if (e.target === m && !m.classList.contains('mp-block')) m.classList.add('hidden');
}));

/* v3.0.0 启动早期:检测主密码状态,启用时立即弹全屏阻断解锁模态。
 * 解锁前会话列表照常显示(密码解出为空,连接会失败——可接受的取态),解锁成功后自动重新加载会话。 */
(async function initMasterPw() {
  try {
    const st = await window.easyssh.masterpwStatus();
    masterState.enabled = !!st.enabled;
    masterState.unlocked = !st.enabled;
    if (st.enabled) showMpUnlock();
  } catch { /* 检测失败按未启用处理,与旧版一致 */ }
})();

/* ---------- 初始化设置/皮肤/快捷命令 ---------- */
(async function initSettings() {
  appSettings = await window.easyssh.loadSettings();
  applyTermSettings();
  renderQuickbar();
})();

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

(async function init() {
  sessions = await window.easyssh.loadSessions();
  // 去重时把显示名称计入:克隆副本与原会话同 host:port:username 但名称带「 - 副本」,必须保留
  const key = (s) => (s.name || '') + '|' + s.host + ':' + (s.port || 22) + ':' + s.username;
  const seen = new Map();
  for (const s of sessions) seen.set(key(s), s);
  sessions = [...seen.values()];
  renderSessions();
  $('#statusBar').textContent = 'EasySSH ' + ver + ' — 就绪,点击「新建连接」开始';
})();
/* ================= v2.5 在线升级(加密检测/加密下载) ================= */
let updateEntry = null; // 有新版本时来自升级清单的安装包信息

function fmtUpdateResult(r) {
  if (!r) return '';
  if (r.status === 'available') return `发现新版本 v${r.latestVersion}(当前 ${r.currentVersion})`;
  if (r.status === 'up-to-date') return `已是最新版本${r.latestVersion ? '(线上 v' + r.latestVersion + ')' : ''}`;
  if (r.status === 'busy') return r.message || '正在检测中…';
  return '检测失败: ' + (r.message || '未知错误');
}

function showUpdateBanner(r) {
  const bar = $('#updateBanner');
  if (!bar) return;
  const sb = $('#btnSettingsDownload');
  if (r && r.status === 'available') {
    updateEntry = r.file;
    $('#updateBannerText').textContent =
      `🔔 发现新版本 v${r.latestVersion}(当前 v${(r.currentVersion || '').replace(/^v/, '')}),已通过加密接口校验`;
    bar.style.display = 'flex';
    // 设置中心同步显示「下载并校验」按钮
    if (sb) { sb.style.display = ''; sb.textContent = (updateStage === 'downloaded' ? '✅ 完成升级' : '⬇️ 下载并校验'); sb.disabled = false; }
  } else {
    bar.style.display = 'none';
    if (sb) { sb.style.display = 'none'; sb.textContent = '⬇️ 下载并校验'; sb.disabled = false; }
  }
}

// 自动检测结果 / 下载进度事件推送
function setUpdateBtn(label) {
  const b = $('#btnUpdateDownload');
  if (b) { b.textContent = label; b.disabled = false; }
  const sb = $('#btnSettingsDownload');
  if (sb) { sb.textContent = label; sb.disabled = false; }
}
if (window.easyssh && window.easyssh.onUpdateEvent) {
  window.easyssh.onUpdateEvent((msg) => {
    if (msg.event === 'result') {
      showUpdateBanner(msg.payload);
      const el = $('#updateMsg');
      if (el) el.textContent = fmtUpdateResult(msg.payload);
      const sb = $('#statusBar');
      if (msg.payload.status === 'available') sb.textContent = fmtUpdateResult(msg.payload);
    } else if (msg.event === 'progress') {
      const el = $('#updateMsg');
      if (el) el.textContent = (msg.payload.phase === 'download' ? '下载加密包中… ' : '解密校验中… ') + msg.payload.percent + '%';
      const bar = $('#updateBannerText');
      if (bar) bar.textContent = (msg.payload.phase === 'download' ? '⬇️ 下载加密安装包中… ' : '🔒 解密校验中… ') + msg.payload.percent + '%';
    } else if (msg.event === 'downloaded') {
      // 下载校验完成：横幅进入「完成升级」阶段（原地换名替换，无需手动找文件，全程无脚本）
      updateStage = 'downloaded';
      $('#updateBannerText').textContent =
        '✅ 新版本已下载并通过加密校验，点击「完成升级」自动替换重启（旧版自动备份，可回滚）';
      setUpdateBtn('✅ 完成升级');
      $('#statusBar').textContent = '新版本已就绪: ' + msg.payload.file;
    } else if (msg.event === 'upgraded') {
      $('#statusBar').textContent = '🎉 已通过在线升级到 v' + msg.payload.version;
      alert('EasySSH 已通过在线升级到 v' + msg.payload.version + '。\n旧版本备份(.bak)已自动清理；会话与密钥配置原样保留。');
    }
  });
}

// 启动时回放最近一次检测结果(自动检测可能先于渲染端就绪)
(async function initUpdate() {
  try { showUpdateBanner(await window.easyssh.updateLast()); } catch {}
})();

// 手工「检查更新」(设置中心)
$('#btnUpdateCheck').addEventListener('click', async () => {
  const el = $('#updateMsg');
  el.textContent = '正在通过加密接口检测更新…';
  try {
    const r = await window.easyssh.updateCheck();
    el.textContent = fmtUpdateResult(r);
    if (r.status === 'available') {
      el.textContent += '\n更新内容: ' + (r.notes || []).slice(0, 3).join(' / ');
      showUpdateBanner(r);
    }
  } catch (e) {
    el.textContent = '检测失败: ' + (e.message || e);
  }
});

// 提醒条 / 设置中心共用：两阶段 —— 未下载时是「下载并校验」；下载完成后变为「✅ 完成升级」(原地换名替换重启)
let updateStage = 'idle'; // idle | downloaded
async function runUpdateAction() {
  if (updateStage === 'downloaded') {
    // 阶段二：完成升级 —— 主进程自改名替换并分离启动新版本，随后自动退出
    setUpdateBtn('✅ 完成升级');
    $('#btnUpdateDownload').disabled = true;
    const sb = $('#btnSettingsDownload');
    if (sb) sb.disabled = true;
    // 反馈必须显示在横幅上(设置弹窗可能关着,写 #updateMsg 会像"没反应")
    $('#updateBannerText').textContent = '🔄 正在替换并重启到新版本（旧版保留 .bak 备份，可回滚）…';
    if (sb) { sb.style.display = ''; sb.textContent = '🔄 正在替换重启…'; }
    let res;
    try { res = await window.easyssh.updateComplete(); }
    catch (e) { res = { ok: false, message: (e && e.message) || String(e) }; }
    if (!res || !res.ok) {
      setUpdateBtn('✅ 完成升级');
      const msg = '❌ 完成升级失败: ' + ((res && res.message) || '未知错误') + '（可到 updates\\ 文件夹手动运行新版本 exe）';
      $('#updateBannerText').textContent = msg;
      $('#updateMsg').textContent = msg;
      if (sb) sb.textContent = '✅ 完成升级';
    }
    return; // 成功时主进程会在 1.2 秒内自动退出，无需更多操作
  }
  // 阶段一：下载并校验（全程加密）
  if (!updateEntry) return;
  $('#btnUpdateDownload').disabled = true;
  const sb2 = $('#btnSettingsDownload');
  if (sb2) sb2.disabled = true;
  // 进度/失败信息必须显示在横幅上(设置弹窗可能关着,写 #updateMsg 会像"没反应")
  $('#updateBannerText').textContent = '⬇️ 正在下载加密安装包…(完成后自动校验)';
  $('#updateMsg').textContent = '开始下载(全程加密,下载后自动解密校验)…';
  const res = await window.easyssh.updateDownload(updateEntry);
  if (!res.ok) {
    setUpdateBtn('⬇️ 下载并校验');
    $('#updateBannerText').textContent = '❌ 下载失败: ' + (res.message || '未知错误') + '(也可到 easyssh.cn/download.html 手动下载)';
    $('#updateMsg').textContent = '下载失败: ' + (res.message || '未知错误') + '\n可手动下载: https://easyssh.cn/download.html';
    return;
  }
  updateStage = 'downloaded';
  $('#updateMsg').textContent = '新版本已下载并校验通过: ' + res.file + '\n点击「完成升级」自动替换重启';
  $('#updateBannerText').textContent = '✅ 新版本已下载并通过加密校验，点击「完成升级」自动替换重启（旧版自动备份，可回滚）';
  setUpdateBtn('✅ 完成升级');
}
$('#btnUpdateDownload').addEventListener('click', runUpdateAction);
// 设置中心里的下载/升级按钮（与提醒条按钮同一逻辑）
$('#btnSettingsDownload').addEventListener('click', runUpdateAction);
$('#btnUpdateSkip').addEventListener('click', () => {
  $('#updateBanner').style.display = 'none';
  updateStage = 'idle';
  setUpdateBtn('⬇️ 下载并校验');
  const sb = $('#btnSettingsDownload');
  if (sb) { sb.style.display = ''; sb.textContent = '⬇️ 下载并校验'; }
});
