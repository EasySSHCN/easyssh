//! agent-wire/1 编解码（EasySSH 独立实现；spec 与网关侧文档逐字一致）。
//!
//! WS Binary 帧头 `[type:1][session_id:8 BE][payload_len:4 BE][payload]`；
//! 指令集 DIAL(0x01)/OPEN_OK(0x02)/OPEN_ERR(0x03)/DATA(0x04)/CLOSE(0x05)/
//! PING(0x06)/PONG(0x07)。心跳：30s 间隔，连续 3 次未应答判死（本端按
//! heartbeat_secs 配置驱动，网关侧同参数）。

/// 帧头字节数：1 type + 8 session + 4 len。
pub const HEADER_BYTES: usize = 13;

/// 单帧 body 上限 4 MiB（与网关侧一致，仅防声明攻击）。
pub const BODY_LIMIT: usize = 4 * 1024 * 1024;

/// agent-wire/1 指令码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Dial = 0x01,
    OpenOk = 0x02,
    OpenErr = 0x03,
    Data = 0x04,
    Close = 0x05,
    Ping = 0x06,
    Pong = 0x07,
}

impl Op {
    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(b: u8) -> Option<Op> {
        Some(match b {
            0x01 => Op::Dial,
            0x02 => Op::OpenOk,
            0x03 => Op::OpenErr,
            0x04 => Op::Data,
            0x05 => Op::Close,
            0x06 => Op::Ping,
            0x07 => Op::Pong,
            _ => return None,
        })
    }
}

/// 一条 agent-wire/1 报文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub op: Op,
    pub session: u64,
    pub body: Vec<u8>,
}

impl Envelope {
    pub fn new(op: Op, session: u64, body: impl Into<Vec<u8>>) -> Self {
        Self {
            op,
            session,
            body: body.into(),
        }
    }

    /// 序列化为完整 WS Binary 负载。
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_BYTES + self.body.len());
        out.push(self.op.code());
        out.extend_from_slice(&self.session.to_be_bytes());
        out.extend_from_slice(&(self.body.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.body);
        out
    }
}

/// 解码缺陷：非法指令码 / 超限 body 声明。调用方应丢弃该连接残余缓冲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireFault {
    UnknownOp(u8),
    BodyTooLong(usize),
}

/// 从连续字节流上切一条完整报文：`Full(报文, 消耗字节数)` 或 `NeedMore`。
pub fn cut_envelope(buf: &[u8]) -> Result<Option<(Envelope, usize)>, WireFault> {
    if buf.len() < HEADER_BYTES {
        return Ok(None);
    }
    let op = Op::from_code(buf[0]).ok_or(WireFault::UnknownOp(buf[0]))?;
    let session = u64::from_be_bytes(buf[1..9].try_into().expect("8 bytes"));
    let body_len = u32::from_be_bytes(buf[9..13].try_into().expect("4 bytes")) as usize;
    if body_len > BODY_LIMIT {
        return Err(WireFault::BodyTooLong(body_len));
    }
    let total = HEADER_BYTES + body_len;
    if buf.len() < total {
        return Ok(None);
    }
    let body = buf[HEADER_BYTES..total].to_vec();
    Ok(Some((Envelope::new(op, session, body), total)))
}

/// DIAL payload 解析：网关下发 `{"backend":"host:port"}`；解析失败视为
/// 非法 DIAL（回 OPEN_ERR）。
pub fn parse_dial_body(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    v.get("backend")?.as_str().map(str::to_string)
}

/// 构造 DIAL payload（对称工具，供测试与文档示例）。
pub fn dial_body(backend: &str) -> Vec<u8> {
    serde_json::json!({ "backend": backend }).to_string().into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_covers_all_ops() {
        for op in [Op::Dial, Op::OpenOk, Op::OpenErr, Op::Data, Op::Close, Op::Ping, Op::Pong] {
            let env = Envelope::new(op, 0x0102_0304_0506_0708, b"ssh-bytes".to_vec());
            let raw = env.to_bytes();
            assert_eq!(raw.len(), HEADER_BYTES + 9);
            // 帧头逐字段校验（wire spec 逐字一致的自证）
            assert_eq!(raw[0], op.code());
            assert_eq!(&raw[1..9], &env.session.to_be_bytes());
            assert_eq!(&raw[9..13], &9u32.to_be_bytes());
            let (back, used) = cut_envelope(&raw).expect("decode").expect("full");
            assert_eq!(used, raw.len());
            assert_eq!(back, env);
        }
    }

    #[test]
    fn cut_envelope_handles_partial_and_pipelined_bytes() {
        let raw = Envelope::new(Op::Data, 7, b"abc".to_vec()).to_bytes();
        assert_eq!(cut_envelope(&raw[..4]).expect("ok"), None);
        assert_eq!(cut_envelope(&raw[..HEADER_BYTES + 2]).expect("ok"), None);
        let mut doubled = raw.clone();
        doubled.extend_from_slice(&raw);
        let (_, used1) = cut_envelope(&doubled).expect("ok").expect("full");
        assert_eq!(used1, raw.len());
        let (_, used2) = cut_envelope(&doubled[used1..]).expect("ok").expect("full");
        assert_eq!(used2, raw.len());
    }

    #[test]
    fn cut_envelope_rejects_unknown_op_and_huge_body() {
        let mut raw = Envelope::new(Op::Data, 1, Vec::new()).to_bytes();
        raw[0] = 0xEE;
        assert_eq!(cut_envelope(&raw), Err(WireFault::UnknownOp(0xEE)));
        let mut big = Envelope::new(Op::Ping, 1, Vec::new()).to_bytes();
        big[9..13].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            cut_envelope(&big),
            Err(WireFault::BodyTooLong(u32::MAX as usize))
        );
    }

    #[test]
    fn zero_body_frame_roundtrip() {
        let env = Envelope::new(Op::Close, u64::MAX, Vec::new());
        let raw = env.to_bytes();
        assert_eq!(raw.len(), HEADER_BYTES);
        let (back, used) = cut_envelope(&raw).expect("ok").expect("full");
        assert_eq!(used, HEADER_BYTES);
        assert_eq!(back, env);
    }

    #[test]
    fn dial_body_parse() {
        let body = dial_body("127.0.0.1:22");
        assert_eq!(parse_dial_body(&body).as_deref(), Some("127.0.0.1:22"));
        assert_eq!(parse_dial_body(b"not json"), None);
        assert_eq!(parse_dial_body(br#"{"nope":1}"#), None);
        assert_eq!(parse_dial_body(br#"{"backend":42}"#), None);
    }
}
