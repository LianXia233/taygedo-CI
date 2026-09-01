# 上游归属声明 (Upstream Attribution)

本目录保存本项目所参考的上游项目的版权与许可信息，确保上游的 MIT 归属不因本项目改用 GPL-3.0 而丢失。

| 项目 | 说明 |
| :--- | :--- |
| 上游仓库 | [zzstar101/taygedo-auto-attendance](https://github.com/zzstar101/taygedo-auto-attendance) |
| 实现语言 | TypeScript |
| 许可证 | MIT（见 [`LICENSE-MIT`](./LICENSE-MIT)） |
| 版权 | Copyright (c) 2026 zzstar101 |

## 关系说明

本项目是上游的 **Rust 重写版本**：签到流程、接口约定（API 签名、加密参数、任务链路等）参考并移植自上游实现。

- **上游（MIT）**：宽松许可，允许其代码被并入 GPL 项目，归属声明见本目录。
- **本项目（GPL-3.0）**：新增与改写的代码整体以 GPL-3.0 授权，见仓库根目录 [`LICENSE`](../../LICENSE)。

> 本目录存放上游的许可与归属信息，**不包含上游源代码** —— 上游为 TypeScript 实现，本项目为 Rust 重写，仓库中不存在可逐文件搬移的上游源码文件。
