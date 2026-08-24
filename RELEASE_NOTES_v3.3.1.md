# STS-X v3.3.1 发布说明

> 发布日期：2026-08-24 ｜ v3.3.0 的体验收尾版：onnxruntime 动态库**自动探测**，语义检索零配置

## 变更

- **onnxruntime 动态库自动探测（核心改进）**：启动时自动查找 `ORT_DYLIB_PATH` 候选路径并注入，语义检索（`STX_SEMANTIC=1`）**开箱即用，用户零配置**：
  1. 用户显式设置 `ORT_DYLIB_PATH` → 优先尊重
  2. 可执行文件同目录 `lib/` 或同目录（Mac 语义包的 `lib/libonnxruntime.1.28.0.dylib`、Win 语义包的 `lib/onnxruntime.dll`）
  3. macOS 系统路径（`/usr/local/lib`、`/opt/homebrew/lib`）
- 修复场景：macOS dyld 不搜 `/usr/local/lib`（Homebrew 路径）导致 dlopen 失败；Windows LoadLibrary 不搜 `lib/` 子目录。
- 现在**Mac / Win 语义包解压后直接 `./sts-x` / `sts-x.exe` 即可**，不再需要 `sts-x.sh` / `.bat` / 手动 export（启动器保留为兜底）。

## 下载（与 v3.3.0 同名结构）

| 包 | 体积 |
|---|---|
| `STS-X-3.3.1-Mac版(给AI搜代码).zip` | 6.1MB |
| `STS-X-3.3.1-Mac版-语义检索(给AI搜代码).zip` | 29.6MB |
| `STS-X-3.3.1-Win版(给AI搜代码).zip` | 6.4MB |
| `STS-X-3.3.1-Win版-语义检索(给AI搜代码).zip` | 27.6MB |
| `STS-X-3.3.1-源码.zip` | 8.1MB |

## 快速开始（语义版，两端都零配置）

```bash
# Mac：解压后直接
./sts-x --version          # 3.3.1
STX_SEMANTIC=1 ./sts-x ai "缓存目录在哪里" -p <项目>

# Win：解压后
sts-x.exe --version
set STX_SEMANTIC=1
sts-x.exe ai "缓存目录在哪里" -p <项目>
```

## 构建门禁
v3.3.1 全链绿（fmt + 41 tests + clippy -D warnings + Mac/Win × default/semantic 四产物）。
