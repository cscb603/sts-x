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
    <a href="#构建">构建</a>
  </p>
  <p>
    <img src="https://img.shields.io/badge/macOS-支持-brightgreen?logo=apple">
    <img src="https://img.shields.io/badge/Windows-开发中-yellow?logo=windows">
    <img src="https://img.shields.io/badge/Linux-计划中-lightgrey?logo=linux">
    <img src="https://img.shields.io/badge/license-MIT-blue">
    <img src="https://img.shields.io/badge/大小-17MB-blue">
  </p>
</div>

---

STS-X 是一个专为 **AI Agent** 设计的代码搜索引擎。与 IDE 内置搜索不同，STS-X 默认输出 JSON、提供 MCP HTTP 服务、基于 AST 语法树切块返回完整的函数/类代码块。

## 特性

- **🧩 AST 感知切块** — 基于 tree-sitter 解析语法树，按函数、类、方法返回完整代码块
- **⚡ 极速 BM25** — Tantivy 倒排索引引擎，搜索 0–2ms 响应
- **🔌 MCP 协议服务** — 内置 HTTP 服务器，AI Agent 可直接 POST /search 调用
- **📦 零依赖** — 17MB 单二进制，无需 Python、Node.js、数据库
- **🔍 三种搜索模式** — Code（代码语义）、Filename（文件名）、All（全文件）
- **🧠 AI 原生输出** — 默认 JSON 格式，含分数、路径、签名、完整代码块
- **🚀 毫秒级索引** — 千级代码文件 <1 秒完成索引

## 快速开始

### 下载

从 [Releases](https://github.com/xtap/sts-x/releases) 下载最新版：

```bash
# macOS 版
wget https://github.com/xtap/sts-x/releases/latest/download/sts-x-macos
chmod +x sts-x-macos
sudo cp sts-x-macos /usr/local/bin/sts-x
```

### 索引项目

```bash
cd /你的项目目录
sts-x index .
```

### 搜索

```bash
# 搜索代码（默认 JSON 输出，适合 AI 消费）
sts-x search "token verification"

# 搜索文件名
sts-x search "config" -f

# 搜索所有文件内容
sts-x search "TODO" --all

# 人类友好模式
sts-x search "token verification" -H
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
      "kind": "Function",
      "name": "verify_token",
      "signature": "pub fn verify_token(token: &str, secret: &[u8]) -> Result<Claims>",
      "language": "rust",
      "code": "pub fn verify_token(token: &str, secret: &[u8]) -> Result<Claims> {\n    ...\n}"
    }
  ],
  "total_hits": 5,
  "search_time_ms": 1
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
sts-x serve --path /你的项目
# 监听 http://127.0.0.1:9876
```

AI Agent 通过 POST /search 调用：

```bash
curl -X POST http://127.0.0.1:9876/search \
  -H "Content-Type: application/json" \
  -d '{"query":"token verification","top_k":5}'
```

### 自定义索引目录

```bash
sts-x index . -o /tmp/my-index
sts-x search "query" -o /tmp/my-index
sts-x status -o /tmp/my-index
sts-x serve --path . -o /tmp/my-index
```

## 技术规格

| 项目 | 详情 |
|------|------|
| 二进制大小 | 17MB |
| 搜索延迟 | 0–2ms |
| 默认输出格式 | JSON |
| 搜索模式 | Code · Filename · All |
| 索引引擎 | Tantivy BM25 |
| AST 解析 | tree-sitter（8 种语言） |
| MCP 服务 | axum HTTP (POST /search) |
| 外部依赖 | 零 |
| 支持语言 | Rust · Python · JavaScript · TypeScript · Java · C · C++ · Go |
| 可选增强 | ONNX embedding + BGE Reranker（需 `--features semantic`） |
| 平台 | macOS · Windows · Linux |
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
│   ├── cli/             # 命令行接口
│   ├── indexer/         # tantivy 索引引擎
│   ├── search/          # 搜索管线
│   ├── chunker/         # tree-sitter AST 切块
│   ├── embed/           # ONNX embedding（可选）
│   ├── server/          # MCP HTTP 服务
│   └── types/           # 数据结构
├── assets/              # 图标资源
├── scripts/             # 构建脚本
└── index.html           # 宣传页
```

## 许可证

MIT License. Copyright © 2026 x tap.
