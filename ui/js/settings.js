/* ============================================================
 * settings.js — 设置 / 皮肤 / 初始化（从 renderer.js 提取）
 * 职责：皮肤 THEMES 与终端设置应用、设置弹窗（打开/保存）、
 *       启动时设置加载与快捷命令播种（initSettings）。
 * ============================================================ */
import { $ } from './dom.js';
import { tabs, sessions, appSettings, setAppSettings, DEFAULT_QUICK_CMDS, sessKey, ver } from './state.js';
import { renderQuickbar } from './shortcuts.js';
import { refreshMpStatus, saveSessionsGuard } from './masterpw.js';

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

const DEFAULT_THEME_KEY = 'dracula';

function applyTermSettings() {
  const t = THEMES[appSettings.theme] || THEMES.dracula;
  for (const tab of tabs.values()) {
    // v5(M4) 修复:分屏后遍历全部窗格(此前只经 tab.term 委派活动窗格,其余窗格收不到字体/主题/渲染器变更)
    const panes = (Array.isArray(tab.panes) && tab.panes.length) ? tab.panes : [tab];
    for (const p of panes) {
      if (!p.term) continue;
      p.term.options.fontFamily = appSettings.fontFamily + ', monospace';
      p.term.options.fontSize = Number(appSettings.fontSize) || 14;
      p.term.options.theme = t.term;
      p.term.options.scrollback = Number(appSettings.scrollback) || 5000; // v2.11 回滚行数运行时生效
      // v5(M2):光标样式/闪烁运行时生效
      p.term.options.cursorStyle = appSettings.cursorStyle || 'block';
      p.term.options.cursorBlink = !!appSettings.cursorBlink;
      // v5(M2):渲染器设置——「DOM」即时降级(dispose webgl,xterm 自动回退 DOM 渲染);
      // 「自动/WebGL」下已有 webgl 不动,无 webgl 的已有标签需新开标签生效
      const want = appSettings.termRenderer || 'auto';
      if (want === 'dom' && p.webgl) { try { p.webgl.dispose(); } catch {} p.webgl = null; }
    }
  }
  // v5(M3):界面 token 映射。默认皮肤(dracula)不再内联覆盖 UI token——
  // 深浅主题由 design-tokens.css 的 [data-theme] 驱动,品牌色走 Emerald;
  // 用户显式选择的非默认皮肤仍完整覆盖界面 token(皮肤系统照常工作)。
  const root = document.documentElement;
  if (appSettings.theme && appSettings.theme !== DEFAULT_THEME_KEY) {
    const map = {
      bg: ['--color-bg-primary', '--color-bg-elevated'],
      bg2: '--color-bg-secondary',
      panel: ['--color-bg-elevated', '--color-bg-hover'],
      fg: '--color-text-primary',
      accent: ['--color-accent', '--color-accent-strong', '--color-accent-hover'],
      border: ['--color-border-subtle', '--color-border-strong', '--color-border-input'],
      redbtn: '--color-danger',
    };
    for (const [k, v] of Object.entries(t.ui)) {
      for (const token of [].concat(map[k] || [])) root.style.setProperty(token, v);
    }
  } else {
    // 回到默认皮肤:清除所有内联覆盖,恢复 token 主题系统
    for (const token of ['--color-bg-primary', '--color-bg-elevated', '--color-bg-secondary', '--color-bg-hover',
      '--color-text-primary', '--color-accent', '--color-accent-strong', '--color-accent-hover',
      '--color-border-subtle', '--color-border-strong', '--color-border-input', '--color-danger']) {
      root.style.removeProperty(token);
    }
  }
  // v5(M2/M3):终端背景同步 --color-bg-terminal token 通道(浅色 UI 主题下终端仍保持深底;
  // xterm 主题只跟随皮肤,不随 UI 主题翻转)
  root.style.setProperty('--color-bg-terminal', t.term.background);
  window.easyssh.setWinTheme(t.ui.bg2, t.ui.fg);
}

/* ---------- v5(M3) 深浅 UI 主题(与终端皮肤 theme 字段独立,持久化到 theme2) ---------- */
export function applyUiTheme() {
  document.documentElement.dataset.theme = appSettings.theme2 === 'light' ? 'light' : 'dark';
}

/* ---------- 设置弹窗 ---------- */
$('#btnSettings').addEventListener('click', async () => {
  setAppSettings(await window.easyssh.loadSettings());
  $('#setFont').value = appSettings.fontFamily;
  $('#setSize').value = appSettings.fontSize;
  $('#setEnc').value = appSettings.encoding;
  $('#setTheme').value = appSettings.theme;
  // v5(M3) 深浅 UI 主题(独立于终端皮肤)
  $('#setUiTheme').value = appSettings.theme2 === 'light' ? 'light' : 'dark';
  // v2.11 新增设置项(旧配置文件缺字段时 loadSettings 已补默认值)
  $('#setScrollback').value = String(Number(appSettings.scrollback) || 5000);
  $('#setCopyOnSelect').checked = !!appSettings.copyOnSelect;
  $('#setSessLog').checked = !!appSettings.sessionLogEnabled;
  $('#setLogDir').value = appSettings.sessionLogDir || '';
  // v3.0.8 界面语言
  $('#setLang').value = appSettings.lang || 'zh-CN';
  refreshMpStatus(); // v3.0.0 主密码状态与启用/更改/关闭入口
  $('#aboutVer').textContent = 'EasySSH ' + (ver ? 'v' + ver : 'v5'); // v5(M3) 关于页版本号
  setPanel('general'); // v5(M3):每次打开回到「常规」分组
  $('#settingsModal').classList.remove('hidden');
});

/* ---------- v5(M3) 设置分组导航(左侧 nav + 右侧面板切换) ---------- */
function setPanel(name) {
  document.querySelectorAll('#settingsModal .settings-nav .item').forEach((b) => {
    const on = b.dataset.panel === name;
    b.classList.toggle('on', on);
    b.setAttribute('aria-selected', on ? 'true' : 'false');
  });
  document.querySelectorAll('#settingsModal .settings-panel').forEach((p) => {
    p.classList.toggle('on', p.dataset.panel === name);
  });
  const body = document.querySelector('#settingsModal .settings-body');
  if (body) body.scrollTop = 0;
}
document.querySelectorAll('#settingsModal .settings-nav .item').forEach((b) => {
  b.addEventListener('click', () => setPanel(b.dataset.panel));
});
$('#setSave').addEventListener('click', async () => {
  appSettings.fontFamily = $('#setFont').value;
  appSettings.fontSize = Number($('#setSize').value) || 14;
  appSettings.encoding = $('#setEnc').value;
  appSettings.theme = $('#setTheme').value;
  // v5(M3) 深浅 UI 主题:立即切换并持久化(与终端皮肤 theme 字段分开存放)
  appSettings.theme2 = $('#setUiTheme').value === 'light' ? 'light' : 'dark';
  // v2.11 新增设置项:回滚行数 / 选中即复制 / 会话日志
  appSettings.scrollback = Number($('#setScrollback').value) || 5000;
  appSettings.copyOnSelect = $('#setCopyOnSelect').checked;
  appSettings.sessionLogEnabled = $('#setSessLog').checked;
  appSettings.sessionLogDir = $('#setLogDir').value.trim();
  // v3.0.8 界面语言:保存后立即切换(无需重启),下次启动按此语言加载
  appSettings.lang = $('#setLang').value || 'zh-CN';
  await window.easyssh.saveSettings(appSettings);
  if (window.EasySSH_I18N_apply) window.EasySSH_I18N_apply(appSettings.lang);
  applyUiTheme();
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
  applyUiTheme(); // v5(M3):先应用深浅 UI 主题,再应用终端皮肤
  applyTermSettings();
  renderQuickbar();
}

/* ================= v5(M2)/F-M2-05 代理与跳板 =================
 * index.html 属于其他代理的地盘，本区块由 JS 动态注入设置弹窗：
 * 会话选择器 + 代理表单（无/SOCKS5/HTTP CONNECT + 可选认证）+ 跳板链
 * （最多 3 级，从已存会话选择，复用其凭据）。配置写入会话记录的
 * `proxy` / `jumpChain` 字段（旧数据无此字段 = 无代理，v4 完全兼容）。 */
function npBuildDom() {
  const host = document.querySelector('#setNetBody');
  if (!host || $('#npSess')) return;
  const sec = document.createElement('div');
  sec.style.cssText = 'display:flex;flex-direction:column;gap:10px;';
  sec.innerHTML =
    '<div class="row" style="align-items:center;"><b style="font-size:13px;display:inline-flex;align-items:center;gap:6px;"><svg class="icon"><use href="#i-wifi"/></svg>网络接入（代理与跳板）:</b>' +
    '<select id="npSess" style="flex:1;"></select></div>' +
    '<label>代理类型' +
    '<select id="npType">' +
    '<option value="none">无(直连)</option>' +
    '<option value="socks5">SOCKS5</option>' +
    '<option value="http">HTTP CONNECT</option>' +
    '</select></label>' +
    '<div class="row"><input id="npHost" placeholder="代理地址" style="flex:1" />' +
    '<input id="npPort" type="number" min="1" max="65535" placeholder="端口" style="width:90px" /></div>' +
    '<div class="row"><input id="npUser" placeholder="认证用户名(可空)" style="flex:1" />' +
    '<input id="npPass" type="password" placeholder="认证密码(可空)" style="flex:1" /></div>' +
    '<label>跳板链(经由其他已存会话连接,最多 3 级)' +
    '<select id="npJump1"></select>' +
    '<select id="npJump2" style="margin-top:4px;"></select>' +
    '<select id="npJump3" style="margin-top:4px;"></select>' +
    '</label>' +
    '<div class="row">' +
    '<button class="primary" id="npSave">应用到所选会话</button>' +
    '<button id="npClear">清除代理与跳板</button>' +
    '<span id="npMsg" style="flex:1;font-size:12px;color:var(--color-text-secondary);"></span></div>' +
    '<div class="tips">跳板逐级复用已存会话的凭据与代理配置；连接顺序 = 跳板1 → 跳板2 → 跳板3 → 目标。仅对「保存的会话」生效，保存后新连接自动走该链路。</div>';
  host.appendChild(sec);
  $('#npSess').addEventListener('change', npLoad);
  $('#npSave').addEventListener('click', npSave);
  $('#npClear').addEventListener('click', npClear);
}

function npOptions() {
  const cur = $('#npSess').value;
  const opts = ['<option value="">— 不经由跳板 —</option>'];
  for (const s of sessions) {
    const k = sessKey(s);
    if (k === cur) continue; // 跳板不能是目标自身
    opts.push('<option value="' + k + '">' + (s.name || (s.username + '@' + s.host)) + '</option>');
  }
  return opts.join('');
}

function npFillSessions() {
  if (!$('#npSess')) npBuildDom();
  const sel = $('#npSess');
  if (!sel) return;
  const prev = sel.value;
  sel.innerHTML = sessions.map((s) => {
    const k = sessKey(s);
    return '<option value="' + k + '">' + (s.name || (s.username + '@' + s.host)) + '</option>';
  }).join('');
  if (prev && sessions.some((s) => sessKey(s) === prev)) sel.value = prev;
  npLoad();
  for (const id of ['npJump1', 'npJump2', 'npJump3']) {
    const el = $('#' + id);
    const pv = el.value;
    el.innerHTML = npOptions();
    if (pv) el.value = pv;
  }
}

function npLoad() {
  const s = sessions.find((x) => sessKey(x) === $('#npSess').value);
  const p = (s && s.proxy) || {};
  $('#npType').value = p.kind || 'none';
  $('#npHost').value = p.host || '';
  $('#npPort').value = p.port || '';
  $('#npUser').value = p.username || '';
  $('#npPass').value = p.password || '';
  const jc = Array.isArray(s && s.jumpChain) ? s.jumpChain : [];
  ['npJump1', 'npJump2', 'npJump3'].forEach((id, i) => {
    const el = $('#' + id);
    el.innerHTML = npOptions();
    el.value = jc[i] || '';
  });
  $('#npMsg').textContent = s
    ? (p.kind && p.kind !== 'none' ? '当前: ' + p.kind.toUpperCase() + ' ' + (p.host || '') + ':' + (p.port || '') : '当前: 直连')
      + (jc.length ? ' · 跳板 x' + jc.length : '')
    : '';
}

async function npSave() {
  const s = sessions.find((x) => sessKey(x) === $('#npSess').value);
  if (!s) { $('#npMsg').textContent = '请选择一个会话'; return; }
  const kind = $('#npType').value;
  if (kind !== 'none') {
    const host = $('#npHost').value.trim();
    const port = Number($('#npPort').value) || 0;
    if (!host || !port) { $('#npMsg').textContent = '请填写代理地址与端口'; return; }
    s.proxy = { kind, host, port };
    if ($('#npUser').value.trim()) {
      s.proxy.username = $('#npUser').value.trim();
      s.proxy.password = $('#npPass').value;
    }
  } else {
    delete s.proxy;
  }
  const jc = ['npJump1', 'npJump2', 'npJump3'].map((id) => $('#' + id).value).filter(Boolean);
  // 防环:链里不得出现目标会话自身,也不得重复
  const uniq = jc.filter((k, i) => k !== sessKey(s) && jc.indexOf(k) === i);
  if (uniq.length) s.jumpChain = uniq.slice(0, 3); else delete s.jumpChain;
  await saveSessionsGuard(sessions);
  npLoad();
  $('#npMsg').textContent = '✅ 已保存到会话「' + (s.name || s.username + '@' + s.host) + '」(对新连接生效)';
}

async function npClear() {
  const s = sessions.find((x) => sessKey(x) === $('#npSess').value);
  if (!s) return;
  delete s.proxy;
  delete s.jumpChain;
  await saveSessionsGuard(sessions);
  npLoad();
  $('#npMsg').textContent = '已清除:该会话恢复直连';
}

// 设置弹窗打开时填充会话选择器（扩写既有打开逻辑）
document.addEventListener('click', (e) => {
  if (e.target && e.target.id === 'btnSettings') {
    setTimeout(npFillSessions, 0); // 在既有 handler 填完基础设置后执行
    setTimeout(tcFill, 15);        // v5(M2):渲染器/光标区在代理区之后填充
  }
}, true);

/* ================= v5(M2)/F-M2-04 终端渲染与光标 =================
 * 渲染器(自动/WebGL/DOM)设置 + 光标样式/闪烁。DOM 模式即时降级
 * (dispose webgl);切回 WebGL 需新开标签生效。配置存 appSettings
 * 的 termRenderer/cursorStyle/cursorBlink 字段(旧配置缺字段=自动/块状/不闪烁)。 */
function tcBuildDom() {
  const host = document.querySelector('#setTermBody');
  if (!host || $('#tcRenderer')) return;
  const sec = document.createElement('div');
  sec.style.cssText = 'display:flex;flex-direction:column;gap:10px;';
  sec.innerHTML =
    '<div class="row" style="align-items:center;"><b style="font-size:13px;display:inline-flex;align-items:center;gap:6px;"><svg class="icon"><use href="#i-terminal"/></svg>终端渲染与光标:</b></div>' +
    '<label>渲染器' +
    '<select id="tcRenderer">' +
    '<option value="auto">自动(推荐,WebGL 可用时 GPU 加速)</option>' +
    '<option value="webgl">强制 WebGL</option>' +
    '<option value="dom">DOM(兼容模式)</option>' +
    '</select></label>' +
    '<div class="row"><label>光标样式' +
    '<select id="tcCursor">' +
    '<option value="block">块状</option>' +
    '<option value="bar">竖线</option>' +
    '<option value="underline">下划线</option>' +
    '</select></label>' +
    '<label style="display:flex;align-items:center;gap:6px;"><input type="checkbox" id="tcBlink" /> 光标闪烁</label></div>' +
    '<div class="row"><button class="primary" id="tcApply">应用渲染与光标</button>' +
    '<span id="tcMsg" style="flex:1;font-size:12px;color:var(--color-text-secondary);"></span></div>' +
    '<div class="tips">渲染器切换:「DOM」对已开标签即时生效;切回 WebGL 需新开标签。自动模式下 WebGL 不可用时自动回退 DOM。</div>';
  host.appendChild(sec);
  $('#tcApply').addEventListener('click', tcApply);
}

function tcFill() {
  if (!$('#tcRenderer')) tcBuildDom();
  if (!$('#tcRenderer')) return;
  $('#tcRenderer').value = appSettings.termRenderer || 'auto';
  $('#tcCursor').value = appSettings.cursorStyle || 'block';
  $('#tcBlink').checked = !!appSettings.cursorBlink;
}

async function tcApply() {
  appSettings.termRenderer = $('#tcRenderer').value || 'auto';
  appSettings.cursorStyle = $('#tcCursor').value || 'block';
  appSettings.cursorBlink = $('#tcBlink').checked;
  await window.easyssh.saveSettings(appSettings);
  applyTermSettings();
  $('#statusBar').textContent = '✅ 渲染与光标设置已保存' + (appSettings.termRenderer !== 'dom' ? '(WebGL 升级对新开标签生效)' : '(DOM 模式已即时生效)');
}
