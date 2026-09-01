# OpenWrt LuCI app: 塔吉多自动签到 (Taygedo auto attendance)
#
# 现代 LuCI JS 前端：htdocs/luci-static/resources/view/taygedo/status.js
# 界面与内置 WebUI 功能一致，直接调用 Rust 后端（默认 8787 端口）的 REST API。
#
# 两种构建方式（二选一）：
#   A. 预编译二进制下载（默认）：从 GitHub Release 下载对应架构的 musl 二进制。
#      由仓库根目录 .github/workflows/build.yml 预编译产出。
#   B. 源码编译：需要 openwrt/packages 的 lang/rust feed，见文件末尾注释。
#
# 安装位置：/usr/bin/taygedo-rs + LuCI JS 界面（服务 -> 塔吉多签到）

include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-taygedo
PKG_VERSION:=0.4.9
PKG_RELEASE:=1
PKG_LICENSE:=GPL-3.0
PKG_LICENSE_FILES:=LICENSE
PKG_MAINTAINER:=LianXia233

# 预编译二进制发布仓库与版本（需与 .github/workflows/build.yml 的 tag 一致）
TAYGEDO_REPO:=https://github.com/LianXia233/taygedo-CI
TAYGEDO_VER:=v$(PKG_VERSION)

include $(INCLUDE_DIR)/package.mk

# ---------------------------------------------------------------------------
# OpenWrt CONFIG_ARCH -> Rust musl target 映射
# ---------------------------------------------------------------------------
ifeq ($(CONFIG_ARCH),x86_64)
  TAYGEDO_TRIPLE:=x86_64-unknown-linux-musl
else ifeq ($(CONFIG_ARCH),aarch64)
  TAYGEDO_TRIPLE:=aarch64-unknown-linux-musl
else ifeq ($(CONFIG_ARCH),arm)
  TAYGEDO_TRIPLE:=armv7-unknown-linux-musleabihf
else ifeq ($(CONFIG_ARCH),mipsel)
  TAYGEDO_TRIPLE:=mipsel-unknown-linux-musl
else ifeq ($(CONFIG_ARCH),mips)
  TAYGEDO_TRIPLE:=mips-unknown-linux-musl
else
  TAYGEDO_TRIPLE:=x86_64-unknown-linux-musl
endif

TAYGEDO_ARCHIVE:=taygedo-rs-$(TAYGEDO_TRIPLE).tar.gz
TAYGEDO_DL_URL:=$(TAYGEDO_REPO)/releases/download/$(TAYGEDO_VER)/$(TAYGEDO_ARCHIVE)

define Package/luci-app-taygedo
  SECTION:=luci
  CATEGORY:=LuCI
  SUBMENU:=3. Applications
  TITLE:=Taygedo auto attendance (塔吉多自动签到)
  URL:=$(TAYGEDO_REPO)
  DEPENDS:=+ca-bundle +luci-base
  PKGARCH:=all
endef

define Package/luci-app-taygedo/description
  LuCI interface (JS) for the Taygedo (Hotta 幻塔 / Neverness to Everness 异环)
  daily attendance tool. Multi-account, SMS/password login, daily scheduling
  and a built-in responsive WebUI with authentication.
endef

define Build/Prepare
	$(call Build/Prepare/Default)
	$(eval TAYGEDO_DL:=$(DL_DIR)/$(TAYGEDO_ARCHIVE))
	@if [ ! -f "$(TAYGEDO_DL)" ]; then \
		echo "Downloading $(TAYGEDO_DL_URL)"; \
		$(SCRIPT_DIR)/download.pl "$(TAYGEDO_DL)" "$(TAYGEDO_DL_URL)"; \
	fi
	$(TAR) -xzf "$(TAYGEDO_DL)" -C $(PKG_BUILD_DIR)
endef

define Build/Compile
	$(TARGET_STRIP) $(PKG_BUILD_DIR)/taygedo-rs
endef

define Package/luci-app-taygedo/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_BUILD_DIR)/taygedo-rs $(1)/usr/bin/taygedo-rs

	# LuCI JS 前端（view）
	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/taygedo
	$(INSTALL_DATA) ./htdocs/luci-static/resources/view/taygedo/status.js \
		$(1)/www/luci-static/resources/view/taygedo/status.js

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
# 方式 B：源码编译（可选，替代上面的预编译下载）
#
# 1. 在 openwrt/packages 里启用 lang/rust feed。
# 2. 把整个 Rust 项目源码复制到本包目录下的 taygedo-rs/（含 Cargo.toml、
#    Cargo.lock、.cargo/config.toml、src/）。
# 3. 把 Makefile 中 Build/Prepare 与 Build/Compile 换成：
#
#   define Build/Prepare
#       $(call Build/Prepare/Default)
#       $(CP) ./taygedo-rs $(PKG_BUILD_DIR)/
#   endef
#
#   define Build/Compile
#       cd $(PKG_BUILD_DIR)/taygedo-rs && \
#           CARGO_HOME=$(CARGO_HOME) \
#           cargo build --release --target $(RUST_TARGET) && \
#           $(TARGET_STRIP) target/$(RUST_TARGET)/release/taygedo-rs
#   endef
#
# 4. Package/install 里改为安装 $(PKG_BUILD_DIR)/taygedo-rs/target/.../taygedo-rs。
#
# 注意：mips/mipsel 目标 aws-lc-rs 不支持，需将 Cargo.toml 的 reqwest 改为
#   features = ["native-tls"]（OpenWrt 用 libopenssl）或 ["rustls-tls"]+ring。
# ---------------------------------------------------------------------------
