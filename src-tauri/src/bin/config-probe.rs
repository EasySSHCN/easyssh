//! 临时探针:直接用 easyssh-core ConfigStore 加载指定目录的 config.json,打印诊断(排查后删除)
use easyssh_core::config::ConfigStore;

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    println!("probe dir: {}", dir);
    let store = ConfigStore::new(std::path::PathBuf::from(&dir));
    match store.load() {
        Ok(cfg) => {
            println!("load OK");
            if let Some(arr) = cfg.sessions.as_array() {
                println!("sessions.len = {}", arr.len());
                for s in arr {
                    let host = s.get("host").and_then(|x| x.as_str()).unwrap_or("");
                    let name = s.get("name").and_then(|x| x.as_str()).unwrap_or("(无名)");
                    println!("  - {} {}", name, host);
                }
            } else {
                println!("sessions 不是数组!");
            }
            println!("settings 类型: {}", if cfg.settings.is_object() { "object" } else { "other" });
        }
        Err(e) => println!("load ERR: {}", e),
    }
}
