//! OpenWrt：不依赖 systemd / TPM / 桌面密钥服务，只用 root 所有的 0600 密钥文件。
//!
//! ## 为什么不用 systemd 凭据
//!
//! OpenWrt 用 procd，没有 systemd，也没有 `CREDENTIALS_DIRECTORY`；
//! 绝大多数设备没有 TPM。因此这里**只有**一种方案：`/etc/taygedo/keys/master.key`，
//! 权限 0600、属主 root。它由 `init.d/taygedo` 在启动前确保目录权限，
//! 由本模块在读取时强制校验。
//!
//! ## BusyBox / musl 适配
//!
//! 本模块只用 `std::fs` + `atomic` 模块的 `O_NOFOLLOW`/`fstat`，不依赖
//! glibc 专有接口，musl 下行为一致。密钥文件是固定 42 字节的小文件，
//! 对 flash 写入量可忽略（只在首次启动写一次）。
//!
//! ## LuCI 与 procd 不受影响
//!
//! 密钥初始化发生在 `Store::new()`（进程启动早期、监听端口之前），
//! 不触碰 procd 的 respawn 逻辑；LuCI 通过 HTTP API 访问，不直接读密钥。

use std::path::Path;

/// 判定当前**运行平台**是否 OpenWrt（存在 `/etc/openwrt_release`）。
///
/// 注意与 `service::IS_OPENWRT` 的区别：那是**构建形态**（这个二进制是不是
/// 为 LuCI 集成编译的），这里是**宿主平台**（这台机器有没有 systemd/TPM）。
/// 密钥保护方式取决于后者：同一份 `webui` 二进制跑在 OpenWrt 上也该用
/// 0600 文件而不是 systemd 凭据。因此这里保留运行期探测，且独立实现，
/// 避免存储层反向依赖业务层（也便于单独测试）。
pub fn is_openwrt() -> bool {
    Path::new("/etc/openwrt_release").exists()
}

#[cfg(test)]
mod tests {
    #[test]
    fn keys_dir_layout() {
        let p = super::super::keys_dir(std::path::Path::new("/etc/taygedo"));
        assert_eq!(p, std::path::Path::new("/etc/taygedo/keys"));
    }
}
