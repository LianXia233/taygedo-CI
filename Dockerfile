# syntax=docker/dockerfile:1.7
#
# 塔吉多自动签到 —— 容器镜像
#
# 形态：webui（独立部署，内置 WebUI）。容器里没有 LuCI，也没有 rpcd，
# 所以必须是带前端的形态；显式写 --features webui 而不是依赖默认值，
# 防止将来有人把默认 feature 改成 openwrt 后，镜像静默变成"无界面纯 API"。
#
# 基础镜像选 Debian 而非 alpine：
#   - 与仓库其它发布产物（.deb）保持同一套 glibc 运行时，排障时行为一致；
#   - glibc 的 malloc / DNS 解析在高并发下表现比 musl 更稳（musl 的
#     线程栈与 resolver 在高连接数场景有已知短板）；
#   - bookworm 生命周期到 2028，比 alpine 的大版本节奏更适合长期挂机。
#   代价是镜像约 85 MB 而非 15 MB —— 对一个常驻签到服务可接受。
#
# 构建参数：
#   CARGO_MIRROR=keep  保留 .cargo/config.mirror.toml（中国网络用 rsproxy 加速）；
#                      默认 off 会把它清空，走官方 crates.io —— GitHub Actions
#                      runner 在海外，访问 rsproxy.cn 反而更慢甚至超时。

# ---------------------------------------------------------------------------
# 构建阶段
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS builder

WORKDIR /app

# 先只复制清单，让依赖编译独占一层缓存。
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo

ARG CARGO_MIRROR=off
RUN if [ "$CARGO_MIRROR" = "off" ]; then \
        printf '' > .cargo/config.mirror.toml; \
    fi

# 依赖层：放一个空 main + 空 lib，让 cargo 先把全部第三方 crate 编完。这一层只随
# Cargo.toml / Cargo.lock 变化失效，改业务代码不会重编 ring / tokio。
#
# 空 lib 不能省：Cargo.toml 声明了 [lib] name="taygedo" path="src/lib.rs"，
# 缺少该文件时 cargo 在目标解析阶段就报
#   can't find lib `taygedo` at path `/app/src/lib.rs`
# 整个镜像构建直接 exit 101（桌面单包把服务端拆成 lib 目标后引入的回归）。
RUN mkdir -p src && printf 'fn main() {}\n' > src/main.rs && : > src/lib.rs && \
    cargo build --release --locked --no-default-features --features webui

# 真实源码与前端资产。static/ 必须在编译前就位：webui 形态下 manifest.rs
# 用 include_str! 内嵌这些文件，缺一个是编译错误（不是运行时错误）。
#
# 这里必须刷新 mtime：Docker COPY 保留源文件的修改时间，而上面那次 dummy
# 编译的产物是"刚刚"生成的；当源文件 mtime 早于产物时，cargo 的 fingerprint
# 判定为 fresh，直接跳过编译 —— 镜像里就会静默装进那个空壳 main
# （实测：产物只有 281 KB，容器起来立刻退出，且构建全绿不报错）。
# 删掉产物是第二道保险：产物缺失时 cargo 一定会重编本 crate。
# lib 产物一并删：dummy 阶段那个 src/lib.rs 是空文件，留着 rlib 只会让
# "跳过重编"的风险多一处入口。
COPY static ./static
COPY src ./src
RUN find src static -type f -exec touch {} + && \
    rm -f target/release/taygedo-rs target/release/libtaygedo.rlib && \
    cargo build --release --locked --no-default-features --features webui

# ---------------------------------------------------------------------------
# 运行阶段
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

# wget 只服务于 HEALTHCHECK：slim 镜像不带任何 HTTP 客户端，
# 而容器自身的 /api/meta 是明文回环端点，用 wget 探活最轻。
# --no-install-recommends + 清 apt 列表，避免把 40 MB 的推荐包带进来。
RUN apt-get update && \
    apt-get install -y --no-install-recommends wget && \
    rm -rf /var/lib/apt/lists/*

# 非 root 运行：容器被拿下时，攻击面从 root 收窄到普通用户。
# 固定 uid/gid 10001，方便宿主机挂载 /data 时对齐权限
# （chown -R 10001:10001 ./data）。
RUN groupadd --system --gid 10001 taygedo && \
    useradd --system --uid 10001 --gid taygedo \
            --home-dir /data --shell /usr/sbin/nologin taygedo && \
    mkdir -p /data && chown taygedo:taygedo /data

COPY --from=builder /app/target/release/taygedo-rs /usr/local/bin/taygedo-rs

WORKDIR /data

# 形态（webui / openwrt）是编译期 feature，不是运行期环境变量：
# `TAYGEDO_EDITION` 自双形态解耦后已彻底废弃，程序不再读取它。
# 这里只留两个真正生效的默认值；其余配置项见 docs/docker.md。
ENV TAYGEDO_DATA_DIR=/data \
    TAYGEDO_LISTEN=0.0.0.0:8787

EXPOSE 8787
VOLUME ["/data"]

USER taygedo

# /api/meta 是不需要鉴权的公开端点，用它探活不会在日志里留下失败登录记录。
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD wget -q -T 3 -O /dev/null http://127.0.0.1:8787/api/meta || exit 1

CMD ["taygedo-rs"]
