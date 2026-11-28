#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""EasySSH v5.2.7 发布收尾：R-20261126-063 / R-20261126-065 完成发布回写（另含两项用户会话 UI 反馈随版修复）"""
import json, io, os, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
now = datetime.datetime.now().strftime("%Y-%m-%dT%H:%M:%S+08:00")
VER = "5.2.7"
IDS = ["R-20261126-063", "R-20261126-065"]
NOTE = ("随 v5.2.7 发布收口：build-v4-local 产物（portable/update/zip/source，SHA256 见 dist）+ push-release intake 冒烟门禁全过"
        "（latestVersion=5.2.7，快照 snapshot-v5.2.7-20261128-092101）+ verify-release 4/4 PASS。"
        "开发内容：①SFTP 文件清单「文件名/大小/修改时间」点击列名排序(目录恒在前,选择本地记忆,行内新增修改时间列,R-20261126-063)；"
        "②顶部【下载选中】按钮勾选批量下载(与右键批量同落盘逻辑)+SFTP 默认进入目录记忆(最近目录自动记忆+右键固定默认入口,R-20261126-065)；"
        "③随版修复:浅色主题已连接标签文字改深绿(#047857)+标题栏 LOGO 圆角版本号徽章(主进程实时读取,与状态栏同源)。"
        "发版注意:11-27 ROOTFIX 冻结 site/ 后 intake 数据文件只落站点源树,本次人工补发 webroot 7 文件+英文动态数据,BOM 已清除")
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
