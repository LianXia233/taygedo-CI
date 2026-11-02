#![cfg_attr(feature = "openwrt", allow(dead_code))]
//! 应用层安全通信层（协议 v2）。
//!
//! ## 结构
//!
//! ```text
//! web_security/
//!   ├── protocol.rs   线上格式：握手 Transcript、信封、AAD 构造
//!   ├── identity.rs   Ed25519 长期身份（签名 / 验签 / 指纹）
//!   ├── cipher.rs     HKDF-SHA256 + XChaCha20-Poly1305 薄封装
//!   ├── replay.rs     会话级防重放滑动窗口
//!   ├── session.rs    会话状态与生命周期上限
//!   ├── error.rs      CryptoError 与 HTTP 状态/对外文案映射
//!   ├── manifest.rs   静态资源签名清单
//!   ├── middleware.rs 加密中间件（请求解密 / 响应加密的唯一入口）
//!   └── mod.rs        CryptoManager：握手、会话表、加解密编排
//! ```
//!
//! ## 一次完整交互
//!
//! ```text
//! 浏览器                                          服务端
//!   │ 1. POST /api/crypto/handshake                 │
//!   │    {proto, client_pub, client_nonce}          │
//!   │ ───────────────────────────────────────────▶  │ 生成临时 X25519 密钥对
//!   │                                               │ shared = ECDH(eph_sk, client_pub)
//!   │                                               │ transcript = T(...)
//!   │                                               │ sig = Ed25519(identity_sk, transcript)
//!   │ ◀───────────────────────────────────────────  │ {sid, server_pub, identity_pub, ts, sig, ...}
//!   │ 2. 用**固定/带外核对过**的 identity_pub 验签    │
//!   │    salt = SHA256(transcript)                  │
//!   │    c2s/s2c = HKDF(shared, salt, info)         │
//!   │                                               │
//!   │ 3. POST /api/accounts   {v,kid,sid,seq,n,ct}  │
//!   │    AAD = f(method, path, sid, seq, nonce)     │
//!   │ ───────────────────────────────────────────▶  │ 查会话 → 查重放 → 解密 → 业务
//!   │ ◀───────────────────────────────────────────  │ 加密响应（同一 AAD 语义 + req_seq）
//! ```
//!
//! ## 与业务层的关系
//!
//! 加解密全部发生在中间件里，业务 handler 只看到明文 JSON。因此
//! **API 路径、请求参数、响应结构完全不变**，外部 API 客户端与 LuCI 视图
//! 不需要改动任何数据结构。

pub mod cipher;
pub mod error;
pub mod identity;
pub mod manifest;
#[cfg(feature = "webui")]
pub mod middleware;
pub mod protocol;
pub mod replay;
pub mod session;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use zeroize::Zeroizing;

#[cfg(feature = "webui")]
pub use error::handshake_error_body;
pub use error::CryptoError;
pub use session::{MAX_SESSIONS, SESSION_MAX_REQUESTS, SESSION_TTL_SECS};

use identity::ServerIdentity;
use protocol::{Envelope, HandshakeResponse, MAX_CIPHERTEXT, NONCE_LEN, SID_LEN};
use replay::ReplayWindow;
use session::Session;

/// 握手限速：窗口秒数与窗口内上限（按来源 IP）。
const HS_WINDOW_SECS: i64 = 60;
const HS_MAX_PER_WINDOW: u32 = 60;
/// 握手产物。
pub struct HandshakeOutcome {
    pub response: HandshakeResponse,
}

/// 会话管理器。
pub struct CryptoManager {
    identity: ServerIdentity,
    sessions: Mutex<HashMap<String, Session>>,
    handshake_rate: Mutex<HashMap<String, (u32, i64)>>,
}

impl CryptoManager {
    /// 由长期身份种子构造。
    pub fn new(identity_seed: &[u8; 32]) -> Self {
        Self {
            identity: ServerIdentity::from_seed(identity_seed),
            sessions: Mutex::new(HashMap::new()),
            handshake_rate: Mutex::new(HashMap::new()),
        }
    }

    /// 服务端身份公钥（可公开；客户端必须与带外核对的指纹比对）。
    pub fn identity_public(&self) -> [u8; 32] {
        self.identity.public_key()
    }

    /// 身份 Key ID。
    pub fn identity_kid(&self) -> &str {
        self.identity.kid()
    }

    /// 身份指纹（32 个十六进制字符），用于带外固定。
    pub fn identity_fingerprint(&self) -> String {
        self.identity.fingerprint()
    }

    /// 身份对象（供签名清单构造使用）。
    pub fn identity(&self) -> &ServerIdentity {
        &self.identity
    }

    /// 当前活跃会话数（诊断用；测试与未来的 /api/secure/stats 使用）。
    #[allow(dead_code)]
    pub fn active_sessions(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    /// 握手限速检查。
    pub fn allow_handshake(&self, peer: &str) -> bool {
        let now = now_unix();
        let mut rate = self.handshake_rate.lock().unwrap();
        rate.retain(|_, (_, start)| now - *start < HS_WINDOW_SECS * 4);
        let entry = rate.entry(peer.to_string()).or_insert((0, now));
        if now - entry.1 >= HS_WINDOW_SECS {
            *entry = (1, now);
            return true;
        }
        entry.0 += 1;
        entry.0 <= HS_MAX_PER_WINDOW
    }

    /// 执行握手。
    ///
    /// 服务端生成**临时** X25519 密钥对（前向保密），用长期 Ed25519 身份
    /// 对完整 Transcript 签名。客户端验签通过后才能派生会话密钥。
    pub fn handshake(
        &self,
        client_pub: &[u8; 32],
        client_nonce: &[u8; 32],
        peer: &str,
    ) -> Result<HandshakeOutcome, CryptoError> {
        let eph_secret = crate::protocol::x25519_generate_secret();
        let server_pub = crate::protocol::x25519_public_from_secret(&eph_secret);

        let shared = crate::protocol::x25519_diffie_hellman(&eph_secret, client_pub)
            .ok_or(CryptoError::HandshakeFailed)?;

        let mut server_nonce = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut server_nonce);

        // 会话 ID：16 字节随机 → 32 个十六进制字符
        let mut sid_raw = [0u8; SID_LEN / 2];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut sid_raw);
        let sid = sid_raw
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();

        let ts = now_unix().max(0) as u64;
        let identity_pub = self.identity.public_key();
        let transcript = protocol::transcript(
            client_pub,
            client_nonce,
            &server_pub,
            &server_nonce,
            &identity_pub,
            &sid,
            ts,
        );
        let signature = self.identity.sign(&transcript);

        // 派生：salt = SHA-256(transcript) —— 把整个握手上下文绑进密钥
        use sha2::{Digest, Sha256};
        let salt = Sha256::digest(&transcript);
        let c2s = cipher::hkdf32(&shared, &salt, protocol::INFO_C2S);
        let s2c = cipher::hkdf32(&shared, &salt, protocol::INFO_S2C);
        let bind = cipher::hkdf32(&shared, &salt, protocol::INFO_BIND);

        {
            let mut map = self.sessions.lock().unwrap();
            let now = now_unix();
            map.retain(|_, s| !s.is_expired(now));
            if map.len() >= MAX_SESSIONS {
                // 淘汰最旧的一个，保证内存上界（不影响正在用的其它会话）
                if let Some(oldest) = map
                    .iter()
                    .min_by_key(|(_, s)| s.created_at)
                    .map(|(k, _)| k.clone())
                {
                    map.remove(&oldest);
                }
            }
            map.insert(
                sid.clone(),
                Session {
                    sid: sid.clone(),
                    c2s,
                    s2c,
                    bind,
                    created_at: now,
                    requests: 0,
                    recv: ReplayWindow::new(),
                    send_seq: 1,
                    authenticated: false,
                    peer: peer.to_string(),
                },
            );
        }

        Ok(HandshakeOutcome {
            response: HandshakeResponse {
                ok: true,
                proto: protocol::PROTO.to_string(),
                alg: protocol::ALG.to_string(),
                sid,
                server_pub: b64(&server_pub),
                server_nonce: b64(&server_nonce),
                identity_pub: b64(&identity_pub),
                kid: self.identity.kid().to_string(),
                ts,
                sig: b64(&signature),
                expires_in: SESSION_TTL_SECS,
            },
        })
    }

    /// 解析并校验信封的外层字段（不含解密）。
    fn parse_envelope(
        &self,
        raw: &[u8],
    ) -> Result<(Envelope, [u8; NONCE_LEN], Vec<u8>), CryptoError> {
        if raw.len() > MAX_CIPHERTEXT * 2 {
            return Err(CryptoError::Malformed);
        }
        let env: Envelope = serde_json::from_slice(raw).map_err(|_| CryptoError::Malformed)?;
        if env.v != protocol::ENVELOPE_VERSION {
            return Err(CryptoError::Unsupported);
        }
        if !protocol::valid_kid(&env.kid) || env.kid != self.identity.kid() {
            // 未知 Key ID 一律拒绝：不做"用当前密钥试一试"的降级
            return Err(CryptoError::Unsupported);
        }
        if !protocol::valid_sid(&env.sid) {
            return Err(CryptoError::Malformed);
        }
        let nonce_raw = URL_SAFE_NO_PAD
            .decode(env.n.as_bytes())
            .map_err(|_| CryptoError::Malformed)?;
        if nonce_raw.len() != NONCE_LEN {
            return Err(CryptoError::Malformed);
        }
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&nonce_raw);
        let ct = URL_SAFE_NO_PAD
            .decode(env.ct.as_bytes())
            .map_err(|_| CryptoError::Malformed)?;
        if ct.len() > MAX_CIPHERTEXT {
            return Err(CryptoError::Malformed);
        }
        Ok((env, nonce, ct))
    }

    /// 解密一次请求。
    ///
    /// `method` / `path` 来自 HTTP 请求本身，参与 AAD，因此密文无法被搬到
    /// 别的接口或改方法重放。
    pub fn decrypt_request(
        &self,
        method: &str,
        path: &str,
        raw: &[u8],
    ) -> Result<(String, u64, Zeroizing<Vec<u8>>), CryptoError> {
        let (env, nonce, ct) = self.parse_envelope(raw)?;

        let aad = protocol::request_aad(&env.kid, &env.sid, env.seq, method, path, &nonce);

        let mut map = self.sessions.lock().unwrap();
        let s = map.get_mut(&env.sid).ok_or(CryptoError::NoSession)?;

        if env.rseq.is_some() {
            // 请求方向不允许携带 rseq
            return Err(CryptoError::Malformed);
        }
        if s.is_expired(now_unix()) {
            map.remove(&env.sid);
            return Err(CryptoError::NoSession);
        }
        if s.requests >= SESSION_MAX_REQUESTS {
            return Err(CryptoError::Exhausted);
        }
        if s.recv.is_duplicate(env.seq) {
            return Err(CryptoError::Replay);
        }

        let pt = cipher::open(&s.c2s, &nonce, &aad, &ct).map_err(|_| CryptoError::AuthFailed)?;

        // 认证成功后才消费序号：伪造报文不能挤掉合法序号
        if !s.recv.accept(env.seq) {
            return Err(CryptoError::Replay);
        }
        s.requests += 1;
        let sid = s.sid.clone();
        Ok((sid, env.seq, pt))
    }

    /// 加密一次响应。
    pub fn encrypt_response(
        &self,
        sid: &str,
        method: &str,
        path: &str,
        req_seq: u64,
        body: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let mut map = self.sessions.lock().unwrap();
        let s = map.get_mut(sid).ok_or(CryptoError::NoSession)?;
        let res_seq = s.send_seq;
        let nonce = cipher::random_nonce();
        let kid = self.identity.kid().to_string();
        let aad = protocol::response_aad(&kid, sid, req_seq, res_seq, method, path, &nonce);
        let ct = cipher::seal(&s.s2c, &nonce, &aad, body).map_err(|_| CryptoError::AuthFailed)?;
        s.send_seq += 1;

        let env = Envelope {
            v: protocol::ENVELOPE_VERSION,
            kid,
            sid: sid.to_string(),
            seq: res_seq,
            rseq: Some(req_seq),
            n: b64(&nonce),
            ct: URL_SAFE_NO_PAD.encode(&ct),
        };
        serde_json::to_vec(&env).map_err(|_| CryptoError::Malformed)
    }

    /// 标记会话已完成口令登录。
    pub fn mark_authenticated(&self, sid: &str) {
        let mut map = self.sessions.lock().unwrap();
        if let Some(s) = map.get_mut(sid) {
            s.authenticated = true;
        }
    }

    /// 销毁单个会话（精细登出 / 异常会话隔离）。
    #[allow(dead_code)]
    pub fn destroy(&self, sid: &str) {
        self.sessions.lock().unwrap().remove(sid);
    }

    /// 销毁全部会话（改密 / 登出 / 策略变更后强制重新握手）。
    pub fn destroy_all(&self) -> usize {
        let mut map = self.sessions.lock().unwrap();
        let n = map.len();
        map.clear();
        n
    }

    /// 销毁除 `keep` 之外的全部会话。
    ///
    /// 用途：`/api/config` 修改加密策略时不能把发起本次请求的会话也销毁，
    /// 否则响应无法加密返回，客户端会误以为操作失败。
    pub fn destroy_all_except(&self, keep: &str) -> usize {
        let mut map = self.sessions.lock().unwrap();
        let before = map.len();
        map.retain(|sid, _| sid == keep);
        before - map.len()
    }
}

/// base64url（无填充）编码。
pub fn b64(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// base64url（无填充）解码（变长）。
#[allow(dead_code)]
pub fn b64_decode(s: &str) -> Result<Vec<u8>, CryptoError> {
    URL_SAFE_NO_PAD
        .decode(s.trim().as_bytes())
        .map_err(|_| CryptoError::Malformed)
}

/// 解码定长字段。
pub fn decode_fixed<const N: usize>(s: &str) -> Result<[u8; N], CryptoError> {
    let raw = URL_SAFE_NO_PAD
        .decode(s.trim().as_bytes())
        .map_err(|_| CryptoError::Malformed)?;
    if raw.len() != N {
        return Err(CryptoError::Malformed);
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&raw);
    Ok(out)
}

/// 服务端时间（UNIX 秒）。
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
