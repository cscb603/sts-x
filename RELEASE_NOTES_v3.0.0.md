# STS-X v3.0.0 — 给 AI Agent 用的「代码 + 文件」统一搜索引擎

> 一个二进制，同时干两件事：**代码搜索**（AST 整块 / 行级定位）+ **任意目录文件搜索**（零索引）。
> 它是为 AI 设计的，不是给人手敲命令用的——所以输出是结构化 JSON、自带 MCP 服务、还会主动告诉你"怎么用我"。

---

## 为什么做这个？

现在的 AI 编程助手搜代码，基本还是 `grep` 找文件 → `Read` 整个文件 → 可能再搜一次。这套流程对 AI 特别费 token：一个大文件动辄上万字符，光读进去就烧掉一堆 API 输入费，还经常读错文件、来回好几句。

STS-X 的出发点很直接：**让 AI 一次就拿对代码，少读废文件，少绕弯子。** 实测下来，深层代码任务 token 能砍掉 **80–92%**，还少 2–3 轮对话。

---

## 真金白银：省多少 token（实测，非估算）

在 `retouch_app` 项目上，对 3 个真实查询用 `tiktoken`(cl100k_base，和 GPT 同款分词) 计量：

| 对比 | grep + Read 流程 | sts-x（expand 整块） | 结果 |
|------|----------------|---------------------|------|
| 三查询总 token | 22,961 | **1,796** | **↓ 92.2%** |
| 对话轮次 | 2–3 轮 | 1 轮 | 少 2 轮 |

**行级定位档（locate）更狠**：单次只回命中行 + 短上下文，平均 ~126 token（严格 ≤200），比 grep+整读省 **98%+**。AI 想先确认"符号在哪"时用它，几乎零开销。

> 这不是 PPT 数字。同样的查询、同样的文件、同一套分词器，可复现。

---

## 核心能力（人话版）

1. **二合一**：代码搜索 + 文件搜索一个工具搞定。不用在 `grep` / `fd` / `rg` / `Everything` 之间切来切去。
2. **AST 感知切块**：返回的是**完整函数 / 类**，不是散落的行。AI 拿到就能懂、就能改。
3. **双档输出，渐进披露**：
   - `--locate`：先看"在哪"（行级，百来 token，极便宜）；
   - `--expand`：确认要看全块时，一次给齐完整代码（省掉再 Read 整文件）。
4. **零索引文件搜索**：`file` 子命令对任意目录（包括 `~/Downloads` 这种没建索引的地方）即时搜文件名 + 内容，rg 优先，零配置。
5. **自带 MCP 服务**：`sts-x serve` 起一个 HTTP 服务，AI Agent 通过 `GET /tools` 自动发现能力，`POST /search`、`POST /file` 直接调。Cursor / CodeBuddy / Claude Code / Aider 这类工具接进去很容易。
6. **自说明响应**：每次搜索结果里带 `_ai_instructions` 字段，AI 一次调用就学会怎么用 STS-X，不用查文档。
7. **零依赖单二进制**：macOS 18MB / Windows 20MB，下载即用；Windows 版静态链接 CRT，**用户机不用装 VC++ 运行库**。

---

## 三平台支持

| 平台 | 状态 | 说明 |
|------|------|------|
| macOS (Apple Silicon) | ✅ 已发布二进制 | arm64，下载即用 |
| Windows (x86_64) | ✅ 已发布二进制 | 静态 CRT，双击即跑，免装 VC++ 运行库 |
| Linux (x86_64) | 🔧 本机构建（一行命令） | 纯 Rust + rg 兜底，**`cargo build --release` 即可**，无任何特殊依赖；交叉编译 musl 静态二进制的工具链在本机环境受限，故不提供预编译包 |

---

## 30 秒上手

```bash
# 进任意项目，直接搜（自动建索引）
cd your-project
sts-x search "token verification"

# 先定位（便宜）
sts-x search "select_best_cfg" --locate

# 再读全块（完整函数）
sts-x search "select_best_cfg" --expand

# 任意目录零索引找文件
sts-x file "invoice" --path ~/Downloads

# 给 AI 用：起 MCP 服务
sts-x serve
```

---

## 这些场景尤其值

- AI 编程助手（Cursor / CodeBuddy / Claude Code / Aider / Windsurf / VS Code Copilot）的代码搜索后端
- 大型代码库理解、重构、迁移前的"先摸清结构"
- CI/CD 里做敏感信息 / TODO / 死代码扫描
- 任何"让 AI 看懂你的仓库但别烧光 token"的需求

---

## 关于作者 & 许可

- **作者**：星TAP实验室软件
- **邮箱**：cscb603@qq.com
- **协议**：MIT（免费、开源、可商用、可集成）
- 不靠这个赚钱，图的是"对 AI 真的有用，大家用着不觉得菜"。欢迎集成、反馈、提建议。

---

## 从 0.2.x 到 3.0 有什么不一样

- **二合一**：原来 `sts`(文件) 和 `sts-x`(代码) 是两个工具，现在合并进一个二进制
- **双档输出**：新增 `locate` / `expand` 渐进披露，比旧版"一股脑给整块"更省
- **零索引 file 模式**：任意目录即时搜，不用进项目、不用建索引
- **MCP 原生**：自带 HTTP 服务，`/search` + `/file` 双工具，AI 即插即用
- **跨平台零依赖**：macOS / Windows 提供预编译二进制（Windows 静态 CRT，免装运行库）；Linux 纯 Rust 一行 `cargo build --release` 即出静态可用二进制
