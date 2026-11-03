/* ============================================================
 * updater.js — 在线升级 UI 流程（从 renderer.js 提取）
 * 职责：升级横幅、检测/下载进度、完成升级（原地替换重启）、
 *       %TEMP% 副本防护提示、设置中心「检查更新/下载」按钮。
 * ============================================================ */
import { $ } from './dom.js';

/* ================= v2.5 在线升级(加密检测/加密下载) ================= */
let updateEntry = null; // 有新版本时来自升级清单的安装包信息
let updateStage = 'idle'; // idle | downloading | downloaded

function fmtUpdateResult(r) {
  if (!r) return '';
  if (r.status === 'available') return `发现新版本 v${r.latestVersion}(当前 ${r.currentVersion})`;
  if (r.status === 'up-to-date') return `已是最新版本${r.latestVersion ? '(线上 v' + r.latestVersion + ')' : ''}`;
  if (r.status === 'busy') return r.message || '正在检测中…';
  return '检测失败: ' + (r.message || '未知错误');
}

function showUpdateBanner(r) {
  const bar = $('#updateBanner');
  if (!bar) return;
  const sb = $('#btnSettingsDownload');
  if (r && r.tempResident) {
    // v3.0.1 防护提示：本进程是 %TEMP% 解压副本，升级无法作用到真实安装目录
    updateEntry = r.status === 'available' ? r.file : updateEntry;
    $('#updateBannerText').textContent = '⚠️ 当前运行的是临时目录中的解压副本（上次在线升级遗留），本次升级无法替换你真正的安装目录。请关闭软件，把本 exe 移回安装目录（或从 easyssh.cn 重新下载放入）后再运行升级。';
    bar.style.display = 'flex';
    if (sb) sb.style.display = 'none';
    return;
  }
  if (r && r.status === 'available') {
    updateEntry = r.file;
    $('#updateBannerText').textContent =
      `🔔 发现新版本 v${r.latestVersion}(当前 v${(r.currentVersion || '').replace(/^v/, '')}),已通过加密接口校验`;
    bar.style.display = 'flex';
    // 设置中心同步显示「下载并校验」按钮
    if (sb) { sb.style.display = ''; sb.textContent = (updateStage === 'downloaded' ? '✅ 完成升级' : '⬇️ 下载并校验'); sb.disabled = false; }
  } else {
    bar.style.display = 'none';
    if (sb) { sb.style.display = 'none'; sb.textContent = '⬇️ 下载并校验'; sb.disabled = false; }
  }
}

// 自动检测结果 / 下载进度事件推送
// v4.3.5:支持禁用态(下载中不可点)
function setUpdateBtn(label, disabled) {
  const b = $('#btnUpdateDownload');
  if (b) { b.textContent = label; b.disabled = !!disabled; }
  const sb = $('#btnSettingsDownload');
  if (sb) { sb.textContent = label; sb.disabled = !!disabled; }
}

if (window.easyssh && window.easyssh.onUpdateEvent) {
  window.easyssh.onUpdateEvent((msg) => {
    if (msg.event === 'result') {
      showUpdateBanner(msg.payload);
      const el = $('#updateMsg');
      if (el) el.textContent = fmtUpdateResult(msg.payload); // 状态行只留简短结果
      // 更新内容放独立整段区域
      setUpdateNotes(msg.payload.status === 'available' && (msg.payload.notes || []).length
        ? '更新内容:\n' + msg.payload.notes.join('\n') : '');
      const sb = $('#statusBar');
      if (msg.payload.status === 'available') sb.textContent = fmtUpdateResult(msg.payload);
    } else if (msg.event === 'progress') {
      // v4.3.5:进度事件也标记下载中(渲染端就绪晚于下载启动时兜底)
      updateStage = 'downloading';
      // v4.3.8 反馈:百分比直接跟在「正在下载加密安装包…」之后
      const el = $('#updateMsg');
      if (el) el.textContent = '正在下载加密安装包… ' + msg.payload.percent + '%';
      const bar = $('#updateBannerText');
      if (bar) bar.textContent = '⬇️ 正在下载加密安装包… ' + msg.payload.percent + '%';
    } else if (msg.event === 'downloading') {
      // v4.3.5:下载开始事件——进入下载中阶段(按钮禁用,防止提前点「完成升级」)
      updateStage = 'downloading';
      $('#updateBannerText').textContent = '⬇️ 正在下载加密安装包…';
      setUpdateBtn('⬇️ 下载中…', true);
    } else if (msg.event === 'downloaded') {
      // v4.3.5 修复:空 payload 的旧复位事件直接忽略(防止误判为下载完成)
      if (!msg.payload || !msg.payload.file) return;
      // 下载校验完成：横幅进入「完成升级」阶段（原地换名替换，无需手动找文件，全程无脚本）
      updateStage = 'downloaded';
      $('#updateBannerText').textContent =
        '✅ 新版本已下载并通过加密校验，点击「完成升级」自动替换重启（旧版自动备份，可回滚）';
      setUpdateBtn('✅ 完成升级');
      $('#statusBar').textContent = '新版本已就绪: ' + msg.payload.file;
    } else if (msg.event === 'warning') {
      // v3.0.1：主进程发现升级位置异常（如进程驻留 %TEMP% 解压目录）时的主动警告
      $('#updateBannerText').textContent = '⚠️ ' + String(msg.payload.message || '').split('\n')[0];
      $('#updateBanner').style.display = 'flex';
      const el = $('#updateMsg');
      if (el) el.textContent = msg.payload.message || '';
      $('#statusBar').textContent = '⚠️ 升级位置异常：请把 exe 移回真实安装目录后再升级';
    } else if (msg.event === 'upgraded') {
      $('#statusBar').textContent = '🎉 已通过在线升级到 v' + msg.payload.version;
      // v4.3.7:升级后新文件按新版本动态命名,桌面快捷方式已自动指向新版
      alert('EasySSH 已通过在线升级到 v' + msg.payload.version + '。\n旧版本备份(.bak)已自动清理；会话与密钥配置原样保留。\n\n本程序文件已更新为新版本命名(EasySSH-' + msg.payload.version + '-portable.exe 或 EasySSH.exe)，桌面快捷方式已自动指向新版，以后固定双击即可。');
    }
  });
}

// 手工「检查更新」(设置中心)
// v4:更新说明放独立整段区域,不与状态行挤在一起
function setUpdateNotes(text) {
  const n = $('#updateNotes');
  if (!n) return;
  if (text) { n.textContent = text; n.style.display = 'block'; }
  else { n.textContent = ''; n.style.display = 'none'; }
}
$('#btnUpdateCheck').addEventListener('click', async () => {
  const el = $('#updateMsg');
  el.textContent = '正在通过加密接口检测更新…';
  setUpdateNotes('');
  try {
    const r = await window.easyssh.updateCheck();
    el.textContent = fmtUpdateResult(r); // 状态行只留简短结果
    if (r.status === 'available') {
      const notes = (r.notes || []);
      setUpdateNotes(notes.length ? '更新内容:\n' + notes.join('\n') : '');
      showUpdateBanner(r);
    } else {
      setUpdateNotes('');
    }
  } catch (e) {
    setUpdateNotes('');
    el.textContent = '检测失败: ' + (e.message || e);
  }
});

// 提醒条 / 设置中心共用：两阶段 —— 未下载时是「下载并校验」；下载完成后变为「✅ 完成升级」(原地换名替换重启)
async function runUpdateAction() {
  // v4.3.5:下载进行中的守卫——横幅被提前置为完成态时,防止在真实下载完成前调用替换
  if (updateStage === 'downloading') {
    $('#updateBannerText').textContent = '⬇️ 下载仍在进行中…(完成后会自动提示「完成升级」)';
    return;
  }
  if (updateStage === 'downloaded') {
    // 阶段二：完成升级 —— 主进程自改名替换并分离启动新版本，随后自动退出
    setUpdateBtn('✅ 完成升级');
    $('#btnUpdateDownload').disabled = true;
    const sb = $('#btnSettingsDownload');
    if (sb) sb.disabled = true;
    // 反馈必须显示在横幅上(设置弹窗可能关着,写 #updateMsg 会像"没反应")
    // v4.3.8:明确告知"替换 → 等待旧进程退出 → 自动启动新版"的顺序
    $('#updateBannerText').textContent = '🔄 正在替换文件并关闭当前版本…旧进程退出后将自动启动新版本（旧版保留 .bak 备份，可回滚）…';
    if (sb) { sb.style.display = ''; sb.textContent = '🔄 正在替换重启…'; }
    let res;
    try { res = await window.easyssh.updateComplete(); }
    catch (e) { res = { ok: false, message: (e && e.message) || String(e) }; }
    if (!res || !res.ok) {
      setUpdateBtn('✅ 完成升级');
      const msg = '❌ 完成升级失败: ' + ((res && res.message) || '未知错误') + '（可到 updates\\ 文件夹手动运行新版本 exe）';
      $('#updateBannerText').textContent = msg;
      $('#updateMsg').textContent = msg;
      if (sb) sb.textContent = '✅ 完成升级';
    }
    return; // 成功时主进程会在 1.2 秒内自动退出，无需更多操作
  }
  // 阶段一：下载并校验（全程加密）
  if (!updateEntry) return;
  $('#btnUpdateDownload').disabled = true;
  const sb2 = $('#btnSettingsDownload');
  if (sb2) sb2.disabled = true;
  // 进度/失败信息必须显示在横幅上(设置弹窗可能关着,写 #updateMsg 会像"没反应")
  $('#updateBannerText').textContent = '⬇️ 正在下载加密安装包…(完成后自动校验)';
  $('#updateMsg').textContent = '开始下载(全程加密,下载后自动解密校验)…';
  const res = await window.easyssh.updateDownload(updateEntry);
  if (!res.ok) {
    setUpdateBtn('⬇️ 下载并校验');
    $('#updateBannerText').textContent = '❌ 下载失败: ' + (res.message || '未知错误') + '(也可到 easyssh.cn/download.html 手动下载)';
    $('#updateMsg').textContent = '下载失败: ' + (res.message || '未知错误') + '\n可手动下载: https://easyssh.cn/download.html';
    return;
  }
  updateStage = 'downloaded';
  $('#updateMsg').textContent = '新版本已下载并校验通过: ' + res.file + '\n点击「完成升级」自动替换重启';
  $('#updateBannerText').textContent = '✅ 新版本已下载并通过加密校验，点击「完成升级」自动替换重启（旧版自动备份，可回滚）';
  setUpdateBtn('✅ 完成升级');
}
$('#btnUpdateDownload').addEventListener('click', runUpdateAction);
// 设置中心里的下载/升级按钮（与提醒条按钮同一逻辑）
$('#btnSettingsDownload').addEventListener('click', runUpdateAction);
$('#btnUpdateSkip').addEventListener('click', () => {
  $('#updateBanner').style.display = 'none';
  updateStage = 'idle';
  setUpdateBtn('⬇️ 下载并校验');
  const sb = $('#btnSettingsDownload');
  if (sb) { sb.style.display = ''; sb.textContent = '⬇️ 下载并校验'; }
});

// 启动时回放最近一次检测结果(自动检测可能先于渲染端就绪)
export async function initUpdate() {
  let last = null;
  try { last = await window.easyssh.updateLast(); showUpdateBanner(last); } catch {}
  // v5(M1)/F-M1-01:升级横幅展示签名公钥指纹(S-OTA-01 验收项)
  if (last && last.status === 'available' && window.easyssh.updatePubkeyFp) {
    try {
      const fp = await window.easyssh.updatePubkeyFp();
      const el = $('#updateMsg');
      if (el && fp) el.textContent += `\n签名公钥指纹: ${fp}`;
    } catch {}
  }
  // v5(M1)/F-M1-02:自动回滚提示(--rollback 启动,复用升级横幅机制)
  if (window.easyssh.rollbackNotice) {
    try {
      const rb = await window.easyssh.rollbackNotice();
      if (rb) {
        const bar = $('#updateBanner');
        if (bar) {
          $('#updateBannerText').textContent = '⚠ 已从 v' + rb + ' 自动回滚，新版启动异常已恢复';
          bar.style.display = 'flex';
        }
        $('#statusBar').textContent = '⚠ 已从 v' + rb + ' 自动回滚（新版启动异常已恢复）';
      }
    } catch {}
  }
}
