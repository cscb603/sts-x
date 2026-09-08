# STS-X 3.3.3 源码包 · Mac 构建手册

送给 Mac 端 AI 的构建手册。Windows 版由合肥这边出，Mac 版请你这边出，两版共用同一份源码（v3.3.3）。

> 本包已内含 v3.3.3 的两处关键改动（相对 3.3.2）：
> 1. **内建空闲 GC**：程序在后台空闲间隙自动回收过期索引版本目录（V8 Idle GC + Postgres autovacuum 思路，见 `src/gc.rs` / `src/cache.rs`）。
> 2. **索引损坏自愈**：打开索引失败时（如被中断的 reindex 写残），自动删除并重建一次，而非把错误甩给用户（见 `src/indexer/mod.rs::open_index_selfheal`）。本次修复正对应此前「索引写坏导致 `Failed to open file ... term`」事故。

---

## 1. 依赖

- **Rust 工具链**：stable，建议 1.97+（与 Windows 端一致）。
- **`core_lib` 本地 path 依赖**（关键）：声明在 `Cargo.toml`：
  ```toml
  core_lib = { path = "../rust_master_workspace/libs/core_lib", default-features = false, features = ["path", "mcp"] }
  ```
  → Mac 上需把 `core_lib` 仓库放到 sts-x 同级的 `rust_master_workspace/libs/core_lib`，
    或把这一行的 `path` 改成你 Mac 上的实际路径（绝对/相对均可）。
  `core_lib` 不是 crates.io 上的包，需单独获取（与 sts-x 同属星TAP实验室仓库组），
  它提供跨平台缓存目录、MCP 运行时等基础设施。
- **（可选）语义检索**：`--features semantic` 需要 ONNX Runtime 动态库。
  macOS 上 `src/main.rs::bootstrap_ort_dylib()` 会自动探测
  `/usr/local/lib` 与 `/opt/homebrew/lib` 下的 `libonnxruntime.dylib`。
  不编语义特性则无需此库，纯 BM25 开箱即用。

---

## 2. 构建

### 默认（BM25 版，对应线上 `STX_SEMANTIC=0`，推荐先出这个）
```bash
cd sts-x
cargo build --release
# 产物: target/release/sts-x   (macOS)
```

### 含语义检索（可选）
```bash
# 先把 onnxruntime 动态库放到 /opt/homebrew/lib 或 /usr/local/lib
cargo build --release --features semantic
```

> 发布前建议 `cargo clippy --all-targets` 零警告；`[profile.release]` 已开 `lto=true` + `codegen-units=1`。

---

## 3. 验证（务必跑，别只编不验）

1. **版本号**
   ```bash
   ./target/release/sts-x --version   # 应含 3.3.3
   ```
2. **基本搜索**（在任一 Rust/Node 项目目录）
   ```bash
   ./target/release/sts-x search "你项目里的某个函数名" --locate
   ./target/release/sts-x search "某个中文描述"        # 走 0 命中自动重试链
   ```
3. **空闲 GC 验证**（制造旧版本索引，确认被回收）
   ```bash
   # 当前 INDEX_VERSION 为 "v4"（见 src/cache.rs 顶部注释）
   ls ~/Library/Caches/sts-x/
   # 复制某个 v4 目录为 v3（更低的版本号），再跑一次搜索 / 等空闲（IDLE_GRACE_SECS=60s），
   # 应只剩 v4
   ```
4. **损坏自愈验证（重要，本次新增）**
   ```bash
   # 找到索引目录 ~/Library/Caches/sts-x/v4/<hash>/tantivy
   rm -f ~/Library/Caches/sts-x/v4/<hash>/tantivy/meta.json   # 故意损坏
   ./target/release/sts-x search "任意词" --locate
   # 期望看到: [sts-x] Recovering a corrupted index, rebuilding ...  然后正常出结果
   ```

---

## 4. 与 Windows 版差异

| 项 | macOS | Windows |
|---|---|---|
| 缓存目录 | `~/Library/Caches/sts-x/` | `%LOCALAPPDATA%\sts-x\cache\` |
| 语义库探测 | `/usr/local/lib`, `/opt/homebrew/lib` | exe 同级 `lib/`、系统路径 |
| 索引格式 | tantivy 0.22 + 版本号 `v4`（两平台通用） | 同左 |

其余逻辑完全一致。

---

## 5. 注意

- **不要降级 `INDEX_VERSION`**；只有升级需要 bump（见 `src/cache.rs` 顶部 v3/v4 注释）。
- 发布二进制时把 `core_lib` 一起带（它是 path 依赖，已编进单二进制，无需额外分发）。
- 南京服务器若只是托管网站，用不上本程序；仅当服务器要跑 AI agent 做代码检索时才需 Linux 原生编译。

---

## 6. 本次打包已修复的两个构建坑（Mac 端不会遇到，但 Windows 端重要）

1. **`H:\存档\星 TAP 软件\STS-X-3.3.2-源码.zip` 漏文件**：该 zip 缺 `src/globsearch.rs`（`lib.rs` 却 `pub mod globsearch;`），直接从它解包会编译报
   `error[E0583]: file not found for module globsearch`。**我交付给你的 `sts-x-3.3.3-src` 已经把 globsearch 补齐，无需你再找。**
2. **`winresource` 找不到 `rc.exe`**：Windows 上 `build.rs` 用 `winresource` 嵌图标，它靠 `RC_PATH` 环境变量定位 SDK 的 `rc.exe`；
   若缺失会退化成相对路径 `bin\x64\rc.exe` 而失败。3.3.3 的 `build.rs` 已改为**自动探测** `C:\Program Files (x86)\Windows Kits\10\bin\*\x64\rc.exe`
   并写入 `RC_PATH`，Windows 构建开箱即编（Mac 原生构建整段跳过，无影响）。

---

## 7. 源码包结构（给你带去 Mac 的）

```
sts-x-3.3.3-src/
├── sts-x/                       # 主程序（含 globsearch、gc.rs、selfheal 等全部改动）
└── rust_master_workspace/
    └── libs/
        └── core_lib/            # path 依赖，放到 sts-x 同级的 ../rust_master_workspace/libs/core_lib
```
解包后 `cd sts-x && cargo build --release` 即可（Mac 不需要 rc.exe）。
