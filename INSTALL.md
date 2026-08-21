# 安装教程

本文介绍 **塔吉多自动签到 (taygedo-rs)** 在 Windows、Debian、OpenWrt 三个平台上的安装与使用。

- 最新安装包：[GitHub Releases](https://github.com/LianXia233/taygedo-CI/releases)
- 默认 WebUI 账号密码：**`admin / admin`**（登录后请在「设置」中修改）

---

## 目录

- [下载对应平台的安装包](#下载对应平台的安装包)
- [Windows 安装](#windows-安装)
- [Debian 安装](#debian-安装)
- [OpenWrt 安装](#openwrt-安装)
- [通用使用指南](#通用使用指南)
- [常见问题](#常见问题)

---

## 下载对应平台的安装包

| 平台 | 文件 | 说明 |
| --- | --- | --- |
| Windows (64 位) | `taygedo-rs-windows-x86_64.zip` | 单文件可执行程序 |
| Debian / Ubuntu (amd64) | `taygedo-rs_<版本>_amd64.deb` | 含 systemd 服务 |
| OpenWrt (x86_64 软路由) | `luci-app-taygedo_<版本>-1_x86_64.ipk` 或 `.apk` | 含 LuCI + 二进制 |
| OpenWrt (ARM64 路由器) | `luci-app-taygedo_<版本>-1_aarch64_cortex-a53.ipk` 或 `.apk` | 含 LuCI + 二进制 |
| Linux 静态 (musl) | `taygedo-rs-<arch>-unknown-linux-musl.tar.gz` | 其他 Linux/容器通用 |

> OpenWrt 24.10 及以上用 **apk** 包，23.05 及以下用 **ipk** 包（opkg）。

---

## Windows 安装

### 1. 下载解压

在 [Releases](https://github.com/LianXia233/taygedo-CI/releases) 下载 `taygedo-rs-windows-x86_64.zip`，解压得到 `taygedo-rs.exe`。

### 2. 运行

**方式一：双击运行**（最简单）

双击 `taygedo-rs.exe`，会弹出一个控制台窗口并启动服务。首次启动会提示：

```
[taygedo-rs] WebUI 默认登录账号：admin / admin
```

**方式二：命令行运行**（推荐，便于自定义）

```powershell
# 进入解压目录后运行
.\taygedo-rs.exe
```

可选环境变量：

```powershell
# 指定监听端口（默认 8787）
$env:TAYGEDO_LISTEN = "0.0.0.0:8787"
# 指定数据目录（默认 .\data）
$env:TAYGEDO_DATA_DIR = "D:\taygedo-data"
# 指定初始登录密码（默认 admin）
$env:TAYGEDO_WEB_PASSWORD = "你的密码"
.\taygedo-rs.exe
```

### 3. 访问 WebUI

浏览器打开 `http://127.0.0.1:8787`，用 `admin / admin` 登录。

### 4. 开机自启（可选）

**方法一：任务计划程序**

1. 打开「任务计划程序」→「创建任务」。
2. 触发器：`登录时` 或 `系统启动时`。
3. 操作：程序填 `taygedo-rs.exe` 完整路径，起始于填解压目录。
4. 勾选「使用最高权限运行」。

**方法二：启动文件夹**

把 `taygedo-rs.exe` 的快捷方式放入 `Win + R` → `shell:startup` 打开的目录。

### 5. 防火墙放行（局域网访问用）

若需局域网其他设备访问，需在 Windows 防火墙放行 8787 端口：

```powershell
netsh advfirewall firewall add rule name="taygedo" dir=in action=allow protocol=TCP localport=8787
```

---

## Debian 安装

### 1. 下载安装

```bash
# 下载（把版本号换成实际的，例如 0.2.3）
wget https://github.com/LianXia233/taygedo-CI/releases/download/v0.2.3/taygedo-rs_0.2.3_amd64.deb

# 安装
sudo dpkg -i taygedo-rs_0.2.3_amd64.deb

# 若有依赖缺失，修复（本项目基本无依赖，通常不需要）
sudo apt-get install -f
```

### 2. 启动并设置开机自启

```bash
sudo systemctl enable --now taygedo-rs
```

查看状态：

```bash
sudo systemctl status taygedo-rs
```

### 3. 自定义配置

编辑服务文件：

```bash
sudo systemctl edit taygedo-rs
```

在弹出内容中覆盖环境变量，例如：

```ini
[Service]
Environment=TAYGEDO_LISTEN=0.0.0.0:8787
Environment=TAYGEDO_DATA_DIR=/var/lib/taygedo
Environment=TAYGEDO_WEB_PASSWORD=你的初始密码
```

保存后重载：

```bash
sudo systemctl daemon-reload
sudo systemctl restart taygedo-rs
```

### 4. 访问 WebUI

浏览器打开 `http://<服务器IP>:8787`，用 `admin / admin` 登录。

### 5. 查看日志

```bash
sudo journalctl -u taygedo-rs -f
```

### 6. 数据目录

数据默认存储在 `/var/lib/taygedo`：
- `accounts.json`：账号列表
- `config.json`：全局配置（含密码哈希）
- `state.json`：每日签到状态

---

## OpenWrt 安装

### 1. 确定包格式与架构

| OpenWrt 版本 | 包管理器 | 文件 |
| --- | --- | --- |
| 24.10 及以上 | `apk` | `*.apk` |
| 23.05 及以下 | `opkg` | `*.ipk` |

架构常见对应：

| 设备/平台 | 架构 | 文件名 |
| --- | --- | --- |
| x86_64 软路由 / 虚拟机 | x86_64 | `..._x86_64.ipk/apk` |
| 常见 ARM64 路由（如部分 MT7986/Rockchip） | aarch64_cortex-a53 | `..._aarch64_cortex-a53.ipk/apk` |

### 2. 安装

**opkg（23.05 及以下）：**

```sh
# 先把 ipk 传到路由器，例如 /tmp/
opkg install /tmp/luci-app-taygedo_0.2.3-1_x86_64.ipk
```

**apk（24.10 及以上）：**

```sh
apk add /tmp/luci-app-taygedo_0.2.3-r1_x86_64.apk
```

### 3. LuCI 界面配置

1. 浏览器打开路由器的 LuCI（通常 `http://192.168.1.1`）。
2. 菜单进入 **服务 → 塔吉多签到**。
3. 在「配置」页：
   - **启用服务**：勾选。
   - **监听端口**：默认 8787。
   - **数据目录**：默认 `/etc/taygedo`。
   - **Web 登录密码**：留空用默认 `admin/admin`，或设置自己的。
   - **默认签到时间**：如 `06:10`。
   - **金币任务 / 云异环时长 / 分享平台**：按需。
4. 「保存并应用」，服务会自动重启。

### 4. 打开 WebUI

- LuCI 菜单点「**打开 WebUI**」，或
- 浏览器访问 `http://<路由器IP>:8787`，用 `admin / admin` 登录。

### 5. 命令行管理（可选）

```sh
# 查看/修改 UCI 配置
uci show taygedo
uci set taygedo.main.enabled=1
uci set taygedo.main.port=8787
uci set taygedo.main.default_schedule=06:10
uci commit taygedo

# 服务管理
/etc/init.d/taygedo start
/etc/init.d/taygedo stop
/etc/init.d/taygedo restart
/etc/init.d/taygedo status

# 查看日志
logread | grep taygedo
```

---

## 通用使用指南

安装并启动后，打开 WebUI 即可开始使用。

### 登录

默认账号密码 `admin / admin`。**强烈建议首次登录后立即修改**：进入「设置」→「修改登录账号密码」。

### 添加账号

点「添加账号」，支持两种方式：

- **密码登录**：输入手机号 + 密码。
- **验证码登录**：输入手机号 → 点「发送验证码」→ 输入短信验证码。

> 添加成功后会自动获取游戏角色（幻塔 / 异环等）。

### 设置每日签到时间

- 全局默认：`设置` → `默认签到时间`。
- 单账号：账号卡片上的「每日签到时间」直接修改。

### 手动签到

账号卡片点「立即签到」可手动触发一次。

### 多账号

重复「添加账号」即可，每个账号独立登录态与签到时间。

---

## 常见问题

**Q：访问 WebUI 提示无法连接？**
检查服务是否运行、端口是否被防火墙/安全组拦截、监听地址是否为 `0.0.0.0`。

**Q：忘记 WebUI 密码？**
删除数据目录下的 `config.json`（或其中的 `web_username`/`web_password_hash`/`web_password_salt` 字段）后重启，会恢复默认 `admin/admin`。

**Q：签到失败 / 登录态失效？**
在 WebUI 中删除该账号并重新登录即可。

**Q：OpenWrt 上如何更新？**
下载新版本对应架构的包，用 `opkg install` / `apk add` 覆盖安装，或在 LuCI 里重新上传安装。

**Q：手机能访问吗？**
能，WebUI 已适配手机浏览器，登录后即可管理。
