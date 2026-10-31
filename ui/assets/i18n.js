/* EasySSH 界面多语言运行时（渲染端，v3.0.8）
 * 零侵入设计：遍历文本节点 + MutationObserver 监听动态内容，
 * 以「简体中文原文」为键在 assets/i18n-dict.js 中查译文整段替换。
 * 跳过用户数据区(#sessionList/#tabbar/#quickbar)与终端(.xterm)，
 * 不会误伤会话名、快捷命令等用户输入内容。
 * 语言存于 settings.lang(默认 zh-CN)；切换语言即时生效无需重启。 */
(function () {
  'use strict';
  const I18N = window.EasySSH_I18N;
  if (!I18N) return;

  // 不翻译的区域（用户数据 / 终端缓冲）
  const SKIP_SELECTOR = '#sessionList,#tabbar,#quickbar,.xterm,script,style,code';
  let locale = 'zh-CN';
  // 原文备份：切换回 zh-CN 时还原
  const origText = new WeakMap();   // TextNode -> original string
  const origAttr = new WeakMap();   // Element -> { attr: originalValue }

  function trText(s) {
    const trimmed = s.trim();
    if (!trimmed) return null;
    const t = I18N.t(locale, trimmed);
    return t === trimmed ? null : t;
  }

  function translateTextNode(node) {
    const s = node.nodeValue;
    if (!s || !/[\u4e00-\u9fff]/.test(s)) return;
    if (locale === 'zh-CN') {
      const o = origText.get(node);
      if (o !== undefined) { node.nodeValue = o; origText.delete(node); }
      return;
    }
    const t = trText(s);
    if (t !== null) {
      if (!origText.has(node)) origText.set(node, s);
      // 保留原前后缀空白，版式不跳动
      const lead = s.match(/^\s*/)[0], tail = s.match(/\s*$/)[0];
      node.nodeValue = lead + t + tail;
    }
  }

  function translateElement(el) {
    for (const attr of ['placeholder', 'title', 'aria-label']) {
      const v = el.getAttribute && el.getAttribute(attr);
      if (!v || !/[\u4e00-\u9fff]/.test(v)) continue;
      if (locale === 'zh-CN') {
        const m = origAttr.get(el);
        if (m && m[attr] !== undefined) el.setAttribute(attr, m[attr]);
        continue;
      }
      const trimmed = v.trim();
      const t = I18N.t(locale, trimmed);
      if (t !== trimmed) {
        if (!origAttr.has(el)) origAttr.set(el, {});
        const m = origAttr.get(el);
        if (m[attr] === undefined) m[attr] = v;
        el.setAttribute(attr, t);
      }
    }
  }

  function inSkip(node) {
    return !!(node.closest && node.closest(SKIP_SELECTOR));
  }

  function walk(root) {
    if (!root || inSkip(root)) return;
    // 根本身是元素时，先处理其子树
    const tw = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT, {
      acceptNode(n) {
        if (inSkip(n)) return NodeFilter.FILTER_REJECT;
        return NodeFilter.FILTER_ACCEPT;
      }
    });
    let cur;
    const els = [];
    while ((cur = tw.nextNode())) {
      if (cur.nodeType === 3) translateTextNode(cur);
      else els.push(cur);
    }
    for (const el of els) translateElement(el);
  }

  // 动态内容（状态栏/横幅/模态提示等）自动翻译
  const mo = new MutationObserver((muts) => {
    if (locale === 'zh-CN') return; // 中文原样，无需处理
    for (const m of muts) {
      if (m.type === 'characterData' && m.target.nodeType === 3) {
        if (!inSkip(m.target)) translateTextNode(m.target);
      } else if (m.type === 'childList') {
        m.addedNodes.forEach((n) => { if (n.nodeType === 3) translateTextNode(n); else walk(n); });
      } else if (m.type === 'attributes') {
        translateElement(m.target);
      }
    }
  });

  window.EasySSH_I18N_apply = function (loc) {
    locale = loc || 'zh-CN';
    walk(document.body);
    mo.disconnect();
    if (locale !== 'zh-CN') {
      mo.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ['placeholder', 'title', 'aria-label'] });
    }
  };
})();
