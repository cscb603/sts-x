# STS-X v3.3.0 发布说明

> 发布日期：2026-08-24 ｜ 定位：中文搜索「抽风」根治版 —— P0-1 自动重试链 + P0-2 中文语义检索

## 新特性

### 🎯 P0-1：0 命中自动重试链（纯本地，零依赖）
中文查询 0 命中时**自动**按固定顺序重试，单次调用即出最佳结果，AI 无需手动换词：
1. **英文同义词展开**（本地中英代码词典 ~200 条：缓存→cache、索引→index、提示→hint…）
2. **符号猜测**（去停用词后抽 ASCII 标识符，如 "hint"）
3. **file 内容兜底**（文件名/内容子串，rg 后端）

任一阶段命中即返回（stderr 日志标注 `auto-retry(...)`），全部不命中才甩 `_ai_instructions`/`hint` 自救文本（兼容旧客户端）。

### 🧠 P0-2：中文语义检索（可选开启，治本跨语言）
- 开关：`STX_SEMANTIC=1` 环境变量 或 `--semantic` flag（`search`/`ai`/`index`）
- 内置 **bge-small-zh-v1.5** 中文 embedding 模型（512 维）：中文描述 → 英文代码语义直达
  （「缓存目录在哪里」→ `cache.rs cache_root`）
- embedding 持久化进索引（跨进程可用）；维度从模型输出 **自适应校准**（en=384/zh=512/任意）
- 模型/dylib 缺失 → 优雅降级到 BM25 + P0-1 重试链，**不崩溃**
- 默认构建不受影响（semantic 不进 default，主包体积不变）

## 修复
- 0 命中不再只甩引导文本（消除中文弱时多往返死循环）
- embedding 维度硬编码 384 → 自适应（zh 模型 512 维错位取数 bug）
- ort 动态库缺失 panic → 预检降级
- 语义请求但索引无 embedding → 自动强制重建

## 下载

| 包 | 内容 | 体积 |
|---|---|---|
| `STS-X-3.3.0-Mac版(给AI搜代码).zip` | 默认版单文件，零依赖 | 6.1MB |
| `STS-X-3.3.0-Mac版-语义检索(给AI搜代码).zip` | + dylib + 中文模型 + 启动器 | 29.6MB |
| `STS-X-3.3.0-Win版(给AI搜代码).zip` | 默认版单 exe（crt-static，Win10/11 直接跑） | 6.4MB |
| `STS-X-3.3.0-Win版-语义检索(给AI搜代码).zip` | + dll + 中文模型 + bat 启动器 | 27.6MB |
| `STS-X-3.3.0-源码.zip` | 完整源码 + 编译指引 | 8.1MB |

每个包内含 `首次打开必看-sts-x.txt`（部署/被拦解决/语义开关说明）。

## 快速开始

```bash
# 默认版（零配置）
sts-x ai "缓存" -p <项目>
sts-x search "McpServer" -p <项目> --locate
sts-x mcp -p <项目根>          # MCP stdio（配 WorkBuddy/Claude Desktop）

# 语义版（中文 NL → 英文代码语义直达）
export STX_SEMANTIC=1
# Mac: 用 sts-x.sh 启动（自动定位 dylib）；Win: 双击 sts-x-semantic.bat
sts-x ai "缓存目录在哪里" -p <项目>
```

## 系统要求
- macOS 12+（Apple Silicon / Intel）
- Windows 10 / 11 64 位（默认版单 exe 免安装；语义版需 lib/onnxruntime.dll 随包，已内置）

## 构建（开发者）
```bash
cargo build --release                              # 默认版
cargo build --release --features semantic          # 语义版（先 ./scripts/download-models.sh）
# Win 交叉编译（Mac 上）：
RUSTC_WRAPPER= SDKROOT= cargo xwin build --release --target x86_64-pc-windows-msvc [--features semantic]
# 门禁：
cargo fmt && cargo test --all-targets --features semantic && cargo clippy --all-targets --features semantic -- -D warnings
```

## 已知边界
- 语义检索需要模型 + onnxruntime 动态库（随包提供）；两者缺失自动降级 BM25，功能不缺失。
- MCP 方式 B/C 为 legacy，推荐方式 A（原生 stdio `sts-x mcp`）。
