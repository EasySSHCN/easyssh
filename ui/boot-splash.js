// v4.3.0:从 index.html 内联脚本抽出(配合 CSP script-src 'self')
// 兜底:前端初始化异常时 15 秒后强制移除启动画面
setTimeout(function () {
  var sp = document.getElementById('bootSplash');
  if (sp) sp.remove();
}, 15000);
