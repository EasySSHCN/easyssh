/**
 * EasySSH 在线升级模块（主进程，零新增依赖）
 *
 * ⚠️ 脱敏说明（SANITIZED）：
 *   本文件对外随源码发布，不含任何真实密钥。真实主密钥位于 exe 同目录的
 *   update-keys.json（打包时随包携带，但不进入对外发布的源码 zip）；
 *   对外发布的脱敏占位样例见 update-keys.example.json。
 *
 * 行为约定（与官网 /api/update/check 加密接口配套）：
 *   - 打开软件后自动检测一次；之后运行期间每 1 小时自动检测一次
 *   - 设置中心可手工「检查更新」
 *   - 检测与下载全程加密（AES-256-GCM 信封 + AES-256-CTR/HMAC-SHA256 安装包）
 *   - 在线升级渠道只服务 v2.5.0 及以上版本（MIN_UPGRADE_VERSION）
 *   - 便携版采用「下载校验后提示用户运行新 exe」的替换方式，绝不静默自替换
 */
'use strict';

const { app, ipcMain } = require('electron');
const path = require('path');
const os = require('os');
const fs = require('fs');
const crypto = require('crypto');
const https = require('https');
const { spawn } = require('child_process');

/* ---------- 与网站端 tools/update-crypto.js 相同的协议常量与实现 ---------- */
const MAGIC = Buffer.from('ESSHENC1', 'ascii');
const CHECK_SALT = 'easyssh-update-check-v1';
const FILE_SALT = 'easyssh-update-file-v1';
const MIN_UPGRADE_VERSION = '2.5.0'; // 在线升级版本号从 2.5 开始
const API_PATH = '/api/update/check';
const CHECK_INTERVAL_MS = 60 * 60 * 1000; // 1 小时

function hkdf(master, salt, info, len) {
  return crypto.hkdfSync('sha256', master, Buffer.from(salt, 'utf8'), Buffer.from(info, 'utf8'), len);
}
function deriveCheckKeys(masterBuf) {
  return {
    encKey: Buffer.from(hkdf(masterBuf, CHECK_SALT, 'aes-256-gcm', 32)),
    macKey: Buffer.from(hkdf(masterBuf, CHECK_SALT, 'hmac-sha256', 32)),
  };
}
function deriveFileKeys(masterBuf, fileName, plainSha256Hex) {
  const info = `${fileName}|${plainSha256Hex}`;
  return {
    encKey: Buffer.from(hkdf(masterBuf, FILE_SALT, `enc:${info}`, 32)),
    macKey: Buffer.from(hkdf(masterBuf, FILE_SALT, `mac:${info}`, 32)),
  };
}
function cmpVersion(a, b) {
  const pa = String(a).split('.'), pb = String(b).split('.');
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const x = pa[i] ?? '', y = pb[i] ?? '';
    const nx = /^\d+$/.test(x), ny = /^\d+$/.test(y);
    if (nx && ny) { const d = Number(x) - Number(y); if (d) return d; }
    else { const d = x.localeCompare(y); if (d) return d; }
  }
  return 0;
}
/** 校验并解开官网返回的加密信封，返回升级清单 manifest */
function openEnvelope(env, masterBuf, now) {
  const SKEW = 15 * 60 * 1000;
  if (!env || env.v !== 1 || env.alg !== 'A256GCM-HS256') throw new Error('升级接口协议版本不符');
  const ts = Number(env.ts);
  if (!Number.isFinite(ts) || Math.abs((now || Date.now()) - ts) > SKEW) throw new Error('升级响应已过期，请检查本机时间');
  const { encKey, macKey } = deriveCheckKeys(masterBuf);
  const want = crypto.createHmac('sha256', macKey)
    .update(`${ts}.${env.nonce}.${env.ct}.${env.tag}`).digest();
  const got = Buffer.from(env.mac, 'base64');
  if (want.length !== got.length || !crypto.timingSafeEqual(want, got)) throw new Error('升级响应校验失败');
  const decipher = crypto.createDecipheriv('aes-256-gcm', encKey, Buffer.from(env.nonce, 'base64'));
  decipher.setAuthTag(Buffer.from(env.tag, 'base64'));
  const plain = Buffer.concat([decipher.update(Buffer.from(env.ct, 'base64')), decipher.final()]);
  return JSON.parse(plain.toString('utf8'));
}

/* ---------- 模块状态 ---------- */
let winRef = null;          // 由 main.js 注入，用于向渲染进程推送事件
let timer = null;           // 1 小时定时器
let checking = false;
let lastResult = null;      // 最近一次检测结果（渲染进程打开设置时回放）

function keysFile() { return path.join(app.getAppPath(), 'update-keys.json'); }
function loadMasterKey() {
  try {
    const j = JSON.parse(fs.readFileSync(keysFile(), 'utf8'));
    const buf = Buffer.from(String(j.masterKeyBase64 || ''), 'base64');
    if (buf.length !== 32) throw new Error('主密钥长度不为 32 字节');
    return buf;
  } catch (e) {
    throw new Error('升级密钥未配置(update-keys.json 缺失或无效)，在线升级不可用');
  }
}
function emit(event, payload) {
  try { if (winRef && !winRef.isDestroyed()) winRef.webContents.send('update:event', { event, payload }); } catch {}
}

/** 运行模式判定（v3.0.3 修复）：
 *  - 'dev'      开发模式（electron .）
 *  - 'portable' 单文件版：EasySSH-<ver>.exe 是 NSIS 启动器，运行时把应用解压到 %TEMP%
 *               再执行解压目录里的 EasySSH.exe（真正跑的进程在临时目录），原始 exe 由
 *               PORTABLE_EXECUTABLE_FILE 指出。启动器进程（即用户双击的那个 exe 文件）
 *               在应用退出前一直存活并锁定自身文件 → 原地改名必然 EBUSY（3.0.2 现场实测）。
 *  - 'folder'   文件夹 zip 版：进程直接运行于安装目录的 EasySSH.exe，目录即安装目录。 */
function launchMode() {
  if (!app.isPackaged) return 'dev';
  if (process.env.PORTABLE_EXECUTABLE_FILE) return 'portable';
  return 'folder';
}

/** 安装目录（v3.0.3 重构）：
 *  portable 单文件版 → 原始单文件 exe 所在目录（PORTABLE_EXECUTABLE_FILE 的目录）；
 *  folder 文件夹版   → 进程 exe 所在目录；dev → 项目目录。
 *  两者都绝不落到 %TEMP%。portable 环境变量意外缺失时，回退到上次记录在
 *  userData（Roaming，永不位于 temp）里的 install-dir.json 锚点——该锚点在每次
 *  成功解析出真实安装目录时刷新。 */
function portableEnvFile() {
  const f = process.env.PORTABLE_EXECUTABLE_FILE;
  try { return (f && fs.existsSync(f)) ? f : null; } catch { return null; }
}
function isInsideTempDir(p) {
  try {
    const dir = path.resolve(p);
    const tmp = path.resolve(process.env.TEMP || process.env.TMP || os.tmpdir());
    const cmp = (x) => (process.platform === 'win32' ? String(x).toLowerCase() : String(x));
    if (cmp(dir) === cmp(tmp) || cmp(dir).startsWith(cmp(tmp) + path.sep)) return true;
    return /[/\\]temp[/\\]/i.test(dir + path.sep); // 兜底：解压目录路径通常带 Temp 字样
  } catch { return true; }
}
function installDirAnchorFile() {
  try { return path.join(app.getPath('userData'), 'install-dir.json'); } catch { return null; }
}
function rememberInstallDir(dir) {
  try {
    const f = installDirAnchorFile();
    if (f && dir && !isInsideTempDir(dir)) fs.writeFileSync(f, JSON.stringify({ dir, at: new Date().toISOString() }));
  } catch {}
}
function readInstallDirAnchor() {
  try {
    const f = installDirAnchorFile();
    if (!f) return null;
    const j = JSON.parse(fs.readFileSync(f, 'utf8'));
    const dir = String(j.dir || '');
    if (!dir || isInsideTempDir(dir) || !fs.existsSync(dir)) return null;
    return dir;
  } catch { return null; }
}
function installDir() {
  if (!app.isPackaged) return app.getAppPath();
  const mode = launchMode();
  if (mode === 'portable') {
    const f = portableEnvFile();
    if (f) { const d = path.dirname(path.resolve(f)); if (!isInsideTempDir(d)) { rememberInstallDir(d); return d; } }
    // 环境变量缺失/指向失效：退回上次记录的真实安装目录锚点
    const anchored = readInstallDirAnchor();
    if (anchored) return anchored;
    return path.dirname(path.resolve(app.getPath('exe'))); // 解析不出真实目录时给出明确临时目录，由 isTempResident 拦截
  }
  const d = path.dirname(path.resolve(app.getPath('exe')));
  if (!isInsideTempDir(d)) rememberInstallDir(d);
  return d;
}
function realExePath() {
  const mode = launchMode();
  if (mode === 'portable') {
    const f = portableEnvFile();
    if (f) return f;
    const anchored = readInstallDirAnchor();
    if (anchored) {
      // 锚点目录里最新的 EasySSH-*.exe 即最可能的原始 exe（排除 updates 子目录与 .bak）
      try {
        const cands = fs.readdirSync(anchored)
          .filter((n) => /^EasySSH-.*\.exe$/i.test(n) && !n.endsWith('.bak'))
          .map((n) => ({ n, m: fs.statSync(path.join(anchored, n)).mtimeMs }))
          .sort((a, b) => b.m - a.m);
        if (cands.length) return path.join(anchored, cands[0].n);
      } catch {}
    }
  }
  return app.getPath('exe');
}

/** 当前解析出的安装目录是否位于系统临时目录(%TEMP%)。
 *  正常单文件版运行时进程住在 temp 解压目录，但 realExePath/installDir 都指向真实
 *  安装目录，因此这里为 false；只有连锚点也解析不出真实目录（如 exe 被移走）时才为
 *  true——此时无法定位安装目录，必须警告并拒绝下载/替换，绝不写 temp。 */
function isTempResident() {
  if (!app.isPackaged) return false;
  return isInsideTempDir(installDir());
}

/** HTTPS GET 一段小响应体（检测信封），10 秒超时 */
function httpsGetText(url) {
  return new Promise((resolve, reject) => {
    const req = https.get(url, { timeout: 10000, headers: { 'User-Agent': 'EasySSH-Updater/' + app.getVersion() } }, (res) => {
      if (res.statusCode !== 200) { res.resume(); return reject(new Error('升级接口 HTTP ' + res.statusCode)); }
      let data = '';
      res.setEncoding('utf8');
      res.on('data', (c) => { data += c; if (data.length > 1024 * 1024) req.destroy(new Error('响应过大')); });
      res.on('end', () => resolve(data));
    });
    req.on('timeout', () => req.destroy(new Error('连接升级服务器超时')));
    req.on('error', reject);
  });
}

/** 检测更新。manual=true 时把结果也通过返回值带给渲染进程 */
async function checkUpdate(manual) {
  if (checking) return manual ? lastResult || { status: 'busy', message: '正在检测中…' } : null;
  checking = true;
  emit('checking', { manual });
  try {
    const master = loadMasterKey();
    const cfg = JSON.parse(fs.readFileSync(keysFile(), 'utf8'));
    const base = String(cfg.apiBase || 'https://easyssh.cn').replace(/\/+$/, '');
    const env = JSON.parse(await httpsGetText(base + API_PATH));
    const manifest = openEnvelope(env, master);
    const current = app.getVersion();
    const latest = String(manifest.latestVersion || '');
    const eligible = cmpVersion(latest, MIN_UPGRADE_VERSION) >= 0 && cmpVersion(latest, current) > 0;
    const tempResident = isTempResident();
    // kind 字段是网站端 v2.5.1 修复后才写入的;旧清单没有 kind,按文件名 .exe 兜底识别
    const exe = (manifest.files || []).find((f) => (f.kind === 'exe' || /\.exe$/i.test(String(f.file || ''))) && f.encUrl) || null;
    let result;
    if (!latest) {
      result = { status: 'error', message: '升级清单为空', checkedAt: new Date().toISOString() };
    } else if (eligible && exe) {
      result = {
        status: 'available',
        currentVersion: current, latestVersion: latest,
        releasedAt: manifest.releasedAt || '', notes: manifest.notes || [],
        // 清单文件条目可能没有 version 字段(旧版服务端),统一补上,供「完成升级」防降级校验使用
        file: { ...exe, version: latest }, checkedAt: new Date().toISOString(), tempResident,
      };
    } else {
      result = { status: 'up-to-date', currentVersion: current, latestVersion: latest, checkedAt: new Date().toISOString(), tempResident };
    }
    lastResult = result;
    emit('result', result);
    return result;
  } catch (e) {
    const result = { status: 'error', message: (e && e.message) || String(e), checkedAt: new Date().toISOString() };
    lastResult = result;
    // 自动检测失败保持安静（仅状态栏/事件），手工检测会拿到返回值
    emit('result', result);
    return result;
  } finally { checking = false; }
}

/** 下载加密安装包 → 流式解密校验 → 落为 exe 同目录下的新版本 exe */
async function downloadUpdate(entry, onProgress) {
  const master = loadMasterKey();
  const cfg = JSON.parse(fs.readFileSync(keysFile(), 'utf8'));
  const base = String(cfg.apiBase || 'https://easyssh.cn').replace(/\/+$/, '');
  const dir = installDir();
  const tempResident = isTempResident();
  if (tempResident) {
    // v3.0.3：真实安装目录无法定位（连锚点都没有）时直接拒绝，绝不把安装包下载到 temp
    throw new Error('无法定位真实安装目录（当前似乎运行于系统临时目录的解压副本，且找不到原始 exe）。\n'
      + '请关闭软件，把 EasySSH exe 移回（或重新从 easyssh.cn 下载放到）安装目录后再运行并升级。');
  }
  const encPath = path.join(dir, 'updates');
  fs.mkdirSync(encPath, { recursive: true });
  const encFile = path.join(encPath, entry.file + '.enc');
  const outFile = path.join(encPath, entry.file);          // EasySSH-<ver>.exe
  const tmpFile = outFile + '.part';

  // 1) 流式下载 .enc（边下边算 hmac 需要完整密文，这里先落盘再解密校验，均流式、常数内存）
  await new Promise((resolve, reject) => {
    const req = https.get(base + '/' + String(entry.encUrl).replace(/^https?:\/\/[^/]+\//, ''), { timeout: 30000 }, (res) => {
      if (res.statusCode !== 200) { res.resume(); return reject(new Error('下载加密包 HTTP ' + res.statusCode)); }
      const total = Number(res.headers['content-length']) || 0;
      let received = 0;
      const out = fs.createWriteStream(encFile + '.dl');
      res.on('data', (c) => { received += c.length; if (onProgress) onProgress('download', total ? received / total : 0); });
      res.pipe(out);
      out.on('finish', () => resolve());
      out.on('error', reject);
    });
    req.on('timeout', () => req.destroy(new Error('下载超时')));
    req.on('error', reject);
  });
  fs.renameSync(encFile + '.dl', encFile);

  // 2) 流式解密 + hmac + sha256 校验（任何一步失败都会抛错并清理）
  emit('verifying', {});
  const got = await decryptEncFile(encFile, tmpFile, entry.file, entry.sha256, master, (p) => {
    if (onProgress) onProgress('verify', p);
  });
  if (got.sha256 !== entry.sha256) { fs.rmSync(tmpFile, { force: true }); throw new Error('安装包校验失败，已中止'); }
  // 版本号兜底:清单文件条目缺 version 时,从文件名 EasySSH-<ver>.exe 解析
  const entryVersion = String(entry.version || (String(entry.file || '').match(/EasySSH-([0-9][0-9A-Za-z.\-]*)\.exe$/i) || [])[1] || '');
  fs.renameSync(tmpFile, outFile);
  fs.rmSync(encFile, { force: true });
  emit('downloaded', { file: outFile, version: entryVersion });
  lastDownloaded = { file: outFile, version: entryVersion };
  return { file: outFile, version: entryVersion };
}

/* ---------- 原地换名升级（无脚本，防杀软误报） ----------
 * Windows 允许对"正在运行中的 exe"执行改名（只禁止删除/覆盖写）。
 * 因此替换全程由旧进程在退出前自己完成，不用 bat/PowerShell，无任何外部脚本可被标记：
 *   1) 运行中的 exe 改名 -> <原名>.bak（Squirrel 等更新器同款技巧）
 *   2) updates\ 新 exe 改名 -> 原 exe 名（快捷方式/固定任务栏无缝沿用）
 *   3) detached 启动新 exe（--upgraded），旧进程退出
 * 回滚：任一步失败时新文件已在原位或 .bak 还在，手动改回原名即可，config/keys 不受影响。 */
let lastDownloaded = null; // 最近一次下载校验通过的新版本 {file, version}
function completeUpgrade() {
  if (!app.isPackaged) throw new Error('开发模式不支持原地替换，请手动运行新版本');
  // v3.0.1 补充防护：进程本身住在 %TEMP% 解压目录时，「原地替换」只会换掉临时副本，
  // 用户安装目录里的旧版原样未动（正是 3.0.0→3.0.1 这次的翻车现场）。此时明确拒绝并给出
  // 手动修复路径，而不是制造"升级成功"的假象。
  if (isTempResident()) {
    const src = lastDownloaded && lastDownloaded.file;
    throw new Error('当前运行的 exe 位于系统临时目录的解压副本中，无法原地替换真实安装目录的版本。'
      + '请手动完成升级：关闭本软件 → 把'
      + (src ? `「${src}」` : '新版本 exe')
      + '复制/移动到你的安装目录（旧版 EasySSH exe 所在文件夹），'
      + '覆盖旧版文件名后运行；也可到 easyssh.cn/download.html 重新下载放到安装目录。');
  }
  if (!lastDownloaded || !fs.existsSync(lastDownloaded.file)) throw new Error('尚未下载新版本，请先下载并校验');
  // 防降级校验:版本号缺失时从文件名兜底解析(历史上清单缺 version 字段曾导致这里误判"不高于当前版本")
  const targetVersion = String(lastDownloaded.version || (String(path.basename(lastDownloaded.file)).match(/EasySSH-([0-9][0-9A-Za-z.\-]*)\.exe$/i) || [])[1] || '');
  if (!targetVersion) throw new Error('无法识别新版本号，已取消替换(可到 updates\\ 文件夹手动运行新版本 exe)');
  if (cmpVersion(targetVersion, app.getVersion()) <= 0) throw new Error('目标版本不高于当前版本，拒绝替换（防降级）');
  // v3.0.3：realExePath() 指向"用户真正双击的 exe"——portable 单文件版取
  // PORTABLE_EXECUTABLE_FILE（进程本身在 temp 解压目录），folder 版取进程 exe。
  const orig = realExePath();
  const bak = orig + '.bak';
  // 0) 清掉可能的历史备份，保证本次改名干净
  try { fs.rmSync(bak, { force: true }); } catch {}
  // 1) 运行中的 exe 自改名（Windows 对"正在运行的 exe"允许改名）
  //    v3.0.3 修复：单文件版(portable)的 NSIS 启动器进程（用户双击的 EasySSH-<ver>.exe）
  //    在应用退出前一直存活并锁定自身文件，rename 直接 EBUSY（3.0.2 实测）。因此：
  //    a) 先带重试改名（覆盖 folder 版被杀软/资源管理器短暂占用的情况）；
  //    b) 仍失败则走"退出后延迟替换"兜底：本进程与启动器随后退出释放锁，由 detached 的
  //       cmd 在约 4 秒后完成 备份→顶替→启动新版。只使用系统 cmd 内建命令，无 bat 落盘。
  let swapped = false;
  let lastErr = null;
  for (let i = 0; i < 4; i++) {
    try { fs.renameSync(orig, bak); swapped = true; break; }
    catch (e) { lastErr = e; if (sleepSync(500)) break; }
  }
  if (swapped) {
    try {
      // 2) 新 exe 顶替原名（瞬时、原子；此后桌面快捷方式即指向新版本）
      fs.renameSync(lastDownloaded.file, orig);
    } catch (e) {
      // 极端情况(如杀软占用新文件)：立即把旧版改回原名，等价于未发生
      try { fs.renameSync(bak, orig); } catch {}
      throw new Error('替换被系统/杀软拦截，已自动恢复旧版本：' + (e.message || e));
    }
  } else {
    // EBUSY 兜底：退出后由 cmd 完成替换。copy 正在运行的 exe 是允许的，move 在
    // 本进程与单文件启动器都退出、锁释放后执行；最后 start 拉起新版（--upgraded）。
    const q = (s) => `"${s}"`;
    const script = [
      'ping -n 5 127.0.0.1 >nul',                       // 等待本进程+单文件启动器退出释放锁(约4秒)
      `copy /y ${q(orig)} ${q(bak)} >nul`,              // 备份旧版（可手动回滚）
      `move /y ${q(lastDownloaded.file)} ${q(orig)} >nul`, // 新版顶替原名，快捷方式无缝沿用
      `start "" ${q(orig)} --upgraded ${targetVersion}`,
    ].join(' & ');
    const child = spawn('cmd.exe', ['/d', '/s', '/c', script],
      { detached: true, stdio: 'ignore', windowsHide: true, cwd: path.dirname(orig) });
    child.unref();
  }
  // 3) 分离退出本进程：folder 版释放进程 exe 锁；portable 版随后连 NSIS 启动器一起退出
  setTimeout(() => { try { app.quit(); } catch {} }, 1200);
  return { ok: true, version: lastDownloaded.version, deferred: !swapped };
}

/** 小工具：同步等待（仅用于改名重试间隔） */
function sleepSync(ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) { /* 忙等极短间隔，仅 4 次 × 0.5s，可接受 */ }
  return false;
}

/** 启动期清理：升级成功后的 .bak、下载残留 .enc/.part（全部尽力而为，失败忽略下次再清） */
function cleanupStale() {
  try {
    const dir = installDir();
    for (const n of fs.readdirSync(dir)) {
      if (n.endsWith('.bak') && n.startsWith('EasySSH-')) {
        try { fs.rmSync(path.join(dir, n), { force: true }); } catch {} // 可能仍被占用，忽略
      }
    }
    const upd = path.join(dir, 'updates');
    if (fs.existsSync(upd)) {
      for (const n of fs.readdirSync(upd)) {
        if (n.endsWith('.enc') || n.endsWith('.part')) { try { fs.rmSync(path.join(upd, n), { force: true }); } catch {} }
      }
    }
  } catch {}
}

/** 客户端侧流式解密实现（与网站端格式一致：魔数+头+CTR 密文） */
function decryptEncFile(encPath, destPath, fileName, plainSha256Hex, masterBuf, onProgress) {
  return new Promise((resolve, reject) => {
    const done = (err, val) => { try { fs.closeSync(fd); } catch {} err ? reject(err) : resolve(val); };
    let fd;
    try {
      fd = fs.openSync(encPath, 'r');
      const magic = Buffer.alloc(8);
      if (fs.readSync(fd, magic, 0, 8, 0) !== 8 || !magic.equals(MAGIC)) return done(new Error('不是有效的 EasySSH 加密包'));
      const lenBuf = Buffer.alloc(4);
      fs.readSync(fd, lenBuf, 0, 4, 8);
      const headLen = lenBuf.readUInt32BE(0);
      if (!headLen || headLen > 65536) return done(new Error('加密包头非法'));
      const headBuf = Buffer.alloc(headLen);
      fs.readSync(fd, headBuf, 0, headLen, 12);
      const header = JSON.parse(headBuf.toString('utf8'));
      if (header.v !== 1 || !header.iv) return done(new Error('加密包头字段缺失'));
      // 独立 hmac 区(64 字节 ASCII hex)，hmac 覆盖"头JSON || 密文"
      const hmacBuf = Buffer.alloc(64);
      if (fs.readSync(fd, hmacBuf, 0, 64, 12 + headLen) !== 64) return done(new Error('hmac 区缺失'));
      const wantHmac = hmacBuf.toString('ascii');
      if (!/^[0-9a-f]{64}$/.test(wantHmac)) return done(new Error('hmac 区非法'));
      if (header.sha256 !== plainSha256Hex) return done(new Error('包指纹与升级清单不符'));
      const { encKey, macKey } = deriveFileKeys(masterBuf, fileName, plainSha256Hex);
      const decipher = crypto.createDecipheriv('aes-256-ctr', encKey, Buffer.from(header.iv, 'base64'));
      const hmac = crypto.createHmac('sha256', macKey);
      hmac.update(headBuf);
      const hashPlain = crypto.createHash('sha256');
      const out = fs.createWriteStream(destPath);
      const buf = Buffer.alloc(1024 * 1024);
      let pos = 12 + headLen + 64;
      const st = fs.fstatSync(fd);
      const step = () => {
        try {
          while (pos < st.size) {
            const n = fs.readSync(fd, buf, 0, Math.min(buf.length, st.size - pos), pos);
            if (n <= 0) break;
            pos += n;
            const chunk = buf.subarray(0, n);
            hmac.update(chunk);
            const plain = decipher.update(chunk);
            hashPlain.update(plain);
            if (!out.write(plain)) { out.once('drain', step); return; }
          }
          const tail = decipher.final();
          if (tail.length) { hashPlain.update(tail); out.write(tail); }
          out.end(() => {
            if (hmac.digest('hex') !== wantHmac) { fs.rmSync(destPath, { force: true }); return done(new Error('安装包完整性校验失败')); }
            const sha = hashPlain.digest('hex');
            if (sha !== plainSha256Hex) { fs.rmSync(destPath, { force: true }); return done(new Error('安装包校验失败(sha256)')); }
            done(null, { sha256: sha });
          });
        } catch (e) { try { fs.rmSync(destPath, { force: true }); } catch {} done(e); }
      };
      step();
    } catch (e) { try { fs.rmSync(destPath, { force: true }); } catch {} reject(e); }
  });
}

/* ---------- 对 main.js 暴露的安装入口 ---------- */
function initUpdater(win) {
  winRef = win;
  ipcMain.handle('update:check', () => checkUpdate(true));
  ipcMain.handle('update:last', () => lastResult);
  ipcMain.handle('update:download', async (_e, entry) => {
    const onProgress = (phase, p) => emit('progress', { phase, percent: Math.round(p * 100) });
    try { return { ok: true, ...(await downloadUpdate(entry, onProgress)) }; }
    catch (e) { return { ok: false, message: (e && e.message) || String(e) }; }
  });
  ipcMain.handle('update:openFile', (_e, file) => {
    try { require('electron').shell.showItemInFolder(file); return true; } catch { return false; }
  });
  ipcMain.handle('update:complete', () => {
    try { return { ok: true, ...(completeUpgrade()) }; }
    catch (e) { return { ok: false, message: (e && e.message) || String(e) }; }
  });

  // 原地升级成功后新进程带 --upgraded 启动：通知渲染端展示"已升级"，并延迟清理备份/残留
  if (process.argv.includes('--upgraded')) {
    setTimeout(() => emit('upgraded', { version: app.getVersion() }), 1500);
  }
  setTimeout(cleanupStale, 8000);

  // 打开软件后自动检测一次；之后每 1 小时检测一次（失败不打扰用户）
  setTimeout(() => checkUpdate(false), 3000);
  timer = setInterval(() => checkUpdate(false), CHECK_INTERVAL_MS);
  win.on('closed', () => { if (timer) clearInterval(timer); timer = null; winRef = null; });
}

module.exports = { initUpdater, checkUpdate, MIN_UPGRADE_VERSION };
