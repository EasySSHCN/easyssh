// v4.3.0:从 index.html 内联脚本抽出(配合 CSP script-src 'self')
// v4:自绘窗口控制（无原生标题栏）
window.addEventListener('DOMContentLoaded', function () {
  try {
    var cur = window.__TAURI__.window.getCurrentWindow();
    var on = function (id, fn) { var el = document.getElementById(id); if (el) el.addEventListener('click', fn); };
    on('winMin', function () { cur.minimize(); });
    on('winMax', function () { cur.toggleMaximize(); });
    on('winClose', function () { cur.close(); });
  } catch (e) { console.error('win controls', e); }
});
