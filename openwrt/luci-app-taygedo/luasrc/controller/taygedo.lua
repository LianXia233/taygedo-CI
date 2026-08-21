module("luci.controller.taygedo", package.seeall)

function index()
	if not nixio.fs.access("/etc/config/taygedo") then
		return
	end

	entry({"admin", "services", "taygedo"},
		alias("admin", "services", "taygedo", "status"),
		_("塔吉多签到"), 40).dependent = true

	entry({"admin", "services", "taygedo", "status"},
		template("taygedo/status"), _("状态"), 10).leaf = true

	entry({"admin", "services", "taygedo", "config"},
		cbi("taygedo"), _("配置"), 20).leaf = true

	entry({"admin", "services", "taygedo", "webui"},
		call("action_webui"), _("打开 WebUI"), 30).leaf = true
end

function action_webui()
	local uci = require "luci.model.uci".cursor()
	local port = uci:get("taygedo", "main", "port") or "8787"
	local host = luci.http.getenv("HTTP_HOST") or ""
	local ip = host:match("([^%]]*)$") or host:match("([^:]+)") or "127.0.0.1"
	if ip:sub(1, 1) == "[" then
		ip = ip:sub(2, -2)
	end
	luci.http.redirect("http://" .. ip .. ":" .. port .. "/")
end
