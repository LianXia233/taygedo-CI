'use strict';
'require view';
'require uci';

/*
 * 塔吉多自动签到 - LuCI 独立版
 *
 * 本页面为 LuCI 端单独重构版本：不再维护登录态 / token，直接以免鉴权
 * 模式调用后端 REST API（需后端 TAYGEDO_NO_AUTH=1 / UCI no_auth=1，页面
 * 进入时通过 /api/meta 探测确认）。若后端未开启免鉴权，页面给出提示并
 * 提供「打开外部 WebUI」入口，跳转到独立 WebUI 使用。
 *
 * 功能与内置 WebUI 一致：账号管理 / 密码或验证码登录 / 每日签到时间 /
 * 立即签到 / 运行日志 / 全局设置 / 修改密码。
 * 额外提供「打开外部 WebUI」按钮，一键在新窗口打开 :port 独立管理界面。
 *
 * 视觉上完全复用当前 LuCI 主题（aurora）的 CSS 变量，保证与路由后台
 * 观感一致，并随主题自动切换亮/暗色，不额外引入独立配色。
 */

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------
var TGD = (function () {
	var apiBase = '';
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

	// 免鉴权模式：直接裸调后端 API，不携带任何 token
	function api(path, method, body) {
		var opts = { method: method || 'GET', headers: {} };
		if (body !== undefined) {
			opts.headers['Content-Type'] = 'application/json';
			opts.body = JSON.stringify(body);
		}
		return fetch(apiBase + path, opts).then(function (r) {
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

	function openWebUI() {
		window.open(apiBase + '/', '_blank');
	}

	return {
		esc: esc, toast: toast, api: api, initApiBase: initApiBase, openWebUI: openWebUI,
		getApiBase: function () { return apiBase; }
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
	'.tgd-root .tgd-card { position: relative; overflow: hidden; background: var(--surface); border: 1px solid var(--hairline); border-radius: var(--radius-base);',
	'  padding: 16px; box-shadow: var(--app-shadow-sm); display: flex; flex-direction: column; gap: 12px;',
	'  transition: box-shadow .15s, border-color .15s; }',
	'.tgd-root .tgd-card:hover { box-shadow: var(--app-shadow-md); border-color: var(--brand); }',
	'.tgd-root .tgd-card::before { content: ""; position: absolute; top: 0; left: 0; right: 0; height: 3px;',
	'  background: linear-gradient(90deg, var(--brand), var(--brand-hover, var(--brand))); opacity: 0; transition: opacity .2s; }',
	'.tgd-root .tgd-card:hover::before { opacity: 1; }',
	'.tgd-root .tgd-card-top { display: flex; justify-content: space-between; align-items: flex-start; gap: 10px; }',
	'.tgd-root .tgd-avatar { width: 46px; height: 46px; border-radius: 14px; flex: none;',
	'  overflow: hidden; background: linear-gradient(135deg, var(--brand), var(--brand-hover, var(--brand)));',
	'  color: var(--on-brand, #fff); display: flex; align-items: center; justify-content: center;',
	'  font-weight: 800; font-size: 18px; box-shadow: 0 4px 12px rgba(0,0,0,.2); }',
	'.tgd-root .tgd-avatar img { width: 100%; height: 100%; object-fit: cover; display: block; }',
'.tgd-root .tgd-card-info { flex: 1; min-width: 0; }',
	'.tgd-root .tgd-card-info .tgd-name-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }',
	'.tgd-root .tgd-card-info .tgd-name { font-weight: 700; font-size: 15.5px; letter-spacing: .3px; color: var(--text); }',
	'.tgd-root .tgd-card-info .tgd-meta-tags { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 7px; }',
	'.tgd-root .tgd-meta-tag { font-size: 11.5px; padding: 2px 8px; border-radius: 6px; background: var(--surface-sunken);',
	'  color: var(--text-subtle); display: inline-flex; align-items: center; gap: 4px; white-space: nowrap; max-width: 160px;',
	'  overflow: hidden; text-overflow: ellipsis; }',
	'.tgd-root .tgd-meta-tag svg { width: 12px; height: 12px; flex: none; opacity: .65; }',
	'.tgd-root .tgd-meta-tag.tgd-uid { font-family: var(--font-mono); font-size: 11px; }',
	'.tgd-root .tgd-meta-tag.tgd-role { background: var(--brand-subtle); color: var(--brand); }',
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
	'.tgd-root .tgd-log-tabs { display: flex; gap: 4px; padding: 10px 14px 0; background: var(--surface-sunken);',
	'  border-bottom: 1px solid var(--hairline); overflow-x: auto; scrollbar-width: none; }',
	'.tgd-root .tgd-log-tabs::-webkit-scrollbar { display: none; }',
	'.tgd-root .tgd-log-tab { padding: 5px 12px; border-radius: 8px; font-size: 12px; font-weight: 600; cursor: pointer;',
	'  white-space: nowrap; border: 1px solid transparent; background: none; color: var(--text-subtle); transition: all .15s; }',
	'.tgd-root .tgd-log-tab:hover { background: var(--surface); color: var(--text); }',
	'.tgd-root .tgd-log-tab.tgd-active { background: var(--brand-subtle); color: var(--brand); border-color: var(--brand); }',
	'.tgd-root .tgd-login-wrap { display: flex; align-items: center; justify-content: center; padding: 30px 16px; }',
	'.tgd-root .tgd-login-card { width: 100%; max-width: 400px; background: var(--surface); border: 1px solid var(--hairline);',
	'  border-radius: var(--radius-base); box-shadow: var(--app-shadow-lg); padding: 32px 28px; text-align: center; }',
	'.tgd-root .tgd-login-card .tgd-logo { width: 52px; height: 52px; border-radius: 16px; margin: 0 auto 14px;',
	'  background: var(--brand); color: var(--on-brand, #fff); display: flex; align-items: center; justify-content: center; }',
	'.tgd-root .tgd-login-card h1 { font-size: 19px; font-weight: 800; color: var(--text); }',
	'.tgd-root .tgd-login-card .tgd-sub { font-size: 13px; color: var(--text-subtle); margin: 6px 0 20px; line-height: 1.7; }',
	'.tgd-root .tgd-login-card .tgd-hint { font-size: 12px; color: var(--text-muted); margin-top: 14px; line-height: 1.7; text-align: left;',
	'  background: var(--surface-sunken); border: 1px solid var(--hairline); border-radius: var(--radius-base); padding: 10px 12px; }',
	'.tgd-root .tgd-login-card .tgd-hint code { font-family: var(--font-mono); font-size: 11.5px; color: var(--brand); }',
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
	gear: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/></svg>',
	external: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/><path d="M15 3h6v6"/><path d="M10 14 21 3"/></svg>',
	phone: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6A19.79 19.79 0 0 1 2.18 4.18 2 2 0 0 1 4 2h3a2 2 0 0 1 2 1.72c.13.96.36 1.9.7 2.81a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45c.9.34 1.85.57 2.81.7A2 2 0 0 1 22 16.92z"/></svg>',
	user: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"/><circle cx="12" cy="7" r="4"/></svg>',
	dot: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><path d="M12 12h.01"/></svg>'
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
		'    <button class="tgd-btn" id="tgd-webui" title="在新窗口打开独立 WebUI">' + ICONS.external + ' 外部 WebUI</button>',
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
		'    <div class="tgd-log-tabs" id="tgd-log-tabs"><button class="tgd-log-tab tgd-active" data-filter="all">全部</button></div>',
		'    <div class="tgd-logs" id="tgd-logs"><div class="tgd-log-empty">暂无日志</div></div>',
		'  </div>',
		'</div>'
	].join('');
}

// 后端未开启免鉴权时的提示页：LuCI 版不承载登录态，引导跳转外部 WebUI
function noAuthHtml() {
	return [
		'<div class="tgd-login-wrap">',
		'  <div class="tgd-login-card">',
		'    <div class="tgd-logo">' + ICONS.logo + '</div>',
		'    <h1>塔吉多自动签到</h1>',
		'    <div class="tgd-sub">LuCI 独立版需要后端开启免鉴权才能直接管理账号</div>',
		'    <button class="tgd-btn tgd-primary" id="tgd-noauth-webui" style="width:100%;min-height:44px;font-size:15px">' + ICONS.external + ' 打开外部 WebUI</button>',
		'    <div class="tgd-hint">如需在 LuCI 内直接使用，请在路由器执行：<br><code>uci set taygedo.main.no_auth=1</code><br><code>uci commit taygedo && /etc/init.d/taygedo restart</code></div>',
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
		'      <div class="tgd-section-title">修改登录账号密码</div>',
		'      <div class="tgd-field"><label>账号</label><input class="tgd-input" id="tgd-old-user" value="admin" autocomplete="username"></div>',
		'      <div class="tgd-field"><label>原密码</label><input class="tgd-input" id="tgd-old-pwd" type="password" autocomplete="current-password"></div>',
		'      <div class="tgd-field"><label>新密码（至少 6 位）</label><input class="tgd-input" id="tgd-new-pwd" type="password" autocomplete="new-password"></div>',
		'      <button class="tgd-btn" id="tgd-pwd-save" style="width:100%">修改账号密码</button>',
		'    </div>',
		'  </div>',
		'</div>'
	].join('');
}

// ---------------------------------------------------------------------------
// 视图逻辑
// ---------------------------------------------------------------------------
function renderMain() {
	var root = document.getElementById('tgd-root');
	root.innerHTML = '<style>' + TGD_CSS + '</style>' + mainHtml() +
		addModalHtml() + settingsModalHtml() + '<div class="tgd-toast"></div>';
	bindMainEvents();
	loadAccounts();
	loadLogs();
	startPoll();
}

function renderNoAuth() {
	var root = document.getElementById('tgd-root');
	root.innerHTML = '<style>' + TGD_CSS + '</style>' + noAuthHtml() + '<div class="tgd-toast"></div>';
	document.getElementById('tgd-noauth-webui').addEventListener('click', TGD.openWebUI);
}

function bindMainEvents() {
	document.getElementById('tgd-webui').addEventListener('click', TGD.openWebUI);
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
var allAccounts = [];       // 全局账号列表，用于日志按账号筛选
var allLogsRaw = [];        // 原始日志（未筛选）
var currentLogFilter = 'all'; // 当前日志筛选：'all' 或 account.id
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
		var badge = a.signed_today ? '<span class="tgd-badge tgd-ok">今日已签</span>' : '<span class="tgd-badge tgd-wait">待签到</span>';
		return '<div class="tgd-card" data-id="' + TGD.esc(a.id) + '">' +
			'<div class="tgd-card-top">' +
			'<div class="tgd-avatar"><img src="data:image/png;base64,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" alt="avatar"></div>' +
			'<div class="tgd-card-info">' +
			'<div class="tgd-name-row"><div class="tgd-name">' + TGD.esc(a.name) + '</div>' + badge + '</div>' +
			'<div class="tgd-meta-tags">' +
			(a.phone ? '<span class="tgd-meta-tag">' + ICONS.phone + TGD.esc(a.phone) + '</span>' : '') +
			'<span class="tgd-meta-tag tgd-uid">' + ICONS.dot + 'UID ' + TGD.esc(a.uid || '-') + '</span>' +
			(a.role_name ? '<span class="tgd-meta-tag tgd-role">' + ICONS.user + TGD.esc(a.role_name) + '</span>' : '') +
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

// 按当前 currentLogFilter 筛选并渲染日志（各账号独立显示）
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

// 构建日志筛选标签栏：全部 + 每个账号
function buildLogTabs(accounts) {
	var wrap = document.getElementById('tgd-log-tabs');
	if (!wrap) return;
	var html = '<button class="tgd-log-tab' + (currentLogFilter === 'all' ? ' tgd-active' : '') + '" data-filter="all">全部</button>';
	accounts.forEach(function (a) {
		html += '<button class="tgd-log-tab' + (currentLogFilter === a.id ? ' tgd-active' : '') + '" data-filter="' + TGD.esc(a.id) + '">' + TGD.esc(a.name) + '</button>';
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
		document.getElementById('tgd-old-user').value = 'admin';
		document.getElementById('tgd-old-pwd').value = '';
		document.getElementById('tgd-new-pwd').value = '';
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
		loadLogs();
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

		// 先探测后端免鉴权状态：no_auth=true 直接渲染主界面；
		// 否则渲染提示页（提供打开外部 WebUI 入口，不承载登录态）。
		root.innerHTML = '<style>' + TGD_CSS + '</style><div class="tgd-login-wrap"><div class="tgd-login-card"><div class="tgd-logo">' +
			ICONS.logo + '</div><h1>塔吉多自动签到</h1><div class="tgd-sub">正在连接签到服务…</div></div></div><div class="tgd-toast"></div>';

		fetch(TGD.getApiBase() + '/api/meta').then(function (r) {
			return r.json().catch(function () { return {}; });
		}).then(function (meta) {
			if (meta.no_auth) {
				renderMain();
				return;
			}
			renderNoAuth();
		}).catch(function () {
			renderNoAuth();
		});

		return root;
	}
});
