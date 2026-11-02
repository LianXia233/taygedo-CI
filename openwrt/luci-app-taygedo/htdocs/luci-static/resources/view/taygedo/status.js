'use strict';
'require view';
'require uci';
'require rpc';

/*
 * 塔吉多自动签到 - LuCI 独立版 (White Material Design 3)
 */

var TGD = (function () {
	// ---- 内部通道（rpcd/ucode）----
	//
	// 浏览器不直连后端端口：页面调用 ubus 对象 luci.taygedo，由 rpcd 的 ucode
	// 插件（/usr/share/rpcd/ucode/luci.taygedo）在服务端以 curl 调本机回环
	// http://127.0.0.1:<port>，再把结果返回给本页。
	//
	// 身份由 OpenWrt 提供：本页只把 LuCI 会话号（L.env.sessionid）作为参数传
	// 给插件，后端用 `ubus call session get` 校验；页面不接触任何加密材料，
	// 也不再加载 taygedo-crypto.js、不再构造 X-TGD-Envelope 信封。

	function sid() {
		try {
			if (typeof L !== 'undefined' && L.env && L.env.sessionid)
				return String(L.env.sessionid);
		} catch (e) {}
		return '';
	}

	var rpcCall = {
		meta: rpc.declare({ object: 'luci.taygedo', method: 'meta' }),
		config_get: rpc.declare({ object: 'luci.taygedo', method: 'config_get', params: ['sid'] }),
		config_set: rpc.declare({ object: 'luci.taygedo', method: 'config_set', params: ['sid', 'body'] }),
		accounts_list: rpc.declare({ object: 'luci.taygedo', method: 'accounts_list', params: ['sid'] }),
		account_add: rpc.declare({ object: 'luci.taygedo', method: 'account_add', params: ['sid', 'body'] }),
		account_delete: rpc.declare({ object: 'luci.taygedo', method: 'account_delete', params: ['sid', 'id'] }),
		signin: rpc.declare({ object: 'luci.taygedo', method: 'signin', params: ['sid', 'id'] }),
		schedule_set: rpc.declare({ object: 'luci.taygedo', method: 'schedule_set', params: ['sid', 'id', 'body'] }),
		logs: rpc.declare({ object: 'luci.taygedo', method: 'logs', params: ['sid', 'limit'] }),
		send_code: rpc.declare({ object: 'luci.taygedo', method: 'send_code', params: ['sid', 'body'] })
	};

	// 把 ucode 返回的 {ok,http,error,data} 归一为带 code 的 Error，
	// 供上层区分"来源不受信 / 会话缺失 / 会话失效 / 后端未运行"。
	function normalizeError(res) {
		var msg = (res && res.error) || '内部通道调用失败';
		var code = (res && res.data && res.data.code) || '';
		if (!code) {
			if (res && res.http === 403)
				code = 'untrusted_source';
			else if (res && res.http === 401)
				code = 'luci_session_invalid';
			else if (res && res.http === 503)
				code = 'backend_unreachable';
		}
		if (code === 'luci_session_invalid' && /missing/i.test(msg))
			code = 'luci_session_missing';
		var e = new Error(msg);
		e.code = code;
		e.status = res && res.http;
		return e;
	}

	function unsupported(path, method) {
		var e = new Error('未接入内部通道的接口：' + method + ' ' + path);
		e.code = 'unsupported_endpoint';
		return e;
	}

	// 把 (HTTP 方法 + 路径) 映射到固定的 ubus 方法。
	// 路径来自本页内部调用，不接受外部输入；id 由 ucode 侧再做白名单校验。
	function dispatch(path, method, body) {
		method = String(method || 'GET').toUpperCase();
		path = String(path || '');

		var m = path.match(/^\/api\/accounts\/([^\/?#]+)(?:\/(signin|schedule))?$/);
		if (m) {
			var id = decodeURIComponent(m[1]);
			if (!m[2] && method === 'DELETE')
				return rpcCall.account_delete(sid(), id);
			if (m[2] === 'signin' && method === 'POST')
				return rpcCall.signin(sid(), id);
			if (m[2] === 'schedule' && method === 'POST')
				return rpcCall.schedule_set(sid(), id, body || {});
			throw unsupported(path, method);
		}

		if (path === '/api/config')
			return method === 'GET' ? rpcCall.config_get(sid()) : rpcCall.config_set(sid(), body || {});
		if (path === '/api/accounts')
			return method === 'GET' ? rpcCall.accounts_list(sid()) : rpcCall.account_add(sid(), body || {});
		if (path === '/api/send-code' && method === 'POST')
			return rpcCall.send_code(sid(), body || {});
		if (path === '/api/meta' && method === 'GET')
			return rpcCall.meta();

		var lm = path.match(/^\/api\/logs(?:\?limit=(\d+))?/);
		if (lm)
			return rpcCall.logs(sid(), lm[1] ? +lm[1] : 200);

		throw unsupported(path, method);
	}

	function api(path, method, body) {
		return Promise.resolve().then(function () {
			return dispatch(path, method, body);
		}).catch(function (err) {
			// rpc.declare 的拒绝（ubus 层错误）也归一为 Error
			var e = new Error((err && (err.message || err.error)) || 'rpcd 调用失败');
			e.code = (err && err.code) || 'rpc_error';
			throw e;
		}).then(function (res) {
			if (!res)
				throw new Error('rpcd 返回空结果');
			if (!res.ok)
				throw normalizeError(res);
			return res.data || {};
		});
	}

	function esc(s) {
		return String(s == null ? '' : s).replace(/[&<>"']/g, function (c) {
			return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
		});
	}

	function toast(msg, type) {
		var t = document.querySelector('.tgd-toast');
		if (!t) return;
		t.textContent = msg;
		t.className = 'tgd-toast tgd-show ' + (type || '');
		clearTimeout(t._t);
		t._t = setTimeout(function () { t.className = 'tgd-toast'; }, 2600);
	}

	return { esc: esc, toast: toast, api: api, sid: sid };
})();

var AVATAR_DATA_URI = 'data:image/png;base64,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';

var TGD_CSS = [
	'.tgd-root {',
	'  --tgd-primary: #3b66f5;',
	'  --tgd-primary-hover: #2b54e3;',
	'  --tgd-primary-container: #edf2fe;',
	'  --tgd-primary-gradient: linear-gradient(135deg, #3b66f5 0%, #4f46e5 100%);',
	'  --tgd-primary-glow: rgba(59, 102, 245, 0.28);',
	'  --tgd-surface: #ffffff;',
	'  --tgd-surface-card: rgba(255, 255, 255, 0.90);',
	'  --tgd-surface-card-hover: rgba(255, 255, 255, 0.96);',
	'  --tgd-surface-modal: rgba(255, 255, 255, 0.98);',
	'  --tgd-outline: rgba(226, 232, 240, 0.85);',
	'  --tgd-outline-variant: #f1f5f9;',
	'  --tgd-text-primary: #1e293b;',
	'  --tgd-text-secondary: #64748b;',
	'  --tgd-text-tertiary: #94a3b8;',
	'  --tgd-success: #10b981;',
	'  --tgd-success-container: #ecfdf5;',
	'  --tgd-success-text: #047857;',
	'  --tgd-warning: #f59e0b;',
	'  --tgd-warning-container: #fffbeb;',
	'  --tgd-warning-text: #b45309;',
	'  --tgd-danger: #ef4444;',
	'  --tgd-danger-container: #fef2f2;',
	'  --tgd-danger-text: #b91c1c;',
	'  --tgd-shadow-1: 0 1px 3px 0 rgba(16, 24, 40, 0.05), 0 1px 2px -1px rgba(16, 24, 40, 0.04);',
	'  --tgd-shadow-2: 0 4px 16px -2px rgba(16, 24, 40, 0.07), 0 2px 6px -1px rgba(16, 24, 40, 0.04);',
	'  --tgd-shadow-3: 0 12px 28px -4px rgba(16, 24, 40, 0.10), 0 6px 12px -2px rgba(16, 24, 40, 0.06);',
	'  --tgd-shadow-modal: 0 24px 48px -6px rgba(16, 24, 40, 0.16), 0 12px 24px -4px rgba(16, 24, 40, 0.10);',
	'  --tgd-radius-card: 20px;',
	'  --tgd-radius-modal: 24px;',
	'  --tgd-radius-md: 14px;',
	'  --tgd-radius-sm: 10px;',
	'  --tgd-radius-full: 9999px;',
	'  --tgd-bg-mask: rgba(246, 248, 252, 0.85);',
	'  --tgd-mono: ui-monospace, "SF Mono", "Cascadia Code", Consolas, monospace;',
	'  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", "PingFang SC", "Microsoft YaHei", sans-serif;',
	'  color: var(--tgd-text-primary); min-height: 100%; padding: 20px; border-radius: var(--tgd-radius-card);',
	'  background-color: #f6f8fc;',
	'  background-image: linear-gradient(var(--tgd-bg-mask), var(--tgd-bg-mask)), url("https://files.seeusercontent.com/2026/06/29/uu4H/bg1.jpg");',
	'  background-size: cover; background-position: center; background-attachment: fixed;',
	'  box-sizing: border-box; -webkit-font-smoothing: antialiased;',
	'}',
	'.tgd-root * { box-sizing: border-box; margin: 0; padding: 0; }',
	'.tgd-root .tgd-head, .tgd-root .tgd-card, .tgd-root .tgd-stat, .tgd-root .tgd-panel, .tgd-root .tgd-modal, .tgd-root .tgd-login-card {',
	'  background: var(--tgd-surface-card); backdrop-filter: blur(20px); -webkit-backdrop-filter: blur(20px);',
	'  border: 1px solid var(--tgd-outline); box-shadow: var(--tgd-shadow-2);',
	'}',
	'.tgd-root .tgd-head { display: flex; align-items: center; justify-content: space-between; gap: 14px;',
	'  padding: 16px 24px; border-radius: var(--tgd-radius-card); margin-bottom: 20px; flex-wrap: wrap; }',
	'.tgd-root .tgd-brand { display: flex; align-items: center; gap: 14px; min-width: 0; }',
	'.tgd-root .tgd-logo { width: 44px; height: 44px; border-radius: var(--tgd-radius-md); flex: none;',
	'  background: var(--tgd-primary-gradient); color: #fff; display: flex; align-items: center; justify-content: center;',
	'  box-shadow: 0 4px 14px var(--tgd-primary-glow); }',
	'.tgd-root .tgd-brand h1 { font-size: 18px; font-weight: 700; letter-spacing: -0.2px; color: var(--tgd-text-primary); }',
	'.tgd-root .tgd-brand p { font-size: 12px; color: var(--tgd-text-secondary); margin-top: 3px; }',
	'.tgd-root .tgd-hdr-actions { display: flex; align-items: center; gap: 10px; flex: none; }',
	'.tgd-root .tgd-btn { display: inline-flex; align-items: center; justify-content: center; gap: 7px;',
	'  padding: 8px 16px; border-radius: var(--tgd-radius-full); border: 1px solid var(--tgd-outline);',
	'  background: var(--tgd-surface); color: var(--tgd-text-primary); font-size: 13.5px; font-weight: 600; cursor: pointer;',
	'  transition: all .18s cubic-bezier(0.2, 0, 0, 1); white-space: nowrap; min-height: 40px; box-shadow: var(--tgd-shadow-1); }',
	'.tgd-root .tgd-btn:hover { border-color: var(--tgd-primary); color: var(--tgd-primary); transform: translateY(-1px); box-shadow: var(--tgd-shadow-2); }',
	'.tgd-root .tgd-btn:active { transform: scale(0.98); }',
	'.tgd-root .tgd-btn:disabled { opacity: .5; cursor: not-allowed; transform: none !important; box-shadow: none !important; }',
	'.tgd-root .tgd-btn svg { width: 16px; height: 16px; flex: none; }',
	'.tgd-root .tgd-btn.tgd-primary { background: var(--tgd-primary-gradient); color: #fff; border: none; box-shadow: 0 4px 14px var(--tgd-primary-glow); }',
	'.tgd-root .tgd-btn.tgd-primary:hover { opacity: .94; color: #fff; box-shadow: 0 6px 18px var(--tgd-primary-glow); }',
	'.tgd-root .tgd-btn.tgd-danger { background: #fff; border-color: #fee2e2; color: var(--tgd-danger); }',
	'.tgd-root .tgd-btn.tgd-danger:hover { border-color: var(--tgd-danger); color: #fff; background: var(--tgd-danger); box-shadow: 0 4px 14px rgba(239, 68, 68, 0.28); }',
	'.tgd-root .tgd-stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 14px; margin-bottom: 20px; }',
	'.tgd-root .tgd-stat { border-radius: var(--tgd-radius-card); padding: 18px 20px; display: flex; align-items: center; gap: 16px;',
	'  transition: transform .2s cubic-bezier(0.2, 0, 0, 1), box-shadow .2s; }',
	'.tgd-root .tgd-stat:hover { transform: translateY(-2px); box-shadow: var(--tgd-shadow-3); }',
	'.tgd-root .tgd-stat-icon { width: 48px; height: 48px; border-radius: 16px; flex: none; display: flex; align-items: center; justify-content: center; }',
	'.tgd-root .tgd-stat.tgd-total .tgd-stat-icon { background: var(--tgd-primary-container); color: var(--tgd-primary); }',
	'.tgd-root .tgd-stat.tgd-done .tgd-stat-icon { background: var(--tgd-success-container); color: var(--tgd-success); }',
	'.tgd-root .tgd-stat.tgd-pending .tgd-stat-icon { background: var(--tgd-warning-container); color: var(--tgd-warning); }',
	'.tgd-root .tgd-stat-icon svg { width: 22px; height: 22px; }',
	'.tgd-root .tgd-stat-info { min-width: 0; }',
	'.tgd-root .tgd-stat .tgd-num { font-size: 26px; font-weight: 800; font-variant-numeric: tabular-nums; line-height: 1.1; }',
	'.tgd-root .tgd-stat .tgd-label { font-size: 12.5px; color: var(--tgd-text-secondary); margin-top: 4px; font-weight: 500; }',
	'.tgd-root .tgd-stat.tgd-total .tgd-num { color: var(--tgd-primary); }',
	'.tgd-root .tgd-stat.tgd-done .tgd-num { color: var(--tgd-success); }',
	'.tgd-root .tgd-stat.tgd-pending .tgd-num { color: var(--tgd-warning); }',
	'.tgd-root .tgd-main { display: grid; grid-template-columns: 1fr; gap: 20px; }',
	'@media (min-width: 960px) { .tgd-root .tgd-main { grid-template-columns: 1.25fr 1fr; align-items: start; } }',
	'.tgd-root .tgd-accounts { display: grid; grid-template-columns: 1fr; gap: 16px; }',
	'@media (min-width: 640px) { .tgd-root .tgd-accounts { grid-template-columns: repeat(2, 1fr); } }',
	'.tgd-root .tgd-card { border-radius: var(--tgd-radius-card); padding: 20px; display: flex; flex-direction: column; gap: 16px;',
	'  position: relative; overflow: hidden; transition: transform .2s cubic-bezier(0.2, 0, 0, 1), box-shadow .2s, border-color .2s; }',
	'.tgd-root .tgd-card:hover { transform: translateY(-2px); box-shadow: var(--tgd-shadow-3); border-color: rgba(59, 102, 245, 0.35); }',
	'.tgd-root .tgd-card::before { content: ""; position: absolute; top: 0; left: 0; right: 0; height: 3px;',
	'  background: var(--tgd-primary-gradient); opacity: 0; transition: opacity .2s; }',
	'.tgd-root .tgd-card:hover::before { opacity: 1; }',
	'.tgd-root .tgd-card-top { display: flex; align-items: flex-start; gap: 14px; }',
	'.tgd-root .tgd-avatar { width: 50px; height: 50px; border-radius: 16px; flex: none;',
	'  overflow: hidden; background: #f8fafc; border: 2px solid #ffffff; box-shadow: 0 4px 12px rgba(0, 0, 0, 0.08); }',
	'.tgd-root .tgd-avatar img { width: 100%; height: 100%; object-fit: cover; display: block; }',
	'.tgd-root .tgd-card-info { flex: 1; min-width: 0; }',
	'.tgd-root .tgd-card-info .tgd-name-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }',
	'.tgd-root .tgd-card-info .tgd-name { font-weight: 700; font-size: 16px; color: var(--tgd-text-primary); letter-spacing: -0.2px; }',
	'.tgd-root .tgd-card-info .tgd-meta-tags { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 8px; }',
	'.tgd-root .tgd-meta-tag { font-size: 11.5px; padding: 3px 9px; border-radius: var(--tgd-radius-full); background: var(--tgd-outline-variant);',
	'  color: var(--tgd-text-secondary); display: inline-flex; align-items: center; gap: 5px; white-space: nowrap;',
	'  max-width: 170px; overflow: hidden; text-overflow: ellipsis; font-weight: 500; }',
	'.tgd-root .tgd-meta-tag svg { width: 12px; height: 12px; flex: none; opacity: .75; }',
	'.tgd-root .tgd-meta-tag.tgd-uid { font-family: var(--tgd-mono); font-size: 11px; }',
	'.tgd-root .tgd-badge { font-size: 11px; font-weight: 700; padding: 3px 9px; border-radius: var(--tgd-radius-full); display: inline-flex; align-items: center; gap: 4px; flex: none; }',
	'.tgd-root .tgd-badge.tgd-ok { background: var(--tgd-success-container); color: var(--tgd-success-text); }',
	'.tgd-root .tgd-badge.tgd-wait { background: var(--tgd-warning-container); color: var(--tgd-warning-text); }',
	'.tgd-root .tgd-row { display: flex; align-items: center; justify-content: space-between; gap: 12px;',
	'  background: rgba(248, 250, 252, 0.7); padding: 8px 12px; border-radius: var(--tgd-radius-md); border: 1px solid var(--tgd-outline); }',
	'.tgd-root .tgd-row .tgd-lab { font-size: 13px; font-weight: 500; color: var(--tgd-text-secondary); display: flex; align-items: center; gap: 6px; }',
	'.tgd-root .tgd-row .tgd-lab svg { width: 15px; height: 15px; color: var(--tgd-primary); }',
	'.tgd-root .tgd-time { border: 1px solid var(--tgd-outline); border-radius: var(--tgd-radius-sm); padding: 6px 10px;',
	'  background: #ffffff; color: var(--tgd-text-primary); font-family: var(--tgd-mono); font-size: 13px; font-weight: 600;',
	'  outline: none; transition: border-color .15s; min-height: 34px; }',
	'.tgd-root .tgd-time:focus { border-color: var(--tgd-primary); }',
	'.tgd-root .tgd-card-actions { display: flex; gap: 10px; }',
	'.tgd-root .tgd-card-actions .tgd-btn { flex: 1; padding: 9px 12px; font-size: 13px; border-radius: var(--tgd-radius-md); }',
	'.tgd-root .tgd-empty { grid-column: 1 / -1; text-align: center; padding: 48px 20px;',
	'  color: var(--tgd-text-secondary); border: 2px dashed var(--tgd-outline); border-radius: var(--tgd-radius-card); background: rgba(255, 255, 255, 0.6); }',
	'.tgd-root .tgd-empty .tgd-big { font-size: 15.5px; font-weight: 700; color: var(--tgd-text-primary); margin-bottom: 6px; }',
	'.tgd-root .tgd-panel { border-radius: var(--tgd-radius-card); overflow: hidden; }',
	'.tgd-root .tgd-panel-head { display: flex; align-items: center; justify-content: space-between; padding: 16px 20px; border-bottom: 1px solid var(--tgd-outline); }',
	'.tgd-root .tgd-panel-head h2 { font-size: 15px; font-weight: 700; display: flex; align-items: center; gap: 8px; color: var(--tgd-text-primary); }',
	'.tgd-root .tgd-panel-head h2 svg { width: 18px; height: 18px; color: var(--tgd-primary); }',
	'.tgd-root .tgd-logs { height: 400px; overflow-y: auto; padding: 14px 18px; background: #0f172a;',
	'  font-family: var(--tgd-mono); font-size: 12px; line-height: 1.7; -webkit-overflow-scrolling: touch; }',
	'.tgd-root .tgd-log-line { color: #e2e8f0; word-break: break-all; margin-bottom: 2px; }',
	'.tgd-root .tgd-log-line .tgd-ts { color: #64748b; margin-right: 8px; }',
	'.tgd-root .tgd-log-line .tgd-lv { font-weight: 700; margin-right: 8px; border-radius: 4px; padding: 1px 4px; font-size: 10.5px; }',
	'.tgd-root .tgd-log-line.tgd-info .tgd-lv { color: #38bdf8; background: rgba(56, 189, 248, 0.12); }',
	'.tgd-root .tgd-log-line.tgd-error .tgd-lv { color: #f43f5e; background: rgba(244, 63, 94, 0.12); }',
	'.tgd-root .tgd-log-line.tgd-warn .tgd-lv { color: #fbbf24; background: rgba(251, 191, 36, 0.12); }',
	'.tgd-root .tgd-log-empty { color: #64748b; padding: 20px 0; text-align: center; }',
	'.tgd-root .tgd-log-tabs { display: flex; gap: 6px; padding: 10px 16px; background: rgba(248, 250, 252, 0.6);',
	'  border-bottom: 1px solid var(--tgd-outline); overflow-x: auto; -webkit-overflow-scrolling: touch; scrollbar-width: none; }',
	'.tgd-root .tgd-log-tabs::-webkit-scrollbar { display: none; }',
	'.tgd-root .tgd-log-tab { padding: 5px 14px; border-radius: var(--tgd-radius-full); font-size: 12px; font-weight: 600;',
	'  cursor: pointer; white-space: nowrap; border: 1px solid transparent; background: transparent; color: var(--tgd-text-secondary); transition: all .15s; }',
	'.tgd-root .tgd-log-tab:hover { background: rgba(0,0,0,0.04); color: var(--tgd-text-primary); }',
	'.tgd-root .tgd-log-tab.tgd-active { background: var(--tgd-primary); color: #fff; box-shadow: 0 2px 8px var(--tgd-primary-glow); }',
	'.tgd-root .tgd-login-wrap { display: flex; align-items: center; justify-content: center; padding: 30px 16px; }',
	'.tgd-root .tgd-login-card { width: 100%; max-width: 420px; border-radius: var(--tgd-radius-modal); padding: 36px 30px; text-align: center; }',
	'.tgd-root .tgd-login-card .tgd-logo { width: 56px; height: 56px; border-radius: 18px; margin: 0 auto 16px;',
	'  background: var(--tgd-primary-gradient); color: #fff; display: flex; align-items: center; justify-content: center;',
	'  box-shadow: 0 6px 18px var(--tgd-primary-glow); }',
	'.tgd-root .tgd-login-card h1 { font-size: 20px; font-weight: 800; color: var(--tgd-text-primary); letter-spacing: -0.3px; }',
	'.tgd-root .tgd-login-card .tgd-sub { font-size: 13.5px; color: var(--tgd-text-secondary); margin: 6px 0 22px; line-height: 1.6; }',
	'.tgd-root .tgd-login-card .tgd-hint { font-size: 12px; color: var(--tgd-text-secondary); margin-top: 16px; line-height: 1.7; text-align: left;',
	'  background: rgba(248, 250, 252, 0.85); border: 1px solid var(--tgd-outline); border-radius: var(--tgd-radius-md); padding: 12px 14px; }',
	'.tgd-root .tgd-login-card .tgd-hint code { font-family: var(--tgd-mono); font-size: 11.5px; color: var(--tgd-primary); font-weight: 600; }',
	'.tgd-root .tgd-input { border: 1px solid var(--tgd-outline); border-radius: var(--tgd-radius-md); padding: 11px 14px; font-size: 14px;',
	'  background: rgba(248, 250, 252, 0.85); color: var(--tgd-text-primary); outline: none; transition: all .18s; width: 100%; min-height: 44px; }',
	'.tgd-root .tgd-input:focus { border-color: var(--tgd-primary); background: #ffffff; box-shadow: 0 0 0 3px rgba(59, 102, 245, 0.15); }',
	'.tgd-root .tgd-field { display: flex; flex-direction: column; gap: 6px; margin-bottom: 14px; text-align: left; }',
	'.tgd-root .tgd-field label { font-size: 13px; font-weight: 600; color: var(--tgd-text-secondary); }',
	'.tgd-root .tgd-modal-mask { position: fixed; inset: 0; background: rgba(15, 23, 42, 0.45);',
	'  backdrop-filter: blur(8px); -webkit-backdrop-filter: blur(8px); z-index: 2000;',
	'  display: flex; align-items: center; justify-content: center; padding: 16px; opacity: 0; pointer-events: none; transition: opacity .2s; }',
	'.tgd-root .tgd-modal-mask.tgd-show { opacity: 1; pointer-events: auto; }',
	'.tgd-root .tgd-modal { background: var(--tgd-surface-modal); border-radius: var(--tgd-radius-modal);',
	'  width: 100%; max-width: 460px; max-height: 90vh; overflow-y: auto; box-shadow: var(--tgd-shadow-modal);',
	'  transform: translateY(12px) scale(0.98); transition: transform .2s cubic-bezier(0.2, 0, 0, 1); }',
	'.tgd-root .tgd-modal-mask.tgd-show .tgd-modal { transform: translateY(0) scale(1); }',
	'.tgd-root .tgd-modal-head { padding: 18px 24px; border-bottom: 1px solid var(--tgd-outline); display: flex; justify-content: space-between; align-items: center; }',
	'.tgd-root .tgd-modal-head h3 { font-size: 17px; font-weight: 700; color: var(--tgd-text-primary); }',
	'.tgd-root .tgd-modal-close { background: none; border: none; color: var(--tgd-text-tertiary); cursor: pointer; font-size: 22px; line-height: 1; padding: 4px; border-radius: 50%; width: 32px; height: 32px; display: flex; align-items: center; justify-content: center; transition: all .15s; }',
	'.tgd-root .tgd-modal-close:hover { background: var(--tgd-outline-variant); color: var(--tgd-text-primary); }',
	'.tgd-root .tgd-modal-body { padding: 20px 24px 28px; }',
	'.tgd-root .tgd-tabs { display: flex; gap: 4px; background: var(--tgd-outline-variant); border-radius: var(--tgd-radius-md); padding: 4px; margin-bottom: 16px; }',
	'.tgd-root .tgd-tab { flex: 1; text-align: center; padding: 9px; border-radius: 10px; font-size: 13.5px; font-weight: 600;',
	'  cursor: pointer; color: var(--tgd-text-secondary); border: none; background: none; transition: all .15s; }',
	'.tgd-root .tgd-tab.tgd-active { background: #ffffff; color: var(--tgd-primary); box-shadow: var(--tgd-shadow-1); }',
	'.tgd-root .tgd-captcha-row { display: flex; gap: 10px; }',
	'.tgd-root .tgd-captcha-row .tgd-input { flex: 1; }',
	'.tgd-root .tgd-captcha-row .tgd-btn { flex: none; border-radius: var(--tgd-radius-md); }',
	'.tgd-root .tgd-switch-row { display: flex; align-items: center; justify-content: space-between; margin-bottom: 14px; }',
	'.tgd-root .tgd-switch-row .tgd-txt { font-size: 14px; font-weight: 500; color: var(--tgd-text-primary); }',
	'.tgd-root .tgd-switch { position: relative; width: 48px; height: 26px; }',
	'.tgd-root .tgd-switch input { opacity: 0; width: 0; height: 0; }',
	'.tgd-root .tgd-switch .tgd-slider { position: absolute; inset: 0; background: #cbd5e1; border-radius: var(--tgd-radius-full); cursor: pointer; transition: all .25s cubic-bezier(0.4, 0, 0.2, 1); }',
	'.tgd-root .tgd-switch .tgd-slider:before { content: ""; position: absolute; width: 20px; height: 20px; left: 3px; top: 3px; background: #ffffff; border-radius: 50%; box-shadow: 0 2px 4px rgba(0,0,0,0.2); transition: all .25s cubic-bezier(0.4, 0, 0.2, 1); }',
	'.tgd-root .tgd-switch input:checked + .tgd-slider { background: var(--tgd-primary); }',
	'.tgd-root .tgd-switch input:checked + .tgd-slider:before { transform: translateX(22px); }',
	'.tgd-root .tgd-divider { border: none; border-top: 1px solid var(--tgd-outline); margin: 8px 0 16px; }',
	'.tgd-root .tgd-section-title { font-size: 13px; font-weight: 700; color: var(--tgd-primary); letter-spacing: 0.3px; text-transform: uppercase; margin-bottom: 12px; }',
	'.tgd-toast { position: fixed; bottom: 28px; left: 50%; transform: translateX(-50%) translateY(20px);',
	'  background: #1e293b; color: #ffffff; padding: 12px 24px; border-radius: var(--tgd-radius-full); font-size: 14px; font-weight: 600;',
	'  z-index: 3000; opacity: 0; transition: all .25s cubic-bezier(0.2, 0, 0, 1); box-shadow: var(--tgd-shadow-3); max-width: 90vw; pointer-events: none; }',
	'.tgd-toast.tgd-show { opacity: 1; transform: translateX(-50%) translateY(0); pointer-events: auto; }',
	'.tgd-toast.tgd-err { background: var(--tgd-danger); color: #fff; }',
	'.tgd-toast.tgd-ok { background: var(--tgd-success); color: #fff; }',
	'.tgd-root .tgd-spin { animation: tgd-spin 1s linear infinite; }',
	'@keyframes tgd-spin { to { transform: rotate(360deg); } }',
	'@media (max-width: 640px) {',
	'  .tgd-root { padding: 12px; }',
	'  .tgd-root .tgd-head { padding: 14px 16px; border-radius: 16px; margin-bottom: 14px; }',
	'  .tgd-root .tgd-logo { width: 38px; height: 38px; }',
	'  .tgd-root .tgd-brand h1 { font-size: 16px; }',
	'  .tgd-root .tgd-stats { gap: 8px; margin-bottom: 14px; }',
	'  .tgd-root .tgd-stat { padding: 12px 14px; gap: 10px; border-radius: 14px; }',
	'  .tgd-root .tgd-stat-icon { width: 38px; height: 38px; border-radius: 12px; }',
	'  .tgd-root .tgd-stat-icon svg { width: 18px; height: 18px; }',
	'  .tgd-root .tgd-stat .tgd-num { font-size: 20px; }',
	'  .tgd-root .tgd-stat .tgd-label { font-size: 11px; }',
	'  .tgd-root .tgd-logs { height: 300px; padding: 12px 14px; }',
	'  .tgd-root .tgd-card { border-radius: 16px; padding: 16px; }',
	'  .tgd-root .tgd-avatar { width: 44px; height: 44px; }',
	'}'
].join('\n');

var ICONS = {
	logo: '<svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 11h4M8 9v4"/><path d="M15 11h.01M18 13h.01"/><path d="M17.32 5H6.68a4 4 0 0 0-3.978 3.59c-.006.052-.01.101-.017.152C2.604 9.416 2 14.456 2 16a3 3 0 0 0 3 3c1 0 1.5-.5 2-1l1.414-1.414A2 2 0 0 1 9.828 16h4.344a2 2 0 0 1 1.414.586L17 18c.5.5 1 1 2 1a3 3 0 0 0 3-3c0-1.545-.604-6.584-.685-7.258-.007-.05-.011-.1-.017-.151A4 4 0 0 0 17.32 5z"/></svg>',
	clock: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>',
	signin: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><path d="M22 4 12 14.01l-3-3"/></svg>',
	del: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>',
	refresh: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/></svg>',
	gear: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>',
	external: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><path d="M15 3h6v6"/><path d="M10 14 21 3"/></svg>',
	phone: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6A19.79 19.79 0 0 1 2.18 4.18 2 2 0 0 1 4 2h3a2 2 0 0 1 2 1.72c.13.96.36 1.9.7 2.81a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45c.9.34 1.85.57 2.81.7A2 2 0 0 1 22 16.92z"/></svg>',
	users: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M23 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/></svg>',
	checkCircle: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><path d="M22 4 12 14.01l-3-3"/></svg>',
	dot: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><path d="M12 12h.01"/></svg>'
};

function mainHtml() {
	return [
		'<div class="tgd-head">',
		'  <div class="tgd-brand">',
		'    <div class="tgd-logo">' + ICONS.logo + '</div>',
		'    <div><h1>塔吉多自动签到</h1><p>多账号 · 每日定时 · 云游戏时长</p></div>',
		'  </div>',
		'  <div class="tgd-hdr-actions">',
		'    <button class="tgd-btn" id="tgd-settings" title="设置">' + ICONS.gear + '</button>',
		'    <button class="tgd-btn tgd-primary" id="tgd-add">＋ 添加账号</button>',
		'  </div>',
		'</div>',
		'<div class="tgd-stats">',
		'  <div class="tgd-stat tgd-total">',
		'    <div class="tgd-stat-icon">' + ICONS.users + '</div>',
		'    <div class="tgd-stat-info"><div class="tgd-num" id="tgd-stat-total">0</div><div class="tgd-label">总账号</div></div>',
		'  </div>',
		'  <div class="tgd-stat tgd-done">',
		'    <div class="tgd-stat-icon">' + ICONS.checkCircle + '</div>',
		'    <div class="tgd-stat-info"><div class="tgd-num" id="tgd-stat-done">0</div><div class="tgd-label">今日已签到</div></div>',
		'  </div>',
		'  <div class="tgd-stat tgd-pending">',
		'    <div class="tgd-stat-icon">' + ICONS.clock + '</div>',
		'    <div class="tgd-stat-info"><div class="tgd-num" id="tgd-stat-pending">0</div><div class="tgd-label">待签到</div></div>',
		'  </div>',
		'</div>',
		'<div class="tgd-main">',
		'  <div class="tgd-accounts" id="tgd-accounts"></div>',
		'  <div class="tgd-panel">',
		'    <div class="tgd-panel-head"><h2>' + ICONS.clock + ' 运行日志</h2>',
		'      <button class="tgd-btn" id="tgd-refresh-logs" title="刷新">' + ICONS.refresh + '</button></div>',
		'    <div class="tgd-log-tabs" id="tgd-log-tabs"><button class="tgd-log-tab tgd-active" data-filter="all">全部</button></div>',
		'    <div class="tgd-logs" id="tgd-logs"><div class="tgd-log-empty">暂无日志</div></div>',
		'  </div>',
		'</div>'
	].join('');
}

// 鉴权未通过时的提示页。
//
// OpenWrt 集成模式下没有"应用自有登录"，登录态完全来自 LuCI/ubus 会话：
// 这里只解释原因并引导重新加载 LuCI，不再提供任何"打开外部 WebUI"或
// "改 UCI 开免鉴权"的入口。
function noAuthHtml(detail) {
	return [
		'<div class="tgd-login-wrap">',
		'  <div class="tgd-login-card">',
		'    <div class="tgd-logo">' + ICONS.logo + '</div>',
		'    <h1>塔吉多自动签到</h1>',
		'    <div class="tgd-sub">本页通过 LuCI 会话访问签到服务，当前未通过 OpenWrt 鉴权</div>',
		detail ? '    <div class="tgd-hint">' + TGD.esc(detail) + '</div>' : '',
		'    <button class="tgd-btn tgd-primary" id="tgd-auth-retry" style="width:100%;min-height:46px;font-size:15px">' + ICONS.refresh + ' 重新加载</button>',
		'    <div class="tgd-hint">内置 WebUI 已退役，LuCI 是唯一入口，登录与鉴权由 OpenWrt 负责。<br>若 LuCI 会话已过期，请重新登录 LuCI 后再打开本页。</div>',
		'  </div>',
		'</div>'
	].join('');
}

function serviceDownHtml() {
	return [
		'<div class="tgd-login-wrap">',
		'  <div class="tgd-login-card">',
		'    <div class="tgd-logo">' + ICONS.logo + '</div>',
		'    <h1>塔吉多自动签到</h1>',
		'    <div class="tgd-sub">签到服务未运行（端口 8787 无响应）</div>',
		'    <button class="tgd-btn tgd-primary" id="tgd-retry-btn" style="width:100%;min-height:46px;font-size:15px">' + ICONS.external + ' 重试连接</button>',
		'    <div class="tgd-hint">0.4.12 起安装包会自动注册开机自启并启动服务。若仍显示此页，请在路由器 SSH 执行：<br><code>/etc/init.d/taygedo enable</code><br><code>/etc/init.d/taygedo start</code></div>',
		'  </div>',
		'</div>'
	].join('');
}

function renderServiceDown() {
	var root = document.getElementById('tgd-root');
	root.innerHTML = '<style>' + TGD_CSS + '</style>' + serviceDownHtml() + '<div class="tgd-toast"></div>';
	document.getElementById('tgd-retry-btn').addEventListener('click', function () {
		window.location.reload();
	});
}

function addModalHtml() {
	return [
		'<div class="tgd-modal-mask" id="tgd-add-modal">',
		'  <div class="tgd-modal">',
		'    <div class="tgd-modal-head"><h3>添加账号</h3><button class="tgd-modal-close" data-close="tgd-add-modal">×</button></div>',
		'    <div class="tgd-modal-body">',
		'      <div class="tgd-tabs">',
		'        <button class="tgd-tab tgd-active" data-mode="password">密码登录</button>',
		'        <button class="tgd-tab" data-mode="captcha">验证码登录</button>',
		'      </div>',
		'      <div class="tgd-field"><label>手机号</label><input class="tgd-input" id="tgd-phone" placeholder="请输入手机号" inputmode="numeric"></div>',
		'      <div class="tgd-field" id="tgd-password-field"><label>密码</label><input class="tgd-input" id="tgd-password" type="password" placeholder="请输入密码"></div>',
		'      <div class="tgd-field" id="tgd-captcha-field" style="display:none"><label>验证码</label>',
		'        <div class="tgd-captcha-row"><input class="tgd-input" id="tgd-captcha" placeholder="短信验证码" inputmode="numeric">',
		'        <button class="tgd-btn" id="tgd-send-code">发送验证码</button></div></div>',
		'      <div class="tgd-field"><label>备注名（可选）</label><input class="tgd-input" id="tgd-name" placeholder="如：主账号"></div>',
		'      <button class="tgd-btn tgd-primary" id="tgd-login-submit" style="width:100%;min-height:46px;margin-top:4px">登 录</button>',
		'    </div>',
		'  </div>',
		'</div>'
	].join('');
}

function settingsModalHtml() {
	return [
		'<div class="tgd-modal-mask" id="tgd-settings-modal">',
		'  <div class="tgd-modal">',
		'    <div class="tgd-modal-head"><h3>全局设置</h3><button class="tgd-modal-close" data-close="tgd-settings-modal">×</button></div>',
		'    <div class="tgd-modal-body">',
		'      <div class="tgd-field"><label>默认签到时间（每天）</label><input class="tgd-input tgd-time" type="time" id="tgd-cfg-schedule"></div>',
		'      <div class="tgd-switch-row"><span class="tgd-txt">金币任务</span><label class="tgd-switch"><input type="checkbox" id="tgd-cfg-coin"><span class="tgd-slider"></span></label></div>',
		'      <div class="tgd-switch-row"><span class="tgd-txt">云异环时长</span><label class="tgd-switch"><input type="checkbox" id="tgd-cfg-cloud"><span class="tgd-slider"></span></label></div>',
		'      <div class="tgd-field"><label>分享平台</label><input class="tgd-input" id="tgd-cfg-share" placeholder="qq / wechat / weibo"></div>',
		'      <button class="tgd-btn tgd-primary" id="tgd-cfg-save" style="width:100%;min-height:44px;margin-top:4px">保存设置</button>',
		'      <hr class="tgd-divider">',
		'      <div class="tgd-section-title">传输加密</div>',
		'      <div class="tgd-field"><label>加密策略</label>',
		'        <select class="tgd-input" id="tgd-cfg-crypto-policy">',
		'          <option value="auto">auto - 内网明文、外网强制加密</option>',
		'          <option value="always">always - 始终强制加密</option>',
		'          <option value="never">never - 始终明文（仅限可信内网）</option>',
		'        </select></div>',
		'      <div class="tgd-field"><label>本机内网网段（逗号分隔 CIDR）</label><input class="tgd-input" id="tgd-cfg-lan-cidrs" placeholder="192.168.0.0/16,10.0.0.0/8"></div>',
		'      <button class="tgd-btn tgd-primary" id="tgd-crypto-save" style="width:100%;min-height:44px;margin-top:4px">保存加密设置</button>',
		'      <hr class="tgd-divider">',
		'      <div class="tgd-hint">登录与账号密码由 OpenWrt（LuCI）负责，本应用不再提供独立登录口令。</div>',
		'    </div>',
		'  </div>',
		'</div>'
	].join('');
}

function renderMain() {
	var root = document.getElementById('tgd-root');
	root.innerHTML = '<style>' + TGD_CSS + '</style>' + mainHtml() +
		addModalHtml() + settingsModalHtml() + '<div class="tgd-toast"></div>';
	bindMainEvents();
	loadAccounts();
	loadLogs();
	startPoll();
}

function renderNoAuth(detail) {
	var root = document.getElementById('tgd-root');
	root.innerHTML = '<style>' + TGD_CSS + '</style>' + noAuthHtml(detail) + '<div class="tgd-toast"></div>';
	document.getElementById('tgd-auth-retry').addEventListener('click', function () {
		window.location.reload();
	});
}

function bindMainEvents() {
	document.getElementById('tgd-settings').addEventListener('click', openSettings);
	document.getElementById('tgd-add').addEventListener('click', function () { openAddModal('password'); });
	document.getElementById('tgd-refresh-logs').addEventListener('click', loadLogs);

	document.querySelectorAll('[data-close]').forEach(function (b) {
		b.addEventListener('click', function () { closeModal(b.getAttribute('data-close')); });
	});
	document.querySelectorAll('.tgd-modal-mask').forEach(function (m) {
		m.addEventListener('click', function (e) { if (e.target === m) m.classList.remove('tgd-show'); });
	});

	document.querySelectorAll('.tgd-tab').forEach(function (t) {
		t.addEventListener('click', function () { openAddModal(t.getAttribute('data-mode')); });
	});

	document.getElementById('tgd-send-code').addEventListener('click', sendCode);
	document.getElementById('tgd-login-submit').addEventListener('click', addAccount);
	document.getElementById('tgd-cfg-save').addEventListener('click', saveConfig);
	document.getElementById('tgd-crypto-save').addEventListener('click', saveCrypto);
}

function openAddModal(mode) {
	document.querySelectorAll('.tgd-tab').forEach(function (x) { x.classList.toggle('tgd-active', x.getAttribute('data-mode') === mode); });
	document.getElementById('tgd-password-field').style.display = mode === 'password' ? '' : 'none';
	document.getElementById('tgd-captcha-field').style.display = mode === 'captcha' ? '' : 'none';
	document.getElementById('tgd-add-modal').classList.add('tgd-show');
}

function closeModal(id) { document.getElementById(id).classList.remove('tgd-show'); }

var sendCodeTimer = null;
var pollTimer = null;
var allAccounts = [];
var allLogsRaw = [];
var currentLogFilter = 'all';

function sendCode() {
	var phone = document.getElementById('tgd-phone').value.trim();
	if (!phone) { TGD.toast('请输入手机号', 'tgd-err'); return; }
	var btn = document.getElementById('tgd-send-code');
	btn.disabled = true;
	TGD.api('/api/send-code', 'POST', { phone: phone }).then(function () {
		TGD.toast('验证码已发送', 'tgd-ok');
		var n = 60;
		btn.textContent = n + 's';
		sendCodeTimer = setInterval(function () {
			n--;
			if (n <= 0) { clearInterval(sendCodeTimer); btn.textContent = '发送验证码'; btn.disabled = false; }
			else btn.textContent = n + 's';
		}, 1000);
	}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); btn.disabled = false; });
}

function addAccount() {
	var phone = document.getElementById('tgd-phone').value.trim();
	var name = document.getElementById('tgd-name').value.trim();
	var mode = document.querySelector('.tgd-tab.tgd-active').getAttribute('data-mode');
	if (!phone) { TGD.toast('请输入手机号', 'tgd-err'); return; }
	var btn = document.getElementById('tgd-login-submit');
	btn.disabled = true; btn.textContent = '登录中...';
	var body = { phone: phone, mode: mode, name: name || null };
	if (mode === 'password') {
		body.password = document.getElementById('tgd-password').value;
		if (!body.password) { TGD.toast('请输入密码', 'tgd-err'); btn.disabled = false; btn.textContent = '登录'; return; }
	} else {
		body.captcha = document.getElementById('tgd-captcha').value;
		if (!body.captcha) { TGD.toast('请输入验证码', 'tgd-err'); btn.disabled = false; btn.textContent = '登录'; return; }
	}
	TGD.api('/api/accounts', 'POST', body).then(function () {
		TGD.toast('登录成功', 'tgd-ok');
		closeModal('tgd-add-modal');
		document.getElementById('tgd-password').value = '';
		document.getElementById('tgd-captcha').value = '';
		document.getElementById('tgd-phone').value = '';
		document.getElementById('tgd-name').value = '';
		loadAccounts();
	}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); });
	btn.disabled = false; btn.textContent = '登录';
}

function loadAccounts() {
	TGD.api('/api/accounts', 'GET').then(function (data) {
		var list = data.accounts || [];
		allAccounts = list;
		document.getElementById('tgd-stat-total').textContent = list.length;
		document.getElementById('tgd-stat-done').textContent = list.filter(function (a) { return a.signed_today; }).length;
		document.getElementById('tgd-stat-pending').textContent = list.filter(function (a) { return !a.signed_today; }).length;
		renderAccounts(list);
		buildLogTabs(list);
		if (allLogsRaw.length) renderLogs();
	}).catch(function (e) { console.error(e); });
}

function renderAccounts(list) {
	var wrap = document.getElementById('tgd-accounts');
	if (!list.length) {
		wrap.innerHTML = '<div class="tgd-empty"><div class="tgd-big">还没有账号</div>点击右上角「添加账号」开始使用</div>';
		return;
	}
	wrap.innerHTML = list.map(function (a) {
		var badge = a.signed_today
			? '<span class="tgd-badge tgd-ok"><svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><polyline points="20 6 9 17 4 12"/></svg>今日已签</span>'
			: '<span class="tgd-badge tgd-wait"><svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>待签到</span>';
		return '<div class="tgd-card" data-id="' + TGD.esc(a.id) + '">' +
			'<div class="tgd-card-top">' +
			'<div class="tgd-avatar"><img src="' + AVATAR_DATA_URI + '" alt="avatar"></div>' +
			'<div class="tgd-card-info">' +
			'<div class="tgd-name-row"><div class="tgd-name">' + TGD.esc(a.role_name || a.name) + '</div>' + badge + '</div>' +
			'<div class="tgd-meta-tags">' +
			(a.phone ? '<span class="tgd-meta-tag">' + ICONS.phone + TGD.esc(a.phone) + '</span>' : '') +
			'<span class="tgd-meta-tag tgd-uid">' + ICONS.dot + 'UID ' + TGD.esc(a.uid || '-') + '</span>' +
			'</div>' +
			'</div>' +
			'</div>' +
			'<div class="tgd-row"><span class="tgd-lab">' + ICONS.clock + ' 每日签到时间</span>' +
			'<input class="tgd-input tgd-time" type="time" value="' + TGD.esc(a.schedule) + '" data-schedule="' + TGD.esc(a.id) + '"></div>' +
			'<div class="tgd-card-actions">' +
			'<button class="tgd-btn tgd-primary" data-signin="' + TGD.esc(a.id) + '">' + ICONS.signin + ' 立即签到</button>' +
			'<button class="tgd-btn tgd-danger" data-del="' + TGD.esc(a.id) + '">' + ICONS.del + ' 删除</button>' +
			'</div></div>';
	}).join('');

	wrap.querySelectorAll('[data-schedule]').forEach(function (inp) {
		inp.addEventListener('change', function () {
			var id = inp.getAttribute('data-schedule');
			TGD.api('/api/accounts/' + id + '/schedule', 'POST', { time: inp.value || null }).then(function () {
				TGD.toast('签到时间已更新', 'tgd-ok');
				loadAccounts();
			}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); });
		});
	});
	wrap.querySelectorAll('[data-signin]').forEach(function (btn) {
		btn.addEventListener('click', function () {
			var id = btn.getAttribute('data-signin');
			btn.disabled = true;
			TGD.api('/api/accounts/' + id + '/signin', 'POST', { force: true }).then(function () {
				TGD.toast('签到完成', 'tgd-ok');
			}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); }).finally(function () {
				btn.disabled = false;
				loadAccounts();
				loadLogs();
			});
		});
	});
	wrap.querySelectorAll('[data-del]').forEach(function (btn) {
		btn.addEventListener('click', function () {
			var id = btn.getAttribute('data-del');
			if (!confirm('确认删除该账号？删除后需重新登录。')) return;
			TGD.api('/api/accounts/' + id, 'DELETE').then(function () {
				TGD.toast('已删除', 'tgd-ok');
				loadAccounts();
			}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); });
		});
	});
}

function loadLogs() {
	TGD.api('/api/logs?limit=200', 'GET').then(function (data) {
		allLogsRaw = data.logs || [];
		renderLogs();
	}).catch(function (e) { console.error(e); });
}

function renderLogs() {
	var box = document.getElementById('tgd-logs');
	if (!box) return;
	var logs = allLogsRaw;
	if (currentLogFilter !== 'all') {
		var acc = null;
		for (var i = 0; i < allAccounts.length; i++) {
			if (allAccounts[i].id === currentLogFilter) { acc = allAccounts[i]; break; }
		}
		if (acc) {
			var keywords = [acc.name, acc.id, acc.uid].filter(function (k) { return k; });
			logs = logs.filter(function (l) {
				var msg = l.message || '';
				return keywords.some(function (k) { return msg.indexOf(k) !== -1; });
			});
		}
	}
	if (!logs.length) {
		box.innerHTML = '<div class="tgd-log-empty">' + (currentLogFilter === 'all' ? '暂无日志' : '该账号暂无日志') + '</div>';
		return;
	}
	box.innerHTML = logs.map(function (l) {
		var lv = (l.level || 'info').toUpperCase().padEnd(5, ' ');
		return '<div class="tgd-log-line tgd-' + TGD.esc(l.level || 'info') + '"><span class="tgd-ts">' + TGD.esc(l.ts) + '</span><span class="tgd-lv">' + TGD.esc(lv) + '</span>' + TGD.esc(l.message) + '</div>';
	}).join('');
	box.scrollTop = box.scrollHeight;
}

function buildLogTabs(accounts) {
	var wrap = document.getElementById('tgd-log-tabs');
	if (!wrap) return;
	var html = '<button class="tgd-log-tab' + (currentLogFilter === 'all' ? ' tgd-active' : '') + '" data-filter="all">全部</button>';
	accounts.forEach(function (a) {
		html += '<button class="tgd-log-tab' + (currentLogFilter === a.id ? ' tgd-active' : '') + '" data-filter="' + TGD.esc(a.id) + '">' + TGD.esc(a.role_name || a.name) + '</button>';
	});
	wrap.innerHTML = html;
	wrap.querySelectorAll('.tgd-log-tab').forEach(function (tab) {
		tab.addEventListener('click', function () {
			wrap.querySelectorAll('.tgd-log-tab').forEach(function (t) { t.classList.remove('tgd-active'); });
			tab.classList.add('tgd-active');
			currentLogFilter = tab.getAttribute('data-filter');
			renderLogs();
		});
	});
}

function openSettings() {
	TGD.api('/api/config', 'GET').then(function (cfg) {
		document.getElementById('tgd-cfg-schedule').value = cfg.default_schedule;
		document.getElementById('tgd-cfg-coin').checked = !!cfg.coin_tasks;
		document.getElementById('tgd-cfg-cloud').checked = !!cfg.cloud_duration;
		document.getElementById('tgd-cfg-share').value = cfg.share_platform || 'qq';
		var policySel = document.getElementById('tgd-cfg-crypto-policy');
		if (policySel) policySel.value = cfg.crypto_policy || 'auto';
		var cidrs = document.getElementById('tgd-cfg-lan-cidrs');
		if (cidrs) cidrs.value = cfg.lan_cidrs || '';
		document.getElementById('tgd-settings-modal').classList.add('tgd-show');
	}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); });
}

function saveConfig() {
	TGD.api('/api/config', 'POST', {
		default_schedule: document.getElementById('tgd-cfg-schedule').value,
		coin_tasks: document.getElementById('tgd-cfg-coin').checked,
		cloud_duration: document.getElementById('tgd-cfg-cloud').checked,
		share_platform: document.getElementById('tgd-cfg-share').value.trim() || 'qq'
	}).then(function () {
		TGD.toast('设置已保存', 'tgd-ok');
		closeModal('tgd-settings-modal');
		loadAccounts();
	}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); });
}

function saveCrypto() {
	TGD.api('/api/config', 'POST', {
		crypto_policy: document.getElementById('tgd-cfg-crypto-policy').value,
		lan_cidrs: document.getElementById('tgd-cfg-lan-cidrs').value.trim()
	}).then(function () {
		TGD.toast('加密设置已保存', 'tgd-ok');
	}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); });
}

function startPoll() {
	if (pollTimer) { clearInterval(pollTimer); }
	pollTimer = setInterval(function () {
		loadLogs();
	}, 3000);
}

return view.extend({
	load: function () {
		return uci.load('taygedo');
	},
	render: function () {
		var root = E('div', { 'id': 'tgd-root', 'class': 'tgd-root' });

		root.innerHTML = '<style>' + TGD_CSS + '</style><div class="tgd-login-wrap"><div class="tgd-login-card"><div class="tgd-logo">' +
			ICONS.logo + '</div><h1>塔吉多自动签到</h1><div class="tgd-sub">正在连接签到服务…</div></div></div><div class="tgd-toast"></div>';

		// 探测后端可用性与鉴权状态。
		//
		// 走 TGD.api（内部 rpcd 通道）而不是裸 fetch：请求必须携带 LuCI 会话
		// 号，且后端只监听回环，浏览器无法直连。
		var probe = TGD.api('/api/config', 'GET');

		probe.then(function () {
			renderMain();
		}).catch(function (e) {
			console.error('taygedo probe:', e);
			var code = (e && e.code) || '';
			if (code === 'untrusted_source') {
				renderNoAuth('当前来源不在受信内网网段（lan_cidrs）内，服务端已拒绝该请求。');
				return;
			}
			if (code === 'luci_session_missing') {
				renderNoAuth('请求未携带 LuCI 会话号，请从 LuCI 菜单「服务 → 塔吉多签到」进入本页。');
				return;
			}
			if (code === 'luci_session_invalid') {
				renderNoAuth('LuCI 会话已失效或不被 OpenWrt 认可，请重新登录 LuCI 后重试。');
				return;
			}
			if (code === 'backend_unreachable' || code === 'rpc_error') { renderServiceDown(); return; }
			if (e instanceof TypeError) { renderServiceDown(); return; }
			renderNoAuth(e && e.message ? e.message : '');
		});

		return root;
	}
});
