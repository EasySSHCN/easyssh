#!/usr/bin/env node
/* =========================================================
 * gen-site-updates.js —— 从《官网更新历史记录.md》生成官网版本数据文件
 * 输出: site/assets/data/site-updates.json
 *  - current: 当前官网版本号
 *  - scheme : 版本号规范（简述）
 *  - versions: [{ ver, date, summary, items: [{ title, detail }] }]，最新在前
 * 维护规则：官网更新后把事件追加进《官网更新历史记录.md》，必要时调整
 * DATE_VER 映射/当前版本号，然后 node tools/gen-site-updates.js 重新生成。
 * ========================================================= */
"use strict";
const fs = require("fs");
const path = require("path");

const ROOT = path.resolve(__dirname, "..");
const MD = path.join(ROOT, "官网更新历史记录.md");
const OUT = path.join(ROOT, "site", "assets", "data", "site-updates.json");

/* 日期 → 官网版本号映射（见 docs/官网版本号规范.md） */
const DATE_VER = {
  "2026-11-01": { ver: "1.0.0", summary: "发布链路与源码体系确立" },
  "2026-11-02": { ver: "1.1.0", summary: "发布流程全渠道打通（intake/GitHub/Gitee/OTA）" },
  "2026-11-03": { ver: "1.2.0", summary: "品牌与内容更新（LOGO/截图/签名公钥公示）" },
  "2026-11-04": { ver: "1.3.0", summary: "内容 v5 化 + 使用反馈体系建立" },
  "2026-11-05": { ver: "1.4.0", summary: "v5.0.8 发布配套与反馈精选完善" },
  "2026-11-06": { ver: "1.5.0", summary: "发版链路梳理与页面配套更新" },
  "2026-11-07": { ver: "1.6.0", summary: "全站 SEO 优化与搜索引擎收录" },
  "2026-11-08": { ver: "1.6.1", summary: "需求池恢复与反馈状态回写" },
  "2026-11-10": { ver: "1.7.0", summary: "v5.0.13 发布官网上线" },
  "2026-11-11": { ver: "2.0.0", summary: "统一后台管理页 admin.html 上线（架构级改版）" },
  "2026-11-12": { ver: "2.1.0", summary: "后台安全加固收尾与明细弹窗" },
  "2026-11-13": { ver: "2.2.0", summary: "官网访客统计标签页上线" },
  "2026-11-14": { ver: "2.3.0", summary: "全站中英双语化 + 访客统计完善" },
  "2026-11-15": { ver: "2.4.0", summary: "后台强化、下载来源归因与发布回滚机制" },
  "2026-11-16": { ver: "2.5.0", summary: "整站灾备热备（la→sv）与反馈附件多媒体链路" },
  "2026-11-17": { ver: "2.6.0", summary: "官网更新记录标签页 + 页脚版本号 + 后台运维修复" }
};
const CURRENT = "2.6.0";
const UPDATED_AT = "2026-11-17";
/* 本次功能事件已写入《官网更新历史记录.md》2026-11-17 小节，由下方解析自动收录。 */

/* ---------- 解析 md ---------- */
const text = fs.readFileSync(MD, "utf8");
const lines = text.split(/\r?\n/);
const byDate = {}; // date -> [{title, detail}]
let curDate = null, curItem = null;
for (const raw of lines) {
  const m = raw.match(/^###\s+(\d{4}-\d{2}-\d{2})/);
  if (m) { curDate = m[1]; curItem = null; byDate[curDate] = byDate[curDate] || []; continue; }
  if (!curDate) continue;
  const t = raw.match(/^- \*\*(.+?)\*\*\s*$/);
  if (t) { curItem = { title: t[1], detail: [] }; byDate[curDate].push(curItem); continue; }
  if (curItem && /^ {2,}- /.test(raw)) {
    const d = raw.replace(/^ {2,}- /, "").trim();
    if (/^来源[:：]/.test(d)) continue; // 来源会话文件名不入 JSON
    curItem.detail.push(d);
  }
}

/* 汇总为版本数组 */
const verMap = {}; // ver -> {ver,date,summary,items}
for (const date of Object.keys(byDate).sort()) {
  for (const it of byDate[date]) {
    // 少量条目标题自带实际日期（如「…（2026-11-17）」），按真实日期归版本
    const dHit = it.title.match(/(2026-\d{2}-\d{2})/);
    const d = (dHit && DATE_VER[dHit[1]]) ? dHit[1] : date;
    const meta = DATE_VER[d];
    if (!meta) { console.warn("无版本映射，跳过:", d, it.title); continue; }
    verMap[meta.ver] = verMap[meta.ver] || { ver: meta.ver, date: d, summary: meta.summary, items: [] };
    verMap[meta.ver].items.push({ title: it.title, detail: it.detail.join("\n") });
  }
}
const versions = Object.values(verMap).sort((a, b) => a.ver < b.ver ? 1 : -1);
const out = {
  current: CURRENT,
  updatedAt: UPDATED_AT,
  scheme: "语义化版本 vX.Y.Z：X=全站架构/改版级变更；Y=新功能模块上线；Z=缺陷修复与样式文案微调。每次官网更新递增对应位，事件明细见《官网更新历史记录.md》。",
  total: versions.reduce((n, v) => n + v.items.length, 0),
  versions
};
fs.mkdirSync(path.dirname(OUT), { recursive: true });
fs.writeFileSync(OUT, JSON.stringify(out, null, 2), "utf8");
console.log("已生成", OUT, "版本数:", versions.length, "事件数:", out.total, "当前版本:", CURRENT);
