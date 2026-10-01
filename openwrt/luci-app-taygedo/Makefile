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
PKG_VERSION:=0.5.1
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

# 升级保留清单：
# - /etc/config/taygedo  —— UCI 服务级配置（端口/免鉴权/加密策略等）
# - /etc/taygedo/*       —— 业务配置 config.json、账号、keyring.json（长期身份私钥）
#
# 说明：/etc/taygedo 是运行期数据目录而非包内文件，opkg/apk 默认不会删除
# 未知文件，因此天然保留。此处显式声明 conffiles 是为了让 **UCI 配置** 在
# 升级时走"保留用户修改、仅合入新增默认项"的标准流程；否则新版新增的
# crypto_policy / lan_cidrs 等选项会被安装为默认值而覆盖用户设置。
define Package/luci-app-taygedo/conffiles
/etc/config/taygedo
endef

define Package/luci-app-taygedo/description
  LuCI interface (JS) for the Taygedo (Hotta 幻塔 / Neverness to Everness 异环)
  daily attendance tool. Multi-account, SMS/password login, daily scheduling
  and a built-in responsive WebUI with authentication.
endef

define Build/Prepare
	$(call Build/Prepare/Default)
	# 构建期架构校验（解析期不可做这种封死判断，见上方架构映射注释）：
	# 若 TAYGEDO_TRIPLE 为空，说明当前 ARCH 没有对应预编译产物，直接 fail-fast
	# 并给出源码编译指引，避免后续以空 triple 下载「.tar.gz」产生晦涩的 404。
	@if [ -z "$(TAYGEDO_TRIPLE)" ]; then \
		echo "ERROR: luci-app-taygedo: 架构 '$(ARCH)' 暂无预编译产物（仅支持 x86_64 / aarch64）。如需使用请改用本 Makefile 末尾「方式 B：源码编译」" >&2; \
		exit 1; \
	fi
	$(eval TAYGEDO_DL:=$(DL_DIR)/$(TAYGEDO_ARCHIVE))
	@if [ ! -f "$(TAYGEDO_DL)" ]; then \
		echo "Downloading $(TAYGEDO_DL_URL)"; \
		curl -fsSL --retry 3 -o "$(TAYGEDO_DL)" "$(TAYGEDO_DL_URL)"; \
	fi
	$(TAR) -xzf "$(TAYGEDO_DL)" -C $(PKG_BUILD_DIR)
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