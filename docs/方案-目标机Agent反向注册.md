# 方案 — 目标机 Agent 反向注册（EasySSH 云网关）

> 状态：**立项方案（待评审）**
> 日期：2026-11-22
> 关联：EasySSH 网关隧道（tunnel.rs）、F-R1-49 部署拓扑口径（两站独立源码/独立实例，仅接口协议兼容）
> 适用仓库：`EasySSH/easyssh-gateway`（服务端）、新增 `EasySSH/easyssh-agent`（目标机端）
> 线缆协议：与 `EasyRDP/docs/方案-目标机Agent反向注册.md` 第 3 节为**同一份 wire spec**（帧头/指令表/心跳语义逐字一致），实现各自独立编写，不共用代码。

---

## 1. 背景与问题

EasySSH 网关现状与 EasyRDP 同构：

```
EasySSH客户端 ⇄ 网关(WSS 443) --网关主动TCP连接--> [[resources]] 登记 host:port
```

`tunnel.rs::tunnel_handler` 由网关主动连接后端 SSH 主机（22 端口）。当目标 SSH 主机位于无公网 IP 的内网（仅可出网）时，网关无法触达，云网关链路断裂。

**目标**：目标机运行 easyssh-agent，主动外连网关建立持久 WSS 反向隧道；`tunnel/{resource_id}` 命中 agent 型资源时复用该隧道，实际 TCP 连接由 Agent 在目标机本地向 `127.0.0.1:22`（sshd）发起。

## 2. 架构（与 EasyRDP 同形，SSH 语义）

```
EasySSH客户端 ⇄ 网关 WSS /tunnel/{rid}
                     │  (resources: mode="agent")
                     ▼
              AgentRegistry（resource_id → AgentConn，含健康度）
                     ▲
                     │ WSS /agent/connect（持久、目标机外连、Bearer AgentToken）
                     ▼
              easyssh-agent（目标机）
                     │ 本地 TCP → 127.0.0.1:22 (sshd)
```

## 3. 线缆协议

直接引用共享 wire spec（EasyRDP 方案第 3 节）：

- Binary 帧头 `[type:1][session_id:8 BE][payload_len:4 BE][payload]`
- 0x01 DIAL / 0x02 OPEN_OK / 0x03 OPEN_ERR / 0x04 DATA / 0x05 CLOSE / 0x06 PING / 0x07 PONG
- DIAL payload 的 backend 由网关配置下发（EasySSH 默认 `127.0.0.1:22`），Agent 不接受任意目标。
- 心跳 30s、3 次未应答判死；单连接并发 session 上限 8（可配）。

## 4. 服务端改造点（easyssh-gateway）

| 模块 | 改动 |
|------|------|
| `resources.rs` | `ResourceConfig` 增 `mode: "direct"|"agent"` 与 `agent_local_backend`（默认 `127.0.0.1:22`）；`resolve_backend` 返回 `BackendResolution::Agent(resource_id)` |
| `resources.rs` 新增 | **动态资源注册表**：与静态 `[[resources]]` 并列的运行时资源层（内存 + 落盘 `resources.dyn.toml`，重启恢复），含 `owner`(用户邮箱)/`mode`/enrollment 状态；静态+动态两层合并解析；动态资源 ID 前缀 `u-` 隔离命名空间 |
| 新增 `agent.rs` | `AgentRegistry` + `GET /agent/connect`（独立 auth_guard，`role=agent` AgentToken）；心跳；帧分发 |
| `tunnel.rs` | agent 分支：取健康通道 → 分配 session_id → DIAL → OPEN_OK（10s 超时，`agent_dial_timeout` 审计）→ AsyncWrite 适配器接入既有 `pump_ws_to_tcp/pump_tcp_to_ws`（泵零改动） |
| `auth.rs` | AgentToken：独立 audience/claim 集 `{sub: agent_id, roles:["agent"], rid}`，与用户 JWT 互斥 |
| `admin.rs` | 新增 `POST /admin/resource`（X-Admin-Key，供官网 PHP 回环）：`register`（mode=direct/agent，direct 注册前 3s 试连探测）/`delete`/`status`；注册同步写 grants（owner 邮箱），**保持"验证通过即自动审批"，全程无人工** |
| `config.rs` | `[agent]` 段：enabled / max_sessions_per_conn / heartbeat_secs / dial_timeout_secs / max_resources_per_user |
| `audit.rs` | `agent_online/offline/dial/dial_error/dial_timeout` |
| `policy.rs` | 不变（用户侧 connect 仍走资源×角色矩阵） |

**SSH 特有注意点**（相对 EasyRDP 的差异项）：

1. **长连接特性**：SSH 会话通常数小时不断，Agent 掉线即掐断所有进行中会话且无法恢复（SSH 无 RDP 的自动重连体验）——心跳判死窗口与重连退避参数在 EasySSH 侧建议更保守（心跳 30s、判死 3 次、退避封顶 30s），文档明示"Agent 断线 = 会话中断"。
2. **scp/sftp/端口转发穿透**：均为 TCP 字节流，帧协议天然透明，无需特判；但流量画像与交互式会话差异大，审计事件可附带 session 字节量便于事后区分。
3. **KeepAlive**：建议客户端 ssh 配置 `ServerAliveInterval`，避免经双层 WSS+NAT 后空闲被中间设备掐断。

## 5. 目标机端（新增 crate easyssh-agent）

- Rust + tokio，Windows/Linux 双平台（SSH 目标机 Linux 占比高，首版即双平台，比 EasyRDP 先行）。
- 配置 `agent.toml`：gateway url、resource_id、local_backend=`127.0.0.1:22`、token（enroll 换取后 DPAPI/文件权限 600 落盘）。
- systemd unit（Linux）+ Windows 服务（`sc create`）双形态；断线指数退避重连。

## 6. 资源注册与用户流程（自助、零人工、公网/内网统一）

### 6.0 现状缺口与本期新增范围

与 EasyRDP 同样的事实（两站资源管理现状一致）：资源为静态配置、gw-admin 无资源增删改、站长后台无用户资源管理、资源未落库。本期新增同款全链路（独立实现，不共用代码）：

- users.db 新表 `resources`（owner/mode/backend/status，自举建表）；
- issue.php 新 action：`register_resource`（direct 探测后落库 / agent 发 enrollment token）、`update_resource`（label/backend）、`delete_resource`（软删+清 grants）、`my_resources` 升级为合并静态+DB 并返回在线态；
- gw-admin.html：资源管理卡片（列表/登记/编辑/删除/enrollment 面板/待清理红标）；
- admin.html 站长后台：「用户资源管理」卡片（全量列表/搜索/删除/封禁/强制摘除 Agent）；
- easyssh-gateway：`POST /admin/resource` + 动态资源注册表。

### 6.1 用户登记流程（两形态）

1. **目标机在公网** → gw-admin「我的资源 → 登记资源」选直连模式，填 `host:port`（如 `203.0.113.9:2222`），网关注册前 3s 试连探测，成功即落库 + grants 自动写入（owner=邮箱）；
2. **目标机在内网（仅可出网）** → 选"内网（Agent 反向）"，网关生成一次性 enrollment token（10 分钟、绑定 resource_id），用户在目标机 `easyssh-agent --enroll <token>` 上线后资源翻「在线」，grants 同步写入；
3. 之后均为既有流程：gw-admin 签发 token → 客户端连。**用户侧两形态体验一致**，网关在隧道层自行选择主动连后端或走 Agent 通道，客户端与签发 UI 零感知 mode。

防滥用边界（同 EasyRDP 6.3）：每用户动态资源配额（默认 3 台，**仅限登记台数，token 签发不限次**）、direct 形态拒绝私网/环回地址（SSRF 白名单）、agent 形态后端固定 `127.0.0.1:22`、资源与 owner 绑定校验、agent 资源 7 天未上线标记待清理并邮件提醒。

**SSH 特有补充**：直连探测对 SSH 目标建议探测后顺手读 banner（前 64 字节含 `SSH-2.0`）再放行，避免把无关 TCP 服务误登记为 SSH 资源；Agent 形态 DIAL 目标固定 22，无此问题。

## 7. 分期与验收

| 期 | 内容 | 验收 |
|----|------|------|
| P1 | wire spec 落库（两仓各存一份）+ `agent.rs` + 单测（mpsc 假 Agent：DIAL/DATA/CLOSE/心跳判死） | `cargo test -p easyssh-gateway` 全过 |
| P2 | tunnel.rs agent 分支 + AsyncWrite 适配器 + 审计 | 回环 e2e 字节级一致 |
| P3 | easyssh-agent（Win+Linux）+ enroll + 服务化 | 真机：内网 Linux sshd + 云网关全链路 ssh/scp/sftp |
| P4 | ①users.db `resources` 表 + issue.php 资源增删改 action ②gw-admin 资源管理 UI ③admin.html 用户资源管理卡片 ④网关 `/admin/resource` + 动态注册表 | 用户全程自助完成两种资源登记并连通（ssh/scp/sftp），站长零介入；站长后台可查/删/封用户资源；审计可查 |

工作量估计：P1–P2 约 2 开发日，P3 约 2.5 日（双平台服务化），P4 约 3 日（资源管理全链路）。

## 8. 风险与备选

- **长会话被掐**：应用层心跳 + ssh ServerAlive 双保险；仍无法做到会话迁移（SSH 协议限制，如实告知）。
- **Linux 服务化差异**：systemd unit 模板 + 文档固化；Windows 侧复用 EasySSH 客户端既有服务注册经验。
- **备选否决**：同 EasyRDP——不引入 frp 等第三方组件，避免绕过鉴权/审计/策略矩阵。

## 9. 与 EasyRDP 的边界（F-R1-49 强化）

- 两仓各自实现 `agent.rs`/Agent crate，**不共享 crate、不共享代码文件**；
- 唯一共享物是 wire spec 文本（本方案第 3 节引用 EasyRDP 方案原文）；
- jwt_secret/admin_key/users.db/grants/policy 全部各自独立（现状不变）；
- 一侧协议演进需向另一侧发变更通知，wire spec 版本号 `agent-wire/1` 起版，破坏性变更升 `agent-wire/2` 并双侧同步发布。
