# EasySSH 官网管理后台 UI 规范（项目级永久）

> 建立：2026-11-22（admin.html 三项改造后定版）
> 适用范围：`site-server/site/` 下官网页面与管理后台（admin.html、gw-admin.html 等），不分项目页面一律执行。

## 一、完整分页六要素强制规范

凡是需要分页的列表，分页条必须逐项包含：

1. 共 X 条 · 第 P/N 页
2. « 首页、‹ 上一页、页码（多页带 … 省略折叠）、下一页 ›、末页 »
3. 每页条数下拉（10/20/50，默认值并 localStorage 记忆，key 前缀 `gw_`）

**交付验收规则**：必须按六要素清单逐项核对后才能交付，禁止以"对齐/复刻参照物现状"替代规范核对。教训（2026-11-22）：gw-admin.html 已签发 Token 列表首版分页漏掉每页条数设置与首页/末页——当时参照物本身就不完整，且实现时没有回头对照本规范。

**现成参考实现**（直接复用模式）：
- `gw-admin.html` tok-pager：`pg-info / pg-btn / pg-gap / pg-size` class + `data-tok-page` 事件委托 + `localStorage("gw_tok_page_size")`（标记 GW-PGFULL-20261122）
- `admin.html` gaPager / renderGapp（标记 GW-AUDPAGE / GW-GRANTPAGE-20261122）

## 二、列表卡片折叠默认态规范

- **官网更新记录**：每版本卡片版本号行以下内容默认收起，仅最新版本（数据第一条）默认展开；点击版本号标题切换。标记 `GW-CARD-FOLD-20261122`。
- **使用反馈**：状态「待回复」的卡片默认展开，其余状态（已回复等）默认收起（标题第二行以下折叠）；点击标题切换，勾选框/按钮等控件点击不触发折叠。标记 `GW-FBK-FOLD-20261122`。

## 三、部署注意（本项目官网）

- 权威源：`la.trae.show:/opt/dsh/workspace/easyssh-site/site/`，改官网文件必须 scp 到权威源再跑 `tools/deploy.sh`（直接写 webroot 会被 root 定时 rsync 覆盖）。
- 部署后必须 curl 线上核对改动标记；注意 Invoke-WebRequest 默认编码会误判，验证文本类标记用 WebClient 强制 UTF-8 解码。
