//! v5(M2)/F-M2-07：russh 集成测试地基（手册：依赖本机 sshd 的协议层回归测试）
//!
//! 门控：环境变量 `EASYSSH_TEST_SSH`（格式 `host:port/user/password`，如
//! `127.0.0.1:22/tester/your-password`）。未设置时所有真实连接测试自动跳过
//! （`#[ignore]` + 运行期检查双保险），CI 无环境不报错。
//!
//! 运行方式（本机已启用 OpenSSH Server 时）：
//!   $env:EASYSSH_TEST_SSH='127.0.0.1:22/tester/<密码>'
//!   cargo test -p easyssh --test ssh_integration_tests -- --ignored
//!
//! 本机启用 OpenSSH Server（Windows）见 docs/v5/集成测试.md。

use std::time::Duration;

/// 解析 EASYSSH_TEST_SSH → (host, port, user, password)；未设置/格式错返回 None
fn ssh_target() -> Option<(String, u16, String, String)> {
    let raw = std::env::var("EASYSSH_TEST_SSH").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (addr, rest) = raw.split_once('/')?;
    let (user, pass) = rest.split_once('/')?;
    let (host, port) = match addr.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(22)),
        None => (addr.to_string(), 22),
    };
    Some((host, port, user.to_string(), pass.to_string()))
}

/// 骨架自检：无环境时本套件安静跳过（始终通过的守卫测试，CI 可跑）
#[test]
fn t00_gate_without_env_is_noop() {
    match ssh_target() {
        Some(_) => println!("检测到 EASYSSH_TEST_SSH——请另以 --ignored 运行真实连接用例"),
        None => println!("未设置 EASYSSH_TEST_SSH——russh 集成测试跳过（骨架自检通过）"),
    }
}

/// 真实连接用例：连接 → exec `echo ok` → 断言输出
/// 需本机 sshd + EASYSSH_TEST_SSH；CI 默认跳过。
#[test]
#[ignore = "依赖本机 sshd 与 EASYSSH_TEST_SSH 环境变量"]
fn t01_connect_exec_echo_ok() {
    let Some((host, port, user, pass)) = ssh_target() else {
        eprintln!("EASYSSH_TEST_SSH 未设置——跳过（视为通过的门控占位）");
        return;
    };

    let config = russh::client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    };
    let connect_fut = russh::client::connect(
        std::sync::Arc::new(config),
        (host.as_str(), port),
        TestClient,
    );
    let mut session = tokio_block(connect_fut).expect("SSH 连接失败");

    let authed = tokio_block(session.authenticate_password(&user, &pass))
        .expect("密码认证失败");
    assert!(authed, "密码认证被拒绝（检查 EASYSSH_TEST_SSH 的用户/密码）");

    let mut channel = tokio_block(session.channel_open_session()).expect("打开会话通道失败");
    tokio_block(channel.exec(false, "echo ok")).expect("exec 失败");

    let mut buf: Vec<u8> = Vec::new();
    loop {
        match tokio_block(channel.wait()) {
            Some(russh::ChannelMsg::Data { ref data }) => buf.extend_from_slice(data),
            Some(russh::ChannelMsg::ExtendedData { ref data, .. }) => buf.extend_from_slice(data),
            Some(russh::ChannelMsg::ExitStatus { .. }) => {}
            Some(russh::ChannelMsg::Eof) | Some(russh::ChannelMsg::Close) | None => break,
            Some(_) => {}
        }
    }
    let out = String::from_utf8_lossy(&buf).trim().to_string();
    assert_eq!(out, "ok", "echo ok 输出不符");
    let _ = tokio_block(session.disconnect(russh::Disconnect::ByApplication, "test done", "en"));
}

/// 统一阻塞执行 async 步骤（测试内使用当前线程 runtime，避免全局 builder 冲突）
fn tokio_block<F: std::future::Future>(fut: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("构建 tokio runtime 失败")
        .block_on(fut)
}

/// 测试客户端：仅信任本机 sshd（测试环境不做主机键校验）
struct TestClient;

#[async_trait::async_trait]
impl russh::client::Handler for TestClient {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh_keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// 后续扩展点（手册 F-M2-07 验收：认证 3 种/转发 2 种/SFTP 8 项/会话事件，共 ≥15 条）：
//   1) 认证矩阵：公钥认证（authenticate_publickey）、keyboard-interactive
//      （authenticate_keyboard_interactive）、错误密码负路径（断言认证失败）
//   2) 端口转发：local forward（channel_open_direct_tcpip）、reverse forward
//      （tcpip_forward）——需 sshd AllowTcpForwarding yes
//   3) SFTP 8 项：open/list/mkdir/rename/remove/remove_dir/metadata/读写回环，
//      经 russh_sftp::client::SftpSession（需测试账号可写目录，如 $TEMP/essh-test）
//   4) 会话事件：多通道并发、通道关闭事件、resize 后输出
//   容器化：手册 W5 规划为本地 sshd 容器（如 linuxserver/openssh-server），
//   用例只需 EASYSSH_TEST_SSH 指向容器端口即可复用本骨架，无需改测试代码。
// ---------------------------------------------------------------------------
