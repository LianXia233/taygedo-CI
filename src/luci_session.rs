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
//! - 校验有并发上限，并且到点**强制终止** ubus 子进程：`Command::output()` 会一直
//!   阻塞到子进程自行退出，而 `spawn_blocking` 一旦开始就无法取消。只靠 `timeout`
//!   丢弃句柄的话，ubus 卡死时阻塞线程与子进程会持续堆积，因此这两条都要做。

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;

/// 单次 ubus 调用超时。路由器负载高时 ubus 仍应在数十毫秒内返回。
const UBUS_TIMEOUT: Duration = Duration::from_secs(2);

/// 同时在跑的 ubus 校验上限。
///
/// 拿不到许可时直接按无效处理（fail-closed），不做排队——排队会让请求方在
/// 超时之后才开始校验，等于把压力延后放大。上限本身要足够小，避免大量伪造
/// 会话号把路由器的进程/线程资源打满。
const UBUS_CONCURRENCY: usize = 4;

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

fn permits() -> &'static Semaphore {
    static PERMITS: OnceLock<Semaphore> = OnceLock::new();
    PERMITS.get_or_init(|| Semaphore::new(UBUS_CONCURRENCY))
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

    // 并发闸门：拿不到许可就直接按无效处理（fail-closed），不做排队。
    // 许可被移进阻塞任务内部，任务彻底结束才释放，因此即使外层已超时，
    // 仍在收尾的 ubus 调用依然受 UBUS_CONCURRENCY 约束。
    let permit = match tokio::time::timeout(UBUS_TIMEOUT, permits().acquire()).await {
        Ok(Ok(p)) => p,
        _ => return false,
    };

    let sid_owned = sid.to_string();
    let handle = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        call_ubus_session_get(&sid_owned)
    });

    let ok = match tokio::time::timeout(UBUS_TIMEOUT, handle).await {
        Ok(Ok(v)) => v,
        // 超时 / 线程 panic：一律按无效处理（fail-closed）。
        // 阻塞任务本身无法取消，但它会在内部到点杀掉 ubus 子进程后自行退出。
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
///
/// 不使用 `Command::output()`：它会一直阻塞到子进程自行退出，ubus 卡死时
/// 阻塞线程永远收不回来。这里自己轮询 `try_wait`，到 `UBUS_TIMEOUT` 仍未退出
/// 就 `kill()` + `wait()` 回收，保证阻塞任务在有界时间内结束。
fn call_ubus_session_get(sid: &str) -> bool {
    let bin = std::env::var("TAYGEDO_UBUS_BIN").unwrap_or_else(|_| "ubus".to_string());
    let arg = format!("{{\"ubus_rpc_session\":\"{sid}\"}}");
    let mut child = match Command::new(bin)
        .args(["call", "session", "get", &arg])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let deadline = Instant::now() + UBUS_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return false,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            // 超时或 try_wait 出错：强制终止并回收，绝不让子进程留在后台。
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }

    match child.wait_with_output() {
        Ok(o) => parse_session_get(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => false,
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
