//! CLI 入口：--enroll / --console / --service（Windows）三种形态。

use easyssh_agent::{agent, config, enroll, store};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut config_path: Option<std::path::PathBuf> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut mode: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" => {
                i += 1;
                config_path = args.get(i).map(std::path::PathBuf::from);
            }
            "--enroll" => mode = Some("enroll".into()),
            "--console" | "--debug" => mode = Some("console".into()),
            "--service" => mode = Some("service".into()),
            "--help" | "-h" => mode = Some("help".into()),
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg_path = config_path.unwrap_or_else(config::default_config_path);
    let cfg = match config::AgentConfig::load_from_path(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("错误：{e}\n（示例见 deploy/agent.example.toml；--config 可指定路径）");
            std::process::exit(2);
        }
    };

    match mode.as_deref() {
        Some("help") => {
            print_help();
        }
        Some("enroll") => {
            let token = positional.first().cloned().unwrap_or_else(|| {
                eprintln!("用法：easyssh-agent --enroll <ENROLLMENT_TOKEN> [--config agent.toml]");
                std::process::exit(2);
            });
            match enroll::enroll_and_store(&cfg, &token) {
                Ok(path) => {
                    println!("enroll 成功，AgentToken 已落盘：{}", path.display());
                    println!("可启动服务：easyssh-agent --console（前台调试）或系统服务。");
                }
                Err(e) => {
                    eprintln!("enroll 失败：{e}");
                    std::process::exit(1);
                }
            }
        }
        Some("service") => {
            // Windows 服务形态：必须由 SCM 启动；Linux 下该开关无意义，提示后按控制台运行。
            #[cfg(windows)]
            {
                if easyssh_agent::winsvc::run_as_windows_service() {
                    return;
                }
                std::process::exit(1);
            }
            #[cfg(not(windows))]
            {
                eprintln!("--service 仅 Windows 形态有效；Linux 请使用 systemd unit（见 deploy/easyssh-agent.service）");
                std::process::exit(2);
            }
        }
        // 默认与 --console 相同：前台调试模式
        _ => run_console(cfg),
    }
}

/// 控台调试模式：加载 token → 前台主循环（Ctrl+C 优雅退出）。
fn run_console(cfg: config::AgentConfig) {
    let token = match store::load_token(cfg.token_path.as_deref()) {
        Ok(Some(t)) => t,
        Ok(None) => {
            eprintln!(
                "错误：尚未 enroll（token 文件 {} 不存在）。先运行 easyssh-agent --enroll <TOKEN>",
                cfg.token_path().display()
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("错误：token 读取失败：{e}");
            std::process::exit(1);
        }
    };
    tracing::info!(
        gateway = %cfg.gateway_url,
        resource_id = %cfg.resource_id,
        local_backend = %cfg.local_backend,
        "easyssh-agent 控制台模式启动"
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(async move {
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("收到 Ctrl+C，退出");
                let _ = shutdown_tx.send(true);
            });
            agent::run_agent(cfg, token, shutdown_rx).await;
        });
}

fn print_help() {
    println!(
        "easyssh-agent — EasySSH 目标机 Agent（agent-wire/1）\n\
         \n\
         用法:\n\
           easyssh-agent                       前台运行（等同 --console）\n\
           easyssh-agent --console             前台调试模式\n\
           easyssh-agent --enroll <TOKEN>      一次性 enrollment token 换 AgentToken 并落盘\n\
           easyssh-agent --service             以 Windows 服务形态运行（须由 SCM 启动）\n\
           easyssh-agent --config <PATH>       指定 agent.toml 路径\n\
         \n\
         配置: agent.toml（gateway_url / resource_id 必填；示例见 deploy/agent.example.toml）\n\
         日志: RUST_LOG=debug 可提级（默认 info）"
    );
}
