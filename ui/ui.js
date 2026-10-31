/* EasySSH v4 PoC：Tauri invoke/event 与 russh 后端联调 */
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let term = null, fit = null, connId = null;

function initTerm() {
  term = new window.Terminal({
    fontSize: 14, fontFamily: 'Consolas, monospace',
    theme: { background: '#181825', foreground: '#cdd6f4' },
    scrollback: 5000, cursorBlink: true,
  });
  fit = new window.FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(document.getElementById('term'));
  fit.fit();
  window.addEventListener('resize', () => fit.fit());
  term.onData((data) => {
    if (connId !== null) invoke('ssh_write', { id: connId, data }).catch(() => {});
  });
}

(async () => {
  initTerm();
  await listen('ssh-data', (ev) => {
    if (ev.payload.id === connId) term.write(ev.payload.data);
  });
  await listen('ssh-closed', (ev) => {
    if (ev.payload.id === connId) {
      term.write('\r\n\x1b[33m[连接已断开]\x1b[0m\r\n');
      document.getElementById('status').textContent = '已断开';
      connId = null;
    }
  });
})();

document.getElementById('connectForm').addEventListener('submit', async (e) => {
  e.preventDefault();
  const status = document.getElementById('status');
  try {
    status.textContent = '连接中…';
    connId = await invoke('ssh_connect', {
      host: document.getElementById('host').value,
      port: Number(document.getElementById('port').value) || 22,
      user: document.getElementById('user').value || 'root',
      password: document.getElementById('password').value,
      cols: term.cols, rows: term.rows,
    });
    status.textContent = '已连接 (id=' + connId + ')';
    term.writeln('\x1b[32m[已连接 ' + document.getElementById('host').value + ']\x1b[0m');
    term.focus();
  } catch (err) {
    status.textContent = '连接失败: ' + err;
    term.writeln('\x1b[31m[连接失败: ' + err + ']\x1b[0m');
  }
});
