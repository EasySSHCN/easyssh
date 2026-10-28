/* global Terminal, FitAddon */
const $ = (s) => document.querySelector(s);
// tab: { id, term, fit, el, name, opts, disconnected }
const tabs = new Map();
let activeId = null;
let sessions = [];
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
    scrollback: 5000,
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit);
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
  return tab;
}

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
  el.addEventListener('click', (e) => {
    if (e.target.classList.contains('x')) closeTab(tab.id);
    else activate(tab.id);
  });
  tab.tabEl = el;
  $('#tabbar').appendChild(el);
}

/* ---------- connect / reconnect ---------- */
async function doConnect(tab, opts) {
  const res = await window.easyssh.connect({ ...opts, cols: 80, rows: 24 });
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
  const eexisting = sessions.find((s) => ekey(s) === ekey(opts));
  const sessionForwards = (eexisting && Array.isArray(eexisting.forwards)) ? eexisting.forwards : []; // 随会话自动启动的端口转发
  const connectWithTimeout = Promise.race([
    window.easyssh.connect({
      host: opts.host,
      port: Number(opts.port) || 22,
      username: opts.username,
      forwards: sessionForwards,
      password: opts.authtype === 'password' ? opts.password : undefined,
      privateKey: opts.authtype === 'key' ? opts.privateKey : undefined,
      passphrase: opts.authtype === 'key' ? opts.passphrase : undefined,
      cols: 80, rows: 24,
    }),
    new Promise((resolve) => setTimeout(() => resolve({ ok: false, info: '连接超时(30秒无响应)' }), 30000)),
  ]);
  let res;
  try {
    res = await connectWithTimeout;
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
    const existing = sessions.findIndex((s) => key(s) === nk);
    const rec = { host: opts.host, port: opts.port, username: opts.username, authtype: opts.authtype };
    if (opts.authtype === 'password') {
      rec.password = opts.password;
    } else {
      rec.privateKey = opts.privateKey;
      rec.passphrase = opts.passphrase || '';
    }
    if (existing >= 0) sessions[existing] = rec; else sessions.push(rec);
    rec.forwards = sessionForwards; // 端口转发配置随会话持久化
    await window.easyssh.saveSessions(sessions);
    renderSessions();
  }

  const tab = { id: res.id, name, opts: {
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
function renderSessions() {
  const ul = $('#sessionList');
  ul.innerHTML = '';
  sessions.forEach((s, i) => {
    const li = document.createElement('li');
    li.innerHTML = '<span>' + s.username + '@' + s.host + '</span><span class="del" title="删除">🗑</span>';
    li.addEventListener('click', (e) => {
      if (e.target.classList.contains('del')) {
        sessions.splice(i, 1);
        window.easyssh.saveSessions(sessions);
        renderSessions();
        return;
      }
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
    });
    ul.appendChild(li);
  });
}

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
let appSettings = { fontFamily: 'Consolas', fontSize: 14, encoding: 'utf8', theme: 'dracula', quickCommands: [] };

function applyTermSettings() {
  const t = THEMES[appSettings.theme] || THEMES.dracula;
  for (const tab of tabs.values()) {
    tab.term.options.fontFamily = appSettings.fontFamily + ', monospace';
    tab.term.options.fontSize = Number(appSettings.fontSize) || 14;
    tab.term.options.theme = t.term;
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
  $('#settingsModal').classList.remove('hidden');
});
$('#setSave').addEventListener('click', async () => {
  appSettings.fontFamily = $('#setFont').value;
  appSettings.fontSize = Number($('#setSize').value) || 14;
  appSettings.encoding = $('#setEnc').value;
  appSettings.theme = $('#setTheme').value;
  await window.easyssh.saveSettings(appSettings);
  applyTermSettings();
  $('#settingsModal').classList.add('hidden');
  $('#statusBar').textContent = '设置已保存' + (appSettings.encoding !== 'utf8' ? '(编码对新连接生效)' : '');
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
  renderPfList();
  $('#pfModal').classList.remove('hidden');
});
async function renderPfList() {
  const ul = $('#pfList');
  ul.innerHTML = '';
  const list = await window.easyssh.pfList(activeId);
  if (!list.length) { const li = document.createElement('li'); li.textContent = '(暂无正在运行的转发)'; ul.appendChild(li); return; }
  list.forEach((f) => {
    const li = document.createElement('li');
    li.innerHTML = '<b>' + f.name + '</b><span class="qc">本机 127.0.0.1:' + f.localPort + ' → ' + f.remoteHost + ':' + f.remotePort + '</span><span class="del">✕</span>';
    li.querySelector('.del').addEventListener('click', async () => {
      await window.easyssh.pfStop(activeId, f.localPort);
      renderPfList();
    });
    ul.appendChild(li);
  });
}
$('#pfAdd').addEventListener('click', async () => {
  const t = tabs.get(activeId);
  if (!t || t.disconnected) return;
  const cfg = {
    name: $('#pfName').value.trim() || ('PF' + $('#pfLocalPort').value.trim()),
    localPort: Number($('#pfLocalPort').value),
    remoteHost: $('#pfRemoteHost').value.trim(),
    remotePort: Number($('#pfRemotePort').value),
  };
  const res = await window.easyssh.pfStart(activeId, cfg);
  if (!res.ok) { $('#statusBar').textContent = '端口转发失败: ' + (res.message || '未知错误'); return; }
  // 配置随会话持久化:重连时自动重启,并写回已保存会话
  t.opts.forwards = t.opts.forwards || [];
  if (!t.opts.forwards.some((x) => Number(x.localPort) === cfg.localPort)) t.opts.forwards.push(cfg);
  const nk = t.opts.host + ':' + (t.opts.port || 22) + ':' + t.opts.username;
  const rec = sessions.find((s) => s.host + ':' + (s.port || 22) + ':' + s.username === nk);
  if (rec) { rec.forwards = t.opts.forwards; await window.easyssh.saveSessions(sessions); }
  $('#pfName').value = ''; $('#pfLocalPort').value = ''; $('#pfRemoteHost').value = ''; $('#pfRemotePort').value = '';
  t.term.write('\x1b[36m[端口转发已启动: 本机 127.0.0.1:' + cfg.localPort + ' → ' + (cfg.remoteHost || '127.0.0.1') + ':' + cfg.remotePort + ']\x1b[0m\r\n');
  renderPfList();
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
$('#sftpUpload').addEventListener('click', async () => {
  const t = tabs.get(activeId);
  if (!t || t.disconnected) return;
  const files = await window.easyssh.pickOpen();
  if (!files.length) return;
  const dir = $('#sftpPath').value.trim() || '/root';
  for (const f of files) {
    const remote = sftpAbs(dir, f.split('\\').pop());
    $('#sftpMsg').textContent = '上传中: ' + f;
    try { await window.easyssh.sftpUpload(t.id, f, remote, { textConvert: sftpTextMode() }); $('#sftpMsg').textContent = '已上传: ' + remote; }
    catch (e) { $('#sftpMsg').textContent = '上传失败: ' + (e.message || e); }
  }
  sftpBrowse(dir);
});
// 双击文件 = 下载
document.addEventListener('dblclick', async (e) => {
  const row = e.target.closest('#sftpList .frow');
  if (!row || !sftpSel || sftpSel.type !== 'file') return;
  const t = tabs.get(activeId);
  if (!t || t.disconnected) return;
  const local = await window.easyssh.pickSave(sftpSel.name);
  if (!local) return;
  $('#sftpMsg').textContent = '下载中: ' + sftpSel.name;
  try {
    await window.easyssh.sftpDownload(t.id, sftpSel.path, local, { textConvert: sftpTextMode() });
    $('#sftpMsg').textContent = '已下载: ' + local;
  } catch (err) { $('#sftpMsg').textContent = '下载失败: ' + (err.message || err); }
});

/* ---------- 弹窗关闭 ---------- */
document.querySelectorAll('.close-x').forEach((b) => b.addEventListener('click', () => {
  $('#' + b.dataset.close).classList.add('hidden');
}));
document.querySelectorAll('.modal').forEach((m) => m.addEventListener('click', (e) => {
  if (e.target === m) m.classList.add('hidden');
}));

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
  const key = (s) => s.host + ':' + (s.port || 22) + ':' + s.username;
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
  if (r && r.status === 'available') {
    updateEntry = r.file;
    $('#updateBannerText').textContent =
      `🔔 发现新版本 v${r.latestVersion}(当前 v${(r.currentVersion || '').replace(/^v/, '')}),已通过加密接口校验`;
    bar.style.display = 'flex';
  } else {
    bar.style.display = 'none';
  }
}

// 自动检测结果 / 下载进度事件推送
function setUpdateBtn(label) {
  const b = $('#btnUpdateDownload');
  if (b) { b.textContent = label; b.disabled = false; }
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

// 提醒条按钮：两阶段 —— 未下载时是「下载并校验」；下载完成后变为「✅ 完成升级」(原地换名替换重启)
let updateStage = 'idle'; // idle | downloaded
$('#btnUpdateDownload').addEventListener('click', async () => {
  if (updateStage === 'downloaded') {
    // 阶段二：完成升级 —— 主进程自改名替换并分离启动新版本，随后自动退出
    $('#btnUpdateDownload').disabled = true;
    // 反馈必须显示在横幅上(设置弹窗可能关着,写 #updateMsg 会像"没反应")
    $('#updateBannerText').textContent = '🔄 正在替换并重启到新版本（旧版保留 .bak 备份，可回滚）…';
    let res;
    try { res = await window.easyssh.updateComplete(); }
    catch (e) { res = { ok: false, message: (e && e.message) || String(e) }; }
    if (!res || !res.ok) {
      setUpdateBtn('✅ 完成升级');
      const msg = '❌ 完成升级失败: ' + ((res && res.message) || '未知错误') + '（可到 updates\\ 文件夹手动运行新版本 exe）';
      $('#updateBannerText').textContent = msg;
      $('#updateMsg').textContent = msg;
    }
    return; // 成功时主进程会在 1.2 秒内自动退出，无需更多操作
  }
  // 阶段一：下载并校验（全程加密）
  if (!updateEntry) return;
  $('#btnUpdateDownload').disabled = true;
  // 进度/失败信息必须显示在横幅上(设置弹窗可能关着,写 #updateMsg 会像"没反应")
  $('#updateBannerText').textContent = '⬇️ 正在下载加密安装包…(完成后自动校验)';
  $('#updateMsg').textContent = '开始下载(全程加密,下载后自动解密校验)…';
  const res = await window.easyssh.updateDownload(updateEntry);
  if (!res.ok) {
    setUpdateBtn('下载并校验');
    $('#updateBannerText').textContent = '❌ 下载失败: ' + (res.message || '未知错误') + '(也可到 easyssh.cn/download.html 手动下载)';
    $('#updateMsg').textContent = '下载失败: ' + (res.message || '未知错误') + '\n可手动下载: https://easyssh.cn/download.html';
    return;
  }
  updateStage = 'downloaded';
  $('#updateMsg').textContent = '新版本已下载并校验通过: ' + res.file;
});
$('#btnUpdateSkip').addEventListener('click', () => {
  $('#updateBanner').style.display = 'none';
  updateStage = 'idle';
  setUpdateBtn('下载并校验');
});
