/* ============================================================
 * feedback.js — 使用反馈弹窗（从 renderer.js 提取）
 * 职责：反馈表单、字数校验、加密提交到官网。
 * ============================================================ */
import { $ } from './dom.js';

/* ---------- 使用反馈(加密提交到官网) ---------- */
$('#btnFeedback').addEventListener('click', () => {
  $('#fbMsg').textContent = '';
  $('#fbModal').classList.remove('hidden');
  $('#fbContent').focus();
});
$('#fbOpenSite').addEventListener('click', (e) => {
  e.preventDefault();
  window.open('https://easyssh.cn/download.html', '_blank');
});
// v4.3.2-ui 新增:「计算字数」按钮——显示当前字符数,并判断是否符合 5-2000 字要求,不符合显示原因
$('#fbCount').addEventListener('click', () => {
  const el = $('#fbCountResult');
  const raw = $('#fbContent').value;
  const n = raw.length;                 // 与提交端/服务端一致的字符统计(.trim 前的原始长度)
  const trimmed = raw.trim().length;    // 提交时按 trim 后内容校验
  const cnt = trimmed;                  // 展示以提交口径(trim 后)为准
  el.classList.remove('ok', 'bad');
  if (cnt === 0) {
    el.textContent = '当前 ' + n + ' 个字符 —— 不符合:内容为空(至少需要 5 个非空白字符)';
    el.classList.add('bad');
  } else if (cnt < 5) {
    el.textContent = '当前 ' + cnt + ' 个字符(原始 ' + n + ') —— 不符合:少于 5 个字符,再补充 ' + (5 - cnt) + ' 个即可提交';
    el.classList.add('bad');
  } else if (cnt > 2000) {
    el.textContent = '当前 ' + cnt + ' 个字符(原始 ' + n + ') —— 不符合:超过 2000 个字符上限,超出 ' + (cnt - 2000) + ' 个,请精简';
    el.classList.add('bad');
  } else {
    el.textContent = '当前 ' + cnt + ' 个字符 —— ✅ 符合要求(5-2000 字),可以提交';
    el.classList.add('ok');
  }
});
$('#fbContent').addEventListener('input', () => { const el = $('#fbCountResult'); if (el) { el.textContent = ''; el.classList.remove('ok', 'bad'); } });

/* ---------- v5.1.7 (R-20261115-005):多媒体附件(PNG/JPEG/GIF/WebP,≤8MB,最多3个) ---------- */
const FB_ATT_MAX = 8 * 1024 * 1024;
const FB_ATT_MAX_COUNT = 3;
const FB_ATT_EXT_OK = ['png', 'jpg', 'jpeg', 'gif', 'webp'];
let fbFiles = []; // [{ path, name, size }]

function fbFilesRender() {
  const box = $('#fbFiles');
  if (!box) return;
  if (!fbFiles.length) { box.style.display = 'none'; box.textContent = ''; return; }
  box.style.display = 'block';
  box.textContent = '';
  fbFiles.forEach((f, i) => {
    const row = document.createElement('div');
    row.textContent = `${i + 1}. ${f.name} (${(f.size / 1024 / 1024).toFixed(2)}MB) `;
    const rm = document.createElement('a');
    rm.href = 'javascript:void(0)';
    rm.textContent = '移除';
    rm.style.color = 'var(--color-danger)';
    rm.addEventListener('click', () => { fbFiles.splice(i, 1); fbFilesRender(); });
    row.appendChild(rm);
    box.appendChild(row);
  });
}
$('#fbAttach').addEventListener('click', async () => {
  const el = $('#fbMsg');
  if (fbFiles.length >= FB_ATT_MAX_COUNT) { el.textContent = `最多附加 ${FB_ATT_MAX_COUNT} 个文件`; el.style.color = 'var(--color-danger)'; return; }
  let paths = [];
  try { paths = await window.easyssh.fbPickImages() || []; } catch (e) { el.textContent = '选择文件失败: ' + (e.message || e); el.style.color = 'var(--color-danger)'; return; }
  for (const p of paths) {
    if (fbFiles.length >= FB_ATT_MAX_COUNT) { el.textContent = `最多附加 ${FB_ATT_MAX_COUNT} 个文件`; el.style.color = 'var(--color-danger)'; break; }
    const name = p.split(/[\\/]/).pop() || p;
    const ext = (name.split('.').pop() || '').toLowerCase();
    if (!FB_ATT_EXT_OK.includes(ext)) { el.textContent = `不支持的文件类型: ${name}(仅 PNG/JPEG/GIF/WebP)`; el.style.color = 'var(--color-danger)'; continue; }
    if (fbFiles.some(f => f.path === p)) continue;
    fbFiles.push({ path: p, name, size: 0 });
  }
  fbFilesRender();
});

$('#fbSubmit').addEventListener('click', async () => {
  const btn = $('#fbSubmit');
  const content = $('#fbContent').value.trim();
  if (content.length < 5) { $('#fbMsg').textContent = '反馈内容至少 5 个字'; $('#fbMsg').style.color = 'var(--color-danger)'; return; }
  btn.disabled = true;
  $('#fbMsg').textContent = '提交中…'; $('#fbMsg').style.color = '';
  try {
    // v5.1.7:先上传附件(逐个加密上传,服务端校验/清洗后返回 attId),再提交反馈文本
    const attachments = [];
    for (const f of fbFiles) {
      $('#fbMsg').textContent = `正在上传附件 ${f.name}…`;
      const r = await window.easyssh.fbUploadFile(f.path);
      if (!r || !r.ok) throw new Error(r && r.message || '上传失败');
      attachments.push(r.id);
    }
    const res = await window.easyssh.fbSubmit({
      type: $('#fbType').value,
      contact: $('#fbContact').value.trim(),
      content,
      attachments,
    });
    $('#fbMsg').textContent = res.message || (res.ok ? '已提交' : '提交失败');
    $('#fbMsg').style.color = res.ok ? 'var(--color-success)' : 'var(--color-danger)';
    if (res.ok) { $('#fbContent').value = ''; $('#fbContact').value = ''; fbFiles = []; fbFilesRender(); }
  } catch (e) {
    $('#fbMsg').textContent = '提交失败: ' + (e.message || e);
    $('#fbMsg').style.color = 'var(--color-danger)';
  } finally { btn.disabled = false; }
});
