# AGENTS.md — STS-X AI 接口契约（v3.2.0 / v5.1 毕业版）

> 本文件是给 **AI Agent / 开发者** 看的接口契约。人类小白看 `首次打开必看.txt`。

## 一、这是什么

STS-X = 给 AI Agent 用的轻量代码+文件搜索引擎。Rust 单二进制、零运行时依赖、CLI + MCP 双入口。

- 代码搜索：AST 切块 + Tantivy BM25，中文注释代码可用
- 文件搜索：任意目录零索引（rg 后端）
- 输出契约：locate（行级，~200 tok）/ expand（整块，可控预算）
- 0 命中自救：响应含双语言引导（换英文词 → 换符号 → 换文件名）

## 二、快速部署（AI 可直接执行）

```bash
# macOS（假设二进制已就位）
sudo cp ./sts-x /usr/local/bin/sts-x
# 验证
sts-x --version        # 3.2.0
sts-x ai "缓存" -p <项目>   # 中文查询应命中 cache.rs 类文件

# Windows（x86_64）
# 解压 sts-x.exe 到任意目录，加入 PATH 或直接调用
sts-x.exe ai "缓存" -p <项目>
```

MCP 服务（AI 客户端接入）——三种方式：

**方式 A（推荐，原生 stdio MCP，零依赖）**：单二进制直接说 MCP 协议，无 Python、无端口、无额外进程：

```json
{
  "mcpServers": {
    "sts-x": {
      "command": "/usr/local/bin/sts-x",   // Win: C:\\tools\\sts-x.exe
      "args": ["mcp", "-p", "/Users/xtap/Documents/AI"],  // -p 默认项目根
      "env": {},
      "disabled": false
    }
  }
}
```
客户端看到的工具：`search` / `file`。每次调用可用 `path` 参数覆盖项目根。

**方式 B（Python bridge，旧版/无原生 mcp 的二进制才用）**：
```json
{
  "mcpServers": {
    "sts-x": {
      "command": "<绝对路径>/scripts/sts-x-mcp-bridge",
      "args": [],
      "env": { "STX_ROOT": "<默认项目根>" },
      "disabled": false
    }
  }
}
```
环境变量：`STX_BIN`（二进制路径）、`STX_PORT`（默认 8765）、`STX_ROOT`（默认项目根）。
⚠️ 需要 Python 3（Win 默认没有）；tools/list 报 `invalid_type` = bridge 旧版（须 ≥ 2026-07-31，`inputSchema` 驼峰）。

**方式 C（HTTP 直连）**：客户端支持 streamable-http 时：
```bash
sts-x serve -p <项目根> --port 9876
# url: http://127.0.0.1:9876；端点：/health /tools /search /file
```
⚠️ sts-x HTTP 端点是自定义 REST（非标准 MCP streamable HTTP），严格校验的客户端会失败——**推荐方式 A**。

## 三、CLI 接口

| 命令 | 用途 | 示例 |
|---|---|---|
| `sts-x ai "<查询>" -p <项目>` | 智能路由（符号→locate，自然语言→expand）| `sts-x ai "缓存" -p .` |
| `sts-x search "<词>" -p <项目> [--locate\|--expand]` | 精确控制搜索 | `sts-x search "McpServer" -p . --locate` |
| `sts-x search "<词>" -p <项目> --path-filter cache.rs` | **单文件圈定** | 结果 100% 落在该文件 |
| `sts-x search "<词>" -p <项目> --sort-recent` | 最近修改优先 | 刚改的文件排前 |
| `sts-x search "<词>" -p <项目> --no-hint` | 省略 `_ai_instructions`（省 ~200 tok）| AI 高频调用建议加 |
| `sts-x search "<词>" -p <项目> -t N` | 结果数（默认 2）| — |
| `sts-x search "<词>" -p <项目> -c N` | expand 上下文行数（0=整块）| — |
| `sts-x file "<词>" -p <目录> [--name-only]` | 任意目录文件搜索（免索引）| `sts-x file "Cargo.toml" -p ~/Downloads` |
| `sts-x index <项目>` | 手动建索引（一般自动）| — |
| `sts-x status -p <项目>` | 索引状态 | — |
| `sts-x serve -p <项目> --port 9876` | MCP HTTP 服务 | — |

## 四、输出契约（AI 解析规则）

### locate 模式（`--locate` / 符号路由）
```json
{
  "query": "McpServer",
  "mode": "locate",
  "matches": [
    {
      "file": "src/mcp/mod.rs",
      "abs_path": "/abs/path/libs/core_lib/src/mcp/mod.rs",
      "line": 64,
      "context": "pub struct McpServer {",
      "score": 1.0,
      "name": "McpServer"
    }
  ],
  "hint": "..."        // 仅 0 命中时出现；有命中省略
}
```
- **拿到 abs_path 直接 Read**，无需二次解析
- `hint` 字段可选（serde skip）——0 命中时是自救引导

### expand 模式（默认 / 自然语言路由）
```json
{
  "query": "缓存",
  "mode": "expand",
  "results": [{
    "path": "src/cache.rs",
    "abs_path": "/abs/path/src/cache.rs",
    "lines": [1, 30],
    "highlight_lines": [],
    "kind": "module",
    "name": "cache",
    "signature": "",
    "code": "...",        // 完整代码块
    "language": "rust"
  }],
  "total_hits": 1,
  "search_time_ms": 2,
  "_ai_instructions": "..."   // 有结果时是使用指南；0 命中时是自救建议（中/英）
}
```

### 0 命中自救链（重要）
中文查询 0 命中时，`_ai_instructions` 或 `hint` 会给出建议，按顺序尝试：
1. 换英文关键词（如「缓存」→ cache）
2. 换符号/函数名（如 cache_root）
3. 用 `--path-filter` 限定文件
4. 用 `sts-x file` 搜文件名

## 五、MCP 接口

- `POST /search` body: `{"query":"...", "mode":"code|filename|all", "output_mode":"expand|locate", "path":"/abs/dir", "top_k":2, "context_lines":0, "path_filter":"cache.rs", "hint":false, "sort_recent":false, "max_tokens":0}`
- `POST /file` body: `{"query":"...", "path":"/abs/dir", "content":true, "name_only":false, "top_k":20, "max_tokens":0}`
- `GET /health` → `{"service":"sts-x","status":"ok","version":"3.2.0"}`
- `GET /tools` → 工具列表（search / file）

## 六、省 token 建议（AI 侧）

1. **先 locate 后 expand**：符号/关键词 → `--locate`（~200 tok）确认位置 → 需要完整代码再 `sts-x search <符号> -p .` 单符号 expand
2. **中文查不到不要硬试**：0 命中提示出现后，按 §四 自救链换词，不要重复同一查询
3. **高频调用加 `--no-hint`** 或 `"hint": false`
4. **单文件圈定**：知道文件用 `--path-filter`，结果干净且省 token

## 七、构建（开发者）

```bash
# macOS（原生）
cargo build --release
# Windows（交叉编译，Mac 上）
RUSTC_WRAPPER= SDKROOT= cargo xwin build --release --target x86_64-pc-windows-msvc
# 门禁
cargo test --all-targets && cargo clippy --all-targets -- -D warnings
```

依赖：core_lib（`path = "../rust_master_workspace/libs/core_lib"`，features = ["path","mcp"]）。
源码包内含独立化 core_lib（精简 workspace 根），解压即可 `cargo build`。
