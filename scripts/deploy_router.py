#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
将本地构建产物部署到 ImmortalWrt 路由器（aarch64）。

用法:
  TAYGEDO_ROUTER_PWD='A1187900' python deploy_router.py [host] [user]

默认 host=192.168.88.1, user=root。密码从环境变量 TAYGEDO_ROUTER_PWD 读取。
部署内容:
  - target/aarch64-unknown-linux-musl/release/taygedo-rs -> /usr/bin/taygedo-rs
  - openwrt/luci-app-taygedo/htdocs/luci-static/resources/view/taygedo/status.js -> /www/luci-static/resources/view/taygedo/status.js
  - openwrt/luci-app-taygedo/root/etc/init.d/taygedo -> /etc/init.d/taygedo
随后重启 taygedo 服务并清理 LuCI 缓存。
"""
import os
import sys
import paramiko

HOST = sys.argv[1] if len(sys.argv) > 1 else "192.168.88.1"
USER = sys.argv[2] if len(sys.argv) > 2 else "root"
PASSWORD = os.environ.get("TAYGEDO_ROUTER_PWD", "")
if not PASSWORD:
    PASSWORD = input("Router root password: ")

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

BIN_LOCAL = os.path.join(REPO, "target", "aarch64-unknown-linux-musl", "release", "taygedo-rs")
STATUS_LOCAL = os.path.join(REPO, "openwrt", "luci-app-taygedo", "htdocs",
                            "luci-static", "resources", "view", "taygedo", "status.js")
INITD_LOCAL = os.path.join(REPO, "openwrt", "luci-app-taygedo", "root", "etc", "init.d", "taygedo")

BIN_REMOTE = "/usr/bin/taygedo-rs"
STATUS_REMOTE = "/www/luci-static/resources/view/taygedo/status.js"
INITD_REMOTE = "/etc/init.d/taygedo"


def ssh_run(ssh, cmd, check=True):
    print(f"[ssh] {cmd}")
    stdin, stdout, stderr = ssh.exec_command(cmd)
    out = stdout.read().decode(errors="replace")
    err = stderr.read().decode(errors="replace")
    rc = stdout.channel.recv_exit_status()
    if out.strip():
        print(out.rstrip())
    if err.strip():
        print("[stderr]", err.rstrip())
    if check and rc != 0:
        raise RuntimeError(f"命令失败 (rc={rc}): {cmd}")
    return rc


def sftp_put(sftp, local, remote):
    print(f"[scp] {local} -> {remote}")
    sftp.put(local, remote)


def main():
    for p in (BIN_LOCAL, STATUS_LOCAL, INITD_LOCAL):
        if not os.path.exists(p):
            raise SystemExit(f"本地文件不存在: {p}")

    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(HOST, port=22, username=USER, password=PASSWORD, timeout=30,
                   look_for_keys=False, allow_agent=False)
    try:
        sftp = client.open_sftp()
        # 0) 先停服务，避免覆盖正在运行的二进制导致页错误
        ssh_run(client, "/etc/init.d/taygedo stop", check=False)
        # 1) 二进制
        sftp_put(sftp, BIN_LOCAL, BIN_REMOTE)
        ssh_run(client, f"chmod 755 {BIN_REMOTE}")
        # 2) LuCI status.js
        ssh_run(client, f"mkdir -p {os.path.dirname(STATUS_REMOTE)}")
        sftp_put(sftp, STATUS_LOCAL, STATUS_REMOTE)
        # 3) init.d
        sftp_put(sftp, INITD_LOCAL, INITD_REMOTE)
        ssh_run(client, f"chmod 755 {INITD_REMOTE}")
        sftp.close()

        # 4) 重启服务（procd 会重新读取新 init.d 的 TAYGEDO_DISABLE_AUTH=1）
        ssh_run(client, "/etc/init.d/taygedo restart")
        # 5) 清理 LuCI 缓存，确保新 status.js 生效
        ssh_run(client, "rm -f /tmp/luci-* 2>/dev/null; echo cleared", check=False)

        # 6) 验证
        ssh_run(client, "opkg list-installed 2>/dev/null | grep -i taygedo || apk list -I 2>/dev/null | grep -i taygedo || true", check=False)
        ssh_run(client, "ps | grep taygedo-rs | grep -v grep || true", check=False)
        ssh_run(client, "/usr/bin/taygedo-rs --version 2>&1 || true", check=False)
        ssh_run(client, "sleep 1; wget -qO- http://127.0.0.1:8787/api/auth 2>&1 || curl -s http://127.0.0.1:8787/api/auth", check=False)
        print("[done] 部署完成。OpenWrt 端现已免鉴权，访问 :8787 或 LuCI 插件无需登录。")
    finally:
        client.close()


if __name__ == "__main__":
    main()
