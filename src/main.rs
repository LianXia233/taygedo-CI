// 构建形态在编译期确定，运行期不再做任何探测。
//
// 三者互斥：一个二进制不可能同时"自带 WebUI"、"由 LuCI 独占入口"和
// "只给本机 GPUI 客户端提供回环 API"。同时开启会让路由与鉴权出现二义性；
// 一个都不开则没有任何访问控制路径。因此这里用编译错误把配置错误挡在
// 构建阶段，而不是留到运行期才暴露。
//
// 注意：同一份诊断在 `src/lib.rs` 里也有一份。库与 bin 是两个独立的 crate
// root，各自按 feature 编译；只在其中一处拦截会让另一种链接方式漏过去。
#[cfg(any(
    all(feature = "webui", feature = "openwrt"),
    all(feature = "webui", feature = "desktop"),
    all(feature = "openwrt", feature = "desktop")
))]
compile_error!("构建形态互斥：一次只能选择 webui / openwrt / desktop 之一");

#[cfg(not(any(feature = "webui", feature = "openwrt", feature = "desktop")))]
compile_error!("必须显式选择构建形态：webui / openwrt / desktop");

fn main() {
    // 全部启动逻辑在 lib 里：桌面单包要在同一进程内复用同一份实现。
    std::process::exit(taygedo::cli_main());
}
