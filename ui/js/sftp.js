/* ============================================================
 * sftp.js — SFTP 面板与传输（从 renderer.js 提取）
 * 职责：远端目录浏览、多选、右键菜单、属性/目录大小、
 *       上传下载（传输队列）、传输进度覆盖层、拖拽上传、
 *       传输队列弹窗与进度事件。
 * ============================================================ */
import { $, escH, fmtSize, fmtSpeed } from './dom.js';
import { tabs, activeId, appSettings, sftpTextMode } from './state.js';
import { hideCtxMenu } from './sessions.js';

/* ---------- SFTP 面板 ---------- */
let sftpSel = null; // 选中的远程文件 { name, path, type, size }
// v4.3.2 多选:sftpSelSet(path->item) 保存勾选集合;sftpSelAnchor 记录 Shift 范围起点
let sftpSelSet = new Map();
let sftpSelAnchor = null;
let sftpViewItems = []; // 当前目录条目(供 Shift 范围选择)
let sftpViewDir = '/';
function sftpAbs(dir, name) { return (dir === '/' ? '' : dir) + '/' + name; }
function sftpClearSel() { sftpSelSet.clear(); sftpSelAnchor = null; }
async function sftpBrowse(dir) {
  const t = tabs.get(activeId);
  if (!t) { $('#sftpMsg').textContent = '请先连接一个会话'; return; }
  if (t.disconnected) { $('#sftpMsg').textContent = '当前连接已断开,按回车重连后再用 SFTP'; return; }
  $('#sftpMsg').textContent = '';
  $('#sftpList').innerHTML = '<div class="frow"><span class="fname">加载中...</span></div>';
  try {
    const items = await window.easyssh.sftpList(t.id, dir);
    $('#sftpPath').value = dir;
    $('#sftpTitle').textContent = t.name;
    sftpSel = null;
    sftpClearSel(); // v4.3.2:刷新后勾选取消(需求明确)
    sftpViewItems = items;
    sftpViewDir = dir;
    const list = $('#sftpList');
    list.innerHTML = '';
    if (dir !== '/') {
      const up = document.createElement('div');
      up.className = 'frow';
      up.innerHTML = '<span class="fname"><svg class="icon"><use href="#i-chevron-up"/></svg>..</span>';
      up.addEventListener('click', () => sftpBrowse(dir.split('/').slice(0, -1).join('/') || '/'));
      list.appendChild(up);
    }
    for (const it of items) {
      const row = document.createElement('div');
      row.className = 'frow';
      const path = sftpAbs(dir, it.name);
      row.innerHTML = '<span class="fsel"><input type="checkbox" data-sel="' + escH(it.name) + '"></span>'
        + '<span class="fname"><svg class="icon"><use href="#' + (it.type === 'dir' ? 'i-folder' : 'i-file') + '"/></svg>' + escH(it.name) + '</span>'
        + '<span class="fsize">' + (it.type === 'dir' ? '-' : fmtSize(it.size)) + '</span>';
      const checkbox = row.querySelector('input[type=checkbox]');
      const setSel = (on) => {
        if (on) { sftpSelSet.set(path, { ...it, path }); row.classList.add('sel'); checkbox.checked = true; }
        else { sftpSelSet.delete(path); row.classList.remove('sel'); checkbox.checked = false; }
      };
      // v4.3.2 反馈:单击行/勾选框即切换勾选(增=勾上,再点=取消),无需按 Ctrl
      const toggleSel = () => {
        const on = !sftpSelSet.has(path);
        setSel(on);
        sftpSel = { ...it, path };
        if (on) sftpSelAnchor = items.indexOf(it);
      };
      checkbox.addEventListener('click', (e) => {
        e.stopPropagation();
        toggleSel();
      });
      row.addEventListener('click', (e) => {
        if (e.target === checkbox) return;
        if (e.shiftKey && sftpSelAnchor !== null) {
          sftpClearSel();
          list.querySelectorAll('.fsel input').forEach((c) => { c.checked = false; });
          const a = Math.min(sftpSelAnchor, items.indexOf(it));
          const b = Math.max(sftpSelAnchor, items.indexOf(it));
          for (let i = a; i <= b; i++) {
            const it2 = items[i];
            sftpSelSet.set(sftpAbs(dir, it2.name), { ...it2, path: sftpAbs(dir, it2.name) });
            const r2 = list.querySelectorAll('.frow')[i + (dir !== '/' ? 1 : 0)];
            if (r2) { r2.classList.add('sel'); const c = r2.querySelector('.fsel input'); if (c) c.checked = true; }
          }
        } else {
          // 反馈 2:单击即切换勾选(增加/移除),Ctrl 同效
          toggleSel();
        }
        if (sftpSelSet.size > 0) sftpSel = { ...it, path };
      });
      row.addEventListener('dblclick', () => { if (it.type === 'dir') sftpBrowse(sftpAbs(dir, it.name)); });
      // v2.10.0 远端行右键菜单;v4.3.2:勾选集中多选时菜单切换为批量模式(下载/删除/属性/刷新)
      row.addEventListener('contextmenu', (e) => {
        e.preventDefault();
        e.stopPropagation();
        if (!sftpSelSet.has(path) || sftpSelSet.size <= 1) {
          list.querySelectorAll('.frow').forEach((r) => r.classList.remove('sel'));
          row.classList.add('sel');
          sftpSel = { ...it, path };
          sftpCtxMulti = null;
        } else {
          sftpCtxMulti = [...sftpSelSet.values()];
          sftpSel = { ...it, path };
        }
        openSftpCtx(e.clientX, e.clientY);
      });
      list.appendChild(row);
    }
  } catch (e) {
    $('#sftpList').innerHTML = '';
    $('#sftpMsg').textContent = 'SFTP 错误: ' + (e.message || e);
  }
}
$('#btnSftp').addEventListener('click', () => {
  const t = tabs.get(activeId);
  if (!t) { $('#statusBar').textContent = '请先连接一个会话再打开 SFTP'; return; }
  $('#sftpModal').classList.remove('hidden');
  sftpBrowse($('#sftpPath').value || '/root');
});
$('#sftpGo').addEventListener('click', () => sftpBrowse($('#sftpPath').value.trim() || '/'));
$('#sftpPath').addEventListener('keydown', (e) => { if (e.key === 'Enter') sftpBrowse($('#sftpPath').value.trim() || '/'); });
$('#sftpUp').addEventListener('click', () => {
  const cur = $('#sftpPath').value.trim() || '/';
  sftpBrowse(cur.split('/').slice(0, -1).join('/') || '/');
});
// v2.10.0:上传改走传输队列(xferStart),不再阻塞等待单个文件传完
// v4.0.10:传输进度覆盖层（进行中常驻显示,完成后才移除）
const xferOverlayItems = new Map(); // taskId -> { name }
function xferOverlayShow() {
  const o = $('#xferOverlay');
  if (o) o.style.display = 'flex';
}
function xferOverlayMaybeHide() {
  const o = $('#xferOverlay');
  if (o && xferOverlayItems.size === 0) o.style.display = 'none';
}
function xferOverlayAdd(id, name) {
  xferOverlayItems.set(id, name);
  const list = $('#xferOverlayList');
  if (!list) return;
  xferOverlayShow();
  const item = document.createElement('div');
  item.className = 'xitem';
  item.id = 'xo-' + id;
  item.innerHTML = '<div class="xname"></div><div class="xbar"><div style="width:0%"></div></div><div class="xpct">排队中…</div>';
  list.appendChild(item);
  updateXferItemText(id, name);
}
function updateXferItemText(id, name) {
  const item = document.getElementById('xo-' + id);
  if (item && name) item.querySelector('.xname').textContent = name;
}
function xferOverlayUpdate(id, received, total) {
  const item = document.getElementById('xo-' + id);
  if (!item) return;
  const pct = total > 0 ? Math.min(100, Math.round((received / total) * 100)) : 0;
  item.querySelector('.xbar > div').style.width = pct + '%';
  item.querySelector('.xpct').textContent = pct + '%（' + fmtSize(received) + ' / ' + fmtSize(total) + '）';
}
function xferOverlayFinish(id, status, errText) {
  const item = document.getElementById('xo-' + id);
  if (item) {
    if (status === 'done') {
      item.querySelector('.xbar > div').style.width = '100%';
      item.querySelector('.xpct').innerHTML = '<svg class="icon"><use href="#i-check"/></svg>完成';
    } else {
      const e = item.querySelector('.xpct');
      if (e) e.innerHTML = '<span class="xerr">✕ ' + (status === 'cancelled' ? '已取消' : '失败') + (errText ? ': ' + errText : '') + '</span>';
    }
  }
  const name = xferOverlayItems.get(id);
  xferOverlayItems.delete(id);
  setTimeout(() => {
    const el = document.getElementById('xo-' + id);
    if (el) el.remove();
    xferOverlayMaybeHide();
    if (name) xferOverlayItems;
  }, status === 'done' ? 1200 : 2600);
}
$('#sftpUpload').addEventListener('click', async () => {
  const t = tabs.get(activeId);
  if (!t || t.disconnected) return;
  const files = await window.easyssh.pickOpen();
  if (!files.length) return;
  const dir = $('#sftpPath').value.trim() || '/root';
  let queued = 0;
  for (const f of files) {
    const remote = sftpAbs(dir, f.split('\\').pop().split('/').pop());
    try {
      const res = await startUploadTask(t.id, f, remote, false);
      if (res && res.ok && res.id !== undefined) {
        xferOverlayAdd(res.id, f.split('\\').pop().split('/').pop());
        queued++;
      } else {
        $('#sftpMsg').textContent = '上传失败: ' + ((res && res.message) || '未知错误');
      }
    } catch (err) {
      $('#sftpMsg').textContent = '上传失败: ' + (err.message || err);
    }
  }
  if (queued) {
    $('#sftpMsg').textContent = '';
    $('#statusBar').textContent = '已加入上传队列 ' + queued + ' 个任务,传输进度见进度条';
    sftpBrowse(dir);
  }
});
// v2.10.0:双击文件 = 下载(保留「选择保存位置」交互,但改走传输队列;目录双击进入,目录下载走右键菜单)
document.addEventListener('dblclick', async (e) => {
  const row = e.target.closest('#sftpList .frow');
  if (!row || !sftpSel || sftpSel.type !== 'file') return;
  const t = tabs.get(activeId);
  if (!t || t.disconnected) return;
  const local = await window.easyssh.pickSave(sftpSel.name);
  if (!local) return;
  const res = await window.easyssh.xferStart(t.id, {
    direction: 'download', localPath: local, remotePath: sftpSel.path, ...xferOpts(),
  });
  if (!res.ok) { $('#sftpMsg').textContent = '下载失败: ' + (res.message || '未知错误'); return; }
  xferOverlayAdd(res.id, sftpSel.name);
  $('#sftpMsg').textContent = '';
  $('#statusBar').textContent = '已加入下载队列: ' + sftpSel.name;
});

/* ================= v2.10.0 SFTP 强化:右键菜单 / 传输队列 / 拖拽 ================= */

/* ---------- 传输配置(文本模式/编码沿用面板设置) ---------- */
function xferOpts() {
  const o = { textMode: sftpTextMode() };
  if (o.textMode && appSettings && appSettings.encoding) o.encoding = appSettings.encoding;
  return o;
}
// 发起上传任务(返回 xfer:start 的结果,便于统计入队数量)
async function startUploadTask(sessionId, localPath, remotePath, recursive) {
  return window.easyssh.xferStart(sessionId, {
    direction: 'upload', localPath, remotePath, recursive: !!recursive, ...xferOpts(),
  });
}

/* ---------- SFTP 远端文件右键菜单 ---------- */
let sftpCtxTarget = null; // 右键菜单当前指向的远端条目 { name, path, type }
let sftpCtxMulti = null;  // v4.3.2 多选模式:非 null 时为选中条目数组(菜单仅 下载/删除/属性/刷新)
function hideSftpCtx() { $('#sftpCtx').classList.add('hidden'); }
function openSftpCtx(x, y) {
  sftpCtxTarget = sftpSel;
  hideCtxMenu();
  const m = $('#sftpCtx');
  // v4.3.2 多选模式:只显示 下载/属性/刷新/删除,隐藏 重命名/新建目录/计算大小
  // v4.3.4 修复:单选文件也要隐藏「计算大小」(仅目录适用)
  const multi = !!sftpCtxMulti && sftpCtxMulti.length > 1;
  m.querySelectorAll('.ctx-item').forEach((el) => {
    const act = el.dataset.act;
    if (multi && (act === 'rename' || act === 'mkdir' || act === 'du')) el.style.display = 'none';
    else if (!multi && act === 'du' && sftpCtxTarget && sftpCtxTarget.type !== 'dir') el.style.display = 'none';
    else el.style.display = '';
  });
  m.classList.remove('hidden'); // 先显示才能量取尺寸,用于贴边防溢出
  const r = m.getBoundingClientRect();
  m.style.left = Math.max(4, Math.min(x, window.innerWidth - r.width - 8)) + 'px';
  m.style.top = Math.max(4, Math.min(y, window.innerHeight - r.height - 8)) + 'px';
}
document.addEventListener('click', (e) => { if (!e.target.closest('#sftpCtx')) hideSftpCtx(); });
document.addEventListener('contextmenu', (e) => { if (!e.target.closest('#sftpList .frow')) hideSftpCtx(); });

// v2.10 远端操作改走 SFTP 通道(sftp:rename / sftp:remove / sftp:mkdir),不再写终端 shell 命令
async function sftpRemoteOp(id, fn, label) {
  const res = await fn(id);
  if (!res.ok) $('#sftpMsg').textContent = label + '失败: ' + (res.message || '未知错误');
  return res.ok;
}
$('#sftpCtx').addEventListener('click', async (e) => {
  const item = e.target.closest('.ctx-item');
  hideSftpCtx();
  if (!item || !sftpCtxTarget) return;
  const t = tabs.get(activeId);
  if (!t || t.disconnected) { $('#statusBar').textContent = '当前连接已断开,无法执行远端操作'; return; }
  const act = item.dataset.act;
  const cur = $('#sftpPath').value.trim() || '/';
  // v4.3.2 多选:勾选集中 >1 个条目时,本轮操作对象为全部勾选项
  const multi = sftpCtxMulti && sftpCtxMulti.length > 1 ? sftpCtxMulti : null;
  const targets = multi || [sftpCtxTarget];
  if (act === 'refresh') { sftpBrowse(cur); sftpClearSel(); return; } // v4.3.2:刷新后勾选取消
  if (act === 'download') {
    if (multi) {
      // v4.3.2 批量下载:统一落到「选择的目录」(沿用 ensureDlDir 记忆),目录递归/文件直下
      const base = await ensureDlDir();
      if (!base) { $('#statusBar').textContent = '已取消批量下载'; return; }
      let queued = 0, failed = 0;
      for (const it of targets) {
        const res = await window.easyssh.xferStart(t.id, {
          direction: 'download',
          localPath: joinLocal(base, it.name),
          remotePath: it.path,
          recursive: it.type === 'dir',
          ...xferOpts(),
        });
        if (res.ok) { queued++; xferOverlayAdd(res.id, it.name); }
        else failed++;
      }
      if (queued) {
        $('#statusBar').textContent = '已加入下载队列 ' + queued + ' 个任务' + (failed ? '(失败 ' + failed + ')' : '') + ',落盘到 ' + base;
        $('#sftpMsg').textContent = failed ? '失败 ' + failed + ' 个' : '';
      } else $('#sftpMsg').textContent = '批量下载失败: 未成功入队任何任务';
      return;
    }
    if (sftpCtxTarget.type === 'dir') {
      // 目录下载:先确定本地落盘目录(exe 目录渲染端拿不到,用一次「选择保存位置」取目录并记住,之后不再询问)
      const base = await ensureDlDir();
      if (!base) { $('#statusBar').textContent = '已取消目录下载'; return; }
      const localDir = joinLocal(base, sftpCtxTarget.name);
      const res = await window.easyssh.xferStart(t.id, {
        direction: 'download', localPath: localDir, remotePath: sftpCtxTarget.path, recursive: true, ...xferOpts(),
      });
      if (!res.ok) { $('#sftpMsg').textContent = '下载失败: ' + (res.message || '未知错误'); return; }
      $('#statusBar').textContent = '目录「' + sftpCtxTarget.name + '」已加入下载队列,落盘到 ' + localDir;
    } else {
      // 单文件下载:保留原「选择保存位置」交互,但改走传输队列
      const local = await window.easyssh.pickSave(sftpCtxTarget.name);
      if (!local) return;
      const res = await window.easyssh.xferStart(t.id, {
        direction: 'download', localPath: local, remotePath: sftpCtxTarget.path, ...xferOpts(),
      });
      if (!res.ok) { $('#sftpMsg').textContent = '下载失败: ' + (res.message || '未知错误'); return; }
      $('#statusBar').textContent = '已加入下载队列: ' + sftpCtxTarget.name;
    }
    return;
  }
  if (act === 'props') {
    // v4.3.2 属性:参照 Windows 资源管理器——单选显示完整属性,多选显示聚合统计
    if (!multi) {
      const md = await window.easyssh.sftpMetadata(t.id, sftpCtxTarget.path).catch(() => null);
      const isDir = (md && md.isDir) || sftpCtxTarget.type === 'dir';
      const sizeTxt = isDir ? '—' : fmtSize((md && md.size) || sftpCtxTarget.size || 0);
      const mtime = md && md.mtime ? new Date(md.mtime * 1000).toLocaleString() : '-';
      const ext = isDir ? '' : (sftpCtxTarget.name.includes('.') ? sftpCtxTarget.name.split('.').pop().toUpperCase() + ' 文件' : '文件');
      $('#sftpPropsTitle').textContent = sftpCtxTarget.name + ' 属性';
      $('#sftpPropsBody').innerHTML =
        '<div><b>类型:</b> ' + (isDir ? '文件夹' : (ext || '文件')) + '</div>' +
        '<div><b>位置:</b> ' + escH(sftpViewDir) + '</div>' +
        '<div><b>大小:</b> <span id="sftpPropsSize">' + sizeTxt + '</span></div>' +
        '<div><b>修改时间:</b> ' + mtime + '</div>' +
        '<div><b>完整路径:</b> ' + escH(sftpCtxTarget.path) + '</div>';
      $('#sftpPropsDuRow').style.display = isDir ? 'flex' : 'none';
      $('#sftpPropsDuResult').textContent = '';
      sftpPropsDuTargets = isDir ? [sftpCtxTarget.path] : [];
      sftpPropsKnownSize = isDir ? 0 : ((md && md.size) || sftpCtxTarget.size || 0);
      $('#sftpPropsModal').classList.remove('hidden');
    } else {
      // 多选:聚合统计(文件/文件夹数量、已知合计大小、共同位置)
      let files = 0, dirs = 0, size = 0; const dirPaths = [];
      for (const it of targets) {
        if (it.type === 'dir') { dirs++; dirPaths.push(it.path); }
        else { files++; size += it.size || 0; }
      }
      $('#sftpPropsTitle').textContent = '属性 — 已选 ' + targets.length + ' 项';
      $('#sftpPropsBody').innerHTML =
        '<div><b>类型:</b> ' + files + ' 个文件, ' + dirs + ' 个文件夹</div>' +
        '<div><b>位置:</b> ' + escH(sftpViewDir) + '</div>' +
        '<div><b>大小:</b> <span id="sftpPropsSize">' + fmtSize(size) + (dirs ? '(不含文件夹内容)' : '') + '</span></div>';
      $('#sftpPropsDuRow').style.display = dirs ? 'flex' : 'none';
      $('#sftpPropsDuResult').textContent = dirs ? '点击下方按钮递归统计所选文件夹占用' : '';
      sftpPropsDuTargets = dirPaths;
      sftpPropsKnownSize = size;
      $('#sftpPropsModal').classList.remove('hidden');
    }
    return;
  }
  if (act === 'du') {
    // v4.3.2 新增:计算目录大小(递归;后端有深度/条目上限防失控)
    if (sftpCtxTarget.type !== 'dir') { $('#statusBar').textContent = '计算大小仅适用于目录(文件大小已显示)'; return; }
    $('#statusBar').textContent = '正在计算目录大小: ' + sftpCtxTarget.path + ' (大目录可能较慢)…';
    const res = await window.easyssh.sftpDirSize(t.id, sftpCtxTarget.path);
    if (!res.ok) { $('#statusBar').textContent = '计算失败: ' + (res.message || '未知错误'); return; }
    const note = res.truncated ? '(条目过多,结果为部分统计)' : '';
    const skip = res.skipped ? ',跳过无权限 ' + res.skipped + ' 项' : '';
    $('#statusBar').textContent = '目录「' + sftpCtxTarget.name + '」大小: ' + fmtSize(res.size)
      + ' — 文件 ' + res.files + ' 个,子目录 ' + Math.max(0, res.dirs - 1) + ' 个' + skip + note;
    // 结果回填到列表行的大小列,便于直接查看
    document.querySelectorAll('#sftpList .frow').forEach((r) => {
      const n = r.querySelector('.fname');
      if (n && n.textContent.endsWith(sftpCtxTarget.name)) r.querySelector('.fsize').textContent = fmtSize(res.size) + (res.truncated ? '+' : '');
    });
    return;
  }
  if (act === 'rename') {
    const nn = window.easysshUi ? await window.easysshUi.prompt('重命名为:', sftpCtxTarget.name) : null;
    if (!nn || nn === sftpCtxTarget.name) return;
    const ok = await sftpRemoteOp(t.id, (id) => window.easyssh.sftpRename(id, sftpCtxTarget.path, sftpAbs(cur, nn)), '重命名');
    if (ok) { $('#statusBar').textContent = '重命名完成'; sftpBrowse(cur); }
    return;
  }
  if (act === 'mkdir') {
    const nn = window.easysshUi ? await window.easysshUi.prompt('新建目录名称:', '') : null;
    if (!nn) return;
    const ok = await sftpRemoteOp(t.id, (id) => window.easyssh.sftpMkdir(id, sftpAbs(cur, nn)), '新建目录');
    if (ok) { $('#statusBar').textContent = '目录已创建'; sftpBrowse(cur); }
    return;
  }
  if (act === 'delete') {
    // v4.3.2 多选删除:逐项递归删除后统一刷新
    if (multi) {
      let okDel = false;
      if (window.easysshUi) okDel = await window.easysshUi.confirm('确认删除选中的 ' + targets.length + ' 项?\n(目录将连同其全部内容一起删除)');
      if (!okDel) return;
      let okCnt = 0, failCnt = 0;
      for (const it of targets) {
        const ok = await sftpRemoteOp(t.id, (id) => window.easyssh.sftpRemove(id, it.path), '删除');
        if (ok) okCnt++; else failCnt++;
      }
      $('#statusBar').textContent = '批量删除完成: 成功 ' + okCnt + ' 个' + (failCnt ? ',失败 ' + failCnt + ' 个' : '');
      if (okCnt) { sftpClearSel(); sftpBrowse(cur); }
      return;
    }
    let okDel = false;
    try {
      if (window.__TAURI__ && window.__TAURI__.dialog) {
        okDel = await window.__TAURI__.dialog.ask('确认删除' + (sftpCtxTarget.type === 'dir' ? '目录(含全部内容)' : '文件') + ': ' + sftpCtxTarget.path + ' ?', { title: '确认删除', kind: 'warning' });
      }
    } catch {}
    if (!okDel && window.easysshUi) okDel = await window.easysshUi.confirm('确认删除: ' + sftpCtxTarget.path + ' ?');
    if (!okDel) return;
    const ok = await sftpRemoteOp(t.id, (id) => window.easyssh.sftpRemove(id, sftpCtxTarget.path), '删除');
    if (ok) { $('#statusBar').textContent = '已删除'; sftpBrowse(cur); }
  }
});

// 目录下载落盘目录:渲染端拿不到 exe 同目录,首次用「选择保存位置」对话框取一个目录并记住(localStorage)
// v4.3.2:属性弹窗内的「计算目录大小」按钮——递归统计所选文件夹(单个或多个)占用并计入属性"大小"
let sftpPropsDuTargets = []; // 属性弹窗当前需要统计的目录路径
let sftpPropsKnownSize = 0;  // 已知文件合计大小
$('#sftpPropsDu').addEventListener('click', async () => {
  const t = tabs.get(activeId);
  if (!t || t.disconnected || !sftpPropsDuTargets.length) return;
  const el = $('#sftpPropsDuResult');
  el.textContent = '计算中…(大目录可能较慢,共 ' + sftpPropsDuTargets.length + ' 个文件夹)';
  let total = sftpPropsKnownSize, files = 0, dirs = 0, skipped = 0, truncated = false, done = 0;
  for (const p of sftpPropsDuTargets) {
    const res = await window.easyssh.sftpDirSize(t.id, p);
    if (res.ok) {
      total += res.size; files += res.files; dirs += Math.max(0, res.dirs - 1);
      skipped += res.skipped || 0;
      if (res.truncated) truncated = true;
    }
    done++;
    el.textContent = '计算中…(' + done + '/' + sftpPropsDuTargets.length + ')';
  }
  const note = truncated ? '(部分统计)' : '';
  const skip = skipped ? ',跳过无权限 ' + skipped + ' 项' : '';
  el.textContent = '合计: ' + fmtSize(total) + ' — 文件 ' + files + ' 个,文件夹 ' + dirs + ' 个' + skip + note;
  const sizeEl = $('#sftpPropsSize');
  if (sizeEl) sizeEl.textContent = fmtSize(total) + (truncated ? '+' : '');
  // 结果回填列表:被统计的目录行大小列
  document.querySelectorAll('#sftpList .frow').forEach((r) => {
    const n = r.querySelector('.fname');
    if (!n) return;
    const hit = sftpPropsDuTargets.find((p) => n.textContent.endsWith(p.split('/').pop()));
    if (hit && sftpPropsDuTargets.length === 1) {
      const s = r.querySelector('.fsize');
      if (s) s.textContent = fmtSize(total) + (truncated ? '+' : '');
    }
  });
});

async function ensureDlDir() {
  const saved = localStorage.getItem('easyssh.dlDir');
  if (saved) return saved;
  const picked = await window.easyssh.pickSave('选择任意位置以确定目录(此文件名会被忽略)');
  if (!picked) return null;
  const dir = picked.replace(/[\\/][^\\/]+$/, '');
  if (!dir) return null;
  localStorage.setItem('easyssh.dlDir', dir);
  return dir;
}
// 本地路径拼接(沿用 picked 路径中的分隔符风格)
function joinLocal(base, name) {
  const sep = base.includes('\\') ? '\\' : '/';
  return base.replace(/[\\/]+$/, '') + sep + name;
}

/* ---------- 拖拽上传 ---------- */
// Electron 32+ 渲染端拿不到 File.path,优先经 preload 透传的 webUtils.getPathForFile 还原真实
// 本地路径;拿不到 path(异常环境)才提示改用「上传文件…」按钮。
(function initSftpDnd() {
  const list = $('#sftpList');
  list.addEventListener('dragover', (e) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
    $('#sftpDropHint').classList.remove('hidden');
  });
  list.addEventListener('dragleave', () => $('#sftpDropHint').classList.add('hidden'));
  list.addEventListener('drop', async (e) => {
    e.preventDefault();
    $('#sftpDropHint').classList.add('hidden');
    const t = tabs.get(activeId);
    if (!t || t.disconnected) { $('#statusBar').textContent = '当前连接已断开,无法上传'; return; }
    const files = [...(e.dataTransfer.files || [])];
    if (!files.length) return;
    const dir = $('#sftpPath').value.trim() || '/root';
    let queued = 0, noPath = 0;
    for (const f of files) {
      let p = f.path || '';
      if (!p) { try { p = window.easyssh.getPathForFile(f) || ''; } catch {} }
      if (!p) { noPath++; continue; }
      const res = await startUploadTask(t.id, p, sftpAbs(dir, f.name), f.type === '' || /\/$/.test(p));
      if (res.ok) queued++;
    }
    if (noPath) $('#sftpMsg').textContent = '提示:无法获取拖入文件的本地路径,请用「上传文件…」按钮选择后上传。';
    if (queued) { $('#statusBar').textContent = '已加入上传队列 ' + queued + ' 个任务'; sftpBrowse(dir); }
  });
})();

/* ---------- 传输队列弹窗 ---------- */
const xferTasks = new Map(); // taskId -> task(进度事件实时更新)
const xferCleared = new Set(); // 「清空已完成」只从视图移除,主进程历史不动
const XFER_PHASE_TXT = { queued: '排队', running: '传输中', resuming: '续传中', done: '完成', failed: '失败', cancelled: '已取消' };
function xferActiveCount() {
  let n = 0;
  for (const t of xferTasks.values()) if (t.phase === 'queued' || t.phase === 'running') n++;
  return n;
}
function renderXferBadge() { $('#xferCount').textContent = xferActiveCount(); }

function renderXferList() {
  const ul = $('#xferList');
  const all = [...xferTasks.values()].filter((t) => !xferCleared.has(t.taskId));
  ul.innerHTML = '';
  if (!all.length) {
    const d = document.createElement('div');
    d.className = 'xempty';
    d.textContent = '(暂无传输任务;在 SFTP 面板右键文件下载,或用「上传文件…」发起传输)';
    ul.appendChild(d);
    return;
  }
  for (const t of all) {
    const pct = t.bytesTotal > 0 ? Math.min(100, Math.round(t.bytesDone / t.bytesTotal * 100)) : (t.phase === 'done' ? 100 : 0);
    const row = document.createElement('div');
    row.className = 'xrow phase-' + t.phase;
    row.innerHTML = '<span class="xdir">' + (t.direction === 'upload' ? '<svg class="icon"><use href="#i-upload"/></svg>' : '<svg class="icon"><use href="#i-download"/></svg>') + '</span>'
      + '<div class="xmain">'
      + '<div class="xtitle"><b>' + (t.direction === 'upload' ? '上传' : '下载') + '</b> '
      + escH(t.direction === 'upload' ? t.localPath : t.remotePath)
      + (t.recursive ? ' <span class="tips">(目录递归)</span>' : '')
      + (t.textMode ? ' <span class="tips">(文本' + escH(t.encoding || '') + ')</span>' : '')
      + '</div>'
      + '<div class="xbar"><div class="xbar-in" style="width:' + pct + '%"></div></div>'
      + '<div class="xmeta">' + fmtSize(t.bytesDone) + ' / ' + fmtSize(t.bytesTotal)
      + (t.speedBps > 0 ? ' · ' + fmtSpeed(t.speedBps) : '')
      + (t.filesTotal > 0 ? ' · 文件 ' + t.filesDone + '/' + t.filesTotal : '')
      + (t.currentFile ? ' · ' + escH(t.currentFile) : '')
      + (t.phase === 'failed' && t.error ? '<br><span class="xerr">失败原因: ' + escH(t.error) + '</span>' : '')
      + '</div></div>'
      + '<span class="xbadge b-' + t.phase + '">' + (XFER_PHASE_TXT[t.phase] || t.phase) + '</span>'
      // v5(M2)/F-M2-02：有断点的失败/取消任务显示「▶ 续传」
      + (((t.phase === 'queued' || t.phase === 'running')) ? '<span class="xcancel" title="取消此任务" aria-label="取消此任务"><svg class="icon"><use href="#i-x"/></svg></span>' : '')
      + ((t.phase === 'failed' || t.phase === 'cancelled') && t.bytesDone > 0 && t.bytesDone < t.bytesTotal
        ? '<span class="xresume" title="从断点续传" aria-label="从断点续传" data-tid="' + t.taskId + '"><svg class="icon"><use href="#i-play"/></svg></span>' : '');
    const c = row.querySelector('.xcancel');
    if (c) c.addEventListener('click', async () => {
      const res = await window.easyssh.xferCancel(t.taskId);
      if (!res.ok) $('#statusBar').textContent = '取消失败: ' + (res.message || '未知错误');
    });
    // v5(M2)/F-M2-02：▶ 续传——按原方向/路径带 resume:true 重新入队，从断点继续
    const rbtn = row.querySelector('.xresume');
    if (rbtn) rbtn.addEventListener('click', async () => {
      const sftpId = Number(t.sftpId) || 0;
      if (!sftpId) { $('#statusBar').textContent = '续传失败: 原 SFTP 通道信息缺失,请重发传输任务'; return; }
      const res = await window.easyssh.xferStart(sftpId, {
        direction: t.direction, localPath: t.localPath, remotePath: t.remotePath, resume: true,
      });
      if (!res.ok) $('#statusBar').textContent = '续传失败: ' + (res.message || '未知错误');
      else $('#statusBar').textContent = '已从断点继续传输(已传 ' + fmtSize(t.bytesDone) + ')';
    });
    ul.appendChild(row);
  }
}

$('#btnXferQueue').addEventListener('click', async () => {
  $('#xferTitle').textContent = ' — 最近 50 条';
  try {
    const list = await window.easyssh.xferList();
    for (const t of list) xferTasks.set(t.taskId, t);
  } catch { /* 拉取失败就用缓存渲染 */ }
  renderXferBadge();
  renderXferList();
  $('#xferModal').classList.remove('hidden');
});
$('#btnXferClear').addEventListener('click', () => {
  for (const t of xferTasks.values()) {
    if (t.phase !== 'queued' && t.phase !== 'running') xferCleared.add(t.taskId);
  }
  renderXferList();
});
// 进度事件实时刷新(约 200ms 节流,phase 变化必达)
window.easyssh.onXferProgress((t) => {
  if (!t || !t.taskId) return;
  xferTasks.set(t.taskId, t);
  renderXferBadge();
  if (!$('#xferModal').classList.contains('hidden')) renderXferList();
});

// v4.0.10:传输进度事件 → 覆盖层刷新
if (window.easyssh && window.easyssh.onXferProgress) {
  window.easyssh.onXferProgress((p) => {
    if (!p) return;
    // 竞态兜底：进度事件先于 startUploadTask 返回时自动补建覆盖层条目
    if (!xferOverlayItems.has(p.id) && (p.status === 'running' || p.status === 'queued')) {
      const nm = p.direction === 'upload'
        ? (p.localPath || '').split('\\').pop().split('/').pop()
        : (p.remotePath || '').split('/').pop();
      xferOverlayAdd(p.id, nm || ('任务 #' + p.id));
    }
    if (!xferOverlayItems.has(p.id)) return;
    if (p.status === 'running') {
      xferOverlayUpdate(p.id, p.received || 0, p.total || 0);
      // 双保险:同步写入 SFTP 面板消息行(文本进度,任何层级下都可见)
      const pct = (p.total || 0) > 0 ? Math.min(100, Math.round((p.received || 0) / p.total * 100)) : 0;
      const sm = $('#sftpMsg');
      const nm = xferOverlayItems.get(p.id) || '';
      if (sm && !$('#sftpModal').classList.contains('hidden')) {
        sm.textContent = '⬆ 上传中 ' + pct + '% (' + fmtSize(p.received || 0) + ' / ' + fmtSize(p.total) + ')  ' + nm;
        sm.style.color = 'var(--color-accent)';
      }
      return;
    }
    const sm2 = $('#sftpMsg');
    if (sm2 && p.status === 'done') { sm2.textContent = '✅ 上传完成'; sm2.style.color = ''; }
    else if (p.status === 'done') xferOverlayFinish(p.id, 'done');
    else if (p.status === 'cancelled') xferOverlayFinish(p.id, 'cancelled');
    else if (p.status === 'failed') xferOverlayFinish(p.id, 'failed', p.error || '');
  });
}
