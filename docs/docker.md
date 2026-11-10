# Docker 部署与运维

本文是 `ghcr.io/lianxia233/taygedo-rs` 镜像的完整使用说明：怎么跑、数据在哪、
怎么备份、出问题怎么查。只想最快跑起来的话，看第一节就够了。

> 镜像固定为 **webui 形态**（内置 WebUI、有登录页）。容器里没有 LuCI / rpcd，
> OpenWrt 上那个无前端的 `openwrt` 形态不适用于 Docker。

---

## 1. 快速开始

### 1.1 直接运行

```bash
mkdir -p ./data
sudo chown -R 10001:10001 ./data     # 容器以 uid 10001 运行，见 §5.2

docker run -d --name taygedo-rs \
  -p 8787:8787 \
  -v "$PWD/data:/data" \
  --restart unless-stopped \
  ghcr.io/lianxia233/taygedo-rs:latest
```

浏览器打开 `http://<宿主机IP>:8787`。首次启动会自动生成随机口令，
**只在启动横幅里打印一次**：

```bash
docker logs taygedo-rs
```

想自己指定初始口令，加 `-e TAYGEDO_WEB_PASSWORD='你的口令'`。

### 1.2 docker compose

仓库里的 `docker-compose.yml` 已经指向 GHCR 成品镜像：

```bash
docker compose up -d
docker compose logs -f
```

### 1.3 首次登录

- 用户名固定 `admin`（可在 WebUI 设置页修改）
- 口令：`docker logs` 里的随机串，或 `TAYGEDO_WEB_PASSWORD` 指定的值
- 登录后拿到 7 天有效的 Bearer token，存在浏览器本地

---

## 2. 镜像说明

| 项目 | 值 |
| :--- | :--- |
| 镜像地址 | `ghcr.io/lianxia233/taygedo-rs` |
| 架构 | `linux/amd64`、`linux/arm64`（同一 tag 的多架构 manifest） |
| 基础镜像 | `debian:bookworm-slim`（与 `.deb` 产物同一套 glibc） |
| 镜像体积 | 80~82 MB（其中二进制约 5 MB） |
| 运行用户 | `taygedo`，uid/gid `10001`，非 root |
| 监听 | `0.0.0.0:8787`（`TAYGEDO_LISTEN` 可改） |
| 数据卷 | `/data` |
| 健康检查 | 每 30 s 请求 `/api/meta`，超时 5 s，启动宽限 10 s，连续 3 次失败判 unhealthy |
| 构建方式 | GitHub Actions，`linux/arm64` 走 QEMU 模拟编译 |

### 2.1 镜像里有什么

- `/usr/local/bin/taygedo-rs`：Rust 后端，`--features webui`
- 内嵌前端：`index.html` 与静态资产全部 `include_str!` 编进二进制，
  不落地成文件，所以**没有单独的 web 根目录**可改
- `wget`：仅用于健康检查（Debian slim 不带任何 HTTP 客户端）
- 一个 `taygedo` 用户和 `/data` 目录

### 2.2 镜像里没有什么

| 没有 | 原因 |
| :--- | :--- |
| `ca-certificates` | reqwest 用 `rustls-tls` + `webpki-roots`，根证书已编进二进制 |
| `curl` / `openssl` / `bash` 工具链 | 只留 `sh` 与 `wget`，减小攻击面。排障用 `docker exec -it taygedo-rs sh` |
| Rust 编译器 | 多阶段构建，构建产物是最终二进制 |
| `TAYGEDO_EDITION` 环境变量 | 双形态解耦后形态由编译期 feature 决定，该变量已废弃，程序不再读取 |

---

## 3. 标签与版本选择

| 标签 | 触发条件 | 建议用途 |
| :--- | :--- | :--- |
| `latest` | 推送 `v*` tag | 个人自用，跟随最新正式版 |
| `0.8.1` | 推送 `v*` tag | 生产钉死，避免被动升级 |
| `0.8` | 推送 `v*` tag | 接受补丁级更新，不接受次要版本变更 |
| `edge` | 每次 `main` 有提交 | 尝鲜，含未发版改动 |
| `sha-56ac142` | 每次 `main` 有提交 | 精确定位到某次提交，复现问题 |

生产环境如果连 tag 都不想赌，可以钉 digest：

```bash
docker pull ghcr.io/lianxia233/taygedo-rs@sha256:7583aa49...
docker run ... ghcr.io/lianxia233/taygedo-rs@sha256:7583aa49...
```

digest 是不可变的，即使同名 tag 被重新推送也不会变。

---

## 4. 网络与访问控制

### 4.1 端口

容器内固定监听 `8787`。要换宿主机端口，改 `-p 9000:8787` 即可，
**不要**改容器内的 `TAYGEDO_LISTEN` 端口号（健康检查写死了 8787）。

### 4.2 关键：服务看到的是"对端 IP"

服务有一道来源白名单（`lan_cidrs`，默认 RFC1918 三段 + 回环 + IPv6 ULA/链路本地）。
判定依据是 **TCP 对端地址**，在 Docker 里它通常不是你的电脑：

| 网络模式 | 服务看到的来源 IP | 结果 |
| :--- | :--- | :--- |
| 默认 bridge + `-p 8787:8787` | `172.17.0.1`（docker 网桥网关） | 命中 `172.16.0.0/12`，放行 |
| `--network host` | 真实客户端 IP | 内网放行；公网来源 403 |
| 前面挂 Nginx / Caddy | 反代容器的 IP | 同网段放行；跨网段需加白名单 |

### 4.3 什么情况会返回 403

业务接口（`/api/*`）要求来源命中 `lan_cidrs`，否则返回：

```json
{"ok": false, "code": "untrusted_source", "error": "仅接受本机或受信内网来源的请求"}
```

这是**默认拒绝**，不是配置错误。服务被误暴露到公网时，这一层挡住业务接口。
要把公网来源放进来，只能显式加白名单：

```bash
-e TAYGEDO_LAN_CIDRS='10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,127.0.0.0/8,203.0.113.0/24'
```

但更推荐的做法是**别把 8787 直接暴露到公网**，见 §4.4。

### 4.4 反向代理

推荐拓扑：`公网 → Nginx/Caddy（TLS 终止）→ 容器 8787`。

```nginx
location / {
    proxy_pass http://127.0.0.1:8787;
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_http_version 1.1;
    # WebSocket / SSE 目前都没用到，但保留升级头不会有害
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

注意两点：

1. **`X-Real-IP` 不会被服务用于来源判定**。白名单看的是 TCP 对端，
   也就是反代自己的地址。所以要么反代和容器在同一私网段，要么把这台
   反代的出口地址加进 `TAYGEDO_LAN_CIDRS`。
2. 反代要放行较大的请求体（导入配置、上传日志），以及**不要**改写
   `Authorization` 头。

### 4.5 应用层加密策略

`TAYGEDO_CRYPTO_POLICY` 控制 `taygedo/v2` 加密握手（X25519 + HKDF-SHA256 +
XChaCha20-Poly1305），与 TLS 是两层独立的东西：

| 值 | 行为 |
| :--- | :--- |
| `auto`（默认） | 受信内网来源允许明文；非受信来源强制走 v2 加密 |
| `always` | 所有来源强制 v2，浏览器端 WebUI 自带实现，访问无感 |
| `never` | 不强制加密，明文与加密请求都接受；仅建议在完全隔离的内网排障时临时使用 |

已经上了 HTTPS 反代的话，保持 `auto` 即可；加密层仍会保护"反代到容器"
这一段明文链路，属于纵深防御。

---

## 5. 数据持久化

### 5.1 `/data` 里有什么

| 文件 | 内容 | 形态 | 何时出现 |
| :--- | :--- | :--- | :--- |
| `config.json` | 调度时间、加密策略、白名单、口令哈希 | 密文 | 首次启动即生成 |
| `keyring.json` | 服务端 Ed25519 长期身份私钥 | 密文 | 首次启动即生成 |
| `accounts.json` | 账号、Cookie/凭据、签到结果 | 密文 | 添加第一个账号后 |
| `state.json` | 运行状态（上次执行时间、连续失败计数） | 密文 | 首次签到后 |
| `keys/master.key` | **主密钥**，解密上面四个文件的唯一钥匙 | 明文密钥文件，权限 `0600` | 首次启动即生成 |
| `.storage.lock` | 进程排他锁，防多实例并发写 | 空文件 | 运行时 |
| `.storage-migration.json` | 存储格式迁移记录（明文 → 密文的进度） | 明文 | 确实发生过迁移时 |

实际开机后通常长这样：

```
/data
├── config.json
├── keyring.json
├── accounts.json          # 添加账号后出现
├── state.json             # 首次签到后出现
├── keys/
│   └── master.key
├── .storage.lock
└── .storage-migration.json
```

> `keys/master.key` 丢了，其余文件就是一堆无法还原的字节。**备份必须包含它。**

密钥轮换后目录里还会多出 `*.rotate-bak-<旧 Key ID>` 备份文件，确认服务正常后可以删。

### 5.2 权限

容器以 `10001:10001` 运行。宿主机目录属主不对会直接启动失败：

```bash
mkdir -p ./data
sudo chown -R 10001:10001 ./data
```

用命名卷则不需要手动处理：

```bash
docker run -d --name taygedo-rs \
  -p 8787:8787 \
  -v taygedo-data:/data \
  ghcr.io/lianxia233/taygedo-rs:latest
```

### 5.3 备份

必须先停容器再复制，避免拿到写了一半的文件：

```bash
docker stop taygedo-rs
sudo tar -czf taygedo-backup-$(date +%F).tar.gz -C ./data .
docker start taygedo-rs
```

备份文件里含主密钥，等同于备份了所有账号凭据，**不要进公开仓库**。

### 5.4 恢复

```bash
docker stop taygedo-rs
sudo rm -rf ./data            # 整个目录删掉，隐藏文件（.storage.lock 等）才会一并清掉
mkdir -p ./data
sudo tar -xzf taygedo-backup-2026-11-03.tar.gz -C ./data
sudo chown -R 10001:10001 ./data
docker start taygedo-rs
```

只执行 `rm -rf ./data/*` 是不够的：`.storage.lock` 与 `.storage-migration.json`
是隐藏文件，通配符匹配不到，残留的迁移记录会让恢复后的目录状态自相矛盾。

### 5.5 换机器迁移

整套 `/data` 原样搬过去即可，密钥与数据是一体的。目标机器架构可以不同
（amd64 ↔ arm64），存储格式与架构无关。

---

## 6. 配置项

优先级：**环境变量 > `config.json`**。环境变量在每次启动时覆盖到内存配置，
其中 `TAYGEDO_WEB_PASSWORD` 还会写回存储（见 §7.2）。

### 6.1 容器里生效的变量

| 变量 | 默认值 | 说明 |
| :--- | :--- | :--- |
| `TAYGEDO_LISTEN` | `0.0.0.0:8787` | 监听地址。容器内不建议改端口，健康检查写死 8787 |
| `TAYGEDO_DATA_DIR` | `/data` | 数据目录 |
| `TAYGEDO_WEB_PASSWORD` | 无 | 初始/重置口令。**设置后每次启动都会把口令对齐成它** |
| `TAYGEDO_LAN_CIDRS` | RFC1918 + 回环 + IPv6 ULA | 受信来源白名单，逗号分隔 |
| `TAYGEDO_CRYPTO_POLICY` | `auto` | `auto` / `always` / `never` |
| `TAYGEDO_ALLOWED_ORIGINS` | 无 | 额外放行的跨域来源，逗号分隔 |
| `TAYGEDO_DEFAULT_SCHEDULE` | `06:10` | 默认签到时间（UTC+8） |
| `TAYGEDO_COIN_TASKS` | `true` | 是否做每日硬币任务 |
| `TAYGEDO_CLOUD_DURATION` | `true` | 是否领云游戏时长 |
| `TAYGEDO_SHARE_PLATFORM` | `qq` | 分享任务平台 |

### 6.2 容器里**无效**的变量

| 变量 | 为什么无效 |
| :--- | :--- |
| `TAYGEDO_NO_AUTH` | 仅 `openwrt` 形态编译，容器形态连这个分支都不存在 |
| `TAYGEDO_LAN_NO_AUTH` | 同上，WebUI 形态有登录页，没有"免鉴权"语义 |
| `TAYGEDO_LUCI_SESSION_CHECK` | LuCI 专用 |
| `TAYGEDO_UBUS_BIN` | LuCI 专用 |
| `TAYGEDO_EDITION` | 已废弃，形态是编译期决定的 |

### 6.3 建议在 WebUI 里改的项

调度时间、多账号、推送配置这类频繁调整的东西，直接改 `config.json` 覆盖
容易和界面打架。用界面改完，它会自己落盘到加密的 `config.json`。

---

## 7. 日常运维

### 7.1 看日志

```bash
docker logs -f --tail 200 taygedo-rs
```

启动横幅里有身份指纹、Key ID、数据目录与鉴权方式。**首次启动的随机口令
只打印一次**，错过了就用 §7.2 重置。

### 7.2 改口令 / 忘记口令

在 WebUI 设置页改是最正常的方式。忘了口令就走环境变量：

```bash
docker rm -f taygedo-rs
docker run -d --name taygedo-rs \
  -p 8787:8787 -v "$PWD/data:/data" \
  -e TAYGEDO_WEB_PASSWORD='NewPassw0rd!' \
  ghcr.io/lianxia233/taygedo-rs:latest
```

> **注意**：`TAYGEDO_WEB_PASSWORD` 的优先级高于存储，**每次启动都会把口令
> 对齐成它**。重置完记得把这个 `-e` 去掉，否则以后在界面上改口令，
> 重启又会被覆盖回去。

改口令会吊销所有已签发 token，浏览器里的旧会话立即失效。

### 7.3 轮换存储主密钥

怀疑 `keys/master.key` 泄露、或想定期轮换时：

```bash
docker stop taygedo-rs
docker run --rm \
  -v "$PWD/data:/data" \
  --entrypoint taygedo-rs \
  ghcr.io/lianxia233/taygedo-rs:latest \
  --rotate-storage-key
docker start taygedo-rs
```

必须在容器停止时执行：轮换要拿数据目录的排他锁，运行中的实例会挡住它。
轮换是"重新加密全部文件 + 提升新密钥"，中途断电会在下次启动时自动续做。

### 7.4 升级

```bash
docker compose pull           # 只跑 up -d 不会重新拉取已存在的镜像
docker compose up -d

# 不用 compose 的话：先 pull，再删旧容器重建
docker pull ghcr.io/lianxia233/taygedo-rs:latest
docker rm -f taygedo-rs && docker run -d --name taygedo-rs ...
```

升级前先按 §5.3 备份。存储层会自动做格式迁移（旧明文 → 密文、密钥格式升级），
迁移是逐文件原子操作且幂等。

### 7.5 回滚

新版本可能已经把存储格式升级到旧版本不认识的样子，因此回滚前**务必**确认
有升级前的备份：

```bash
docker rm -f taygedo-rs
docker run -d --name taygedo-rs ... ghcr.io/lianxia233/taygedo-rs:0.8.1
```

如果启动报解密失败或版本未知，别删数据文件 —— 用 §5.4 恢复备份。

### 7.6 进容器

```bash
docker exec -it taygedo-rs sh
```

镜像里只有 `sh`、`wget` 和业务二进制，没有 curl / vim / ps。
要看进程状态用宿主机的 `docker stats` / `docker top`。

---

## 8. 本地构建

### 8.1 单架构

```bash
docker build -t taygedo-rs:local .

# 国内网络让 cargo 走 rsproxy 镜像（默认会清空镜像配置走官方源）
docker build --build-arg CARGO_MIRROR=keep -t taygedo-rs:local .
```

首次构建取决于依赖下载速度：海外网络约 8~10 分钟，国内直连 crates.io 可能
更久，用 `CARGO_MIRROR=keep` 走 rsproxy 通常 3~5 分钟。之后改业务代码只重编
本 crate，约 30 秒。

### 8.2 多架构

```bash
docker buildx create --name tgd --use --bootstrap
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t ghcr.io/<你的用户名>/taygedo-rs:test \
  --push .
```

arm64 走 QEMU 模拟，冷构建约 20 分钟；没有 `--push` 时多平台产物无法落到
本地 docker images，只能导出成 OCI 目录：

```bash
docker buildx build --platform linux/amd64,linux/arm64 \
  -o type=oci,dest=taygedo-oci.tar .
```

### 8.3 构建缓存

Dockerfile 把依赖编译单独放一层（先放一个空 `main` 预热）。只要
`Cargo.toml` / `Cargo.lock` 不变，改 `src/*.rs` 不会重编 ring / tokio。

---

## 9. 故障排查

| 症状 | 原因 | 处理 |
| :--- | :--- | :--- |
| 容器起来就退出，日志有 `[fatal]` | 数据目录不可写、密钥与密文不匹配、或存储版本未知 | 按日志提示处理；权限见 §5.2，密钥问题见 §5.4 |
| 启动报 `创建数据目录失败` / `Permission denied` | 挂载目录属主不是 10001 | `sudo chown -R 10001:10001 ./data` |
| 启动报解密失败 / `KeyIdMismatch` | `keys/master.key` 与数据文件不匹配，或密钥丢失 | 用备份恢复；切勿删除数据文件 |
| 浏览器 403 `untrusted_source` | 来源不在 `lan_cidrs` | 加白名单，或改用反代 + 内网出口 |
| 浏览器 401 | token 过期（7 天）或改过口令 | 重新登录 |
| `docker ps` 显示 `unhealthy` | 健康检查失败：进程没起来或 8787 不通 | `docker logs` 看启动错误 |
| `pull access denied` | GHCR 包被设为私有，或未登录 | `docker login ghcr.io`，或把包设为 Public |
| `exec format error` | 拉到了异构架构镜像 | 确认 `docker image inspect --format '{{.Architecture}}'`，ARM 机器用 arm64 |
| `docker: no matching manifest` | 目标平台不在 manifest 里（如 386 / armv7） | 本镜像只出 amd64 与 arm64 |

---

## 10. 安全建议

1. **不要直接把 8787 暴露到公网。** 需要外网访问就上反代 + TLS，
   应用层加密（v2）作为第二层保留。
2. **钉版本或 digest**，不要让生产环境自动跟 `latest`。
3. `keys/master.key` 与备份文件按密钥对待：不进 Git、不放公开网盘。
4. 定期 `docker pull` 后重建容器，基础镜像的 CVE 修复才会进来。
5. 容器已是非 root，进一步可以加：

   ```bash
   --read-only --tmpfs /tmp --cap-drop=ALL --security-opt no-new-privileges
   ```

   注意 `--read-only` 下 `/data` 必须是挂载卷，程序只需要写 `/data`。
6. 家庭内网自用也建议改掉 `admin` 用户名，并把调度时间错开整点
   （上游接口在整点前后更容易限流）。

---

## 相关文档

- [README](../README.md) —— 项目总览与分平台安装
- [加密存储](encrypted-storage.md) —— `/data` 里那些密文文件是怎么加密的
- [传输协议 v2](security-transport.md) —— `taygedo/v2` 握手与签名清单
