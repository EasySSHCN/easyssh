/* ============================================================
 * knock.js — 连接表单「端口敲门 / 前置命令」折叠区逻辑(R-20261122-001)
 * 职责：knock 配置回填/收集/复位、knockd.conf 片段解析导入、
 *       SPA 密钥生成(等价 fwknop --key-gen:32 随机字节 base64)、
 *       敲门状态可视化(监听后端 knock:log 事件:未敲/敲门中/已开门+倒计时/失败)。
 * 配置结构与 src-tauri knock.rs KnockCfg 保持一致:
 *   { enabled, type:'sequence'|'spa', sequence:[u16], protocol:'udp'|'tcp',
 *     seqTimeout, delayMs, spaPort, key, hmacKey, openPorts, fwTimeout }
 * 会话持久化: sessions[].knock + sessions[].preConnectCommand
 *   (见 sessions.js fillConnectForm / terminal.js 保存分支;导出脱敏剔除 key/hmacKey)。
 * 敲门执行在后端: ssh.rs::connect 前置门 → knock.rs(钩子→敲门→可达性轮询)。
 * ============================================================ */
import { $ } from './dom.js';

/* ---------- 表单显隐联动 ---------- */
export function syncKnockUi() {
  const form = document.getElementById('connectForm');
  if (!form || !form.knEnabled) return;
  const on = form.knEnabled.checked;
  const isSpa = on && form.knType.value === 'spa';
  document.getElementById('knType').disabled = !on;
  document.getElementById('knSeqRows').style.display = isSpa ? 'none' : '';
  document.getElementById('knSpaRows').style.display = isSpa ? '' : 'none';
}

/* ---------- 会话 → 表单回填 / 表单 → 配置收集 ---------- */

/** 回填:knock 配置对象 + 前置命令(无配置时复位为默认)。 */
export function fillKnockForm(knock, preCmd) {
  resetKnockForm();
  if (!knock || typeof knock !== 'object') {
    if (preCmd) document.getElementById('connectForm').preConnectCommand.value = preCmd;
    return;
  }
  const form = document.getElementById('connectForm');
  form.knEnabled.checked = !!knock.enabled;
  form.knType.value = knock.type === 'spa' ? 'spa' : 'sequence';
  form.knSequence.value = Array.isArray(knock.sequence) ? knock.sequence.join(',') : '';
  form.knProtocol.value = knock.protocol === 'tcp' ? 'tcp' : 'udp';
  form.knSeqTimeout.value = knock.seqTimeout || 10;
  form.knDelayMs.value = knock.delayMs || 150;
  form.knSpaPort.value = knock.spaPort || 62201;
  form.knKey.value = knock.key || '';
  form.knHmacKey.value = knock.hmacKey || '';
  form.knOpenPorts.value = knock.openPorts || 'tcp/22';
  form.knFwTimeout.value = knock.fwTimeout || 30;
  form.preConnectCommand.value = preCmd || '';
  syncKnockUi();
}

/** 收集:未启用敲门且无前置命令时返回空对象 {knock:null, preConnectCommand:''}。 */
export function collectKnockForm() {
  const form = document.getElementById('connectForm');
  if (!form || !form.knEnabled) return { knock: null, preConnectCommand: '' };
  const pre = (form.preConnectCommand.value || '').trim();
  let knock = null;
  if (form.knEnabled.checked) {
    const seq = (form.knSequence.value || '')
      .split(/[,\s;]+/).map((s) => parseInt(s, 10)).filter((n) => n > 0 && n < 65536);
    knock = {
      enabled: true,
      type: form.knType.value === 'spa' ? 'spa' : 'sequence',
      sequence: seq,
      protocol: form.knProtocol.value === 'tcp' ? 'tcp' : 'udp',
      seqTimeout: Number(form.knSeqTimeout.value) || 10,
      delayMs: Number(form.knDelayMs.value) || 150,
      spaPort: Number(form.knSpaPort.value) || 62201,
      key: form.knKey.value || '',
      hmacKey: form.knHmacKey.value || '',
      openPorts: (form.knOpenPorts.value || '').trim() || 'tcp/22',
      fwTimeout: Number(form.knFwTimeout.value) || 30,
    };
  }
  return { knock, preConnectCommand: pre };
}

/** 复位折叠区为默认初始态(新建连接/取消时调用)。 */
export function resetKnockForm() {
  const form = document.getElementById('connectForm');
  if (!form || !form.knEnabled) return;
  form.knEnabled.checked = false;
  form.knType.value = 'sequence';
  form.knSequence.value = '';
  form.knProtocol.value = 'udp';
  form.knSeqTimeout.value = 10;
  form.knDelayMs.value = 150;
  form.knSpaPort.value = 62201;
  form.knKey.value = '';
  form.knHmacKey.value = '';
  form.knOpenPorts.value = 'tcp/22';
  form.knFwTimeout.value = 30;
  form.preConnectCommand.value = '';
  document.getElementById('knMsg').textContent = '';
  document.getElementById('knImportBox').style.display = 'none';
  setKnockStatus('idle', '', 0);
  syncKnockUi();
}

/* ---------- knockd.conf 片段解析导入(P3) ---------- */
/* 支持两种 knockd 常见写法:
 *   逗号式: sequence = 7000,8000,9000        (协议取全局,默认 udp)
 *   冒号式: sequence    = 7000:udp:8000:udp:9000:udp
 * 以及 Seq_Timeout / seq_timeout 数值行。取第一个非空 sequence 块。 */
export function parseKnockdConf(text) {
  const out = { sequence: [], protocol: 'udp', seqTimeout: 10 };
  const lines = String(text || '').split(/\r?\n/);
  for (const raw of lines) {
    const line = raw.replace(/\t/g, ' ').trim();
    if (!line || line.startsWith('#') || line.startsWith('[')) continue;
    const m = line.match(/^(sequence|Seq_Timeout|seq_timeout)\s*=\s*(.+)$/i);
    if (!m) continue;
    const key = m[1].toLowerCase();
    const val = m[2].trim();
    if (key === 'seq_timeout') {
      const n = parseFloat(val);
      if (n > 0) out.seqTimeout = Math.round(n);
    } else if (val.includes(':')) {
      // 冒号式: 端口[:协议] 交替
      const parts = val.split(':');
      const seq = [];
      let proto = 'udp';
      for (let i = 0; i < parts.length; i += 2) {
        const p = parseInt(parts[i], 10);
        if (p > 0 && p < 65536) seq.push(p);
        const pr = (parts[i + 1] || '').trim().toLowerCase();
        if (pr === 'tcp') proto = 'tcp';
      }
      if (seq.length) { out.sequence = seq; out.protocol = proto; }
    } else {
      const seq = val.split(/[,;\s]+/).map((s) => parseInt(s, 10)).filter((n) => n > 0 && n < 65536);
      if (seq.length && !out.sequence.length) out.sequence = seq;
    }
  }
  return out;
}

document.addEventListener('click', (e) => {
  const t = e.target.closest ? e.target.closest('#knImportBtn, #knConfApply, #knGenKeys') : null;
  if (!t) return;
  const form = document.getElementById('connectForm');
  if (t.id === 'knImportBtn') {
    const box = document.getElementById('knImportBox');
    box.style.display = box.style.display === 'none' ? '' : 'none';
  } else if (t.id === 'knConfApply') {
    const r = parseKnockdConf(document.getElementById('knConfText').value);
    if (!r.sequence.length) {
      document.getElementById('knMsg').textContent = '未解析到 sequence 端口序列,请检查格式';
      return;
    }
    form.knEnabled.checked = true;
    form.knType.value = 'sequence';
    form.knSequence.value = r.sequence.join(',');
    form.knProtocol.value = r.protocol;
    form.knSeqTimeout.value = r.seqTimeout;
    syncKnockUi();
    document.getElementById('knMsg').textContent =
      '✅ 已导入: ' + r.sequence.join('→') + ' / ' + r.protocol.toUpperCase() + ' / seq_timeout=' + r.seqTimeout + 's';
  } else if (t.id === 'knGenKeys') {
    const gen = () => {
      const raw = new Uint8Array(32);
      crypto.getRandomValues(raw);
      return btoa(String.fromCharCode.apply(null, raw));
    };
    form.knKey.value = gen();
    form.knHmacKey.value = gen();
    document.getElementById('knMsg').textContent = '✅ 已生成 fwknop 原生密钥对(32B base64)。R-20261122-002 起 SPA 采用 libfko 原生语义:此密钥可直接写入服务器 access.conf 的 KEY_BASE64:/HMAC_KEY_BASE64:';
  }
});
document.addEventListener('change', (e) => {
  if (e.target && (e.target.name === 'knEnabled' || e.target.name === 'knType')) syncKnockUi();
});

/* ---------- 敲门状态可视化(P3):监听后端 knock:log 事件 ---------- */
/* 状态: 未敲(idle) / 敲门中(knocking) / 已开门+窗口倒计时(open) / 失败(failed)。
 * 倒计时用后端随事件下发的 windowSec;到期回落「未敲」。 */
let knTimer = null;
function setKnockStatus(state, message, windowSec) {
  const el = document.getElementById('knStatus');
  if (!el) return;
  if (knTimer) { clearInterval(knTimer); knTimer = null; }
  if (state === 'knocking') {
    el.textContent = '🚪 敲门中… ' + (message || '');
    el.style.color = 'var(--color-accent)';
  } else if (state === 'open') {
    const total = Number(windowSec) || 30;
    let left = total;
    const render = () => {
      el.textContent = '🟢 已开门 ' + (message || '') + (left > 0 ? ' · 窗口剩余 ' + left + 's' : ' · 窗口已过,下次连接将重新敲门');
      el.style.color = 'var(--color-success, var(--color-accent))';
    };
    render();
    knTimer = setInterval(() => {
      left -= 1;
      if (left <= 0) { clearInterval(knTimer); knTimer = null; setKnockStatus('idle', '', 0); return; }
      render();
    }, 1000);
  } else if (state === 'failed') {
    el.textContent = '🔴 敲门失败 ' + (message || '');
    el.style.color = 'var(--color-danger)';
  } else {
    el.textContent = '⚪ 未敲门';
    el.style.color = '';
  }
}

/* 事件订阅(Tauri 环境;浏览器直开 dev 页时跳过)。 */
(function subscribeKnockLog() {
  try {
    if (window.__TAURI__ && window.__TAURI__.event && window.__TAURI__.event.listen) {
      window.__TAURI__.event.listen('knock:log', (ev) => {
        const p = ev.payload || {};
        setKnockStatus(p.state, p.message, p.windowSec);
      });
    }
  } catch { /* 非 Tauri 环境忽略 */ }
})();

/* 连接失败错误分类提示文案(P3):terminal.js 失败分支调用,返回追加的 hint 文本 */
export function knockFailHint(firstLine) {
  const s = String(firstLine || '');
  if (/^PRECONNECT_FAIL/.test(s)) {
    return '\n提示:连接前本地命令执行失败(见上方命令与退出码)。请确认命令存在、可执行且 15 秒内能完成;不需要时清空「连接前本地命令」。';
  }
  if (/^KNOCK_INVALID/.test(s)) {
    return '\n提示:敲门配置无效或敲门包被服务器拒绝。请检查:序列端口是否与服务器 knockd 配置一致 / SPA 密钥是否正确(R-20261122-002 起为 fwknop 原生语义:32B base64 密钥对应服务器 KEY_BASE64:,纯文本口令对应 KEY:;旧自研格式密钥需重新生成) / 本机与服务器时钟偏差(SPA 默认容忍 ±120 秒)。';
  }
  if (/^KNOCK_TIMEOUT/.test(s)) {
    return '\n提示:敲门包已发出,但目标端口在轮询窗口内仍未开放。请检查服务器端 knockd/fwknopd 是否运行、seq_timeout 与客户端序列超时是否一致、防火墙开门端口是否为当前连接端口。';
  }
  return '';
}
