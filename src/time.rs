//! 上海时区时间辅助。

use chrono::{FixedOffset, Utc};

pub fn shanghai_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8")
}

pub fn now_shanghai() -> chrono::DateTime<FixedOffset> {
    Utc::now().with_timezone(&shanghai_offset())
}

/// `2026-08-21T12:00:00+08:00`
pub fn shanghai_datetime() -> String {
    now_shanghai().format("%Y-%m-%dT%H:%M:%S+08:00").to_string()
}

/// `2026-08-21`
pub fn shanghai_date() -> String {
    now_shanghai().format("%Y-%m-%d").to_string()
}

/// `12:00`
pub fn shanghai_hhmm() -> String {
    now_shanghai().format("%H:%M").to_string()
}

/// `2026/08/21 12:00:00`
pub fn log_ts() -> String {
    now_shanghai().format("%Y/%m/%d %H:%M:%S").to_string()
}

/// 校验 "HH:MM"（小时、分钟均为两位数字，且范围合法）。
/// 统一入口，供 web / service / scheduler 共用，避免多处重复实现。
pub fn valid_hhmm(t: &str) -> bool {
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() != 2 || parts[0].len() != 2 || parts[1].len() != 2 {
        return false;
    }
    matches!(
        (parts[0].parse::<u32>(), parts[1].parse::<u32>()),
        (Ok(h), Ok(m)) if h < 24 && m < 60
    )
}

/// 将 "H:M" / "HH:MM" 归一化为零填充的 "HH:MM"；非法或空返回 None。
/// 归一化后可用字符串字典序直接比较时间先后。
pub fn normalize_hhmm(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(format!("{:02}:{:02}", h, m))
}
