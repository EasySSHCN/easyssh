/**
 * EasySSH 使用反馈 — 软件端加密提交（v2.8.5）
 *
 * 用途：用户在「💬 反馈」弹窗提交建议/问题，经加密信封 POST 到官网
 *       https://easyssh.cn/api/feedback/submit.php，进入官网反馈库；
 *       被官方回复并公开的反馈会展示在官网下载页「使用反馈精选 TOP10」。
 *
 * 密钥与协议（与在线升级检测接口同族，服务端 api/feedback/common.php 同一套实现）：
 *   主密钥复用 update-keys.json 的 masterKeyBase64（不新增密钥文件，随包携带、不进源码 zip）；
 *   信封 = { v:1, alg:"A256GCM-HS256", ts:<ms>, nonce:b64(12B), ct:b64, tag:b64, mac:b64 }
 *   ct   = AES-256-GCM(fbEncKey, nonce, 明文JSON)
 *   mac  = HMAC-SHA256(fbMacKey, `${ts}.${nonce}.${ct}.${tag}`)
 *   fbEncKey / fbMacKey = HKDF-SHA256(master, salt='easyssh-feedback-v1', info) 派生
 */
'use strict';

const { app, ipcMain } = require('electron');
const crypto = require('crypto');
const https = require('https');
const http = require('http');
const path = require('path');
const fs = require('fs');
const os = require('os');
const { execFile } = require('child_process');


const API_URL = 'https://easyssh.cn/api/feedback/submit.php';
const TIMEOUT_MS = 15000;

function hkdf(master, salt, info, len) {
  return crypto.hkdfSync('sha256', master, Buffer.from(salt, 'utf8'), Buffer.from(info, 'utf8'), len);
}

function loadMasterKey() {
  try {
    const j = JSON.parse(fs.readFileSync(path.join(app.getAppPath(), 'update-keys.json'), 'utf8'));
    const buf = Buffer.from(String(j.masterKeyBase64 || ''), 'base64');
    if (buf.length !== 32) throw new Error('主密钥长度不为 32 字节');
    return buf;
  } catch (e) {
    throw new Error('反馈通道密钥未配置(update-keys.json 缺失或无效)');
  }
}

function deriveFbKeys(masterBuf) {
  // HMAC-SHA256 派生(与官网 api/feedback/common.php 完全一致;不依赖 HKDF,规避跨 OpenSSL 实现差异)
  return {
    encKey: crypto.createHmac('sha256', masterBuf).update('enc').digest(),
    macKey: crypto.createHmac('sha256', masterBuf).update('mac').digest(),
  };
}

/** 构造反馈加密信封 */
function buildEnvelope(payload) {
  const masterBuf = loadMasterKey();
  const { encKey, macKey } = deriveFbKeys(masterBuf);
  const nonce = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv('aes-256-gcm', encKey, nonce);
  const ct = Buffer.concat([cipher.update(Buffer.from(JSON.stringify(payload), 'utf8')), cipher.final()]);
  const tag = cipher.getAuthTag();
  const env = {
    v: 1,
    alg: 'A256GCM-HS256',
    ts: Date.now(),
    nonce: nonce.toString('base64'),
    ct: ct.toString('base64'),
    tag: tag.toString('base64'),
  };
  env.mac = crypto.createHmac('sha256', macKey)
    .update(env.ts + '.' + env.nonce + '.' + env.ct + '.' + env.tag)
    .digest('base64');
  return env;
}

/** HTTPS POST JSON，返回解析后的响应体 */
function httpsPostJson(url, obj) {
  return new Promise((resolve, reject) => {
    const body = Buffer.from(JSON.stringify(obj), 'utf8');
    const req = https.request(url, {
      method: 'POST',
      timeout: TIMEOUT_MS,
      headers: {
        'Content-Type': 'application/json',
        'Content-Length': body.length,
        'User-Agent': 'EasySSH-Feedback/' + app.getVersion(),
      },
    }, (res) => {
      let data = '';
      res.setEncoding('utf8');
      res.on('data', (c) => { data += c; if (data.length > 1024 * 1024) req.destroy(new Error('响应过大')); });
      res.on('end', () => {
        let j = null;
        try { j = JSON.parse(data); } catch {}
        if (res.statusCode !== 200 && !j) return reject(new Error('反馈接口 HTTP ' + res.statusCode));
        resolve(j || { ok: false, message: '反馈接口响应异常' });
      });
    });
    req.on('timeout', () => req.destroy(new Error('连接反馈服务器超时')));
    req.on('error', reject);
    req.end(body);
  });
}

/** 执行只读系统命令，失败返回空串（Windows 用 wmic，非 Windows 返回空） */
function runCmd(cmd, args, timeoutMs) {
  return new Promise((resolve) => {
    try {
      const t = setTimeout(() => { try { child.kill(); } catch {} resolve(''); }, timeoutMs || 4000);
      const child = execFile(cmd, args, { timeout: timeoutMs || 4000, windowsHide: true }, (err, stdout) => {
        clearTimeout(t);
        resolve(err ? '' : String(stdout || ''));
      });
    } catch { resolve(''); }
  });
}

function fetchText(url, timeoutMs) {
  return new Promise((resolve) => {
    try {
      const mod = url.startsWith('https:') ? https : http;
      const req = mod.get(url, { timeout: timeoutMs || 5000, headers: { 'User-Agent': 'EasySSH/' + app.getVersion() } }, (res) => {
        let d = '';
        res.setEncoding('utf8');
        res.on('data', (c) => { d += c; if (d.length > 4096) req.destroy(); });
        res.on('end', () => resolve(d.trim().slice(0, 64)));
      });
      req.on('timeout', () => { req.destroy(); resolve(''); });
      req.on('error', () => resolve(''));
    } catch { resolve(''); }
  });
}

/**
 * 静默采集本机运行环境概要（用户无感知，仅随反馈提交，用于复现环境问题）。
 * 只收集只读摘要字段；任一项失败自动跳过，不阻塞反馈提交。
 */
async function collectSystemInfo() {
  const info = {};
  try { info['操作系统'] = os.type() + ' ' + os.release() + ' (' + os.arch() + ')'; } catch {}
  try { info['主机名'] = os.hostname(); } catch {}
  try {
    const cpus = os.cpus();
    if (cpus.length) info['CPU'] = cpus[0].model.trim() + ' × ' + cpus.length + ' 核';
  } catch {}
  try { info['内存'] = (os.totalmem() / (1024 * 1024 * 1024)).toFixed(1) + ' GB'; } catch {}
  try { info['系统运行时长'] = Math.floor(os.uptime() / 3600) + ' 小时'; } catch {}
  try { info['EasySSH版本'] = app.getVersion(); } catch {}
  try { info['Electron/Node'] = 'Electron ' + process.versions.electron + ' / Node ' + process.versions.node; } catch {}

  if (process.platform === 'win32') {
    // 显卡
    const gpu = await runCmd('wmic', ['path', 'win32_VideoController', 'get', 'name']);
    const gpuNames = gpu.split(/\r?\n/).map(s => s.trim()).filter(s => s && s.toLowerCase() !== 'name');
    if (gpuNames.length) info['显卡'] = gpuNames.join(' / ').slice(0, 200);
    // 磁盘（本地逻辑盘：盘符/总容量/剩余）
    const disks = await runCmd('wmic', ['logicaldisk', 'where', 'drivetype=3', 'get', 'caption,size,freespace']);
    const lines = disks.split(/\r?\n/).map(s => s.trim()).filter(Boolean);
    const parts = [];
    for (let i = 1; i < lines.length; i++) {
      const m = lines[i].split(/\s+/);
      if (m.length >= 3) {
        const total = parseFloat(m[1]) || 0, free = parseFloat(m[0]) || 0;
        if (total > 0) parts.push(m[2] + ' ' + (total / 1e9).toFixed(0) + 'GB(剩' + (free / 1e9).toFixed(0) + 'GB)');
      }
    }
    if (parts.length) info['磁盘'] = parts.join(', ').slice(0, 240);
    // 系统安装日期/语言等轻量补充
    const os2 = await runCmd('wmic', ['os', 'get', 'caption,oslanguage', '/format:list']);
    const cap = /Caption=(.+)/.exec(os2), lang = /OSLanguage=(.+)/.exec(os2);
    if (cap) info['系统版本'] = cap[1].trim().slice(0, 80);
    if (lang) info['系统语言代码'] = lang[1].trim().slice(0, 16);
  }

  // 网卡（名称/MAC/私有 IP，来自 os.networkInterfaces，只读）
  try {
    const nets = [];
    const ifs = os.networkInterfaces();
    for (const name of Object.keys(ifs)) {
      for (const ni of ifs[name] || []) {
        if (ni.internal) continue;
        nets.push(name + '[' + (ni.mac || '') + ' ' + (ni.address || '') + '/' + (ni.family || '') + ']');
        break; // 每张网卡只取第一个外部地址，控制长度
      }
    }
    if (nets.length) info['网卡'] = nets.join(', ').slice(0, 300);
  } catch {}

  // 公网出口 IP（超时/失败自动跳过）
  const pub = await fetchText('https://api.ipify.org');
  if (pub) info['公网IP'] = pub;

  // 字段总量兜底限制
  const keys = Object.keys(info);
  const out = {};
  for (const k of keys.slice(0, 24)) out[String(k).slice(0, 24)] = String(info[k]).slice(0, 300);
  return out;
}

async function submitFeedback(payload) {
  const type = String(payload && payload.type || '其他');
  const contact = String(payload && payload.contact || '').trim();
  const content = String(payload && payload.content || '').trim();
  if (content.length < 5 || content.length > 2000) {
    return { ok: false, message: '反馈内容需为 5-2000 个字符' };
  }
  if (contact.length > 120) return { ok: false, message: '联系方式过长' };
  try {
    const sysInfo = await collectSystemInfo();
    const env = buildEnvelope({
      type, contact, content, appVersion: app.getVersion(), ts: Date.now(), systemInfo: sysInfo,
    });
    const res = await httpsPostJson(API_URL, env);
    if (res && res.ok) return { ok: true, message: res.message || '反馈已提交，感谢您的支持！' };
    return { ok: false, message: (res && res.message) || '提交失败，请稍后再试' };
  } catch (e) {
    return { ok: false, message: '提交失败: ' + (e && e.message || e) };
  }
}

let winRef = null;

function initFeedback(win) {
  winRef = win;
  ipcMain.handle('feedback:submit', (_e, payload) => submitFeedback(payload));
}

module.exports = { initFeedback, submitFeedback };
