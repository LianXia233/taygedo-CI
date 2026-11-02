//! 会话状态与生命周期规则。
//!
//! 一个会话 = 一把 c2s 密钥 + 一把 s2c 密钥 + 一个请求方向的重放窗口 +
//! 一个响应方向序号。三者互不共享，任一处串用都会立刻 AEAD 失败。
//!
//! 生命周期参数集中在这里，是为了让"会话多久失效、能发多少请求、最多并存
//! 几个"这类安全上限只有一个出处，不散落在中间件与业务代码里。

use zeroize::Zeroizing;

use super::replay::ReplayWindow;

/// 会话有效期（秒）。超时必须重新握手。
pub const SESSION_TTL_SECS: i64 = 30 * 60;
/// 单会话最大请求数。
///
/// 上限的作用是限制**同一把密钥的加密次数**（降低 nonce 复用与密钥磨损面），
/// 同时给出一个自然的"该重新握手了"的时机。20 万次远高于正常客户端使用量。
pub const SESSION_MAX_REQUESTS: u64 = 200_000;
/// 同时存在的会话上限。超出时淘汰最旧的，避免内存被握手洪水打爆。
pub const MAX_SESSIONS: usize = 256;

/// 单个会话（服务端视角）。
pub struct Session {
    pub sid: String,
    /// 客户端 → 服务端 密钥。
    pub c2s: Zeroizing<[u8; 32]>,
    /// 服务端 → 客户端 密钥。
    pub s2c: Zeroizing<[u8; 32]>,
    /// 会话绑定密钥（保留：未来做信道绑定 / 审计 MAC）。
    #[allow(dead_code)]
    pub bind: Zeroizing<[u8; 32]>,
    /// 创建时刻（unix 秒）。
    pub created_at: i64,
    /// 已接受的请求数。
    pub requests: u64,
    /// 请求方向防重放窗口。
    pub recv: ReplayWindow,
    /// 响应方向序号（独立计数，与请求序号无关）。
    pub send_seq: u64,
    /// 是否已通过口令登录（用于审计与后续策略）。
    pub authenticated: bool,
    /// 来源标识（IP）。仅供诊断（"这个会话来自哪"），日志默认不输出。
    #[allow(dead_code)]
    pub peer: String,
}

impl Session {
    /// 是否已过期。
    pub fn is_expired(&self, now: i64) -> bool {
        now - self.created_at >= SESSION_TTL_SECS
    }
}
