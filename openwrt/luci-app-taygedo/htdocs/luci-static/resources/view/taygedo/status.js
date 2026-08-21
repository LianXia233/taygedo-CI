'use strict';
'require view';
'require uci';

/*
 * 塔吉多自动签到 - LuCI JS 前端
 * 功能与内置 WebUI 一致：账号管理 / 密码或验证码登录 / 每日签到时间 /
 * 立即签到 / 运行日志 / 全局设置 / 修改密码。
 *
 * 视觉上完全复用当前 LuCI 主题（aurora）的 CSS 变量，保证与路由后台
 * 观感一致，并随主题自动切换亮/暗色，不额外引入独立配色。
 *
 * LuCI 页面已由 OpenWrt root 鉴权保护，进入后自动静默登录后端
 * （UCI web_password，默认 admin），无需用户二次输入密码。
 */

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------
var TGD = (function () {
	var apiBase = '';
	// LuCI 页面已由 OpenWrt root 鉴权保护，token 仅存内存、每次进入页面静默登录，
	// 不持久化到 localStorage（避免旧 token 因后端重启失效而误判为未登录）。
	var token = '';
	var noAuth = false;
	var pollTimer = null;

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

	function api(path, method, body, _retry) {
		var opts = { method: method || 'GET', headers: {} };
		if (token && !noAuth) opts.headers['Authorization'] = 'Bearer ' + token;
		if (body !== undefined) {
			opts.headers['Content-Type'] = 'application/json';
			opts.body = JSON.stringify(body);
		}
		return fetch(apiBase + path, opts).then(function (r) {
			if (r.status === 401 && !noAuth && !_retry) {
				// token 失效：静默重新登录后端后重试一次
				token = '';
				return autoLogin().then(function () {
					return api(path, method, body, true);
				});
			}
			if (r.status === 401 && !noAuth) {
				token = '';
				showLogin();
				throw new Error('未登录或登录已过期');
			}
			return r.json().catch(function () { return {}; }).then(function (data) {
				if (!r.ok) throw new Error(data.error || ('HTTP ' + r.status));
				return data;
			});
		});
	}

	function initApiBase() {
		var port = '8787';
		try { port = uci.get('taygedo', 'main', 'port') || '8787'; } catch (e) {}
		apiBase = 'http://' + location.hostname + ':' + port;
	}

	return {
		esc: esc, toast: toast, api: api, initApiBase: initApiBase,
		getToken: function () { return token; },
		setToken: function (t) { token = t || ''; },
		getApiBase: function () { return apiBase; },
		getNoAuth: function () { return noAuth; },
		setNoAuth: function (v) { noAuth = !!v; }
	};
})();

// ---------------------------------------------------------------------------
// CSS —— 全部复用当前 LuCI 主题变量，保证与路由后台观感一致
// ---------------------------------------------------------------------------
var TGD_CSS = [
	'.tgd-root { font-family: var(--font-sans); color: var(--text); }',
	'.tgd-root * { box-sizing: border-box; margin: 0; padding: 0; }',
	'.tgd-root .tgd-head { display: flex; align-items: center; justify-content: space-between; gap: 12px;',
	'  padding: 16px 20px; background: var(--surface); border: 1px solid var(--hairline); border-radius: var(--radius-base);',
	'  box-shadow: var(--app-shadow-sm); margin-bottom: 16px; flex-wrap: wrap; }',
	'.tgd-root .tgd-brand { display: flex; align-items: center; gap: 12px; min-width: 0; }',
	'.tgd-root .tgd-logo { width: 40px; height: 40px; border-radius: calc(var(--radius-base) * 1.5); flex: none;',
	'  background: var(--brand); color: var(--on-brand, #fff); display: flex; align-items: center; justify-content: center; }',
	'.tgd-root .tgd-brand h1 { font-size: 16px; font-weight: 700; color: var(--text); }',
	'.tgd-root .tgd-brand p { font-size: 12px; color: var(--text-subtle); margin-top: 2px; }',
	'.tgd-root .tgd-hdr-actions { display: flex; align-items: center; gap: 8px; flex: none; }',
	'.tgd-root .tgd-btn { display: inline-flex; align-items: center; justify-content: center; gap: 6px;',
	'  padding: 8px 14px; border-radius: var(--radius-base); border: 1px solid var(--hairline);',
	'  background: var(--surface); color: var(--text); font-size: 13px; font-weight: 600; cursor: pointer;',
	'  transition: all .15s; white-space: nowrap; min-height: 36px; }',
	'.tgd-root .tgd-btn:hover { border-color: var(--brand); color: var(--brand); }',
	'.tgd-root .tgd-btn:disabled { opacity: .5; cursor: not-allowed; }',
	'.tgd-root .tgd-btn svg { width: 15px; height: 15px; flex: none; }',
	'.tgd-root .tgd-btn.tgd-primary { background: var(--brand); color: var(--on-brand, #fff); border: 1px solid var(--brand); }',
	'.tgd-root .tgd-btn.tgd-primary:hover { opacity: .88; color: var(--on-brand, #fff); }',
	'.tgd-root .tgd-btn.tgd-danger:hover { border-color: var(--danger); color: var(--danger); }',
	'.tgd-root .tgd-stats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 12px; margin-bottom: 16px; }',
	'.tgd-root .tgd-stat { background: var(--surface); border: 1px solid var(--hairline); border-radius: var(--radius-base);',
	'  padding: 14px 16px; box-shadow: var(--app-shadow-sm); }',
	'.tgd-root .tgd-stat .tgd-num { font-size: 22px; font-weight: 800; }',
	'.tgd-root .tgd-stat .tgd-label { font-size: 12px; color: var(--text-subtle); margin-top: 2px; }',
	'.tgd-root .tgd-stat.tgd-total .tgd-num { color: var(--brand); }',
	'.tgd-root .tgd-stat.tgd-done .tgd-num { color: var(--success); }',
	'.tgd-root .tgd-stat.tgd-pending .tgd-num { color: var(--warning); }',
	'.tgd-root .tgd-main { display: grid; grid-template-columns: 1fr; gap: 16px; }',
	'@media (min-width: 960px) { .tgd-root .tgd-main { grid-template-columns: 1.25fr 1fr; align-items: start; } }',
	'.tgd-root .tgd-accounts { display: grid; grid-template-columns: 1fr; gap: 12px; }',
	'@media (min-width: 640px) { .tgd-root .tgd-accounts { grid-template-columns: repeat(2, 1fr); } }',
	'.tgd-root .tgd-card { background: var(--surface); border: 1px solid var(--hairline); border-radius: var(--radius-base);',
	'  padding: 16px; box-shadow: var(--app-shadow-sm); display: flex; flex-direction: column; gap: 12px;',
	'  transition: box-shadow .15s, border-color .15s; }',
	'.tgd-root .tgd-card:hover { box-shadow: var(--app-shadow-md); border-color: var(--brand); }',
	'.tgd-root .tgd-card-top { display: flex; justify-content: space-between; align-items: flex-start; gap: 10px; }',
	'.tgd-root .tgd-avatar { width: 40px; height: 40px; border-radius: 50%; background: var(--brand-subtle); color: var(--brand);',
	'  display: flex; align-items: center; justify-content: center; font-weight: 800; font-size: 16px; flex: none; }',
	'.tgd-root .tgd-card-info { flex: 1; min-width: 0; }',
	'.tgd-root .tgd-card-info .tgd-name { font-weight: 700; font-size: 14px; display: flex; align-items: center; gap: 8px; flex-wrap: wrap; color: var(--text); }',
	'.tgd-root .tgd-card-info .tgd-sub { font-size: 12px; color: var(--text-subtle); margin-top: 3px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }',
	'.tgd-root .tgd-badge { font-size: 11px; font-weight: 700; padding: 3px 9px; border-radius: 20px; flex: none; }',
	'.tgd-root .tgd-badge.tgd-ok { background: var(--success-surface); color: var(--success); }',
	'.tgd-root .tgd-badge.tgd-wait { background: var(--warning-surface); color: var(--warning); }',
	'.tgd-root .tgd-row { display: flex; align-items: center; justify-content: space-between; gap: 10px; }',
	'.tgd-root .tgd-row .tgd-lab { font-size: 12.5px; color: var(--text-subtle); display: flex; align-items: center; gap: 6px; }',
	'.tgd-root .tgd-row .tgd-lab svg { width: 14px; height: 14px; }',
	'.tgd-root .tgd-time { border: 1px solid var(--hairline); border-radius: var(--radius-base); padding: 6px 8px;',
	'  background: var(--control-bg); color: var(--text); font-family: var(--font-mono); font-size: 13px; min-height: 34px; }',
	'.tgd-root .tgd-card-actions { display: flex; gap: 8px; }',
	'.tgd-root .tgd-card-actions .tgd-btn { flex: 1; padding: 8px 10px; font-size: 12.5px; }',
	'.tgd-root .tgd-empty { grid-column: 1 / -1; text-align: center; padding: 40px 20px; color: var(--text-subtle);',
	'  border: 1.5px dashed var(--hairline); border-radius: var(--radius-base); }',
	'.tgd-root .tgd-empty .tgd-big { font-size: 14px; font-weight: 600; color: var(--text); margin-bottom: 6px; }',
	'.tgd-root .tgd-panel { background: var(--surface); border: 1px solid var(--hairline); border-radius: var(--radius-base);',
	'  box-shadow: var(--app-shadow-sm); overflow: hidden; }',
	'.tgd-root .tgd-panel-head { display: flex; align-items: center; justify-content: space-between; padding: 13px 16px; border-bottom: 1px solid var(--hairline); }',
	'.tgd-root .tgd-panel-head h2 { font-size: 14px; font-weight: 700; display: flex; align-items: center; gap: 8px; color: var(--text); }',
	'.tgd-root .tgd-panel-head h2 svg { width: 16px; height: 16px; color: var(--brand); }',
	'.tgd-root .tgd-logs { height: 360px; overflow-y: auto; padding: 12px 14px; background: var(--surface-sunken);',
	'  font-family: var(--font-mono); font-size: 12px; line-height: 1.7; }',
	'.tgd-root .tgd-log-line { color: var(--text); word-break: break-all; }',
	'.tgd-root .tgd-log-line .tgd-ts { color: var(--text-muted); margin-right: 6px; }',
	'.tgd-root .tgd-log-line .tgd-lv { font-weight: 700; margin-right: 6px; }',
	'.tgd-root .tgd-log-line.tgd-info .tgd-lv { color: var(--info); }',
	'.tgd-root .tgd-log-line.tgd-error .tgd-lv { color: var(--danger); }',
	'.tgd-root .tgd-log-line.tgd-warn .tgd-lv { color: var(--warning); }',
	'.tgd-root .tgd-log-empty { color: var(--text-muted); }',
	'.tgd-root .tgd-login-wrap { display: flex; align-items: center; justify-content: center; padding: 30px 16px; }',
	'.tgd-root .tgd-login-card { width: 100%; max-width: 360px; background: var(--surface); border: 1px solid var(--hairline);',
	'  border-radius: var(--radius-base); box-shadow: var(--app-shadow-lg); padding: 32px 28px; text-align: center; }',
	'.tgd-root .tgd-login-card .tgd-logo { width: 52px; height: 52px; border-radius: 16px; margin: 0 auto 14px;',
	'  background: var(--brand); color: var(--on-brand, #fff); display: flex; align-items: center; justify-content: center; }',
	'.tgd-root .tgd-login-card h1 { font-size: 19px; font-weight: 800; color: var(--text); }',
	'.tgd-root .tgd-login-card .tgd-sub { font-size: 13px; color: var(--text-subtle); margin: 6px 0 20px; }',
	'.tgd-root .tgd-input { border: 1px solid var(--hairline); border-radius: var(--radius-base); padding: 10px 12px; font-size: 14px;',
	'  background: var(--control-bg); color: var(--text); outline: none; transition: border-color .15s; width: 100%; min-height: 42px; }',
	'.tgd-root .tgd-input:focus { border-color: var(--brand); }',
	'.tgd-root .tgd-field { display: flex; flex-direction: column; gap: 6px; margin-bottom: 12px; text-align: left; }',
	'.tgd-root .tgd-field label { font-size: 12.5px; font-weight: 600; color: var(--text-subtle); }',
	'.tgd-root .tgd-modal-mask { position: fixed; inset: 0; background: var(--scrim, rgba(0,0,0,.5)); z-index: 1000;',
	'  display: flex; align-items: center; justify-content: center; padding: 16px; opacity: 0; pointer-events: none; transition: opacity .2s; }',
	'.tgd-root .tgd-modal-mask.tgd-show { opacity: 1; pointer-events: auto; }',
	'.tgd-root .tgd-modal { background: var(--surface); border: 1px solid var(--hairline); border-radius: var(--radius-base);',
	'  width: 100%; max-width: 440px; max-height: 90vh; overflow-y: auto; box-shadow: var(--app-shadow-lg); }',
	'.tgd-root .tgd-modal-head { padding: 15px 20px; border-bottom: 1px solid var(--hairline); display: flex; justify-content: space-between; align-items: center; }',
	'.tgd-root .tgd-modal-head h3 { font-size: 16px; font-weight: 700; color: var(--text); }',
	'.tgd-root .tgd-modal-close { background: none; border: none; color: var(--text-subtle); cursor: pointer; font-size: 22px; line-height: 1; padding: 4px; }',
	'.tgd-root .tgd-modal-close:hover { color: var(--text); }',
	'.tgd-root .tgd-modal-body { padding: 18px 20px; }',
	'.tgd-root .tgd-tabs { display: flex; gap: 4px; background: var(--surface-sunken); border-radius: var(--radius-base); padding: 4px; margin-bottom: 14px; }',
	'.tgd-root .tgd-tab { flex: 1; text-align: center; padding: 9px; border-radius: var(--radius-base); font-size: 13px; font-weight: 600;',
	'  cursor: pointer; color: var(--text-subtle); border: none; background: none; }',
	'.tgd-root .tgd-tab.tgd-active { background: var(--surface); color: var(--brand); box-shadow: var(--app-shadow-sm); }',
	'.tgd-root .tgd-captcha-row { display: flex; gap: 8px; }',
	'.tgd-root .tgd-captcha-row .tgd-input { flex: 1; }',
	'.tgd-root .tgd-captcha-row .tgd-btn { flex: none; }',
	'.tgd-root .tgd-switch-row { display: flex; align-items: center; justify-content: space-between; margin-bottom: 12px; }',
	'.tgd-root .tgd-switch-row .tgd-txt { font-size: 14px; color: var(--text); }',
	'.tgd-root .tgd-switch { position: relative; width: 44px; height: 24px; }',
	'.tgd-root .tgd-switch input { opacity: 0; width: 0; height: 0; }',
	'.tgd-root .tgd-switch .tgd-slider { position: absolute; inset: 0; background: var(--hairline); border-radius: 24px; cursor: pointer; transition: .2s; }',
	'.tgd-root .tgd-switch .tgd-slider:before { content: ""; position: absolute; width: 18px; height: 18px; left: 3px; top: 3px; background: #fff; border-radius: 50%; transition: .2s; }',
	'.tgd-root .tgd-switch input:checked + .tgd-slider { background: var(--brand); }',
	'.tgd-root .tgd-switch input:checked + .tgd-slider:before { transform: translateX(20px); }',
	'.tgd-root .tgd-divider { border: none; border-top: 1px solid var(--hairline); margin: 6px 0 14px; }',
	'.tgd-root .tgd-section-title { font-size: 13px; font-weight: 700; color: var(--text-subtle); margin-bottom: 10px; }',
	'.tgd-toast { position: fixed; bottom: 24px; left: 50%; transform: translateX(-50%) translateY(20px);',
	'  background: var(--text); color: var(--surface); padding: 11px 20px; border-radius: var(--radius-base); font-size: 13.5px; font-weight: 600;',
	'  z-index: 2000; opacity: 0; transition: all .25s; box-shadow: var(--app-shadow-lg); max-width: 90vw; }',
	'.tgd-toast.tgd-show { opacity: 1; transform: translateX(-50%) translateY(0); }',
	'.tgd-toast.tgd-err { background: var(--danger); color: #fff; }',
	'.tgd-toast.tgd-ok { background: var(--success); color: #fff; }',
	'.tgd-root .tgd-spin { animation: tgd-spin 1s linear infinite; }',
	'@keyframes tgd-spin { to { transform: rotate(360deg); } }'
].join('\n');

// ---------------------------------------------------------------------------
// SVG 图标
// ---------------------------------------------------------------------------
var ICONS = {
	logo: '<svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 11h4M8 9v4"/><path d="M15 11h.01M18 13h.01"/><path d="M17.32 5H6.68a4 4 0 0 0-3.978 3.59c-.006.052-.01.101-.017.152C2.604 9.416 2 14.456 2 16a3 3 0 0 0 3 3c1 0 1.5-.5 2-1l1.414-1.414A2 2 0 0 1 9.828 16h4.344a2 2 0 0 1 1.414.586L17 18c.5.5 1 1 2 1a3 3 0 0 0 3-3c0-1.545-.604-6.584-.685-7.258-.007-.05-.011-.1-.017-.151A4 4 0 0 0 17.32 5z"/></svg>',
	clock: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>',
	signin: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><path d="M22 4 12 14.01l-3-3"/></svg>',
	del: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2m3 0v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/></svg>',
	refresh: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a9 9 0 1 1-2.64-6.36"/><path d="M21 3v6h-6"/></svg>',
	gear: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>'
};

// ---------------------------------------------------------------------------
// 模板
// ---------------------------------------------------------------------------
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
		'  <div class="tgd-stat tgd-total"><div class="tgd-num" id="tgd-stat-total">0</div><div class="tgd-label">总账号</div></div>',
		'  <div class="tgd-stat tgd-done"><div class="tgd-num" id="tgd-stat-done">0</div><div class="tgd-label">今日已签到</div></div>',
		'  <div class="tgd-stat tgd-pending"><div class="tgd-num" id="tgd-stat-pending">0</div><div class="tgd-label">待签到</div></div>',
		'</div>',
		'<div class="tgd-main">',
		'  <div class="tgd-accounts" id="tgd-accounts"></div>',
		'  <div class="tgd-panel">',
		'    <div class="tgd-panel-head"><h2>' + ICONS.clock + ' 运行日志</h2>',
		'      <button class="tgd-btn" id="tgd-refresh-logs" title="刷新">' + ICONS.refresh + '</button></div>',
		'    <div class="tgd-logs" id="tgd-logs"><div class="tgd-log-empty">暂无日志</div></div>',
		'  </div>',
		'</div>'
	].join('');
}

function loginHtml() {
	return [
		'<div class="tgd-login-wrap">',
		'  <div class="tgd-login-card">',
		'    <div class="tgd-logo">' + ICONS.logo + '</div>',
		'    <h1>塔吉多自动签到</h1>',
		'    <div class="tgd-sub">请输入签到服务的账号密码</div>',
		'    <div class="tgd-field"><label>账号</label><input class="tgd-input" id="tgd-login-user" value="admin" autocomplete="username"></div>',
		'    <div class="tgd-field"><label>密码</label><input class="tgd-input" id="tgd-login-pwd" type="password" autocomplete="current-password"></div>',
		'    <button class="tgd-btn tgd-primary" id="tgd-login-btn" style="width:100%;min-height:44px;font-size:15px">登 录</button>',
		'  </div>',
		'</div>'
	].join('');
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
		'      <button class="tgd-btn tgd-primary" id="tgd-login-submit" style="width:100%">登录</button>',
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
		'      <button class="tgd-btn tgd-primary" id="tgd-cfg-save" style="width:100%">保存</button>',
		'      <hr class="tgd-divider">',
		'      <div id="tgd-pwd-section">',
		'      <div class="tgd-section-title">修改登录账号密码</div>',
		'      <div class="tgd-field"><label>账号</label><input class="tgd-input" id="tgd-old-user" value="admin" autocomplete="username"></div>',
		'      <div class="tgd-field"><label>原密码</label><input class="tgd-input" id="tgd-old-pwd" type="password" autocomplete="current-password"></div>',
		'      <div class="tgd-field"><label>新密码（至少 6 位）</label><input class="tgd-input" id="tgd-new-pwd" type="password" autocomplete="new-password"></div>',
		'      <button class="tgd-btn" id="tgd-pwd-save" style="width:100%">修改账号密码</button>',
		'      </div>',
		'    </div>',
		'  </div>',
		'</div>'
	].join('');
}

// ---------------------------------------------------------------------------
// 视图逻辑
// ---------------------------------------------------------------------------
function showLogin() {
	var root = document.getElementById('tgd-root');
	if (!root) return;
	root.innerHTML = '<style>' + TGD_CSS + '</style>' + loginHtml() + '<div class="tgd-toast"></div>';
	var btn = document.getElementById('tgd-login-btn');
	btn.addEventListener('click', function () { doLogin(); });
	var pwd = document.getElementById('tgd-login-pwd');
	pwd.addEventListener('keydown', function (e) { if (e.key === 'Enter') doLogin(); });
}

// LuCI 页面已由 OpenWrt root 鉴权保护，无需用户再次输入签到服务密码。
// 每次进入页面都强制用 UCI web_password（默认 admin）静默登录后端拿新 token，
// 仅当后端密码与 UCI 不同步时才兜底显示登录框。
function autoLogin() {
	var pwd = 'admin';
	try {
		var wp = uci.get('taygedo', 'main', 'web_password');
		if (wp) pwd = wp;
	} catch (e) {}
	return fetch(TGD.getApiBase() + '/api/login', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ username: 'admin', password: pwd })
	}).then(function (r) { return r.json().catch(function () { return {}; }); }).then(function (j) {
		if (!j.ok) throw new Error(j.error || '登录失败');
		TGD.setToken(j.token);
	});
}

function doLogin() {
	var user = document.getElementById('tgd-login-user').value.trim() || 'admin';
	var pwd = document.getElementById('tgd-login-pwd').value;
	if (!pwd) { TGD.toast('请输入密码', 'tgd-err'); return; }
	var btn = document.getElementById('tgd-login-btn');
	btn.disabled = true; btn.textContent = '登录中...';
	fetch(TGD.getApiBase() + '/api/login', {
		method: 'POST',
		headers: { 'Content-Type': 'application/json' },
		body: JSON.stringify({ username: user, password: pwd })
	}).then(function (r) { return r.json().catch(function () { return {}; }); }).then(function (j) {
		if (!j.ok) throw new Error(j.error || '登录失败');
		TGD.setToken(j.token);
		renderMain();
		TGD.toast('登录成功', 'tgd-ok');
	}).catch(function (e) {
		TGD.toast(e.message, 'tgd-err');
		btn.disabled = false; btn.textContent = '登 录';
	});
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
	document.getElementById('tgd-pwd-save').addEventListener('click', changePassword);
}

function openAddModal(mode) {
	document.querySelectorAll('.tgd-tab').forEach(function (x) { x.classList.toggle('tgd-active', x.getAttribute('data-mode') === mode); });
	document.getElementById('tgd-password-field').style.display = mode === 'password' ? '' : 'none';
	document.getElementById('tgd-captcha-field').style.display = mode === 'captcha' ? '' : 'none';
	document.getElementById('tgd-add-modal').classList.add('tgd-show');
}

function closeModal(id) { document.getElementById(id).classList.remove('tgd-show'); }

var sendCodeTimer = null;
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
		document.getElementById('tgd-stat-total').textContent = list.length;
		document.getElementById('tgd-stat-done').textContent = list.filter(function (a) { return a.signed_today; }).length;
		document.getElementById('tgd-stat-pending').textContent = list.filter(function (a) { return !a.signed_today; }).length;
		renderAccounts(list);
	}).catch(function (e) { console.error(e); });
}

function renderAccounts(list) {
	var wrap = document.getElementById('tgd-accounts');
	if (!list.length) {
		wrap.innerHTML = '<div class="tgd-empty"><div class="tgd-big">还没有账号</div>点击右上角「添加账号」开始使用</div>';
		return;
	}
	wrap.innerHTML = list.map(function (a) {
		var badge = a.signed_today ? '<span class="tgd-badge tgd-ok">今日已签</span>' : '<span class="tgd-badge tgd-wait">待签到</span>';
		return '<div class="tgd-card" data-id="' + TGD.esc(a.id) + '">' +
			'<div class="tgd-card-top">' +
			'<div class="tgd-avatar">' + TGD.esc(a.name).charAt(0).toUpperCase() + '</div>' +
			'<div class="tgd-card-info"><div class="tgd-name">' + TGD.esc(a.name) + ' ' + badge + '</div>' +
			'<div class="tgd-sub">' + (a.phone || '未绑定手机') + ' · UID ' + TGD.esc(a.uid || '-') + (a.role_name ? ' · ' + TGD.esc(a.role_name) : '') + '</div></div>' +
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
		var logs = data.logs || [];
		var box = document.getElementById('tgd-logs');
		if (!box) return;
		if (!logs.length) { box.innerHTML = '<div class="tgd-log-empty">暂无日志</div>'; return; }
		box.innerHTML = logs.map(function (l) {
			var lv = (l.level || 'info').toUpperCase().padEnd(5, ' ');
			return '<div class="tgd-log-line tgd-' + TGD.esc(l.level || 'info') + '"><span class="tgd-ts">' + TGD.esc(l.ts) + '</span><span class="tgd-lv">' + TGD.esc(lv) + '</span>' + TGD.esc(l.message) + '</div>';
		}).join('');
		box.scrollTop = box.scrollHeight;
	}).catch(function (e) { console.error(e); });
}

function openSettings() {
	TGD.api('/api/config', 'GET').then(function (cfg) {
		document.getElementById('tgd-cfg-schedule').value = cfg.default_schedule;
		document.getElementById('tgd-cfg-coin').checked = !!cfg.coin_tasks;
		document.getElementById('tgd-cfg-cloud').checked = !!cfg.cloud_duration;
		document.getElementById('tgd-cfg-share').value = cfg.share_platform || 'qq';
		document.getElementById('tgd-old-user').value = 'admin';
		document.getElementById('tgd-old-pwd').value = '';
		document.getElementById('tgd-new-pwd').value = '';
		// 免鉴权模式（OpenWrt）：隐藏修改密码区块
		var ps = document.getElementById('tgd-pwd-section');
		if (ps) ps.style.display = TGD.getNoAuth() ? 'none' : '';
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

function changePassword() {
	var oldUser = document.getElementById('tgd-old-user').value.trim() || 'admin';
	var oldPwd = document.getElementById('tgd-old-pwd').value;
	var newPwd = document.getElementById('tgd-new-pwd').value;
	if (!oldPwd || !newPwd) { TGD.toast('请输入原密码和新密码', 'tgd-err'); return; }
	TGD.api('/api/password', 'POST', { username: oldUser, old_password: oldPwd, new_password: newPwd }).then(function () {
		TGD.toast('账号密码已修改', 'tgd-ok');
		document.getElementById('tgd-old-pwd').value = '';
		document.getElementById('tgd-new-pwd').value = '';
	}).catch(function (e) { TGD.toast(e.message, 'tgd-err'); });
}

function startPoll() {
	if (pollTimer) { clearInterval(pollTimer); }
	pollTimer = setInterval(function () {
		if (TGD.getToken()) { loadLogs(); }
	}, 3000);
}

// ---------------------------------------------------------------------------
// LuCI 视图入口
// ---------------------------------------------------------------------------
return view.extend({
	load: function () {
		TGD.initApiBase();
		return uci.load('taygedo');
	},
	render: function () {
		TGD.initApiBase();
		var root = E('div', { 'id': 'tgd-root', 'class': 'tgd-root' });

		root.innerHTML = '<style>' + TGD_CSS + '</style><div class="tgd-login-wrap"><div class="tgd-login-card"><div class="tgd-logo">' +
			ICONS.logo + '</div><h1>塔吉多自动签到</h1><div class="tgd-sub">正在连接签到服务…</div></div></div><div class="tgd-toast"></div>';

		// 先探测后端是否需要登录鉴权（OpenWrt 免鉴权模式下 no_auth=true）。
		fetch(TGD.getApiBase() + '/api/auth').then(function (r) {
			return r.json().catch(function () { return {}; });
		}).then(function (j) {
			if (j.no_auth) {
				// 免鉴权：直接进入主界面，不弹登录框、不读/写 token。
				TGD.setNoAuth(true);
				TGD.setToken('');
				renderMain();
				return;
			}
			// 需要鉴权：默认静默自动登录后端，直接进入主界面；
			// 仅当后端密码与 UCI 不同步时才兜底显示登录框。
			autoLogin().then(function () {
				renderMain();
			}).catch(function () {
				showLogin();
			});
		}).catch(function () {
			// 探测失败按需要鉴权处理
			autoLogin().then(function () {
				renderMain();
			}).catch(function () {
				showLogin();
			});
		});

		return root;
	}
});
