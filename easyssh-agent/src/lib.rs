//! easyssh-agent — EasySSH 目标机 Agent（方案-目标机Agent反向注册.md 第 5 节，P3）。
//!
//! 与 EasyRDP 侧及 easyssh-gateway 各自独立实现，唯一共享物是 agent-wire/1
//! spec 文本（帧头/指令表/心跳语义逐字一致），代码互不引用（F-R1-49 口径）。
//!
//! 模块划分：
//! - [`wire`]    agent-wire/1 编解码（Envelope / cut_envelope）
//! - [`config`]  agent.toml 解析
//! - [`store`]   AgentToken 落盘（Linux 0600 文件 / Windows DPAPI）
//! - [`backoff`] 断线指数退避（封顶 30s）
//! - [`agent`]   连接主循环：握手、心跳、DIAL→本地 TCP、双向转发、重连
//! - [`enroll`]  `--enroll` 一次性 enrollment token 换 AgentToken
//! - [`winsvc`]  Windows 服务形态（SCM 调度，仅 windows 目标编译）

pub mod agent;
pub mod backoff;
pub mod config;
pub mod enroll;
pub mod store;
pub mod wire;

#[cfg(windows)]
pub mod winsvc;
