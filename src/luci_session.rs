//! LuCI / ubus 会话校验（OpenWrt 集成模式专用）。
//!
//! ## 为什么由服务端自己校验
//!
//! OpenWrt 集成模式下内置 WebUI 已退役：LuCI 页面（uhttpd 上的原生 JS 视图）
//! 直接跨端口调用本服务的 REST API。登录与身份由 OpenWrt 自己的 LuCI 会话
//! 负责，本服务**不再签发、也不再校验自有口令/Token**。
//!
//! 页面在每次请求上带 `X-TGD-Luci-Session`（取自 LuCI 的 `L.env.sessionid`），
//! 本模块通过本机 ubus 的 `session get` 让 rpcd 判定该会话是否真实有效。
//! 这样"能否访问"完全取决于 OpenWrt 的会话状态：用户在 LuCI 注销/会话过期后，
//! 后端 API 立即拒绝，无需另行维护第二套凭据。
//!
//! ## 边界
//!
//! - 该路径只在 OpenWrt 集成模式下启用；独立部署（Windows/服务器/Docker）
//!   仍走自有的 Bearer Token 登录。
//! - 校验失败一律**拒绝**（fail-closed）：ubus 不可用、超时、返回非 JSON
//!   都视为会话无效，不做"看起来能用就放行"的降级。
//! - 结果做短 TTL 缓存：避免每个 API 请求都 fork 一次 ubus。有效会话缓存 30s，
//!   无效结果缓存 3s（防止无效会话把 ubus 打满）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 单次 ubus 调用超时。路由器负载高时 ubus 仍应在数十毫秒内返回。
const UBUS_TIMEOUT: Duration = Duration::from_secs(2);

/// 有效会话的缓存时长。
const TTL_VALID: Duration = Duration::from_secs(30);

/// 无效会话的缓存时长（短缓存，避免高频无效请求反复 fork ubus）。
const TTL_INVALID: Duration = Duration::from_secs(3);

/// 缓存条目上限，超出时整体清空（会话数量天然很小，不必做 LRU）。
const CACHE_LIMIT: usize = 128;

type Cache = Mutex<HashMap<String, (bool, Instant)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 会话号形态校验。
///
/// rpcd 的 `ubus_rpc_session` 是十六进制串；这里只做"可安全拼进 ubus JSON
/// 参数"的宽松校验（长度 + ASCII 字母数字），不锁死具体长度，避免上游格式
/// 变化导致全部请求被拒。
pub fn is_well_formed(sid: &str) -> bool {
    let n = sid.len();
    (8..=128).contains(&n) && sid.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// 校验 LuCI 会话是否有效。
///
/// 仅 OpenWrt 集成模式调用。内部走 `ubus call session get`，由 rpcd 判定。
pub async fn validate(sid: &str) -> bool {
    if !is_well_formed(sid) {
        return false;
    }

    if let Some(hit) = cached(sid) {
        return hit;
    }

    let sid_owned = sid.to_string();
    let ok = match tokio::time::timeout(
        UBUS_TIMEOUT,
        tokio::task::spawn_blocking(move || call_ubus_session_get(&sid_owned)),
    )
    .await
    {
        Ok(Ok(v)) => v,
        // 超时 / 线程 panic：一律按无效处理（fail-closed）
        _ => false,
    };

    store(sid, ok);
    ok
}

fn cached(sid: &str) -> Option<bool> {
    let guard = cache().lock().ok()?;
    let (ok, at) = guard.get(sid)?;
    let ttl = if *ok { TTL_VALID } else { TTL_INVALID };
    if at.elapsed() <= ttl {
        Some(*ok)
    } else {
        None
    }
}

fn store(sid: &str, ok: bool) {
    if let Ok(mut guard) = cache().lock() {
        if guard.len() >= CACHE_LIMIT {
            guard.clear();
        }
        guard.insert(sid.to_string(), (ok, Instant::now()));
    }
}

/// 执行 `ubus call session get`，判断会话是否由 rpcd 认可。
///
/// ubus 二进制名可由 `TAYGEDO_UBUS_BIN` 指定（默认 `ubus`，走 PATH，
/// 不硬编码固件绝对路径）。
fn call_ubus_session_get(sid: &str) -> bool {
    let bin = std::env::var("TAYGEDO_UBUS_BIN").unwrap_or_else(|_| "ubus".to_string());
    let arg = format!("{{\"ubus_rpc_session\":\"{sid}\"}}");
    let out = std::process::Command::new(bin)
        .args(["call", "session", "get", &arg])
        .output();
    match out {
        Ok(o) if o.status.success() => parse_session_get(&String::from_utf8_lossy(&o.stdout)),
        _ => false,
    }
}

/// 解析 `session get` 的输出。
///
/// 有效会话返回形如 `{"username":"root","timeout":900, ...}`；
/// 未知会话由 rpcd 返回错误（非零退出码或错误 JSON），不予认可。
pub fn parse_session_get(stdout: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout) else {
        return false;
    };
    matches!(v.get("username").and_then(|u| u.as_str()), Some(u) if !u.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_sid() {
        assert!(is_well_formed("0123456789abcdef0123456789abcdef"));
        assert!(!is_well_formed(""));
        assert!(!is_well_formed("short"));
        assert!(!is_well_formed("0123456789abcdef\"}")); // 不允许注入字符
        assert!(!is_well_formed(&"a".repeat(129)));
    }

    #[test]
    fn session_get_output_parsing() {
        assert!(parse_session_get(r#"{"username":"root","timeout":900}"#));
        assert!(!parse_session_get(r#"{"username":""}"#));
        assert!(!parse_session_get(r#"{"error":"unknown session"}"#));
        assert!(!parse_session_get("not json"));
        assert!(!parse_session_get(""));
    }
}
