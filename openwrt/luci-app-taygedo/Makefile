# OpenWrt LuCI app: 塔吉多自动签到 (Taygedo auto attendance)
#
# 现代 LuCI JS 前端：htdocs/luci-static/resources/view/taygedo/status.js
# 这是 OpenWrt 上的唯一入口，只做展示与交互：
#   - 二进制以 `openwrt` feature 单独编译：**不含** WebUI、登录/改密接口、
#     静态资源与浏览器侧加密前端（不是"注册了路由再 404"，是编译期不存在）；
#   - 数据经 rpcd 内部通道取得：页面调用 ubus 对象 luci.taygedo，由
#     /usr/share/rpcd/ucode/luci.taygedo 在服务端以 curl 调本机回环
#     http://127.0.0.1:<port>；后端只监听 127.0.0.1，不对 LAN 暴露；
#   - 鉴权由 OpenWrt 负责（LuCI/ubus 会话，鉴权边界是 rpcd ACL；后端在回环
#     路径只校验会话号形态，不再嵌套 ubus session get）。
#
# 两种构建方式（自动选择，无需改本文件）：
#   A. 源码交叉编译（优先）：本目录下存在 `local/taygedo-rs` 时直接使用。
#      该二进制由 `Scripts/Build-Taygedo.sh`（OpenWRT-CI 仓库）用宿主机
#      cargo + 目标 musl 工具链编译产出，与固件同源、同 triple、同 feature。
#   B. 预编译二进制下载（回退）：`local/taygedo-rs` 缺失时，从 GitHub Release
#      下载 `taygedo-rs-<triple>-openwrt.tar.gz`（由 taygedo-CI 的 build.yml 产出）。
#
# 安装位置：/usr/bin/taygedo-rs + LuCI JS 界面（服务 -> 塔吉多签到）

include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-taygedo
PKG_VERSION:=0.7.2
PKG_RELEASE:=1
PKG_LICENSE:=GPL-3.0
PKG_LICENSE_FILES:=LICENSE
PKG_MAINTAINER:=LianXia233

# 预编译二进制发布仓库与版本（需与 .github/workflows/build.yml 的 tag 一致）
TAYGEDO_REPO:=https://github.com/LianXia233/taygedo-CI
TAYGEDO_VER:=v$(PKG_VERSION)

include $(INCLUDE_DIR)/package.mk

# ---------------------------------------------------------------------------
# OpenWrt ARCH -> Rust musl target 映射（预编译下载路径）
# ---------------------------------------------------------------------------
# 注意：CI 仅发布 x86_64 与 aarch64 两个 musl 架构的预编译二进制
# （见 .github/workflows/build.yml），其余架构没有对应 Release 产物。
# 若继续映射到不存在产物，下载阶段会 404 且报错信息晦涩（曾在
# arm_cortex-a7 等常见架构上导致下游编译失败），因此这里对不支持的
# 架构在 build 阶段 fail-fast 并给出源码编译指引，而不是静默下载错误架构。
#
# 重要：绝不能把“不支持架构”写成解析期 `else $(error ...)`——
# OpenWrt 在 make defconfig 生成 tmp/.config-package.in 的元数据扫描阶段会
# include 本 Makefile，而此刻 $(ARCH) 尚未从 target 配置解析出来（为空）。
# 一旦在这里写 `else $(error ...)`，$(ARCH) 为空即触发解析期 make 语法错误，
# 该包会被 packageinfo 扫描丢弃，CONFIG_PACKAGE_luci-app-taygedo 变成未知
# 符号并被 defconfig 清除，最终 .config 里根本不出现本包（表现为“包未选中”），
# 下游 CI 报“luci-app-taygedo 未选中”且很难定位。
#
# 因此这里只做“已知架构 → triple”的正向映射：未知/空架构令 TAYGEDO_TRIPLE
# 为空，真正的 fail-fast 放到 Build/Prepare 的 recipe 头部（见下）。
ifeq ($(ARCH),x86_64)
  TAYGEDO_TRIPLE:=x86_64-unknown-linux-musl
endif
ifeq ($(ARCH),aarch64)
  TAYGEDO_TRIPLE:=aarch64-unknown-linux-musl
endif

# 预编译产物名带 `-openwrt` 后缀：同一 target 的默认产物是 webui 形态
# （Windows/Debian/静态部署用），两者不是同一个二进制。下载错文件会把
# 一个带 WebUI 的通用版本装进路由器，所以这里显式区分而不是靠约定。
TAYGEDO_ARCHIVE:=taygedo-rs-$(TAYGEDO_TRIPLE)-openwrt.tar.gz
TAYGEDO_DL_URL:=$(TAYGEDO_REPO)/releases/download/$(TAYGEDO_VER)/$(TAYGEDO_ARCHIVE)
TAYGEDO_DL:=$(DL_DIR)/$(TAYGEDO_ARCHIVE)

# 源码交叉编译产物（构建脚本写入；存在即优先使用，完全跳过网络下载）。
# 放在包目录内而不是 dl/：dl/ 会被全局缓存复用，跨架构/跨版本同名时会串味；
# 包目录随本次构建树生命周期存在，语义更准确。
TAYGEDO_LOCAL_BIN:=./local/taygedo-rs

define Package/luci-app-taygedo
  SECTION:=luci
  CATEGORY:=LuCI
  SUBMENU:=3. Applications
  TITLE:=Taygedo auto attendance (塔吉多自动签到)
  URL:=$(TAYGEDO_REPO)
  DEPENDS:=+ca-bundle +luci-base +curl
  PKGARCH:=all
endef

# 升级保留清单：
# - /etc/config/taygedo  —— UCI 服务级配置（端口/免鉴权/加密策略等）
# - /etc/taygedo/*       —— 业务配置 config.json、账号、keyring.json（长期身份私钥）
#
# 说明：/etc/taygedo 是运行期数据目录而非包内文件，opkg/apk 默认不会删除
# 未知文件，因此天然保留。此处显式声明 conffiles 是为了让 **UCI 配置** 在
# 升级时走"保留用户修改、仅合入新增默认项"的标准流程；否则新版新增的
# lan_cidrs / luci_session_check 等选项会被安装为默认值而覆盖用户设置。
define Package/luci-app-taygedo/conffiles
/etc/config/taygedo
endef

define Package/luci-app-taygedo/description
  LuCI interface (JS) for the Taygedo (Hotta 幻塔 / Neverness to Everness 异环)
  daily attendance tool. Multi-account, SMS/password login, daily scheduling.
  LuCI is the only entry on OpenWrt. The bundled binary is built with the
  `openwrt` feature: it contains no WebUI, no login/password endpoints, no
  static assets and no browser-side crypto frontend at all. Data is fetched
  through an internal rpcd/ucode channel over loopback; authentication is
  delegated to the OpenWrt session.
endef

define Build/Prepare
	$(call Build/Prepare/Default)
	# 两条路径必须在同一个 shell 里分支：OpenWrt 的 recipe 逐行独立执行，
	# 一旦把 if/else 拆成多行，条件判断会与下游命令脱节（表现为两条路径都跑，
	# 或在无 dl/ 文件时仍尝试 tar）。所以整段用反斜杠续行包成一个 recipe。
	@if [ -f "$(TAYGEDO_LOCAL_BIN)" ]; then \
		echo "luci-app-taygedo: 使用源码交叉编译产物 $(TAYGEDO_LOCAL_BIN)"; \
		$(CP) "$(TAYGEDO_LOCAL_BIN)" $(PKG_BUILD_DIR)/taygedo-rs; \
	else \
		echo "luci-app-taygedo: 未发现本地产物，回退到预编译 Release 下载"; \
		if [ -z "$(TAYGEDO_TRIPLE)" ]; then \
			echo "ERROR: luci-app-taygedo: 架构 '$(ARCH)' 暂无预编译产物（仅支持 x86_64 / aarch64）。请改用 Scripts/Build-Taygedo.sh 源码编译" >&2; \
			exit 1; \
		fi; \
		if [ ! -f "$(TAYGEDO_DL)" ]; then \
			echo "Downloading $(TAYGEDO_DL_URL)"; \
			curl -fsSL --retry 3 -o "$(TAYGEDO_DL)" "$(TAYGEDO_DL_URL)"; \
		fi; \
		$(TAR) -xzf "$(TAYGEDO_DL)" -C $(PKG_BUILD_DIR); \
	fi
endef

# 注意：预编译产物为静态 musl 二进制（Release 资产已 stripped），且编译 step
# 跑在 x86_64 宿主上，不能对 aarch64 目标二进制执行 $(TARGET_STRIP)（展开为空
# 时会把整条 recipe 退化成“直接执行该二进制”，宿主无法执行 aarch64 ELF）。
# 因此这里只做文件操作（chmod），不执行目标二进制。
define Build/Compile
	chmod 755 $(PKG_BUILD_DIR)/taygedo-rs
endef

define Package/luci-app-taygedo/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_BUILD_DIR)/taygedo-rs $(1)/usr/bin/taygedo-rs

	# LuCI JS 前端（view）
	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/taygedo
	$(INSTALL_DATA) ./htdocs/luci-static/resources/view/taygedo/status.js \
		$(1)/www/luci-static/resources/view/taygedo/status.js

	# rpcd 内部通道（ucode 插件，服务端调 loopback）
	$(INSTALL_DIR) $(1)/usr/share/rpcd/ucode
	$(INSTALL_DATA) ./root/usr/share/rpcd/ucode/luci.taygedo \
		$(1)/usr/share/rpcd/ucode/luci.taygedo

	# 菜单 / ACL
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d
	$(INSTALL_DATA) ./root/usr/share/luci/menu.d/taygedo.json \
		$(1)/usr/share/luci/menu.d/taygedo.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) ./root/usr/share/rpcd/acl.d/luci-app-taygedo.json \
		$(1)/usr/share/rpcd/acl.d/luci-app-taygedo.json

	# UCI 默认配置 + init 脚本
	$(INSTALL_DIR) $(1)/etc/config
	$(INSTALL_DATA) ./root/etc/config/taygedo $(1)/etc/config/taygedo
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) ./root/etc/init.d/taygedo $(1)/etc/init.d/taygedo
endef

$(eval $(call BuildPackage,luci-app-taygedo))

# ---------------------------------------------------------------------------
# 附：源码交叉编译路径（Build/Prepare 的分支 A）说明
#
# 由 OpenWRT-CI 仓库的 Scripts/Build-Taygedo.sh 在固件编译前执行：
#
#   1. 把本仓库的 Rust 工程（Cargo.toml / Cargo.lock / src / .cargo / static）
#      放到本包目录下的 src/；
#   2. 在宿主机上 `cargo build --release --locked --target <triple>
#      --no-default-features --features openwrt`，链接器优先取 OpenWrt 自身
#      staging_dir 里的 `<arch>-openwrt-linux-musl-gcc`，与固件同 libc；
#   3. 产物落到本目录 local/taygedo-rs。
#
# 本 Makefile 不感知交叉编译细节：只要 local/taygedo-rs 存在就直接用，
# 因此「源码编译」与「预编译下载」对打包逻辑完全等价，install 段无需改动。
#
# 不用 OpenWrt 的 lang/rust feed（$(TOPDIR)/feeds/packages/lang/rust）的原因：
# 那会把 rustc 本体也编译一遍（约 1.5~3 小时），而本包只需要一个静态 musl
# 可执行文件；宿主机 rustup 交叉编译是分钟级。
#
# 注意：mips/mipsel 目标 ring 后端不支持交叉编译，需改用 native-tls 或
# 关闭该架构的包依赖。
# ---------------------------------------------------------------------------