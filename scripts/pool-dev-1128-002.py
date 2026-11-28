#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""R-20261128-002 状态流转：记录 → 已开发未发布（标题栏最大化按钮 SVG 空心方框已改完，随下一版发布）"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
ID = "R-20261128-002"
NOTE = ("开发完成待发布：ui/index.html #winMax 的 &#x25A1;(U+25A1) 字符已替换为内联 SVG 空心方框"
        "（11x11 viewBox，rect 1,1 9x9 描边 1.6，currentColor，display:block），与 ─/✕ 视觉一致；"
        "#winControls 按钮为 flex 居中，SVG 自适应无需改 CSS。发布时无需再改代码。")
POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
d = json.load(io.open(POOL, encoding="utf-8-sig"))
for it in d["items"]:
    if it["id"] == ID:
        it["status"] = "已开发未发布"
        it.setdefault("statusHistory", []).append({"status": "已开发未发布", "at": now, "note": NOTE})
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)
chk = json.load(io.open(POOL, encoding="utf-8-sig"))
it = next(x for x in chk["items"] if x["id"] == ID)
assert it["status"] == "已开发未发布"
print("OK R-20261128-002 -> 已开发未发布 (读回复核通过)")
