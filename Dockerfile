# 塔吉多自动签到 (Rust) - 多阶段构建
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
# 显式指定形态：容器是独立部署，必须有 WebUI。默认 feature 就是 webui，
# 但写出来可以防止将来改默认值后容器静默变成"无界面纯 API"。
RUN cargo build --release --locked --no-default-features --features webui

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /app/target/release/taygedo-rs /usr/local/bin/taygedo-rs
ENV TAYGEDO_DATA_DIR=/data \
    TAYGEDO_LISTEN=0.0.0.0:8787 \
    TAYGEDO_EDITION=standalone
EXPOSE 8787
VOLUME ["/data"]
CMD ["taygedo-rs"]
