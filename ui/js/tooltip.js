/* ============================================================
 * tooltip.js — 功能按钮悬停说明 + 官网手册外链（v5.2.x/F-R10-21，R-20261120-003）
 * 对标 EasyRDP v0.4.0 同款实现：
 *   - 鼠标悬停/键盘聚焦目标功能按钮时弹出极简说明层
 *     （说明文本取按钮 title 原文，经全局 i18n 字典按当前语言即时取词）；
 *   - 说明尾部固定「详细手册 ↗」链接，点击调 Rust open_manual_url
 *     （系统默认浏览器打开，Rust 侧仅放行 easyssh.cn 域）；
 *   - 按钮 → 手册页 URL 在 MANUAL_LINKS 集中维护，与官网
 *     easyssh.cn/manual/<page>.html 一一对应，后续同步只改这一处。
 * 原生 title 冲突处理：绑定时把 title 文本迁入 data-tip 并清掉原生 title，
 * 避免「原生 title + 自定义 tooltip」双层弹出。
 * ============================================================ */

// ---- 功能按钮 → 官网手册页映射（集中维护，与官网 manual/ 目录一一对应）----
const MANUAL_BASE = 'https://easyssh.cn/manual/';
const MANUAL_LINKS = {
  btnNew: 'connections.html',      // 新建连接
  btnNewGroup: 'groups.html',      // 新建分组
  btnSessExport: 'export-json.html',     // 导出会话 JSON
  btnSessImport: 'import-json.html',     // 导入会话 JSON
  btnSessImportExt: 'import-ext.html',   // 外部会话导入(Xshell/PuTTY/Termius/OpenSSH)
  btnSftp: 'sftp.html',            // SFTP 文件传输
  btnQuick: 'quick-commands.html', // 快捷命令
  btnPf: 'port-forward.html',      // 端口转发
  btnSplit: 'split.html',          // 分屏布局
  btnBroadcast: 'broadcast.html',  // 批量命令广播
  btnMacro: 'macros.html',         // 宏序列自动化
  btnFind: 'find.html',            // 查找终端内容
  btnFeedback: 'feedback.html',    // 使用反馈
  btnSettings: 'settings.html',    // 设置
  btnGenKey: 'keygen.html',        // 一键生成本机密钥对
  btnImportPpk: 'ppk-import.html', // 导入 PuTTY .ppk
  gwAddBackup: 'gateway.html',     // 网关(可选):添加备用网关(R-20261120-005 ③)
  btnGwCheck: 'gateway.html',      // 网关(可选):检测网关连通性(R-20261120-005 ③)
  btnUpdateCheck: 'update.html',   // 检查更新/在线升级
};

import { appSettings } from './state.js';

let tipEl = null;
let hideTimer = null;

function tipT(zh) {
  const loc = (appSettings && appSettings.lang) || 'zh-CN';
  return (window.EasySSH_I18N && window.EasySSH_I18N.t) ? window.EasySSH_I18N.t(loc, zh) : zh;
}

function ensureTipEl() {
  if (tipEl) return tipEl;
  tipEl = document.createElement('div');
  tipEl.id = 'manual-tip';
  tipEl.className = 'manual-tip hidden';
  tipEl.setAttribute('role', 'tooltip');
  document.body.appendChild(tipEl);
  return tipEl;
}

function showTip(btn) {
  const zh = btn.dataset.tip;
  if (!zh) return;
  const page = MANUAL_LINKS[btn.id];
  const el = ensureTipEl();
  el.innerHTML = '';
  const text = document.createElement('span');
  text.className = 'manual-tip-text';
  text.textContent = tipT(zh);
  el.appendChild(text);
  if (page) {
    const link = document.createElement('a');
    link.className = 'manual-tip-link';
    link.href = '#';
    link.textContent = ' ' + tipT('详细手册 ↗');
    link.addEventListener('click', (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      hideTip();
      try {
        window.__TAURI__.core.invoke('open_manual_url', { url: MANUAL_BASE + page })
          .catch((e) => console.warn('打开手册失败:', e));
      } catch { /* __TAURI__ 不可用（浏览器直开 dev 页）：忽略 */ }
    });
    el.appendChild(link);
  }
  el.classList.remove('hidden');
  bindTipHover(el);
  // 定位：优先按钮下方左对齐；贴底时改上方；横向贴右时右对齐，且不出视口
  const r = btn.getBoundingClientRect();
  const tw = el.offsetWidth, th = el.offsetHeight;
  let x = r.left;
  let y = r.bottom + 8;
  if (y + th > window.innerHeight - 8) y = r.top - th - 8;
  if (x + tw > window.innerWidth - 8) x = window.innerWidth - tw - 8;
  if (x < 8) x = 8;
  el.style.left = Math.round(x) + 'px';
  el.style.top = Math.round(Math.max(8, y)) + 'px';
}

function hideTip() {
  clearTimeout(hideTimer);
  if (tipEl) tipEl.classList.add('hidden');
}

// 说明层自身 hover 处理：进入取消隐藏计时，移出延迟隐藏——
// 按钮 → 说明层之间的 8px 间隙不会导致层提前消失，层内链接可正常点击。
function bindTipHover(el) {
  if (el.dataset.hoverBound === '1') return;
  el.dataset.hoverBound = '1';
  el.addEventListener('mouseenter', () => clearTimeout(hideTimer));
  el.addEventListener('mouseleave', () => {
    clearTimeout(hideTimer);
    hideTimer = setTimeout(hideTip, 300);
  });
}

/** 初始化：为全部已登记功能按钮挂接 tooltip（幂等，可重复调用）。 */
export function initManualTooltips() {
  for (const id of Object.keys(MANUAL_LINKS)) {
    const btn = document.getElementById(id);
    if (!btn || btn.dataset.tipBound === '1') continue;
    // 原生 title 迁移：title 文本 → data-tip（展示时再按当前语言取词），清掉原生 title
    const title = btn.getAttribute('title') || btn.getAttribute('aria-label') || '';
    if (title.trim()) btn.dataset.tip = title.trim();
    btn.removeAttribute('title');
    btn.dataset.tipBound = '1';
    btn.addEventListener('mouseenter', () => {
      clearTimeout(hideTimer);
      showTip(btn);
    });
    btn.addEventListener('mouseleave', () => {
      clearTimeout(hideTimer);
      hideTimer = setTimeout(hideTip, 300); // 留时间让鼠标跨过间隙进入说明层
    });
    btn.addEventListener('focus', () => showTip(btn));
    btn.addEventListener('blur', () => { clearTimeout(hideTimer); hideTip(); });
    btn.addEventListener('mousedown', hideTip); // 点击/拖拽时不遮挡
  }
}
