#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""EasySSH v5.2.6 发布收尾：R-20261122-004 完成发布回写"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "5.2.6"
ID = "R-20261122-004"
NOTE = "随 v5.2.6 发布收口：build-v4-local 六件产物（portable/update/zip/source/官网 exe/EasySSH-5.2.6-gateway-deploy.zip）+ push-release intake 全过（latestVersion=5.2.6，快照 snapshot-v5.2.6-20261123-082352）+ GitHub/Gitee main+v4-dev+tag v5.2.6 与 Release 三附件；features.html 双语新增「云网关反向注册 v5.2.6」卡片、site-data.en.json 追加英文条目（111 条置顶）"

POOL = os.path.join(ROOT, "reqpool", "requirements-pool.json")
d = json.load(io.open(POOL, encoding="utf-8-sig"))
for it in d["items"]:
    if it["id"] == ID:
        it["status"] = "完成发布"
        it["releaseVersion"] = VER
        it.setdefault("statusHistory", []).append({"status": "完成发布", "at": now, "note": NOTE})
json.dump(d, io.open(POOL, "w", encoding="utf-8", newline=""), ensure_ascii=False, indent=4)

REQ = os.path.join(ROOT, "reqpool", "release-request.json")
if os.path.exists(REQ):
    os.remove(REQ)
    print("release-request.json deleted")
print("机器池 R-20261122-004 → 完成发布 v5.2.6 回写完成")
