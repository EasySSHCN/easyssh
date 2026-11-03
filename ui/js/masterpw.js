/* ============================================================
 * masterpw.js — 主密码 / 已知主机指纹写保护（从 renderer.js 提取）
 * 职责：masterState 锁定态、会话写盘守卫 saveSessionsGuard、
 *       启动期解锁模态、设置内的启用/更改/关闭管理。
 * 说明：与 sessions.js 存在 import 环（本模块引用 renderSessions，
 *       sessions.js 引用 saveSessionsGuard），全部为函数声明，
 *       ESM 提升机制下运行期调用安全。
 * ============================================================ */
import { $ } from './dom.js';
import { sessions, setSessions } from './state.js';
import { renderSessions } from './sessions.js';

/* ================= v3.0.0 主密码 / 已知主机指纹(渲染端流程) ================= */
// masterState:enabled=config.key.enc 存在;unlocked=本次运行已解包 DEK。
// 未启用主密码时 unlocked 恒为 true,行为与旧版一致(零迁移成本)。
export const masterState = { enabled: false, unlocked: true };
export function masterLocked() { return masterState.enabled && !masterState.unlocked; }
// 锁定态下渲染端内存中的会话密码全是空串,直接保存会把所有已存密码覆盖掉——
// 主密码未解锁时一律拒绝会话写盘(只读浏览不受影响),解锁后自动恢复可用。
export async function saveSessionsGuard(list) {
  if (masterLocked()) {
    $('#statusBar').textContent = '⚠ 主密码未解锁:为保护已存密码,本次会话修改未保存。请先在启动弹窗输入主密码解锁';
    return;
  }
  await window.easyssh.saveSessions(list);
}

/* ---------- 主密码解锁模态(启动早期出现,全屏阻断) ---------- */
export async function showMpUnlock() {
  $('#mpUnlockModal').classList.remove('hidden');
  $('#mpUnlockMsg').textContent = '';
  $('#mpUnlockPw').value = '';
  $('#mpResetRow').classList.add('hidden');
  $('#mpUnlockPw').focus();
}
async function tryMpUnlock() {
  const pw = $('#mpUnlockPw').value;
  if (!pw) { $('#mpUnlockMsg').textContent = '请输入主密码'; return; }
  $('#mpUnlockBtn').disabled = true;
  let r;
  try { r = await window.easyssh.masterpwUnlock(pw); }
  catch (e) { r = { ok: false, message: String(e && e.message || e) }; }
  $('#mpUnlockBtn').disabled = false;
  if (r && r.ok) {
    masterState.unlocked = true;
    $('#mpUnlockModal').classList.add('hidden');
    $('#mpUnlockPw').value = '';
    // 解锁成功后重新加载会话:此前的加载因 DEK 未就绪解不出密码
    try { setSessions(await window.easyssh.loadSessions()); renderSessions(); } catch {}
    $('#statusBar').textContent = '✅ 已解锁:会话密码可用';
  } else if (r && r.locked) {
    $('#mpUnlockMsg').textContent = r.message || '错误次数过多,已锁定';
    $('#mpResetRow').classList.remove('hidden'); // 逃生路径:重置密码字段
  } else {
    $('#mpUnlockMsg').textContent = (r && r.remaining != null)
      ? '主密码错误,还可尝试 ' + r.remaining + ' 次'
      : ((r && r.message) || '解锁失败');
    $('#mpUnlockPw').value = '';
    $('#mpUnlockPw').focus();
  }
}
$('#mpUnlockBtn').addEventListener('click', tryMpUnlock);
$('#mpUnlockPw').addEventListener('keydown', (e) => { if (e.key === 'Enter') tryMpUnlock(); });
$('#mpResetBtn').addEventListener('click', async () => {
  if (!(window.easysshUi && await window.easysshUi.confirm('确认重置?将删除主密码文件并清空所有已保存的会话密码(会话列表保留,连接时需重新录入密码)。此操作不可撤销。'))) return;
  try { await window.easyssh.masterpwReset(); } catch {}
  masterState.enabled = false;
  masterState.unlocked = true;
  $('#mpUnlockModal').classList.add('hidden');
  try { setSessions(await window.easyssh.loadSessions()); renderSessions(); } catch {}
  $('#statusBar').textContent = '已重置:主密码已清除,会话密码已清空,请重新录入';
});

/* ---------- 主密码管理(设置弹窗内) ---------- */
let mpMode = 'enable'; // enable | change | disable
export function refreshMpStatus() {
  window.easyssh.masterpwStatus().then((st) => {
    masterState.enabled = !!st.enabled;
    if (!st.enabled) masterState.unlocked = true;
    $('#mpStatus').textContent = st.enabled ? '已启用(本机已启用主密码保护)' : '未启用(仅本机密钥文件保护)';
    $('#btnMpEnable').style.display = st.enabled ? 'none' : '';
    $('#btnMpChange').style.display = st.enabled ? '' : 'none';
    $('#btnMpDisable').style.display = st.enabled ? '' : 'none';
  }).catch(() => {});
}
function openMpModal(mode) {
  mpMode = mode;
  $('#mpMsg').textContent = '';
  $('#mpOld').value = ''; $('#mpNew').value = ''; $('#mpNew2').value = '';
  $('#mpOldRow').style.display = (mode === 'enable') ? 'none' : 'flex';
  $('#mpNewRow').style.display = (mode === 'disable') ? 'none' : 'flex';
  $('#mpNew2Row').style.display = (mode === 'disable') ? 'none' : 'flex';
  const titles = { enable: '启用主密码', change: '更改主密码', disable: '关闭主密码' };
  $('#mpModeTitle').textContent = titles[mode] || '';
  $('#mpTips').textContent = mode === 'enable'
    ? '启用后:现有会话密钥(config.key)被主密码包裹为 config.key.enc,原文件改名为 config.key.bak(首次成功解锁后自动删除)。主密码遗忘无法找回。'
    : (mode === 'change'
      ? '输入当前主密码验证后,用新主密码重新包裹会话密钥。'
      : '输入主密码验证后,还原 config.key 明文文件并删除 config.key.enc(恢复到无主密码状态,保护强度回到仅本机密钥)。');
  $('#mpModal').classList.remove('hidden');
  (mode === 'enable' ? $('#mpNew') : $('#mpOld')).focus();
}
$('#btnMpEnable').addEventListener('click', () => openMpModal('enable'));
$('#btnMpChange').addEventListener('click', () => openMpModal('change'));
$('#btnMpDisable').addEventListener('click', () => openMpModal('disable'));
$('#mpCancel2').addEventListener('click', () => $('#mpModal').classList.add('hidden'));
$('#mpGo').addEventListener('click', async () => {
  const oldPw = $('#mpOld').value, newPw = $('#mpNew').value, newPw2 = $('#mpNew2').value;
  const fail = (msg) => { $('#mpMsg').textContent = msg; };
  let r;
  try {
    if (mpMode === 'enable') {
      if (!newPw || newPw.length < 6) return fail('主密码至少需要 6 位');
      if (newPw !== newPw2) return fail('两次输入的主密码不一致');
      r = await window.easyssh.masterpwEnable(newPw);
    } else if (mpMode === 'change') {
      if (!oldPw) return fail('请输入当前主密码');
      if (!newPw || newPw.length < 6) return fail('新主密码至少需要 6 位');
      if (newPw !== newPw2) return fail('两次输入的新主密码不一致');
      r = await window.easyssh.masterpwChange(oldPw, newPw);
    } else {
      if (!oldPw) return fail('请输入主密码');
      r = await window.easyssh.masterpwDisable(oldPw);
    }
  } catch (e) { r = { ok: false, message: String(e && e.message || e) }; }
  if (!r || !r.ok) return fail((r && r.message) || '操作失败');
  masterState.unlocked = true;
  $('#mpModal').classList.add('hidden');
  $('#statusBar').textContent = {
    enable: '✅ 主密码已启用:config.key 已包裹为 config.key.enc',
    change: '✅ 主密码已更改:会话密钥已用新口令重新包裹',
    disable: '✅ 主密码已关闭:已还原 config.key,删除 config.key.enc',
  }[mpMode] || '已更新';
  refreshMpStatus();
});

/* v3.0.0 启动早期:检测主密码状态,启用时立即弹全屏阻断解锁模态。
 * 解锁前会话列表照常显示(密码解出为空,连接会失败——可接受的取态),解锁成功后自动重新加载会话。 */
export async function initMasterPw() {
  try {
    const st = await window.easyssh.masterpwStatus();
    masterState.enabled = !!st.enabled;
    masterState.unlocked = !st.enabled;
    if (st.enabled) showMpUnlock();
  } catch { /* 检测失败按未启用处理,与旧版一致 */ }
}
