const { app, BrowserWindow, ipcMain } = require('electron');
const path = require('path');
const fs = require('fs');
const { Client } = require('ssh2');
const iconv = require('iconv-lite');
const { initUpdater } = require('./updater'); // v2.5 在线升级(加密检测+加密下载)

let win = null;
const connections = new Map(); // id -> { conn, stream }

function baseDir() {
  if (process.env.PORTABLE_EXECUTABLE_DIR) return process.env.PORTABLE_EXECUTABLE_DIR;
  if (app.isPackaged) return path.dirname(app.getPath('exe'));
  return app.getAppPath();
}

function sessionsFile() {
  // 单文件portable版:配置存 exe 真实所在目录
  return path.join(baseDir(), 'sessions.json');
}

/* ---------- 设置(字体/编码/皮肤/快捷命令) ---------- */
const DEFAULT_SETTINGS = {
  fontFamily: 'Consolas',
  fontSize: 14,
  encoding: 'utf8',
  theme: 'dracula',
  // 默认常用快捷指令:用户未自定义过(设置里没有 quickCommands 字段)时提供,已自定义则完全尊重用户数据
  quickCommands: [
    { name: '系统信息', cmd: 'uname -a && cat /etc/os-release 2>/dev/null | head -2' },
    { name: '磁盘占用', cmd: 'df -h' },
    { name: '内存使用', cmd: 'free -h' },
    { name: '负载/运行时长', cmd: 'uptime' },
    { name: '监听端口', cmd: 'ss -tlnp' },
    { name: '耗资源进程', cmd: 'ps aux --sort=-%cpu | head -11' },
    { name: '最近登录', cmd: 'last -n 10' },
    { name: '网络连通', cmd: 'ping -c 4 223.5.5.5' },
    { name: '重启 sshd', cmd: 'systemctl restart sshd' },
    { name: '清屏', cmd: 'clear' },
  ],
};
/* ---------- 统一配置文件 config.json(会话 + 设置) ----------
   兼容迁移:首次运行自动读取旧 sessions.json / settings.json 并合并 */
function configFile() { return path.join(baseDir(), 'config.json'); }
function loadConfig() {
  let cfg = null;
  try { const j = JSON.parse(fs.readFileSync(configFile(), 'utf8')); if (j && typeof j === 'object') cfg = j; } catch {}
  if (!cfg) {
    // 首次运行:迁移旧 sessions.json / settings.json
    cfg = { sessions: [], settings: {} };
    try { const s = JSON.parse(fs.readFileSync(sessionsFile(), 'utf8')); if (Array.isArray(s)) cfg.sessions = s; } catch {}
    try { const st = JSON.parse(fs.readFileSync(path.join(baseDir(), 'settings.json'), 'utf8')); if (st && typeof st === 'object') cfg.settings = st; } catch {}
    try { saveConfig(cfg); } catch {}
  }
  cfg.sessions = Array.isArray(cfg.sessions) ? cfg.sessions : [];
  cfg.settings = { ...DEFAULT_SETTINGS, ...(cfg.settings || {}) };
  // 默认快捷指令播种:对 config.json 存在与否的两条路径都生效;仅注入一次并打标,
  // 已自定义过的(quickCommands 非空)或已注入过的完全不碰,删光也不复活
  const st = cfg.settings;
  if (!st.quickCommandsSeeded && (!Array.isArray(st.quickCommands) || st.quickCommands.length === 0)) {
    st.quickCommands = DEFAULT_SETTINGS.quickCommands.map((c) => ({ ...c }));
    st.quickCommandsSeeded = true;
  }
  return cfg;
}
function saveConfig(cfg) {
  fs.writeFileSync(configFile(), JSON.stringify({ sessions: cfg.sessions || [], settings: { ...DEFAULT_SETTINGS, ...(cfg.settings || {}) } }, null, 2), 'utf8');
}
function loadSettings() {
  return { ...DEFAULT_SETTINGS, ...(loadConfig().settings || {}) };
}
ipcMain.handle('settings:load', () => loadSettings());
ipcMain.handle('settings:save', (_e, s) => {
  const cfg = loadConfig();
  cfg.settings = { ...DEFAULT_SETTINGS, ...s };
  saveConfig(cfg);
  return true;
});

// 单实例:防止残留进程叠加导致文件/配置被占用
const gotLock = app.requestSingleInstanceLock();
if (!gotLock) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (win) { if (win.isMinimized()) win.restore(); win.focus(); }
  });
}

function createWindow() {
  win = new BrowserWindow({
    width: 1280,
    height: 820,
    minWidth: 900,
    minHeight: 600,
    title: 'EasySSH',
    backgroundColor: '#181825',
    titleBarStyle: 'hidden',
    titleBarOverlay: {
      color: '#181825',
      symbolColor: '#cdd6f4',
      height: 34,
    },
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
    },
  });
  win.setMenuBarVisibility(false);
  win.loadFile('index.html');
  // v2.5 在线升级:启动 3 秒后自动检测一次,运行期间每 1 小时检测一次
  try { initUpdater(win); } catch (e) {
    fs.appendFileSync(path.join(require('os').tmpdir(), 'easyssh-crash.log'),
      new Date().toISOString() + ' initUpdater: ' + (e && e.stack || String(e)) + '\n');
  }
}

/* 皮肤联动:标题栏(窗口控制按钮区)配色随主题更新 */
ipcMain.on('win:theme', (_e, { color, symbolColor }) => {
  try { if (win) win.setTitleBarOverlay({ color, symbolColor }); } catch {}
});

process.on('uncaughtException', (e) => {
  try {
    const os = require('os');
    fs.appendFileSync(path.join(os.tmpdir(), 'easyssh-crash.log'),
      new Date().toISOString() + '\n' + (e && e.stack || String(e)) + '\n\n');
  } catch {}
});
process.on('unhandledRejection', (e) => {
  try {
    const os = require('os');
    fs.appendFileSync(path.join(os.tmpdir(), 'easyssh-crash.log'),
      new Date().toISOString() + ' unhandledRejection\n' + (e && e.stack || String(e)) + '\n\n');
  } catch {}
});

app.whenReady().then(createWindow);
app.on('window-all-closed', () => { for (const { conn } of connections.values()) { try { conn.end(); } catch {} } app.quit(); });

/* ---------- sessions (portable,存于 config.json) ---------- */
ipcMain.handle('sessions:load', () => {
  return loadConfig().sessions || [];
});
ipcMain.handle('sessions:save', (_e, list) => {
  const cfg = loadConfig();
  cfg.sessions = list;
  saveConfig(cfg);
  return true;
});

/* ---------- ssh ---------- */
let seq = 0;
ipcMain.on('app:getVersion', (e) => { e.returnValue = app.getVersion(); });
ipcMain.handle('ssh:connect', (event, opts) => {
  return new Promise((resolve) => {
    const id = 's' + (++seq);
    const wc = event.sender;
    const conn = new Client();
    const rec = { conn, stream: null };
    connections.set(id, rec);

    let settled = false;
    const done = (ok, info) => {
      if (settled) return;
      settled = true;
      if (!ok) connections.delete(id); // 成功时保留连接供后续读写;失败才清理
      if (!ok) {
        writeDebugLog(info);
        // 把调试尾部直接带回界面,方便排查
        const tail = debugLines.slice(-12).join('\n');
        info = info + (tail ? '\n---- 调试信息 ----\n' + tail : '');
      }
      resolve({ id, ok, info });
    };

    // 调试日志:失败时写入 %TEMP%\easyssh-debug.log 便于排查
    const debugLines = [];
    let keyMeta = null; // key 认证时记录实际加载的私钥路径与指纹,失败时随报错带回
    // 主进程收到的参数回显:定位「渲染端选了密钥,主进程却走密码分支」这类参数丢失问题
    const recvNote = () => '[主进程收到] authtype=' + JSON.stringify(opts.authtype)
      + ', privateKey=' + (opts.privateKey === undefined ? 'undefined' : JSON.stringify(String(opts.privateKey).slice(0, 60)))
      + ', password=' + (opts.password === undefined ? 'undefined' : '(已提供,长度' + String(opts.password).length + ')')
      + ' ⇒ 走的分支: ' + (useKey ? 'key' : 'password');
    const writeDebugLog = (reason) => {
      try {
        const os = require('os');
        const f = path.join(os.tmpdir(), 'easyssh-debug.log');
        fs.appendFileSync(f, '===== ' + new Date().toISOString() + ' ' + opts.host + ':' + (opts.port || 22) + ' 失败: ' + reason + '\n'
          + debugLines.slice(-400).join('\n') + '\n\n');
      } catch {}
    };

    conn.on('ready', () => {
      // 自动启动该会话配置的端口转发(SecureCRT 式:随会话建立生效)
      for (const f of (opts.forwards || [])) {
        const localPort = Number(f.localPort) | 0, remotePort = Number(f.remotePort) | 0;
        if (!(localPort > 0 && remotePort > 0)) continue;
        const remoteHost = String(f.remoteHost || '').trim() || '127.0.0.1';
        const key = pfKey(id, localPort);
        if (pfServers.has(key)) continue;
        const server = net.createServer((sock) => {
          rec.conn.forwardOut(sock.remoteAddress || '127.0.0.1', sock.remotePort || 0, remoteHost, remotePort, (err, channel) => {
            if (err) { try { sock.destroy(); } catch {} pfNotify(wc, id, '端口转发 ' + localPort + ' 失败: ' + (err.message || err)); return; }
            channel.on('error', () => { try { sock.destroy(); } catch {} });
            sock.on('error', () => { try { channel.end(); } catch {} });
            channel.pipe(sock); sock.pipe(channel);
          });
        });
        server.on('error', (err) => {
          pfServers.delete(key);
          pfNotify(wc, id, err.code === 'EADDRINUSE' ? '端口转发: 本地端口 ' + localPort + ' 已被占用' : '端口转发监听失败: ' + (err.message || err));
        });
        server.listen(localPort, '127.0.0.1', () => {
          pfServers.set(key, { server, name: f.name || ('PF' + localPort), remoteHost, remotePort });
          pfNotify(wc, id, '端口转发已启动: 本机 127.0.0.1:' + localPort + ' → ' + remoteHost + ':' + remotePort + ' (经 ' + opts.host + ')');
        });
      }
      conn.shell({ term: 'xterm-256color', cols: opts.cols || 80, rows: opts.rows || 24 }, (err, stream) => {
        if (err) { try { conn.end(); } catch {} connections.delete(id); return done(false, String(err)); }
        rec.stream = stream;
        const dec = (buf) => {
          const enc = loadSettings().encoding || 'utf8';
          if (enc !== 'utf8' && iconv.encodingExists(enc)) return iconv.decode(buf, enc);
          return buf.toString('utf8');
        };
        stream.on('data', (d) => { if (!wc.isDestroyed()) wc.send('ssh:data', id, dec(d)); });
        stream.stderr.on('data', (d) => { if (!wc.isDestroyed()) wc.send('ssh:data', id, dec(d)); });
        stream.on('close', () => {
          connections.delete(id);
          stopAllForwards(id);
          if (!wc.isDestroyed()) wc.send('ssh:closed', id);
        });
        done(true, 'connected');
      });
    });
    conn.on('error', (err) => {
      let msg = err.message || String(err);
      if (/authentication methods failed/i.test(msg)) {
        msg += '\n' + recvNote();
        if (keyMeta) msg += '\n[本机已加载私钥] ' + keyMeta;
      }
      done(false, msg);
      if (!wc.isDestroyed()) wc.send('ssh:closed', id);
    });
    conn.on('end', () => { if (!rec.stream) done(false, 'connection ended'); });
    conn.on('close', () => { if (!rec.stream) done(false, 'connection closed'); });
    // 支持 keyboard-interactive 认证(部分服务器不开放 password 直登)
    conn.on('keyboard-interactive', (_name, _instr, _lang, _prompts, finish) => finish([opts.password || '']));

    const cfg = {
      debug: (m) => { debugLines.push(String(m).slice(0, 300)); },
      host: opts.host,
      port: opts.port || 22,
      username: opts.username,
      readyTimeout: 10000,
      keepaliveInterval: 10000,
      tryKeyboard: true,
    };
    // 密钥认证分支:authtype 明确为 key,或旧版保存的连接参数没带 authtype 但带了 privateKey
    // (v2.5.5 及之前断线重连丢 authtype 字段,密钥重连被误当密码处理)——绝不静默降级为空密码认证
    const useKey = opts.authtype === 'key' || (!opts.authtype && opts.privateKey && !opts.password);
    if (useKey) {
      // 密钥认证:路径未填时自动回退到默认密钥文件,绝不静默降级为空密码认证
      let keyPath = (opts.privateKey || '').trim();
      if (!keyPath) keyPath = path.join(baseDir(), 'keys', 'id_ed25519_easyssh');
      if (!fs.existsSync(keyPath)) {
        return done(false, '私钥文件不存在: ' + keyPath + '\n请先在连接面板点击「一键生成本机密钥对」');
      }
      try { cfg.privateKey = normalizePrivateKeyPem(fs.readFileSync(keyPath, 'utf8'), opts.passphrase || undefined); }
      catch (e) { return done(false, '无法读取私钥: ' + e.message); }
      if (opts.passphrase) cfg.passphrase = opts.passphrase;
      // 计算与 ssh-keygen -lf 一致的公钥指纹(取同目录 .pub 的 wire blob 做 SHA256),
      // 失败时报错直接带回,用于和服务器 authorized_keys 指纹比对
      let fp = null;
      try {
        const parts = fs.readFileSync(keyPath + '.pub', 'utf8').trim().split(/\s+/);
        if (parts[1]) fp = 'SHA256:' + crypto.createHash('sha256').update(Buffer.from(parts[1], 'base64')).digest('base64').replace(/=+$/, '');
      } catch {}
      keyMeta = keyPath + (fp ? ' (指纹: ' + fp + ')' : ' (指纹未知: 未找到 ' + keyPath + '.pub)');
    } else {
      cfg.password = opts.password || '';
    }
    try {
      conn.connect(cfg);
    } catch (e) {
      done(false, String(e && e.message || e));
    }
  });
});

ipcMain.on('ssh:write', (_e, id, data) => {
  const c = connections.get(id);
  if (c && c.stream) c.stream.write(data);
});
ipcMain.on('ssh:resize', (_e, id, cols, rows) => {
  const c = connections.get(id);
  if (c && c.stream) c.stream.setWindow(rows, cols, 0, 0);
});
ipcMain.on('ssh:disconnect', (_e, id) => {
  const c = connections.get(id);
  if (c) { try { c.conn.end(); } catch {} connections.delete(id); }
  stopAllForwards(id);
});

/* ---------- 本地端口转发(SecureCRT 会话选项→连接→端口转发 同款业务) ----------
 * 用途:SSH 会话建立后,本机监听一个端口;连到该端口的流量经加密隧道由 SSH 服务器
 * 转发到"目标主机:端口"(目标默认就是 SSH 服务器本身,也可指向内网其他机器),
 * 用于安全访问内网数据库/内网 Web 管理台等只有服务器能到达的服务。
 * 实现:本地 net.Server 监听 127.0.0.1:localPort → 每个进来的连接调
 * ssh2 的 forwardOut 开一条 forwarded-tcpip 通道 → 与本地 socket 对接。
 * 会话断开自动停掉该连接的所有监听。 */
const net = require('net');
const pfServers = new Map(); // key: connId:localPort -> { server, name, remoteHost, remotePort }
const pfKey = (id, port) => id + ':' + port;

function pfNotify(wc, id, message) {
  try { if (wc && !wc.isDestroyed()) wc.send('pf:event', { id, message }); } catch {}
}

function stopAllForwards(id) {
  for (const [key, item] of pfServers) {
    if (key.startsWith(id + ':')) {
      try { item.server.close(); } catch {}
      pfServers.delete(key);
    }
  }
}

ipcMain.handle('pf:start', (e, id, f) => {
  return new Promise((resolve) => {
    const c = connections.get(id);
    if (!c) return resolve({ ok: false, message: '连接不存在或已断开' });
    const localPort = Number(f.localPort) | 0;
    const remotePort = Number(f.remotePort) | 0;
    if (!(localPort > 0 && localPort < 65536) || !(remotePort > 0 && remotePort < 65536)) {
      return resolve({ ok: false, message: '端口必须是 1-65535 的数字' });
    }
    const remoteHost = String(f.remoteHost || '').trim() || '127.0.0.1'; // 留空=SSH 服务器自己
    const key = pfKey(id, localPort);
    if (pfServers.has(key)) return resolve({ ok: false, message: '本连接已在监听本地端口 ' + localPort });
    const server = net.createServer((sock) => {
      c.conn.forwardOut(sock.remoteAddress || '127.0.0.1', sock.remotePort || 0, remoteHost, remotePort, (err, channel) => {
        if (err) {
          try { sock.destroy(); } catch {}
          pfNotify(e.sender, id, '端口转发 ' + localPort + ' → ' + remoteHost + ':' + remotePort + ' 失败: ' + (err.message || err));
          return;
        }
        channel.on('error', () => { try { sock.destroy(); } catch {} });
        sock.on('error', () => { try { channel.end(); } catch {} });
        channel.pipe(sock);
        sock.pipe(channel);
      });
    });
    server.on('error', (err) => {
      pfServers.delete(key);
      const msg = err.code === 'EADDRINUSE'
        ? '本地端口 ' + localPort + ' 已被其他程序占用'
        : '本地监听失败: ' + (err.message || err);
      pfNotify(e.sender, id, msg);
      if (!server.listening) resolve({ ok: false, message: msg });
    });
    server.listen(localPort, '127.0.0.1', () => {
      pfServers.set(key, { server, name: f.name || ('PF' + localPort), remoteHost, remotePort });
      resolve({ ok: true, name: f.name || ('PF' + localPort), localPort, remoteHost, remotePort });
    });
  });
});

ipcMain.handle('pf:stop', (_e, id, localPort) => {
  const key = pfKey(id, Number(localPort) | 0);
  const item = pfServers.get(key);
  if (!item) return { ok: false, message: '该端口没有正在运行的转发' };
  try { item.server.close(); } catch {}
  pfServers.delete(key);
  return { ok: true };
});

ipcMain.handle('pf:list', (_e, id) => {
  const out = [];
  for (const [key, item] of pfServers) {
    if (key.startsWith(id + ':')) out.push({ name: item.name, localPort: Number(key.split(':')[1]), remoteHost: item.remoteHost, remotePort: item.remotePort });
  }
  return out;
});

/* ---------- SFTP ---------- */
const { dialog } = require('electron');

function getSftp(id) {
  return new Promise((resolve, reject) => {
    const c = connections.get(id);
    if (!c) return reject(new Error('连接不存在或已断开'));
    if (c.sftp) return resolve(c.sftp);
    c.conn.sftp((err, sftp) => {
      if (err) return reject(err);
      c.sftp = sftp;
      resolve(sftp);
    });
  });
}

ipcMain.handle('sftp:list', async (_e, id, dir) => {
  const sftp = await getSftp(id);
  return new Promise((resolve, reject) => {
    sftp.readdir(dir || '.', (err, list) => {
      if (err) return reject(err);
      const items = list.map((it) => ({
        name: it.filename,
        type: it.longname.startsWith('d') ? 'dir' : 'file',
        size: it.attrs.size,
        mtime: it.attrs.mtime * 1000,
      })).sort((a, b) => (a.type === b.type ? a.name.localeCompare(b.name) : a.type === 'dir' ? -1 : 1));
      resolve(items);
    });
  });
});

ipcMain.handle('sftp:pickSave', async (_e, defaultName) => {
  const r = await dialog.showSaveDialog(win, { defaultPath: defaultName });
  return r.canceled ? null : r.filePath;
});
ipcMain.handle('sftp:pickOpen', async () => {
  const r = await dialog.showOpenDialog(win, { properties: ['openFile', 'multiSelections'] });
  return r.canceled ? [] : r.filePaths;
});

ipcMain.handle('sftp:download', async (_e, id, remotePath, localPath, opts) => {
  const sftp = await getSftp(id);
  await new Promise((resolve, reject) => {
    sftp.fastGet(remotePath, localPath, (err) => err ? reject(err) : resolve());
  });
  // 文本转换模式:服务器编码(设置) -> 本地 UTF-8,并统一换行为 CRLF
  if (opts && opts.textConvert) {
    const enc = loadSettings().encoding || 'utf8';
    if (enc !== 'utf8' && iconv.encodingExists(enc)) {
      try {
        const text = iconv.decode(fs.readFileSync(localPath), enc);
        fs.writeFileSync(localPath, text.replace(/\r?\n/g, '\r\n'), 'utf8');
      } catch {}
    }
  }
  return localPath;
});
ipcMain.handle('sftp:upload', async (_e, id, localPath, remotePath, opts) => {
  const sftp = await getSftp(id);
  let putPath = localPath;
  let tmp = null;
  // 文本转换模式:本地 UTF-8 -> 服务器编码(设置),换行统一为 LF
  if (opts && opts.textConvert) {
    const enc = loadSettings().encoding || 'utf8';
    if (enc !== 'utf8' && iconv.encodingExists(enc)) {
      try {
        const text = fs.readFileSync(localPath, 'utf8');
        tmp = localPath + '.easyssh-tmp';
        fs.writeFileSync(tmp, iconv.encode(text.replace(/\r\n/g, '\n'), enc));
        putPath = tmp;
      } catch {}
    }
  }
  try {
    await new Promise((resolve, reject) => {
      sftp.fastPut(putPath, remotePath, (err) => err ? reject(err) : resolve());
    });
  } finally {
    if (tmp) { try { fs.unlinkSync(tmp); } catch {} }
  }
  return remotePath;
});

/* SFTP 断开清理 */
const _origDisconnect = null; // 连接关闭时 sftp 随 conn 一起失效,无需单独清理

/* ---------- 密钥登录:一键生成/自动安装 ---------- */
const crypto = require('crypto');
const os = require('os');

function sshStr(buf) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(buf.length);
  return Buffer.concat([len, buf]);
}

// 把 ed25519 私钥(Node KeyObject)编码为 OpenSSH "openssh-key-v1" 未加密 PEM。
// 原因:ssh2 的 keyParser 不支持 PKCS#8("BEGIN PRIVATE KEY")PEM,只认
// OpenSSH 格式 / 旧式 PKCS#1 PEM / PPK,否则报 "Unsupported key format"。
function ed25519OpenSSHPem(privKeyObj, pubKeyObj, comment) {
  const pkcs8 = privKeyObj.export({ type: 'pkcs8', format: 'der' });
  const seed = pkcs8.subarray(pkcs8.length - 32); // PKCS#8 尾部即 32 字节种子
  const spki = pubKeyObj.export({ type: 'spki', format: 'der' });
  const pubRaw = spki.subarray(spki.length - 32);
  const keyType = Buffer.from('ssh-ed25519');
  const pubBlob = Buffer.concat([sshStr(keyType), sshStr(pubRaw)]);
  const chk = crypto.randomBytes(4);
  let priv = Buffer.concat([
    chk, chk,
    sshStr(keyType),
    sshStr(pubRaw),
    sshStr(Buffer.concat([seed, pubRaw])), // OpenSSH 里 ed25519 私钥段为 seed+pub 共 64 字节
    sshStr(Buffer.from(comment || '', 'utf8')),
  ]);
  const padLen = 8 - (priv.length % 8); // 按 8 字节块补 1..8 填充
  const pad = Buffer.alloc(padLen);
  for (let i = 0; i < padLen; i++) pad[i] = i + 1;
  priv = Buffer.concat([priv, pad]);
  const head = Buffer.concat([
    Buffer.from('openssh-key-v1\0'),
    sshStr(Buffer.from('none')),          // ciphername
    sshStr(Buffer.from('none')),          // kdf
    sshStr(Buffer.alloc(0)),              // kdfoptions
    (() => { const n = Buffer.alloc(4); n.writeUInt32BE(1); return n; })(), // numkeys
    sshStr(pubBlob),
    sshStr(priv),
  ]);
  const b64 = head.toString('base64');
  const lines = b64.match(/.{1,70}/g) || [];
  return '-----BEGIN OPENSSH PRIVATE KEY-----\n' + lines.join('\n') + '\n-----END OPENSSH PRIVATE KEY-----\n';
}

// 连接前把用户给的私钥规范化为 ssh2 可解析的格式。
// 兼容历史版本生成过的 PKCS#8 文件:ed25519→OpenSSH,RSA→PKCS#1,EC→SEC1。
function normalizePrivateKeyPem(content, passphrase) {
  let ok = true;
  try {
    const parsed = require('ssh2').utils.parseKey(content, passphrase);
    if (parsed instanceof Error) ok = false;
  } catch { ok = false; }
  if (ok) return content;
  try {
    const keyObj = passphrase ? crypto.createPrivateKey({ key: content, passphrase }) : crypto.createPrivateKey(content);
    const t = keyObj.asymmetricKeyType;
    if (t === 'ed25519') {
      const pub = crypto.createPublicKey(keyObj);
      return ed25519OpenSSHPem(keyObj, pub, '');
    }
    if (t === 'rsa') return keyObj.export({ type: 'pkcs1', format: 'pem' });
    if (t === 'ec') return keyObj.export({ type: 'sec1', format: 'pem' });
  } catch {}
  return content; // 交给 ssh2 原生报错
}

// 生成本机 Ed25519 密钥对:私钥 OpenSSH 格式 PEM(ssh2 直接可用),公钥 OpenSSH 一行格式
// 复用逻辑:keys/ 下已有密钥对时直接返回,避免重新生成导致服务器 authorized_keys 与新公钥不匹配
ipcMain.handle('key:generate', () => {
  const dir = path.join(baseDir(), 'keys');
  fs.mkdirSync(dir, { recursive: true });
  const privPath = path.join(dir, 'id_ed25519_easyssh');
  const pubPath = privPath + '.pub';
  if (fs.existsSync(privPath) && fs.existsSync(pubPath)) {
    const pubLine = fs.readFileSync(pubPath, 'utf8').trim();
    if (pubLine.startsWith('ssh-ed25519 ')) return { privPath, pubPath, pubLine, reused: true };
  }
  const { publicKey, privateKey } = crypto.generateKeyPairSync('ed25519');
  const spki = publicKey.export({ type: 'spki', format: 'der' });
  const raw = spki.subarray(spki.length - 32); // ed25519 公钥原始 32 字节
  const keyType = Buffer.from('ssh-ed25519');
  const blob = Buffer.concat([sshStr(keyType), sshStr(raw)]);
  const comment = 'easyssh-' + os.hostname() + '-' + new Date().toISOString().slice(0, 10);
  const pubLine = 'ssh-ed25519 ' + blob.toString('base64') + ' ' + comment;

  // ssh2 不解析 PKCS#8,必须写 OpenSSH 格式私钥
  fs.writeFileSync(privPath, ed25519OpenSSHPem(privateKey, publicKey, comment), { mode: 0o600 });
  fs.writeFileSync(pubPath, pubLine + '\n');
  return { privPath, pubPath, pubLine };
});

// 利用当前已连接的会话,把公钥自动追加到服务器 authorized_keys(幂等:已存在不重复写)
// 注意:除写入公钥外,还必须修正权限/上下文,否则 sshd StrictModes 会静默拒绝密钥,
// 表现为「安装成功」后密钥登录仍报 All configured authentication methods failed:
//  1) 家目录不能组/其他可写(sshd StrictModes 会检查 ~ 与 ~/.ssh);
//  2) SELinux 系统(CentOS/RHEL)需 restorecon 恢复 ~/.ssh 的 ssh_home_t 上下文;
//  3) 安装后回读校验,若服务器关闭了公钥认证(PubkeyAuthentication no)直接给出明确提示。
ipcMain.handle('key:install', (_e, id, pubLine) => {
  return new Promise((resolve, reject) => {
    const c = connections.get(id);
    if (!c) return reject(new Error('无活动连接,请先用密码连接一次'));
    const q = "'" + String(pubLine).replace(/'/g, "'" + String.fromCharCode(92) + "''") + "'";
    // 目标文件按服务器 sshd 真实配置决定:默认 ~/.ssh/authorized_keys;
    // 若 sshd -T 报告了非默认 AuthorizedKeysFile(如 /etc/ssh/keys/%u),写到它声明的路径,否则公钥写了也无效。
    const cmd =
      'AK=$(sshd -T 2>/dev/null | awk \'tolower($1)=="authorizedkeysfile"{print $2; exit}\');'
      + ' case "$AK" in ""|".ssh/authorized_keys") F="$HOME/.ssh/authorized_keys";;'
      + ' *) F=$(echo "$AK" | sed -e "s/%u/$(id -un 2>/dev/null)/" -e "s|%h|$HOME|");; esac;'
      + ' mkdir -p "$(dirname "$F")"'
      + ' && case "$F" in "$HOME"/*) chmod 700 "$(dirname "$F")";; esac;'
      + ' chmod g-w,o-w "$HOME" 2>/dev/null;'
      + ' touch "$F" && chmod 600 "$F";'
      + ' (command -v restorecon >/dev/null 2>&1 && restorecon -R "$(dirname "$F")" "$HOME" 2>/dev/null || true);'
      + ' (grep -qF ' + q + ' "$F" 2>/dev/null || echo ' + q + ' >> "$F");'
      // 回读校验:公钥确实写入,并顺带收集 sshd 公钥认证相关配置
      + ' echo EASYSSH_OK:$(grep -cF ' + q + ' "$F" 2>/dev/null);'
      + ' echo EASYSSH_FILE:$F;'
      + ' (sshd -T 2>/dev/null | grep -iE "^(pubkeyauthentication|strictmodes|authorizedkeysfile|permitrootlogin)" || true)';
    c.conn.exec(cmd, (err, stream) => {
      if (err) return reject(err);
      let out = '';
      stream.on('data', (d) => { out += d; });
      stream.stderr.on('data', (d) => { out += d; });
      stream.on('close', (code) => {
        if (code !== 0) return reject(new Error(out.trim() || ('exit ' + code)));
        if (!/EASYSSH_OK:[1-9]/.test(out)) {
          return reject(new Error('公钥写入后回读校验失败(公钥不在服务器 authorized_keys 中)' + (out.trim() ? '\n' + out.trim() : '')));
        }
        const fm = out.match(/^EASYSSH_FILE:(.+)$/m);
        const file = fm ? fm[1].trim() : '~/.ssh/authorized_keys';
        const m = out.match(/^pubkeyauthentication\s+(\S+)/mi);
        if (m && /^no$/i.test(m[1])) {
          return reject(new Error('服务器 sshd 已关闭公钥认证(PubkeyAuthentication no),公钥已写入 ' + file + ' 但无法生效。'
            + '\n请在服务器上修改 /etc/ssh/sshd_config:把「PubkeyAuthentication」设为 yes(或删除该行),再执行 systemctl restart sshd'));
        }
        resolve('公钥已写入服务器 ' + file + '(按 sshd 实际 AuthorizedKeysFile 配置,家目录权限/SELinux 上下文已自动修正,回读校验通过)');
      });
    });
  });
});

// 密钥登录失败时的服务器端自动诊断:借助当前仍在线的密码会话,把 sshd 真实配置、
// 权限/属主、SELinux、authorized_keys 内容与 sshd 日志里的拒绝原因取回来,
// 让客户端能给出「到底是哪一条原因」而不是让用户反复盲试。
// 全部命令走 POSIX sh / busybox 兼容写法,逐段容错,任何一段失败不影响其余输出。
ipcMain.handle('key:diagnose', (_e, id, pubLine) => {
  return new Promise((resolve, reject) => {
    const c = connections.get(id);
    if (!c) return reject(new Error('无活动连接,无法诊断。请先用密码方式连上服务器再试'));
    // pubLine 为空(如 App 重启后 generatedPub 丢失)时回退读取默认公钥文件,
    // 否则 grep -cF '' 会匹配所有行,导致「公钥是否存在」检查假阳性
    if (!pubLine) {
      try { pubLine = fs.readFileSync(path.join(baseDir(), 'keys', 'id_ed25519_easyssh.pub'), 'utf8').trim(); } catch {}
    }
    const q = "'" + String(pubLine || '').replace(/'/g, "'" + String.fromCharCode(92) + "''") + "'";
    const cmd = [
      'echo "@@WHO"; id -un 2>/dev/null; echo "home=$HOME"',
      'AK=$(sshd -T 2>/dev/null | awk \'tolower($1)=="authorizedkeysfile"{print $2; exit}\'); case "$AK" in ""|".ssh/authorized_keys") F="$HOME/.ssh/authorized_keys";; *) F=$(echo "$AK" | sed -e "s/%u/$(id -un 2>/dev/null)/" -e "s|%h|$HOME|");; esac; echo "akfile=$F"',
      'echo "@@PERMS"',
      'ls -ld "$HOME" "$(dirname "$F")" "$F" 2>&1',
      'echo "@@SELINUX"',
      '(command -v getenforce >/dev/null 2>&1 && getenforce; ls -Zd "$(dirname "$F")" 2>/dev/null) || echo no-selinux',
      'echo "@@SSHD_CONFIG"',
      'if command -v sshd >/dev/null 2>&1; then sshd -T 2>/dev/null | grep -iE "^(pubkeyauthentication|strictmodes|authorizedkeysfile|permitrootlogin|authenticationmethods|pubkeyacceptedalgorithms|allowusers|denyusers)"; else echo "sshd-binary-not-found"; fi',
      'command -v dropbear >/dev/null 2>&1 && echo "dropbear-present"',
      'echo "@@KEY_PRESENT"',
      'grep -cF ' + q + ' "$F" 2>/dev/null || echo 0',
      'echo "@@AUTHKEYS_TAIL"',
      'tail -5 "$F" 2>/dev/null | cut -c1-80',
      'echo "@@AUTHKEYS_FP"',
      'ssh-keygen -lf "$F" 2>/dev/null | tail -5 || echo fingerprint-tool-unavailable',
      'echo "@@SSHD_LOG"',
      // 只留拒绝/错误类记录(密钥被拒必含 Failed publickey),去掉 root 匹配,
      // 避免 root 的密码登录日志把真正的密钥拒绝记录挤出 tail 窗口
      '(tail -200 /var/log/secure 2>/dev/null; tail -200 /var/log/auth.log 2>/dev/null; tail -200 /var/log/messages 2>/dev/null; journalctl -u sshd -u ssh -n 200 --no-pager 2>/dev/null) | grep -iE "sshd|dropbear" | grep -iE "publickey|refused|denied|error|bad ownership" | grep -viE "accepted" | tail -12',
      'echo "@@END"',
    ].join('; ');
    c.conn.exec(cmd, (err, stream) => {
      if (err) return reject(err);
      let out = '';
      stream.on('data', (d) => { out += d; });
      stream.stderr.on('data', (d) => { out += d; });
      stream.on('close', () => resolve(out));
    });
  });
});
