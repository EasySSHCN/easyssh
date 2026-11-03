/* ============================================================
 * palette.js — v5(M4) F-M4-05 全局命令面板
 * Ctrl+Shift+P 呼出 / Esc 关闭;居中搜索框 + 分组结果列表;
 * ↑↓ 选择、Enter 执行;样式全部走 design-tokens 的 token 变量。
 * 数据源分组:
 *   会话     → 点击回填连接表单并直接提交连接(fillConnectForm 语义复刻)
 *   快捷命令 → 复用 shortcuts.js 的 runQuickCommand(含宏/占位符解析)
 *   设置项   → 打开设置弹窗并跳到对应分组卡片
 *   视图动作 → SFTP/端口转发/快捷命令/查找/分屏布局/深浅主题切换等
 * 无外部依赖;通过 DOM click 复用既有接线(与 terminal.js 零加载期耦合)。
 * ============================================================ */
import { $ } from './dom.js';
import { tabs, activeId, sessions, appSettings, setPendingSess } from './state.js';
import { runQuickCommand } from './shortcuts.js';

/* ---------- 面板 DOM(动态构建,避免改动 index.html 结构) ---------- */
let overlay, input, list, hint;
let paletteItems = []; // 本次打开时的全量动作
let filtered = [];     // 当前过滤结果
let selIndex = 0;      // 当前选中下标

function buildDom() {
  overlay = document.createElement('div');
  overlay.id = 'paletteOverlay';
  overlay.className = 'hidden';
  overlay.setAttribute('role', 'dialog');
  overlay.setAttribute('aria-modal', 'true');
  overlay.setAttribute('aria-label', '命令面板');
  overlay.innerHTML =
    '<div id="paletteBox">' +
    '<input id="paletteInput" role="combobox" aria-expanded="true" aria-controls="paletteList" aria-autocomplete="list" placeholder="搜索:会话 / 快捷命令 / 设置 / 视图动作…" autocomplete="off" spellcheck="false" />' +
    '<ul id="paletteList" role="listbox" aria-label="命令结果"></ul>' +
    '<div id="paletteHint">↑↓ 选择 · Enter 执行 · Esc 关闭 · Ctrl+Shift+P 呼出</div>' +
    '</div>';
  document.body.appendChild(overlay);
  input = overlay.querySelector('#paletteInput');
  list = overlay.querySelector('#paletteList');
  hint = overlay.querySelector('#paletteHint');

  input.addEventListener('input', () => { applyFilter(input.value); });
  input.addEventListener('keydown', (e) => {
    e.stopPropagation(); // 面板内按键不透传到终端/全局快捷键
    if (e.key === 'ArrowDown') { e.preventDefault(); moveSel(1); }
    else if (e.key === 'ArrowUp') { e.preventDefault(); moveSel(-1); }
    else if (e.key === 'Enter') { e.preventDefault(); runSel(); }
    else if (e.key === 'Escape') { e.preventDefault(); closePalette(); }
  });
  list.addEventListener('click', (e) => {
    const li = e.target.closest('li.p-item');
    if (li) { selIndex = Number(li.dataset.idx); runSel(); }
  });
  overlay.addEventListener('click', (e) => { if (e.target === overlay) closePalette(); }); // 点遮罩关闭
}

/* ---------- 动作收集 ---------- */
function clickBtn(id) { const b = $(id); if (b) b.click(); }

/* 会话动作:复刻 sessions.js fillConnectForm 语义(回填表单 + setPendingSess)后直接提交 */
function connectSession(s) {
  setPendingSess(s);
  $('#connectPanel').style.display = 'block';
  const form = $('#connectForm');
  form.host.value = s.host; form.port.value = s.port || 22;
  form.username.value = s.username;
  const at = s.authtype === 'key' && s.privateKey ? 'key' : 'password';
  form.authtype.value = at;
  form.authtype.dispatchEvent(new Event('change'));
  form.password.value = at === 'password' ? (s.password || '') : '';
  form.privateKey.value = at === 'key' ? (s.privateKey || '') : '';
  form.passphrase.value = at === 'key' ? (s.passphrase || '') : '';
  form.requestSubmit();
}

/* 设置动作:打开设置弹窗并切到对应分组(复用 settings.js 的导航接线) */
function openSettingsPanel(panel) {
  clickBtn('#btnSettings');
  const nav = document.querySelector('#settingsModal .settings-nav .item[data-panel="' + panel + '"]');
  if (nav) nav.click();
}

/* 分屏布局动作:点击 #splitMenu 中 terminal.js 已接线的菜单项(零耦合复用) */
function runSplitAction(selector) {
  const t = tabs.get(activeId);
  if (!t) { $('#statusBar').textContent = '请先连接一个会话,再使用分屏'; return; }
  const item = document.querySelector('#splitMenu ' + selector);
  if (item) item.click();
}

function toggleUiTheme() {
  const next = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
  document.documentElement.dataset.theme = next;
  appSettings.theme2 = next;
  window.easyssh.saveSettings(appSettings);
  $('#statusBar').textContent = '界面主题已切换: ' + (next === 'light' ? '浅色' : '深色');
}

function collectItems() {
  const items = [];
  /* 会话 */
  for (const s of sessions) {
    items.push({
      group: '会话',
      label: s.name || (s.username + '@' + s.host),
      hint: s.host + ':' + (s.port || 22) + (s.note ? ' · ' + s.note : ''),
      run: () => connectSession(s),
    });
  }
  /* 快捷命令(宏语义由 runQuickCommand 统一处理) */
  for (const qc of (appSettings.quickCommands || [])) {
    const first = String(qc.cmd || '').split('\n')[0] || '';
    items.push({
      group: '快捷命令',
      label: qc.name,
      hint: first + (String(qc.cmd || '').includes('\n') ? ' …(多行宏)' : ''),
      run: () => runQuickCommand(qc),
    });
  }
  /* 设置项 */
  const panels = [
    ['general', '打开设置:常规'], ['appearance', '打开设置:外观(深浅主题/终端皮肤)'],
    ['terminal', '打开设置:终端(字体/编码/行为)'], ['sftp', '打开设置:SFTP'],
    ['network', '打开设置:网络(代理/跳板)'], ['update', '打开设置:升级'], ['about', '打开设置:关于'],
  ];
  for (const [p, label] of panels) items.push({ group: '设置', label, hint: '设置', run: () => openSettingsPanel(p) });
  /* 视图动作 */
  items.push({ group: '视图', label: '新建连接', hint: '', run: () => clickBtn('#btnNew') });
  items.push({ group: '视图', label: '打开 SFTP 文件传输', hint: '', run: () => clickBtn('#btnSftp') });
  items.push({ group: '视图', label: '打开本地端口转发', hint: '', run: () => clickBtn('#btnPf') });
  items.push({ group: '视图', label: '管理快捷命令(脚本库)', hint: '', run: () => clickBtn('#btnQuick') });
  items.push({ group: '视图', label: '查找终端内容', hint: 'Ctrl+F', run: () => clickBtn('#btnFind') });
  items.push({ group: '视图', label: '切换界面深浅主题', hint: '深色/浅色', run: toggleUiTheme });
  items.push({ group: '视图', label: '分屏:左右分屏', hint: '', run: () => runSplitAction('[data-layout="split2v"]') });
  items.push({ group: '视图', label: '分屏:上下分屏', hint: '', run: () => runSplitAction('[data-layout="split2h"]') });
  items.push({ group: '视图', label: '分屏:四宫格(2×2)', hint: '', run: () => runSplitAction('[data-layout="grid"]') });
  items.push({ group: '视图', label: '分屏:恢复单屏', hint: '', run: () => runSplitAction('[data-layout="single"]') });
  items.push({ group: '视图', label: '分屏:新增窗格(再开一个会话)', hint: '', run: () => runSplitAction('[data-newpane]') });
  return items;
}

/* ---------- v5(beta) 任务三:拼音首字母过滤(零依赖) ----------
 * 内置「常用词条 → 拼音首字母」小映射表,只覆盖面板数据源的常用词(约 55 词条),
 * 不引 npm 包/在线服务;数据源条目可携带可选 pinyinShort 字段(如用户自定义会话名)
 * 直接参与首字母匹配。整表查不到的汉字按 '_' 占位(不产生误匹配)。
 */
const PINYIN_SHORT = {
  /* 常用二字词 */
  '新建': 'xj', '连接': 'lj', '设置': 'sz', '端口': 'dk', '转发': 'zf',
  '快捷': 'kj', '快速': 'ks', '命令': 'ml', '查找': 'cz', '终端': 'zd',
  '外观': 'wg', '网络': 'wl', '升级': 'sj', '关于': 'gy', '常规': 'cg',
  '会话': 'hs', '分屏': 'fp', '主题': 'zt', '字体': 'zt', '界面': 'jm',
  '打开': 'dk', '本地': 'bd', '文件': 'wj', '传输': 'cs', '内容': 'nr',
  '切换': 'qh', '脚本': 'jb', '管理': 'gl', '新增': 'xz', '窗格': 'cg',
  '恢复': 'hf', '单屏': 'dp', '再开': 'zk', '一个': 'yg', '深浅': 'sq',
  '浅色': 'qs', '深色': 'ss', '密码': 'mm', '密钥': 'my', '主机': 'zj',
  '用户': 'yh', '代理': 'dl', '皮肤': 'pf', '字号': 'zh', '编码': 'bm',
  '日志': 'rz', '反馈': 'fk', '帮助': 'bz', '版本': 'bb', '服务器': 'fwq',
  '克隆': 'kl', '删除': 'sc', '编辑': 'bj', '备注': 'bz', '颜色': 'ys',
  /* 多字固定词(整词优先,首字母更自然) */
  '左右分屏': 'zypf', '上下分屏': 'sxfp', '四宫格': 'sgg',
  '新建连接': 'xj', '端口转发': 'dkzf', '快捷命令': 'kjml', '快速命令': 'ksml',
  '脚本库': 'jbk', '深浅主题': 'sqzt', '文件传输': 'wjcs',
  '管理快捷命令': 'glkjml', '恢复单屏': 'hfdp', '新增窗格': 'xzcg',
  '本地端口转发': 'bddkzf', '打开设置': 'dksz',
};

/* 对一段文本按「最长优先」扫描映射表生成首字母串;表外汉字 → '_',ASCII 原样保留 */
function pinyinInitials(text) {
  let out = '';
  let i = 0;
  while (i < text.length) {
    const ch = text[i];
    if (/[\u4e00-\u9fff]/.test(ch)) {
      let matched = '';
      for (let len = 4; len >= 1; len--) { // 表内最长词条 6 字,按 4 起扫足够覆盖两字组合+单字
        if (i + len <= text.length && PINYIN_SHORT[text.substr(i, len)] !== undefined) {
          matched = PINYIN_SHORT[text.substr(i, len)];
          i += len;
          break;
        }
      }
      if (!matched) { out += '_'; i += 1; }
      else out += matched;
    } else {
      out += ch;
      i += 1;
    }
  }
  return out;
}

/* ---------- 过滤与渲染 ---------- */
function applyFilter(q) {
  const s = String(q || '').trim().toLowerCase();
  if (!s) {
    filtered = paletteItems.slice();
  } else {
    // 匹配优先级:前缀(3) > 子串(2) > 拼音首字母(1)
    const scored = [];
    for (const it of paletteItems) {
      const label = it.label.toLowerCase();
      const hay = (it.label + ' ' + (it.hint || '') + ' ' + it.group).toLowerCase();
      let score = -1;
      if (label.startsWith(s)) score = 3;
      else if (hay.includes(s)) score = 2;
      else if (/[a-z]/.test(s)) {
        const py = (it.pinyinShort ? String(it.pinyinShort).toLowerCase() + ' ' : '')
          + pinyinInitials(it.label + ' ' + it.group + ' ' + (it.hint || ''));
        if (py.includes(s)) score = 1;
      }
      if (score >= 0) scored.push({ it, score });
    }
    scored.sort((a, b) => b.score - a.score);
    filtered = scored.map((x) => x.it);
  }
  selIndex = 0;
  renderList();
}

function renderList() {
  list.innerHTML = '';
  if (!filtered.length) {
    const li = document.createElement('li');
    li.className = 'p-empty';
    li.textContent = '(无匹配项)';
    list.appendChild(li);
    input.setAttribute('aria-activedescendant', '');
    return;
  }
  let lastGroup = null;
  filtered.forEach((it, i) => {
    if (it.group !== lastGroup) {
      lastGroup = it.group;
      const gh = document.createElement('li');
      gh.className = 'p-group';
      gh.setAttribute('role', 'presentation');
      gh.textContent = it.group;
      list.appendChild(gh);
    }
    const li = document.createElement('li');
    li.className = 'p-item' + (i === selIndex ? ' sel' : '');
    li.id = 'p-opt-' + i;
    li.dataset.idx = i;
    li.setAttribute('role', 'option');
    li.setAttribute('aria-selected', i === selIndex ? 'true' : 'false');
    li.setAttribute('aria-label', it.group + ' - ' + it.label);
    const lab = document.createElement('span');
    lab.className = 'p-label';
    lab.textContent = it.label;
    li.appendChild(lab);
    if (it.hint) {
      const h = document.createElement('span');
      h.className = 'p-hint';
      h.textContent = it.hint;
      li.appendChild(h);
    }
    list.appendChild(li);
  });
  const sel = list.querySelector('li.p-item.sel');
  if (sel) {
    sel.scrollIntoView({ block: 'nearest' });
    input.setAttribute('aria-activedescendant', sel.id);
  }
}

function moveSel(delta) {
  if (!filtered.length) return;
  selIndex = (selIndex + delta + filtered.length) % filtered.length;
  renderList();
}

function runSel() {
  const it = filtered[selIndex];
  if (!it) return;
  closePalette();
  try { it.run(); } catch (err) { $('#statusBar').textContent = '命令执行失败: ' + (err && err.message || err); }
}

/* ---------- 开关 ---------- */
function openPalette() {
  if (!overlay) buildDom();
  paletteItems = collectItems();
  overlay.classList.remove('hidden');
  input.value = '';
  applyFilter('');
  setTimeout(() => input.focus(), 30);
}
function closePalette() {
  if (overlay) overlay.classList.add('hidden');
}

/* 全局快捷键:Ctrl+Shift+P 呼出/再按切回 */
document.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && (e.key === 'p' || e.key === 'P')) {
    e.preventDefault();
    e.stopPropagation();
    if (overlay && !overlay.classList.contains('hidden')) closePalette();
    else openPalette();
  }
});
