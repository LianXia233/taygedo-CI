//! 静态资源签名清单。
//!
//! ## 解决什么问题
//!
//! 纯 HTTP 下，中间人可以替换浏览器加载的 `taygedo-crypto.js`，把
//! "加密客户端"改造成"明文客户端 + 抄送攻击者"。**只加密 API 报文
//! 而不校验前端资源，等于什么都没做**。
//!
//! 因此服务端对所有关键静态资源计算 SHA-256，用 Ed25519 身份密钥对
//! 清单签名；浏览器启动器先用**固定过的身份公钥**验签清单，再逐个校验
//! 资源哈希，全部通过后才执行。
//!
//! ## 可信启动链的现实边界（必须如实说明）
//!
//! 本方案的可信根是"操作者带外核对的身份指纹"。具体来说：
//!
//! 1. 服务端启动横幅 / `--identity-fingerprint` 打印 32 字符指纹；
//! 2. 操作者在**第一次**访问时通过非 HTTP 渠道（SSH、串口、本机终端）
//!    比对页面显示的指纹，确认后写入浏览器 localStorage；
//! 3. 之后每次加载都验签 + 校验哈希，任何不一致都会拒绝执行并报错。
//!
//! **首次访问（TOFU 窗口）仍然可能被中间人利用**：如果操作者在核对指纹
//! 之前就被劫持，他看到的是攻击者的指纹。这是"只有普通浏览器 + 普通
//! HTTP"这一前提的固有局限，无法用代码消除。README 中给出了可落地的
//! 缓解部署（带外分发指纹、内网隔离、`always` 策略 + 网段白名单）。
//!
//! 清单只覆盖**静态资源**（加密客户端脚本及其依赖）。内置 WebUI 已移除，
//! 不再有首页 HTML 参与校验；OpenWrt 集成模式下浏览器侧不存在消费者，
//! 清单只对独立部署的外部 API 客户端有意义。

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::identity::ServerIdentity;

/// 清单格式版本。
pub const MANIFEST_VERSION: u8 = 1;

/// 受清单管理的静态资源：`(URL 路径, 内容)`。
///
/// 内容在编译期嵌入，因此"服务端发出的字节"与"签名的哈希"必然一致，
/// 不存在运行期被替换文件的可能。
pub const ASSETS: &[(&str, &str)] = &[
    (
        "/static/taygedo-crypto.js",
        include_str!("../../static/taygedo-crypto.js"),
    ),
    (
        "/static/vendor/noble-ciphers.js",
        include_str!("../../static/vendor/noble-ciphers.js"),
    ),
    (
        "/static/vendor/noble-curves-ed25519.js",
        include_str!("../../static/vendor/noble-curves-ed25519.js"),
    ),
    (
        "/static/vendor/noble-hashes-sha256.js",
        include_str!("../../static/vendor/noble-hashes-sha256.js"),
    ),
    (
        "/static/vendor/noble-hashes-sha512.js",
        include_str!("../../static/vendor/noble-hashes-sha512.js"),
    ),
    (
        "/static/vendor/noble-hashes-hkdf.js",
        include_str!("../../static/vendor/noble-hashes-hkdf.js"),
    ),
    (
        "/static/vendor/noble-hashes-utils.js",
        include_str!("../../static/vendor/noble-hashes-utils.js"),
    ),
    (
        "/static/vendor/noble-hashes-crypto.js",
        include_str!("../../static/vendor/noble-hashes-crypto.js"),
    ),
    (
        "/static/vendor/noble-loader.js",
        include_str!("../../static/vendor/noble-loader.js"),
    ),
    (
        "/static/web_guard.js",
        include_str!("../../static/web_guard.js"),
    ),
];

/// 单个资源的清单项。
#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub path: String,
    /// 小写十六进制 SHA-256。
    pub sha256: String,
    pub size: usize,
}

/// 签名清单。
#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub manifest_version: u8,
    pub proto: String,
    pub kid: String,
    pub identity_pub: String,
    pub generated_at: u64,
    pub files: Vec<FileEntry>,
    /// Ed25519 签名（base64url）。为空时表示未签名的原始清单（不对外提供）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

/// 十六进制 SHA-256。
pub fn sha256_hex(data: &[u8]) -> String {
    let d = Sha256::digest(data);
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 构造待签名的规范化字节串。
///
/// 格式（与浏览器侧 `buildCanonical()` 必须逐字节一致）：
///
/// ```text
/// "taygedo/v2/manifest" 0x00
/// manifest_version=1
/// proto=taygedo/v2
/// kid=xxxx
/// identity_pub=xxxx
/// generated_at=1700000000
/// file:/static/x.js=<sha256>:<size>
/// ... （按 path 升序）
/// ```
pub fn canonical_bytes(m: &Manifest) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("taygedo/v2/manifest\0");
    out.push_str(&format!("manifest_version={}\n", m.manifest_version));
    out.push_str(&format!("proto={}\n", m.proto));
    out.push_str(&format!("kid={}\n", m.kid));
    out.push_str(&format!("identity_pub={}\n", m.identity_pub));
    out.push_str(&format!("generated_at={}\n", m.generated_at));
    let mut files = m.files.clone();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for f in files {
        out.push_str(&format!("file:{}={}:{}\n", f.path, f.sha256, f.size));
    }
    out.into_bytes()
}

/// 生成并签名清单。
///
pub fn build(identity: &ServerIdentity, generated_at: u64) -> Manifest {
    let mut files: Vec<FileEntry> = ASSETS
        .iter()
        .map(|(path, content)| FileEntry {
            path: (*path).to_string(),
            sha256: sha256_hex(content.as_bytes()),
            size: content.len(),
        })
        .collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut m = Manifest {
        manifest_version: MANIFEST_VERSION,
        proto: super::protocol::PROTO.to_string(),
        kid: identity.kid().to_string(),
        identity_pub: super::b64(&identity.public_key()),
        generated_at,
        files,
        sig: None,
    };
    let sig = identity.sign(&canonical_bytes(&m));
    m.sig = Some(super::b64(&sig));
    m
}

/// 按 URL 路径查找资源内容。
pub fn asset(path: &str) -> Option<&'static str> {
    ASSETS.iter().find(|(p, _)| *p == path).map(|(_, c)| *c)
}

/// 按 URL 路径取 MIME 类型。
pub fn content_type(path: &str) -> &'static str {
    if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> ServerIdentity {
        ServerIdentity::from_seed(&[0x5Au8; 32])
    }

    #[test]
    fn manifest_covers_every_asset() {
        let m = build(&id(), 1);
        assert_eq!(m.files.len(), ASSETS.len());
        assert!(m
            .files
            .iter()
            .any(|f| f.path == "/static/taygedo-crypto.js"));
        // 内置 WebUI 已移除：清单不再包含首页 `/`
        assert!(!m.files.iter().any(|f| f.path == "/"));
        for f in &m.files {
            assert_eq!(f.sha256.len(), 64);
            assert!(f.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn manifest_signature_verifies() {
        let id = id();
        let m = build(&id, 12345);
        let sig = super::super::decode_fixed::<64>(m.sig.as_ref().unwrap()).unwrap();
        assert!(super::super::identity::verify(
            &id.public_key(),
            &canonical_bytes(&m),
            &sig
        ));
    }

    #[test]
    fn tampering_with_any_field_breaks_signature() {
        let id = id();
        let m = build(&id, 12345);
        let sig = super::super::decode_fixed::<64>(m.sig.as_ref().unwrap()).unwrap();

        // 改哈希
        let mut m2 = m.clone();
        m2.files[0].sha256 = "0".repeat(64);
        assert!(!super::super::identity::verify(
            &id.public_key(),
            &canonical_bytes(&m2),
            &sig
        ));

        // 增删条目
        let mut m3 = m.clone();
        m3.files.pop();
        assert!(!super::super::identity::verify(
            &id.public_key(),
            &canonical_bytes(&m3),
            &sig
        ));

        // 换身份
        let mut m4 = m.clone();
        m4.kid = "0000000000000000".into();
        assert!(!super::super::identity::verify(
            &id.public_key(),
            &canonical_bytes(&m4),
            &sig
        ));
    }

    #[test]
    fn canonical_is_stable_regardless_of_input_order() {
        let id = id();
        let mut m = build(&id, 1);
        let a = canonical_bytes(&m);
        m.files.reverse();
        assert_eq!(a, canonical_bytes(&m));
    }

    #[test]
    fn assets_are_non_empty_and_unique() {
        let mut seen = std::collections::HashSet::new();
        for (p, c) in ASSETS {
            assert!(seen.insert(*p), "重复路径 {p}");
            assert!(!c.is_empty(), "{p} 内容为空");
        }
    }

    #[test]
    fn content_types() {
        assert!(content_type("/static/a.js").starts_with("application/javascript"));
        assert!(content_type("/static/a.html").starts_with("text/html"));
    }
}
