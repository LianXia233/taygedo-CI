//! 每日定时签到调度器。
//!
//! 设计要点（修复"各平台定时签到失效"）：
//! - 每 15 秒检查一次；
//! - 若账号的设定时间（默认或单独设定）"已过 / 正好到达"今天，且今天尚未触发，则立即签到（含补签）；
//! - 进程在设定时间之后才启动（如 Windows 开机、服务重启）也能补签，避免错过唯一的一分钟窗口；
//! - 每个账号每天只触发一次，跨天自动重置。

use std::collections::HashMap;
use std::sync::Arc;

use crate::service::AppState;

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // acc.id -> 已触发的日期（用于每天只签到一次 + 跨天重置）
        let mut fired_on: HashMap<String, String> = HashMap::new();
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let today = crate::time::shanghai_date();
            let now_hhmm = crate::time::shanghai_hhmm();

            // 清理昨天的记录，避免无限增长
            fired_on.retain(|_, d| d == &today);

            let config = state.config.read().await.clone();
            let accounts = state.accounts.read().await.clone();
            let default_time = normalize_hhmm(&config.default_schedule);

            // 找出"到时间且今天还没签到"的账号
            let due: Vec<String> = accounts
                .iter()
                .filter(|acc| {
                    if fired_on.get(&acc.id).map(|d| d == &today).unwrap_or(false) {
                        return false; // 今天已触发，跳过
                    }
                    let t = config
                        .schedules
                        .get(&acc.id)
                        .map(|s| normalize_hhmm(s))
                        .unwrap_or(default_time.clone());
                    match t {
                        Some(t) => now_hhmm >= t, // 设定时间已过或正好到达
                        None => false,            // 无有效时间，不自动签到
                    }
                })
                .map(|acc| acc.id.clone())
                .collect();

            if !due.is_empty() {
                // 先标记，避免 run_signin 执行期间（可能跨越多个 15s tick）被重复触发导致重复签到
                for id in &due {
                    fired_on.insert(id.clone(), today.clone());
                }
                state.push_log(
                    "info",
                    format!(
                        "定时任务触发：北京时间 {}，待签到账号 {} 个",
                        now_hhmm,
                        due.len()
                    ),
                );
                let _ = crate::service::run_signin(&state, false, Some(due.as_slice())).await;
            }
        }
    });
}

/// 将 "H:M" / "HH:MM" 归一化为零填充的 "HH:MM"；非法或空返回 None。
/// 归一化后可用字符串字典序直接比较时间先后。
fn normalize_hhmm(s: &str) -> Option<String> {
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
