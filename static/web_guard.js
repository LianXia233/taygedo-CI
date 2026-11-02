/*
 * 前端资源完整性校验（纯 HTTP 环境下的 best-effort 防线）
 *
 * 流程：
 *   1. 拉取 /static/manifest.json
 *   2. 用 Ed25519 公钥验证清单签名（公钥来自清单本身，并与本地固定指纹比对）
 *   3. 逐个重新拉取清单中的资源，比对 SHA-256
 *   4. 全部通过 → window.TgdIntegrity.ok = true
 *
 * ## 能力边界（务必如实理解）
 *
 * - 能防的：**拿到指纹之后**的中间人替换、服务端静态文件被篡改、
 *   CDN/缓存投毒。攻击者没有身份私钥，无法伪造清单签名。
 * - 不能防的：**首次访问**。此时本地还没有固定指纹，页面本身也可能
 *   已被替换。这是"只有浏览器 + 普通 HTTP"的固有上限，代码无法消除。
 *   正确的做法是带外核对指纹（启动横幅 / CLI），见 docs/security-transport.md。
 */
(function (global) {
  'use strict';

  var PIN_KEY = 'taygedo_identity_pin';
  var MANIFEST = '/static/manifest.json';

  global.TgdIntegrity = { ok: false, checked: false, error: '尚未完成校验' };

  function b64uDecode(str) {
    var s = String(str).replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    var bin = atob(s);
    var out = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  function utf8(s) { return new TextEncoder().encode(s); }

  function hex(bytes) {
    var s = '';
    for (var i = 0; i < bytes.length; i++) s += ('0' + bytes[i].toString(16)).slice(-2);
    return s;
  }

  /** 与 Rust `manifest::canonical_bytes()` 逐字节一致。 */
  function buildCanonical(m) {
    var files = (m.files || []).slice().sort(function (a, b) {
      return a.path < b.path ? -1 : (a.path > b.path ? 1 : 0);
    });
    var s = 'taygedo/v2/manifest\0';
    s += 'manifest_version=' + m.manifest_version + '\n';
    s += 'proto=' + m.proto + '\n';
    s += 'kid=' + m.kid + '\n';
    s += 'identity_pub=' + m.identity_pub + '\n';
    s += 'generated_at=' + m.generated_at + '\n';
    for (var i = 0; i < files.length; i++) {
      s += 'file:' + files[i].path + '=' + files[i].sha256 + ':' + files[i].size + '\n';
    }
    return utf8(s);
  }

  function fetchText(path) {
    return fetch(path, { cache: 'no-store' }).then(function (r) {
      if (!r.ok) throw new Error('资源不可读：' + path + '（HTTP ' + r.status + '）');
      return r.text();
    });
  }

  function fail(msg) {
    global.TgdIntegrity = { ok: false, checked: true, error: msg };
    if (global.console && console.error) console.error('[taygedo] 完整性校验失败：', msg);
    return global.TgdIntegrity;
  }

  /**
   * 执行校验。
   * @param {string} baseUrl 后端根地址（LuCI 跨端口访问时需要传入）
   */
  function check(baseUrl) {
    var base = baseUrl || '';
    if (!global.TgdVendorReady) return Promise.resolve(fail('密码学库未加载'));

    return global.TgdVendorReady.then(function (v) {
      return fetchText(base + MANIFEST).then(function (text) {
        var m;
        try { m = JSON.parse(text); } catch (e) { throw new Error('清单不是合法 JSON'); }
        if (!m.sig || !m.identity_pub || !m.files) throw new Error('清单字段缺失');

        var sig = b64uDecode(m.sig);
        var pub = b64uDecode(m.identity_pub);
        if (sig.length !== 64 || pub.length !== 32) throw new Error('清单签名字段长度非法');

        // 1) 验签
        if (!v.ed25519.verify(sig, buildCanonical(m), pub)) {
          throw new Error('清单签名校验失败（资源可能被替换）');
        }

        // 2) 指纹固定比对
        var fp = hex(v.sha256(pub).slice(0, 16));
        var pinned = null;
        try { pinned = global.localStorage.getItem(PIN_KEY); } catch (e) { pinned = null; }
        if (pinned && pinned !== fp) {
          throw new Error('服务端身份指纹已变更（' + pinned + ' → ' + fp + '）');
        }

        // 3) 逐资源哈希校验
        return Promise.all(m.files.map(function (f) {
          return fetchText(base + f.path).then(function (body) {
            var h = hex(v.sha256(utf8(body)));
            if (h !== f.sha256) throw new Error('资源哈希不匹配：' + f.path);
          });
        })).then(function () {
          global.TgdIntegrity = {
            ok: true,
            checked: true,
            error: null,
            kid: m.kid,
            fingerprint: fp,
            pinned: !!pinned,
            generatedAt: m.generated_at
          };
          if (!pinned && global.console && console.warn) {
            console.warn('[taygedo] 尚未固定服务端身份指纹，当前为 TOFU 状态；指纹=' + fp);
          }
          return global.TgdIntegrity;
        });
      }).catch(function (e) {
        return fail(e && e.message ? e.message : String(e));
      });
    });
  }

  global.TgdCheckIntegrity = check;
  global.TgdIntegrityPromise = check('');
})(window);
