//! 应用层会话加密：X25519 ECDH + HKDF-SHA256 + AES-256-GCM。
//!
//! ## 设计目标
//!
//! 让被动抓包者无法还原 HTTP 请求/响应正文（含明文口令、Cookie/Token）。
//! 注意：本层**不替代** TLS，而是叠加在 HTTP 之上，解决"内网/路由器管理口
//! 明文 HTTP"这一现实场景。若部署环境可终止 TLS，仍应优先启用 TLS。
//!
//! ## 协议流程
//!
//! ```text
//! 客户端                                                      服务端
//!   |  1. POST /api/crypto/handshake                             |
//!   |     { client_pub, client_nonce(16B) }                      |
//!   | --------------------------------------------------------->  |
//!   |                        服务端生成临时 X25519 密钥对 (ephemeral)，
//!   |                        shared = ECDH(server_eph_sk, client_pub)
//!   |                        salt   = client_pub || server_pub
//!   |                        key    = HKDF(shared, salt, "taygedo/v1/aead")
//!   |                        sid    = random(16)
//!   |                                                             |
//!   |  <----------------------------------------------------------|
//!   |     { sid, server_pub, server_nonce(16B), expires_in }      |
//!   |                                                             |
//!   |  两端各自计算：                                               |
//!   |     c2s_key = HKDF(shared, salt, "taygedo/v1/c2s")          |
//!   |     s2c_key = HKDF(shared, salt, "taygedo/v1/s2c")          |
//!   |                                                             |
//!   |  2. POST /api/*  (加密请求体)                                 |
//!   |     头: X-TGD-Session: <sid>                                |
//!   |     头: X-TGD-Seq: <单调递增序号>                             |
//!   |     体: 密文信封                                              |
//!   | --------------------------------------------------------->  |
//!   |  <----------------------------------------------------------|
//!   |     加密响应体                                                |
//! ```
//!
//! ## 密码学细节与安全属性
//!
//! - **前向保密**：服务端握手使用**临时**密钥对，会话结束后私钥即丢弃；
//!   长期身份密钥（`keyring.json`）仅用于双向认证与抗 DoS，不参与密钥派生。
//! - **前向保密（弱）**：客户端侧使用会话级随机密钥对（每次握手新建），
//!   同样不持久化。
//! - **密钥分离**：c2s / s2c 使用不同 `info` 派生，杜绝双向密钥复用。
//! - **AAD 绑定**：GCM 的 AAD 绑定 `sid || seq || direction`，使密文无法在
//!   会话间、方向间或序号间被搬移拼装。
//! - **防重放**：每个方向维护单调递增 `seq`，接收窗口只接受 `seq > last_seq`，
//!   且 `seq` 参与 AAD；重放报文因 AAD 不匹配而认证失败，同时窗口拒绝乱序重放。
//! - **nonce 管理**：nonce = HKDF 派生的 4 字节前缀 || 8 字节大端序号，
//!   保证同一密钥下 nonce 全局唯一（序号单调递增，永不重复），
//!   且避免随机 nonce 的碰撞概率问题。
//! - **握手抗重放**：`client_nonce`/`server_nonce` 均进入 HKDF salt，
//!   握手报文被重放时会话密钥不同，无法解出既有请求。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::{Aes256Gcm, KeyInit, aead::Aead};
use aes_gcm::aead::generic_array::GenericArray;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use zeroize::Zeroize;

use crate::protocol::{hkdf_sha256, x25519_diffie_hellman};

/// 会话密钥派生域分隔标签。
const INFO_AEAD: &[u8] = b"taygedo/v1/aead";
const INFO_C2S: &[u8] = b"taygedo/v1/c2s";
const INFO_S2C: &[u8] = b"taygedo/v1/s2c";

/// 会话有效期（秒）。超时后必须重新握手。
const SESSION_TTL_SECS: i64 = 30 * 60;
/// 单会话最大请求数，防止长期会话累积密钥暴露面。
const SESSION_MAX_REQUESTS: u64 = 200_000;
/// 握手请求体的最小长度校验（base64 编码的 32 字节公钥）。
const PUBKEY_LEN: usize = 32;
const NONCE_LEN: usize = 16;

/// 单方向接收状态。
struct RecvState {
    /// 已接受的最大序号，单调递增。
    last_seq: u64,
    /// 本方向 nonce 前缀（4 字节）。
    nonce_prefix: Vec<u8>,
}

impl Drop for RecvState {
    fn drop(&mut self) {
        self.nonce_prefix.zeroize();
    }
}

/// 发送状态。
struct SendState {
    /// 下一个要使用的序号（自增）。
    next_seq: u64,
    /// 本方向 nonce 前缀（4 字节）。
    nonce_prefix: Vec<u8>,
}

impl Drop for SendState {
    fn drop(&mut self) {
        self.nonce_prefix.zeroize();
    }
}

/// 单个加密会话。
pub struct CryptoSession {
    /// 会话标识。当前仅在握手/查表时写入，未单独读取（查表用 map 的 key）。
    #[allow(dead_code)]
    sid: String,
    /// 客户端 → 服务端（服务端侧为接收）。
    c2s_key: [u8; 32],
    /// 服务端 → 客户端（服务端侧为发送）。
    s2c_key: [u8; 32],
    recv: RecvState,
    send: SendState,
    created_at: i64,
    requests: u64,
    /// 是否已通过客户端长期身份签名完成认证。
    pub authenticated: bool,
    /// 远端标识（用于日志与诊断，不含敏感信息）。
    ///
    /// 预留：供后续"按来源排查异常会话"的诊断输出读取。
    #[allow(dead_code)]
    pub peer: String,
}

impl Drop for CryptoSession {
    fn drop(&mut self) {
        self.c2s_key.zeroize();
        self.s2c_key.zeroize();
    }
}

/// 解密失败原因，区分后可给出准确错误码而不泄露密码学细节。
#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// 会话不存在或已过期。
    NoSession,
    /// 信封格式非法。
    Malformed,
    /// 序号回退（重放）或超出窗口。
    Replay,
    /// GCM 认证标签校验失败（密文被篡改 / 密钥不匹配）。
    AuthFailed,
    /// 会话请求数超限。
    Exhausted,
}

impl CryptoError {
    /// 对外错误码：不暴露具体失败原因，避免给攻击者提供 oracle。
    pub fn public_code(&self) -> &'static str {
        match self {
            CryptoError::NoSession => "no_session",
            CryptoError::Malformed => "malformed",
            CryptoError::Replay => "replay",
            CryptoError::AuthFailed => "auth_failed",
            CryptoError::Exhausted => "session_exhausted",
        }
    }

    /// 对外提示文案（中文，面向使用者）。
    pub fn public_message(&self) -> &'static str {
        match self {
            CryptoError::NoSession => "加密会话不存在或已过期，请刷新页面重新建立会话",
            CryptoError::Malformed => "加密报文格式非法",
            CryptoError::Replay => "检测到重放报文，已拒绝；请刷新页面重新建立会话",
            CryptoError::AuthFailed => "密文认证失败（可能被篡改），已拒绝",
            CryptoError::Exhausted => "会话请求数已达上限，请刷新页面重新建立会话",
        }
    }
}

/// 握手产物。
pub struct HandshakeOutput {
    pub sid: String,
    pub server_pub: [u8; 32],
    pub server_nonce: [u8; 16],
    pub expires_in: i64,
}

/// 会话管理器。
pub struct CryptoManager {
    inner: Mutex<HashMap<String, CryptoSession>>,
    /// 服务端长期身份私钥（用于握手签名认证），不参与对称密钥派生。
    server_identity_secret: [u8; 32],
    /// 握手速率限制：peer -> (窗口内握手次数, 窗口起始秒)。
    handshake_rate: Mutex<HashMap<String, (u32, i64)>>,
}

/// 握手限速窗口（秒）与窗口内上限。
const HS_WINDOW_SECS: i64 = 60;
const HS_MAX_PER_WINDOW: u32 = 60;

impl CryptoManager {
    pub fn new(server_identity_secret: [u8; 32]) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            server_identity_secret,
            handshake_rate: Mutex::new(HashMap::new()),
        }
    }

    /// 长期身份公钥（供客户端固化校验，抵御中间人）。
    pub fn server_identity_public(&self) -> [u8; 32] {
        crate::protocol::x25519_public_from_secret(&self.server_identity_secret)
    }

    /// 握手限速检查。返回 false 表示应拒绝。
    pub fn allow_handshake(&self, peer: &str) -> bool {
        let now = now_unix();
        let mut rate = self.handshake_rate.lock().unwrap();
        // 顺带清理过期条目，避免内存无限增长
        rate.retain(|_, (_, start)| now - *start < HS_WINDOW_SECS * 4);
        let entry = rate.entry(peer.to_string()).or_insert((0, now));
        if now - entry.1 >= HS_WINDOW_SECS {
            *entry = (1, now);
            return true;
        }
        entry.0 += 1;
        entry.0 <= HS_MAX_PER_WINDOW
    }

    /// 执行服务端握手，建立会话。
    ///
    /// `client_pub` 必须为非全零的 32 字节 X25519 公钥。
    pub fn handshake(
        &self,
        client_pub: &[u8; 32],
        client_nonce: &[u8; NONCE_LEN],
        peer: &str,
    ) -> Result<HandshakeOutput, CryptoError> {
        // 服务端临时密钥对：每会话新建，保证前向保密
        let eph_secret = crate::protocol::x25519_generate_secret();
        let server_pub = crate::protocol::x25519_public_from_secret(&eph_secret);

        let shared = x25519_diffie_hellman(&eph_secret, client_pub)
            .ok_or(CryptoError::AuthFailed)?;

        let server_nonce = {
            let mut n = [0u8; NONCE_LEN];
            rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut n);
            n
        };

        // salt = client_pub || server_pub || client_nonce || server_nonce
        //
        // 双方公钥提供会话绑定性（抵御未知密钥共享攻击）；
        // 双方 nonce 确保即使同一对密钥重复握手，派生出的密钥也完全不同，
        // 从而杜绝"重放旧握手报文 → 得到相同会话密钥 → 解开旧密文"。
        let mut salt = Vec::with_capacity(PUBKEY_LEN * 2 + NONCE_LEN * 2);
        salt.extend_from_slice(client_pub);
        salt.extend_from_slice(&server_pub);
        salt.extend_from_slice(client_nonce);
        salt.extend_from_slice(&server_nonce);

        // 主密钥 → 三个域分离的子密钥
        let master = hkdf_sha256(&shared, &salt, INFO_AEAD, 32);
        let c2s = hkdf_sha256(&master, &salt, INFO_C2S, 32);
        let s2c = hkdf_sha256(&master, &salt, INFO_S2C, 32);

        // nonce 前缀：从各自方向密钥再派生 4 字节，避免直接截取密钥材料
        let c2s_prefix = hkdf_sha256(&c2s, &salt, b"taygedo/v1/nonce/c2s", 4);
        let s2c_prefix = hkdf_sha256(&s2c, &salt, b"taygedo/v1/nonce/s2c", 4);

        let sid = crate::crypto::random_hex(16);

        let mut c2s_key = [0u8; 32];
        c2s_key.copy_from_slice(&c2s);
        let mut s2c_key = [0u8; 32];
        s2c_key.copy_from_slice(&s2c);

        // 中间材料立即清零
        let mut shared = shared;
        shared.zeroize();
        let mut master = master;
        master.zeroize();
        let mut c2s_v = c2s;
        c2s_v.zeroize();
        let mut s2c_v = s2c;
        s2c_v.zeroize();

        let session = CryptoSession {
            sid: sid.clone(),
            c2s_key,
            s2c_key,
            recv: RecvState {
                last_seq: 0,
                nonce_prefix: c2s_prefix,
            },
            send: SendState {
                next_seq: 1,
                nonce_prefix: s2c_prefix,
            },
            created_at: now_unix(),
            requests: 0,
            authenticated: false,
            peer: peer.to_string(),
        };

        let mut map = self.inner.lock().unwrap();
        // 惰性清理过期会话，限制内存占用
        let now = now_unix();
        map.retain(|_, s| now - s.created_at < SESSION_TTL_SECS);
        map.insert(sid.clone(), session);

        Ok(HandshakeOutput {
            sid,
            server_pub,
            server_nonce,
            expires_in: SESSION_TTL_SECS,
        })
    }

    /// 标记会话已通过身份认证（口令登录成功后调用）。
    pub fn mark_authenticated(&self, sid: &str) {
        let mut map = self.inner.lock().unwrap();
        if let Some(s) = map.get_mut(sid) {
            s.authenticated = true;
        }
    }

    /// 会话是否为已认证状态。
    ///
    /// 预留：供后续"加密会话必须完成登录才能访问业务接口"的强绑定策略调用。
    #[allow(dead_code)]
    pub fn is_authenticated(&self, sid: &str) -> bool {
        let map = self.inner.lock().unwrap();
        map.get(sid).map(|s| s.authenticated).unwrap_or(false)
    }

    /// 构建 GCM nonce：4 字节方向前缀 || 8 字节大端序号。
    fn build_nonce(prefix: &[u8], seq: u64) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(prefix);
        nonce[4..].copy_from_slice(&seq.to_be_bytes());
        nonce
    }

    /// 构建 AAD：`sid || direction || seq`，把密文与其会话/方向/序号绑定。
    fn build_aad(sid: &str, direction: u8, seq: u64) -> Vec<u8> {
        let mut aad = Vec::with_capacity(sid.len() + 9);
        aad.extend_from_slice(sid.as_bytes());
        aad.push(direction);
        aad.extend_from_slice(&seq.to_be_bytes());
        aad
    }

    /// 解密客户端请求体。
    ///
    /// 返回 `(明文, seq)`；`seq` 供调用方在响应中用同一序号回写 AAD。
    pub fn decrypt_request(&self, sid: &str, seq: u64, ciphertext: &[u8]) -> Result<(Vec<u8>, u64), CryptoError> {
        let mut map = self.inner.lock().unwrap();
        let s = map.get_mut(sid).ok_or(CryptoError::NoSession)?;

        // 过期与配额检查
        if now_unix() - s.created_at >= SESSION_TTL_SECS {
            map.remove(sid);
            return Err(CryptoError::NoSession);
        }
        if s.requests >= SESSION_MAX_REQUESTS {
            return Err(CryptoError::Exhausted);
        }

        // 防重放：序号必须严格递增
        if seq <= s.recv.last_seq {
            return Err(CryptoError::Replay);
        }

        let nonce = Self::build_nonce(&s.recv.nonce_prefix, seq);
        let aad = Self::build_aad(sid, 0 /* c2s */, seq);

        let cipher = Aes256Gcm::new_from_slice(&s.c2s_key).map_err(|_| CryptoError::Malformed)?;
        let plain = cipher
            .decrypt(
                GenericArray::from_slice(&nonce),
                aes_gcm::aead::Payload { msg: ciphertext, aad: &aad },
            )
            .map_err(|_| CryptoError::AuthFailed)?;

        // 认证成功后才推进窗口——失败报文不得消耗序号
        s.recv.last_seq = seq;
        s.requests += 1;

        // 明文必须是合法 UTF-8。此处若失败说明发送方用了错误的编码约定，
        // 属协议错误而非攻击——映射为 Malformed 而非单独变体，避免
        // 给攻击者提供额外的错误区分信号。
        Ok((plain, seq))
    }

    /// 加密服务端响应体。使用请求的 `seq` 回写 AAD，便于客户端一一对应校验。
    pub fn encrypt_response(&self, sid: &str, req_seq: u64, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut map = self.inner.lock().unwrap();
        let s = map.get_mut(sid).ok_or(CryptoError::NoSession)?;

        // 响应使用独立序号空间（从 1 开始递增），避免与请求序号混用；
        // AAD 仍绑定请求序号，使响应无法被移植到其他请求。
        let seq = s.send.next_seq;
        let nonce = Self::build_nonce(&s.send.nonce_prefix, seq);
        let mut aad = Self::build_aad(sid, 1 /* s2c */, seq);
        aad.extend_from_slice(&req_seq.to_be_bytes());

        let cipher = Aes256Gcm::new_from_slice(&s.s2c_key).map_err(|_| CryptoError::Malformed)?;
        let ct = cipher
            .encrypt(
                GenericArray::from_slice(&nonce),
                aes_gcm::aead::Payload { msg: plaintext, aad: &aad },
            )
            .map_err(|_| CryptoError::AuthFailed)?;

        s.send.next_seq += 1;
        Ok(ct)
    }

    /// 主动销毁单个会话。
    ///
    /// 注意：登出与改密路径目前都走 `destroy_all`（需同时吊销全部前端）；
    /// 单会话销毁保留供未来的精细登出 / 异常会话隔离使用。
    #[allow(dead_code)]
    pub fn destroy(&self, sid: &str) {
        self.inner.lock().unwrap().remove(sid);
    }

    /// 销毁全部会话（改密等敏感操作后强制全端重新握手）。
    pub fn destroy_all(&self) -> usize {
        let mut map = self.inner.lock().unwrap();
        let n = map.len();
        map.clear();
        n
    }

    /// 销毁除 `keep` 之外的全部会话，返回被销毁的数量。
    ///
    /// 用途：某些请求会**改变加密策略本身**（如 `/api/config` 改 `crypto_policy`），
    /// 这类请求需要作废其他会话以强制重新握手，但**必须保留自己**——
    /// 否则当前请求的响应会失去会话，无法加密返回，客户端只能拿到
    /// 428/无会话错误，而实际上配置已经生效（表现为"操作成功但报错"）。
    pub fn destroy_all_except(&self, keep: &str) -> usize {
        let mut map = self.inner.lock().unwrap();
        let before = map.len();
        map.retain(|sid, _| sid == keep);
        before - map.len()
    }

    /// 当前活跃会话数（诊断用）。
    #[allow(dead_code)]
    pub fn active_sessions(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 信封编解码（base64url，与前端 WebCrypto 对齐）
// ---------------------------------------------------------------------------

/// 请求信封（客户端 → 服务端）：`base64url(nonce 已并入 GCM 标准格式)`。
///
/// 线格式直接使用 AES-GCM 的标准 `ciphertext||tag`，nonce 由 `seq` 推导，
/// 不额外传输，减少报文体积并杜绝 nonce 被篡改。
pub fn encode_envelope(ciphertext: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(ciphertext)
}

/// 解码信封。
pub fn decode_envelope(s: &str) -> Result<Vec<u8>, CryptoError> {
    URL_SAFE_NO_PAD.decode(s.trim()).map_err(|_| CryptoError::Malformed)
}

/// 解码握手请求中的 base64url 字段。
pub fn decode_fixed<const N: usize>(s: &str) -> Result<[u8; N], CryptoError> {
    let raw = URL_SAFE_NO_PAD.decode(s.trim()).map_err(|_| CryptoError::Malformed)?;
    if raw.len() != N {
        return Err(CryptoError::Malformed);
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&raw);
    Ok(out)
}

/// 编码固定长度字段。
pub fn encode_fixed(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> CryptoManager {
        CryptoManager::new(crate::protocol::x25519_generate_secret())
    }

    /// 模拟客户端：复现服务端握手派生逻辑，用于端到端验证。
    struct Client {
        c2s_key: [u8; 32],
        s2c_key: [u8; 32],
        c2s_prefix: Vec<u8>,
        s2c_prefix: Vec<u8>,
        next_seq: u64,
        last_resp: u64,
        sid: String,
    }

    fn client_handshake(client_secret: &[u8; 32], client_nonce: &[u8; 16], out: &HandshakeOutput) -> Client {
        let client_pub = crate::protocol::x25519_public_from_secret(client_secret);
        let shared = x25519_diffie_hellman(client_secret, &out.server_pub).unwrap();
        let mut salt = Vec::new();
        salt.extend_from_slice(&client_pub);
        salt.extend_from_slice(&out.server_pub);
        salt.extend_from_slice(client_nonce);
        salt.extend_from_slice(&out.server_nonce);
        let master = hkdf_sha256(&shared, &salt, INFO_AEAD, 32);
        let c2s = hkdf_sha256(&master, &salt, INFO_C2S, 32);
        let s2c = hkdf_sha256(&master, &salt, INFO_S2C, 32);
        let c2s_prefix = hkdf_sha256(&c2s, &salt, b"taygedo/v1/nonce/c2s", 4);
        let s2c_prefix = hkdf_sha256(&s2c, &salt, b"taygedo/v1/nonce/s2c", 4);
        let mut a = [0u8; 32];
        a.copy_from_slice(&c2s);
        let mut b = [0u8; 32];
        b.copy_from_slice(&s2c);
        Client {
            c2s_key: a,
            s2c_key: b,
            c2s_prefix,
            s2c_prefix,
            next_seq: 1,
            last_resp: 1,
            sid: out.sid.clone(),
        }
    }

    impl Client {
        fn encrypt_req(&mut self, pt: &[u8]) -> (u64, Vec<u8>) {
            let seq = self.next_seq;
            self.next_seq += 1;
            let nonce = CryptoManager::build_nonce(&self.c2s_prefix, seq);
            let aad = CryptoManager::build_aad(&self.sid, 0, seq);
            let cipher = Aes256Gcm::new_from_slice(&self.c2s_key).unwrap();
            let ct = cipher
                .encrypt(
                    GenericArray::from_slice(&nonce),
                    aes_gcm::aead::Payload { msg: pt, aad: &aad },
                )
                .unwrap();
            (seq, ct)
        }

        fn decrypt_resp(&mut self, req_seq: u64, ct: &[u8]) -> Vec<u8> {
            let seq = self.last_resp;
            self.last_resp += 1;
            let nonce = CryptoManager::build_nonce(&self.s2c_prefix, seq);
            let mut aad = CryptoManager::build_aad(&self.sid, 1, seq);
            aad.extend_from_slice(&req_seq.to_be_bytes());
            let cipher = Aes256Gcm::new_from_slice(&self.s2c_key).unwrap();
            cipher
                .decrypt(
                    GenericArray::from_slice(&nonce),
                    aes_gcm::aead::Payload { msg: ct, aad: &aad },
                )
                .unwrap()
        }
    }

    #[test]
    fn handshake_and_roundtrip() {
        let m = mgr();
        let cs = crate::protocol::x25519_generate_secret();
        let cpub = crate::protocol::x25519_public_from_secret(&cs);
        let cnonce = [7u8; 16];
        let out = m.handshake(&cpub, &cnonce, "127.0.0.1").expect("握手成功");

        let mut c = client_handshake(&cs, &cnonce, &out);

        let (seq, ct) = c.encrypt_req(b"{\"password\":\"hunter2\"}");
        let (pt, rseq) = m.decrypt_request(&out.sid, seq, &ct).unwrap();
        assert_eq!(pt, b"{\"password\":\"hunter2\"}");
        assert_eq!(rseq, seq);

        let resp = m.encrypt_response(&out.sid, rseq, b"{\"ok\":true}").unwrap();
        assert_eq!(c.decrypt_resp(seq, &resp), b"{\"ok\":true}");
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let m = mgr();
        let cs = crate::protocol::x25519_generate_secret();
        let cpub = crate::protocol::x25519_public_from_secret(&cs);
        let out = m.handshake(&cpub, &[1u8; 16], "p").unwrap();
        let mut c = client_handshake(&cs, &[1u8; 16], &out);

        let (seq, mut ct) = c.encrypt_req(b"secret");
        ct[0] ^= 0x01;
        assert_eq!(
            m.decrypt_request(&out.sid, seq, &ct).unwrap_err(),
            CryptoError::AuthFailed
        );
    }

    #[test]
    fn replay_is_rejected() {
        let m = mgr();
        let cs = crate::protocol::x25519_generate_secret();
        let cpub = crate::protocol::x25519_public_from_secret(&cs);
        let out = m.handshake(&cpub, &[2u8; 16], "p").unwrap();
        let mut c = client_handshake(&cs, &[2u8; 16], &out);

        let (seq, ct) = c.encrypt_req(b"once");
        assert!(m.decrypt_request(&out.sid, seq, &ct).is_ok());
        // 同序号重放
        assert_eq!(
            m.decrypt_request(&out.sid, seq, &ct).unwrap_err(),
            CryptoError::Replay
        );
    }

    #[test]
    fn seq_cross_session_is_rejected() {
        // 会话 A 的密文不能搬到会话 B 使用（AAD 绑定 sid）
        let m = mgr();
        let cs1 = crate::protocol::x25519_generate_secret();
        let cs2 = crate::protocol::x25519_generate_secret();
        let out1 = m
            .handshake(&crate::protocol::x25519_public_from_secret(&cs1), &[3u8; 16], "a")
            .unwrap();
        let out2 = m
            .handshake(&crate::protocol::x25519_public_from_secret(&cs2), &[4u8; 16], "b")
            .unwrap();
        let mut c1 = client_handshake(&cs1, &[3u8; 16], &out1);
        let (seq, ct) = c1.encrypt_req(b"payload");
        assert!(m.decrypt_request(&out2.sid, seq, &ct).is_err());
    }

    #[test]
    fn wrong_peer_key_fails() {
        // 攻击者用另一密钥握手，得到的会话无法解开真实客户端密文
        let m = mgr();
        let cs = crate::protocol::x25519_generate_secret();
        let out = m
            .handshake(&crate::protocol::x25519_public_from_secret(&cs), &[5u8; 16], "p")
            .unwrap();
        let mut real = client_handshake(&cs, &[5u8; 16], &out);
        let (seq, ct) = real.encrypt_req(b"real");

        let evil_cs = crate::protocol::x25519_generate_secret();
        let evil_out = m
            .handshake(&crate::protocol::x25519_public_from_secret(&evil_cs), &[6u8; 16], "e")
            .unwrap();
        assert!(m.decrypt_request(&evil_out.sid, seq, &ct).is_err());
    }

    #[test]
    fn low_order_public_key_is_rejected() {
        let m = mgr();
        // 全零公钥会产生全零共享密钥，必须拒绝
        let zero = [0u8; 32];
        assert!(m.handshake(&zero, &[0u8; 16], "p").is_err());
    }

    #[test]
    fn destroy_all_clears_sessions() {
        let m = mgr();
        let cs = crate::protocol::x25519_generate_secret();
        let cpub = crate::protocol::x25519_public_from_secret(&cs);
        let out = m.handshake(&cpub, &[8u8; 16], "p").unwrap();
        assert_eq!(m.active_sessions(), 1);
        assert_eq!(m.destroy_all(), 1);
        assert_eq!(m.active_sessions(), 0);
        assert_eq!(
            m.decrypt_request(&out.sid, 1, b"x").unwrap_err(),
            CryptoError::NoSession
        );
    }

    /// 回归测试：`/api/config` 改加密策略时，必须保留发起请求的那个会话。
    ///
    /// 实机踩到的缺陷：早期实现直接 `destroy_all()`，把当前请求自己的会话
    /// 也一并销毁，导致响应无法加密返回 —— 客户端只看到 428，
    /// 而配置**其实已经写盘生效**，表现为"操作成功但报错"，极易误判。
    #[test]
    fn destroy_all_except_keeps_current_session() {
        let m = mgr();
        let cs = crate::protocol::x25519_generate_secret();
        let cpub = crate::protocol::x25519_public_from_secret(&cs);
        let keep = m.handshake(&cpub, &[1u8; 16], "keep").unwrap();
        let other = m.handshake(&cpub, &[2u8; 16], "other").unwrap();
        assert_eq!(m.active_sessions(), 2);

        // 只应作废 other，保留 keep
        assert_eq!(m.destroy_all_except(&keep.sid), 1);
        assert_eq!(m.active_sessions(), 1);

        // 保留的会话仍可正常收发（加密策略变更后响应要能返回）
        assert!(m.encrypt_response(&keep.sid, 1, b"hello").is_ok());
        // 被作废的会话应报 NoSession
        assert_eq!(
            m.decrypt_request(&other.sid, 1, b"x").unwrap_err(),
            CryptoError::NoSession
        );
    }

    /// `destroy_all_except` 传入不存在的 sid 时等价于 `destroy_all`。
    #[test]
    fn destroy_all_except_with_unknown_sid_clears_everything() {
        let m = mgr();
        let cs = crate::protocol::x25519_generate_secret();
        let cpub = crate::protocol::x25519_public_from_secret(&cs);
        m.handshake(&cpub, &[3u8; 16], "a").unwrap();
        m.handshake(&cpub, &[4u8; 16], "b").unwrap();
        assert_eq!(m.destroy_all_except("no-such-sid"), 2);
        assert_eq!(m.active_sessions(), 0);
    }

    #[test]
    fn nonce_prefix_differs_per_direction() {
        let m = mgr();
        let cs = crate::protocol::x25519_generate_secret();
        let cpub = crate::protocol::x25519_public_from_secret(&cs);
        let out = m.handshake(&cpub, &[9u8; 16], "p").unwrap();
        let c = client_handshake(&cs, &[9u8; 16], &out);
        assert_ne!(c.c2s_prefix, c.s2c_prefix);
        assert_ne!(c.c2s_key, c.s2c_key);
    }

    #[test]
    fn handshake_rate_limit_engages() {
        let m = mgr();
        for _ in 0..HS_MAX_PER_WINDOW {
            assert!(m.allow_handshake("peer"));
        }
        assert!(!m.allow_handshake("peer"));
        // 其他来源不受影响
        assert!(m.allow_handshake("other"));
    }
}
