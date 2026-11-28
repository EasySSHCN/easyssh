/* ============================================================
 * settings.js — 设置 / 皮肤 / 初始化（从 renderer.js 提取）
 * 职责：皮肤 THEMES 与终端设置应用、设置弹窗（打开/保存）、
 *       启动时设置加载与快捷命令播种（initSettings）。
 * ============================================================ */
import { $ } from './dom.js';
import { tabs, sessions, appSettings, setAppSettings, setSessions, DEFAULT_QUICK_CMDS, sessKey, ver } from './state.js';
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
  window.dispatchEvent(new Event('easyssh-theme-change')); // 标签自定义颜色按主题对比度重刷(用户反馈 2026-11-28)
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
  // v5.1.0 匿名使用统计开关(旧配置缺字段=默认开启)
  $('#setStats').checked = appSettings.statsEnabled !== false;
  $('#statsMsg').textContent = appSettings.statsEnabled === false ? '当前: 已关闭' : '当前: 已开启';
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
  // v5.1.0 匿名使用统计开关:持久化,下次启动生效
  appSettings.statsEnabled = $('#setStats').checked;
  await window.easyssh.saveSettings(appSettings);
  if (window.EasySSH_I18N_apply) window.EasySSH_I18N_apply(appSettings.lang);
  applyUiTheme();
  applyTermSettings();
  renderQuickbar(); // v5.1.4:语言切换后快捷命令栏默认命令标签同步切换
  $('#settingsModal').classList.add('hidden');
  $('#statusBar').textContent = '设置已保存' + (appSettings.encoding !== 'utf8' ? '(编码对新连接生效)' : '')
    + (appSettings.sessionLogEnabled ? ';会话日志对新连接生效' : '');
});

/* ================= v5.0.9/F-R9-01 快捷方式手动生成 =================
 * 设置 → 快捷方式：手动点击生成 桌面 / 快速启动栏 快捷方式
 * （升级完成流程仍会自动同步刷新两类快捷方式指向新版文件）。 */
$('#btnMakeDesktop').addEventListener('click', () => makeShortcut('desktop'));
$('#btnMakeQuickLaunch').addEventListener('click', () => makeShortcut('quicklaunch'));
async function makeShortcut(kind) {
  const msg = $('#shortcutMsg');
  msg.textContent = '正在生成…';
  try {
    const r = await window.easyssh.shortcutCreate(kind);
    msg.textContent = r || '✅ 已生成';
    msg.style.color = 'var(--color-text-secondary)';
  } catch (e) {
    msg.textContent = String(e || '生成失败');
    msg.style.color = 'var(--color-danger)';
  }
}

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
  // v5.1.4 修复:启动时按已保存的界面语言立即应用翻译(此前只在设置保存时应用,
  // 导致重启后语言回到中文,设置里的语言选择没有被记住)
  if (window.EasySSH_I18N_apply) window.EasySSH_I18N_apply(appSettings.lang || 'zh-CN');
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

/* ================= v5.2.x 备份与同步(配置打包导出/导入 + WebDAV 云同步) =================
 * 对接后端 backup_sync.rs 命令(cfg_export_package / cfg_import_preflight /
 * cfg_import_package / cfg_webdav_test / cfg_webdav_push / cfg_webdav_pull)。
 * WebDAV 配置持久化在 appSettings.webdavCfg 对象(url/username/password/filename),
 * 随 saveSettings 落盘到 config.json 的 settings 段(config.json 本身有加密语义)。
 * 注意:后端 cfg_webdav_pull 直接按 mode 导入(无预检命令),故拉取恢复为
 * 「先选模式 → 执行」流程,替换模式有二次确认。 */
const BAK_DEFAULT_FILENAME = 'easyssh-backup.esshbak';

// v5.2.x 消息文案按现有 i18n 机制翻译(拼接消息逐段走字典,完整节点由 apply 翻译)
function bsT(zh) {
  try {
    const loc = appSettings.lang || 'zh-CN';
    if (loc !== 'zh-CN' && window.EasySSH_I18N && window.EasySSH_I18N.t) return window.EasySSH_I18N.t(loc, zh);
  } catch {}
  return zh;
}

function bsSetMsg(el, text, cls) {
  el.textContent = text;
  el.classList.remove('ok', 'err');
  if (cls) el.classList.add(cls);
}
// 异步按钮防重复提交:置禁用,完成/失败恢复文案
function bsBusy(btn, on, busyText) {
  if (on) {
    btn.dataset.origText = btn.textContent;
    btn.disabled = true;
    if (busyText) btn.textContent = busyText;
  } else {
    btn.disabled = false;
    if (btn.dataset.origText) btn.textContent = btn.dataset.origText;
  }
}
// 口令输入框「显示/隐藏」切换(事件委托,覆盖 bsPass 与 bsDavPass)
document.addEventListener('click', (e) => {
  const b = e.target.closest && e.target.closest('.bs-eye');
  if (!b) return;
  const inp = $('#' + b.dataset.for);
  if (!inp) return;
  const show = inp.type === 'password';
  inp.type = show ? 'text' : 'password';
  b.textContent = show ? bsT('🙈 隐藏') : bsT('👁 显示');
}, true);

/* ---------- WebDAV 配置:读写 appSettings.webdavCfg ---------- */
function davCfgFromUi() {
  return {
    url: $('#bsDavUrl').value.trim(),
    username: $('#bsDavUser').value,
    password: $('#bsDavPass').value,
    filename: $('#bsDavFile').value.trim() || BAK_DEFAULT_FILENAME,
  };
}
function davFillUi() {
  const c = (appSettings.webdavCfg && typeof appSettings.webdavCfg === 'object') ? appSettings.webdavCfg : {};
  $('#bsDavUrl').value = c.url || '';
  $('#bsDavUser').value = c.username || '';
  $('#bsDavPass').value = c.password || '';
  $('#bsDavFile').value = c.filename || '';
}
function davCfgValid(cfg) {
  if (!cfg.url) { bsSetMsg($('#bsDavMsg'), bsT('请填写服务器地址'), 'err'); return false; }
  return true;
}
$('#btnDavSave').addEventListener('click', async () => {
  const cfg = davCfgFromUi();
  if (!davCfgValid(cfg)) return;
  bsBusy($('#btnDavSave'), true, bsT('保存中…'));
  try {
    appSettings.webdavCfg = cfg;
    await window.easyssh.saveSettings(appSettings);
    bsSetMsg($('#bsDavMsg'), bsT('✅ WebDAV 配置已保存(密码存于 config.json 设置段)'), 'ok');
  } catch (e) {
    bsSetMsg($('#bsDavMsg'), bsT('保存失败: ') + String(e && e.message || e), 'err');
  } finally { bsBusy($('#btnDavSave'), false); }
});

/* ---------- 本地导出 ---------- */
$('#btnBakExport').addEventListener('click', async () => {
  const msg = $('#bsBakMsg');
  const pw = $('#bsPass').value;
  if ([...pw].length < 8) { bsSetMsg(msg, bsT('请输入至少 8 位的口令'), 'err'); return; }
  bsBusy($('#btnBakExport'), true, bsT('正在导出…'));
  bsSetMsg(msg, bsT('正在选择保存位置…'));
  try {
    const outPath = await window.easyssh.pickSave(BAK_DEFAULT_FILENAME);
    if (!outPath) { bsSetMsg(msg, bsT('已取消')); return; }
    bsSetMsg(msg, bsT('正在导出…'));
    const r = await window.easyssh.cfgExportPackage(pw, outPath);
    bsSetMsg(msg, bsT('✅ 已导出 ') + (r.sessions || 0) + bsT(' 个会话到: ') + (r.path || outPath), 'ok');
    $('#statusBar').textContent = bsT('✅ 备份已导出: ') + (r.path || outPath);
  } catch (e) {
    bsSetMsg(msg, bsT('导出失败: ') + String(e && e.message || e), 'err');
  } finally { bsBusy($('#btnBakExport'), false); }
});

/* ---------- 导入弹窗(本地导入预检 / WebDAV 拉取恢复共用) ---------- */
let bsPending = null; // { source:'local'|'dav', path }
function bsModalOpen(source, path, title, desc) {
  bsPending = { source, path: path || null };
  $('#bsImportTitle').textContent = title;
  bsSetMsg($('#bsImportDesc'), desc || '');
  $('#bsPreflight').style.display = 'none';
  $('#bsConflicts').style.display = 'none';
  $('#bsConflictList').innerHTML = '';
  $('#bsMergeBtn').disabled = false;
  $('#bsReplaceBtn').disabled = false;
  $('#bsImportModal').classList.remove('hidden');
}
function bsRenderPreflight(r) {
  const pf = $('#bsPreflight');
  const created = r.createdAt ? new Date(Number(r.createdAt) * 1000).toLocaleString() : bsT('(未知)');
  pf.textContent = bsT('预检结果') + '\n' + bsT('包内会话: ') + (r.sessionsTotal || 0)
    + '\n' + bsT('新增: ') + (r.sessionsNew || 0)
    + '\n' + bsT('更新(冲突): ') + (r.sessionsUpdate || 0)
    + '\n' + bsT('新增主机指纹: ') + (r.knownHostsNew || 0)
    + '\n' + bsT('备份包创建时间: ') + created;
  pf.style.display = 'block';
  const list = Array.isArray(r.sessionsConflict) ? r.sessionsConflict : [];
  const det = $('#bsConflicts');
  if (list.length) {
    $('#bsConflictsSummary').textContent = bsT('冲突列表(最多显示 50 条,共 ') + (r.sessionsUpdate || list.length) + bsT(' 条)');
    $('#bsConflictList').innerHTML = list.map((k) => '<li>' + String(k).replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c])) + '</li>').join('');
    det.style.display = 'block';
  } else {
    det.style.display = 'none';
  }
}
async function bsRunImport(mode) {
  const desc = $('#bsImportDesc');
  const mergeBtn = $('#bsMergeBtn'), repBtn = $('#bsReplaceBtn');
  mergeBtn.disabled = true; repBtn.disabled = true;
  bsSetMsg(desc, bsT('正在导入…'));
  try {
    const pw = $('#bsPass').value;
    let r;
    if (bsPending.source === 'local') {
      r = await window.easyssh.cfgImportPackage(pw, bsPending.path, mode);
    } else {
      r = await window.easyssh.cfgWebdavPull(pw, davCfgFromUi(), mode);
    }
    const modeTxt = mode === 'merge' ? bsT('合并(保留现有)') : bsT('整包替换');
    bsSetMsg(desc, bsT('✅ 导入完成(') + modeTxt + bsT('): 导入 ') + (r.imported || 0) + bsT(' 个, 跳过 ') + (r.skipped || 0)
      + (mode === 'merge' && Array.isArray(r.conflicts) && r.conflicts.length ? '\n' + bsT('冲突(保留现有): ') + r.conflicts.length + bsT(' 条') : '')
      + bsT(' (会话列表重启后刷新显示)'), 'ok');
    $('#statusBar').textContent = bsT('✅ 备份导入完成(') + modeTxt + bsT('): 新增 ') + (r.imported || 0) + bsT(' 个, 跳过 ') + (r.skipped || 0);
    // 同步内存会话快照(列表 UI 在下次重渲/重启时刷新)
    try {
      const list = await window.easyssh.loadSessions();
      setSessions(list || []);
    } catch {}
  } catch (e) {
    bsSetMsg(desc, bsT('导入失败: ') + String(e && e.message || e), 'err');
    mergeBtn.disabled = false; repBtn.disabled = false;
  }
}
$('#bsMergeBtn').addEventListener('click', () => bsRunImport('merge'));
$('#bsReplaceBtn').addEventListener('click', async () => {
  const ok = await window.easysshUi.confirm(bsT('⚠ 整包替换将覆盖现有全部会话(导入前会自动备份 config.json),确定继续?'));
  if (ok) bsRunImport('replace');
});

/* ---------- 本地导入:选文件 → 口令 → 预检 → 选择模式 ---------- */
$('#btnBakImport').addEventListener('click', async () => {
  const msg = $('#bsBakMsg');
  if (!$('#bsPass').value) { bsSetMsg(msg, bsT('请先输入备份口令'), 'err'); return; }
  bsBusy($('#btnBakImport'), true, bsT('正在选择文件…'));
  try {
    const paths = await window.easyssh.pickOpen();
    const path = Array.isArray(paths) ? paths[0] : paths;
    if (!path) { bsSetMsg(msg, bsT('已取消')); return; }
    bsSetMsg(msg, bsT('预检中…'));
    bsModalOpen('local', path, bsT('导入备份预检'), bsT('正在读取并校验备份包…'));
    $('#bsMergeBtn').disabled = true; $('#bsReplaceBtn').disabled = true;
    try {
      const r = await window.easyssh.cfgImportPreflight($('#bsPass').value, path);
      bsSetMsg($('#bsImportDesc'), bsT('请核对预检结果后选择导入方式:'));
      bsRenderPreflight(r);
    } catch (e) {
      bsSetMsg($('#bsImportDesc'), bsT('预检失败: ') + String(e && e.message || e), 'err');
    } finally { bsBusy($('#btnBakImport'), false); }
  } catch (e) {
    bsSetMsg(msg, bsT('导入失败: ') + String(e && e.message || e), 'err');
    bsBusy($('#btnBakImport'), false);
  }
});

/* ---------- WebDAV:测试连接 / 上传备份 / 拉取恢复 ---------- */
$('#btnDavTest').addEventListener('click', async () => {
  const cfg = davCfgFromUi();
  if (!davCfgValid(cfg)) return;
  bsBusy($('#btnDavTest'), true, bsT('测试中…'));
  bsSetMsg($('#bsDavMsg'), bsT('正在测试连接…'));
  try {
    const r = await window.easyssh.cfgWebdavTest(cfg);
    bsSetMsg($('#bsDavMsg'), bsT('✅ 测试连接成功(HTTP ') + (r.status || '') + bsT(', ') + (r.probe || '') + ')', 'ok');
  } catch (e) {
    bsSetMsg($('#bsDavMsg'), bsT('测试失败: ') + String(e && e.message || e), 'err');
  } finally { bsBusy($('#btnDavTest'), false); }
});
$('#btnDavPush').addEventListener('click', async () => {
  const cfg = davCfgFromUi();
  if (!davCfgValid(cfg)) return;
  const pw = $('#bsPass').value;
  if ([...pw].length < 8) { bsSetMsg($('#bsDavMsg'), bsT('请先在「本地备份」区输入至少 8 位的口令'), 'err'); return; }
  bsBusy($('#btnDavPush'), true, bsT('上传中…'));
  bsSetMsg($('#bsDavMsg'), bsT('正在打包并上传…'));
  try {
    const r = await window.easyssh.cfgWebdavPush(pw, cfg);
    bsSetMsg($('#bsDavMsg'), bsT('✅ 已上传: ') + (r.filename || cfg.filename) + ' (' + (r.bytes || 0) + bsT(' 字节)'), 'ok');
  } catch (e) {
    bsSetMsg($('#bsDavMsg'), bsT('上传失败: ') + String(e && e.message || e), 'err');
  } finally { bsBusy($('#btnDavPush'), false); }
});
$('#btnDavPull').addEventListener('click', () => {
  const cfg = davCfgFromUi();
  if (!davCfgValid(cfg)) return;
  if (!$('#bsPass').value) { bsSetMsg($('#bsDavMsg'), bsT('请先在「本地备份」区输入备份口令'), 'err'); return; }
  bsSetMsg($('#bsDavMsg'), '');
  bsModalOpen('dav', null, bsT('拉取恢复'), bsT('将从 ') + cfg.url + bsT(' 拉取 ') + cfg.filename + bsT(' 并导入。注意:拉取不支持预检,请选择导入方式:'));
});

/* ---------- 设置弹窗打开时填充 WebDAV 配置与状态 ---------- */
document.addEventListener('click', (e) => {
  if (e.target && e.target.id === 'btnSettings') {
    setTimeout(davFillUi, 20);
  }
}, true);
