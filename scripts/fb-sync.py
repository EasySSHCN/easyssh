#!/usr/bin/env python3
# fb-sync.py —— 需求池→官网反馈 双向同步回写助手(由 harvest-feedback.ps1 调用)
# 输入: /tmp/fb-updates.json  =>  [{"fbId": "...", "reply": "...", "public": true/false}]
# 行为: 对 feedback-db.json 中对应条目写入官方回复并按需公开/下架;
#       已有完全相同回复的跳过(防重复);统一置 replyAt 并修正属主;输出变更统计。
import json, time

DB = "/www/wwwroot/easyssh.cn/data/feedback-db.json"
UPD = "/tmp/fb-updates.json"

db = json.load(open(DB, encoding="utf-8-sig"))
raw = json.load(open(UPD, encoding="utf-8-sig"))
# 兼容两种格式:裸数组,或 PowerShell ConvertTo-Json 的 {"u":[...]} 包装(单元素时可能是单对象)
if isinstance(raw, list):
    updates = raw
elif isinstance(raw, dict):
    u = raw.get("u", [])
    updates = u if isinstance(u, list) else [u]
else:
    updates = [raw]
by_id = {it.get("id"): it for it in db["items"]}

changed, missed = 0, []
for u in updates:
    it = by_id.get(u["fbId"])
    if not it:
        missed.append(u["fbId"])
        continue
    if (it.get("reply") or "").strip() == u["reply"].strip() and bool(it.get("public")) == bool(u["public"]):
        continue
    it["reply"] = u["reply"].strip()
    it["public"] = bool(u["public"])
    it["replyAt"] = int(time.time() * 1000)
    changed += 1

json.dump(db, open(DB, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
print("updated:", changed, "| skipped(same):", len(updates) - changed - len(missed), "| not-found:", missed)
