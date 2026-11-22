/* slider-captcha.js — 滑块拼图验证（自包含，canvas 生成背景，无外部依赖）
 * 用法: window.SliderCaptcha.show({ onSuccess }) ; verified 属性表示当前是否已通过
 * CSP: script-src 'self' 兼容（外部文件，无内联）
 */
'use strict';
(function () {
  const W = 320, H = 160;          // 背景尺寸
  const PW = 44, PH = 44;          // 拼图块尺寸
  const TRACK_H = 40;
  const TOLERANCE = 6;             // 判定误差 px
  let verified = false;
  let overlay = null, drag = null, state = null;

  function el(tag, cls, parent) {
    const e = document.createElement(tag);
    if (cls) e.className = cls;
    if (parent) parent.appendChild(e);
    return e;
  }

  /* ---------- 背景图生成（夜色渐变 + 霓虹色块 + 噪点） ---------- */
  function paintBackground(ctx) {
    const g = ctx.createLinearGradient(0, 0, 0, H);
    g.addColorStop(0, '#1a2340'); g.addColorStop(0.5, '#3b2a5a'); g.addColorStop(1, '#12172e');
    ctx.fillStyle = g; ctx.fillRect(0, 0, W, H);
    // 随机霓虹竖条（模拟街景灯牌）
    const colors = ['#ff5f7e', '#ffd166', '#5df2d6', '#7aa2ff', '#c77dff', '#ff9f45'];
    let x = 6;
    while (x < W - 20) {
      const bw = 10 + Math.random() * 26, bh = 30 + Math.random() * 80;
      const c = colors[(Math.random() * colors.length) | 0];
      ctx.globalAlpha = 0.55 + Math.random() * 0.4;
      ctx.fillStyle = c;
      ctx.fillRect(x, 20 + Math.random() * 40, bw, bh);
      ctx.globalAlpha = 0.18;
      ctx.fillStyle = '#ffffff';
      ctx.fillRect(x + 2, 22 + Math.random() * 36, bw - 4, 3);
      x += bw + 4 + Math.random() * 18;
    }
    ctx.globalAlpha = 1;
    // 噪点
    for (let i = 0; i < 900; i++) {
      ctx.fillStyle = 'rgba(255,255,255,' + (Math.random() * 0.08) + ')';
      ctx.fillRect(Math.random() * W, Math.random() * H, 1, 1);
    }
    // 地面
    ctx.fillStyle = 'rgba(8,10,24,0.8)'; ctx.fillRect(0, H - 18, W, 18);
  }

  /* ---------- 拼图块路径 ---------- */
  function puzzlePath(ctx, x, y, r) {
    const s = PW - 2 * r;
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.lineTo(x + r + s * 0.4, y);
    ctx.arc(x + r + s * 0.5, y, r * 0.7, Math.PI, 0);          // 顶部凸起
    ctx.lineTo(x + PW - r, y);
    ctx.arc(x + PW - r, y + r, r, -Math.PI / 2, 0);
    ctx.lineTo(x + PW, y + r + s * 0.5 - r * 0.7);
    ctx.arc(x + PW + r * 0.7, y + r + s * 0.5, r * 0.7, -Math.PI / 2, Math.PI / 2); // 右侧凸起
    ctx.lineTo(x + PW, y + PH - r);
    ctx.arc(x + PW - r, y + PH - r, r, 0, Math.PI / 2);
    ctx.lineTo(x + r + s * 0.5 + r * 0.7, y + PH);
    ctx.arc(x + r + s * 0.5, y + PH + r * 0.7, r * 0.7, Math.PI, 0); // 底部凸起
    ctx.lineTo(x + r, y + PH);
    ctx.arc(x + r, y + PH - r, r, Math.PI / 2, Math.PI);
    ctx.lineTo(x, y + r);
    ctx.arc(x + r, y + r, r, Math.PI, Math.PI * 1.5);
    ctx.closePath();
  }

  function buildImages() {
    const targetX = 8 + Math.round(Math.random() * (W - PW - 40)) + PW * 0; // 目标缺口 x
    const tx = Math.max(W * 0.55, Math.min(W - PW - 6, W * 0.55 + Math.random() * (W * 0.4 - PW)));
    // 背景（带缺口阴影）
    const bg = document.createElement('canvas'); bg.width = W; bg.height = H;
    const bctx = bg.getContext('2d');
    paintBackground(bctx);
    bctx.save();
    puzzlePath(bctx, tx, (H - PH) / 2, 8);
    bctx.clip();
    bctx.fillStyle = 'rgba(0,0,0,0.55)'; bctx.fillRect(0, 0, W, H);
    bctx.restore();
    // 拼图块
    const piece = document.createElement('canvas'); piece.width = PW + 16; piece.height = PH + 16;
    const pctx = piece.getContext('2d');
    pctx.save();
    puzzlePath(pctx, 8, 8, 8); pctx.clip();
    pctx.drawImage(bg, tx - 8, (H - PH) / 2 - 8, PW + 16, PH + 16, 0, 0, PW + 16, PH + 16);
    // 覆盖原缺口阴影 -> 用无缺口背景对应区域重绘
    pctx.restore();
    const clean = document.createElement('canvas'); clean.width = W; clean.height = H;
    paintBackground(clean.getContext('2d'));
    pctx.save();
    puzzlePath(pctx, 8, 8, 8); pctx.clip();
    pctx.drawImage(clean, tx - 8, (H - PH) / 2 - 8, PW + 16, PH + 16, 0, 0, PW + 16, PH + 16);
    pctx.restore();
    // 块描边
    pctx.save();
    puzzlePath(pctx, 8, 8, 8);
    pctx.strokeStyle = 'rgba(255,255,255,0.9)'; pctx.lineWidth = 2; pctx.stroke();
    pctx.fillStyle = 'rgba(0,0,0,0.25)'; pctx.shadowColor = '#000'; pctx.shadowBlur = 6;
    pctx.globalCompositeOperation = 'destination-over';
    puzzlePath(pctx, 8, 10, 8); pctx.fill();
    pctx.restore();
    return { bg, piece, targetX: tx, pieceY: (H - PH) / 2 };
  }

  /* ---------- UI ---------- */
  function buildOverlay() {
    overlay = el('div', 'sc-overlay');
    overlay.innerHTML = `
<div class="sc-mask"></div>
<div class="sc-dialog" role="dialog" aria-label="请完成安全验证">
  <div class="sc-head"><span>请完成安全验证</span><button class="sc-close" type="button" aria-label="关闭">&times;</button></div>
  <div class="sc-body">
    <div class="sc-canvas-wrap">
      <canvas class="sc-bg" width="${W}" height="${H}"></canvas>
      <canvas class="sc-piece" width="${PW + 16}" height="${PH + 16}"></canvas>
      <button class="sc-refresh" type="button" title="换一张">&#8635;</button>
      <div class="sc-tip"></div>
    </div>
    <div class="sc-track"><span class="sc-track-text">向右拖动滑块填充拼图</span>
      <button class="sc-knob" type="button" aria-label="滑块">&#10148;</button>
    </div>
  </div>
</div>`;
    const css = document.createElement('style');
    css.textContent = `
.sc-overlay{position:fixed;inset:0;z-index:99999;display:none}
.sc-overlay.sc-show{display:block}
.sc-mask{position:absolute;inset:0;background:rgba(0,0,0,.45)}
.sc-dialog{position:absolute;left:50%;top:50%;transform:translate(-50%,-50%);width:${W + 32}px;background:#fff;border-radius:10px;box-shadow:0 12px 40px rgba(0,0,0,.35);overflow:hidden;font-family:system-ui,"Microsoft YaHei",sans-serif}
.sc-head{display:flex;justify-content:space-between;align-items:center;padding:12px 16px;font-size:15px;color:#333;border-bottom:1px solid #eee}
.sc-close{background:none;border:none;font-size:20px;color:#999;cursor:pointer;line-height:1}
.sc-body{padding:14px 16px 16px}
.sc-canvas-wrap{position:relative;width:${W}px;height:${H}px;border-radius:6px;overflow:hidden;margin-bottom:12px}
.sc-bg{display:block}
.sc-piece{position:absolute;top:0;left:0;will-change:transform}
.sc-refresh{position:absolute;top:8px;right:8px;width:28px;height:28px;border:none;border-radius:50%;background:rgba(0,0,0,.45);color:#fff;font-size:16px;cursor:pointer}
.sc-refresh:hover{background:rgba(0,0,0,.65)}
.sc-tip{position:absolute;left:0;bottom:0;right:0;text-align:center;font-size:13px;color:#fff;padding:5px 0;background:rgba(0,0,0,.4);opacity:0;transition:opacity .2s}
.sc-tip.sc-ok{background:rgba(76,175,80,.85)}
.sc-tip.sc-bad{background:rgba(229,57,53,.85)}
.sc-track{position:relative;height:${TRACK_H}px;background:#eef1f6;border-radius:6px;overflow:hidden;user-select:none}
.sc-track-text{position:absolute;inset:0;text-align:center;line-height:${TRACK_H}px;font-size:13px;color:#9aa3b2;pointer-events:none}
.sc-track.sc-ok{background:#e3f6e5}.sc-track.sc-ok .sc-track-text{color:#4caf50}
.sc-knob{position:absolute;top:0;left:0;width:44px;height:${TRACK_H}px;border:none;border-radius:6px;background:#2f6fe4;color:#fff;font-size:15px;cursor:grab;touch-action:none}
.sc-knob:active{cursor:grabbing}
.sc-track.sc-shake{animation:scShake .35s}
@keyframes scShake{0%,100%{transform:translateX(0)}25%{transform:translateX(-7px)}75%{transform:translateX(7px)}}`;
    document.head.appendChild(css);
    document.body.appendChild(overlay);

    overlay.querySelector('.sc-close').addEventListener('click', hide);
    overlay.querySelector('.sc-refresh').addEventListener('click', reset);
    overlay.querySelector('.sc-mask').addEventListener('click', hide);

    const knob = overlay.querySelector('.sc-knob');
    knob.addEventListener('pointerdown', onDown);
  }

  function onDown(ev) {
    ev.preventDefault();
    const knob = ev.currentTarget;
    knob.setPointerCapture(ev.pointerId);
    const startX = ev.clientX;
    const max = W - 44;
    const piece = overlay.querySelector('.sc-piece');
    const move = (e) => {
      let dx = Math.max(0, Math.min(max, e.clientX - startX));
      knob.style.left = dx + 'px';
      piece.style.transform = 'translateX(' + dx + 'px)';
    };
    const up = (e) => {
      knob.removeEventListener('pointermove', move);
      knob.removeEventListener('pointerup', up);
      knob.removeEventListener('pointercancel', up);
      let dx = Math.max(0, Math.min(max, e.clientX - startX));
      judge(dx);
    };
    knob.addEventListener('pointermove', move);
    knob.addEventListener('pointerup', up);
    knob.addEventListener('pointercancel', up);
  }

  function judge(dx) {
    const need = state.targetX - 8;           // 拼图需移动的距离
    const tip = overlay.querySelector('.sc-tip');
    const track = overlay.querySelector('.sc-track');
    if (Math.abs(dx - need) <= TOLERANCE) {
      verified = true;
      tip.textContent = '验证通过'; tip.className = 'sc-tip sc-show sc-ok'; tip.style.opacity = 1;
      track.classList.add('sc-ok');
      track.querySelector('.sc-track-text').textContent = '验证通过';
      setTimeout(() => { hide(); if (state.onSuccess) state.onSuccess(); }, 450);
    } else {
      tip.textContent = Math.abs(dx - need) <= 18 ? '再试一次' : '验证失败，请重试';
      tip.className = 'sc-tip sc-show sc-bad'; tip.style.opacity = 1;
      track.classList.add('sc-shake');
      setTimeout(() => { track.classList.remove('sc-shake'); tip.style.opacity = 0; }, 500);
      setTimeout(reset, 550);
    }
  }

  function reset() {
    verified = false;
    if (!overlay) return;
    const cb = state && state.onSuccess;
    state = buildImages();
    state.onSuccess = cb;
    const bg = overlay.querySelector('.sc-bg');
    const piece = overlay.querySelector('.sc-piece');
    bg.getContext('2d').drawImage(state.bg, 0, 0);
    piece.getContext('2d').clearRect(0, 0, piece.width, piece.height);
    piece.getContext('2d').drawImage(state.piece, 0, 0);
    // 纵向对齐修正：块内拼图起始点为 (8,8)，显示 top 需减去 8px 才能与背景缺口 y 精确重合
    piece.style.top = (state.pieceY - 8) + 'px';
    piece.style.transform = 'translateX(0px)';
    const knob = overlay.querySelector('.sc-knob');
    knob.style.left = '0px';
    const track = overlay.querySelector('.sc-track');
    track.classList.remove('sc-ok');
    track.querySelector('.sc-track-text').textContent = '向右拖动滑块填充拼图';
  }

  function show(opts) {
    opts = opts || {};
    if (!overlay) buildOverlay();
    state = buildImages();
    state.onSuccess = opts.onSuccess || null;
    reset();
    overlay.classList.add('sc-show');
  }
  function hide() { if (overlay) overlay.classList.remove('sc-show'); }

  window.SliderCaptcha = {
    show, hide, reset,
    get verified() { return verified; },
    set verified(v) { verified = !!v; }
  };
})();
