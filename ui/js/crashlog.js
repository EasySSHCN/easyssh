/* ============================================================
 * crashlog.js — v5(beta) 任务五:崩溃日志查看入口
 * 主进程(main.rs)在 panic 时写 cfgDir/crash-<毫秒>.log,仅保留最新 5 份。
 * 现有桥接命令只有 cfg_dir_path/read_text_file/save_text_file——没有列目录
 * 与删除命令;crash 文件名是毫秒时间戳,readTextFile 穷举探测不可行。
 * 因此本模块按「特性探测」接入:桥接补齐以下命令后即自动生效(前端零再改):
 *   crash_logs()             -> Array<{ name, size?, mtime? }>   仅 crash-*.log
 *   crash_log_read(name)     -> string                            文件内容
 *   crash_log_delete(name)   -> void                              删除单份
 * 缺命令时静默降级(不报错、不打扰),所需命令签名见仓库任务报告。
 * ============================================================ */
import { $ } from './dom.js';

const CRASH_RE = /^crash-\d+\.log$/;

function bridgeReady() {
  return !!(window.easyssh
    && typeof window.easyssh.crashLogs === 'function'
    && typeof window.easyssh.crashLogRead === 'function'
    && typeof window.easyssh.crashLogDelete === 'function');
}

/* 设置-关于分组注入崩溃日志卡片(动态构建,不改 index.html) */
function ensureAboutCard() {
  const panel = document.querySelector('#settingsModal .settings-panel[data-panel="about"]');
  if (!panel) return null;
  let card = panel.querySelector('#crashLogCard');
  if (card) return card;
  card = document.createElement('div');
  card.className = 'card';
  card.id = 'crashLogCard';
  card.innerHTML =
    '<h2>异常退出日志</h2>' +
    '<div class="tips" id="crashLogTip">程序异常崩溃时会在此目录留下 crash-*.log(仅保留最近 5 份),可用于反馈问题。</div>' +
    '<div id="crashLogList" style="display:flex;flex-direction:column;gap:4px;margin-top:6px;"></div>';
  panel.appendChild(card);
  return card;
}

function fmtSize(n) {
  const v = Number(n) || 0;
  return v >= 1024 ? (v / 1024).toFixed(1) + ' KB' : v + ' B';
}

export function renderCrashLogs(logs) {
  const card = ensureAboutCard();
  if (!card) return;
  const list = card.querySelector('#crashLogList');
  list.textContent = '';
  if (!logs || !logs.length) {
    const tips = document.createElement('div');
    tips.className = 'tips';
    tips.textContent = '暂无异常退出日志(一切正常)。';
    list.appendChild(tips);
    return;
  }
  for (const log of logs) {
    const name = String(log.name || '');
    if (!CRASH_RE.test(name)) continue;
    const row = document.createElement('div');
    row.className = 'row';
    row.style.cssText = 'align-items:center;gap:8px;flex-wrap:wrap;';
    const label = document.createElement('span');
    label.style.cssText = 'font-size:12px;font-family:monospace;';
    label.textContent = name + (log.size ? '(' + fmtSize(log.size) + ')' : '');
    const btnView = document.createElement('button');
    btnView.type = 'button';
    btnView.textContent = '查看';
    btnView.addEventListener('click', async () => {
      try {
        const raw = await window.easyssh.crashLogRead(name);
        const text = String(raw || '');
        // alert 承载长文本有限,截断到前 4000 字符
        alert(text.length > 4000 ? text.slice(0, 4000) + '\n…(内容过长已截断)' : text);
      } catch (e) { $('#statusBar').textContent = '读取崩溃日志失败: ' + (e && e.message || e); }
    });
    const btnDel = document.createElement('button');
    btnDel.type = 'button';
    btnDel.textContent = '删除';
    btnDel.addEventListener('click', async () => {
      try {
        await window.easyssh.crashLogDelete(name);
        renderCrashLogs((logs || []).filter((x) => x.name !== name));
        $('#statusBar').textContent = '已删除崩溃日志: ' + name;
      } catch (e) { $('#statusBar').textContent = '删除崩溃日志失败: ' + (e && e.message || e); }
    });
    row.appendChild(label);
    row.appendChild(btnView);
    row.appendChild(btnDel);
    list.appendChild(row);
  }
}

/* 启动检测:检测到 crash 日志 → 状态栏提示 + 预渲染关于页列表 */
export async function checkCrashLogs() {
  if (!bridgeReady()) return 0; // 桥接命令未提供:静默降级(报告见任务报告)
  let logs = null;
  try { logs = await window.easyssh.crashLogs(); } catch { return 0; }
  const valid = (logs || []).filter((x) => CRASH_RE.test(String(x.name || '')));
  if (valid.length) {
    $('#statusBar').textContent = '检测到 ' + valid.length + ' 份异常退出日志(可在设置-关于查看)';
  }
  renderCrashLogs(valid);
  return valid.length;
}
