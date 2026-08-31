#!/bin/bash
# 打包脚本：把编译好的二进制 + LuCI 文件打包成 .deb / .ipk / .apk
#
# 用法:
#   package.sh deb <arch> <二进制路径> <版本> <输出目录> [release]
#   package.sh ipk <arch> <二进制路径> <版本> <输出目录> [release]
#   package.sh apk <arch> <二进制路径> <版本> <输出目录> [release]
#
# arch 为 OpenWrt 架构名（x86_64 / aarch64_cortex-a53 / arm_cortex-a7 ...），
# deb 模式下 arch 映射为 amd64/arm64/armhf。
#
# 版本组合规则（各包管理器格式不同，必须分别处理）：
#   deb -> Version: <ver>                  文件 taygedo-rs_<ver>_<arch>.deb
#   ipk -> Version: <ver>-<rel>            文件 luci-app-taygedo_<ver>-<rel>_<arch>.ipk
#   apk -> version: <ver>-r<rel>           文件 luci-app-taygedo_<ver>-r<rel>_<arch>.apk
# release 缺省为 1。tag 构建传 1，非 tag（手动触发）传 GITHUB_RUN_NUMBER。
set -euo pipefail

MODE="$1"
ARCH="$2"
BIN="$3"
VER="$4"
OUT="$5"
REL="${6:-1}"

PKG_NAME="taygedo-rs"
LUCI_NAME="luci-app-taygedo"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LUCI_DIR="$ROOT/openwrt/luci-app-taygedo"

# 版本规范化：
#   1) 去掉 v 前缀；
#   2) 剔除包管理器不允许的字符，并去掉首尾的点/横杠；
#   3) 若结果仍不以数字开头（典型：workflow_dispatch 时 GITHUB_REF_NAME=main，
#      ${GITHUB_REF_NAME#v} 得到 "main"），回退读取 Cargo.toml 的 version；
#   4) 仍失败则明确报错退出——绝不能再静默产出 main-1 这类废包。
normalize_version() {
    local raw="$1" v cv=""
    v="${raw#v}"
    v="$(printf '%s' "$v" | sed -E 's/[^0-9A-Za-z.+-]//g; s/^[.-]+//; s/[.-]+$//')"
    if printf '%s' "$v" | grep -qE '^[0-9]'; then
        printf '%s' "$v"
        return 0
    fi
    if [ -f "$ROOT/Cargo.toml" ]; then
        cv="$(grep -m1 -E '^[[:space:]]*version[[:space:]]*=' "$ROOT/Cargo.toml" \
              | sed -E 's/.*"([^"]+)".*/\1/')"
    fi
    cv="${cv#v}"
    if printf '%s' "$cv" | grep -qE '^[0-9]'; then
        echo "警告: 传入版本 '$raw' 非法，已回退为 Cargo.toml 版本 '$cv'" >&2
        printf '%s' "$cv"
        return 0
    fi
    echo "错误: 版本号非法（传入 '$raw'，且无法从 Cargo.toml 推断）" >&2
    exit 1
}

VER="$(normalize_version "$VER")"
# release 必须是非负整数，否则回落为 1
case "$REL" in
    ''|*[!0-9]*) REL=1 ;;
esac

[ -x "$BIN" ] || { echo "二进制不存在或不可执行: $BIN"; exit 1; }
mkdir -p "$OUT"
# 转绝对路径，避免后续 (cd "$STAGE" && tar ...) 中相对路径失效
OUT="$(cd "$OUT" && pwd)"
BIN="$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")"

# 构建文件系统根（含二进制 + 可选 LuCI 文件）
stage_root() {
    local PKGROOT="$1"
    local WITH_LUCI="${2:-1}"
    install -Dm755 "$BIN" "$PKGROOT/usr/bin/$PKG_NAME"
    if [ "$WITH_LUCI" = "1" ]; then
        if [ -d "$LUCI_DIR/root" ]; then
            cp -a "$LUCI_DIR/root/." "$PKGROOT/"
        fi
        # 现代 LuCI JS 前端（htdocs -> /www）
        if [ -d "$LUCI_DIR/htdocs" ]; then
            mkdir -p "$PKGROOT/www"
            cp -a "$LUCI_DIR/htdocs/." "$PKGROOT/www/"
        fi
    fi
}

# Debian 架构映射
deb_arch() {
    case "$ARCH" in
        x86_64) echo "amd64" ;;
        aarch64*) echo "arm64" ;;
        arm*) echo "armhf" ;;
        *) echo "$ARCH" ;;
    esac
}

case "$MODE" in
    deb)
        DARCH="$(deb_arch)"
        STAGE="$(mktemp -d)"
        PKGROOT="$STAGE"
        DEBIAN="$STAGE/DEBIAN"
        mkdir -p "$DEBIAN"

        stage_root "$PKGROOT" 0

        # systemd unit
        mkdir -p "$PKGROOT/usr/lib/systemd/system"
        cat > "$PKGROOT/usr/lib/systemd/system/$PKG_NAME.service" <<EOF
[Unit]
Description=Taygedo auto attendance
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/usr/bin/$PKG_NAME
Environment=TAYGEDO_LISTEN=0.0.0.0:8787
Environment=TAYGEDO_DATA_DIR=/var/lib/taygedo
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

        cat > "$DEBIAN/control" <<EOF
Package: $PKG_NAME
Version: $VER
Section: net
Priority: optional
Architecture: $DARCH
Maintainer: LianXia233
Depends: libc6
Description: Taygedo auto attendance (塔吉多自动签到)
  Rust 重写的塔吉多（幻塔/异环）每日自动签到工具，
  带鉴权的响应式 WebUI。
EOF

        # conffiles / postinst 目录
        cat > "$DEBIAN/postinst" <<EOF
#!/bin/sh
set -e
if [ ! -d /var/lib/taygedo ]; then mkdir -p /var/lib/taygedo; fi
if command -v systemctl >/dev/null 2>&1; then systemctl enable $PKG_NAME.service >/dev/null 2>&1 || true; fi
exit 0
EOF
        chmod 755 "$DEBIAN/postinst"

        dpkg-deb --build --root-owner-group "$STAGE" "$OUT/${PKG_NAME}_${VER}_${DARCH}.deb"
        rm -rf "$STAGE"
        echo "built: $OUT/${PKG_NAME}_${VER}_${DARCH}.deb"
        ;;

    ipk)
        STAGE="$(mktemp -d)"
        PKGROOT="$STAGE/data"
        CTRLDIR="$STAGE/control"
        mkdir -p "$PKGROOT" "$CTRLDIR"

        stage_root "$PKGROOT"

        cat > "$CTRLDIR/control" <<EOF
Package: $LUCI_NAME
Version: ${VER}-${REL}
Depends: libc
Section: luci
Architecture: $ARCH
Maintainer: LianXia233
Description: Taygedo auto attendance (塔吉多自动签到)
EOF

        printf '2.0\n' > "$STAGE/debian-binary"
        (cd "$PKGROOT" && tar czf "$STAGE/data.tar.gz" .)
        (cd "$CTRLDIR" && tar czf "$STAGE/control.tar.gz" .)
        (
            cd "$STAGE"
            ar rcs "$OUT/${LUCI_NAME}_${VER}-${REL}_${ARCH}.ipk" debian-binary control.tar.gz data.tar.gz
        )
        rm -rf "$STAGE"
        echo "built: $OUT/${LUCI_NAME}_${VER}-${REL}_${ARCH}.ipk"
        ;;

    apk)
        # OpenWrt 24.10+ 使用 apk-tools 3.x 的 v3 ADB 格式（非 v2 gzip 格式）。
        # 手工拼接 gzip(PKGINFO)+gzip(data) 的 v2 包无法被 apk-tools 3.0 安装，
        # 必须用 apk-tools 3.0 自带的 `apk mkpkg` 生成正确的 ADB v3 包。
        # 需要 apk-tools >= 3.0，可通过 APK_BIN 指定 apk.static 路径，或已在 PATH 中。
        APKBIN="${APK_BIN:-apk}"

        # 校验 apk 命令存在且支持 mkpkg
        if ! command -v "$APKBIN" >/dev/null 2>&1; then
            echo "错误: 未找到 apk-tools ($APKBIN)。apk 打包需要 apk-tools >= 3.0 的 mkpkg 命令。" >&2
            echo "      在 CI 中会自动下载 apk-tools-static；本地可 export APK_BIN=/path/to/apk.static" >&2
            exit 1
        fi

        STAGE="$(mktemp -d)"
        PKGROOT="$STAGE/data"
        mkdir -p "$PKGROOT"

        stage_root "$PKGROOT"

        APK="$OUT/${LUCI_NAME}_${VER}-r${REL}_${ARCH}.apk"
        "$APKBIN" mkpkg \
            --info "name:$LUCI_NAME" \
            --info "version:${VER}-r${REL}" \
            --info "arch:$ARCH" \
            --info "description:Taygedo auto attendance (塔吉多自动签到)" \
            --info "license:MIT" \
            --info "maintainer:LianXia233" \
            --info "url:https://github.com/LianXia233/taygedo-CI" \
            --info "origin:$LUCI_NAME" \
            --files "$PKGROOT" \
            --output "$APK"
        rm -rf "$STAGE"
        echo "built: $APK"
        ;;

    *)
        echo "未知模式: $MODE (可选 deb/ipk/apk)"; exit 1 ;;
esac
