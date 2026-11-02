//! 防重放：会话级滑动窗口。
//!
//! ## 为什么不是"严格递增 last_seq"
//!
//! 客户端会并发发起多个请求（例如 `loadAccounts()` 与 `loadLogs()` 不 await
//! 就一起发出）。TCP 层的到达顺序不保证，严格单调的 `last_seq` 会把
//! **合法的乱序请求**误判为重放。
//!
//! 因此这里用一个固定宽度的滑动窗口：
//!
//! - 记录窗口内已接受的序号集合（`HashSet`），重复出现即拒绝；
//! - 序号小于 `max_seq - WINDOW` 视为过期，直接拒绝（不必查集合）；
//! - 窗口随 `max_seq` 前移，集合大小恒定为 O(WINDOW)。
//!
//! 该结构在"拒绝重放"和"允许并发乱序"之间取得平衡，且内存有上界。

use std::collections::HashSet;

/// 滑动窗口宽度。
pub const WINDOW: u64 = 4096;

/// 单会话单方向的接收窗口。
pub struct ReplayWindow {
    max_seq: u64,
    seen: HashSet<u64>,
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayWindow {
    pub fn new() -> Self {
        Self {
            max_seq: 0,
            seen: HashSet::new(),
        }
    }

    /// 尝试接受序号 `seq`。返回 `false` 表示重复 / 过期 / 非法（0）。
    pub fn accept(&mut self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }
        if seq.saturating_add(WINDOW) <= self.max_seq {
            return false; // 太旧，已滑出窗口
        }
        if !self.seen.insert(seq) {
            return false; // 重复
        }
        if seq > self.max_seq {
            self.max_seq = seq;
        }
        if self.seen.len() > (WINDOW as usize) * 2 {
            self.prune();
        }
        true
    }

    /// 该序号是否已经出现过（不改变状态）。
    ///
    /// 用于"先解密、认证成功后再消费序号"的流程：伪造报文不会挤掉合法序号。
    pub fn is_duplicate(&self, seq: u64) -> bool {
        if seq == 0 {
            return true;
        }
        if seq.saturating_add(WINDOW) <= self.max_seq {
            return true;
        }
        self.seen.contains(&seq)
    }

    /// 当前已接受的最大序号（诊断用）。
    #[allow(dead_code)]
    pub fn max_seq(&self) -> u64 {
        self.max_seq
    }

    fn prune(&mut self) {
        let floor = self.max_seq.saturating_sub(WINDOW);
        self.seen.retain(|s| *s > floor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_ok_replay_rejected() {
        let mut w = ReplayWindow::new();
        for i in 1..=100 {
            assert!(w.accept(i));
        }
        assert!(!w.accept(50), "重复序号必须拒绝");
        assert!(!w.accept(100));
    }

    #[test]
    fn out_of_order_within_window_is_accepted() {
        let mut w = ReplayWindow::new();
        assert!(w.accept(5));
        assert!(w.accept(3));
        assert!(w.accept(4));
        assert!(!w.accept(3));
    }

    #[test]
    fn zero_is_rejected() {
        let mut w = ReplayWindow::new();
        assert!(!w.accept(0));
    }

    #[test]
    fn too_old_is_rejected() {
        let mut w = ReplayWindow::new();
        assert!(w.accept(10_000));
        // 落在 max - WINDOW 之前
        assert!(!w.accept(10_000 - WINDOW));
        assert!(!w.accept(1));
        // 边界内仍可接受
        assert!(w.accept(10_000 - WINDOW + 1));
    }

    #[test]
    fn memory_bounded() {
        let mut w = ReplayWindow::new();
        for i in 1..=50_000u64 {
            w.accept(i);
        }
        assert!(w.seen.len() <= (WINDOW as usize) * 2);
    }

    #[test]
    fn replayed_ciphertext_same_seq_rejected() {
        // 协议层面：同 seq 同一密文只能成功一次
        let mut w = ReplayWindow::new();
        assert!(w.accept(1));
        assert!(!w.accept(1));
    }
}
