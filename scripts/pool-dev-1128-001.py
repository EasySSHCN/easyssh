#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""R-20261128-001 状态流转：记录 → 已开发未发布（默认窗口尺寸 1083x821 已改完，随下一版发布）"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
ID = "R-20261128-001"
NOTE = ("开发完成待发布：tauri.conf.json 默认窗口尺寸 1280x820 → 1083x821（实测本机 UI 像素，"
        "与 window.json 自记 1081x820 逻辑像素等值确认）；JSON 校验通过；仅影响首次运行默认值，"
        "已有用户窗口几何由 v5.2.2 记忆机制保护不受影响。发布时无需再改代码。")
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
print("OK R-20261128-001 -> 已开发未发布 (读回复核通过)")
