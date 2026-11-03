/* ============================================================
 * settings.js — 设置 / 皮肤 / 初始化（从 renderer.js 提取）
 * 职责：皮肤 THEMES 与终端设置应用、设置弹窗（打开/保存）、
 *       启动时设置加载与快捷命令播种（initSettings）。
 * ============================================================ */
import { $ } from './dom.js';
import { tabs, appSettings, setAppSettings, DEFAULT_QUICK_CMDS } from './state.js';
import { renderQuickbar } from './shortcuts.js';
import { refreshMpStatus } from './masterpw.js';

/* ================= v2.0 设置 / 皮肤 ================= */
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

/* ---------- 设置弹窗 ---------- */
$('#btnSettings').addEventListener('click', async () => {
  setAppSettings(await window.easyssh.loadSettings());
  $('#setFont').value = appSettings.fontFamily;
  $('#setSize').value = appSettings.fontSize;
  $('#setEnc').value = appSettings.encoding;
  $('#setTheme').value = appSettings.theme;
  // v2.11 新增设置项(旧配置文件缺字段时 loadSettings 已补默认值)
  $('#setScrollback').value = String(Number(appSettings.scrollback) || 5000);
  $('#setCopyOnSelect').checked = !!appSettings.copyOnSelect;
  $('#setSessLog').checked = !!appSettings.sessionLogEnabled;
  $('#setLogDir').value = appSettings.sessionLogDir || '';
  // v3.0.8 界面语言
  $('#setLang').value = appSettings.lang || 'zh-CN';
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
  // v3.0.8 界面语言:保存后立即切换(无需重启),下次启动按此语言加载
  appSettings.lang = $('#setLang').value || 'zh-CN';
  await window.easyssh.saveSettings(appSettings);
  if (window.EasySSH_I18N_apply) window.EasySSH_I18N_apply(appSettings.lang);
  applyTermSettings();
  $('#settingsModal').classList.add('hidden');
  $('#statusBar').textContent = '设置已保存' + (appSettings.encoding !== 'utf8' ? '(编码对新连接生效)' : '')
    + (appSettings.sessionLogEnabled ? ';会话日志对新连接生效' : '');
});

/* ---------- 初始化设置/皮肤/快捷命令 ---------- */
export async function initSettings() {
  setAppSettings(await window.easyssh.loadSettings());
  // v4 补齐:从未写入过 quickCommands 时播种默认快捷命令(用户自定义/清空过则不动)
  if (!appSettings.quickCommands || typeof appSettings.quickCommands !== 'object' || !(appSettings.quickCommands.length >= 0)) {
    appSettings.quickCommands = DEFAULT_QUICK_CMDS.slice();
    try { await window.easyssh.saveSettings(appSettings); } catch {}
  }
  applyTermSettings();
  renderQuickbar();
}
