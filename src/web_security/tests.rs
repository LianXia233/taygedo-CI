//! 应用层安全通信测试。
//!
//! 测试里实现了一个**客户端模拟器**，严格按协议重建 Transcript、派生密钥、
//! 组装信封。它与浏览器端 `static/taygedo-crypto.js` 使用同一份字节级
//! 定义（见 `protocol.rs` 的文档注释），因此能真实覆盖：
//! 验签、密钥派生、AAD 绑定、防重放、防跨接口搬移。
//!
//! 这不是"只测加密工具函数"：`CryptoManager` 的握手 / 解密 / 加密
//! 全部走线上路径。

use super::*;
use crate::crypto_storage::envelope;
use crate::web_security::protocol::{request_aad, response_aad, transcript, Envelope, NONCE_LEN};
use sha2::{Digest, Sha256};

/// 客户端模拟器（浏览器侧协议的等价实现）。
struct Client {
    sid: String,
    kid: String,
    c2s: [u8; 32],
    s2c: [u8; 32],
    next_seq: u64,
}

impl Client {
    /// 按协议完成握手（含验签）。
    fn connect(mgr: &CryptoManager) -> Option<Client> {
        let secret = crate::protocol::x25519_generate_secret();
        let pubkey = crate::protocol::x25519_public_from_secret(&secret);
        let cnonce = [0x33u8; 32];
        let out = mgr.handshake(&pubkey, &cnonce, "127.0.0.1").ok()?;
        let r = &out.response;

        let server_pub = decode_fixed::<32>(&r.server_pub).unwrap();
        let server_nonce = decode_fixed::<32>(&r.server_nonce).unwrap();
        let identity_pub = decode_fixed::<32>(&r.identity_pub).unwrap();
        let sig = decode_fixed::<64>(&r.sig).unwrap();

        let t = transcript(
            &pubkey,
            &cnonce,
            &server_pub,
            &server_nonce,
            &identity_pub,
            &r.sid,
            r.ts,
        );
        // 客户端必须先验签，失败即中止
        assert!(
            identity::verify(&identity_pub, &t, &sig),
            "客户端验签必须通过"
        );

        let shared = crate::protocol::x25519_diffie_hellman(&secret, &server_pub).unwrap();
        let salt = Sha256::digest(&t);
        let c2s = cipher::hkdf32(&shared, &salt, protocol::INFO_C2S);
        let s2c = cipher::hkdf32(&shared, &salt, protocol::INFO_S2C);

        Some(Client {
            sid: r.sid.clone(),
            kid: r.kid.clone(),
            c2s: *c2s,
            s2c: *s2c,
            next_seq: 1,
        })
    }

    fn encrypt_req(&mut self, method: &str, path: &str, plaintext: &[u8]) -> (u64, Vec<u8>) {
        let seq = self.next_seq;
        self.next_seq += 1;
        let nonce = [seq as u8; NONCE_LEN];
        let aad = request_aad(&self.kid, &self.sid, seq, method, path, &nonce);
        let ct = cipher::seal(&self.c2s, &nonce, &aad, plaintext).unwrap();
        let env = Envelope {
            v: protocol::ENVELOPE_VERSION,
            kid: self.kid.clone(),
            sid: self.sid.clone(),
            seq,
            rseq: None,
            n: b64(&nonce),
            ct: b64(&ct),
        };
        (seq, serde_json::to_vec(&env).unwrap())
    }

    fn decrypt_resp(&self, method: &str, path: &str, req_seq: u64, raw: &[u8]) -> Option<Vec<u8>> {
        let env: Envelope = serde_json::from_slice(raw).ok()?;
        if env.rseq != Some(req_seq) {
            return None;
        }
        let nonce_raw = decode_fixed::<NONCE_LEN>(&env.n).ok()?;
        let aad = response_aad(
            &self.kid, &self.sid, req_seq, env.seq, method, path, &nonce_raw,
        );
        let ct = b64_decode(&env.ct).ok()?;
        cipher::open(&self.s2c, &nonce_raw, &aad, &ct)
            .ok()
            .map(|v| v.to_vec())
    }
}

fn mgr() -> CryptoManager {
    CryptoManager::new(&[0x2Bu8; 32])
}

#[test]
fn handshake_signature_verifies_and_roundtrip_works() {
    let m = mgr();
    let mut c = Client::connect(&m).expect("握手应成功");

    let (seq, env) = c.encrypt_req("POST", "/api/login", br#"{"password":"x"}"#);
    let (sid, rseq, pt) = m.decrypt_request("POST", "/api/login", &env).unwrap();
    assert_eq!(sid, c.sid);
    assert_eq!(rseq, seq);
    assert_eq!(pt.as_slice(), br#"{"password":"x"}"#);

    let resp = m
        .encrypt_response(&sid, "POST", "/api/login", rseq, br#"{"ok":true}"#)
        .unwrap();
    assert_eq!(
        c.decrypt_resp("POST", "/api/login", seq, &resp).unwrap(),
        br#"{"ok":true}"#
    );
}

#[test]
fn server_public_key_substitution_is_detected() {
    // 中间人把 server_pub 换成自己的：客户端重建的 Transcript 与原签名不符
    let m = mgr();
    let secret = crate::protocol::x25519_generate_secret();
    let pubkey = crate::protocol::x25519_public_from_secret(&secret);
    let cnonce = [1u8; 32];
    let out = m.handshake(&pubkey, &cnonce, "p").unwrap();
    let r = &out.response;

    let identity_pub = decode_fixed::<32>(&r.identity_pub).unwrap();
    let sig = decode_fixed::<64>(&r.sig).unwrap();
    let server_nonce = decode_fixed::<32>(&r.server_nonce).unwrap();

    let evil_pub =
        crate::protocol::x25519_public_from_secret(&crate::protocol::x25519_generate_secret());
    let t = transcript(
        &pubkey,
        &cnonce,
        &evil_pub,
        &server_nonce,
        &identity_pub,
        &r.sid,
        r.ts,
    );
    assert!(!identity::verify(&identity_pub, &t, &sig));
}

#[test]
fn identity_public_key_substitution_is_detected() {
    // 中间人整体换身份：签名对不上
    let m = mgr();
    let secret = crate::protocol::x25519_generate_secret();
    let pubkey = crate::protocol::x25519_public_from_secret(&secret);
    let out = m.handshake(&pubkey, &[2u8; 32], "p").unwrap();
    let r = &out.response;

    let server_pub = decode_fixed::<32>(&r.server_pub).unwrap();
    let server_nonce = decode_fixed::<32>(&r.server_nonce).unwrap();
    let sig = decode_fixed::<64>(&r.sig).unwrap();
    let evil_identity = crate::protocol::x25519_generate_secret();

    let t = transcript(
        &pubkey,
        &[2u8; 32],
        &server_pub,
        &server_nonce,
        &evil_identity,
        &r.sid,
        r.ts,
    );
    assert!(!identity::verify(&evil_identity, &t, &sig));
}

#[test]
fn tampered_transcript_fields_fail_verification() {
    let m = mgr();
    let secret = crate::protocol::x25519_generate_secret();
    let pubkey = crate::protocol::x25519_public_from_secret(&secret);
    let out = m.handshake(&pubkey, &[3u8; 32], "p").unwrap();
    let r = &out.response;

    let sp = decode_fixed::<32>(&r.server_pub).unwrap();
    let sn = decode_fixed::<32>(&r.server_nonce).unwrap();
    let ip = decode_fixed::<32>(&r.identity_pub).unwrap();
    let sig = decode_fixed::<64>(&r.sig).unwrap();

    let t = transcript(&pubkey, &[3u8; 32], &sp, &sn, &ip, &r.sid, r.ts + 1);
    assert!(!identity::verify(&ip, &t, &sig), "改 ts 必须验签失败");
    let t2 = transcript(
        &pubkey,
        &[3u8; 32],
        &sp,
        &sn,
        &ip,
        "00000000000000000000000000000000",
        r.ts,
    );
    assert!(!identity::verify(&ip, &t2, &sig), "改 sid 必须验签失败");
}

#[test]
fn ciphertext_bound_to_method_and_path() {
    let m = mgr();
    let mut c = Client::connect(&m).unwrap();
    let (_, env) = c.encrypt_req("POST", "/api/password", b"secret");

    // 同一份密文搬到别的路径 / 改方法，都必须认证失败
    assert_eq!(
        m.decrypt_request("POST", "/api/accounts", &env)
            .unwrap_err(),
        CryptoError::AuthFailed
    );
    assert_eq!(
        m.decrypt_request("GET", "/api/password", &env).unwrap_err(),
        CryptoError::AuthFailed
    );
}

#[test]
fn ciphertext_tamper_is_rejected() {
    let m = mgr();
    let mut c = Client::connect(&m).unwrap();
    let (_, raw) = c.encrypt_req("GET", "/api/logs", b"");
    let mut env: Envelope = serde_json::from_slice(&raw).unwrap();
    let mut ct = b64_decode(&env.ct).unwrap();
    ct[0] ^= 0xff;
    env.ct = b64(&ct);
    let raw2 = serde_json::to_vec(&env).unwrap();
    assert_eq!(
        m.decrypt_request("GET", "/api/logs", &raw2).unwrap_err(),
        CryptoError::AuthFailed
    );
}

#[test]
fn replay_same_envelope_is_rejected() {
    let m = mgr();
    let mut c = Client::connect(&m).unwrap();
    let (_, env) = c.encrypt_req("GET", "/api/accounts", b"");
    assert!(m.decrypt_request("GET", "/api/accounts", &env).is_ok());
    assert_eq!(
        m.decrypt_request("GET", "/api/accounts", &env).unwrap_err(),
        CryptoError::Replay
    );
}

#[test]
fn cross_session_envelope_is_rejected() {
    let m = mgr();
    let mut a = Client::connect(&m).unwrap();
    let b = Client::connect(&m).unwrap();
    let (_, env) = a.encrypt_req("GET", "/api/accounts", b"");
    let mut e: Envelope = serde_json::from_slice(&env).unwrap();
    e.sid = b.sid.clone();
    let raw = serde_json::to_vec(&e).unwrap();
    assert!(matches!(
        m.decrypt_request("GET", "/api/accounts", &raw),
        Err(CryptoError::NoSession) | Err(CryptoError::AuthFailed)
    ));
}

#[test]
fn unknown_key_id_and_version_are_rejected() {
    let m = mgr();
    let mut c = Client::connect(&m).unwrap();
    let (_, raw) = c.encrypt_req("GET", "/api/accounts", b"");

    let mut e: Envelope = serde_json::from_slice(&raw).unwrap();
    e.kid = "0000000000000000".into();
    assert_eq!(
        m.decrypt_request("GET", "/api/accounts", &serde_json::to_vec(&e).unwrap())
            .unwrap_err(),
        CryptoError::Unsupported
    );

    let mut e2: Envelope = serde_json::from_slice(&raw).unwrap();
    e2.v = 1;
    assert_eq!(
        m.decrypt_request("GET", "/api/accounts", &serde_json::to_vec(&e2).unwrap())
            .unwrap_err(),
        CryptoError::Unsupported
    );
}

#[test]
fn malformed_bodies_and_plaintext_downgrade_are_rejected() {
    let m = mgr();
    let _c = Client::connect(&m).unwrap();
    for body in [&b"not json"[..], &b"{}"[..], &b"{\"v\":2}"[..]] {
        assert_eq!(
            m.decrypt_request("GET", "/api/accounts", body).unwrap_err(),
            CryptoError::Malformed
        );
    }
    // 明文 JSON（降级请求）不可能被当成信封解析成功
    assert!(m
        .decrypt_request("POST", "/api/login", br#"{"username":"a","password":"b"}"#)
        .is_err());
}

#[test]
fn response_is_bound_to_request_sequence() {
    let m = mgr();
    let mut c = Client::connect(&m).unwrap();
    let (seq, env) = c.encrypt_req("GET", "/api/logs", b"");
    let (sid, rseq, _) = m.decrypt_request("GET", "/api/logs", &env).unwrap();
    let resp = m
        .encrypt_response(&sid, "GET", "/api/logs", rseq, b"[]")
        .unwrap();

    assert!(c.decrypt_resp("GET", "/api/logs", seq + 1, &resp).is_none());
    assert!(c.decrypt_resp("GET", "/api/accounts", seq, &resp).is_none());
    assert_eq!(
        c.decrypt_resp("GET", "/api/logs", seq, &resp).unwrap(),
        b"[]"
    );
}

#[test]
fn concurrent_out_of_order_requests_are_accepted() {
    let m = mgr();
    let mut c = Client::connect(&m).unwrap();
    let (_, e1) = c.encrypt_req("GET", "/api/a", b"1");
    let (_, e2) = c.encrypt_req("GET", "/api/b", b"2");
    // 乱序到达（并发场景）
    assert!(m.decrypt_request("GET", "/api/b", &e2).is_ok());
    assert!(m.decrypt_request("GET", "/api/a", &e1).is_ok());
    // 重放仍然被拒
    assert_eq!(
        m.decrypt_request("GET", "/api/a", &e1).unwrap_err(),
        CryptoError::Replay
    );
}

#[test]
fn destroyed_session_rejects_all_further_requests() {
    let m = mgr();
    let mut c = Client::connect(&m).unwrap();
    let (_, e1) = c.encrypt_req("GET", "/api/a", b"1");
    assert!(m.decrypt_request("GET", "/api/a", &e1).is_ok());
    m.destroy_all();
    let (_, e2) = c.encrypt_req("GET", "/api/a", b"2");
    assert_eq!(
        m.decrypt_request("GET", "/api/a", &e2).unwrap_err(),
        CryptoError::NoSession
    );
}

#[test]
fn destroy_all_except_keeps_current_session() {
    let m = mgr();
    let mut keep = Client::connect(&m).unwrap();
    let _other = Client::connect(&m).unwrap();
    assert_eq!(m.active_sessions(), 2);
    assert_eq!(m.destroy_all_except(&keep.sid), 1);
    let (_, e) = keep.encrypt_req("GET", "/api/config", b"");
    assert!(m.decrypt_request("GET", "/api/config", &e).is_ok());
}

#[test]
fn session_keys_are_isolated_and_direction_separated() {
    let m = mgr();
    let a = Client::connect(&m).unwrap();
    let b = Client::connect(&m).unwrap();
    assert_ne!(a.c2s, b.c2s, "不同会话的请求密钥必须不同");
    assert_ne!(a.s2c, b.s2c, "不同会话的响应密钥必须不同");
    assert_ne!(a.c2s, a.s2c, "请求与响应密钥必须不同");
}

#[test]
fn handshake_rate_limit_engages() {
    let m = mgr();
    for _ in 0..HS_MAX_PER_WINDOW {
        assert!(m.allow_handshake("peer"));
    }
    assert!(!m.allow_handshake("peer"));
    assert!(m.allow_handshake("other"));
}

#[test]
fn identity_fingerprint_matches_public_key() {
    let m = mgr();
    let fp = m.identity_fingerprint();
    assert_eq!(fp.len(), 32);
    assert_eq!(fp, identity::fingerprint_of(&m.identity_public()));
    assert_eq!(m.identity_kid().len(), 16);
}

#[test]
fn transport_envelope_is_not_storage_format() {
    // 传输信封是 JSON 文本，存储密文以二进制 Magic 开头，两者不会互相误判。
    let m = mgr();
    let mut c = Client::connect(&m).unwrap();
    let (_, env) = c.encrypt_req("GET", "/api/x", b"");
    assert!(!envelope::is_envelope(&env));
    assert!(serde_json::from_slice::<Envelope>(&env).is_ok());
}
