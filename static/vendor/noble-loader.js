/*
 * 内置密码学库加载器（classic script）。
 *
 * 浏览器没有原生 XChaCha20-Poly1305，且**纯 HTTP 下 `crypto.subtle` 不存在**
 * （仅安全上下文可用），因此这里用动态 import 载入随包内置的 noble 系列
 * 纯 JS 实现（MIT，Cure53 审计），并把模块挂到 window 供后续脚本使用。
 *
 * 只做加载，不做任何算法改写。
 */
(function (global) {
  'use strict';

  var BASE = '/static/vendor/';

  function load() {
    return Promise.all([
      import(BASE + 'noble-ciphers.js'),
      import(BASE + 'noble-curves-ed25519.js'),
      import(BASE + 'noble-hashes-hkdf.js'),
      import(BASE + 'noble-hashes-sha256.js')
    ]).then(function (mods) {
      var ciphers = mods[0];
      var curves = mods[1];
      var hkdfMod = mods[2];
      var shaMod = mods[3];

      if (!ciphers.xchacha20poly1305 || !curves.ed25519 || !curves.x25519 ||
          !hkdfMod.hkdf || !shaMod.sha256) {
        throw new Error('内置密码学模块导出不完整');
      }

      var v = {
        xchacha20poly1305: ciphers.xchacha20poly1305,
        ed25519: curves.ed25519,
        x25519: curves.x25519,
        hkdf: hkdfMod.hkdf,
        sha256: shaMod.sha256
      };
      global.TgdVendor = v;
      return v;
    });
  }

  global.TgdVendorReady = load();
  global.TgdVendorReady.catch(function (e) {
    // 不吞掉错误：上层会据此显示"无法建立加密会话"并拒绝明文降级。
    if (global.console && console.error) {
      console.error('[taygedo] 加载内置密码学库失败：', e);
    }
  });
})(window);
