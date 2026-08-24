# AGENTS.md — STS-X AI 接口契约（v3.3.1：P0-1 自动重试链 + P0-2 中文语义检索）

> 本文件是给 **AI Agent / 开发者** 看的接口契约。人类小白看 `首次打开必看.txt`。

## 一、这是什么

STS-X = 给 AI Agent 用的轻量代码+文件搜索引擎。Rust 单二进制、零运行时依赖、CLI + MCP 双入口。

- 代码搜索：AST 切块 + Tantivy BM25，中文注释代码可用
- 文件搜索：任意目录零索引（rg 后端）
- 输出契约：locate（行级，~200 tok）/ expand（整块，可控预算）
- 0 命中自动重试链（P0-1）：BM25 0 命中时按 ①英文同义词（本地词典）②符号猜测 ③file 内容兜底 自动重试，单次调用即出最佳结果，不再要求 AI 手动换词再调
- 语义检索（P0-2，可选）：`STX_SEMANTIC=1` / `--semantic` 启用 ONNX embedding，0 命中时向量召回补回跨语言语义

## 二、快速部署（AI 可直接执行）

```bash
# macOS（假设二进制已就位）
sudo cp ./sts-x /usr/local/bin/sts-x
# 验证
sts-x --version        # 3.3.1
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
| `sts-x search "<词>" -p <项目> --semantic` | 启用语义向量召回（同 `STX_SEMANTIC=1`）| 需 semantic 构建 + 本地模型 |
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
  "search_time_ms": 2,     // v5.1-3: 波动字段已移末尾，不破坏前缀缓存
  // v5.1-3: _ai_instructions 仅在 0 命中时出现（中/英自救建议）；
  // 有命中时字段省略——741 字使用指南是纯流量浪费（工具描述已含用法）
}
```

### 0 命中自救链（v3.2.0+ 已自动执行，对外仍是单次调用）
P0-1 起 0 命中不再只是甩引导文本——引擎内部按固定顺序自动重试并合并结果：
1. **英文同义词**（本地中英代码词典：缓存→cache、索引→index、提示→hint…）
2. **符号猜测**（去停用词后抽 ASCII 标识符，如 "hint"）
3. **file 内容兜底**（文件名/内容子串，rg 后端）

任一阶段命中即返回（结果 `explanation` 或 stderr 日志标注 `auto-retry(english-synonym|symbol-guess|file-fallback)`）。
全部不命中才返回空 + `_ai_instructions`/`hint`（兼容旧客户端）。**AI 无需再手动换词重试**。

### semantic 语义检索（P0-2，可选开启）
- 开关：`STX_SEMANTIC=1` 环境变量 或 `--semantic` flag（CLI `search`/`ai`/`index`）。
- 前置：二进制须以 `--features semantic` 构建；本地放 embedding 模型。
- 模型查找：`$STX_MODEL_DIR` > 可执行文件同目录 `models/` > 缓存根 `models/` > `config.model_path`；
  目录布局 `model.onnx + tokenizer.json` 平铺，或**一层任意名子目录**（如 `bge-small-zh-v1.5/`、`bge-small-en-v1.5/`）。
- 行为：语义索引含向量（embedding 持久化进 tantivy）；BM25 0 命中时自动向量召回（中文 NL → 英文代码）；
  模型/dylib 缺失时优雅降级到 BM25 + P0-1 重试链（不崩）。
- 动态库：`load-dynamic` 需要 `libonnxruntime`（macOS 设 `ORT_DYLIB_PATH=/path/libonnxruntime.1.28.0.dylib`；
  Windows 把 `onnxruntime.dll` 放 exe 同目录或 `lib/`）。
- **推荐模型**：`bge-small-zh-v1.5`（中文 512 维，下载脚本 `scripts/download-models.sh`）。
  维度由引擎从模型输出 shape **自适应校准**（en=384 / zh=512 / 其他任意），换模型无需改代码。

## 五、MCP 接口

- `POST /search` body: `{"query":"...", "mode":"code|filename|all", "output_mode":"expand|locate", "path":"/abs/dir", "top_k":2, "context_lines":0, "path_filter":"cache.rs", "hint":false, "sort_recent":false, "max_tokens":0}`
- `POST /file` body: `{"query":"...", "path":"/abs/dir", "content":true, "name_only":false, "top_k":20, "max_tokens":0}`
- `GET /health` → `{"service":"sts-x","status":"ok","version":"3.3.1"}`
- `GET /tools` → 工具列表（search / file）

## 六、省 token 建议（AI 侧）

1. **先 locate 后 expand**：符号/关键词 → `--locate`（~200 tok）确认位置 → 需要完整代码再 `sts-x search <符号> -p .` 单符号 expand
2. **中文 0 命中已自动重试**（P0-1：英文同义词→符号→file 兜底），仍 0 命中说明代码里确实没有相关内容——换一个更具体的业务词或先 `sts-x index` 确认索引新鲜，不要反复重复同一查询
3. **v5.1-3 起有命中自动无指南**（`_ai_instructions` 仅 0 命中出现）——不需要再手动 `--no-hint` 省指南；`--no-hint`/`"hint": false` 仍可连 0 命中自救串一起省掉（极致省流量场景）
4. **单文件圈定**：知道文件用 `--path-filter`，结果干净且省 token
5. **缓存友好**：输出字段顺序稳定、波动字段在末尾——重复查询命中 LLM 前缀缓存（DeepSeek 0.02¥/M 档）；AI 侧尽量保持查询方式一致（同参数同 top_k）以复用缓存

## 七、构建（开发者）

```bash
# macOS（原生）
cargo build --release
# macOS（原生 + 语义检索，可选）
cargo build --release --features semantic
# Windows（交叉编译，Mac 上；+ 语义检索）
RUSTC_WRAPPER= SDKROOT= cargo xwin build --release --target x86_64-pc-windows-msvc [--features semantic]
# Windows 语义包运行时（缺一不可）：
#   1. onnxruntime.dll 放 sts-x.exe 同目录或 lib/（nuget: Microsoft.ML.OnnxRuntime 1.28.0，或 onnxruntime-win-x64 官方 zip）
#   2. models/bge-small-zh-v1.5/{model.onnx,tokenizer.json}（或 STX_MODEL_DIR 指向）
#   3. 设 STX_SEMANTIC=1（缺 dll/模型时自动降级 BM25 + 重试链，不崩）
# 门禁（含 semantic feature）
cargo test --all-targets --features semantic && cargo clippy --all-targets --features semantic -- -D warnings
```

依赖：core_lib（`path = "../rust_master_workspace/libs/core_lib"`，features = ["path","mcp"]）。
源码包内含独立化 core_lib（精简 workspace 根），解压即可 `cargo build`。
