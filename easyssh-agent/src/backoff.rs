//! 断线指数退避重连（方案：指数退避，封顶 30s；连接成功后复位）。

use std::time::Duration;

/// 指数退避状态机：base → ×2 → cap（方案口径 base 1s、cap 30s）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backoff {
    base_ms: u64,
    cap_ms: u64,
    attempts: u32,
}

impl Backoff {
    pub fn new(base_ms: u64, cap_ms: u64) -> Self {
        let base_ms = base_ms.max(1);
        Self {
            base_ms,
            cap_ms: cap_ms.max(base_ms),
            attempts: 0,
        }
    }

    /// 按方案默认参数：base 1s、cap 30s。
    pub fn default_policy() -> Self {
        Self::new(1_000, 30_000)
    }

    /// 取下一次等待时长并推进计数（1s, 2s, 4s, ..., 30s, 30s, ...）。
    pub fn next(&mut self) -> Duration {
        let n = self.attempts.saturating_add(1);
        self.attempts = n;
        // 尝试次数用移位表达倍增；>63 次后饱和到 cap（saturating 保护）。
        let ms = self
            .base_ms
            .saturating_mul(1u64 << (n - 1).min(63))
            .min(self.cap_ms);
        Duration::from_millis(ms)
    }

    /// 连接成功后复位（下一次断线从 base 重新开始）。
    pub fn reset(&mut self) {
        self.attempts = 0;
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubling_sequence_caps_at_thirty_seconds() {
        let mut b = Backoff::default_policy();
        let seq_ms: Vec<u64> = (0..8)
            .map(|_| b.next().as_millis() as u64)
            .collect();
        assert_eq!(seq_ms, vec![1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000, 30_000]);
    }

    #[test]
    fn reset_returns_to_base() {
        let mut b = Backoff::default_policy();
        for _ in 0..4 {
            b.next();
        }
        b.reset();
        assert_eq!(b.attempts(), 0);
        assert_eq!(b.next(), Duration::from_millis(1_000));
    }

    #[test]
    fn custom_base_and_cap_clamped() {
        let mut b = Backoff::new(500, 2_000);
        assert_eq!(b.next().as_millis(), 500);
        assert_eq!(b.next().as_millis(), 1_000);
        assert_eq!(b.next().as_millis(), 2_000);
        assert_eq!(b.next().as_millis(), 2_000);
    }

    #[test]
    fn tiny_base_does_not_zero_out() {
        let mut b = Backoff::new(1, 5);
        // 1 → 2 → 4 → 5(cap)，不会因溢出归零
        assert_eq!(b.next().as_millis(), 1);
        assert_eq!(b.next().as_millis(), 2);
        assert_eq!(b.next().as_millis(), 4);
        assert_eq!(b.next().as_millis(), 5);
        assert_eq!(b.next().as_millis(), 5);
    }

    #[test]
    fn long_run_stays_saturated() {
        let mut b = Backoff::default_policy();
        for _ in 0..80 {
            b.next();
        }
        assert_eq!(b.next(), Duration::from_millis(30_000));
    }
}
