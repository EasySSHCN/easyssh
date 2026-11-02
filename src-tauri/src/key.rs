/* EasySSH v4：本机密钥对生成 / 公钥安装 / 失败诊断（对应 3.x key:* 命令）
 * - generate：Ed25519 密钥对写入 <baseDir>/keys/id_ed25519_easyssh(.pub)，幂等复用
 *   （私钥格式 PKCS#8 PEM——v4 russh 直接支持；3.x 因 ssh2 不解析 PKCS#8 才用 OpenSSH 格式）
 * - install：在已连会话上 exec 安装脚本（自动适配 sshd AuthorizedKeysFile、
 *   修正家目录/.ssh 权限、SELinux restorecon、幂等写入、回读校验、PubkeyAuthentication 检查）
 * - diagnose：收集服务器端 sshd 配置/权限/SELinux/authorized_keys/日志拒绝记录 */
use std::path::PathBuf;

use russh::ChannelMsg;
use base64::Engine as _;
use russh_keys::key::KeyPair;

use crate::ssh::SessionMap;

/// 在会话上执行命令并收集完整输出（stdout+stderr）
async fn exec_collect(
    sessions: &SessionMap,
    session_id: u64,
    cmd: &str,
) -> Result<String, String> {
    let sess = sessions
        .0
        .lock()
        .get(&session_id)
        .cloned()
        .ok_or_else(|| "无活动连接".to_string())?;
    let mut chan = sess
        .handle
        .lock()
        .await
        .channel_open_session()
        .await
        .map_err(|e| format!("打开通道失败: {e}"))?;
    chan.exec(false, cmd.to_string())
        .await
        .map_err(|e| format!("exec 失败: {e}"))?;
    let mut out = Vec::new();
    loop {
        match chan.wait().await {
            Some(ChannelMsg::Data { ref data }) => out.extend_from_slice(data),
            Some(ChannelMsg::ExtendedData { ref data, .. }) => out.extend_from_slice(data),
            Some(_) => {}
            None => break,
        }
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 生成（或复用）本机 Ed25519 密钥对
pub fn generate(base_dir: &PathBuf) -> Result<serde_json::Value, String> {
    let dir = base_dir.join("keys");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 keys 目录失败: {e}"))?;
    let priv_path = dir.join("id_ed25519_easyssh");
    let pub_path = dir.join("id_ed25519_easyssh.pub");

    if priv_path.exists() && pub_path.exists() {
        let pub_line = std::fs::read_to_string(&pub_path)
            .unwrap_or_default()
            .trim()
            .to_string();
        if pub_line.starts_with("ssh-ed25519 ") {
            return Ok(serde_json::json!({
                "privPath": priv_path.to_string_lossy(),
                "pubPath": pub_path.to_string_lossy(),
                "pubLine": pub_line,
                "reused": true,
            }));
        }
    }

    let kp_full = KeyPair::generate_ed25519().ok_or("生成密钥失败")?;
    let kp = kp_full
        .clone_public_key()
        .map_err(|e| format!("生成密钥失败: {e}"))?;
    let wire = russh_keys::PublicKeyBase64::public_key_bytes(&kp);
    let blob = {
        // ssh 公钥 wire 格式：[len]["ssh-ed25519"][len][32B 原始公钥]
        let t = b"ssh-ed25519";
        let mut v = Vec::with_capacity(4 + t.len() + 4 + wire.len());
        v.extend_from_slice(&(t.len() as u32).to_be_bytes());
        v.extend_from_slice(t);
        v.extend_from_slice(&(wire.len() as u32).to_be_bytes());
        v.extend_from_slice(&wire);
        v
    };
    let hostname = hostname();
    let date = chrono_date();
    let comment = format!("easyssh-{hostname}-{date}");
    let pub_line = format!(
        "ssh-ed25519 {} {}",
        base64::engine::general_purpose::STANDARD.encode(&blob),
        comment
    );

    // 私钥写 PKCS#8 PEM（russh load_secret_key 原生支持）
    let mut pem = Vec::new();
    russh_keys::encode_pkcs8_pem(&kp_full, &mut pem).map_err(|e| format!("编码私钥失败: {e}"))?;
    std::fs::write(&priv_path, &pem).map_err(|e| format!("写入私钥失败: {e}"))?;
    std::fs::write(&pub_path, format!("{pub_line}\n")).map_err(|e| format!("写入公钥失败: {e}"))?;

    Ok(serde_json::json!({
        "privPath": priv_path.to_string_lossy(),
        "pubPath": pub_path.to_string_lossy(),
        "pubLine": pub_line,
    }))
}



fn hostname() -> String {
    hostname_val().unwrap_or_else(|| "localhost".into())
}
#[cfg(windows)]
fn hostname_val() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}
#[cfg(not(windows))]
fn hostname_val() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname").ok().map(|s| s.trim().to_string())
}
/// 公开：UNIX 秒 → ISO 8601 (UTC, 秒级)
pub fn iso_from_unix(secs: u64) -> String {
    let (y, m, d) = civil_from_days((secs / 86400) as i64);
    let rem = secs % 86400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn chrono_date() -> String {
    // 无 chrono 依赖：使用 UNIX 时间生成日期（UTC）
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    // 2026-01-01 起 2069 天内无需精确闰年处理：直接用简化民用算法
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 公钥安装（幂等 + 权限/SELinux 修正 + 回读校验）
pub async fn install(
    sessions: &SessionMap,
    session_id: u64,
    pub_line: &str,
) -> Result<String, String> {
    let q = shell_quote(pub_line);
    let cmd = concat!(
        "AK=$(sshd -T 2>/dev/null | awk 'tolower($1)==\"authorizedkeysfile\"{print $2; exit}');",
        " case \"$AK\" in \"\"|\".ssh/authorized_keys\") F=\"$HOME/.ssh/authorized_keys\";;",
        " *) F=$(echo \"$AK\" | sed -e \"s/%u/$(id -un 2>/dev/null)/\" -e \"s|%h|$HOME|\");; esac;",
        " mkdir -p \"$(dirname \"$F\")\"",
        " && case \"$F\" in \"$HOME\"/*) chmod 700 \"$(dirname \"$F\")\";; esac;",
        " chmod g-w,o-w \"$HOME\" 2>/dev/null;",
        " touch \"$F\" && chmod 600 \"$F\";",
        " (command -v restorecon >/dev/null 2>&1 && restorecon -R \"$(dirname \"$F\")\" \"$HOME\" 2>/dev/null || true);",
        " (grep -qF '", "' \"$F\" 2>/dev/null || echo '", "' >> \"$F\");",
        " echo EASYSSH_OK:$(grep -cF '", "' \"$F\" 2>/dev/null);",
        " echo EASYSSH_FILE:$F;",
        " (sshd -T 2>/dev/null | grep -iE \"^(pubkeyauthentication|strictmodes|authorizedkeysfile|permitrootlogin)\" || true)",
    )
    .replace("', '", &format!("', '{}', '", pub_line.replace('\'', "'\\''")));
    let out = exec_collect(sessions, session_id, &cmd).await?;
    if !out.contains("EASYSSH_OK:") || out.contains("EASYSSH_OK:0") {
        return Err(format!(
            "公钥写入后回读校验失败(公钥不在服务器 authorized_keys 中){}",
            if out.trim().is_empty() { String::new() } else { format!("\n{}", out.trim()) }
        ));
    }
    let file = out
        .lines()
        .find(|l| l.starts_with("EASYSSH_FILE:"))
        .map(|l| l["EASYSSH_FILE:".len()..].trim().to_string())
        .unwrap_or_else(|| "~/.ssh/authorized_keys".into());
    let pk_no = out
        .lines()
        .find(|l| l.to_lowercase().starts_with("pubkeyauthentication "))
        .map(|l| l.split_whitespace().last().unwrap_or("").to_string());
    if let Some(v) = pk_no {
        if v.eq_ignore_ascii_case("no") {
            return Err(format!(
                "服务器 sshd 已关闭公钥认证(PubkeyAuthentication no),公钥已写入 {file} 但无法生效。\n请在服务器上修改 /etc/ssh/sshd_config:把「PubkeyAuthentication」设为 yes(或删除该行),再执行 systemctl restart sshd"
            ));
        }
    }
    Ok(format!(
        "公钥已写入服务器 {file}(按 sshd 实际 AuthorizedKeysFile 配置,家目录权限/SELinux 上下文已自动修正,回读校验通过)"
    ))
}

/// 密钥登录失败自动诊断（收集服务器端信息，返回原始诊断文本）
pub async fn diagnose(
    sessions: &SessionMap,
    base_dir: &PathBuf,
    session_id: u64,
    pub_line: &str,
) -> Result<String, String> {
    let mut pub_line = pub_line.to_string();
    if pub_line.is_empty() {
        let p = base_dir.join("keys").join("id_ed25519_easyssh.pub");
        pub_line = std::fs::read_to_string(&p).unwrap_or_default().trim().to_string();
    }
    let q = shell_quote(&pub_line);
    let cmd = [
        "echo \"@@WHO\"; id -un 2>/dev/null; echo \"home=$HOME\"".to_string(),
        "AK=$(sshd -T 2>/dev/null | awk 'tolower($1)==\"authorizedkeysfile\"{print $2; exit}'); case \"$AK\" in \"\"|\".ssh/authorized_keys\") F=\"$HOME/.ssh/authorized_keys\";; *) F=$(echo \"$AK\" | sed -e \"s/%u/$(id -un 2>/dev/null)/\" -e \"s|%h|$HOME|\");; esac; echo \"akfile=$F\"".to_string(),
        "echo \"@@PERMS\"".to_string(),
        "ls -ld \"$HOME\" \"$(dirname \"$F\")\" \"$F\" 2>&1".to_string(),
        "echo \"@@SELINUX\"".to_string(),
        "(command -v getenforce >/dev/null 2>&1 && getenforce; ls -Zd \"$(dirname \"$F\")\" 2>/dev/null) || echo no-selinux".to_string(),
        "echo \"@@SSHD_CONFIG\"".to_string(),
        "if command -v sshd >/dev/null 2>&1; then sshd -T 2>/dev/null | grep -iE \"^(pubkeyauthentication|strictmodes|authorizedkeysfile|permitrootlogin|authenticationmethods|pubkeyacceptedalgorithms|allowusers|denyusers)\"; else echo \"sshd-binary-not-found\"; fi".to_string(),
        "command -v dropbear >/dev/null 2>&1 && echo \"dropbear-present\"".to_string(),
        "echo \"@@KEY_PRESENT\"".to_string(),
        format!("grep -cF {q} \"$F\" 2>/dev/null || echo 0"),
        "echo \"@@AUTHKEYS_TAIL\"".to_string(),
        "tail -5 \"$F\" 2>/dev/null | cut -c1-80".to_string(),
        "echo \"@@AUTHKEYS_FP\"".to_string(),
        "ssh-keygen -lf \"$F\" 2>/dev/null | tail -5 || echo fingerprint-tool-unavailable".to_string(),
        "echo \"@@SSHD_LOG\"".to_string(),
        "(tail -200 /var/log/secure 2>/dev/null; tail -200 /var/log/auth.log 2>/dev/null; tail -200 /var/log/messages 2>/dev/null; journalctl -u sshd -u ssh -n 200 --no-pager 2>/dev/null) | grep -iE \"sshd|dropbear\" | grep -iE \"publickey|refused|denied|error|bad ownership\" | grep -viE \"accepted\" | tail -12".to_string(),
        "echo \"@@END\"".to_string(),
    ]
    .join("; ");
    exec_collect(sessions, session_id, &cmd).await
}
