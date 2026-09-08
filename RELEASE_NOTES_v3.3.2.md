# STS-X v3.3.2 发布说明

> 发布日期：2026-08-27 ｜ 在 v3.3.1（语义检索零配置）基础上的**文件发现增强**
> 目标：让 Agent 用原生的 `glob` 命令 / MCP 工具替代系统 Glob，把"文件列举"也纳入
> 同一套**排序 / 截断 / 诊断**体系（与 `search` / `file` 一致）。

## 新增能力

- **`sts-x glob "<pattern>" -p <dir>` 子命令**：多模式 glob（`**/*.rs`、`*.toml,*.lock`、支持 `!` 否定）。
- **MCP `glob` 工具**（与 `search` / `file` 同契约）：`tools_list()` 现暴露 `search` / `file` / `glob` 三个工具。
- **AI 友好排序**：浅层源码 > 深层；`test`/`vendor`/`target`/`node_modules`/`dist`/`build` 降权；
  `--sort-recent` 按修改时间升权；`--git-aware` 把 git 修改/未跟踪文件排前；隐藏文件降权。
- **截断不静默**：返回 `total_hits` + `returned` + `omitted`，让 Agent 知道是否漏看。
- **0 命中诊断**：扫描项目实际扩展名，给出"项目有 .rs(18)…"式的建议（中/英按查询自动切换）。
- **`--top-k` / `--max-tokens`** 双预算控制，默认 20 / 500；默认尊重 `.gitignore`。

## 底层改动

- `Cargo.toml` 显式补 `globset = "0.4"`（此前仅作为 `ignore` 的传递依赖藏在 lock 里）；
  **未引入任何新真实依赖**，编译体积几乎不变。
- 新增 `src/globsearch/mod.rs`（glob 匹配 + 排序 + 截断 + 诊断 + 单测）、
  `types/format.rs` 新增 `AiGlobOutput` / `AiGlobItem` / `format_glob_human()`。
- `cli/mod.rs` 新增 `Commands::Glob` + `cmd_glob()`；`mcp.rs` 新增 `McpEngine::glob()` 与 `tools/call` 分支。

## 质量门禁

- `cargo fmt --check` / `cargo check --all-targets` / `cargo clippy --all-targets -D warnings` 全绿。
- `cargo test`：48 passed（含 6 个 globsearch 单测 + 2 个 MCP glob 工具 schema 单测）。
- Mac 原生 `cargo build --release` 通过；Windows 交叉编译（`cargo xwin` +crt-static 单文件 exe）按 Phase 4 验证。

## 下一步（未在本期实现，留作迭代）

- Phase 3 可选增强：frecency 缓存（长期项目文件优先级）、与现有 AST 索引联动（含定义文件优先）。
- 如需让 WorkBuddy 默认调用 `glob`：把 `~/.workbuddy/mcp.json` 里 `sts-x` 的二进制指向本版，
  Agent 检索代码时会优先用 `glob` 而非系统 Glob。
