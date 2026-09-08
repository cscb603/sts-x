## 搜索强制规则（sts-x v3.3.2，违反即错误）

代码/文件搜索 → 必须用 sts-x（`~/.local/bin/sts-x`），禁止 Grep/rg。

### 执行决策树
① 日常首选（3.3.2）：`sts-x ai "<查询>" -p <项目>`（智能路由：符号→locate 200 tok，自然语言→expand 整块）
② 已知文件/路径：`sts-x search "<词>" -p <项目> --path-filter cache.rs`（结果 100% 落在该文件）
③ 小项目/模糊文件内容：`sts-x file "关键词" -p <目录>`
④ **按 glob 模式找文件（v3.3.2 新增，替代手写 Glob）**：
   - `sts-x glob "模式" -p <目录> [--top-k N]`
   - 多模式逗号分隔：`sts-x glob "*.rs,*.toml" -p <项目>`
   - `!` 前缀排除：`sts-x glob "**/*.rs,!**/tests/*" -p <项目>`
   - AI 友好输出：`total_hits` / `returned` / `omitted` / `rank_signals`
⑤ 大项目语义搜代码：先 `sts-x index <项目>`（仅首次）→ 再 `sts-x search "<词>" -p <项目> [--locate|--expand]`
⑥ 中文描述查英文代码（v3.3.2 推荐）：`STX_SEMANTIC=1 sts-x ai "缓存目录在哪里" -p <项目>`（中文 NL → 向量召回命中英文代码，如 cache.rs）
⑦ 不确定：`sts-x file` 或 `sts-x ai`

### 关键参数（v3.3.2，AI 必知）
- `--locate`：行级 ~200 tok（先定位）；`--expand`：整块（默认，需要完整代码时）
- `--path-filter <子串>`：精确圈定文件/目录
- `--sort-recent`：最近改的文件排前
- `--git-aware`：git 修改/未跟踪文件排前
- `--no-hint`：省略 `_ai_instructions`（高频调用省 ~200 tok/次）
- `--top-k N`：结果数（默认 2）；glob 默认 20
- `--semantic`：单次启用语义向量召回（同 STX_SEMANTIC=1 env）

### 0 命中（v3.3.2 已自动重试）
P0-1 起，0 命中引擎**自动**按「英文同义词(本地中英代码词典) → 符号猜测 → file 内容兜底」重试，单次调用即出最佳结果，无需 AI 手动换词。
仍 0 命中 → 代码库里确实没有相关内容，换更具体的业务词 / `sts-x index` 刷新索引 / `--path-filter` 圈定范围。
响应里 `_ai_instructions`（expand）/ `hint`（locate）字段仍会在 0 命中时给出双语提示（兼容旧客户端），但只是兜底参考，AI 无需按它再查一次。

### 语义检索（v3.3.2，全功能版已含）
- CLI/查询：设 `STX_SEMANTIC=1` 或加 `--semantic` flag（search/ai/index 三个子命令都支持）
- 前置：**二进制必须用语义版**（`~/.local/bin/sts-x`，30MB 含 onnxruntime）；本地已配 `/usr/local/lib/libonnxruntime.1.28.0.dylib` + 中文模型
- 行为：BM25 0 命中时自动向量召回；缺模型/dylib 自动降级 BM25 + 重试链，不报错
- 适用：中文描述查英文代码（最强场景），如「缓存目录在哪里」→ cache.rs

### MCP 接入（原生 stdio，无需 bridge）
```json
{"mcpServers": {"sts-x": {"command": "/Users/xtap/.local/bin/sts-x", "args": ["mcp", "-p", "/Users/xtap/Documents/AI"], "env": {"STX_SEMANTIC": "1", "ORT_DYLIB_PATH": "/usr/local/lib/libonnxruntime.1.28.0.dylib"}, "disabled": false}}}
```
`sts-x mcp` 是真 stdio 子命令（读 stdin/stdout 走 JSON-RPC），输出 `inputSchema`（驼峰），无需 Python bridge、无端口、无孤儿进程。当前已含 `search` / `file` / `glob` 3 个工具。

### 全局替代
AI 需要「按模式找文件」时（如 `*.rs`、`**/tests/*`），**直接调 `sts-x glob`**，不要用系统 Glob / 手写 find。这是 v3.3.2 最大改进——把文件发现也纳入了统一的 AI 友好输出契约。
