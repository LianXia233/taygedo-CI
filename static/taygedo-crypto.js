/*
 * taygedo 应用层加密客户端（协议 v2）
 *
 * 设计要点见 docs/security-transport.md。核心约束：
 *
 * 1. 纯 HTTP 下 `crypto.subtle` 不存在，因此**全部**密码学操作走内置
 *    noble 纯 JS 实现（X25519 / Ed25519 / HKDF-SHA256 / XChaCha20-Poly1305）。
 * 2. 握手响应必须用**固定过的**服务端 Ed25519 公钥验签；验签失败即中止，
 *    绝不"先建立会话再校验"。
 * 3. 每条消息的 AAD 绑定 method + path + sid + seq + 方向 + nonce，
 *    密文无法跨接口/跨会话搬移。
 * 4. 没有明文回退：拿不到加密能力就抛错，由界面显示"无法建立安全会话"。
 *
 * 用法：
 *   var C = TaygedoCrypto.create({ baseUrl: '' });
 *   C.ready().then(function(){ return C.request('/api/accounts','GET'); });
 */
(function (global) {
  'use strict';

  var PROTO = 'taygedo/v2';
  var ALG_LABEL = 'X25519+HKDF-SHA256+XChaCha20-Poly1305';
  var PIN_KEY = 'taygedo_identity_pin';
  var SID_RE = /^[0-9a-f]{32}$/;
  var KID_RE = /^[0-9a-f]{16}$/;

  // ---------------------------------------------------------------- 工具

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
    for (i = 0; i < arguments.length; i++) { out.set(arguments[i], off); off += arguments[i].length; }
    return out;
  }

  function utf8(s) { return new TextEncoder().encode(s); }
  function fromUtf8(b) { return new TextDecoder().decode(b); }

  function ascii(s) {
    var out = new Uint8Array(s.length);
    for (var i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 0xff;
    return out;
  }

  function u64be(n) {
    var buf = new ArrayBuffer(8), v = new DataView(buf);
    v.setUint32(0, Math.floor(n / 4294967296), false);
    v.setUint32(4, n >>> 0, false);
    return new Uint8Array(buf);
  }

  /** 与 Rust `push_lp()` 一致：u16 大端长度 + 内容。 */
  function lp(str) {
    var b = utf8(str);
    if (b.length > 65535) throw new Error('字段过长');
    var head = new Uint8Array([(b.length >>> 8) & 0xff, b.length & 0xff]);
    return concat(head, b);
  }

  function hex(bytes) {
    var s = '';
    for (var i = 0; i < bytes.length; i++) s += ('0' + bytes[i].toString(16)).slice(-2);
    return s;
  }

  function randomBytes(n) {
    var b = new Uint8Array(n);
    global.crypto.getRandomValues(b);
    return b;
  }

  function Client(opts) {
    opts = opts || {};
    this.baseUrl = opts.baseUrl || '';
    this.handshakePath = opts.handshakePath || '/api/crypto/handshake';
    this.pinKey = opts.pinKey || PIN_KEY;
    this.reset();
  }

  Client.prototype.reset = function () {
    this.sid = null;
    this.kid = null;
    this.c2s = null;
    this.s2c = null;
    this.nextSeq = 1;
    this.identityPub = null;
    this.identityFingerprint = null;
    this.serverAlg = null;
    this.enabled = false;
    this.lastError = null;
    this._inflight = null;
  };

  /** 内置密码学库是否就绪。 */
  Client.prototype._vendor = function () {
    if (!global.TgdVendorReady) {
      return Promise.reject(new Error('内置密码学库未加载'));
    }
    return global.TgdVendorReady.then(function (v) {
      if (!v) throw new Error('内置密码学库不可用');
      return v;
    });
  };

  /**
   * 资源完整性门禁：未通过校验时拒绝建立会话。
   * 由 /static/web_guard.js 设置 window.TgdIntegrity。
   */
  Client.prototype._requireIntegrity = function () {
    var g = global.TgdIntegrity;
    if (!g) {
      // guard 脚本缺失（被剥离/被拦截）——按失败处理，不给"静默跳过"留口子
      throw new Error('缺少资源完整性校验模块，已拒绝建立加密会话');
    }
    if (!g.ok) {
      throw new Error('前端资源完整性校验失败：' + (g.error || '未知原因'));
    }
  };

  Client.prototype.handshake = function () {
    var self = this;
    if (self._inflight) return self._inflight;

    self._inflight = self._vendor().then(function (v) {
      self._requireIntegrity();

      var priv = randomBytes(32);
      var pub = v.x25519.getPublicKey(priv);
      var clientNonce = randomBytes(32);

      return fetch(self.baseUrl + self.handshakePath, {
        method: 'POST',
        cache: 'no-store',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          proto: PROTO,
          client_pub: b64uEncode(pub),
          client_nonce: b64uEncode(clientNonce)
        })
      }).then(function (r) {
        return r.json().catch(function () { return {}; }).then(function (j) {
          if (!r.ok || !j.ok) {
            var err = new Error(j.error || ('握手失败 HTTP ' + r.status));
            err.crypto_error = j.crypto_error;
            err.status = r.status;
            throw err;
          }
          return j;
        });
      }).then(function (j) {
        // ---- 协议与类型校验（任何不符立即中止，不做兼容降级）----
        if (j.proto !== PROTO) throw new Error('服务端协议版本不匹配：' + j.proto);
        if (!KID_RE.test(String(j.kid || ''))) throw new Error('服务端 Key ID 非法');
        if (!SID_RE.test(String(j.sid || ''))) throw new Error('会话 ID 非法');

        var serverPub = b64uDecode(j.server_pub);
        var serverNonce = b64uDecode(j.server_nonce);
        var identityPub = b64uDecode(j.identity_pub);
        var sig = b64uDecode(j.sig);
        if (serverPub.length !== 32 || serverNonce.length !== 32 ||
            identityPub.length !== 32 || sig.length !== 64) {
          throw new Error('握手字段长度非法');
        }

        // ---- 重建 Transcript 并验签 ----
        var transcript = concat(
          ascii('taygedo/v2/handshake'), new Uint8Array([0]),
          pub, clientNonce, serverPub, serverNonce, identityPub,
          lp(String(j.sid)), u64be(Number(j.ts))
        );

        var ok = v.ed25519.verify(sig, transcript, identityPub);
        if (!ok) throw new Error('服务端身份签名校验失败（可能存在中间人）');

        // ---- 指纹固定校验（TOFU 之后必须一致）----
        var fp = hex(v.sha256(identityPub).slice(0, 16));
        var pinned = null;
        try { pinned = global.localStorage.getItem(self.pinKey); } catch (e) { pinned = null; }
        if (pinned && pinned !== fp) {
          throw new Error('服务端身份指纹与已固定值不一致（' + pinned + ' ≠ ' + fp + '）');
        }

        // ---- 派生会话密钥 ----
        var shared = v.x25519.getSharedSecret(priv, serverPub);
        var salt = v.sha256(transcript);
        var c2s = v.hkdf(v.sha256, shared, salt, utf8('taygedo/v2/c2s'), 32);
        var s2c = v.hkdf(v.sha256, shared, salt, utf8('taygedo/v2/s2c'), 32);

        // 临时私钥立即丢弃（JS 无法保证内存清零，至少解除引用）
        priv = null;

        self.sid = j.sid;
        self.kid = j.kid;
        self.c2s = c2s;
        self.s2c = s2c;
        self.nextSeq = 1;
        self.identityPub = identityPub;
        self.identityFingerprint = fp;
        self.serverAlg = j.alg;
        self.enabled = true;
        return j;
      });
    });

    self._inflight.catch(function () { self._inflight = null; });
    return self._inflight;
  };

  /** 确保会话可用；失败即抛错（没有明文回退）。 */
  Client.prototype.ready = function () {
    var self = this;
    if (self.sid && self.c2s) return Promise.resolve(true);
    return self.handshake().then(function () { return true; });
  };

  Client.prototype._aad = function (domain, seq, method, path, nonce, extraSeq) {
    var parts = [ascii(domain), new Uint8Array([0]),
      lp(this.kid), lp(this.sid), u64be(seq)];
    if (extraSeq !== undefined) parts.push(u64be(extraSeq));
    parts.push(lp(method), lp(path), nonce);
    return concat.apply(null, parts);
  };

  /**
   * 发起一次加密请求。
   * @param {string} path   API 路径（参与 AAD，必须与实际 URL 一致）
   * @param {string} method HTTP 方法（参与 AAD）
   * @param {*} body        对象则 JSON 序列化；undefined 表示无正文
   * @param {object} extraHeaders 附加头（如 Authorization）
   */
  Client.prototype.request = function (path, method, body, extraHeaders) {
    var self = this;
    var mtd = (method || 'GET').toUpperCase();
    var attempt = 0;

    function once() {
      attempt++;
      var v;
      return self.ready().then(function () {
        return self._vendor();
      }).then(function (vendor) {
        v = vendor;
        var headers = {};
        var h = extraHeaders || {};
        for (var k in h) if (Object.prototype.hasOwnProperty.call(h, k)) headers[k] = h[k];

        var plaintext = '';
        if (body !== undefined && mtd !== 'GET' && mtd !== 'HEAD') {
          plaintext = JSON.stringify(body);
        }

        var seq = self.nextSeq++;
        var nonce = randomBytes(24);
        var aad = self._aad('taygedo/v2/req', seq, mtd, path, nonce);
        var ct;
        try {
          ct = v.xchacha20poly1305(self.c2s, nonce, aad).encrypt(utf8(plaintext));
        } catch (e) {
          return Promise.reject(new Error('请求加密失败'));
        }

        var env = {
          v: 2,
          kid: self.kid,
          sid: self.sid,
          seq: seq,
          n: b64uEncode(nonce),
          ct: b64uEncode(ct)
        };

        headers['X-TGD-Session'] = self.sid;
        headers['X-TGD-Seq'] = String(seq);
        headers['Content-Type'] = 'application/json';

        // GET/HEAD：浏览器与 undici 都不允许带 body，信封改走请求头。
        // 头里放的是同一个信封的 base64url，服务端走同一条解析与 AAD 校验路径。
        var noBody = (mtd === 'GET' || mtd === 'HEAD');
        var init = {
          method: mtd,
          headers: headers,
          cache: 'no-store'
        };
        if (noBody) {
          headers['X-TGD-Envelope'] = b64uEncode(utf8(JSON.stringify(env)));
        } else {
          init.body = JSON.stringify(env);
        }

        return fetch(self.baseUrl + path, init).then(function (r) {
          return r.text().then(function (text) {
            var parsed = null;
            try { parsed = JSON.parse(text); } catch (e) { parsed = null; }
            var marker = r.headers && r.headers.get ? r.headers.get('X-TGD-Enc') : null;
            // 业务错误（非 2xx）同样会被中间件加密。必须不分状态统一解密，
            // 否则调用方只拿到一串密文，丢掉真实错误信息。
            // 标记缺失时（例如跨源未暴露该头）退化为按信封结构判断。
            var encrypted = marker === '1' || (marker !== '0' && parsed && parsed.v === 2);
            if (encrypted) {
              var j = parsed;
              if (!j || j.v !== 2) throw new Error('响应不是合法加密信封');
              if (j.kid !== self.kid) throw new Error('响应 Key ID 不匹配');
              if (j.sid !== self.sid) throw new Error('响应会话 ID 不匹配');
              if (Number(j.rseq) !== seq) throw new Error('响应与请求不匹配（可能被替换）');

              var rn = b64uDecode(j.n);
              if (rn.length !== 24) throw new Error('响应 nonce 长度非法');
              var raad = self._aad('taygedo/v2/res', Number(j.seq), mtd, path, rn, seq);
              var pt;
              try {
                pt = v.xchacha20poly1305(self.s2c, rn, raad).decrypt(b64uDecode(j.ct));
              } catch (e) {
                throw new Error('响应解密失败（认证标签校验不通过）');
              }
              var data = {};
              var s = fromUtf8(pt);
              if (s.length) {
                try { data = JSON.parse(s); } catch (e) { data = { raw: s }; }
              }
              return { ok: r.ok, status: r.status, data: data };
            }

            // 明文响应只可能来自显式策略分支（crypto_policy=never / 内网直通）
            // 或加密层自身的错误。带 crypto_error 的必须抛错，
            // 否则重试逻辑永远看不到 no_session / replay，页面会一直失败。
            var pdata = parsed || {};
            if (pdata.crypto_error) {
              var err = new Error(pdata.error || pdata.crypto_error);
              err.crypto_error = pdata.crypto_error;
              err.status = r.status;
              throw err;
            }
            return { ok: r.ok, status: r.status, data: pdata };
          });
        });
      });
    }

    return once().catch(function (e) {
      // 会话失效类错误：重新握手后重试一次（不无限重试）
      var code = e && e.crypto_error;
      var retryable = code === 'no_session' || code === 'replay' || code === 'session_exhausted';
      if (retryable && attempt < 2) {
        self.reset();
        return once();
      }
      throw e;
    });
  };

  /** 服务端身份指纹（32 字符十六进制）。 */
  Client.prototype.fingerprint = function () {
    return this.identityFingerprint;
  };

  /** 把当前身份指纹固定到 localStorage（操作者带外核对后调用）。 */
  Client.prototype.pinIdentity = function (expected) {
    var fp = expected || this.identityFingerprint;
    if (!fp) return false;
    try { global.localStorage.setItem(this.pinKey, String(fp).toLowerCase()); } catch (e) { return false; }
    return true;
  };

  /** 读取已固定的指纹。 */
  Client.prototype.pinnedFingerprint = function () {
    try { return global.localStorage.getItem(this.pinKey); } catch (e) { return null; }
  };

  /** 从服务端获取身份信息（用于首次 TOFU 展示，不含任何私钥）。 */
  Client.fetchIdentity = function (baseUrl) {
    return fetch((baseUrl || '') + '/api/secure/identity', { cache: 'no-store' })
      .then(function (r) { return r.json(); });
  };

  global.TaygedoCrypto = {
    create: function (opts) { return new Client(opts); },
    fetchIdentity: Client.fetchIdentity,
    b64uEncode: b64uEncode,
    b64uDecode: b64uDecode,
    ALG_LABEL: ALG_LABEL,
    PROTO: PROTO,
    PIN_KEY: PIN_KEY,
    _internals: { lp: lp, u64be: u64be, hex: hex }
  };
})(window);
