//! 签到核心逻辑（上游 runner.ts 的 Rust 移植，含幽灵角色修复与中文名/详细日志）。

use std::collections::HashSet;

use crate::api::{Api, ApiError};
use crate::models::{
    Account, AccountResult, AppSigninResult, CloudDurationResult, CoinTaskResult, GameRole,
    GameSigninResult, Reward, RunResult,
};

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub coin_tasks: bool,
    pub cloud_duration: bool,
    pub share_platform: String,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            coin_tasks: true,
            cloud_duration: true,
            share_platform: "qq".into(),
        }
    }
}

/// 游戏名显示：优先中文名，回退 gameId。
pub fn game_label(game_id: &str, game_name: Option<&str>) -> String {
    match game_name {
        Some(n) if !n.is_empty() && n != game_id => format!("{n}({game_id})"),
        _ => game_id.to_string(),
    }
}

fn is_already_signed(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    msg.contains("已签到")
        || msg.contains("签到过")
        || msg.contains("重复签到")
        || msg.contains("已经签到")
        || (lower.contains("already") && lower.contains("sign"))
}

fn is_auth_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("auth_expired")
        || lower.contains("http 401")
        || lower.contains("http 402")
        || lower.contains("http 403")
        || lower.contains("登录")
        || lower.contains("token")
        || lower.contains("未授权")
        || lower.contains("请先")
        || lower.contains("过期")
        || lower.contains("失效")
        || lower.contains("invalid_token")
}

/// 单账号签到：返回更新后的账号与结果。
pub async fn run_account(
    api: &Api,
    account: &Account,
    credential_key: Option<&str>,
    opts: &RunOptions,
    log: &mut (dyn FnMut(String) + Send),
) -> Result<(Account, AccountResult), ApiError> {
    // 修复：原逻辑「本地已有 accessToken 就直接使用」，而 accessToken 会过期。
    // 过期后网关层（istio-envoy）对部分端点直接返回 HTTP 401（空响应体），
    // 且并非所有失败都会命中 is_auth_error，导致当天签到永久失败。
    // 改为签到前主动续期（refreshToken 代价最小、不触发风控）；
    // 续期失败时回退复用现有会话，由后续失败重试兜底，避免弱网下误伤。
    let (start_account, access_token) =
        match refresh_or_rebuild_session(api, account, credential_key, log).await {
            Ok(v) => v,
            Err(e) => {
                log(format!(
                    "账号 {}：会话预刷新失败（{}），回退复用现有会话",
                    account.id, e.0
                ));
                match account.access_token.as_deref() {
                    Some(at) => (account.clone(), at.to_string()),
                    None => return Err(e),
                }
            }
        };

    match sign_with_session(api, &start_account, &access_token, credential_key, opts, log).await {
        Ok(res) => Ok(res),
        Err(e) if is_auth_error(&e.0) => {
            log(format!(
                "账号 {}：会话失效（{}），刷新/重建会话后重试",
                account.id, e.0
            ));
            let (acc, token) =
                refresh_or_rebuild_session(api, &start_account, credential_key, log).await?;
            sign_with_session(api, &acc, &token, credential_key, opts, log).await
        }
        Err(e) => Err(e),
    }
}

async fn refresh_or_rebuild_session(
    api: &Api,
    account: &Account,
    credential_key: Option<&str>,
    log: &mut (dyn FnMut(String) + Send),
) -> Result<(Account, String), ApiError> {
    // 1. refreshToken 刷新（优先，代价最小、不触发登录风控）
    match api.refresh_token(&account.refresh_token, &account.device_id).await {
        Ok((at, rt, uid)) => {
            log(format!("账号 {}：刷新会话（refreshToken）", account.id));
            return Ok((with_session(account, &at, &rt, uid.as_deref(), None, None), at));
        }
        Err(e) => {
            log(format!("账号 {}：refreshToken 刷新失败（{}）", account.id, e.0));
            if !e.0.contains("REFRESH_REJECTED_402")
                || account.laohu_token.is_none()
                || account.laohu_user_id.is_none()
            {
                // 非 402 或缺少 laohuToken 时，尝试密码重登兜底，仍失败则返回原始错误
                if let (Some(phone), Some(pwd)) =
                    (account.phone.as_deref(), resolve_password(account, credential_key).as_deref())
                {
                    if let Ok((token, user_id)) =
                        api.login_with_password(phone, pwd, &account.device_id).await
                    {
                        let (at, rt, uid) = api
                            .user_center_login(&token, &user_id, &account.device_id)
                            .await?;
                        log(format!("账号 {}：密码重新登录成功", account.id));
                        return Ok((
                            with_session(account, &at, &rt, Some(&uid), Some(&token), Some(&user_id)),
                            at,
                        ));
                    }
                }
                return Err(e);
            }
            log(format!("账号 {}：refreshToken 失效，使用 laohuToken 重建会话", account.id));
        }
    }

    // 2. laohuToken 重建
    match api
        .user_center_login(
            account.laohu_token.as_deref().unwrap(),
            account.laohu_user_id.as_deref().unwrap(),
            &account.device_id,
        )
        .await
    {
        Ok((at, rt, uid)) => {
            log(format!("账号 {}：laohuToken 重建会话成功", account.id));
            return Ok((with_session(account, &at, &rt, Some(&uid), None, None), at));
        }
        Err(e) => {
            log(format!("账号 {}：laohuToken 重建失败（{}）", account.id, e.0));
            // 3. 密码重登（最后兜底）
            if let (Some(phone), Some(pwd)) =
                (account.phone.as_deref(), resolve_password(account, credential_key).as_deref())
            {
                let (token, user_id) =
                    api.login_with_password(phone, pwd, &account.device_id).await?;
                let (at, rt, uid) = api
                    .user_center_login(&token, &user_id, &account.device_id)
                    .await?;
                log(format!("账号 {}：密码重新登录成功", account.id));
                return Ok((
                    with_session(account, &at, &rt, Some(&uid), Some(&token), Some(&user_id)),
                    at,
                ));
            }
            return Err(e);
        }
    }
}

async fn sign_with_session(
    api: &Api,
    account: &Account,
    access_token: &str,
    credential_key: Option<&str>,
    opts: &RunOptions,
    log: &mut (dyn FnMut(String) + Send),
) -> Result<(Account, AccountResult), ApiError> {
    let roles = get_all_game_roles(api, access_token, &account.uid, &account.device_id).await;

    if !roles.is_empty() {
        let list = roles
            .iter()
            .map(|r| {
                format!(
                    "{} / {}",
                    game_label(&r.game_id, r.game_name.as_deref()),
                    r.role_name.as_deref().unwrap_or(&r.role_id)
                )
            })
            .collect::<Vec<_>>()
            .join("；");
        log(format!(
            "账号 {}（{}）：获取到 {} 个角色：{}",
            account.name,
            account.id,
            roles.len(),
            list
        ));
    } else {
        log(format!(
            "账号 {}（{}）：未获取到任何角色",
            account.name, account.id
        ));
    }

    // APP 签到
    // 修复：原实现用 `?` 传播错误，APP 签到这一独立子任务失败会中断整轮，
    // 导致游戏签到、金币任务、云时长全部不执行。改为降级记录、继续执行。
    let mut app_error: Option<String> = None;
    let app = match sign_app_idempotently(api, access_token, account).await {
        Ok(a) => Some(a),
        Err(e) => {
            log(format!(
                "账号 {}（{}）：APP 签到失败：{}（继续执行游戏签到）",
                account.name, account.id, e.0
            ));
            app_error = Some(e.0);
            None
        }
    };
    if let Some(a) = &app {
        if a.already_signed == Some(true) {
            log(format!("账号 {}（{}）：APP 签到：今日已签到", account.name, account.id));
        } else {
            log(format!(
                "账号 {}（{}）：APP 签到：获得 {} 金币，{} 经验",
                account.name,
                account.id,
                a.gold_coin.unwrap_or(0.0),
                a.exp.unwrap_or(0.0)
            ));
        }
    }

    // 逐游戏签到：单个游戏失败（非鉴权错误）只记录该游戏失败，不中断其他游戏与后续任务
    let mut game_signins = Vec::new();
    let mut game_failures: Vec<String> = Vec::new();
    for role in &roles {
        let (already, err) = match api.game_signin(access_token, &role.role_id, &role.game_id).await {
            Ok(()) => (false, None),
            Err(e) if is_already_signed(&e.0) => (true, None),
            Err(e) if is_auth_error(&e.0) => return Err(e),
            Err(e) => {
                let label = game_label(&role.game_id, role.game_name.as_deref());
                log(format!(
                    "账号 {}（{}）：游戏 {} / {} 签到失败：{}",
                    account.name,
                    account.id,
                    label,
                    role.role_name.as_deref().unwrap_or(&role.role_id),
                    e.0
                ));
                game_failures.push(format!("{label}: {}", e.0));
                (false, Some(e.0))
            }
        };

        let (days, reward) = if err.is_some() {
            (None, None)
        } else {
            let days = api.get_signin_state(access_token, &role.game_id).await?;
            let rewards = api.get_signin_rewards(access_token, &role.game_id).await?;
            let reward = rewards
                .get((days - 1).max(0) as usize)
                .map(|(n, num)| Reward { name: n.clone(), num: *num });

            log(format!(
                "账号 {}（{}）：游戏 {} / {}：{}，本月第 {} 天{}",
                account.name,
                account.id,
                game_label(&role.game_id, role.game_name.as_deref()),
                role.role_name.as_deref().unwrap_or(&role.role_id),
                if already { "今日已签到" } else { "签到成功" },
                days,
                reward
                    .as_ref()
                    .map(|r| format!("，奖励 {} x{}", r.name, r.num))
                    .unwrap_or_default()
            ));
            (Some(days), reward)
        };

        game_signins.push(GameSigninResult {
            game_id: role.game_id.clone(),
            game_name: role.game_name.clone(),
            role_name: role.role_name.clone().unwrap_or_else(|| role.role_id.clone()),
            days,
            reward,
            already_signed: Some(already),
            success: err.is_none(),
            error: err,
        });
    }

    let mut acc = account.clone();
    if let Some(first) = roles.first() {
        acc.role_id = Some(first.role_id.clone());
        if let Some(rn) = &first.role_name {
            acc.role_name = Some(rn.clone());
        }
    }

    // 金币任务
    let coin_tasks = if opts.coin_tasks {
        run_coin_tasks(api, account, access_token, opts, log).await
    } else {
        None
    };

    // 云异环时长
    let (cloud_duration, cloud_account) = if opts.cloud_duration {
        run_cloud_duration(api, &acc, credential_key, log).await
    } else {
        (None, None)
    };
    if let Some(ca) = cloud_account {
        acc = ca;
    }

    // 状态汇总：全部游戏失败，或 APP 签到失败且无任何游戏成功，才判为 failed；
    // 其余情况视为 success（部分失败信息通过 error 字段透出，供前端展示）。
    let all_games_failed = !game_signins.is_empty() && game_signins.iter().all(|g| !g.success);
    let no_game_succeeded = !game_signins.iter().any(|g| g.success);
    let status = if all_games_failed || (app_error.is_some() && no_game_succeeded) {
        "failed".into()
    } else {
        "success".into()
    };

    let mut failures: Vec<String> = Vec::new();
    if let Some(e) = &app_error {
        failures.push(format!("APP签到：{}", e));
    }
    if !game_failures.is_empty() {
        failures.push(format!("游戏签到：{}", game_failures.join("；")));
    }
    let error = if failures.is_empty() {
        None
    } else {
        Some(failures.join("；"))
    };

    log(format!("账号 {}（{}）：签到流程结束，状态：{}", account.name, account.id, status));

    Ok((
        acc,
        AccountResult {
            id: account.id.clone(),
            name: account.name.clone(),
            status,
            app_signin: app,
            game_signins,
            coin_tasks,
            cloud_duration,
            error,
            skipped_reason: None,
        },
    ))
}

async fn sign_app_idempotently(
    api: &Api,
    access_token: &str,
    account: &Account,
) -> Result<AppSigninResult, ApiError> {
    match api.app_signin(access_token, &account.uid, &account.device_id).await {
        Ok((exp, gold)) => Ok(AppSigninResult {
            already_signed: Some(false),
            exp: Some(exp),
            gold_coin: Some(gold),
        }),
        Err(e) if is_already_signed(&e.0) => Ok(AppSigninResult {
            already_signed: Some(true),
            exp: None,
            gold_coin: None,
        }),
        Err(e) => Err(e),
    }
}

/// 优先使用战绩卡（权威角色映射，修复幽灵角色 5050），为空再回退 getGameRoles。
async fn get_all_game_roles(
    api: &Api,
    access_token: &str,
    uid: &str,
    device_id: &str,
) -> Vec<GameRole> {
    if let Ok(cards) = api.get_game_record_cards(access_token, uid, device_id).await {
        if !cards.is_empty() {
            return cards
                .into_iter()
                .filter(|c| c.role_id.is_some())
                .map(|c| GameRole {
                    game_id: c.game_id,
                    role_id: c.role_id.unwrap(),
                    role_name: c.role_name.or_else(|| c.game_name.clone()),
                    game_name: c.game_name,
                })
                .collect();
        }
    }

    // 回退：逐游戏 getGameRoles
    let mut roles = Vec::new();
    let mut seen = HashSet::new();
    for game_id in crate::constants::GAME_IDS {
        if let Ok(list) = api.get_game_roles(access_token, uid, device_id, game_id).await {
            for r in list {
                if r.role_id.is_empty() || seen.contains(&r.role_id) {
                    continue;
                }
                seen.insert(r.role_id.clone());
                roles.push(GameRole {
                    game_id: game_id.to_string(),
                    role_id: r.role_id,
                    role_name: r.role_name,
                    game_name: None,
                });
            }
        }
    }
    roles
}

async fn run_coin_tasks(
    api: &Api,
    account: &Account,
    access_token: &str,
    opts: &RunOptions,
    log: &mut (dyn FnMut(String) + Send),
) -> Option<CoinTaskResult> {
    let tasks = match api.get_user_tasks(access_token, &account.uid, &account.device_id).await {
        Ok(t) => t,
        Err(e) => {
            log(format!("账号 {}：获取金币任务失败：{}", account.id, e.0));
            return None;
        }
    };

    let bbs_target = remaining(&tasks, "signin_c", 1);
    let browse_target = remaining(&tasks, "browse_post_c", 5);
    let like_target = remaining(&tasks, "like_post_c", 5);
    let share_target = remaining(&tasks, "share", 1);

    let mut result = CoinTaskResult {
        bbs_signin: None,
        browse_done: 0,
        browse_target,
        like_done: 0,
        like_target,
        share_done: 0,
        share_target,
        platform: opts.share_platform.clone(),
        today_coin: None,
        limit_coin: None,
        error: None,
    };

    if bbs_target > 0 {
        match api.bbs_signin(access_token, &account.uid, &account.device_id).await {
            Ok(()) => result.bbs_signin = Some(true),
            Err(e) if is_already_signed(&e.0) => result.bbs_signin = Some(true),
            Err(e) => push_error(&mut result, format!("BBS 签到失败：{}", e.0)),
        }
    } else {
        result.bbs_signin = Some(true);
    }

    let posts = if browse_target > 0 || like_target > 0 || share_target > 0 {
        match api.get_recommend_post_list(access_token, &account.uid, &account.device_id).await {
            Ok(p) => p,
            Err(e) => {
                push_error(&mut result, format!("获取帖子列表失败：{}", e.0));
                return Some(result);
            }
        }
    } else {
        vec![]
    };

    // 浏览
    let mut browsed = Vec::new();
    for post in posts.iter() {
        if result.browse_done >= browse_target {
            break;
        }
        sleep_range(700, 1500).await;
        match api.get_post_full(access_token, &account.uid, &account.device_id, &post.post_id).await {
            Ok(full) => {
                browsed.push(full);
                result.browse_done += 1;
            }
            Err(e) => push_error(&mut result, format!("浏览帖子 {} 失败：{}", post.post_id, e.0)),
        }
    }

    // 点赞
    let mut seen = HashSet::new();
    let like_candidates: Vec<&crate::api::RecommendPost> =
        browsed.iter().chain(posts.iter()).collect();
    for post in like_candidates {
        if result.like_done >= like_target {
            break;
        }
        if !seen.insert(post.post_id.clone()) {
            continue;
        }
        if post.liked == Some(true) {
            continue;
        }
        sleep_range(500, 1000).await;
        match api.like_post(access_token, &account.uid, &account.device_id, &post.post_id).await {
            Ok(()) => result.like_done += 1,
            Err(e) => push_error(&mut result, format!("点赞帖子 {} 失败：{}", post.post_id, e.0)),
        }
    }

    // 分享
    if share_target > 0 {
        let share_post = browsed.first().or_else(|| posts.first());
        if let Some(p) = share_post {
            match api.share_post(access_token, &account.uid, &account.device_id, &p.post_id, &opts.share_platform).await {
                Ok(()) => result.share_done = 1,
                Err(e) => push_error(&mut result, format!("分享帖子 {} 失败：{}", p.post_id, e.0)),
            }
        }
    }

    match api.get_user_coin_task_state(access_token).await {
        Ok(cs) => {
            result.today_coin = cs.today_coin;
            result.limit_coin = cs.limit_coin;
        }
        Err(e) => push_error(&mut result, format!("获取金币状态失败：{}", e.0)),
    }

    log(format!(
        "账号 {}：金币任务：签到{} 浏览{}/{} 点赞{}/{} 分享{}{}",
        account.id,
        if result.bbs_signin == Some(true) { "✓" } else { "×" },
        result.browse_done,
        result.browse_target,
        result.like_done,
        result.like_target,
        if result.share_done >= result.share_target { "✓".to_string() } else { format!("{}/{}", result.share_done, result.share_target) },
        match (result.today_coin, result.limit_coin) {
            (Some(t), Some(l)) => format!(" 今日金币{}/{}", t, l),
            _ => String::new(),
        }
    ));

    Some(result)
}

async fn run_cloud_duration(
    api: &Api,
    account: &Account,
    credential_key: Option<&str>,
    log: &mut (dyn FnMut(String) + Send),
) -> (Option<CloudDurationResult>, Option<Account>) {
    let mut laohu_token = account.laohu_token.clone();
    let mut laohu_user_id = account.laohu_user_id.clone();
    let mut updated_account = None;

    if laohu_token.is_none() || laohu_user_id.is_none() {
        let password = resolve_password(account, credential_key);
        if account.phone.is_none() || password.is_none() {
            let reason = if account.phone.is_none() {
                "账号缺少手机号".to_string()
            } else {
                "缺少密码或密码解密失败".to_string()
            };
            return (
                Some(CloudDurationResult {
                    status: "skipped".into(),
                    gave: None,
                    remained: None,
                    error: None,
                    skipped_reason: Some(reason),
                }),
                None,
            );
        }
        match api
            .login_with_password(
                account.phone.as_deref().unwrap(),
                password.as_deref().unwrap(),
                &account.device_id,
            )
            .await
        {
            Ok((token, user_id)) => {
                laohu_token = Some(token);
                laohu_user_id = Some(user_id);
                let mut acc = account.clone();
                acc.laohu_token = laohu_token.clone();
                acc.laohu_user_id = laohu_user_id.clone();
                acc.token_updated_at = Some(crate::time::shanghai_datetime());
                updated_account = Some(acc);
            }
            Err(e) => {
                return (
                    Some(CloudDurationResult {
                        status: "failed".into(),
                        gave: None,
                        remained: None,
                        error: Some(format!("老虎登录失败：{}", e.0)),
                        skipped_reason: None,
                    }),
                    None,
                );
            }
        }
    }

    match api
        .cloud_get_user_info(
            laohu_token.as_deref().unwrap(),
            laohu_user_id.as_deref().unwrap(),
            &account.device_id,
        )
        .await
    {
        Ok(info) => {
            let text = if info.gave > 0.0 {
                format!("+{} 分钟", info.gave)
            } else {
                "今日已领".to_string()
            };
            let remained = info.remained.map(|r| format!("，剩余 {r} 分钟")).unwrap_or_default();
            log(format!("账号 {}：云异环时长：{}{}", account.id, text, remained));
            (
                Some(CloudDurationResult {
                    status: "success".into(),
                    gave: Some(info.gave),
                    remained: info.remained,
                    error: None,
                    skipped_reason: None,
                }),
                updated_account,
            )
        }
        Err(e) => (
            Some(CloudDurationResult {
                status: "failed".into(),
                gave: None,
                remained: None,
                error: Some(e.0),
                skipped_reason: None,
            }),
            updated_account,
        ),
    }
}

fn resolve_password(account: &Account, credential_key: Option<&str>) -> Option<String> {
    let ep = account.encrypted_password.as_ref()?;
    let ck = credential_key?;
    crate::crypto::decrypt_password(ep, ck).ok()
}

fn with_session(
    account: &Account,
    at: &str,
    rt: &str,
    uid: Option<&str>,
    laohu_token: Option<&str>,
    laohu_user_id: Option<&str>,
) -> Account {
    let mut acc = account.clone();
    if let Some(uid) = uid {
        acc.uid = uid.to_string();
    }
    acc.access_token = Some(at.to_string());
    acc.refresh_token = rt.to_string();
    acc.token_updated_at = Some(crate::time::shanghai_datetime());
    if let Some(t) = laohu_token {
        acc.laohu_token = Some(t.to_string());
    }
    if let Some(u) = laohu_user_id {
        acc.laohu_user_id = Some(u.to_string());
    }
    acc
}

fn remaining(tasks: &[crate::api::CoinTask], code: &str, fallback: i64) -> i64 {
    tasks
        .iter()
        .find(|t| t.code == code)
        .map(|t| (t.limit_times - t.complete_times).max(0))
        .unwrap_or(fallback)
}

fn push_error(result: &mut CoinTaskResult, msg: String) {
    match &result.error {
        Some(e) => result.error = Some(format!("{e}；{msg}")),
        None => result.error = Some(msg),
    }
}

async fn sleep_range(min: u64, max: u64) {
    use rand::Rng;
    let ms = rand::thread_rng().gen_range(min..=max);
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}

/// 生成文本摘要（用于日志展示）。
pub fn summary_text(result: &RunResult) -> String {
    let mut lines = vec![
        "塔吉多每日签到结果".to_string(),
        format!(
            "总账号：{}，成功：{}，失败：{}，跳过：{}",
            result.accounts.len(),
            result.success_count,
            result.failed_count,
            result.skipped_count
        ),
        String::new(),
    ];
    for acc in &result.accounts {
        let status = match acc.status.as_str() {
            "success" => "成功",
            "skipped" => "跳过",
            _ => "失败",
        };
        lines.push(format!("{}（{}）：{}", acc.name, acc.id, status));
        if let Some(app) = &acc.app_signin {
            if app.already_signed == Some(true) {
                lines.push("- APP 签到：今日已签到".into());
            } else {
                lines.push(format!(
                    "- APP 签到：获得 {} 金币，{} 经验",
                    app.gold_coin.unwrap_or(0.0),
                    app.exp.unwrap_or(0.0)
                ));
            }
        }
        for g in &acc.game_signins {
            let reward = g.reward.as_ref().map(|r| format!("，奖励 {} x{}", r.name, r.num)).unwrap_or_default();
            let days = g.days.map(|d| format!("，本月第 {d} 天")).unwrap_or_default();
            let status = if g.already_signed == Some(true) {
                "今日已签到".to_string()
            } else if g.success {
                "签到成功".to_string()
            } else {
                format!("签到失败（{}）", g.error.as_deref().unwrap_or("未知错误"))
            };
            lines.push(format!(
                "- 游戏 {} / {}：{}{}{}",
                game_label(&g.game_id, g.game_name.as_deref()),
                g.role_name,
                status,
                days,
                reward
            ));
        }
        if let Some(ct) = &acc.coin_tasks {
            lines.push(format!(
                "- 金币任务：签到{} 浏览{}/{} 点赞{}/{} 分享{}{}",
                if ct.bbs_signin == Some(true) { "✓" } else { "×" },
                ct.browse_done,
                ct.browse_target,
                ct.like_done,
                ct.like_target,
                if ct.share_done >= ct.share_target { "✓".to_string() } else { format!("{}/{}", ct.share_done, ct.share_target) },
                match (ct.today_coin, ct.limit_coin) {
                    (Some(t), Some(l)) => format!(" 今日金币{}/{}", t, l),
                    _ => String::new(),
                }
            ));
        }
        if let Some(cd) = &acc.cloud_duration {
            lines.push(format_cloud(cd));
        }
        if let Some(e) = &acc.error {
            lines.push(format!("- 失败原因：{e}"));
        }
        if let Some(r) = &acc.skipped_reason {
            lines.push(format!("- 跳过原因：{r}"));
        }
        lines.push(String::new());
    }
    lines.join("\n").trim().to_string()
}

fn format_cloud(cd: &CloudDurationResult) -> String {
    match cd.status.as_str() {
        "skipped" => format!("- 云异环时长：跳过（{}）", cd.skipped_reason.as_deref().unwrap_or("未执行")),
        "failed" => format!("- 云异环时长：失败（{}）", cd.error.as_deref().unwrap_or("未知错误")),
        _ => {
            let gave = cd.gave.unwrap_or(0.0);
            let remained = cd.remained.map(|r| format!("，剩余 {r} 分钟")).unwrap_or_default();
            if gave > 0.0 {
                format!("- 云异环时长：+{gave} 分钟{remained}")
            } else {
                format!("- 云异环时长：今日已领{remained}")
            }
        }
    }
}
