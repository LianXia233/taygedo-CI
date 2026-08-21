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
