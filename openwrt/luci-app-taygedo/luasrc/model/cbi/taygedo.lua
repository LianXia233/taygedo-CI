m = Map("taygedo", translate("塔吉多签到"),
	translate("塔吉多（幻塔 / 异环）每日自动签到工具。支持多账号、短信/密码登录、每日定时签到，内置带鉴权的响应式 WebUI。"))

s = m:section(TypedSection, "taygedo", translate("基本设置"))
s.anonymous = true

s:option(Flag, "enabled", translate("启用服务"),
	translate("启用后自动启动签到服务。"))

o = s:option(Value, "port", translate("监听端口"),
	translate("WebUI 与 API 的监听端口。"))
o.datatype = "port"
o.default = 8787

o = s:option(Value, "data_dir", translate("数据目录"),
	translate("账号与配置数据的存储目录。"))
o.default = "/etc/taygedo"

o = s:option(Value, "web_password", translate("Web 登录密码"),
	translate("WebUI 登录密码。留空则首次启动自动生成随机密码（见系统日志）。"))
o.password = true

o = s:option(Value, "default_schedule", translate("默认签到时间"),
	translate("每天自动签到的时间（HH:MM，北京时间）。"))
o.default = "06:10"

return m
