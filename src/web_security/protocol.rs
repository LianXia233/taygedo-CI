//! 应用层安全通信（协议 v2）的线上格式定义。
//!
//! ## 为什么要 v2
//!
//! v1（`X25519 + AES-GCM`，见 git 历史中的 `src/session.rs`）有三个硬伤：
//!
//! 1. **服务端身份未经签名**：客户端拿到的 `identity_pub` 与 `server_pub`
//!    来自同一个未经认证的响应，中间人可以一并替换，形成完整 MITM；
//! 2. **AAD 未绑定方法与路径**：把 `/api/accounts` 的密文搬到
//!    `/api/password` 上仍能通过认证（凭据不会自动生效，但这是不必要的
//!    攻击面，也违反"密文不得跨 API 搬移"的要求）；
//! 3. **依赖 WebCrypto**：纯 HTTP 下 `crypto.subtle` 根本不存在，
//!    实际部署中加密层会整体退化为明文。
//!
//! v2 的做法：
//!
//! - 服务端用 **Ed25519 长期身份密钥**对**完整握手 Transcript** 签名，
//!   客户端先验签再派生会话密钥；
//! - 每条消息的 AAD 绑定 `方法 + 路径 + 版本 + KeyID + SessionID + 序号 +
//!   方向 + nonce`；
//! - 消息加密使用 **XChaCha20-Poly1305**（24 字节随机 nonce），
//!   浏览器侧用内置的 noble 实现，不依赖 `crypto.subtle`。
//!
//! ## 握手 Transcript（字节级定义，前后端必须完全一致）
//!
//! ```text
//! "taygedo/v2/handshake" 0x00
//! client_pub        32
//! client_nonce      32
//! server_eph_pub    32
//! server_nonce      32
//! identity_pub      32
//! sid_len           u8   (sid 为 32 个 ASCII 十六进制字符)
//! sid               sid_len
//! ts                u64 大端（服务端 UNIX 秒）
//! ```
//!
//! 签名 = `Ed25519(identity_sk, transcript)`。任何字段被改动都会导致验签失败。
//!
//! ## 消息 AAD
//!
//! ```text
//! 请求: "taygedo/v2/req" 0x00 || lp(kid) || lp(sid) || seq_be(8)
//!                          || lp(method) || lp(path) || nonce(24)
//! 响应: "taygedo/v2/res" 0x00 || lp(kid) || lp(sid) || req_seq_be(8)
//!                          || res_seq_be(8) || lp(method) || lp(path) || nonce(24)
//! ```
//!
//! `lp(x)` = `u16 大端长度 || x`，避免字段拼接歧义（例如 path 不同但拼接后相同）。

use serde::{Deserialize, Serialize};

/// 协议版本标识。
pub const PROTO: &str = "taygedo/v2";
/// 算法套件标识（仅用于展示与协商校验）。
pub const ALG: &str = "X25519+HKDF-SHA256+XChaCha20-Poly1305";

/// 信封格式版本。
pub const ENVELOPE_VERSION: u8 = 2;

pub const PUBKEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const SID_LEN: usize = 32;
pub const KID_LEN: usize = 16;
/// 加密信封的密文上限（34 MiB，兼容大响应；JSON 外层再留余量）。
pub const MAX_CIPHERTEXT: usize = 34 * 1024 * 1024;

const HANDSHAKE_DOMAIN: &[u8] = b"taygedo/v2/handshake";
const REQ_DOMAIN: &[u8] = b"taygedo/v2/req";
const RES_DOMAIN: &[u8] = b"taygedo/v2/res";

/// HKDF 域分隔标签。
pub const INFO_C2S: &[u8] = b"taygedo/v2/c2s";
pub const INFO_S2C: &[u8] = b"taygedo/v2/s2c";
/// 会话绑定密钥（可用于未来的信道绑定 / 审计 MAC）。
pub const INFO_BIND: &[u8] = b"taygedo/v2/bind";

/// 服务端握手响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandshakeResponse {
    pub ok: bool,
    pub proto: String,
    pub alg: String,
    pub sid: String,
    /// 服务端临时 X25519 公钥。
    pub server_pub: String,
    /// 服务端随机数。
    pub server_nonce: String,
    /// 服务端 Ed25519 长期身份公钥（客户端必须与固定指纹比对）。
    pub identity_pub: String,
    /// 身份密钥 ID（16 个十六进制字符）。
    pub kid: String,
    /// 服务端时间戳（UNIX 秒），参与 Transcript 签名。
    pub ts: u64,
    /// 对完整 Transcript 的 Ed25519 签名（base64url，64 字节）。
    pub sig: String,
    pub expires_in: i64,
}

/// 加密信封（请求与响应共用结构，方向体现在 AAD 与 `rseq` 的有无上）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    /// 信封版本，必须为 2。
    pub v: u8,
    /// 身份 Key ID。
    pub kid: String,
    /// 会话 ID。
    pub sid: String,
    /// 请求序号（请求方向）；响应方向为响应序号。
    pub seq: u64,
    /// 关联的请求序号（仅响应使用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rseq: Option<u64>,
    /// XChaCha20 nonce（base64url，24 字节）。
    pub n: String,
    /// 密文 + Poly1305 tag（base64url）。
    pub ct: String,
}

/// 追加长度前缀字段。
fn push_lp(buf: &mut Vec<u8>, bytes: &[u8]) {
    let n = u16::try_from(bytes.len()).unwrap_or(u16::MAX);
    buf.extend_from_slice(&n.to_be_bytes());
    buf.extend_from_slice(&bytes[..n as usize]);
}

/// 构造握手 Transcript。
#[allow(clippy::too_many_arguments)]
pub fn transcript(
    client_pub: &[u8; PUBKEY_LEN],
    client_nonce: &[u8; PUBKEY_LEN],
    server_pub: &[u8; PUBKEY_LEN],
    server_nonce: &[u8; PUBKEY_LEN],
    identity_pub: &[u8; PUBKEY_LEN],
    sid: &str,
    ts: u64,
) -> Vec<u8> {
    let mut t = Vec::with_capacity(64 + 32 * 5 + sid.len() + 8);
    t.extend_from_slice(HANDSHAKE_DOMAIN);
    t.push(0x00);
    t.extend_from_slice(client_pub);
    t.extend_from_slice(client_nonce);
    t.extend_from_slice(server_pub);
    t.extend_from_slice(server_nonce);
    t.extend_from_slice(identity_pub);
    push_lp(&mut t, sid.as_bytes());
    t.extend_from_slice(&ts.to_be_bytes());
    t
}

/// 构造请求 AAD。
pub fn request_aad(
    kid: &str,
    sid: &str,
    seq: u64,
    method: &str,
    path: &str,
    nonce: &[u8; NONCE_LEN],
) -> Vec<u8> {
    let mut a = Vec::with_capacity(96 + kid.len() + sid.len() + method.len() + path.len());
    a.extend_from_slice(REQ_DOMAIN);
    a.push(0x00);
    push_lp(&mut a, kid.as_bytes());
    push_lp(&mut a, sid.as_bytes());
    a.extend_from_slice(&seq.to_be_bytes());
    push_lp(&mut a, method.as_bytes());
    push_lp(&mut a, path.as_bytes());
    a.extend_from_slice(nonce);
    a
}

/// 构造响应 AAD。
pub fn response_aad(
    kid: &str,
    sid: &str,
    req_seq: u64,
    res_seq: u64,
    method: &str,
    path: &str,
    nonce: &[u8; NONCE_LEN],
) -> Vec<u8> {
    let mut a = Vec::with_capacity(112 + kid.len() + sid.len() + method.len() + path.len());
    a.extend_from_slice(RES_DOMAIN);
    a.push(0x00);
    push_lp(&mut a, kid.as_bytes());
    push_lp(&mut a, sid.as_bytes());
    a.extend_from_slice(&req_seq.to_be_bytes());
    a.extend_from_slice(&res_seq.to_be_bytes());
    push_lp(&mut a, method.as_bytes());
    push_lp(&mut a, path.as_bytes());
    a.extend_from_slice(nonce);
    a
}

/// 校验会话 ID 形态（32 个十六进制字符）。
pub fn valid_sid(s: &str) -> bool {
    s.len() == SID_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// 校验 Key ID 形态（16 个十六进制字符）。
pub fn valid_kid(s: &str) -> bool {
    s.len() == KID_LEN && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_is_field_order_sensitive() {
        let a = [1u8; 32];
        let b = [2u8; 32];
        let t1 = transcript(&a, &b, &a, &b, &a, "sid", 1);
        let t2 = transcript(&b, &a, &a, &b, &a, "sid", 1);
        assert_ne!(t1, t2);
    }

    #[test]
    fn transcript_changes_on_every_field() {
        let base = transcript(
            &[1u8; 32], &[2u8; 32], &[3u8; 32], &[4u8; 32], &[5u8; 32], "ab", 7,
        );
        let variants = [
            transcript(
                &[9u8; 32], &[2u8; 32], &[3u8; 32], &[4u8; 32], &[5u8; 32], "ab", 7,
            ),
            transcript(
                &[1u8; 32], &[9u8; 32], &[3u8; 32], &[4u8; 32], &[5u8; 32], "ab", 7,
            ),
            transcript(
                &[1u8; 32], &[2u8; 32], &[9u8; 32], &[4u8; 32], &[5u8; 32], "ab", 7,
            ),
            transcript(
                &[1u8; 32], &[2u8; 32], &[3u8; 32], &[9u8; 32], &[5u8; 32], "ab", 7,
            ),
            transcript(
                &[1u8; 32], &[2u8; 32], &[3u8; 32], &[4u8; 32], &[9u8; 32], "ab", 7,
            ),
            transcript(
                &[1u8; 32], &[2u8; 32], &[3u8; 32], &[4u8; 32], &[5u8; 32], "ac", 7,
            ),
            transcript(
                &[1u8; 32], &[2u8; 32], &[3u8; 32], &[4u8; 32], &[5u8; 32], "ab", 8,
            ),
        ];
        for v in variants {
            assert_ne!(base, v);
        }
    }

    #[test]
    fn aad_binds_method_and_path() {
        let n = [0u8; NONCE_LEN];
        let a = request_aad("kid", "sid", 1, "POST", "/api/password", &n);
        let b = request_aad("kid", "sid", 1, "POST", "/api/accounts", &n);
        let c = request_aad("kid", "sid", 1, "GET", "/api/password", &n);
        assert_ne!(a, b);
        assert_ne!(a, c);
        // 长度前缀防止 "a"+"bc" 与 "ab"+"c" 撞车
        let d = request_aad("ki", "d", 1, "POST", "/api/password", &n);
        assert_ne!(a, d);
    }

    #[test]
    fn aad_binds_sequence_and_direction() {
        let n = [0u8; NONCE_LEN];
        assert_ne!(
            request_aad("k", "s", 1, "GET", "/x", &n),
            request_aad("k", "s", 2, "GET", "/x", &n)
        );
        assert_ne!(
            request_aad("k", "s", 1, "GET", "/x", &n),
            response_aad("k", "s", 1, 1, "GET", "/x", &n)
        );
        assert_ne!(
            response_aad("k", "s", 1, 1, "GET", "/x", &n),
            response_aad("k", "s", 2, 1, "GET", "/x", &n)
        );
    }

    #[test]
    fn sid_and_kid_shape() {
        assert!(valid_sid(&"a".repeat(32)));
        assert!(!valid_sid(&"a".repeat(31)));
        assert!(!valid_sid(&"z".repeat(32)));
        assert!(valid_kid(&"0".repeat(16)));
        assert!(!valid_kid(&"0".repeat(15)));
    }
}
