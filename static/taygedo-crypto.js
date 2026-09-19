/*
 * taygedo 应用层加密客户端（浏览器侧，基于 WebCrypto）
 *
 * 协议：X25519 ECDH → HKDF-SHA256 → 双向 AES-256-GCM，与 Rust 后端 src/session.rs 一一对应。
 *
 * ## 用法
 *
 *   var C = TaygedoCrypto.create({
 *     baseUrl: '',                       // 可选，默认同源
 *     probePath: '/api/crypto/handshake' // 握手路径
 *   });
 *   C.ready().then(function () { ... C.request('/api/accounts', 'GET') ... });
 *
 * ## 关键约束
 *
 * - `request()` 返回解析后的 JSON。内部自动完成握手、序号管理、加解密。
 * - 若后端策略为 auto 且当前来源属内网，握手可省略，`request()` 直接明文直通。
 * - **不要在外部手动构造 Authorization 之外的头**；序号由本模块单调维护。
 * - 会话失效（PRE_CONDITION_REQUIRED / crypto_error）时自动重新握手并重试一次。
 *
 * ## 安全说明
 *
 * - 客户端私钥为内存中的临时 `CryptoKey`（non-extractable 由 WebCrypto 保证），
 *   页面关闭即销毁，不落盘、不进 localStorage。
 * - 会话密钥仅存在于内存。
 * - 为抵御中间人，可将 `identity_pub` 与已知指纹比对（见 `verifyIdentity`）。
 */
(function (global) {
  'use strict';

  var PROTO = 'taygedo/v1';
  var ALG_LABEL = 'X25519+HKDF-SHA256+AES-256-GCM';

  // ---- base64url 工具 ----
  function b64uEncode(bytes) {
    var s = '';
    for (var i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }
  function b64uDecode(str) {
    var s = String(str).replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    var bin = atob(s);
    var out = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  function concat() {
    var total = 0, i;
    for (i = 0; i < arguments.length; i++) total += arguments[i].length;
    var out = new Uint8Array(total), off = 0;
    for (i = 0; i < arguments.length; i++) {
      out.set(arguments[i], off);
      off += arguments[i].length;
    }
    return out;
  }
  function u64be(n) {
    // JS 数值安全范围内的大端 64 位编码
    var buf = new ArrayBuffer(8);
    var view = new DataView(buf);
    view.setUint32(0, Math.floor(n / 4294967296), false);
    view.setUint32(4, n >>> 0, false);
    return new Uint8Array(buf);
  }
  function utf8Encode(s) { return new TextEncoder().encode(s); }
  function utf8Decode(b) { return new TextDecoder().decode(b); }

  /** 检测 WebCrypto 是否可用（含 X25519 支持）。
   *  HTTP 非安全上下文下 `crypto.subtle` 为 undefined，需提前降级。 */
  function webcryptoAvailable() {
    return !!(global.crypto && global.crypto.subtle && global.crypto.getRandomValues);
  }

  function CryptoClient(opts) {
    opts = opts || {};
    this.baseUrl = opts.baseUrl || '';
    this.handshakePath = opts.handshakePath || '/api/crypto/handshake';
    this.sid = null;
    this.c2sKey = null;      // CryptoKey
    this.s2cKey = null;      // CryptoKey
    this.c2sPrefix = null;   // Uint8Array(4)
    this.s2cPrefix = null;   // Uint8Array(4)
    this.nextSeq = 1;        // 客户端 → 服务端 序号
    this.nextRespSeq = 1;    // 服务端 → 客户端 序号
    this.identityPub = null; // 服务端长期身份公钥（base64url）
    this.serverAlg = null;
    this.handshakePromise = null;
    this.enabled = false;
    this.lastError = null;
    // 避免重复握手：并发调用共享同一个 in-flight Promise
    this._inflight = null;
  }

  /** 派生会话密钥（客户端侧复现服务端 HKDF 流程）。 */
  CryptoClient.prototype._derive = function (sharedBits, salt) {
    var self = this;
    return global.crypto.subtle
      .importKey('raw', sharedBits, { name: 'HKDF' }, false, ['deriveBits'])
      .then(function (hkdfKey) {
        function derive(infoStr, len) {
          return global.crypto.subtle.deriveBits(
            {
              name: 'HKDF',
              hash: 'SHA-256',
              salt: salt,
              info: utf8Encode(infoStr)
            },
            hkdfKey,
            len * 8
          ).then(function (bits) { return new Uint8Array(bits); });
        }
        // 与服务端顺序严格一致：master → c2s / s2c → nonce prefix
        return derive('taygedo/v1/aead', 32).then(function (masterBits) {
          return global.crypto.subtle
            .importKey('raw', masterBits, { name: 'HKDF' }, false, ['deriveBits'])
            .then(function (masterKey) {
              function derive2(infoStr, len) {
                return global.crypto.subtle.deriveBits(
                  {
                    name: 'HKDF',
                    hash: 'SHA-256',
                    salt: salt,
                    info: utf8Encode(infoStr)
                  },
                  masterKey,
                  len * 8
                ).then(function (bits) { return new Uint8Array(bits); });
              }
              return Promise.all([
                derive2('taygedo/v1/c2s', 32),
                derive2('taygedo/v1/s2c', 32),
                derive2('taygedo/v1/nonce/c2s', 4),
                derive2('taygedo/v1/nonce/s2c', 4)
              ]);
            });
        });
      })
      .then(function (r) {
        var c2sBits = r[0], s2cBits = r[1];
        self.c2sPrefix = r[2];
        self.s2cPrefix = r[3];
        return Promise.all([
          global.crypto.subtle.importKey('raw', c2sBits, { name: 'AES-GCM' }, false, ['encrypt']),
          global.crypto.subtle.importKey('raw', s2cBits, { name: 'AES-GCM' }, false, ['decrypt'])
        ]);
      })
      .then(function (keys) {
        self.c2sKey = keys[0];
        self.s2cKey = keys[1];
      });
  };

  /** 执行握手。 */
  CryptoClient.prototype.handshake = function () {
    var self = this;
    if (self._inflight) return self._inflight;

    self._inflight = (function () {
      if (!webcryptoAvailable()) {
        // 无 WebCrypto：交由调用方决定降级策略
        return Promise.reject(new Error('WEBCRYPTO_UNAVAILABLE'));
      }
      // X25519 在部分内核版本才支持；不支持时明确报错而非静默降级
      if (!global.crypto.subtle.generateKey) {
        return Promise.reject(new Error('WEBCRYPTO_UNAVAILABLE'));
      }

      var kp;
      return global.crypto.subtle
        .generateKey({ name: 'X25519' }, false, ['deriveBits'])
        .catch(function () {
          // 浏览器不支持 X25519
          throw new Error('X25519_UNSUPPORTED');
        })
        .then(function (pair) {
          kp = pair;
          return global.crypto.subtle.exportKey('raw', kp.publicKey);
        })
        .then(function (rawPub) {
          var clientPub = new Uint8Array(rawPub);
          var clientNonce = new Uint8Array(16);
          global.crypto.getRandomValues(clientNonce);

          return fetch(self.baseUrl + self.handshakePath, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              client_pub: b64uEncode(clientPub),
              client_nonce: b64uEncode(clientNonce)
            })
          })
            .then(function (r) {
              return r.json().catch(function () { return {}; }).then(function (j) {
                if (!r.ok) {
                  var err = new Error(j.error || ('握手失败 HTTP ' + r.status));
                  err.crypto_error = j.crypto_error;
                  err.status = r.status;
                  throw err;
                }
                return j;
              });
            })
            .then(function (j) {
              var serverPub = b64uDecode(j.server_pub);
              var serverNonce = b64uDecode(j.server_nonce);

              // 服务端算法协商校验：协议不符直接拒绝，避免降级
              if (j.proto && j.proto !== PROTO) {
                throw new Error('协议版本不匹配：' + j.proto);
              }

              var serverKey = global.crypto.subtle.importKey(
                'raw', serverPub, { name: 'X25519' }, false, []
              );
              return serverKey.then(function (sk) {
                return global.crypto.subtle.deriveBits(
                  { name: 'X25519', public: sk },
                  kp.privateKey,
                  256
                );
              }).then(function (sharedBits) {
                // salt = client_pub || server_pub || client_nonce || server_nonce
                var salt = concat(clientPub, serverPub, clientNonce, serverNonce);
                return self._derive(sharedBits, salt).then(function () {
                  self.sid = j.sid;
                  self.identityPub = j.identity_pub || null;
                  self.serverAlg = j.alg || null;
                  self.nextSeq = 1;
                  self.nextRespSeq = 1;
                  self.enabled = true;
                  return j;
                });
              });
            });
        });

      return self._inflight;
    })();

    // 无论成败都释放 in-flight 句柄，允许后续重试
    self._inflight.catch(function () {}).then(function () { self._inflight = null; });
    return self._inflight;
  };

  /** 确保会话可用。`required` 为 true 时失败将 reject。 */
  CryptoClient.prototype.ready = function (required) {
    var self = this;
    if (self.sid && self.c2sKey) return Promise.resolve(true);
    if (!webcryptoAvailable()) {
      return required
        ? Promise.reject(new Error('当前页面非安全上下文，浏览器已禁用 WebCrypto；请通过 https 或 localhost 访问'))
        : Promise.resolve(false);
    }
    return self.handshake().then(function () { return true; })
      .catch(function (e) {
        self.lastError = e;
        if (required) throw e;
        return false;
      });
  };

  CryptoClient.prototype._aad = function (direction, seq, extraSeq) {
    // 与服务端 build_aad 完全一致：sid || direction(1B) || seq(8B) [|| reqSeq(8B)]
    var sidBytes = utf8Encode(this.sid);
    var parts = [sidBytes, new Uint8Array([direction]), u64be(seq)];
    if (extraSeq !== undefined) parts.push(u64be(extraSeq));
    return concat.apply(null, parts);
  };

  CryptoClient.prototype._nonce = function (prefix, seq) {
    return concat(prefix, u64be(seq));
  };

  /** 加密请求体，返回 { seq, body }。 */
  CryptoClient.prototype.encryptBody = function (plaintext) {
    var self = this;
    var seq = self.nextSeq++;
    var nonce = self._nonce(self.c2sPrefix, seq);
    var aad = self._aad(0, seq);
    var data = typeof plaintext === 'string' ? utf8Encode(plaintext) : plaintext;
    return global.crypto.subtle
      .encrypt({ name: 'AES-GCM', iv: nonce, additionalData: aad, tagLength: 128 }, self.c2sKey, data)
      .then(function (ct) {
        return { seq: seq, body: b64uEncode(new Uint8Array(ct)) };
      });
  };

  /** 解密响应体。 */
  CryptoClient.prototype.decryptBody = function (reqSeq, b64) {
    var self = this;
    var seq = self.nextRespSeq++;
    var nonce = self._nonce(self.s2cPrefix, seq);
    var aad = self._aad(1, seq, reqSeq);
    var ct = b64uDecode(b64);
    return global.crypto.subtle
      .decrypt({ name: 'AES-GCM', iv: nonce, additionalData: aad, tagLength: 128 }, self.s2cKey, ct)
      .then(function (pt) { return utf8Decode(new Uint8Array(pt)); });
  };

  /**
   * 发起请求。
   *
   * @param path    路径（如 '/api/accounts'）
   * @param method  HTTP 方法
   * @param body    可选，对象则 JSON 序列化
   * @param extraHeaders 可选，附加头（如 Authorization）
   */
  CryptoClient.prototype.request = function (path, method, body, extraHeaders) {
    var self = this;
    var attempt = 0;

    function once() {
      attempt++;
      var headers = {};
      var h = extraHeaders || {};
      for (var k in h) if (Object.prototype.hasOwnProperty.call(h, k)) headers[k] = h[k];

      var payload = undefined;
      if (body !== undefined && method !== 'GET' && method !== 'HEAD') {
        payload = JSON.stringify(body);
        headers['Content-Type'] = 'application/json';
      }

      function sendEncrypted() {
        var reqSeq;
        return (payload === undefined
          ? Promise.resolve(null)
          : self.encryptBody(payload)
        )
          .then(function (enc) {
            if (enc) {
              reqSeq = enc.seq;
              headers['X-TGD-Session'] = self.sid;
              headers['X-TGD-Seq'] = String(enc.seq);
              headers['Content-Type'] = 'text/plain; charset=utf-8';
            } else {
              // 无正文的请求仍需携带会话与序号，服务端据此走加密分支
              reqSeq = self.nextSeq++;
              headers['X-TGD-Session'] = self.sid;
              headers['X-TGD-Seq'] = String(reqSeq);
            }
            return fetch(self.baseUrl + path, {
              method: method || 'GET',
              headers: headers,
              body: enc ? enc.body : undefined
            });
          })
          .then(function (r) {
            var enc = r.headers.get('X-TGD-Enc');
            return r.text().then(function (text) {
              if (enc === '1' && reqSeq !== null) {
                return self.decryptBody(reqSeq, text).then(function (pt) {
                  var data;
                  try { data = JSON.parse(pt); } catch (e) { data = {}; }
                  return { ok: r.ok, status: r.status, data: data };
                });
              }
              var data;
              try { data = JSON.parse(text); } catch (e) { data = {}; }
              return { ok: r.ok, status: r.status, data: data };
            });
          });
      }

      // 尚未建立会话：先探测是否必须加密
      if (!self.sid) {
        return fetch(self.baseUrl + '/api/meta', {
          method: 'GET',
          headers: headers
        })
          .then(function (r) { return r.json().catch(function () { return {}; }); })
          .then(function (meta) {
            if (meta && meta.crypto_required) {
              return self.ready(true).then(sendEncrypted);
            }
            // 内网直通：明文请求
            return fetch(self.baseUrl + path, {
              method: method || 'GET',
              headers: headers,
              body: payload
            }).then(function (r) {
              return r.text().then(function (text) {
                var data;
                try { data = JSON.parse(text); } catch (e) { data = {}; }
                return { ok: r.ok, status: r.status, data: data };
              });
            });
          });
      }

      return sendEncrypted();
    }

    return once().then(function (res) {
      // 会话失效：重新握手后重试一次（避免无限循环）
      var needRetry = res.status === 428 ||
        (res.data && (res.data.crypto_error === 'no_session' ||
          res.data.crypto_error === 'replay' ||
          res.data.crypto_error === 'session_exhausted'));
      if (needRetry && attempt < 2) {
        self.reset();
        return self.ready(true).then(once);
      }
      return res;
    });
  };

  /** 重置会话状态（下次请求将重新握手）。 */
  CryptoClient.prototype.reset = function () {
    this.sid = null;
    this.c2sKey = null;
    this.s2cKey = null;
    this.c2sPrefix = null;
    this.s2cPrefix = null;
    this.nextSeq = 1;
    this.nextRespSeq = 1;
    this.enabled = false;
    this._inflight = null;
  };

  /**
   * 校验服务端长期身份指纹。
   *
   * 用法：把服务端 data/keyring.json 对应公钥的 SHA-256 前 16 字节
   * （十六进制）作为 `expected` 传入。首次可留空并在控制台打印指纹供人工核对。
   */
  CryptoClient.prototype.verifyIdentity = function (expected) {
    if (!this.identityPub) return Promise.resolve(false);
    return global.crypto.subtle
      .digest('SHA-256', b64uDecode(this.identityPub))
      .then(function (d) {
        var hex = '';
        var u = new Uint8Array(d);
        for (var i = 0; i < u.length; i++) hex += ('0' + u[i].toString(16)).slice(-2);
        var fp = hex.slice(0, 32);
        if (expected) return hex.indexOf(String(expected).toLowerCase()) === 0;
        // 未提供期望值时打印指纹，便于人工首次固化
        if (global.console && console.info) {
          console.info('[taygedo] 服务端身份指纹(SHA-256 前16字节): ' + fp);
        }
        return true;
      });
  };

  global.TaygedoCrypto = {
    create: function (opts) { return new CryptoClient(opts); },
    available: webcryptoAvailable,
    b64uEncode: b64uEncode,
    b64uDecode: b64uDecode,
    ALG_LABEL: ALG_LABEL,
    PROTO: PROTO
  };
})(window);
