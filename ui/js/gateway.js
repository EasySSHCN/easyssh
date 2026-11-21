/* ============================================================
 * gateway.js — 连接表单「网关(可选)」折叠区逻辑(R-20261120-005 ③)
 * 职责：网关地址粘贴自动拆分(host/port/TLS)、自签豁免红字提示联动、
 *       认证方式切换显隐、备用网关增删列表、连通性检测
 *       (invoke gateway_health_check,绿/红 + 延迟显示)。
 * 配置结构与 src-tauri gateway.rs GatewayConfig 保持一致:
 *   { enabled, host, port, tls, skipVerify, auth:'token'|'password'|'inherit',
 *     token, username, password, resourceId, otp, backups:[{host,port,tls}] }
 * 会话持久化: sessions[].gateway(见 sessions.js fillConnectForm / terminal.js 保存分支)。
 * ============================================================ */
import { $, escH } from './dom.js';

/* ---------- 会话 → 表单回填 / 表单 → 配置收集 ---------- */

/** 把会话里的网关配置回填到折叠区表单(无配置时复位为默认)。 */
export function fillGatewayForm(gw) {
  resetGatewayForm();
  if (!gw || typeof gw !== 'object') return;
  const form = document.getElementById('connectForm');
  form.gwEnabled.checked = !!gw.enabled;
  form.gwAddr.value = gw.host || '';
  form.gwPort.value = gw.port || 443;
  form.gwTls.checked = gw.tls !== false;
  form.gwSkipVerify.checked = !!gw.skipVerify;
  form.gwAuth.value = ['token', 'password', 'inherit'].includes(gw.auth) ? gw.auth : 'token';
  form.gwToken.value = gw.token || '';
  form.gwUser.value = gw.username || '';
  form.gwPass.value = gw.password || '';
  form.gwResource.value = gw.resourceId || '';
  // 备用网关列表逐条重建(只回填地址三元组,凭据不随备用条目保存)
  for (const b of Array.isArray(gw.backups) ? gw.backups : []) addBackupRow(b);
  syncGatewayUi();
}

/** 收集折叠区当前表单值为网关配置对象;未勾选启用时返回 null。 */
export function collectGatewayForm() {
  const form = document.getElementById('connectForm');
  if (!form || !form.gwEnabled || !form.gwEnabled.checked) return null;
  const gw = {
    enabled: true,
    host: form.gwAddr.value.trim(),
    port: Number(form.gwPort.value) || 443,
    tls: form.gwTls.checked,
    skipVerify: form.gwSkipVerify.checked,
    auth: form.gwAuth.value,
    resourceId: form.gwResource.value.trim(),
    backups: collectBackups(),
  };
  if (gw.auth === 'token') gw.token = form.gwToken.value;
  else if (gw.auth === 'password') { gw.username = form.gwUser.value.trim(); gw.password = form.gwPass.value; }
  return gw;
}

/** 复位折叠区为默认初始态(新建连接/取消时调用)。 */
export function resetGatewayForm() {
  const form = document.getElementById('connectForm');
  if (!form || !form.gwEnabled) return;
  form.gwEnabled.checked = false;
  form.gwAddr.value = '';
  form.gwPort.value = 443;
  form.gwTls.checked = true;
  form.gwSkipVerify.checked = false;
  form.gwAuth.value = 'token';
  form.gwToken.value = '';
  form.gwUser.value = '';
  form.gwPass.value = '';
  form.gwResource.value = '';
  const ul = document.getElementById('gwBackups');
  if (ul) ul.innerHTML = '';
  const msg = document.getElementById('gwCheckMsg');
  if (msg) msg.textContent = '';
  syncGatewayUi();
}

/* ---------- 地址粘贴自动拆分:https://gw:8443 → host=gw / port=8443 / TLS=开 ---------- */
export function splitGatewayAddr(raw) {
  let s = String(raw || '').trim();
  if (!s) return null;
  const out = {};
  const m = s.match(/^(https?):\/\/([^/:?#]+)(?::(\d+))?/i);
  if (m) {
    out.tls = m[1].toLowerCase() === 'https';
    out.host = m[2];
    out.port = m[3] ? Number(m[3]) : null;
    return out;
  }
  // 无协议:支持 host:port 写法(IPv6 字面量含多个冒号,整体视作 host)
  const i = s.lastIndexOf(':');
  if (i > 0 && (s.match(/:/g) || []).length === 1) {
    const p = Number(s.slice(i + 1));
    if (p > 0 && p <= 65535) { out.host = s.slice(0, i); out.port = p; return out; }
  }
  out.host = s;
  return out;
}

/* ---------- 表单联动 ---------- */

/** 认证方式显隐 / 自签豁免红字提示(由事件绑定调用,回填后也需手动触发一次)。 */
export function syncGatewayUi() {
  const form = document.getElementById('connectForm');
  if (!form || !form.gwAuth) return;
  const auth = form.gwAuth.value;
  document.getElementById('gwTokenLabel').style.display = auth === 'token' ? 'flex' : 'none';
  document.getElementById('gwPwRows').style.display = auth === 'password' ? 'flex' : 'none';
  document.getElementById('gwSkipWarn').style.display = form.gwSkipVerify.checked ? 'block' : 'none';
}

/* ---------- 备用网关增删列表 ---------- */
function addBackupRow(b) {
  const ul = document.getElementById('gwBackups');
  const li = document.createElement('li');
  li.className = 'gw-backup-row';
  li.style.cssText = 'display:flex;gap:6px;align-items:center;margin:2px 0;';
  li.innerHTML =
    '<input class="gb-host" placeholder="备用网关地址" style="flex:1;min-width:0;" />'
    + '<input class="gb-port" type="number" placeholder="端口" style="width:74px;" />'
    + '<label class="chk" style="flex-direction:row;align-items:center;gap:4px;white-space:nowrap;"><input class="gb-tls" type="checkbox" checked />TLS</label>'
    + '<button type="button" class="gb-del" title="删除" aria-label="删除备用网关"><svg class="icon"><use href="#i-trash"/></svg></button>';
  li.querySelector('.gb-host').value = (b && b.host) || '';
  li.querySelector('.gb-port').value = (b && b.port) || '';
  li.querySelector('.gb-tls').checked = !b || b.tls !== false;
  li.querySelector('.gb-del').addEventListener('click', () => li.remove());
  ul.appendChild(li);
  return li;
}

function collectBackups() {
  const out = [];
  for (const li of document.querySelectorAll('#gwBackups .gw-backup-row')) {
    const host = li.querySelector('.gb-host').value.trim();
    if (!host) continue;
    out.push({
      host,
      port: Number(li.querySelector('.gb-port').value) || 443,
      tls: li.querySelector('.gb-tls').checked,
    });
  }
  return out;
}

/* ---------- 连通性检测(invoke gateway_health_check) ---------- */
export async function gatewayHealthCheck(cfg) {
  try {
    return await window.__TAURI__.core.invoke('gateway_health_check', { cfg });
  } catch (e) {
    return { ok: false, message: String(e && e.message || e) };
  }
}

/* ---------- 事件绑定(模块加载时执行一次) ---------- */
export function initGatewayForm() {
  const form = document.getElementById('connectForm');
  if (!form || !form.gwAddr || form.gwAddr.dataset.gwBound === '1') return;
  form.gwAddr.dataset.gwBound = '1';

  // 地址粘贴/输入自动拆分协议与端口
  form.gwAddr.addEventListener('change', () => {
    const p = splitGatewayAddr(form.gwAddr.value);
    if (!p) return;
    form.gwAddr.value = p.host;
    if (typeof p.port === 'number') form.gwPort.value = p.port;
    if (typeof p.tls === 'boolean') form.gwTls.checked = p.tls;
    syncGatewayUi();
  });

  form.gwAuth.addEventListener('change', syncGatewayUi);
  form.gwSkipVerify.addEventListener('change', syncGatewayUi);

  document.getElementById('gwAddBackup').addEventListener('click', () => addBackupRow(null));

  // 检测网关连通性:主网关(或首个备用网关)发起,绿/红 + 延迟
  document.getElementById('btnGwCheck').addEventListener('click', async () => {
    const msg = document.getElementById('gwCheckMsg');
    const gw = collectGatewayForm();
    const cfg = gw || {
      host: form.gwAddr.value.trim(),
      port: Number(form.gwPort.value) || 443,
      tls: form.gwTls.checked,
      skipVerify: form.gwSkipVerify.checked,
      auth: form.gwAuth.value,
      token: form.gwToken.value,
      username: form.gwUser.value.trim(),
      password: form.gwPass.value,
      resourceId: form.gwResource.value.trim(),
    };
    if (!cfg.host) { msg.textContent = '请先填写网关地址'; msg.style.color = 'var(--color-danger)'; return; }
    msg.textContent = '正在检测网关连通性…';
    msg.style.color = 'var(--color-text-secondary)';
    const t0 = Date.now();
    const r = await gatewayHealthCheck(cfg);
    const ms = Date.now() - t0;
    if (r && r.ok) {
      const latency = typeof r.latencyMs === 'number' ? r.latencyMs : ms;
      msg.textContent = '✅ 网关连通(' + escH(String(latency)) + 'ms)';
      msg.style.color = 'var(--color-success, #2ecc71)';
    } else {
      msg.textContent = '❌ 网关不可达: ' + escH(String((r && r.message) || '未知错误'));
      msg.style.color = 'var(--color-danger)';
    }
  });

  syncGatewayUi();
}
