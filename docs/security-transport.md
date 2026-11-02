# WebUI ↔ 后端 应用层加密协议 v2

本文是 **`taygedo/v2`** 协议的完整规格。协议运行在**普通 HTTP 之上**，不使用
TLS、不需要证书。它同时解决四件事：机密性、完整性、来源认证、防重放；并额外
处理一个常被回避的难题——**纯 HTTP 下前端 JS 本身也可能被替换**。

> 先说清结论，避免被后文的密码学细节误导：
>
> 本协议把「被动嗅探」和「无密钥的主动篡改」挡住了。
> 它**不能**凭空消除「首次访问时的信任建立窗口（TOFU）」，也**不能**在攻击者
> 能完全控制首次响应的前提下提供绝对安全。完整边界见 §9。

---

## 1. 算法与参数

| 用途 | 算法 | 参数 |
| :--- | :--- | :--- |
| 服务端长期身份 | Ed25519 | 32 字节 seed → 32 字节公钥；签名 64 字节 |
| 会话密钥交换 | X25519 | 服务端每次握手生成**临时**密钥对 |
| 密钥派生 | HKDF-SHA256 | `salt = SHA-256(transcript)`，info 分域 |
| 正文加密 | XChaCha20-Poly1305 | 192-bit nonce，16 字节 tag |
| 随机源（服务端） | `OsRng`（OS CSPRNG） | |
| 随机源（浏览器） | `crypto.getRandomValues` | |
| 资源完整性 | Ed25519 签名清单 + SHA-256 逐文件 | |

**为什么是 XChaCha20 而不是 AES-GCM**：XChaCha20 的 24 字节 nonce 允许"每次随机
生成 nonce"而无需担心碰撞（192-bit 随机数即使海量请求也不会有实际碰撞风险），
省掉了 AES-GCM 在 96-bit nonce 下必须维护计数器、并且一旦计数器重置就灾难性
失败的问题。在 aarch64 软路由上 ChaCha20 也不依赖 AES 硬件加速，避免
"没开 `aes_armv8` 就退化成慢速软件 AES + cache-timing 侧信道"这类隐患。

**为什么客户端不用 `crypto.subtle`**：`window.crypto.subtle` 只在**安全上下文**
（HTTPS 或 localhost）中可用。本项目的典型部署是 `http://192.168.1.1:8787`，
属于非安全上下文，`subtle` 直接是 `undefined`。因此前端使用随包内置的纯 JS
实现（`@noble/ciphers`、`@noble/curves`、`@noble/hashes`，MIT，Cure53 审计），
只依赖 `crypto.getRandomValues`（非安全上下文亦可用）。

---

## 2. 握手

`POST /api/crypto/handshake`

### 2.1 请求

```json
{
  "proto": "taygedo/v2",
  "client_pub": "<base64url, 32B X25519 公钥>",
  "client_nonce": "<base64url, 32B 随机>"
}
```

`proto` 不匹配 → `415/428` + `crypto_error: "unsupported"`。**不做版本协商、
不降级。**

### 2.2 响应

```json
{
  "ok": true,
  "proto": "taygedo/v2",
  "alg": "X25519+HKDF-SHA256+XChaCha20-Poly1305",
  "sid": "<32 个十六进制字符>",
  "server_pub": "<base64url, 32B X25519 临时公钥>",
  "server_nonce": "<base64url, 32B 随机>",
  "identity_pub": "<base64url, 32B Ed25519 身份公钥>",
  "kid": "<16 个十六进制字符>",
  "ts": 1730000000,
  "sig": "<base64url, 64B Ed25519 签名>",
  "expires_in": 1800
}
```

### 2.3 Transcript 与签名

```
transcript =
    "taygedo/v2/handshake" || 0x00
 || client_pub   (32B)
 || client_nonce (32B)
 || server_pub   (32B)
 || server_nonce (32B)
 || identity_pub (32B)
 || lp(sid)                      # u16 大端长度 + 内容
 || ts (8B, 大端)

sig = Ed25519.sign(identity_sk, transcript)
```

**为什么必须签整个 Transcript**：如果只签 `server_pub`，攻击者可以保留签名、
替换 `identity_pub`（换成自己的公钥）或替换 `sid`/`ts`，客户端仍会验签通过。
把六元组全部纳入签名，任何一个字段被改都会导致验签失败。

`sid` 与 `ts` 用长度前缀，避免"`sid` 尾部字节与 `ts` 首字节拼接歧义"。

### 2.4 会话密钥派生

```
shared  = X25519(server_eph_sk, client_pub)      # 32B，全零则握手失败
salt    = SHA-256(transcript)                     # 32B
c2s     = HKDF-SHA256(ikm=shared, salt, info="taygedo/v2/c2s", 32)
s2c     = HKDF-SHA256(ikm=shared, salt, info="taygedo/v2/s2c", 32)
bind    = HKDF-SHA256(ikm=shared, salt, info="taygedo/v2/bind", 32)
```

- **`salt` 就是 transcript 的哈希**：等价于把整个握手上下文绑进密钥。若双方
  对 transcript 的任一字节理解不同，派生出的密钥就不同，后续请求立刻 AEAD 失败。
- **c2s / s2c 分离**：两个方向用不同密钥。共用一个密钥时，攻击者可以把"服务端
  发出的密文"原样反射给服务端，被当成合法请求（反射攻击）。分离后方向不同即
  解密失败。
- **`bind`** 保留用于未来的信道绑定/审计 MAC，当前不参与加解密。

### 2.5 客户端必须做的验证

1. 重建 transcript，用 `identity_pub` **验签**；失败 → 中止，不建立会话。
2. 把 `identity_pub` 的指纹与本地固定值比对：

   ```
   fingerprint = hex( SHA-256(identity_pub)[0..16] )   # 32 个十六进制字符
   ```

   - `localStorage["tgd.id.fp"]` 为空 → **TOFU**：记录本次指纹（并提示用户核对）。
   - 已有值且不一致 → **拒绝握手**，提示"服务端身份已变更"。
3. 只有验签 + 指纹都通过，才派生会话密钥并发起业务请求。

前端实现见 `static/taygedo-crypto.js`（`handshake()` / `pinIdentity()`）。

---

## 3. 请求信封

业务请求（`/api/*`，除握手、`/api/meta`、`/api/secure/identity` 外）必须携带信封。

### 3.1 传输位置

| 方法 | 位置 |
| :--- | :--- |
| `POST` / `PUT` / `DELETE` / `PATCH` | 请求体，`Content-Type: application/json` |
| `GET` / `HEAD` | **请求头 `X-TGD-Envelope`**（base64url 的 JSON） |

GET 不放 body 的原因：HTTP 语义上 GET 不应有 body，浏览器 `fetch` 与 Node
`undici` 都会直接抛 `Request with GET/HEAD method cannot have body`，部分代理
还会丢弃 body 或据此改变缓存行为。

头里的信封与 body 里的信封走**完全相同**的解析与 AAD 校验路径，不存在旁路。
若同一请求既有非空 body 又有 `X-TGD-Envelope`，直接 `400 malformed`——
消除"两份信封哪份为准"的歧义。

同时携带 `X-TGD-Session: <sid>` 头用于路由（该头仅用于快速判定"是否加密请求"，
**不参与**任何认证；真正的绑定在 AAD 里）。

### 3.2 信封结构

```json
{
  "v": 2,
  "kid": "<16 hex>",
  "sid": "<32 hex>",
  "seq": 42,
  "n": "<base64url, 24B nonce>",
  "ct": "<base64url, 密文 + 16B tag>"
}
```

### 3.3 AAD

```
aad = "taygedo/v2/req" || 0x00
   || lp(kid) || lp(sid)
   || seq (8B 大端)
   || lp(method)                 # 大写，如 "GET"
   || lp(path)                   # 含 query，如 "/api/logs?limit=50"
   || nonce (24B)
```

- **绑定 method + path**：把 `/api/logs` 的密文发到 `/api/accounts` 会被拒绝
  （AAD 不同 → AEAD 失败）。这防的是"越权搬移"：低权限接口的合法密文不能
  被搬到高权限接口上重复使用。
- **绑定 seq**：同一份密文换个序号也解不开，重放不能靠"改序号"绕过。
- `path` 含 query：`/api/logs?limit=10` 与 `?limit=1000` 是两个不同的 AAD，
  不能互相替换。

明文是 JSON 序列化的请求体；无 body 的方法明文为空串。

---

## 4. 响应信封

```json
{
  "v": 2,
  "kid": "<16 hex>",
  "sid": "<32 hex>",
  "seq": 43,
  "rseq": 42,
  "n": "<base64url, 24B>",
  "ct": "<base64url>"
}
```

```
aad = "taygedo/v2/res" || 0x00
   || lp(kid) || lp(sid)
   || req_seq (8B)               # 对应请求的 seq
   || res_seq (8B)               # 本次响应的 seq
   || lp(method) || lp(path)
   || nonce (24B)
```

- 域的起始串不同（`/req` vs `/res`），即使双方密钥相同也不会串用；
  实际 c2s/s2c 已分离，这是双保险。
- **`rseq` 绑定到请求**：客户端必须校验 `response.rseq == request.seq`。
  防的是"把 A 请求的响应塞给 B 请求"（响应替换/乱序错配）。
- 响应头 `X-TGD-Enc: 1` 表示已加密；`0` 表示明文（仅出现在显式策略分支）。

---

## 5. 防重放与并发

### 5.1 服务端

每个会话维护一个**滑动窗口**（`ReplayWindow`，`WINDOW = 4096`）：

- `HashSet<u64>` 保存窗口内已接受的序号 + 一个 `max_seq`。
- 收到 `seq`：
  - `seq < max_seq - WINDOW` → `Replay`（太旧，已滑出窗口）；
  - 在集合中 → `Replay`（重复）；
  - 否则接受，插入集合并推进 `max_seq`。
- **只有 AEAD 解密成功后**才 `accept(seq)`。顺序不能反：若先占位再解密，
  攻击者发一个畸形信封就能把合法序号"烧掉"，造成合法请求被误判为重放（DoS）。

为什么需要窗口而不是"严格递增"：HTTP/1.1 的 keep-alive 与浏览器并发会让请求
乱序到达。严格递增会把"先发的请求后到"当成重放，直接废掉并发。

### 5.2 客户端

- `nextSeq` 单调递增，**失败不回收序号**（失败也消耗掉，防止"用同一个
  seq+nonce 重试"被服务端判重放）。
- 收到 `no_session` / `replay` / `session_exhausted` → 重建会话后重试**一次**，
  不无限重试。

### 5.3 会话生命周期

| 项 | 值 |
| :--- | :--- |
| TTL | 1800 秒 |
| 最大请求数 | 200 000 |
| 最大并发会话 | 256（超出淘汰最旧） |
| 握手限流 | 每来源 60 秒内 60 次 |

会话结束（TTL/超限/登出/服务重启）后：会话密钥从 map 中移除，
`Zeroizing` 在 drop 时清零临时私钥与两个方向密钥。**服务重启后旧 sid 全部失效**，
这是设计而非缺陷——内存中的密钥本来就不落盘。

---

## 6. 防降级

| 场景 | 行为 |
| :--- | :--- |
| 无信封 + 策略要求加密 | `428 Precondition Required`，`crypto_error: "no_session"` |
| 握手失败 | 不建立会话，客户端拒绝发业务请求（不发明文） |
| 未知 `proto` | `unsupported`，拒绝 |
| 未知 `v` / 未知 `kid` | `unsupported` / `auth_failed`，拒绝，不尝试旧版本 |
| 信封里塞明文 JSON | `malformed`（JSON 解析成功但字段缺失/类型错） |
| 解密失败 | `auth_failed`，**不**回退到"当作明文处理" |
| 中间件自身出错 | 返回加密层的错误，不调用下游业务（错误处理不能绕过解密） |

唯一允许明文的路径是**显式策略**：`crypto_policy=never`，或
`crypto_policy=auto` 且来源命中 `lan_cidrs`。这两条都在启动横幅与 `/api/meta`
里公开暴露（`crypto_policy` / `crypto_required`），运维可随时核对，不是暗门。

### 6.1 客户端如何跟随策略

- **策略选择传输方式**：WebUI 先读 `/api/meta` 的 `crypto_policy`，
  只有 `never` 时改走明文 `fetch`（该策略下服务端不解密也不加密，再发信封
  只会让业务 handler 收到密文 JSON）；其余策略一律发信封，`auto + 内网`
  虽然允许明文，但加密请求同样被接受，因此不做额外分支。
- **错误响应同样加密**：业务 handler 返回非 2xx 时，中间件仍然加密响应体并
  置 `X-TGD-Enc: 1`。客户端必须**先解密再判断状态码**，否则调用方只能拿到
  一串密文。加密层自身的错误则以明文返回并带 `crypto_error`，客户端遇到
  这类明文错误会**抛错**（而不是当成普通业务结果），从而触发 5.2 的
  重建会话 + 重试一次。
- **跨源预检必须声明 `X-TGD-Envelope`**：LuCI 与服务进程不同端口却同主机时，
  GET/HEAD 的信封走该请求头。`cors_middleware` 的
  `Access-Control-Allow-Headers` 已固定声明 `Authorization, Content-Type,
  X-TGD-Session, X-TGD-Seq, X-TGD-Envelope`；漏掉任一项都会让浏览器在预检
  阶段直接拦下请求。

---

## 7. 前端资源完整性

这一节回答"既然走明文 HTTP，攻击者直接把 JS 换掉不就完了"。

### 7.1 清单

`GET /static/manifest.json`（服务端每次进程启动生成一次并缓存）：

```json
{
  "manifest_version": 1,
  "proto": "taygedo/v2",
  "kid": "<身份 Key ID>",
  "identity_pub": "<base64url>",
  "generated_at": 1730000000,
  "files": [
    { "path": "/static/taygedo-crypto.js", "sha256": "<hex>", "size": 12345 },
    ...
  ],
  "sig": "<base64url, Ed25519 签名>"
}
```

被签名的规范化字节（`canonical_bytes`）：

```
"taygedo/v2/manifest" || 0x00
manifest_version=1
proto=taygedo/v2
kid=...
identity_pub=...
generated_at=...
file:/static/taygedo-crypto.js=<sha256>:<size>
file:/static/vendor/...=<sha256>:<size>
...                                  # 按 path 升序
```

覆盖的资源：`/`（`src/ui.html` 本体）、`/static/taygedo-crypto.js`、
`/static/web_guard.js`、`/static/vendor/noble-loader.js` 与 7 个 noble 模块，共 11 个。

### 7.2 启动顺序（`static/web_guard.js`）

```
1. 拉 /static/manifest.json
2. 重建 canonical 字节，用 manifest 里的 identity_pub 验签
3. 与 localStorage 中的指纹比对（TOFU / 不一致即拒绝）
4. 逐个 fetch 文件、算 SHA-256、与清单比对
5. 全部通过 → window.TgdIntegrity = { ok: true }
   任一失败 → window.TgdIntegrity = { ok: false, error: "..." }
```

`taygedo-crypto.js` 的 `_requireIntegrity()` 在每次握手前检查
`window.TgdIntegrity.ok`，为 `false` 直接抛错；`ui.html` 的 `init()` 在
`TgdIntegrityPromise` 失败时**禁用登录按钮**并显示原因。

也就是说：**资源校验不通过时，业务 JS 不会被执行，登录框不可用。**
不是"警告一下继续用"。

### 7.3 这套机制能挡什么、不能挡什么

| 攻击 | 结果 |
| :--- | :--- |
| 中途替换 `/static/taygedo-crypto.js` | 哈希不符 → 拒绝启动 ❌ |
| 同时替换清单 + 资源，但用攻击者自己的密钥签名 | 指纹不符（已 pin）→ 拒绝 ❌ |
| 只替换清单 | 签名验证失败 ❌ |
| 替换 `/`（HTML）指向攻击者脚本 | `/` 在清单内，哈希不符 → 拒绝 ❌ |
| **首次访问时**就替换全部内容（含清单） | **TOFU 窗口内无法防御** ⚠️ |

最后一条是这套方案的真实边界：第一次访问时客户端没有任何可信锚点，
只能"先信一次"。缓解手段：

- 部署后**立即在浏览器里核对一次指纹**（`/api/secure/identity` 或登录页展示），
  并把它记录在运维文档里；后续任何变更都应被拒绝。
- 高安全场景应在前面加一层 TLS 反代（本协议不排斥 TLS，两者可叠加）。

`localStorage` 不是安全存储：同一台机器上的其它脚本（若同源）能改它。
本方案假定浏览器本身未被攻破。

### 7.4 公钥分发与轮换

- 分发：`/api/secure/identity`（公开，返回 `identity_pub` / `kid` / `fingerprint`）
  与握手响应里的 `identity_pub`。两者必须一致，客户端以握手响应为准。
- 轮换：更换 `keyring.json` 里的身份私钥后 `kid` 与指纹都会变，所有已 pin 的
  客户端会拒绝握手并提示"服务端身份已变更"。这是**预期行为**——需要人工确认
  后在浏览器里清除 pin（或提供新的指纹）。协议不提供"自动接受新身份"。

---

## 8. 登录 / Token / Cookie

- 登录走 `POST /api/login`，**必须在加密会话内**（`ui.html` 里 `doLogin` 只走
  `api()`，`api()` 没有 cryptoClient 时直接抛错，不存在明文登录分支）。
- Token 通过 `Authorization: Bearer <token>` 传递，位于信封**内部**（作为明文
  JSON 的一部分被加密），不出现在 URL、Cookie 或日志中。
- 服务端**不**下发 Cookie，因此不存在 `Secure`/`SameSite` 的取舍问题，
  也从根上规避了 CSRF（没有 ambient credential 可用）。
- 会话建立后**不**降级：登录成功会给当前加密会话打 `authenticated` 标记，
  改密码时会销毁除当前会话外的所有会话（其它设备的旧 token 立即失效）。
- 服务端记录/回显的错误信息不含密钥、明文、token；日志只记 `sid` 与错误码。

---

## 9. 威胁模型与已知限制

### 能防

- 被动嗅探（tcpdump、交换机镜像、共享 Wi-Fi）：正文全密文。
- 无密钥的主动篡改：改一个字节 → AEAD 失败。
- 重放：会话 + 序号 + AAD 绑定。
- 跨接口搬移 / 跨会话搬移 / 跨方向反射：AAD 与密钥分离。
- 服务端身份伪造（在非首次访问时）：Ed25519 签名 + 指纹 pin。
- 前端 JS 被替换（在非首次访问时）：签名清单 + 逐文件哈希。
- 版本降级：未知版本一律拒绝，无协商。

### 不能防（务必知情）

1. **TOFU 首次信任窗口**：第一次访问时若攻击者已在链路上完整伪造响应，
   客户端没有可信锚点可比对。§7.3 已说明缓解方式。
2. **HTTP 头/元数据泄露**：`X-TGD-Session`、请求方法、路径、状态码、报文长度
   仍是明文。攻击者能看出"某人在访问 /api/accounts"，但看不出内容。
   流量分析（时序、体积）无法防护。
3. **浏览器被完全控制**：恶意扩展、被注入的浏览器、同源其它脚本都能读到解密后
   的明文与 `localStorage`。
4. **服务端被完全控制**：本协议不防服务端自身。
5. **DoS**：加密层有长度上限、握手限流与会话上限，但不做完整的抗 DDoS。
6. **`localStorage` 可被同源脚本修改**：见 §7.3。
7. **不替代 TLS**：本协议的目标是"无法部署证书的场景下，把明文从链路上拿掉"。
   能上 TLS 就应该上 TLS，两者叠加是最佳实践。

### 与"端到端加密"这个说法的关系

严格说这是 **client-to-server 的应用层加密**，加密边界在浏览器与服务进程之间。
项目的宣传文案（README、徽章）在 v0.6.0 起统一改成
"应用层加密（X25519 + Ed25519 + XChaCha20-Poly1305）"，
不再使用容易被误解为"绕过了服务端"的表述。

---

## 10. 错误码

| `crypto_error` | HTTP | 含义 |
| :--- | ---: | :--- |
| `no_session` | 428 | 缺少会话或会话已过期/失效 |
| `crypto_required` | 428 | 该来源被要求加密，但请求没有信封 |
| `malformed` | 400 | 信封结构/编码非法；或正文与 `X-TGD-Envelope` 并存 |
| `auth_failed` | 400 | AEAD 认证失败（篡改/密钥或 AAD 不符） |
| `replay` | 400 | 重放或序号滑出窗口 |
| `handshake_failed` | 400 | 握手本身失败（ECDH 全零/签名问题） |
| `unsupported` | 428 | 未知协议版本 / 未知 Key ID |
| `rate_limited` | 429 | 握手过于频繁 |
| `session_exhausted` | 429 | 会话请求数超限 |
| `body_too_large` | 413 | 请求体超过上限 |

客户端对 `no_session` / `replay` / `session_exhausted` 的处理是
**重建会话 + 重试一次**，其余错误直接展示，不重试。

---

## 11. 代码地图

| 文件 | 职责 |
| :--- | :--- |
| `src/web_security/protocol.rs` | 常量、`transcript()`、`request_aad()`、`response_aad()`、信封结构 |
| `src/web_security/identity.rs` | Ed25519 身份、`fingerprint_of()`、`key_id_of()` |
| `src/web_security/cipher.rs` | HKDF、XChaCha20 seal/open、随机 nonce |
| `src/web_security/replay.rs` | 滑动窗口 |
| `src/web_security/session.rs` | 会话状态与生命周期上限（TTL / 请求数 / 并发数） |
| `src/web_security/error.rs` | `CryptoError`：错误码、文案、HTTP 状态映射 |
| `src/web_security/manifest.rs` | 资源清单、签名、`canonical_bytes()` |
| `src/web_security/middleware.rs` | 加密中间件：策略判定、信封提取（body / `X-TGD-Envelope`）、AAD 绑定、响应加密 |
| `src/web_security/mod.rs` | `CryptoManager`：握手、会话表、加解密、限流、销毁 |
| `src/web.rs` | 路由与业务 handler（只看得到明文 JSON） |
| `static/web_guard.js` | 清单验签 + 逐文件哈希 + 指纹比对 |
| `static/taygedo-crypto.js` | 浏览器端客户端（握手、请求、响应解密、TOFU pin） |
| `static/vendor/*` | noble 纯 JS 密码学实现（MIT） |
| `scripts/e2e_web.mjs` | 端到端协议测试（真实二进制 + 真实前端文件） |
