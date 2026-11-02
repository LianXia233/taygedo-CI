# 内置第三方 JavaScript 库

本目录的文件是**原封不动的上游构建产物**（jsDelivr `+esm` 打包），
仅在必要时改写了模块间的 import 路径，未修改任何算法实现。

| 文件 | 上游包 | 版本 | 许可证 | 用途 |
|---|---|---|---|---|
| `noble-ciphers.js` | [@noble/ciphers](https://github.com/paulmillr/noble-ciphers) | 1.2.1 | MIT | XChaCha20-Poly1305（浏览器无原生实现） |
| `noble-curves-ed25519.js` | [@noble/curves](https://github.com/paulmillr/noble-curves) | 1.8.1 | MIT | Ed25519 验签、X25519 ECDH |
| `noble-hashes-sha256.js` | [@noble/hashes](https://github.com/paulmillr/noble-hashes) | 1.7.1 | MIT | SHA-256 |
| `noble-hashes-sha512.js` | 同上 | 1.7.1 | MIT | SHA-512（ed25519 内部依赖） |
| `noble-hashes-hkdf.js` | 同上 | 1.7.1 | MIT | HKDF-SHA256 |
| `noble-hashes-utils.js` | 同上 | 1.7.1 | MIT | 通用工具 |
| `noble-hashes-crypto.js` | 同上 | 1.7.1 | MIT | CSPRNG 桥接 |

## 为什么必须内置

1. **纯 HTTP 下 `crypto.subtle` 不存在**。浏览器只在安全上下文（HTTPS /
   `localhost`）暴露 WebCrypto。本项目的部署前提就是普通 HTTP，因此
   任何依赖 `crypto.subtle` 的实现都会直接失效。
   `crypto.getRandomValues` 在非安全上下文仍可用，noble 只用它取随机数。
2. **浏览器没有原生 XChaCha20-Poly1305**。协议要求 24 字节 nonce 的
   XChaCha20，必须引入经过审计的兼容实现（noble 由 Cure53 审计）。

## 改写说明

`noble-curves-ed25519.js` 中的两处 import 原本指向 jsDelivr 的绝对路径
（`/npm/@noble/hashes@1.7.1/sha512/+esm`），已改写为本地相对路径
`./noble-hashes-sha512.js`。`noble-hashes-utils.js` 同理改写为
`./noble-hashes-crypto.js`。**没有任何算法代码被修改。**

## 校验

`cargo test` 里的 `web_security::manifest` 测试会计算这些文件的 SHA-256，
服务端签名清单（`/static/manifest.json`）也基于同一份哈希。
若文件被替换，清单校验会失败。
