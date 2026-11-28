#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""EasySSH v5.2.9 发布收尾：R-20261128-001 / R-20261128-002 完成发布回写（另含随版修复:single_instance 重复注册致 --smoke 冒烟失效）"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "5.2.9"
IDS = ["R-20261128-001", "R-20261128-002"]
NOTE = ("随 v5.2.9 发布收口：build-v4-local 产物（portable/update/zip/source，SHA256 见 dist）+ push-release intake 冒烟门禁全过"
        "（latestVersion=5.2.9，快照 snapshot-v5.2.9-20261128-111027）+ verify-release 4/4 PASS（webroot 数据文件沿用人工补发流程 7 文件+去 BOM）。"
        "开发内容：①默认窗口显示大小改为 1083x821（对齐本机实测 UI 像素，仅影响首次运行默认值，已有用户几何由记忆机制保护，R-20261128-001）；"
        "②标题栏最大化按钮 □(U+25A1) 字符替换为内联 SVG 空心方框（11x11，描边 1.6，currentColor），与 ─/✕ 视觉一致（R-20261128-002）；"
        "③随版修复:main.rs single_instance 被无条件二次注册致 --smoke 冒烟门禁误报「8 秒内退出」，已删除重复注册，v5.0.8 的 --smoke 跳过语义恢复。"
        "版本校验:next-version.ps1 -Current 5.2.8 → 5.2.9，三级递进规则一致")
POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
d = json.load(io.open(POOL, encoding="utf-8-sig"))
hit = 0
for it in d["items"]:
    if it["id"] in IDS:
        it["status"] = "完成发布"
        it["releaseVersion"] = VER
        it.setdefault("statusHistory", []).append({"status": "完成发布", "at": now, "note": NOTE})
        hit += 1
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)

REQ = os.path.join(ROOT, "reqpool", "release-request.json")
if os.path.exists(REQ):
    os.remove(REQ)
    print("release-request.json deleted")
print("机器池回写完成: %d/%d 条 → 完成发布 v%s" % (hit, len(IDS), VER))
