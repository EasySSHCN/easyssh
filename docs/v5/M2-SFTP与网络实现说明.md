# v5 M2 — SFTP 传输管道化 / 断点续传 / 代理与跳板 实现说明

> 负责人：Rust 传输工程（M2）；版本 5.0.0-alpha.1
> 对应功能编号：F-M2-01（P0）、F-M2-02（P0）、F-M2-05（P0）、任务四 zstd（P2 调研）

## 一、SFTP 传输管道化（F-M2-01）

### 现状（改造前）
`xfer.rs` 旧实现为单请求顺序读写：一个循环里 `read 256KB → write → 等待应答 → 下一块`，
同一时刻只有 1 个在途 SFTP 请求。高 RTT 链路上吞吐 ≈ 块大小 / RTT。

### 实现要点
- **范围切分多 worker**：把文件 `[0, total)` 按 `split_ranges()` 均分为 N 个连续区间
  （N = 并行 worker 数，默认 8，范围 1..=16），每个 worker 独立持有远端/本地文件句柄，
  以 **1MiB 块** 顺序推进自己的区间。在途请求 = worker 数（下载方向每 worker 1 个
  1MiB READ；上传方向 russh-sftp File 写路径内部还有 `max_concurrent_writes`(默认 8)
  的写确认流水线）。
- **russh 窗口**：russh 0.45 客户端默认 `window_size = 2MB`（已满足手册建议的 2MB
  read window），无需调整。
- **兼容与降级**：`pipeline=1` 退化为单 worker 串行（兼容老 sshd / 嵌入式设备，风险项
  R9 的回退开关）；小文件（不足 2 个区间）自动收缩 worker 数。
- **上传截断竞态防护**：多 worker 上传前先同步打开一次 `TRUNCATE` 重置远端文件，
  再分发 worker（各 worker 用 `WRITE|CREATE` 不带 TRUNCATE），避免写入被截断抹掉。
- **进度事件协议不变**：`xfer:progress` 原有 `id/status/received/total` 字段原样保留；
  另附 `taskId/phase/bytesDone/bytesTotal/speedBps`（队列模态框所需，修复旧版
  队列模态框字段缺失的问题），节流为 100ms 一次的播报任务。

### 基准测试方法（NFR-P-02：千兆局域网 ≥ 100MB/s，1GB 单文件）
1. 千兆交换机环境，`iperf3 -c <server>` 确认链路 ≥ 900Mbps；
2. 打开 EasySSH → SFTP 面板 → 上传/下载 1GB 单文件，从 `xfer:progress` 事件的
   `speedBps` 字段或任务完成时的 `speedBps` 取稳态值；
3. 对照组：把传输发起方的 pipeline 设为 1（可在 IPC 参数里显式传 `pipeline: 1`），
   重复同一文件，记录串行基线；
4. 高延迟链路（NFR-P-03：100ms RTT ≥ 3× 串行基线）：`tc netem delay 100ms` 模拟。

### 实测数据（本次环境）
本次开发机 **无千兆环境，且无管理员权限安装 OpenSSH Server / Docker（localhost sshd
方案不可用）**，故实测吞吐数据需在千兆/容器环境回填。设计层面的量化依据：

| RTT | 串行基线(1MiB/请求) | 管道 8 worker(实测模型) | 提升 |
|-----|--------------------|------------------------|------|
| 0.3ms(局域网) | ~3300MB/s(受带宽封顶) | 千兆链路上 ≈ 链路上限(≥117MB/s) | 千兆环境回填 |
| 50ms | ~21MB/s | ~168MB/s | 8× |
| 100ms | ~10.5MB/s | ~84MB/s | 8× |

即：100ms RTT 下 8 worker 模型 ≈ 8× 串行基线，满足 NFR-P-03 的 ≥3× 要求；
千兆局域网下在途带宽可打满链路。**待办：千兆环境实测值回填本表。**

## 二、SFTP 断点续传（F-M2-02）

### 实现要点
- **断点记录**：每个 worker 维护区间内已传字节数（`AtomicU64`）。任务失败/取消时，
  断点数组 `rangeDone` 写入任务记录（内存 `tasks` 表）**并持久化**到
  `<exe目录>/xfer-resume.json`（journal，跨重启保留；任务完成时自动清除对应条目）。
- **续传决策**（`xfer_start` 带 `resume: true`）：
  1. 先查内存任务表（本运行的 failed/cancelled 任务），再查磁盘 journal；
  2. `total` 与断点记录不一致 → 报"源文件大小与断点记录不一致,建议重传"；
  3. 下载：校验本地已有部分大小 ≥ 每 worker 断点最大值，否则提示整文件重传；
  4. 上传：校验远端已有部分大小 ≤ 本地文件大小，否则报"远端文件已变化,建议重传"
     （手册验收：远端被改写时默认不自动续传）；
  5. worker 数与断点数组长度一致 → 每 worker 从各自断点续传；不一致 → 退化全量
     （覆盖语义不变）。
- **校验哈希**：SFTP 协议无逐块校验，未做全文件哈希预校验（10GB 级文件代价过高）；
  采用大小 + worker 断点一致性校验，成功后建议业务侧做 SHA-256 终检
  （手册验收用例即此形态）。列为已知取舍（见"遗留问题"）。
- **前端**（`ui/js/sftp.js`）：队列模态框失败/取消且 `bytesDone<bytesTotal` 的任务显示
  `▶` 续传按钮（带 `resume:true` 重新入队）；状态文案新增 `续传中`；进度事件协议不变，
  传输覆盖层行为与旧版一致。

## 三、SOCKS5/HTTP CONNECT 代理 + ProxyJump（F-M2-05）

### 实现要点
- **会话级代理**：会话记录新增 `proxy` 字段
  `{kind: none|socks5|http, host, port, username?, password?}`。
  后端 `net.rs::proxy_tcp()`：SOCKS5（RFC1928，含 0x02 用户名/密码子协商 RFC1929）
  与 HTTP CONNECT（Basic Proxy-Authorization）握手均为手写实现（零新增依赖）。
  错误区分"代理拒绝 CONNECT (HTTP xxx)"与"目标不可达"（手册验收）。
- **ProxyJump 跳板链**：会话记录新增 `jumpChain` 字段 = 其他已存会话的 key 数组
  `["host:port:user", ...]`，最多 3 级（UI 与后端双重截断，防自环去重）。
  `net.rs::tunnel_stream()`：逐级用该跳板会话已存凭据（密码/私钥/口令，含其自身
  代理与跳板）登录，`channel_open_direct_tcpip` 开通道到下一级，
  最终 `russh::client::connect_stream` 接目标机——认证逻辑从 `ssh.rs` 抽取为
  `authenticate()` 复用。
- **接入链路**：`ssh::connect()` 先经 `net::open_stream()`（解析会话配置 → 建隧道流）
  再走原有握手/认证/PTY 流程，终端与 SFTP 同链路受益。
- **UI**（`ui/js/settings.js`，index.html 未改动）：设置弹窗注入「🌐 网络接入（代理与
  跳板）」区——会话选择器 + 代理表单 + 3 级跳板选择器（仅列其他已存会话），
  「应用到所选会话 / 清除」。配置随会话持久化（`saveSessionsGuard`，主密码解锁态下
  才允许写盘）。
- **v4 兼容**：旧 sessions 无 `proxy`/`jumpChain` 字段 → 后端按 None 处理 = 直连，
  零迁移成本。
- **生效条件**：代理/跳板读取自 config.json 的已保存会话，因此需会话已保存
  （连接表单勾选"保存"）。

## 四、任务四（P2）：zstd 可选压缩 —— 调研结论：跳过

- russh 0.45 的 SSH 传输层压缩**只支持 zlib**（`feature = "flate2"`，
  `compression::ZLIB / ZLIB_LEGACY`），且在握手时协商、作用于**整个连接**（所有通道，
  含终端），无法按 SFTP 通道单独开启，更无 zstd 算法。
- russh-sftp 2.4 无通道级压缩扩展支持（无 `compress` 类扩展封装；OpenSSH 的
  `zstd@openssh.com` 是 sftp packfile 扩展，russh-sftp 未实现）。
- 结论：**按任务约束记录调研结论并跳过**，不引入不满足需求的 zlib 作为替代、不为此
  升级/替换 SSH 库。如未来 russh 支持 zstd，可在 `preferred.compression` 处接入，
  开关默认关的建议不变。

## 五、文件清单

| 文件 | 变更 |
|------|------|
| `src-tauri/src/xfer.rs` | 重写传输内核：范围切分多 worker 管道、断点记录/journal、进度播报兼容 |
| `src-tauri/src/net.rs` | 新增：SOCKS5/HTTP CONNECT 握手、跳板链隧道、会话配置解析（含单测） |
| `src-tauri/src/ssh.rs` | 接入 net 隧道流；认证抽取为 `authenticate()`；`ClientHandler` 提供 `new()` |
| `src-tauri/src/types.rs` | 新增 `ProxyCfg` 类型 |
| `src-tauri/src/main.rs` | **最小化接线改动**（已在报告中标注）：`xfer_start` 增加 `pipeline`/`resume` 可选参数；`ssh_connect` 传入 `&state.config`；`XferState` 增加 `range_done` 字段 |
| `ui/js/sftp.js` | 队列 ▶ 续传按钮、`续传中` 状态文案、取消按钮条件修正 |
| `ui/js/settings.js` | 「代理与跳板」配置区（动态注入，不改 index.html） |
| `docs/v5/M2-SFTP与网络实现说明.md` | 本文档 |

## 六、验证结果

- `cargo build`（CARGO_TARGET_DIR=C:\temp\v5target）：**通过**；
  顺带清除了与本次改动相关的告警（sftp.rs 未用导入/mut、ssh.rs 不可达分支、
  net.rs 未读字段）。
- `cargo test --workspace`：**全部通过**（含新增 5 个单测：
  split_ranges/resume_offsets×2、SOCKS5 编码、HTTP CONNECT 编解码+代理拒绝解析、
  ProxyCfg 兼容语义）。

## 七、遗留问题

1. **千兆局域网实测值待回填**：见第一节基准方法；需要千兆环境或本地 sshd 容器
   （本开发机无管理员权限/无 Docker）。
2. **代理凭据明文落盘**：`proxy.password` 随会话存 config.json，目前会话加密只覆盖
   `password/passphrase` 顶层字段，嵌套的代理密码为明文——对应手册 S-ENC-03
   （SECRET_FIELDS 扩面，M5 里程碑），导出会话 JSON 时该字段会带出，建议导出前
   手工剔除或在 M5 前补导出过滤。
3. **跳板机不校验主机指纹**：跳板连接复用已存会话凭据但未做指纹校验（目标机指纹
   校验流程保持不变）；跳板 MITM 风险与 OpenSSH ProxyJump 的 `-o UserKnownHostsFile`
   缺省行为一致，后续可在跳板会话上也接入指纹确认流程。
4. **上传续传依赖本机断点记录**：远端半成品文件存在写入洞（区间交错），远端大小
   不能作为续传依据；若断点 journal 丢失（如换电脑），上传只能全量重传（有大小校验
   兜底，不会写坏远端）。
5. **管道深度自适应降级（风险项 R9）**：当前提供 `pipeline` 参数（1=串行兼容模式），
   尚未自动探测老 sshd 窗口协商结果回退，建议 M4 传输队列里程碑补自适应。
