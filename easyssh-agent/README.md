# easyssh-agent — EasySSH 目标机 Agent

Rust + tokio 实现的目标机反向隧道 Agent（方案：`EasySSH/docs/方案-目标机Agent反向注册.md`，
P3 交付，Windows/Linux 双平台首版）。与 easyssh-gateway / EasyRDP 各自独立实现，
唯一共享物是 **agent-wire/1** wire spec 文本（帧头 `[type:1][session_id:8 BE][len:4 BE]`、
指令 DIAL/OPEN_OK/OPEN_ERR/DATA/CLOSE/PING/PONG、心跳 30s × 3 次判死、单连接
session ≤ 8），代码互不引用（F-R1-49 口径）。

## 工作原理

```
EasySSH客户端 ⇄ 网关 /tunnel/{rid} ⇄ /agent/connect(WSS) ⇄ easyssh-agent ⇄ 本地 sshd(127.0.0.1:22)
```

- Agent 主动外连网关 `GET /agent/connect?resource_id=<rid>`（Bearer AgentToken，WSS）；
- 网关下发 `DIAL{"backend":"127.0.0.1:22"}` → Agent 白名单校验后本地 TCP 连 sshd
  → `OPEN_OK`/`OPEN_ERR` → 双向 DATA/CLOSE 字节转发；
- 双向心跳：网关每 30s 发 PING，Agent 回 PONG；Agent 也主动发 PING(0) 保 NAT，
  静默 3×30s 无入站即判死断开；
- 断线指数退避重连：1s → 2s → 4s → … 封顶 30s；连接成功后复位。

## 安装与运行

### 1. 配置 `agent.toml`（可执行文件同目录，或 `--config` 指定）

见 `deploy/agent.example.toml`。必填 `gateway_url`、`resource_id`；
`local_backend` 默认 `127.0.0.1:22`（Agent 不接受配置外目标）。

### 2. enroll：一次性 enrollment token 换 AgentToken

```bash
easyssh-agent --enroll <ENROLLMENT_TOKEN> [--config /etc/easyssh-agent/agent.toml]
```

Agent 向 `{gateway}/agent/enroll` POST `{"enrollment_token":"..."}`，网关返回
`{"agent_token": "...", "resource_id": "..."}`（enrollment token 一次性、10 分钟有效、
绑定 resource_id）。换取后立即落盘：

- **Linux**：`~/.config/easyssh-agent/agent-token`，权限强制 `0600`（目录 0700）；
- **Windows**：`%APPDATA%\EasySSHAgent\agent-token.bin`，DPAPI（当前用户作用域）加密。

### 3. Linux：systemd 服务

```bash
sudo cp target/release/easyssh-agent /usr/local/bin/
sudo mkdir -p /etc/easyssh-agent
sudo cp deploy/agent.example.toml /etc/easyssh-agent/agent.toml
sudo easyssh-agent --config /etc/easyssh-agent/agent.toml --enroll <TOKEN>
sudo cp deploy/easyssh-agent.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now easyssh-agent
journalctl -u easyssh-agent -f
```

### 4. Windows：服务形态（SCM）

以管理员身份：

```bat
sc create EasySSHAgent binPath= "C:\Program Files\EasySSHAgent\easyssh-agent.exe --service" start= auto
sc description EasySSHAgent "EasySSH 目标机反向隧道 Agent"
sc start EasySSHAgent
sc stop EasySSHAgent    & rem 停止
sc delete EasySSHAgent  & rem 卸载
```

`--service` 由 SCM 调度（StartServiceCtrlDispatcher），STOP 控制码触发优雅退出。
注意 `binPath= ` 等号后必须有空格。首次仍需在**同用户会话**先 enroll 一次
（DPAPI 绑定解密用户；服务以 LocalSystem 跑时请让服务账户 enroll，
可用 `psexec -s easyssh-agent --enroll ...` 或改配 `token_path` 共享路径 + 
服务账户权限）。

### 5. 控制台调试模式

```bash
easyssh-agent            # 前台运行，Ctrl+C 优雅退出
RUST_LOG=debug easyssh-agent --console
```

## 测试

```bash
cd EasySSH
cargo test -p easyssh-agent     # wire 编解码 / DIAL 白名单与上限 / 假网关内存回环 /
                                # 回环 TcpListener 双向字节转发 / 退避序列 / DPAPI / 配置解析
```

## 注意事项（SSH 长会话，务必阅读）

1. **Agent 断线 = 会话中断**。SSH 无会话迁移能力，Agent 掉线即掐断所有进行中的
   ssh/scp/sftp 会话且无法恢复。心跳（30s×3）与退避（≤30s）只能缩短不可用窗口。
2. **客户端侧 KeepAlive 双保险**：建议 ssh 客户端配置
   `ServerAliveInterval 30` / `ServerAliveCountMax 3`，避免经双层 WSS+NAT 后
   空闲连接被中间设备静默掐断（Agent 侧心跳只覆盖 Agent↔网关段）。
3. scp/sftp/端口转发均为 TCP 字节流，帧协议透明，无需特判。
4. **TLS**：默认走 webpki 公共根；自签网关联调设 `accept_invalid_certs = true`
   （仍加密，仅跳过证书链校验），生产建议网关换受信证书后关闭。
5. enroll 端点（`POST /agent/enroll`）属网关侧 P4 范围；本版 agent 已按契约实现，
   网关落地前可用 `--console` + 手工放置 token 文件联调。
6. Windows DPAPI 落盘绑定加密用户；服务形态以 LocalSystem 运行时需用该账户
   enroll（见上）。
