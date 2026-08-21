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
end
