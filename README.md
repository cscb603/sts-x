<div align="center">
  <img src="assets/icon.png" width="80" alt="STS-X logo">
  <h1 align="center">STS-X</h1>
  <p align="center"><strong>AI 代码搜索引擎</strong><br>
  AST 感知切块 + BM25 全文搜索 + MCP 服务协议<br>
  17MB 单二进制，零外部依赖，开箱即用</p>
  <p>
    <a href="#特性">特性</a> ·
    <a href="#快速开始">快速开始</a> ·
    <a href="#搜索模式">搜索模式</a> ·
    <a href="#mcp-服务">MCP 服务</a> ·
    <a href="#ai-原生设计">AI 原生设计</a> ·
    <a href="#构建">构建</a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/macOS-支持-brightgreen?logo=apple">
    <img src="https://img.shields.io/badge/Windows-支持-blue?logo=windows">
    <img src="https://img.shields.io/badge/Linux-计划中-lightgrey?logo=linux">
    <img src="https://img.shields.io/badge/license-MIT-blue">
    <img src="https://img.shields.io/badge/大小-17MB-blue">
    <img src="https://img.shields.io/badge/版本-v0.2.0-blue">
  </p>
</div>

---

STS-X 是一个专为 **AI Agent** 设计的代码搜索引擎。与 IDE 内置搜索不同，STS-X 默认输出 JSON、提供 MCP HTTP 服务、基于 AST 语法树切块返回完整的函数/类代码块。

## 特性

- **🧩 AST 感知切块** — 基于 tree-sitter 解析语法树，按函数、类、方法返回完整代码块
- **⚡ 极速 BM25** — Tantivy 倒排索引引擎，搜索 0–2ms 响应
- **🔌 MCP 协议服务** — 内置 HTTP 服务器，支持工具发现（GET /tools）和服务自文档（GET /）
- **📦 零依赖** — 17MB 单二进制，无需 Python、Node.js、数据库
- **🔍 三种搜索模式** — Code（代码语义）、Filename（文件名）、All（全文件）
- **🧠 AI 原生输出** — 默认 JSON 格式，每个响应自带 `_ai_instructions` 使用指南，AI 首次接触即完全掌握用法
- **🔄 自动索引管理** — 索引自动存入系统缓存，无需手动指定目录；文件变更自动检测并重建
- **📍 项目根自动探测** — 自动向上查找 `.git`/`Cargo.toml`/`package.json` 等标记文件
- **🎯 高亮匹配行** — 每个代码块精确标注查询词所在行号，AI 可直接定位
- **🚀 毫秒级索引** — 千级代码文件 <1 秒完成索引

## 快速开始

### 下载

从 [Releases](https://github.com/cscb603/sts-x/releases) 下载最新版：

```bash
# macOS 版
wget https://github.com/cscb603/sts-x/releases/latest/download/sts-x-macos
chmod +x sts-x-macos
sudo cp sts-x-macos /usr/local/bin/sts-x
```

```powershell
# Windows 版（PowerShell）
curl -L https://github.com/cscb603/sts-x/releases/latest/download/sts-x.exe -o sts-x.exe
# 或直接下载 exe 文件放入任意目录
```

### 搜索（自动索引）

STS-X 无需手动索引。在任意项目目录下直接搜索，自动构建索引并缓存：

```bash
cd /你的项目目录

# 搜索代码（默认 JSON 输出，适合 AI 消费）
sts-x search "token verification"

# 搜索文件名
sts-x search "config" -f

# 搜索所有文件内容
sts-x search "TODO" --all

# 人类友好模式
sts-x search "token verification" -H
```

### 手动索引（可选）

```bash
sts-x index .
```

### JSON 输出示例

```json
{
  "query": "token verification",
  "results": [
    {
      "score": 0.97,
      "path": "src/auth/jwt.rs",
      "lines": [15, 42],
      "highlight_lines": [18, 25],
      "kind": "Function",
      "name": "verify_token",
      "signature": "pub fn verify_token(token: &str, secret: &[u8]) -> Result<Claims>",
      "language": "rust",
      "code": "pub fn verify_token(token: &str, secret: &[u8]) -> Result<Claims> {\n    ...\n}"
    }
  ],
  "total_hits": 5,
  "search_time_ms": 1,
  "_ai_instructions": "STS-X 使用指南..."
}
```

## 搜索模式

| 模式 | 命令 | 说明 |
|------|------|------|
| Code | `search "query"` | 默认。搜代码内容，AST 切块返回 |
| Filename | `search "query" -f` | 搜文件名，忽略 .gitignore |
| All | `search "query" --all` | 搜所有文件（代码 + 文本 + 配置） |

## MCP 服务

启动 MCP HTTP 服务，供 AI Agent 调用：

```bash
sts-x serve
# 自动探测当前目录的项目根，监听 http://127.0.0.1:9876
```

### 端点一览

| 方法 | 路径 | 说明 |
|------|------|------|
| `GET` | `/` | 服务文档 + curl 使用示例 |
| `GET` | `/health` | 健康检查 |
| `GET` | `/tools` | MCP 标准工具发现（AI Agent 自动发现搜索能力） |
| `POST` | `/search` | 执行搜索 |

### AI Agent 调用

```bash
# 搜索代码
curl -X POST http://127.0.0.1:9876/search \
  -H "Content-Type: application/json" \
  -d '{"query":"token verification","top_k":3}'

# 搜索文件名
curl -X POST http://127.0.0.1:9876/search \
  -H "Content-Type: application/json" \
  -d '{"query":"config","filename":true,"top_k":5}'

# 指定项目路径
curl -X POST http://127.0.0.1:9876/search \
  -H "Content-Type: application/json" \
  -d '{"query":"error handling","path":"/path/to/project"}'
```

### 工具发现（AI 自学习）

```bash
# AI Agent 可以通过 GET /tools 自动发现搜索能力
curl http://127.0.0.1:9876/tools
# 返回标准 MCP Tool Schema，包含参数描述和默认值
```

## AI 原生设计

STS-X 从内核设计即面向 AI：

### 1. 自说明响应

每个 JSON 搜索响应自动包含 `_ai_instructions` 字段，完整描述 STS-X 的用法、参数说明和 MCP 端点。AI Agent 只需一次调用即可学会全部用法。

### 2. 智能默认值

| 参数 | v0.2.0 默认值 | 说明 |
|------|---------------|------|
| `top_k` | 3 | 返回最相关结果，节省 token |
| `context_lines` | 5 | 匹配行上下文的行数，足够理解代码结构 |
| 输出格式 | JSON | 结构化数据，AI 可直接消费 |

### 3. 代码后处理

每个搜索结果经过后处理：
- **高亮行标注**：`highlight_lines` 字段精确标注查询词所在行号
- **上下文控制**：通过 `--context N` 控制返回代码块的上下文行数

### 4. 索引管理自动化

- 索引自动存储在系统缓存目录（macOS: `~/Library/Caches/sts-x/`，Windows: `%LOCALAPPDATA%/sts-x/cache/`），不污染项目目录
- 文件发生变更时自动检测并重建索引
- 项目根自动向上探测（`.git`/`Cargo.toml`/`package.json`/`go.mod`/`pom.xml` 等 13 种标记）

### 5. Windows 路径兼容

内建 POSIX 路径归一化，git bash 传入的中文路径（如 `/d/项目`）自动转为 Windows 原生格式（`D:\项目`），跨平台无缝使用。

## 技术规格

| 项目 | 详情 |
|------|------|
| 版本 | v0.2.0 |
| 二进制大小 | 17MB |
| 搜索延迟 | 0–2ms |
| 默认输出格式 | JSON |
| 搜索模式 | Code · Filename · All |
| 索引引擎 | Tantivy BM25 |
| AST 解析 | tree-sitter（9 种语言） |
| MCP 服务 | axum HTTP (RESTful) |
| 外部依赖 | 零 |
| 支持语言 | Rust · Python · JavaScript · TypeScript · Java · C · C++ · Go |
| 可选增强 | ONNX embedding + BGE Reranker（需 `--features semantic`） |
| 平台 | macOS ARM64 · Windows x86_64 |
| 许可证 | MIT |

## 构建

```bash
# 默认构建（BM25 only，~17MB）
cargo build --release

# 带语义搜索（ONNX embedding + reranker，~30MB）
cargo build --release --features semantic

# macOS .app 包
bash scripts/build.sh mac

# Windows 交叉编译（需 mingw-w64）
bash scripts/build.sh windows
```

## 项目结构

```
sts-x/
├── src/
│   ├── main.rs          # 入口
│   ├── lib.rs           # 库入口
│   ├── cli/             # 命令行接口
│   ├── indexer/         # tantivy 索引引擎
│   ├── search/          # 搜索管线
│   ├── chunker/         # tree-sitter AST 切块
│   ├── embed/           # ONNX embedding（可选）
│   ├── server/          # MCP HTTP 服务
│   ├── postprocess.rs   # 代码后处理（高亮行、上下文控制）
│   ├── cache.rs         # 跨平台缓存目录管理
│   └── types/           # 数据结构 + AI 输出格式
├── assets/              # 图标资源
├── scripts/             # 构建脚本
└── index.html           # 宣传页
```

## 许可证

MIT License. Copyright © 2026 x tap.
