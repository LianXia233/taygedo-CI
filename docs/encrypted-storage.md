# 配置文件静态加密（加密存储）

本文描述 taygedo 对**落盘态**敏感数据的加密实现：文件格式、密钥引导、旧版迁移、
备份与恢复、密钥轮换，以及这套方案明确**不**解决的问题。

> 一句话概括：数据目录里除 `keys/` 之外的每一个受管 JSON 都是密文；
> 打开它们需要主密钥；主密钥丢了，密文就永久打不开——没有后门、没有恢复码。

---

## 1. 受管文件

| 文件 | 内容 | 是否受管 |
| :--- | :--- | :---: |
| `accounts.json` | 游戏账号（手机号、密码、token、设备指纹） | ✅ |
| `config.json` | 全局配置（`credential_key`、调度、开关、加密策略、网段白名单） | ✅ |
| `state.json` | 签到幂等状态缓存 | ✅ |
| `keyring.json` | 服务端长期身份私钥（Ed25519 seed） | ✅ |
| `keys/master.key` | 配置加密主密钥 | 否（它就是密钥本身，见 §4） |
| `keys/master.key.dpapi` | Windows DPAPI 包裹后的主密钥 | 否（密文由 OS 保护） |
| `.storage-migration.json` | 迁移日志（不含任何敏感值） | 否 |

**受管文件的判定依据是「逻辑身份」而不是文件名。**
`FileIdentity`（`src/crypto_storage/mod.rs`）枚举了四个逻辑文件，每个都有自己的
AAD 域。把 `accounts.json` 的密文改名成 `config.json` 不会被接受：AAD 里绑定的
身份不同，解密直接 `AuthFailed`。同理，把密文搬到另一个数据目录、或用另一个
主密钥（另一个 Key ID）来解，都会失败。

---

## 2. 密文格式

固定 86 字节头部 + 密文 + 16 字节 Poly1305 tag：

| 偏移 | 长度 | 字段 | 说明 |
| ---: | ---: | :--- | :--- |
| 0 | 8 | Magic | `TGDENV01`（`54 47 44 45 4E 56 30 31`） |
| 8 | 1 | `format_version` | 当前 `1`，未知版本一律拒绝 |
| 9 | 1 | `alg` | `1` = XChaCha20-Poly1305 + HKDF-SHA256 |
| 10 | 16 | `key_id` | 主密钥标识（32 字符十六进制，见 §3） |
| 26 | 32 | `salt` | 每次写文件都重新生成，**不**从密钥派生 |
| 58 | 24 | `nonce` | 每次写文件都重新生成（XChaCha20 的 192-bit nonce） |
| 82 | 4 | `ct_len` | 大端 u32，密文长度（不含 tag） |
| 86 | n | 密文 | |
| 86+n | 16 | tag | Poly1305 认证标签（AEAD 输出尾部） |

硬上限：单文件 `MAX_FILE_BYTES = 8 MiB`。解析器先看 `ct_len` 再判断实际长度是否
**恰好**等于 `86 + ct_len + 16`——截断、追加、零长度密文全部报错，不做"尽力解析"。

### 2.1 密钥派生

```
file_key = HKDF-SHA256(
    ikm  = master_key (32B)
    salt = header.salt (32B, 每次写入随机)
    info = "taygedo/storage/file/v1/" || identity_str
    len  = 32
)
```

- **每次写入都换 salt**：同一账号内容改两次，两次的密文前缀就不同，无法通过比较
  密文判断"内容是否变化"。
- **identity 进 info**：这是跨文件搬移防护的第一层；第二层是 AAD。

### 2.2 AAD（附加认证数据）

```
aad = "taygedo/storage/aad/v1" || 0x00
      || format_version (1B) || alg (1B) || key_id (16B)
      || le_u16(len(identity)) || identity
```

所有变长字段都带 u16 大端长度前缀（`lp()`），避免"两个字段拼起来恰好等于另一组
字段拼接"的歧义。格式版本与算法号也在 AAD 里，防止降级到旧格式解析路径。

因此以下攻击都会在**解密前**失败，而不是解出错误内容：

| 攻击 | 结果 |
| :--- | :--- |
| 换文件（accounts ↔ config） | `AuthFailed`（identity 不同） |
| 换目录（拷到另一台机器） | `KeyIdMismatch` 或 `AuthFailed` |
| 换主密钥 | `KeyIdMismatch` |
| 改 header 任一字节 | `AuthFailed` |
| 改/截断/追加密文 | `AuthFailed` 或 `Corrupt` |
| 未知版本 / 未知算法 | `UnsupportedVersion` / `UnsupportedAlg` |
| 明文冒充密文 | `BadMagic` |

**没有任何一条路径会"认证失败就返回默认配置"。** 这是刻意的：静默返回空配置
在用户侧表现为"账号全没了"，随后一次保存就把真正的密文覆盖掉，属于不可恢复的
数据丢失。启动期失败走 `[fatal]` 退出，运行期读取失败返回 `Err` 并保留原文件。

---

## 3. Key ID

```
key_id = hex( SHA-256("taygedo/storage/key-id/v1" || master_key)[0..8] )
```

32 字符十六进制。作用：

1. **诊断**：日志/`--rotate-storage-key` 输出里只出现 Key ID，不出现密钥。
2. **失配早退**：用错密钥时在解密前就能报 `KeyIdMismatch`，不必等 AEAD 失败。
3. **轮换可见性**：轮换前后 Key ID 不同，能直观确认"新密钥确实生效了"。

Key ID 是密钥的**公开指纹**，不具保密性；但它也不足以反推密钥。

---

## 4. 主密钥保护（三平台）

主密钥 32 字节，是整条信任链的根。各平台获取顺序见
`src/crypto_storage/platform/mod.rs::load_master_key`。

### 4.1 Debian / 通用 Linux

优先级：

1. **systemd 凭据**：若 `$CREDENTIALS_DIRECTORY` 中存在 `taygedo_storage_key`，
   直接使用。配合 `LoadCredentialEncrypted=`（systemd ≥ 250），密钥由 TPM2 或
   `/var/lib/systemd/credential.secret` 封存，**不以可读文件形式存在于文件系统**。
   凭据内容支持 32 字节裸值、64 字符十六进制、base64 三种编码。
2. **独立密钥文件**：`<data_dir>/keys/master.key`，root 属主、`0600`、目录 `0700`。

读密钥文件时的防御（`crypto_storage/atomic.rs::read_secure`）：

- `O_NOFOLLOW` 打开 + `fstat` 二次确认，拒绝符号链接与重解析点（防 TOCTOU）；
- `st_nlink == 1`，拒绝硬链接（防"同一个 inode 被别处读到"）；
- 属主必须是当前 euid；
- 权限必须严格 `0600`（**过宽直接报错，不自动 chmod**——自动修复会把"曾经泄露"
  这件事掩盖掉）；
- 长度上限，防被指向超大文件。

### 4.2 Windows

`keys/master.key.dpapi`，用 `CryptProtectData` 包裹：

- 默认 **用户作用域**（`CRYPTPROTECT_UI_FORBIDDEN`）：同一台机器上别的用户
  无法解开，把数据目录整体拷到另一台机器也解不开。
- 设 `TAYGEDO_DPAPI_MACHINE=1` 切到**机器作用域**（`CRYPTPROTECT_LOCAL_MACHINE`）：
  适合"以服务身份运行、换用户仍要能读"的场景，代价是同机任意用户都可解。

边界必须说清：DPAPI 保护的是"别的用户/别的机器读不到"，
**不防**已在你登录会话里运行的恶意程序——它可以直接调 `CryptUnprotectData`。

### 4.3 OpenWrt

没有 systemd、没有 TPM，也不引入额外依赖：

- `keys/master.key`，`root:root` `0600`，目录 `0700`；
- `init.d/taygedo` 在启动时兜底 `chmod 700` 目录、`chmod 600` 密钥文件
  （overlayfs 上重建/升级可能带上宽松权限）；
- 不依赖 procd 的任何特性，LuCI 正常工作。

也就是说：**OpenWrt 上主密钥的安全强度 = 文件系统权限的强度**。能拿到 root shell
的人就能拿到密钥。这一点在启动日志里会明确写出来（`storage_summary()` 会追加
"文件权限保护：仅限 root 可读，不等价于 TPM/DPAPI"）。

---

## 5. 引导顺序（为什么不会死锁）

密钥必须在读 `config.json` **之前**就绪——但密钥路径又存在"数据目录"这个概念里，
看起来像循环依赖。实际顺序（`Store::new`）：

```
1. create_dir_all(data_dir)
2. 取排他锁 <data_dir>/.lock（fs4，10s 超时）
3. 清理残留临时文件 .tgd-tmp-*
4. scan_has_ciphertext()      ← 只读每个受管文件的前 8 字节 Magic
5. KeyManager::load_or_init(data_dir, has_existing_ciphertext)
6. resume_rotation()          ← 若存在 keys/master.key.pending，续做未完成的轮换
7. migrate_all()              ← 旧明文 → 密文
```

第 4 步是解开循环依赖的关键：**判断"是否已有密文"只需要看 Magic，不需要密钥。**
于是第 5 步可以做出正确决策：

| 有密文 | 有密钥 | 行为 |
| :---: | :---: | :--- |
| 否 | 否 | 首次启动：生成新密钥 → 落盘 → 后续迁移
| 否 | 是 | 正常启动（空数据/仅明文遗留在迁移阶段处理）
| 是 | 是 | 正常启动
| **是** | **否** | **`MasterKeyMissing`，拒绝启动**
| — | 损坏/全零/权限过宽 | `MasterKeyCorrupt`，拒绝启动

最后一行是整套设计里最重要的一条：**绝不在"已有密文但没有密钥"时生成新密钥。**
那样做会得到一个"能启动、但所有数据都解不开"的假正常状态，比直接拒绝启动恶劣得多。

---

## 6. 旧版明文迁移

### 6.1 单文件流程

```
read_secure(path)                    # 符号链接/硬链接/权限检查
  ├─ 不存在         → Absent
  ├─ 已是 Magic     → AlreadyEncrypted：尝试解密校验（解不开 = 致命错误）
  └─ 明文
       ├─ validate_plaintext(id)     # 按文件身份校验 JSON 结构
       │    ├─ 不合法 → SkippedInvalid（**原文件保留，不动**）
       │    └─ 合法
       ├─ 生成备份 <name>.enc-bak-<ts>（**加密**备份，回读校验内容一致）
       ├─ encrypt → 随机临时文件 .tgd-tmp-*（O_EXCL，0600）
       ├─ fsync(文件) → rename(原子替换) → fsync(目录)
       └─ 回读校验：重新读回、解密、与原文逐字节比对
```

原子性靠三个东西叠加：`O_EXCL` 独占创建（防并发写同一临时文件）、`rename`
（同一文件系统内原子替换）、`fsync` 文件 + 目录（防掉电后 rename 丢元数据）。
任何一步失败，**原文件不动**，进程以 `Err` 退出，不覆盖、不删除明文。

### 6.2 结构与幂等

- 每个受管文件的结构校验不同：`accounts` 要求是数组且 `id` 非空、`config` 要求
  `credential_key` 非空、`state` 要求是对象、`keyring` 要求 `secret_bytes` 存在。
  结构不合法说明这文件不是我们认识的东西，**跳过而不是"试着加密一下"**。
- 已经加密的文件在每次启动时都会**重新解一遍**：能解开说明密钥对、密文完好；
  解不开就是致命错误（不是"跳过"）。
- 迁移结果记在 `.storage-migration.json`（journal），记录 version / key_id /
  updated_at / 每文件结果。journal 损坏时按空日志处理——它不承载数据，
  丢了只会让"本次迁移了几个文件"这行统计不准，不影响安全性。

### 6.3 中断恢复

迁移中途崩溃时，磁盘上可能同时存在：旧明文、新密文、临时文件、加密备份。
下次启动按下面的顺序收敛：

1. 清理 `.tgd-tmp-*`（半成品临时文件）；
2. 已加密的文件解密校验通过即视为完成（幂等）；
3. 仍是明文的重新走一遍迁移（备份名带新时间戳，不会踩旧备份）；
4. 备份按 `BACKUP_KEEP = 3` 保留最新三份。

### 6.4 关于"备份也是加密的"

迁移备份名是 `<原名>.enc-bak-<时间戳>`，内容是**密文**。这是刻意的：
明文备份等于把"加密"这件事作废（ attacker 只要读备份即可）。

由此得到一个必须接受的推论：**主密钥丢失时，迁移备份同样解不开。**
备份防的是"迁移过程本身出错"，不是"密钥丢失"。

同时明确：代码**不承诺**删除明文等于物理擦除。`unlink` 只解除目录项，
底层块设备上仍可能留有旧数据（尤其是在 COW 文件系统、SSD 磨损均衡、
快照/备份系统下）。迁移完成后如需确保明文不残留，应走全盘加密或整盘擦除。

---

## 7. 密钥轮换

```bash
taygedo-rs --rotate-storage-key
```

流程（`migration::rotate_key`）：

```
1. 生成新主密钥 → 写 keys/master.key.pending（不覆盖正式密钥）
2. 逐个受管文件：旧密钥解密 → 新密钥加密 → 写 *.rotate-bak-<旧KeyID> → 原子替换
3. 全部成功 → promote_pending_key()：pending 提升为 master.key
4. 打印  主密钥已轮换：<旧 KeyID> -> <新 KeyID>
```

- 步骤 2 中途崩溃：下次启动 `resume_rotation()` 检测到 pending，自动从断点续做
  （已用新密钥加密的文件按 Key ID 跳过）。
- 步骤 3 之前崩溃，磁盘上仍是旧密钥 + 部分新密文：续做逻辑会读 pending 密钥把
  剩下的文件转完，**不会**出现"一半文件解不开"的终态。
- 轮换后 Key ID 变化，能直接从日志确认。

轮换**不**重新生成 `keyring.json` 里的身份私钥——那是传输层身份，换了会让所有
已 pin 指纹的客户端报"身份变更"。要换身份私钥请用显式操作（见
[`security-transport.md`](security-transport.md)），并且必须接受客户端会告警。

---

## 8. 备份与恢复

### 备份

需要备份的是**整个数据目录**，尤其是：

```
<data_dir>/keys/master.key          ← 没有它，其余备份全是废纸
<data_dir>/*.json                   ← 密文
<data_dir>/*.enc-bak-*              ← 迁移备份（也是密文）
```

要求：

- `master.key` 与密文**不要放在同一介质/同一备份桶**。放在一起等于没加密。
- 备份 `master.key` 时保持 `0600`，或直接用 `systemd-creds` / 密码管理器封存。

### 恢复

```
1. 停服务
2. 把数据目录整体还原（含 keys/）
3. 确认权限：chown -R root:root <data_dir>; chmod 700 <data_dir> <data_dir>/keys; chmod 600 <data_dir>/keys/*
4. 启动。日志里 "Key ID" 应与备份时一致
```

### 密钥丢失 / 损坏

| 情况 | 现象 | 处置 |
| :--- | :--- | :--- |
| `master.key` 丢失 | `[fatal] ... MasterKeyMissing` | 从备份还原。**没有任何绕过办法** |
| `master.key` 内容被改 | `MasterKeyCorrupt` 或 `KeyIdMismatch` | 从备份还原 |
| 密文被改（一位） | `AuthFailed`，且原文件不被修改 | 从备份还原该文件 |
| 权限被放宽 | `Platform("权限过于宽松")` | `chmod 600` 后重启（这是唯一"修一下就好"的情况） |
| journal 损坏 | 正常启动，迁移统计不准 | 可删，无副作用 |

**没有恢复后门。** 项目不提供"万能密钥"、不提供"找回码"、不会在检测到密钥
缺失时自动重建一套新密钥（那只会让用户以为数据还在）。唯一出路是备份。

---

## 9. 并发与文件系统安全

| 风险 | 措施 |
| :--- | :--- |
| 两个实例同时写 | `<data_dir>/.lock` 排他锁（`fs4`），第二个实例 10s 超时后 `Locked` 退出 |
| 写到一半掉电 | 临时文件 + `fsync` + `rename` + 目录 `fsync` |
| 临时文件被抢注 | `O_EXCL` + 随机名（`.tgd-tmp-<随机>`） |
| 符号链接指向敏感文件 | `O_NOFOLLOW` + `fstat` 校验 |
| 硬链接泄露 | `st_nlink == 1` 校验 |
| 权限被放宽 | 读时严格校验 `0600`，不自动修复 |
| 写入后没落盘就被读到 | 写入后**回读自校验**（解密 + 逐字节比对） |

---

## 10. 已知限制（务必读完）

1. **不防"已经 root/Admin 的攻击者"**：密钥与密文在同一台机器上，进程能解，
   拿到同等权限的人也能解。本方案的目标是"磁盘/备份/镜像泄露"这一类静态风险。
2. **不防内存取证**：运行期明文在内存中；敏感缓冲用 `Zeroizing` 包裹，
   释放时清零，但无法对抗同机调试器/内存 dump。
3. **不防 swap/hibernation 落盘**：明文可能被换页到磁盘。需要时请开启全盘加密。
4. **OpenWrt 上等于文件权限**：没有 TPM，root 就能读。
5. **`unlink` ≠ 擦除**：见 §6.4。
6. **备份同样是密文，同样依赖同一把密钥**：见 §6.4。
7. **Windows DPAPI 用户作用域绑定用户**：换用户/换机器会解不开，属预期行为。
8. **`--rotate-storage-key` 期间不要并发写**：轮换会拿排他锁，但如果你在轮换
   进行时手工改文件，仍可能出现意料外结果。轮换前请停服务。

---

## 11. 相关代码

| 文件 | 职责 |
| :--- | :--- |
| `src/crypto_storage/mod.rs` | `FileIdentity` / `StorageCrypto::seal,open` / `StorageError` |
| `src/crypto_storage/envelope.rs` | 头部布局、严格解析、AAD 构造 |
| `src/crypto_storage/key_manager.rs` | 主密钥装载、`derive_file_key`、Key ID |
| `src/crypto_storage/atomic.rs` | 原子写、安全读、临时文件清理 |
| `src/crypto_storage/platform/*` | 三平台密钥保护 |
| `src/crypto_storage/migration.rs` | 迁移、备份、轮换、journal |
| `src/store.rs` | 唯一入口：加锁 → 引导 → 迁移 → 读写 |
| `src/crypto_storage/tests.rs` | 格式/篡改/边界测试 |
| `src/store/tests.rs` | 端到端：落盘是密文、迁移、密钥缺失、轮换 |
