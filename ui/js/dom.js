/* ============================================================
 * dom.js — 通用 DOM 工具（从 renderer.js 提取）
 * 职责：选择器快捷方式、HTML 转义、通用尺寸格式化
 * ============================================================ */

/* 全局选择器快捷方式（原 renderer.js 首行 const $） */
export const $ = (s) => document.querySelector(s);

/* HTML 转义（原 v2.9.0 会话列表使用的 escH，SFTP/传输队列复用） */
export function escH(v) {
  return String(v === null || v === undefined ? '' : v).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

/* 字节数人性化（原 SFTP 面板 fmtSize） */
export function fmtSize(n) {
  if (n < 1024) return n + ' B';
  if (n < 1048576) return (n / 1024).toFixed(1) + ' KB';
  if (n < 1073741824) return (n / 1048576).toFixed(1) + ' MB';
  return (n / 1073741824).toFixed(2) + ' GB';
}

/* 传输速率人性化（原传输队列 fmtSpeed） */
export function fmtSpeed(bps) {
  if (!bps || bps <= 0) return '';
  if (bps < 1024) return Math.round(bps) + ' B/s';
  if (bps < 1048576) return (bps / 1024).toFixed(1) + ' KB/s';
  return (bps / 1048576).toFixed(1) + ' MB/s';
}
