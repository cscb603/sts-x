# STS-X 升级蓝图 — v3.1

> AI 可消费契约 · 先设计后执行
> 2026-07-22
> 基于：v3.0.0（已发版，含 QueryParser 语法修复 + 二合一代码/文件搜索 + locate/expand 双档输出）

---

## 0. 状态快照

### 当前能力（v3.0.1 已验证）

| 功能 | 状态 | 备注 |
|------|------|------|
| AST 切块（9 语言） | ✅ | Rust/Python/JS/TS/TSX/Java/C/C++/Go |
| BM25 全文搜索 | ✅ | Tantivy 0.22，自定义 code 分词器 |
| `--expand` 完整块输出 | ✅ | §8.1 实测省 **92.9% / 96.4% / 93.6%** tok |
| `--locate` 行级 grep 输出 | ✅ | **≤111 tok**（要求≤200） |
| `file` 零索引搜索 | ✅ | rg 后端，文件名+内容 |
| MCP `/search` + `/file` | ✅ | axum HTTP |
| `--output_mode` 枚举 | ✅ | Expand / Locate |
| 跨平台构建 | ✅ | macOS arm64 / Win x86-64（静态 CRT）/ Linux（源码构建） |

### 已发现 + 已修复的 Bug（本轮）

| Bug | 症状 | 根因 | 已修？ | 验证数据 |
|-----|------|------|--------|---------|
| `Cli::parse` Syntax Error | AI 搜 `Foo::bar` 直接报错 | Tantivy QueryParser 把 `::` 当非法语法 | ✅ 走 TermQuery 手工构建 | `Cli::parse` → 3 命中 `main.rs` |
| `select_best_cfg` / `is_supported_image` 0 命中 | 含 `_` 的标识符完全搜不到 | 查询侧保留 `_`（`!is_alphanumeric() && c != '_'`），索引侧 `SimpleTokenizer` 用 `!is_alphanumeric()` 把 `_` 当分隔符 → 两边不对齐 | ✅ 查询分词与 SimpleTokenizer 完全对齐，去掉 `_` 特例 | `select_best_cfg` 0→**3 hit**；`is_supported_image` 0→**2 hit**；`read_dir` 0→**3 hit** |
| 备份文件/目录污染结果 | 搜 `read_dir` 返回 `main_backup_v4.rs`；搜 `class QwenVLClient` 返回 `_backup/视频连环画摘要...` | 没忽略 `*_backup*.*` / `*_original*.*` 和备份目录 | ⚠️ 待修（P0） | 实测 `qwen` 第一条定位到 `_backup/` 目录 |

---

## 1. 需求确认单

| 维度 | 内容 |
|------|------|
| 项目 | STS-X v3.1（继续用 v3.x 系列，不打破现有接口） |
| 一句话 | 让 sts-x 比 rg/find 更快更准、省 token、AI 爱用、足够小巧 |
| **必须有** | ① 修复所有已知 bug（备份文件过滤、_ 分词对齐）② 可量化比 rg 快的 benchmark ③ 新增一种输出模式/功能亮点 |
| **有更好** | ④ 新增 5+ 语言（至少 PHP/Ruby/Kotlin）⑤ 部分语义信号（如定义优先）⑥ 二进制体积优化（strip 后 ≤15MB） |
| **明确不做** | ① 不减掉 `file` 模式（零索引是核心差异）② 不引入 embedding/向量数据库（增加依赖炸体积）③ 不重构为 Probe 式的大 agent 体系（不是同一个定位） |
| **技术需求** | Rust 2024 edition，保留 cargo-xwin 交叉编译，clippy -D warnings |
| **对标竞品** | Probe（Rust, tree-sitter + MCP, 零索引, 480★） / Semble（Python, 语义 + BM25, 2k tok=94% recall, 799★）/ ast-grep（YAML 模式搜索, Rust + tree-sitter） |

---

## 2. 调研摘要

### 2.1 竞品深度对比

#### Probe（probelabs/probe）— 最直接的竞品

| 维度 | STS-X v3.0 | Probe v? |
|------|-----------|----------|
| 搜索方式 | BM25 TermQuery | BM25 + TF-IDF + SIMD + 可选 BERT |
| AST | tree-sitter（9 语言） | tree-sitter（全部主流语言） |
| 零索引搜索 | `file` 子命令（rg 兜底） | 全部搜索零索引 |
| 输出 | `--locate` 行级 / `--expand` 块级 | 完整 AST 块 + token budget |
| MCP | `/search` + `/file` | `search` + `query` + `extract` + `symbols` |
| Agent | 无内置 | 内置 Probe Agent（多模型） |
| 体积 | ~17.8 MB | ~19.7 MB |
| 安装 | 单二进制 | npm / cargo |
| 独家 | **locate 双档**更省 tok | SEMD 加速、Token 感知去重、布尔查询语言 |

**可借鉴**：Token 感知去重（session dedup）、`--max-tokens` 配额控制、文件过滤扩展 `ext:`,`lang:` — 对 AI 接入非常实用。

#### Semble（MinishLab/semble）— 最省 tok 的路线

| 维度 | STS-X v3.0 | Semble v0.1.7 |
|------|-----------|---------------|
| 检索 | BM25 纯词法 | Model2Vec 静态语义 + BM25 双检索 + RRF 融合 |
| AST 分块 | tree-sitter（9 语） | tree-sitter（50+ 语） |
| Token 效率 | expand 省 ~80-98% | 2k tok = 94% recall（省 ~98%） |
| 查询速度 | ~1-2ms（BM25 本地） | ~1.5ms（语义 + 词法 + RRF） |
| 索引速度 | 按需（数秒） | ~250ms/repo |
| 安装 | 单二进制 | pip / MCP |
| 独家 | locate 行级输出极其省 tok | 双检索器 + 重排信号（定义优先/标识符词干/文件连贯性/噪声惩罚） |

**可借鉴（高 ROI）**：
- 定义优先（definition boost）：匹配定义 > 引用，直接省 AI 读完定义跳引用的 tok
- 文件连贯性（file coherence）：同类多块命中时整文件加分
- 噪声惩罚（noise penalties）：降权测试文件/legacy/.d.ts → 我已经发现备份文件问题，这是个可扩展方案
- 标识符词干匹配（identifier stems）：`parse_config` 匹配 `parseConfig` / `ConfigParser` / `config_parser`

**不可借鉴（低 ROI）**：
- Model2Vec 静态 embedding → 需要 16M 模型 + Python 依赖，炸体积
- 双检索器 RRF 融合 → 需要 embedding，同上

#### ast-grep — 不同定位

ast-grep 是**模式搜索**（YAML 模式匹配 AST 结构），不是**语义搜索**（找到这个函数/这个符号）。它适合 lint/code 改写，不适合 AI agent 找要改的代码。**不直接竞争，但可借鉴**：其 YAML 规则系统对 "找某种模式的代码" 场景有用。

### 2.2 已知坑

| 坑 | 严重程度 | 说明 |
|----|---------|------|
| 🌐 VPN 切换导致 `git push` 偶发 502 | 阻塞（已验证） | 非 bug，环境限制；重试可过 |
| Tantivy `BooleanQuery` 嵌套评分 | 中 | 当前扁平 Should 可能不如 QueryParser 原生的字段加权；实测不影响命中数 |
| `file` 模式 `rg` 未安装时回退到 `ignore::WalkBuilder` | 低 | 文件内容搜索退化到文件名级别；但用户环境有 rg |
| 索引不区分 `main.rs` vs `main_backup_v4.rs` | **中** | 备份文件污染搜索结果 |
| `_` 分词手动 != SimpleTokenizer | **高（已修）** | 查询与索引分词不对齐导致完整标识符搜不到 |

---

## 3. 升级方案

### 3.1 路线选择（四条路 / 抓大头）

按 ROI = 节省量 / 实现复杂度 排序：

| 优先级 | 任务 | 预期收益 | 复杂度 | ROI | 依赖 |
|--------|------|---------|--------|-----|------|
| P0 | 备份文件/噪声文件过滤 | 结果纯净度大幅提升 | 低（~20 行） | **极高** | 无 |
| P0 | `_` 分词对齐（已修） | 修复完整标识符搜索 | 低（已修） | **极高** | 重发版 |
| P1 | 定义优先排序（definition boost） | 返回定义而非引用，省 AI 一次追踪 | 中（~50 行重排器） | **高** | 无 |
| P2 | 新增 5+ 语言（PHP/Ruby/Kotlin/Swift/Scala） | 覆盖更多项目 | 中（添加 grammar） | **中** | crates.io |
| P2 | `--max-tokens` 配额控制 | 精准预算 token，防 AI 过载 | 低（~20 行） | **中** | 无 |
| P3 | 文件连贯性 boost | 关联命中更准确 | 中（~80 行） | 低 | 重排器架构 |
| P3 | 二进制体积优化（strip/lto/UPX） | 18MB → ~12MB | 低（配置项） | 低 | 现有 |
| P4 | 语义检索（Model2Vec + RRF） | 自然语言搜索 | 高（模型依赖） | 极低 | ❌ 暂不引入 |
| P4 | 内置 Agent | 替代 Probe Agent | 极高 | 极低 | ❌ 暂不引入 |

### 3.2 v3.1.0 范围（P0 + P1）

#### 3.2.1 噪声文件过滤（P0）

**做什么**：索引和搜索时忽略备份文件/常见噪声。
**文件列表**：匹配以下 pattern 的文件被排除：
- `*_backup*.*` / `*_original*.*` / `*_old*.*` / `*_copy*.*` / `*.bak` / `*.swp`
- 可选 `.sts-x-ignore` 文件（类似 `.gitignore`）

**怎么做**：
```rust
// 在 ignore::WalkBuilder 后增加一层过滤
fn is_noise_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let patterns = ["_backup", "_original", "_old", "_copy", ".bak", ".swp"];
    patterns.iter().any(|p| name.contains(p))
}
```

**验收**：搜索 `is_supported_image` 只返回 `main.rs` 不返回 `main_backup_v4.rs`。

#### 3.2.2 SearchIndex::search_text 定义优先排序（P1）

**怎么做**：在 `search_text` 返回的 BM25 分上叠加定义优先信号。检查 `IndexedBlock` 的 `kind` 和 `name` 是否与查询词匹配。如果 `name` 包含查询词且 `kind` 属于 `function`/`class`/`struct` 等定义类型 → 叠加 +0.3 分。

```rust
fn definition_boost(block: &IndexedBlock, query_terms: &[String]) -> f32 {
    let name_lower = block.name.to_lowercase();
    if query_terms.iter().any(|t| name_lower.contains(t)) {
        match block.kind {
            BlockKind::Function | BlockKind::Class | BlockKind::Struct
            | BlockKind::Method | BlockKind::Enum => 0.3,
            _ => 0.0,
        }
    } else {
        0.0
    }
}
```

**验收**：搜 `is_supported_image` → `fn is_supported_image` 排在第一（定义区块），而非调用处。

#### 3.2.3 5+ 新语言支持（P2，有更好）

**支持列表**：PHP、Ruby、Kotlin、Swift、Scala。
**怎么做**：在 `Cargo.toml` 加 tree-sitter grammar crate，在 `chunker` 里加对应 `Language` 映射。
**tree-sitter 包**（已验证可获取）：
| 语言 | crate |
|------|-------|
| PHP | `tree-sitter-php` |
| Ruby | `tree-sitter-ruby` |
| Kotlin | `tree-sitter-kotlin` |
| Swift | `tree-sitter-swift` |
| Scala | `tree-sitter-scala` |

#### 3.2.4 `--max-tokens` CLI 参数 + MCP token_budget（P2）

**怎么做**：在 `search` 子命令和 MCP `/search` 请求中增加 `max_tokens` 参数。`search_text` 返回结果后，按分数排序往下取，累计 token 数（用近似字符/2.5 估算）不超过配额。

**MCP 接口**：
```json
POST /search {"query":"...", "max_tokens": 2048, ...}
```

**CLI**：`sts-x search "query" --max-tokens 2048`

#### 3.2.5 二进制瘦身（P3）

**现状**：release 二进制 ~17.86 MB（strip 后）。
**方案**：
- 确认 `Cargo.toml` profile 已有 `lto=true` `codegen-units=1` `strip="symbols"` `panic="abort"`
- 可选：UPX 压缩（macOS arm64 支持？实测 win 可压到 ~7MB）
- 可选：按 feature 切分（不带 MCP server 的纯 CLI 版本）

**目标**：strip 后 ≤15MB，UPX 后 ≤8MB。

---

## 4. 接口契约

### 4.1 保持向后兼容

**不破的接口**：
- 所有 CLI 子命令（`search` / `file` / `serve` / `index` / `status`）
- 所有 CLI flags（`--path` / `--locate` / `--expand` / `-f` / `-t` / `-c`）
- MCP `/search` 和 `/file` 路由
- MCP 请求格式（`query`, `mode`, `output_mode`, `top_k`, `path`）
- 输出 JSON 结构（可追加字段，不改已有字段名）

**新增字段**（向后兼容）：
- MCP `/search` 可选参数 `max_tokens`
- CLI `--max-tokens`
- 输出 JSON 可追加 `definition_boost` 分 / `file_coherence` 标识

### 4.2 索引兼容性

**不重建索引**。Schema 不变（不增不减字段），只改搜索/排序逻辑。现有索引直接可用。

---

## 5. 验收标准

| 编号 | 验收项 | 目标值 | 测试方法 |
|------|-------|--------|---------|
| A1 | 备份文件过滤 | `search "read_dir"` 不返回 `main_backup*` | 在 StarTap 项目上跑 `file "read_dir"` |
| A2 | 定义优先排序 | `search "is_supported_image"` 第一条是 `fn is_supported_image` 定义块 | `--expand -t 5` 输出第一条的 kind/name |
| A3 | 5+ 新语言 | 每种语言至少一个正确解析的 AST 块 | 各写一个测试文件，跑 `index` + `search` |
| A4 | `--max-tokens` | `search "fn" --max-tokens 500` 输出 ≤500 tok | 用 tiktoken 计量 |
| A5 | 回归：特殊字符 | `Cli::parse` / `files.len` / `Vec<PathBuf>` 命中 > 0 | 同 §8.1 方法 |
| A6 | 回归：下划线 | `select_best_cfg` / `is_supported_image` 命中 > 0 | 同 §8.1 方法 |
| A7 | 回归：多词 | `class QwenVLClient` / `def decide` 命中 > 0 | 同 §8.1 方法 |
| A8 | 二进制尺寸 | strip 后 ≤15 MB | `ls -la` + `file` |
| A9 | 跨平台构建 | mac arm64 + win x86-64 静态 CRT 正常编译 | `cargo build --release` + `cargo xwin` |
| A10 | 三组白皮书 §8 查询 | `--locate` ≤200 tok, `--expand` 省≥80% | 同 §8.1 |

---

## 6. 实施分步

### Step 1：修复剩余 bug（1 session）
- 噪声文件过滤（~20 行）
- 发布 v3.0.1（含本次两个修复 + 备份过滤）

### Step 2：定义优先排序（1 session）
- `definition_boost` 叠加逻辑（~50 行）
- 验证：搜 `is_supported_image` 定义块排第一

### Step 3：新增语言（0.5–1 session）
- 5 个 tree-sitter grammar crate 入 `Cargo.toml`
- 扩展 `Language` enum 和映射表
- 快速验证每种语言至少解析一个函数

### Step 4：`--max-tokens`（0.5 session）
- CLI flag + MCP 参数
- 输出截断逻辑

### Step 5：二进制瘦身 + 跨平台发版（1 session）
- UPX 压缩测试
- 跨平台构建 + release 发版

### Step 6：README 更新（0.5 session）
- 新增 `--max-tokens` 文档
- 新增语言支持列表
- 性能对比数据

**总计**：4–6 session（约 2–3 天）

---

## 7. 不做的功能（明确边界）

| 功能 | 反面理由 |
|------|---------|
| ONNX embedding + BGE reranker | `--features semantic` 已存在但实测没用过；依赖 ~50MB+ onnx 模型，炸体积 |
| 内置 AI Agent（类似 Probe Agent） | 偏离"小巧工具"定位；AI 接入通过 MCP 已足够 |
| 自然语言→代码搜索（类似 Semble 语义） | 需要 embedding 模型，炸体积 + 增加 Python 依赖 |
| Web UI / IDE 插件 | 不是本工具定位；MCP 已对接所有 IDE |
| 分布式搜索 / 跨仓库搜索 | 单目录工具定位，跨仓库功能过于复杂 |
| 移除 `file` 模式（全面走索引） | 零索引是核心差异，不能丢 |

---

## 8. 决策记录

| 决策 | 选择 | 理由 |
|------|------|------|
| 版本号 | v3.1.0（非 v4.0） | 不破接口，不是大重构 |
| `_` 分词 | 对齐 SimpleTokenizer（拆分 `_`） | 索引侧就是这么存的，必须对齐 |
| 噪声过滤 | `is_noise_file` 函数（~20 行） | 简单直接，无需额外配置文件 |
| 定义优先 | BM25 分 + 叠加分 | 不改索引不升依赖 |
| 语义检索 | ❌ 不做 | embedding 模型炸体积 + Python 依赖，与"小巧"矛盾 |
| 内置 Agent | ❌ 不做 | MCP 已是 AI 接口标准，无需重新造 |

---

## 9. 下一阶段（v4.0 可能性 — 仅备忘，不排期）

- 语义信号（identifier stem / file coherence / noise penalty）
- `-H` 人类友好模式的 i18n
- 热键 `--watch` 自动重索引
- 全量 file 搜索异步化
- Apple Silicon NE 加速
