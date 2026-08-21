//! 每日定时签到调度器：每 15 秒检查一次，匹配到账号设定的 HH:MM 即触发。

use std::sync::Arc;

use crate::service::AppState;

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut last_fired = String::new();
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let now_hhmm = crate::time::shanghai_hhmm();
            let today = crate::time::shanghai_date();
            let fire_key = format!("{today} {now_hhmm}");
            if fire_key == last_fired {
                continue;
            }

            let config = state.config.read().await.clone();
            let accounts = state.accounts.read().await.clone();
            let default_time = config.default_schedule.clone();

            let should_run = accounts.iter().any(|acc| {
                let t = config
                    .schedules
                    .get(&acc.id)
                    .cloned()
                    .unwrap_or_else(|| default_time.clone());
                t == now_hhmm
            });

            if should_run {
                last_fired = fire_key;
                state.push_log("info", format!("定时任务触发：北京时间 {}", now_hhmm));
                let _ = crate::service::run_signin(&state, false, None).await;
            }
        }
    });
}
