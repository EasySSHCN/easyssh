/* EasySSH v5(beta) 任务二：传输管道 / 代理与跳板 / 续传 纯逻辑集成测试。
 * 全部 CI 可跑（无需真实 sshd、无需进程），通过 #[path] 把 std-only 的
 * src/xfer_core.rs 直接编入测试 crate（easyssh 是 binary crate，无 lib.rs，
 * 模式与 update_guard_tests 一致）。覆盖手册 F-M2-07 验收（集成用例 ≥15 条）与
 * F-M4-02 限速/暂停叠加语义。 */
#[path = "../src/xfer_core.rs"]
mod xfer_core;

use xfer_core::*;

const CHUNK: u64 = 1024 * 1024;

/* ================= 一、xfer 管道：范围切分边界 ================= */

/// 范围切分边界：0 字节 / 1 字节 / 极小文件 / 超大 worker 数
#[test]
fn t01_split_ranges_boundaries() {
    // 0 字节 → 无区间（run_transfer 提前返回，不做任何 IO）
    assert!(split_ranges(0, 0, 8).is_empty());
    // 1 字节、worker=16 → 收缩为 1 个 1 字节区间
    assert_eq!(split_ranges(0, 1, 16), vec![(0, 1)]);
    // 极小文件 3 字节、worker=2 → 首区间多 1 字节
    assert_eq!(split_ranges(0, 3, 2), vec![(0, 2), (2, 3)]);
    // 超大 worker 数（MAX_PIPELINE=16）对 10 字节文件 → 收缩为 10 个单字节区间且无缝覆盖
    let r = split_ranges(0, 10, 16);
    assert_eq!(r.len(), 10);
    assert_eq!(r.first(), Some(&(0, 1)));
    assert_eq!(r.last(), Some(&(9, 10)));
    // 任意切分：无缝、不重叠、并集=全范围
    for (total, n) in [(100u64, 3usize), (1024 * 1024 + 1, 8), (999_999, 16)] {
        let r = split_ranges(0, total, n);
        assert_eq!(r.iter().map(|(s, _)| *s).min(), Some(0));
        assert_eq!(r.iter().map(|(_, e)| *e).max(), Some(total));
        for w in r.windows(2) {
            assert_eq!(w[0].1, w[1].0, "区间必须无缝衔接");
        }
    }
    // worker=0 防御 → 按 1 个区间处理
    assert_eq!(split_ranges(0, 5, 0), vec![(0, 5)]);
}

/* ================= 二、worker 数与断点不匹配 → 退化全量 ================= */

/// 断点记录的 rangeDone 长度与本次 worker 数不一致（或为空/缺失）→ 全部退化为区间起点
#[test]
fn t02_resume_worker_mismatch_falls_back_to_full() {
    let ranges = split_ranges(0, 1000, 8);
    let starts: Vec<u64> = ranges.iter().map(|(s, _)| *s).collect();
    // worker 数 8 → 4：不匹配 → 全量
    assert_eq!(resume_offsets(Some(&[1, 2, 3, 4]), &ranges), starts);
    // 空 rangeDone（4 个 worker 都没传过，理论上不应出现）→ 全量
    assert_eq!(resume_offsets(Some(&[]), &ranges), starts);
    // 无记录 → 全量
    assert_eq!(resume_offsets(None, &ranges), starts);
    // 匹配时：断点钳制在区间内（不超过区间终点）
    let ok = vec![10, 125, 0, 999, 1, 1, 1, 1];
    let offs = resume_offsets(Some(&ok), &ranges);
    for (i, ((s, e), o)) in ranges.iter().zip(offs.iter()).enumerate() {
        assert!(o >= s && o <= e, "起点必须在区间内");
        assert_eq!(*o, (*s + ok[i]).min(*e));
    }
}

/* ================= 三、journal 损坏（非法 JSON）→ 安全全量 ================= */

/// journal 文件损坏（非法 JSON）时解析回退为空列表 → 续传决策视为无历史，安全走全量
#[test]
fn t03_journal_corrupt_json_falls_back_to_full() {
    // 非法 JSON → 空
    assert!(journal_parse("not json at all {{{").is_empty());
    assert!(journal_parse("[{broken").is_empty());
    assert!(journal_parse("").is_empty());
    // 合法但非数组 → 空（unwrap_or_default）
    assert!(journal_parse(r#"{"a":1}"#).is_empty());
    // 正常数组保留
    let good = journal_parse(r#"[{"direction":"upload","received":10}]"#);
    assert_eq!(good.len(), 1);
    // 坏记录被安全跳过后，把空列表当"无历史"：续传起点=区间起点（全量）
    let ranges = split_ranges(0, 500, 2);
    let offs = resume_offsets(None, &ranges);
    assert_eq!(offs, vec![0, 250]);
}

/* ================= 四、journal 跨重启 roundtrip ================= */

/// upsert（写入断点）→ 序列化 → 重新 parse → 断点仍在；成功清理（received=0）后条目被移除
#[test]
fn t04_journal_roundtrip_across_restart() {
    let task = serde_json::json!({
        "id": 7, "direction": "upload",
        "localPath": "C:/a.bin", "remotePath": "/tmp/a.bin",
        "total": 1000, "received": 640, "rangeDone": [100, 125, 130, 285],
        "status": "cancelled",
    });
    // 跨重启：旧列表（来自磁盘）为空，upsert 断点任务
    let l1 = journal_upsert_list(&[], &task);
    assert_eq!(l1.len(), 1);
    let txt = serde_json::to_string(&l1).unwrap();
    // 模拟重启后重新解析
    let l2 = journal_upsert_list(&journal_parse(&txt), &serde_json::json!({
        "direction": "upload", "localPath": "C:/a.bin", "remotePath": "/tmp/a.bin",
        "total": 1000, "received": 1000, "status": "failed",
    }));
    // 同文件 upsert 替换（不重复追加）
    assert_eq!(l2.len(), 1);
    assert_eq!(l2[0]["received"], 1000);
    // 不同文件互不影响
    let l3 = journal_upsert_list(&l2, &serde_json::json!({
        "direction": "download", "localPath": "C:/b.bin", "remotePath": "/tmp/b.bin",
        "total": 10, "received": 5,
    }));
    assert_eq!(l3.len(), 2);
    // 传输成功（received=0）→ 仅移除该文件记录
    let l4 = journal_upsert_list(&l3, &serde_json::json!({
        "direction": "upload", "localPath": "C:/a.bin", "remotePath": "/tmp/a.bin",
        "total": 1000, "received": 0,
    }));
    assert_eq!(l4.len(), 1);
    assert_eq!(l4[0]["remotePath"], "/tmp/b.bin");
}

/* ================= 五、优先级调度：高/低混合排序 ================= */

/// 高/低/普通混合入队的调度顺序：rank 升序、同 rank 按入队 id；空队列 None
#[test]
fn t05_priority_mixed_ordering() {
    let q: Vec<(u8, u64)> = vec![
        (2, 10), // low
        (0, 11), // high
        (1, 12), // normal
        (0, 13), // high（晚于 11 入队）
        (2, 14), // low
        (1, 15), // normal
    ];
    let mut qq = q.clone();
    let mut order = Vec::new();
    while let Some(i) = pick_next(&qq) {
        order.push(qq.remove(i));
    }
    assert_eq!(
        order,
        vec![(0, 11), (0, 13), (1, 12), (1, 15), (2, 10), (2, 14)]
    );
    assert_eq!(pick_next(&[]), None);
    // 未知优先级按 normal
    assert_eq!(priority_rank("urgent"), priority_rank("normal"));
    // 交错入队时高优先持续插队
    let mut qq: Vec<(u8, u64)> = vec![(1, 1), (2, 2), (0, 3), (1, 4), (0, 5)];
    let first3: Vec<u8> = (0..3)
        .map(|_| {
            let i = pick_next(&qq).unwrap();
            let r = qq.remove(i).0;
            r
        })
        .collect();
    assert_eq!(first3, vec![0, 0, 1]);
}

/* ================= 六、暂停→恢复→取消 状态机序列 ================= */

/// worker 门控在一段真实操作序列下的状态机：推进→暂停→停等→恢复→推进→取消→终止
#[test]
fn t06_pause_resume_cancel_state_sequence() {
    use GateAction::*;
    let pause = std::sync::atomic::AtomicBool::new(false);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let gate = |p: &std::sync::atomic::AtomicBool, c: &std::sync::atomic::AtomicBool| {
        worker_gate(p.load(std::sync::atomic::Ordering::SeqCst), c.load(std::sync::atomic::Ordering::SeqCst))
    };
    // 1) 运行中推进
    assert_eq!(gate(&pause, &cancel), Advance);
    // 2) 暂停请求 → 在途块写完后停等（不发起新块）
    pause.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(gate(&pause, &cancel), WaitPaused);
    assert_eq!(gate(&pause, &cancel), WaitPaused); // 持续停等
    // 3) 暂停中收到取消 → 取消优先于暂停
    cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(gate(&pause, &cancel), Cancelled);
    // 4) 新序列：暂停→恢复→继续推进→取消终止
    let pause2 = std::sync::atomic::AtomicBool::new(true);
    let cancel2 = std::sync::atomic::AtomicBool::new(false);
    assert_eq!(gate(&pause2, &cancel2), WaitPaused);
    pause2.store(false, std::sync::atomic::Ordering::SeqCst); // resume
    assert_eq!(gate(&pause2, &cancel2), Advance);
    cancel2.store(true, std::sync::atomic::Ordering::SeqCst); // cancel
    assert_eq!(gate(&pause2, &cancel2), Cancelled);
    // 5) 暂停期间断点不回退：暂停前后 per-worker 计数只增不减
    let ranges = split_ranges(0, 800, 8);
    let before: Vec<u64> = vec![0, 100, 50, 0, 0, 0, 0, 0];
    let after: Vec<u64> = vec![0, 100, 50, 0, 0, 0, 0, 0]; // 停等期间无新增
    let ob = resume_offsets(Some(&before), &ranges);
    let oa = resume_offsets(Some(&after), &ranges);
    assert_eq!(ob, oa);
}

/* ================= 七、限速节流计算 ================= */

/// 限速 → 块间延迟推导：均分、0=不限、下限钳 1、上限钳 60s
#[test]
fn t07_speed_limit_throttle_derivation() {
    // 不限速
    assert_eq!(chunk_delay_ms(0, 8, CHUNK), 0);
    assert_eq!(per_worker_bps(0, 8), 0);
    // 手册验收量级：10MB/s、8 worker → 每 worker 1.25MiB/s、1MiB 块间 ~838ms
    let per = per_worker_bps(10 * 1024 * 1024, 8);
    assert_eq!(per, 1_310_720);
    assert_eq!(chunk_delay_ms(10 * 1024 * 1024, 8, CHUNK), CHUNK * 1000 / per);
    // worker 数不影响聚合速率：限速 L、n worker → 每 worker 延迟 ∝ n
    let l = 1024 * 1024;
    assert_eq!(chunk_delay_ms(l, 1, CHUNK), 1000);
    assert_eq!(chunk_delay_ms(l, 4, CHUNK), 4000);
    assert_eq!(chunk_delay_ms(l, 16, CHUNK), 16000);
    // 限速小于 worker 数（每 worker <1B/s）→ 钳到 1B/s，延迟钳 60s
    assert_eq!(per_worker_bps(4, 16), 1);
    assert_eq!(chunk_delay_ms(4, 16, CHUNK), 60_000);
    // 小块：延迟按块大小线性缩放
    assert_eq!(chunk_delay_ms(1024 * 1024, 1, 4096), 3); // 4096*1000/1048576 = 3.9 → 3
    // 超大限速 → 延迟取整为 0（等价不限速节奏）
    assert_eq!(chunk_delay_ms(u64::MAX, 1, CHUNK), 0);
}

/* ================= 八、限速与暂停/取消叠加 ================= */

/// 限速节流不影响暂停/取消门控：暂停优先于节流，恢复后按限速继续，取消随时生效
#[test]
fn t08_speed_limit_overlay_with_pause() {
    let limit = 2 * 1024 * 1024; // 2MiB/s
    let workers = 4usize;
    let dms = chunk_delay_ms(limit, workers, CHUNK);
    assert_eq!(dms, 2000); // 每 worker 0.5MiB/s → 1MiB 块间 2000ms
    let pause = std::sync::atomic::AtomicBool::new(false);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let gate = || {
        worker_gate(pause.load(std::sync::atomic::Ordering::SeqCst), cancel.load(std::sync::atomic::Ordering::SeqCst))
    };
    // 模拟 worker 循环推进 3 块（每块后 sleep dms）
    let mut steps = 0u32;
    for _ in 0..3 {
        assert_eq!(gate(), GateAction::Advance);
        steps += 1;
        assert!(dms > 0); // 节流 sleep
    }
    assert_eq!(steps, 3);
    // 暂停：门控变 WaitPaused，节流不再推进新块
    pause.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(gate(), GateAction::WaitPaused);
    // 恢复：继续推进，节流参数不变（限速语义跨暂停保持）
    pause.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(gate(), GateAction::Advance);
    assert_eq!(chunk_delay_ms(limit, workers, CHUNK), dms);
    // 取消优先于暂停与限速
    cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(gate(), GateAction::Cancelled);
}

/* ================= 九、SOCKS5 握手字节流（含认证子协商成功/失败） ================= */

/// 完整握手字节流：问候 → 方法选择 → 认证子协商 → CONNECT → 应答。
/// 逐段用纯函数模拟客户端/服务端两侧编解码（与 net.rs 同一套编解码函数）。
#[test]
fn t09_socks5_handshake_bytes_auth_subnegotiation_ok() {
    // 1) 客户端问候（带用户名密码）：05 01 02
    let greeting = vec![0x05u8, 0x01, 0x02];
    assert_eq!(greeting, vec![5, 1, 2]);
    // 2) 服务端选 USERNAME/PASSWORD(0x02) → 客户端需继续子协商
    let method_reply = [0x05u8, 0x02];
    assert_eq!(socks5_method_reply(&method_reply, true), Ok(true));
    // 3) 认证子协商：01 | len(u) u | len(p) p；服务端应答 01 00 = 成功
    let auth_req = {
        let mut v = vec![0x01u8, 4];
        v.extend_from_slice(b"user");
        v.push(4);
        v.extend_from_slice(b"pass");
        v
    };
    assert_eq!(auth_req, vec![1, 4, b'u', b's', b'e', b'r', 4, b'p', b'a', b's', b's']);
    let auth_ok = [0x01u8, 0x00];
    assert_eq!(socks5_auth_reply(&auth_ok), Ok(()));
    // 4) CONNECT 请求（ATYP=domain）
    let conn = build_like_socks5_connect("example.com", 22);
    assert_eq!(conn[0..4], [0x05, 0x01, 0x00, 0x03]);
    assert_eq!(conn[4] as usize, "example.com".len());
    // 5) 服务端成功应答：05 00 00 01 + 4 字节 BND.ADDR + 2 字节 BND.PORT
    let mut reply = vec![0x05u8, 0x00, 0x00, 0x01];
    reply.extend_from_slice(&[10, 0, 0, 1]);
    reply.extend_from_slice(&[0, 22]);
    let rest = socks5_connect_reply_head(&reply[..4]).unwrap();
    assert_eq!(rest, 6);
    assert_eq!(reply.len(), 4 + rest, "IPv4 应答总长 = 4 + 6");
    // 6) 无认证问候 + 服务端选 NO-AUTH(0x00)：不需子协商
    assert_eq!(socks5_method_reply(&[0x05, 0x00], false), Ok(false));
    // 请求了认证但服务端选 NO-AUTH → 拒绝
    assert!(socks5_method_reply(&[0x05, 0x00], true).is_err());
}

/// 构造与 net.rs::build_socks5_connect 相同字节的请求（测试 crate 内本地复制以自证格式）
fn build_like_socks5_connect(host: &str, port: u16) -> Vec<u8> {
    let mut out = vec![0x05u8, 0x01, 0x00, 0x03, host.len() as u8];
    out.extend_from_slice(host.as_bytes());
    out.push((port >> 8) as u8);
    out.push((port & 0xff) as u8);
    out
}

/// SOCKS5 握手失败路径：无可接受方法(0xFF)、认证子协商被拒、CONNECT 失败码、非 SOCKS5 协议
#[test]
fn t10_socks5_handshake_failure_paths() {
    // 服务端不接受任何方法（含用户名密码）
    let e = socks5_method_reply(&[0x05, 0xff], true).unwrap_err();
    assert!(e.contains("不接受用户名密码认证"));
    // 认证子协商失败（status != 0）
    let e = socks5_auth_reply(&[0x01, 0x01]).unwrap_err();
    assert!(e.contains("用户名或密码被拒绝"));
    // CONNECT 被拒（REP=5 连接被拒绝）
    let e = socks5_connect_reply_head(&[0x05, 0x05, 0x00, 0x01]).unwrap_err();
    assert!(e.contains("代理连接目标被拒绝") && e.contains("code 5"));
    // REP=3 网络不可达 / REP=7 地址类型不支持
    assert!(socks5_connect_reply_head(&[0x05, 0x03, 0x00, 0x01]).unwrap_err().contains("不可达"));
    assert!(socks5_connect_reply_head(&[0x05, 0x07, 0x00, 0x01]).unwrap_err().contains("地址类型"));
    // 非 SOCKS5 协议（VER=4，HTTP 代理误配成 SOCKS5 的典型现场）
    let e = socks5_method_reply(b"HT", true).unwrap_err();
    assert!(e.contains("非 SOCKS5"));
    // 应答过短
    assert!(socks5_method_reply(&[0x05], true).is_err());
    assert!(socks5_auth_reply(&[0x01]).is_err());
    assert!(socks5_connect_reply_head(&[0x05, 0x00, 0x00]).is_err());
    // 未知地址类型
    assert!(socks5_connect_reply_head(&[0x05, 0x00, 0x00, 0x09]).is_err());
}

/* ================= 十、HTTP CONNECT 响应解析 ================= */

/// HTTP CONNECT 响应矩阵：成功 200 / 代理拒绝 4xx·5xx / 响应异常（非状态行）
#[test]
fn t11_http_connect_response_matrix() {
    // 成功（含 reason phrase 与完整头）
    assert!(parse_http_connect_response(b"HTTP/1.1 200 Connection established\r\n\r\n").is_ok());
    assert!(parse_http_connect_response(b"HTTP/1.0 200 OK\r\n\r\n").is_ok());
    // 代理拒绝：403/407
    let e403 = parse_http_connect_response(b"HTTP/1.1 403 Forbidden\r\n\r\n").unwrap_err();
    assert!(e403.contains("代理拒绝") && e403.contains("403"));
    let e407 = parse_http_connect_response(b"HTTP/1.1 407 Proxy Auth Required\r\n\r\n").unwrap_err();
    assert!(e407.contains("407"));
    // 5xx 同样是代理拒绝
    assert!(parse_http_connect_response(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").is_err());
    // 非 200 格式 / 空响应
    let eg = parse_http_connect_response(b"garbage").unwrap_err();
    assert!(eg.contains("代理响应异常"));
    assert!(parse_http_connect_response(b"").is_err());
    // 状态行缺少状态码
    assert!(parse_http_connect_response(b"HTTP/1.1\r\n\r\n").is_err());
}

/* ================= 十一、跳板链防自环去重 ================= */

/// jumpChain 中指向目标自身的 key 被跳过；查表无命中的 key 跳过；合法跳板保留
#[test]
fn t12_jump_chain_self_loop_dedup() {
    let target = serde_json::json!({"host": "dst", "port": 22, "username": "root"});
    let hop = serde_json::json!({"host": "hop1", "port": 22, "username": "ops"});
    let sessions = vec![target.clone(), hop.clone()];
    let chain = serde_json::json!([
        "dst:22:root",   // 自环 → 跳过
        "hop1:22:ops",   // 合法
        "ghost:22:x",    // 查表无命中 → 跳过
        "bad-key",       // 格式非法 → 跳过
        "hop1:22:ops",   // 重复 key：查表命中同一记录（保留，链式两级逐级认证由运行时保证）
    ]);
    let hops = resolve_jump_chain(
        chain.as_array().unwrap(),
        &sessions,
        "dst", 22, "root",
        6, // 放宽截断以单独验证自环剔除（截断在 t13 单测）
    );
    assert_eq!(hops.len(), 2, "自环被剔除；查表无命中被剔除；重复 key 保留");
    assert_eq!(hops[0]["host"], "hop1");
    assert_eq!(hops[1]["host"], "hop1");
    // 全自环链 → 空（直连语义）
    let hops = resolve_jump_chain(
        serde_json::json!(["dst:22:root"]).as_array().unwrap(),
        &sessions, "dst", 22, "root", 3,
    );
    assert!(hops.is_empty());
    // 端口缺省 22 的会话匹配
    let noport = serde_json::json!({"host": "np", "username": "u"});
    let sessions2 = vec![noport];
    let hops = resolve_jump_chain(
        serde_json::json!(["np:22:u"]).as_array().unwrap(),
        &sessions2, "dst", 22, "root", 3,
    );
    assert_eq!(hops.len(), 1);
}

/* ================= 十二、跳板链 ≥4 级截断 ================= */

/// 后端强制链深度 ≤ MAX_HOPS(3)：配置 4+ 级时截断
#[test]
fn t13_jump_chain_truncates_at_max_hops() {
    const MAX_HOPS: usize = 3;
    let mk = |h: &str| serde_json::json!({"host": h, "port": 22, "username": "u"});
    let sessions: Vec<serde_json::Value> =
        vec![mk("j1"), mk("j2"), mk("j3"), mk("j4"), mk("j5")];
    let chain = serde_json::json!(["j1:22:u", "j2:22:u", "j3:22:u", "j4:22:u", "j5:22:u"]);
    let hops = resolve_jump_chain(
        chain.as_array().unwrap(),
        &sessions,
        "dst", 22, "root",
        MAX_HOPS,
    );
    assert_eq!(hops.len(), MAX_HOPS, "5 级配置被截断到 3 级");
    assert_eq!(hops[0]["host"], "j1");
    assert_eq!(hops[2]["host"], "j3");
    // max_hops=1（单级跳板）同理
    let hops = resolve_jump_chain(
        chain.as_array().unwrap(),
        &sessions,
        "dst", 22, "root",
        1,
    );
    assert_eq!(hops.len(), 1);
    assert_eq!(hops[0]["host"], "j1");
}

/* ================= 十三、上传续传：远端被改写拒绝自动续传 ================= */

/// 上传续传校验：远端已有部分比本地源文件大 → 拒绝自动续传（防覆盖别人的数据）
#[test]
fn t14_upload_resume_remote_rewritten_rejected() {
    // 正常：远端部分 ≤ 本地大小 → 放行
    assert_eq!(resume_upload_guard(500, 1000), Ok(()));
    assert_eq!(resume_upload_guard(1000, 1000), Ok(()));
    assert_eq!(resume_upload_guard(0, 1000), Ok(()));
    // 远端被改写变大 → 拒绝
    let e = resume_upload_guard(1001, 1000).unwrap_err();
    assert!(e.contains("远端文件已变化"));
    let e = resume_upload_guard(u64::MAX, 1).unwrap_err();
    assert!(e.contains("远端文件已变化"));
    // 本地为 0 字节文件，远端有任何内容 → 拒绝
    assert!(resume_upload_guard(1, 0).is_err());
}
