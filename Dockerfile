# 塔吉多自动签到 (Rust) - 多阶段构建
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /app/target/release/taygedo-rs /usr/local/bin/taygedo-rs
ENV TAYGEDO_DATA_DIR=/data \
    TAYGEDO_LISTEN=0.0.0.0:8787
EXPOSE 8787
VOLUME ["/data"]
CMD ["taygedo-rs"]
