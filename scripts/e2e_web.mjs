/**
 * 端到端协议测试（Node ≥ 18）。
 *
 * 它加载**仓库里真实的内置库与前端客户端文件**，对着**真实的 Rust 服务**
 * 走一遍：资源清单验签 → 握手验签 → 业务请求 → 响应解密，
 * 并额外验证重放与跨接口搬移都会被拒绝。
 *
 * 服务端已无内置 WebUI、也没有自有登录接口：本测试不携带任何 Authorization，
 * 靠"来源是本机回环"通过访问控制（这是独立部署下的受信来源分支）。
 *
 * 用法：
 *   node scripts/e2e_web.mjs http://127.0.0.1:18787
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const base = process.argv[2] || 'http://127.0.0.1:18787';
// 用于验证 CORS 预检声明：取同主机 Origin，保证命中服务端来源白名单
const originForTest = new URL(base).origin;

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');
const vendor = (f) => path.join(root, 'static', 'vendor', f);

// 动态 import 必须传 URL：Windows 下裸盘符路径（C:\...）会被 ESM loader
// 判为未知 scheme，Linux 上两种写法都可，故统一用 pathToFileURL。
const vend = (f) => pathToFileURL(vendor(f)).href;

const { xchacha20poly1305 } = await import(vend('noble-ciphers.js'));
const { ed25519, x25519 } = await import(vend('noble-curves-ed25519.js'));
const { hkdf } = await import(vend('noble-hashes-hkdf.js'));
const { sha256 } = await import(vend('noble-hashes-sha256.js'));

// 模拟浏览器全局，让前端客户端文件可以原样 eval
globalThis.window = globalThis;
globalThis.TgdVendorReady = Promise.resolve({ xchacha20poly1305, ed25519, x25519, hkdf, sha256 });
globalThis.TgdIntegrity = { ok: true, checked: true, error: null };

const clientSrc = fs.readFileSync(path.join(root, 'static', 'taygedo-crypto.js'), 'utf8');
// eslint-disable-next-line no-eval
(0, eval)(clientSrc);

let failures = 0;
function ok(name) { console.log(`  ✓ ${name}`); }
function bad(name, detail) { failures++; console.log(`  ✗ ${name} — ${detail}`); }

function assert(cond, name, detail) {
  if (cond) ok(name); else bad(name, detail || 'assertion failed');
}

// ---------------------------------------------------------------------------
console.log('1) 静态资源签名清单');
{
  const m = await (await fetch(base + '/static/manifest.json')).json();
  const files = (m.files || []).slice().sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
  let canonical = 'taygedo/v2/manifest\0';
  canonical += `manifest_version=${m.manifest_version}\n`;
  canonical += `proto=${m.proto}\n`;
  canonical += `kid=${m.kid}\n`;
  canonical += `identity_pub=${m.identity_pub}\n`;
  canonical += `generated_at=${m.generated_at}\n`;
  for (const f of files) canonical += `file:${f.path}=${f.sha256}:${f.size}\n`;

  const b64d = (s) => Uint8Array.from(Buffer.from(String(s).replace(/-/g, '+').replace(/_/g, '/'), 'base64'));
  const sig = b64d(m.sig);
  const pub = b64d(m.identity_pub);
  assert(ed25519.verify(sig, new TextEncoder().encode(canonical), pub), '清单签名有效');

  let mismatched = [];
  for (const f of files) {
    const body = await (await fetch(base + f.path)).text();
    const h = Array.from(sha256(new TextEncoder().encode(body))).map((b) => b.toString(16).padStart(2, '0')).join('');
    if (h !== f.sha256) mismatched.push(f.path);
  }
  assert(mismatched.length === 0, `清单内全部资源哈希一致（${files.length} 个）`, mismatched.join(', '));
}

// ---------------------------------------------------------------------------
// 1b) 前端资源完整性校验（加载**仓库里真实的 web_guard.js**，对真实服务执行）
// ---------------------------------------------------------------------------
console.log('1b) 前端资源完整性（真实 verifier）');
{
  // 最小 localStorage 替身：web_guard 只用 getItem/setItem
  const store = new Map();
  globalThis.localStorage = {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: (k) => store.delete(k),
  };
  const guardSrc = fs.readFileSync(path.join(root, 'static', 'web_guard.js'), 'utf8');
  const realError = console.error;
  const realWarn = console.warn;
  // 整个 1b 段静音 guard 自身的 console.error/warn：
  // 本段会**故意**制造校验失败（指纹不符、资源被替换），这些日志是预期噪声；
  // 判定结果由下面逐条 assert 输出。段末统一恢复。
  console.error = () => {};
  console.warn = () => {};
  // 脚本末尾会立即用相对路径自检一次（Node 下无 origin，必然失败），一并静音
  // eslint-disable-next-line no-eval
  (0, eval)(guardSrc);

  const good = await globalThis.TgdCheckIntegrity(base);
  assert(good.ok === true, '清单验签 + 逐文件哈希全部通过', good.error);
  assert(
    typeof good.fingerprint === 'string' && good.fingerprint.length === 32,
    '校验结果带身份指纹',
    String(good.fingerprint)
  );
  assert(good.pinned === false, '首次校验为 TOFU（未固定）');

  // 固定正确指纹后应继续通过
  store.set('taygedo_identity_pin', good.fingerprint);
  const pinnedOk = await globalThis.TgdCheckIntegrity(base);
  assert(pinnedOk.ok === true && pinnedOk.pinned === true, '指纹已固定时继续通过');

  // 负例 1：指纹被改成别的值 → 必须拒绝
  store.set('taygedo_identity_pin', 'deadbeefdeadbeefdeadbeefdeadbeef');
  const badPin = await globalThis.TgdCheckIntegrity(base);
  assert(
    badPin.ok === false && String(badPin.error).includes('指纹'),
    '指纹变更被拒绝',
    String(badPin.error)
  );

  // 负例 2：某个资源被换掉 → 必须因哈希不符而拒绝
  store.clear();
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (u, init) => {
    const r = await realFetch(u, init);
    if (String(u).endsWith('/static/taygedo-crypto.js')) {
      const body = (await r.text()) + '\n/* tampered */';
      return new Response(body, { status: 200, headers: { 'content-type': 'application/javascript' } });
    }
    return r;
  };
  const tampered = await globalThis.TgdCheckIntegrity(base);
  globalThis.fetch = realFetch;
  assert(
    tampered.ok === false && String(tampered.error).includes('哈希'),
    '资源被替换时哈希校验拒绝',
    String(tampered.error)
  );

  // 恢复现场：资源恢复后重新校验必须通过（同时把 TgdIntegrity 复位，
  // 否则后续业务用例会被挡在完整性检查之外）
  store.clear();
  const recovered = await globalThis.TgdCheckIntegrity(base);
  assert(recovered.ok === true, '资源恢复后重新校验通过（状态可自愈）', recovered.error);

  console.error = realError;
  console.warn = realWarn;
}

console.log('2) 握手 + 验签 + 业务往返');
// 服务元信息：据此判断本来源是否被强制加密
const meta = await (await fetch(base + '/api/meta')).json();
// 跨源预检必须声明 GET/HEAD 的信封头，否则独立部署的外部 API 客户端读取会被浏览器拦下
{
  const pre = await fetch(base + '/api/logs', {
    method: 'OPTIONS',
    headers: {
      Origin: originForTest,
      'Access-Control-Request-Method': 'GET',
      'Access-Control-Request-Headers': 'X-TGD-Session,X-TGD-Seq,X-TGD-Envelope',
    },
  });
  const allow = String(pre.headers.get('access-control-allow-headers') || '').toLowerCase();
  assert(
    pre.status === 204 && allow.includes('x-tgd-envelope'),
    '跨源预检允许 X-TGD-Envelope',
    `status=${pre.status} allow=${allow}`
  );
}
const C = window.TaygedoCrypto.create({ baseUrl: base });
{
  const hs = await C.handshake();
  assert(hs.proto === 'taygedo/v2', '协议版本为 v2');
  assert(typeof C.fingerprint() === 'string' && C.fingerprint().length === 32, '取得服务端身份指纹');

  // 无内置登录：业务接口靠"来源受信"放行，不带任何 Authorization
  const list = await C.request('/api/accounts', 'GET');
  assert(list.ok && Array.isArray(list.data.accounts), '读取账号列表成功');

  const cfg = await C.request('/api/config', 'GET');
  assert(cfg.ok && typeof cfg.data.default_schedule === 'string', '读取配置成功');

  const logs = await C.request('/api/logs', 'GET');
  assert(logs.ok && Array.isArray(logs.data.logs), '读取日志成功');

  console.log('3) 防重放 / 防搬移 / body 路径');

  const lp = (x) => {
    const b = new TextEncoder().encode(x);
    return Uint8Array.from([(b.length >> 8) & 0xff, b.length & 0xff, ...b]);
  };
  const u64 = (n) => {
    const b = new DataView(new ArrayBuffer(8));
    b.setUint32(0, Math.floor(n / 2 ** 32));
    b.setUint32(4, n >>> 0);
    return new Uint8Array(b.buffer);
  };
  const b64u = (u) => Buffer.from(u).toString('base64')
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

  // 手工构造一个合法信封（指定路径与序号），用于重放/搬移实验
  const encFor = (seqNo, path, method, body) => {
    const nonce = new Uint8Array(24);
    crypto.getRandomValues(nonce);
    const aad = Uint8Array.from([
      ...new TextEncoder().encode('taygedo/v2/req'), 0,
      ...lp(C.kid), ...lp(C.sid), ...u64(seqNo), ...lp(method), ...lp(path), ...nonce
    ]);
    const ct = xchacha20poly1305(C.c2s, nonce, aad).encrypt(new TextEncoder().encode(body || ''));
    return JSON.stringify({ v: 2, kid: C.kid, sid: C.sid, seq: seqNo, n: b64u(nonce), ct: b64u(ct) });
  };

  // 与前端客户端一致：GET/HEAD 把信封放请求头（HTTP 不允许 GET 带 body）
  const send = (p, m, envJson) => {
    const noBody = (m === 'GET' || m === 'HEAD');
    const headers = {
      'Content-Type': 'application/json',
      'X-TGD-Session': C.sid,
    };
    if (noBody) {
      headers['X-TGD-Envelope'] = b64u(Buffer.from(envJson, 'utf8'));
    }
    return fetch(base + p, { method: m, headers, body: noBody ? undefined : envJson });
  };

  const env = encFor(9001, '/api/logs', 'GET', '');
  const first = await send('/api/logs', 'GET', env);
  assert(first.ok, '首次提交被接受');

  const replay = await send('/api/logs', 'GET', env);
  const rj = await replay.json().catch(() => ({}));
  assert(!replay.ok && rj.crypto_error === 'replay', '重放被拒绝', JSON.stringify(rj));

  // 同一份密文换路径（AAD 绑定路径，应鉴权失败）
  const moved = await send('/api/accounts', 'GET', encFor(9002, '/api/logs', 'GET', ''));
  const mj = await moved.json().catch(() => ({}));
  assert(!moved.ok && mj.crypto_error === 'auth_failed', '跨接口搬移被拒绝', JSON.stringify(mj));

  // 有正文的方法仍走 body 路径：解密成功时中间件会重新加密响应（X-TGD-Enc: 1）
  const postEnv = encFor(9003, '/api/accounts', 'POST', '{}');
  const postResp = await send('/api/accounts', 'POST', postEnv);
  assert(
    postResp.headers.get('x-tgd-enc') === '1',
    'POST 信封走 body 路径可正常解密',
    String(postResp.status) + ' enc=' + postResp.headers.get('x-tgd-enc')
  );

  // 同时携带信封正文与信封头视为构造异常
  const bothHeaders = {
    'Content-Type': 'application/json',
    'X-TGD-Session': C.sid,
    'X-TGD-Envelope': b64u(Buffer.from(encFor(9004, '/api/logs', 'GET', ''), 'utf8')),
  };
  const both = await fetch(base + '/api/logs', {
    method: 'POST',
    headers: bothHeaders,
    body: encFor(9005, '/api/logs', 'POST', ''),
  });
  const bj = await both.json().catch(() => ({}));
  assert(!both.ok && bj.crypto_error === 'malformed', '正文与信封头并存被拒绝', JSON.stringify(bj));

  console.log('3b) 错误响应解密 / 会话失效重握手');

  // 业务错误（400）同样被中间件加密：客户端必须解密后拿到 error 文本
  const wrong = await C.request('/api/config', 'POST', { default_schedule: '25:99' });
  assert(!wrong.ok && wrong.status === 400, '非法配置返回 400', String(wrong.status));
  assert(
    typeof (wrong.data && wrong.data.error) === 'string' && !wrong.data.ct,
    '错误响应已解密（调用方拿到的不是密文）',
    JSON.stringify(wrong.data)
  );

  // 服务端不认识的 sid：明文返回 no_session，客户端应自动重握手并重试一次
  const stale = '0'.repeat(32);
  C.sid = stale;
  const retried = await C.request('/api/accounts', 'GET');
  assert(retried.ok && Array.isArray(retried.data.accounts), 'no_session 触发重握手后请求成功');
  assert(C.sid !== stale, '重握手后会话 ID 已更新', String(C.sid));

  console.log('4) 明文降级被拒绝');
  const plain = await fetch(base + '/api/accounts', { method: 'GET' });
  if (meta.crypto_required === true) {
    // crypto_policy=always（或来源不在白名单）：明文必须被硬拒，不得回退
    assert(plain.status === 428, '无信封的业务请求返回 428', String(plain.status));
  } else {
    // auto 策略且来源命中 lan_cidrs：明文是**显式允许**的运维分支，
    // 不是失败后的回退。此时无法在此环境断言 428，需以 always 策略复跑。
    console.log('  ! 当前 crypto_policy 未对本来源强制加密，跳过 428 断言');
    console.log('    以 TAYGEDO_CRYPTO_POLICY=always 启动服务可完整验证该分支');
  }
}

// 自检：服务端已无自有登录，脚本不应再构造授权头
assert(
  !new RegExp('Authori' + 'zation').test(fs.readFileSync(fileURLToPath(import.meta.url), 'utf8')),
  '测试脚本不再使用 Authorization'
);

console.log(failures === 0 ? '\nE2E 全部通过' : `\nE2E 失败项：${failures}`);
process.exit(failures === 0 ? 0 : 1);
