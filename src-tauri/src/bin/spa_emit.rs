/* R-20261122-002 SPA 互通真机回归入口。
 * 用法: spa_emit <key> <hmac_key> <open_ports> <timeout>
 * 输出: fwknop 原生线格式 SPA 包（base64 单行）到 stdout。
 * WSL 回归：wsl 内 `spa_emit.exe ... | python3 send.py` 把 stdout 转发为
 * UDP 数据报发往 127.0.0.1:62201（Windows→WSL2 无 UDP localhost 转发）。
 * 经 #[path] 复用 src/ 下的 spa 模块（与 easyssh bin 同源编译，fko 静态链接
 * 由包级 build.rs 提供）。
 */
#[path = "../fko_ffi.rs"]
mod fko_ffi;
#[path = "../spa.rs"]
mod spa;

use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: spa_emit <key> <hmac_key> <open_ports> <timeout>");
        std::process::exit(2);
    }
    let packet = spa::build_packet(&a[1], &a[2], &a[3], a[4].parse().unwrap_or(30))
        .unwrap_or_else(|e| {
            eprintln!("build error: {e}");
            std::process::exit(1);
        });
    std::io::stdout().write_all(&packet).unwrap();
    println!();
}
