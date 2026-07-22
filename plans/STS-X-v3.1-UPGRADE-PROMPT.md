# STS-X v3.1 执行指令

> AI 可消费契约 · 复制此段到新对话执行

## 仓库信息

- 位置：`/Users/xtap/Documents/AI/sts-x`
- 分支：`v3.0`（基于 0.2.1，已含 v3.0.0 发版代码 + QueryParser 修复 + `_` 分词对齐修复）
- 当前 commit：`e83a6e2`（HEAD）
- 新发布二进制：`target/release/sts-x`（17.86MB，Mach-O arm64）

## 依赖环境（已就绪）

| 工具 | 版本 | 备注 |
|------|------|------|
| rustc | 1.93.1 | ✅ |
| cargo-xwin | 已安装 | Windows 交叉编译 |
| RIP | 无 | ✅ |
| rg | 14.1.1 | file 模式依赖 |
| tiktoken | 已装（Python venv） | token 计量 `/Users/xtap/.workbuddy/binaries/python/envs/default/bin/python` |
| UPX | 需确认 | 可选瘦身 |

## 不做的事（边界，开工前先读）

- ❌ 不引入 ONNX/embedding/向量数据库
- ❌ 不重构为 Probe 式内置 Agent
- ❌ 不写 Web UI / IDE 插件
- ❌ 不拆 schema / 不重建索引
- ❌ 不改已有 CLI flags 名 / MCP 路由 / JSON 输出字段

## 任务分步

### Step 0：验证基线

开工前先确认代码能编译，已有测试全过：

```bash
cd /Users/xtap/Documents/AI/sts-x
cargo check && cargo test -p retouch-core 2>&1 | tail -5
```

如果 `cargo test` 没有 `retouch-core` crate（那个是另一个项目），就只跑 `cargo check` 确认。

### Step 1：备份/噪声文件过滤（P0，~20 行）

**做什么**：索引时跳过备份、原始、旧版、交换文件。需要同时过滤目录和文件，涵盖 `class QwenVLClient` 命中 `_backup/视频连环画摘要/` 这种场景。

**改哪个文件**：`src/indexer/mod.rs` 内的 `handle_project`（或负责 walk 文件的函数）。在 `ignore::WalkBuilder` 遍历路径后、判断 `is_supported_extension` 前，加一层 `is_noise_path()` 过滤：

```rust
fn is_noise_path(path: &Path) -> bool {
    // 检查文件/目录名中是否含噪声标记
    let name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let noise_patterns = ["_backup", "_original", "_old", "_copy",
                          "复制", "副本", ".bak", ".swp", ".tmp"];
    noise_patterns.iter().any(|p| name.contains(p))
}
```

**验收**：在 `/Users/xtap/Documents/AI/online_repos_backup/StarTap-Image-Shrinking-Tool` 上重索引后：
- `sts-x search "is_supported_image" -p <path> --expand -t 5` 不再出现 `main_backup_v4.rs`
- `sts-x search "class QwenVLClient" -p /Users/xtap/Documents/AI/retouch_app --locate -t 3` 不再出现 `_backup/` 目录

### Step 2：定义优先排序（P1，~50 行）

**做什么**：搜索排序时，匹配定义的块（函数定义、类定义、结构体定义）排名高于引用的块。

**改哪个文件**：修改 `src/indexer/mod.rs` 的 `search_text` 返回后的 BM25 评分。

具体：在 `search_text` 的循环 `for (score, doc_addr) in top_docs {` 内，找到 `let norm_score = (score / 10.0).clamp(0.0, 1.0);` 这句话。在此之后叠加定义分：

```rust
// 定义优先：如果块的名字包含查询词且类型是定义型，叠加 +0.3
let def_types = [BlockKind::Function, BlockKind::Class,
    BlockKind::Struct, BlockKind::Method, BlockKind::Enum,
    BlockKind::Interface, BlockKind::Trait];
let def_boost = if query_terms.iter().any(|t| entry.block.name.to_lowercase().contains(t))
    && def_types.contains(&entry.block.kind) {
    0.3
} else {
    0.0
};
let final_score = (norm_score + def_boost).clamp(0.0, 1.0);
results.push((final_score, entry));
```

**注意**：`query_terms` 需要作为参数传给 `search_text`（当前它内部自己 split query）。你要么在函数签名加 `terms: &[String]`，要么在 `search_text` 内重复一次 split。

**验收**：搜 `is_supported_image` → 第一条的 `kind` 应该是 `function` 且 `name` 是 `is_supported_image`，而非调用处。

### Step 3：多词查询最小匹配优化（P0+，与 Step 2 同时完成，~15 行）

**做什么**：当前 OR 查询把所有词当 Should，对于 3+ 词的查询，设 `minimum_should_match = terms.len() - 1`（匹配 N-1 项），避免 `select_best_cfg` 拆词后只匹配到 `select` 的无关块。

**改哪个文件**：`src/indexer/mod.rs` 的 `search_text` 中构建 `BooleanQuery` 的地方。

关键代码段（当前 `indexer/mod.rs` 第 565–570 行附近）：

```rust
// 当前构建
let tantivy_query: Box<dyn Query> = Box::new(BooleanQuery::new(field_clauses));
```

改为：

```rust
// 多词查询：短查询（1-2 词）用 OR；3+ 词要求匹配至少 N-1 项
let min_should = if terms.len() <= 2 { 1 } else { terms.len() as u32 - 1 };
let mut bq = BooleanQuery::new(field_clauses);
bq.set_minimum_should_match(min_should);
let tantivy_query: Box<dyn Query> = Box::new(bq);
```

**注意**：`BooleanQuery::set_minimum_should_match` 是否在 tantivy 0.22 的公开 API 中？如果不在，改用 `BooleanQuery::new_with_minimum_should_match(subqueries, min_should)`。检查 tantivy 文档或 grep `BooleanQuery` 的 API：

```bash
# 在 cargo deps 里搜
grep -r "minimum_should_match" /Users/xtap/.cargo/registry/src/*/tantivy-0.22*/src/ 2>/dev/null | head -5
```

**验收**：搜 `select_best_cfg`（3 词）→ `total_hits` 应减少但命中代码指向定义 `select_best_cfg` 的块。

### Step 4：查询侧对齐 RemoveLongFilter（~5 行）

**做什么**：索引侧 code tokenizer 有 `RemoveLongFilter::limit(40)`，移除 >40 字符的 term。查询侧也应过滤 >40 字符的 term，避免毫无意义的 TermQuery。

**改哪个文件**：`src/indexer/mod.rs` 的 `search_text`，在 `terms` 的 filter 链中加 `.filter(|s| s.len() <= 40)`：

```rust
let mut terms: Vec<String> = query
    .split(|c: char| !c.is_alphanumeric())
    .map(|s| s.to_lowercase())
    .filter(|s| !s.is_empty() && s.len() <= 40)
    .collect();
```

**验收**：不需要单独验收，同时不会破坏功能。

### Step 5：5+ 新语言（有更好，~0.5 session）

**做什么**：在 `Cargo.toml` 加入以下 tree-sitter grammar：

| 语言 | crate | 版本（近期） |
|------|-------|-------------|
| PHP | `tree-sitter-php` | 0.23 |
| Ruby | `tree-sitter-ruby` | 0.22 |
| Kotlin | `tree-sitter-kotlin` | 0.4 |
| Swift | `tree-sitter-swift` | 0.7 |
| Scala | `tree-sitter-scala` | 0.24 |

**改哪些文件**：

1. `Cargo.toml` → 在 `[dependencies]` 加这 5 个 crate
2. `src/chunker/mod.rs` → 扩展 `Language` enum 和 `parse_language()` 函数（找到 `Language::Rust` 等定义的地方）
3. `src/indexer/mod.rs` → 扩展 `is_supported_extension()` 函数（已有，在文件顶部附近），加入 `.php` `.rb` `.kt` `.swift` `.scala` 映射

**注意**：每个 grammar crate 导入时名字可能带连字符，看上游 crate 怎么导的。不确定的 `cargo check` 会告诉你。

**验收**：在 `src/chunker/tests/` 或单独跑每语言测试文件。至少每语言一个 `fn/def/class` 能被正确解析：

```bash
# 例如验证 PHP
echo '<?php function hello() { return 1; }' > /tmp/test.php
cd /Users/xtap/Documents/AI/sts-x && cargo run --release -- index /tmp/ 2>&1 | tail -1
cargo run --release -- search "hello" -p /tmp/ --expand -t 1 2>&1 | grep 'kind.*function'
```

### Step 6：`--max-tokens` 配额控制（P2，~0.5 session）

**做什么**：CLI `--max-tokens` 参数 + MCP 请求 `max_tokens` 字段。搜索结果按分数排序，累计 token 数超过配额时截断。

**估算 Token**：使用 char/2.0 的粗略近似（因为代码有大量标点，比纯自然语言更密）。所以在 Rust 端：

```rust
fn estimate_tokens(s: &str) -> usize {
    (s.chars().count() + 1) / 2  // 保守：2 char ≈ 1 tok
}
```

**改哪些文件**：

1. `src/types/mod.rs`：`SearchQuery` 加 `max_tokens: Option<usize>` 字段
2. `src/cli/mod.rs`：Search 子命令加 `--max-tokens` flag
3. `src/search/mod.rs` 的 `search_code_mode`：在组装 `SearchResponse` 前，遍历 `sorted` 结果，累计估计 token，超过时 break
4. `src/server/mod.rs` 的 `handle_search`：从 JSON 读 `max_tokens` 并传入 `SearchQuery`

**验收**：
```bash
cargo run --release -- search "fn" -p /Users/xtap/Documents/AI/retouch_app --expand -t 10 --max-tokens 500 2>&1 | \
  /Users/xtap/.workbuddy/binaries/python/envs/default/bin/python -c "
import sys, json, tiktoken
enc = tiktoken.get_encoding('cl100k_base')
data = json.load(sys.stdin)
code = '\n'.join(r.get('code','') for r in data.get('results',[]))
print(len(enc.encode(code, disallowed_special=())))
"
# 输出应 ≤500
```

### Step 7：跨平台构建 + 发版

**macOS**（当前机器就是，简单）：
```bash
cargo build --release 2>&1 | tail -3
file target/release/sts-x  # 应为 Mach-O 64-bit arm64
```

**Windows**（交叉编译）：
```bash
cd /Users/xtap/Documents/AI/sts-x
export EMBED_RESOURCE_LLVM_RC=1
cargo xwin build --release --target x86_64-pc-windows-msvc 2>&1 | tail -5
file target/x86_64-pc-windows-msvc/release/sts-x.exe  # 应为 PE32+ x86-64
```

**验证静态 CRT**（不能有 VCRUNTIME140.dll 依赖）：
```bash
strings target/x86_64-pc-windows-msvc/release/sts-x.exe | grep -c "VCRUNTIME140"  # 应输出 0
```

**GitHub 发布**：
```bash
gh release create v3.1.0 --title "STS-X 3.1.0 — 定义优先排序 + 备份过滤 + 新语言" \
  --notes-file /tmp/release_notes_v310.md \
  target/release/sts-x#sts-x-macos-arm64 \
  target/x86_64-pc-windows-msvc/release/sts-x.exe#sts-x-windows-x86_64.exe
```

## 验收总表

执行完所有步骤后，逐一验证：

| # | 验收项 | 命令 | 预期 |
|---|--------|------|------|
| A1 | 备份过滤 | `sts-x search "is_supported_image" -p <StarTap> -t 5` 输出不含 `_backup` | ✅ |
| A2 | 备份目录过滤 | `sts-x search "class QwenVLClient" -p <retouch> --locate -t 3` 不含 `_backup` | ✅ |
| A3 | 定义优先 | `sts-x search "is_supported_image" -p <StarTap> -t 3` 首条 kind=function | ✅ |
| A4 | 多词最小匹配 | `sts-x search "select_best_cfg" -p <retouch> -t 3` 结果指向相关块 | ✅ |
| A5 | :: 符号 | `sts-x search "Cli::parse" -p <StarTap>` > 0 命中 | ✅ |
| A6 | 下划线 | `sts-x search "select_best_cfg" -p <retouch>` > 0 命中 | ✅ |
| A7 | locate ≤200 | `scripts/acceptance_s8.py` 三组 ≤200 tok | ✅ |
| A8 | expand 省≥80% | `scripts/acceptance_s8.py` 三组 ≥80% | ✅ |
| A9 | 5+ 新语言 | 每语言至少 1 个函数正确解析 | ✅ |
| A10 | --max-tokens | `search "fn" -t 10 --max-tokens 500` ≤500 tok | ✅ |
| A11 | Win 静态 CRT | `grep VCRUNTIME140` = 0 | ✅ |
| A12 | 回归 | `cargo check` + 已有功能不退化 | ✅ |

## 提示

- **编译慢**：`cargo build --release` 含 tree-sitter + tantivy 约 1-2 分钟。用 `cargo check` 先验证类型。
- **VPN 切换**：`git push` 偶发 502，重试即过。`gh` 走 API 路线通常更稳。
- **分段提交**：每个 Step 完成后 `git add` + `git commit`，不要等全部完成才提交。
- **索引缓存**：改查询逻辑（query parsing/ranking）不需要重索引；改索引逻辑（walk / filtering / chunking）必须 `rm -rf ~/Library/Caches/sts-x` 清缓存再测。
- **tiktoken 计量**：`/Users/xtap/.workbuddy/binaries/python/envs/default/bin/python -c "import tiktoken; enc=tiktoken.get_encoding('cl100k_base'); print(len(enc.encode('your json here')))"`
