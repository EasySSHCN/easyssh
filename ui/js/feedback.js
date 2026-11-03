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
$('#fbSubmit').addEventListener('click', async () => {
  const btn = $('#fbSubmit');
  const content = $('#fbContent').value.trim();
  if (content.length < 5) { $('#fbMsg').textContent = '反馈内容至少 5 个字'; $('#fbMsg').style.color = 'var(--red)'; return; }
  btn.disabled = true;
  $('#fbMsg').textContent = '提交中…'; $('#fbMsg').style.color = '';
  try {
    const res = await window.easyssh.fbSubmit({
      type: $('#fbType').value,
      contact: $('#fbContact').value.trim(),
      content,
    });
    $('#fbMsg').textContent = res.message || (res.ok ? '已提交' : '提交失败');
    $('#fbMsg').style.color = res.ok ? 'var(--green, #4caf50)' : 'var(--red)';
    if (res.ok) { $('#fbContent').value = ''; $('#fbContact').value = ''; }
  } catch (e) {
    $('#fbMsg').textContent = '提交失败: ' + (e.message || e);
    $('#fbMsg').style.color = 'var(--red)';
  } finally { btn.disabled = false; }
});
