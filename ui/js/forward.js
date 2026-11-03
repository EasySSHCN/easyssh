/* ============================================================
 * forward.js — 本地端口转发（从 renderer.js 提取）
 * 职责：端口转发弹窗、转发列表（运行中/已保存）、启动/停止/
 *       编辑、SOCKS 动态转发、转发配置随会话持久化、pf 事件。
 * ============================================================ */
import { $ } from './dom.js';
import { tabs, activeId, sessions } from './state.js';
import { saveSessionsGuard } from './masterpw.js';

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
  // v5(M4):有已保存未启动的规则时,提供「一键应用全部」(后端批量启动,重复端口自动跳过)
  if (savedOnly.length && t && t.id) {
    const li = document.createElement('li');
    li.innerHTML = '<button class="primary" id="pfApplyAll" style="width:100%;">▶ 一键应用全部已保存转发(' + savedOnly.length + ' 条)</button>';
    li.querySelector('#pfApplyAll').addEventListener('click', async () => {
      const res = await window.easyssh.pfApplySession(activeId, t.opts.host + ':' + (t.opts.port || 22) + ':' + t.opts.username);
      if (!res || !res.ok) { $('#statusBar').textContent = '一键应用失败: ' + ((res && res.message) || '未知错误'); return; }
      $('#statusBar').textContent = '已批量应用转发: 启动 ' + (res.started || 0) + ' 条' + (res.skipped ? ',跳过重复端口 ' + res.skipped + ' 条' : '');
      t.term.write('\x1b[36m[端口转发] 一键应用: 启动 ' + (res.started || 0) + ' 条,跳过 ' + (res.skipped || 0) + ' 条]\x1b[0m\r\n');
      renderPfList();
    });
    ul.appendChild(li);
  }
  // 正在运行的转发:⏹ 停止(置于最前) + ✎ 编辑(以新配置重启)
  running.forEach((f) => {
    const li = document.createElement('li');
    li.innerHTML = '<span class="use" title="停止此转发"><svg class="icon"><use href="#i-pause"/></svg></span><b>' + (f.name || '') + '</b><span class="qc">' + (f.dynamic
      ? 'SOCKS4/5 动态代理 本机 127.0.0.1:' + f.localPort
      : '本机 127.0.0.1:' + f.localPort + ' → ' + f.remoteHost + ':' + f.remotePort) + ' (运行中)</span>'
      + '<span class="edit" title="编辑此转发"><svg class="icon"><use href="#i-edit"/></svg></span>';
    li.querySelector('.use').addEventListener('click', async () => {
      const res = await window.easyssh.pfStop(activeId, f.localPort);
      if (!res || !res.ok) { $('#statusBar').textContent = '停止失败: ' + ((res && res.message) || '未知错误'); return; }
      $('#statusBar').textContent = '转发已停止: 本地端口 ' + f.localPort;
      renderPfList();
    });
    li.querySelector('.edit').addEventListener('click', () => editPfConfig(f));
    ul.appendChild(li);
  });
  // 已保存但未运行的转发记录:▶ 一键启动 / ✎ 编辑 / ✕ 仅删除记录(不影响运行中的转发)
  savedOnly.forEach((f) => {
    const li = document.createElement('li');
    li.innerHTML = '<b>' + (f.name || '') + '</b><span class="qc">' + (f.dynamic
      ? 'SOCKS4/5 动态代理 本机 127.0.0.1:' + f.localPort
      : '本机 127.0.0.1:' + f.localPort + ' → ' + (f.remoteHost || '127.0.0.1') + ':' + f.remotePort) + ' (未启动)</span>'
      + '<span class="use" title="启动此转发"><svg class="icon"><use href="#i-play"/></svg></span><span class="edit" title="编辑此转发"><svg class="icon"><use href="#i-edit"/></svg></span><span class="del" title="删除此记录"><svg class="icon"><use href="#i-x"/></svg></span>';
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
