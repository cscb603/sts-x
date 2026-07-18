<p align="center">
  <img src="assets/icon.png" width="96" alt="STS-X">
</p>

<h1 align="center">STS-X</h1>
<p align="center">
  <strong>为 AI Agent 而生的「代码 + 文件」统一搜索引擎</strong><br>
  AST感知切块 · BM25极速全文搜索 · MCP原生协议 · 18MB零依赖<br>
  <em>二合一：代码搜索（locate 行级 / expand 整块）+ 任意目录零索引文件搜索</em><br>
  <em>深层任务省约 80% token（约为 grep+Read 流程的 1/5）</em>
</p>

<p align="center">
  <a href="https://github.com/cscb603/sts-x/releases">
    <img src="https://img.shields.io/github/v/release/cscb603/sts-x?label=版本&color=4F46E5" alt="版本 3.0.0">
  </a>
  <img src="https://img.shields.io/badge/大小-18MB-10B981" alt="大小">
  <img src="https://img.shields.io/badge/定位-为_AI_而生-4F46E5" alt="定位">
  <img src="https://img.shields.io/badge/AI场景-省~80%25_token-10B981" alt="省token">
  <img src="https://img.shields.io/badge/延迟-0–2ms-F59E0B" alt="延迟">
  <a href="https://github.com/cscb603/sts-x/blob/main/LICENSE">
    <img src="https://img.shields.io/badge/许可证-MIT-6366F1" alt="许可证">
  </a>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/macOS_ARM64-✅_支持-4F46E5?logo=apple">
  <img src="https://img.shields.io/badge/Windows_x86_64-✅_支持-2563EB?logo=windows">
  <img src="https://img.shields.io/badge/Linux_x86_64-✅_源码构建-9CA3AF?logo=linux">
</p>

<p align="center">
  <a href="#快速开始">快速开始</a> ·
  <a href="#ai-原生设计">AI原生设计</a> ·
  <a href="#省-tokenai-场景实测">省 Token</a> ·
  <a href="#mcp-服务">MCP服务</a> ·
  <a href="#用例">用例</a> ·
  <a href="#技术规格">技术规格</a> ·
  <a href="#安装与分发">安装与分发</a> ·
  <a href="#构建">构建</a>
</p>

---

STS-X 是一个**面向 AI Agent 的代码搜索引擎**，专为大模型时代设计。与 IDE 内置搜索或 grep/ripgrep 等传统工具不同，STS-X 从内核就围绕 AI 的使用场景构建：默认输出结构化 JSON、内置 MCP HTTP 服务供 AI Agent 调用、基于 AST 语法树切块返回完整的函数/类代码块而非零散行。

### 它解决什么问题？

| 场景 | 传统工具 | STS-X |
|------|---------|-------|
| AI Agent 搜索代码 | 返回纯文本，AI 需要自己解析 | 返回结构化 JSON，字段清晰，自带 `_ai_instructions` 使用指南 |
| 需要完整函数上下文 | grep 返回零散行，看不懂 | AST 感知切块，按函数/类/方法返回完整代码块 |
| 集成到 AI 工作流 | 需要写脚本解析输出 | 内置 MCP HTTP 服务，GET /tools 自动发现，POST /search 即用 |
| 跨平台部署 | 需要安装运行时（Python/Node） | 18MB 单二进制，零依赖，下载即用 |
| 索引管理 | 手动创建、手动更新 | 自动索引、自动缓存、文件变更自动重建 |
| Windows 中文路径 | 乱码/编码问题 | 内建 POSIX 路径归一化，原生兼容 |
| 任意目录找文件/内容 | 要先 `cd` 进项目、建索引 | `file` 子命令零索引：文件名+内容，rg 优先（无索引目录如 ~/Downloads 直接搜） |

---

## 快速开始

### 一分钟上手

```bash
# 1. 下载（macOS）
curl -L https://github.com/cscb603/sts-x/releases/latest/download/sts-x -o sts-x && chmod +x sts-x
sudo mv sts-x /usr/local/bin/

# 2. 进入任意项目目录，直接搜索（自动索引、无需额外步骤）
cd /your-project
sts-x search "token verification"

# 3. 搜索文件名
sts-x search "config" -f

# 4. 任意目录零索引文件搜索（无需进项目、无需建索引）
sts-x file "invoice" --path ~/Downloads

# 5. 行级定位（先看在哪，再决定要不要读全块）
sts-x search "select_best_cfg" --locate

# 6. 人类友好模式
sts-x search "token verification" -H
```

```powershell
# Windows（PowerShell）
curl -L https://github.com/cscb603/sts-x/releases/latest/download/sts-x.exe -o sts-x.exe
.\sts-x.exe search "token verification"
```

---

## AI 原生设计

STS-X 从架构设计之初就面向 AI，而非事后适配。

### 自说明响应

每个搜索响应自动携带 `_ai_instructions` 字段，包含完整的 STS-X 使用指南、参数说明、MCP 端点用法。**AI Agent 只需一次调用即可完全掌握 STS-X 的全部能力**，无需查阅外部文档。

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
      "code": "pub fn verify_token(token: &str, secret: &[u8]) -> Result<Claims> { ... }"
    }
  ],
  "total_hits": 5,
  "search_time_ms": 1,
  "_ai_instructions": "STS-X is an AI code search engine..."
}
```

### 智能默认值（v3.0.0）

| 参数 | 默认值 | 设计理由 |
|------|--------|---------|
| `top_k` | **2** | 返回最相关结果，AI 场景下节省大量 token |
| `context_lines` | **0** | expand 模式默认返回**完整 AST 块**（函数/类），而非截断窗口 |
| 输出格式 | **JSON** | AI 最擅长的结构化数据格式 |
| `output_mode` | **expand** | 默认给完整代码块（读懂/修改）；`--locate` 切换为行级 grep 尺寸（~80–130 tok）用于先定位 |

### 代码后处理引擎

- **高亮行标注**：`highlight_lines` 字段精确标注查询词在代码块中的行号，AI 可直接定位关键代码
- **上下文控制**：通过 `--context N` 参数灵活控制返回行数，N=0 时返回完整代码块

---

## 省 Token：AI 场景实测（真金白银）

> **一句话定位：STS-X 是给 AI 用的搜索工具。** 在"读懂 / 改代码"这类深层任务上，传统做法是 `grep` 找到文件 → 再 `Read` 整个文件 → 可能还要再搜一次。STS-X 一次就把"最相关的完整函数/类 + 上下文"给齐，token 消耗**直接砍掉一大截**，还少 2–3 轮往返——这省的是实打实的大模型 API 输入费。

**实测方法**：在 `retouch_app` 项目里，对 3 个真实查询（`select_best_cfg`、`class QwenVLClient`、`def decide`），用 `tiktoken`(cl100k_base，和 GPT 系列同款分词器) 分别计量 `grep`+Read 流程 vs `sts-x` 的总 token。

| 对比项 | grep + Read 流程 | sts-x（expand 整块, top_k=1） | 省多少 |
|------|----------------|-------------------------------|--------|
| 三查询合计 token | **22,961** | **1,796** | **↓ 92.2%** |
| 调用轮次 | 2–3 轮 | 1 轮 | 少 2 轮 |

**行级定位档（locate）更夸张**：单次只回命中行 + 短上下文，三个查询分别 136 / 120 / 122 token（平均 ~126，**严格 ≤200**），比 `grep`+`Read` 整文件省 **98%+**。AI 想先确认"符号在哪"时用它，几乎零 token 开销。

要点（说人话）：

- **浅层任务**（只想知道"哪一行有这符号"）：`--locate` 档最合适，单次才百来 token，比整块读还省。
- **深层任务**（读懂 / 修改 / 定位实现）：`expand` 档一次给完整代码块，免去 AI 再 `Read` 整文件，**综合省 ~80–92% token**；结果自带相关度排序、高亮行、函数签名，AI 拿来就能用。
- **代价**：单次原始输出比单行 grep 大（因为给的是整块），这是"一次给全"的交换。但总成本远低于"先 grep 再 Read 整个大文件"。
- 上面是真实分词器实测，不是估算；"深层任务省一大截、浅层任务用 locate 更省"的结论稳定可复现。

### 双档输出：先 `locate` 定位，再 `expand` 读全块

STS-X 3.0 引入**渐进式披露**，让 AI 自己决定要不要"读全块"：

- **`--locate`（默认先定位）**：只返回命中行 + 短上下文（grep 尺寸，单次 **≤200 tok**，远低于 grep+Read）。AI 先看清"符号在哪个文件哪一行"，大部分"在哪"类问题这一档就够，几乎零 token 开销。
- **`--expand`（需要再读全块）**：返回完整 AST 代码块（函数/类），用于"读懂/改"深层任务，综合省 60–80% token。

```bash
# 第一档：极便宜地确认位置（~80–130 tok）
sts-x search "select_best_cfg" --locate
# → {"matches":[{"file":"retouch_app/decide.py","line":112,"context":"sel = G.select_best_cfg(cfg, mix, cur_cfg, base_…"}]}

# 第二档：确认要看全块时，再取完整函数（一次给齐，免再 Read）
sts-x search "select_best_cfg" --expand -t 1
```

`MCP /search` 同样用 `output_mode: "locate" | "expand"` 切换；`locate` 单独返回 `matches`（无 `_ai_instructions`、无整块），刻意压到最小。

---

## MCP 服务

STS-X 内置完整的 MCP（Model Context Protocol）HTTP 服务，专为 AI Agent 集成设计。

### 端点总览

| 方法 | 路径 | 用途 | AI Agent 使用场景 |
|------|------|------|-----------------|
| `GET` | `/` | 服务文档 + curl 示例 | AI 探索能力时获取帮助 |
| `GET` | `/health` | 健康检查 | 确认服务可用性 |
| `GET` | `/tools` | MCP 工具发现 | **自动发现搜索能力**，返回标准 MCP Tool Schema |
| `POST` | `/search` | 执行搜索 | **核心搜索接口**，支持所有搜索模式 + `output_mode` |
| `POST` | `/file` | 文件搜索（零索引） | 任意目录的文件名/内容搜索，`{"path":"/abs/dir"}` 指定目录 |

### 工具发现（AI 无需预配置）

```bash
# AI Agent 自动发现 STS-X 的全部搜索能力
curl http://127.0.0.1:9876/tools
# 返回 MCP 标准格式，包含参数名、类型、描述、默认值
```

### AI Agent 调用示例

```bash
# 代码搜索（默认）
curl -X POST http://127.0.0.1:9876/search \
  -H "Content-Type: application/json" \
  -d '{"query":"error handling","top_k":3}'

# 文件名搜索
curl -X POST http://127.0.0.1:9876/search \
  -H "Content-Type: application/json" \
  -d '{"query":"config","filename":true}'

# 指定项目路径
curl -X POST http://127.0.0.1:9876/search \
  -H "Content-Type: application/json" \
  -d '{"query":"database","path":"/path/to/project"}'

# locate 行级（便宜先定位）
curl -X POST http://127.0.0.1:9876/search \
  -H "Content-Type: application/json" \
  -d '{"query":"select_best_cfg","output_mode":"locate","top_k":1}'

# 任意目录零索引文件搜索（如 ~/Downloads）
curl -X POST http://127.0.0.1:9876/file \
  -H "Content-Type: application/json" \
  -d '{"query":"invoice","path":"/Users/me/Downloads","content":true,"top_k":10}'
```

---

## 搜索模式

| 模式 | CLI 命令 | MCP 参数 | 搜索范围 | 输出类型 |
|------|----------|---------|---------|---------|
| **Code** | `search "query"` | `{"query":"..."}` | 代码内容 | AST 切块（函数/类）；`--locate` 行级 / `--expand` 整块 |
| **Filename** | `search "query" -f` | `{"query":"...","filename":true}` | 文件名 | 匹配的文件路径 |
| **All** | `search "query" --all` | `{"query":"...","all":true}` | 所有文件内容 | 代码 + 文本 + 配置 |
| **File** | `file "query" [--path DIR]` | `{"query":"...","path":"/abs/dir","content":true}` | **任意目录**（零索引） | 文件名 + 内容行（rg 优先） |

---

## 用例

### 1. AI 编程助手集成

Cursor、Windsurf、VS Code Copilot 等 AI 编程工具在执行代码搜索时，可直接调用 STS-X MCP 服务获取结构化结果，替代传统的 grep/ripgrep。

### 2. 代码库理解与迁移

```bash
# 快速理解项目中所有数据库操作
sts-x search "INSERT INTO|SELECT.*FROM"

# 定位所有错误处理逻辑
sts-x search "Error|Result<" --context 0

# 搜索某函数的完整实现
sts-x search "fn authenticate" --context 0
```

### 3. 自动化 CI/CD

```bash
# 检查是否所有 TODO/FIXME 都已处理
sts-x search "TODO|FIXME|HACK" --all -H

# 检查敏感信息是否泄露
sts-x search "password|secret_key|api_key"
```

---

## 技术规格

| 项目 | 详情 |
|------|------|
| **版本** | v3.0.0 |
| **二进制大小** | macOS 18MB / Windows 18MB（strip 后） |
| **搜索延迟** | 0–2ms（千级文件） |
| **索引引擎** | Tantivy BM25（自定义 code 分词器） |
| **AST 解析** | tree-sitter（9 种语言） |
| **MCP 服务** | axum HTTP，RESTful 设计（`/search` + `/file` 双工具） |
| **外部依赖** | 零 |
| **默认输出** | JSON（AI 原生格式） |
| **支持语言** | Rust · Python · JavaScript · TypeScript · TSX · Java · C · C++ · Go |
| **输出模式** | `expand`（完整 AST 块）/ `locate`（行级 grep 尺寸，≤200 tok） |
| **文件搜索** | `file` 子命令 / `/file` 工具，零索引，rg 优先（任意目录） |
| **响应字段** | score · path · lines · highlight_lines · kind · name · signature · language · code · _ai_instructions |
| **索引存储** | 系统缓存目录（不污染项目，自动重建） |
| **可选增强** | ONNX embedding + BGE Reranker（`--features semantic`） |
| **平台** | macOS ARM64 · Windows x86_64（静态 CRT）· Linux x86_64（源码构建） |
| **许可证** | MIT |

---

## 安装与分发

### macOS

- 从 Release 下载 `sts-x` 二进制，放到 `/usr/local/bin/` 并 `chmod +x`。
- **若从浏览器 / AirDrop 下载被 Gatekeeper 拦截**（"无法验证开发者"）：
  - 人类使用：在 Finder 里对 `sts-x` **右键 → 打开**，弹窗再点【打开】，一次后永久放行；
  - 或终端一行解除隔离：`xattr -dr com.apple.quarantine /usr/local/bin/sts-x`。
- **给 AI 助手用**（WorkBuddy / TRAE / Cursor 等）：放进 PATH 后解除一次 quarantine，AI 即可直接调用 `sts-x` 命令处理任意项目目录——**AI 自己解析文件路径，无需人类右键打开**（右键打开是给人类 GUI 程序的，CLI 工具被 AI 调用时走终端，不受 GUI 拦截影响）。
- 注意：当前仅提供 Apple Silicon (arm64) 版本，Intel Mac 需自行从源码编译。

### Windows

- 下载 `sts-x.exe`（MSVC + **静态 CRT** 编译，双击即跑，无需安装 VC++ 运行库）。

---

## 构建

```bash
# 默认构建（BM25 模式，约 18MB macOS / 20MB Windows）
cargo build --release

# 语义搜索增强（ONNX embedding，约 30MB）
cargo build --release --features semantic

# macOS .app 包
bash scripts/build.sh mac

# Windows 交叉编译（macOS → Windows，cargo-xwin + MSVC 静态 CRT）
# 需先安装: cargo install cargo-xwin
# .cargo/config.toml 已配置 +crt-static（静态链接 CRT，用户机无需装 VC++ 运行库）
EMBED_RESOURCE_LLVM_RC=1 cargo xwin build --release --target x86_64-pc-windows-msvc
```

### Linux（x86_64）

本项目在 macOS 上交叉编译 Windows；**Linux 二进制需在一台 Linux x86_64 主机上原生构建**（本机无 Linux 目标/链接器）：

```bash
# 在 Linux x86_64 主机上
git clone https://github.com/cscb603/sts-x && cd sts-x
cargo build --release            # 产物：target/release/sts-x（ELF x86-64）
file target/release/sts-x        # → ELF 64-bit LSB executable, x86-64
```

### 项目结构

```
sts-x/
├── src/
│   ├── main.rs            # 程序入口
│   ├── lib.rs             # 库入口
│   ├── cli/               # 命令行接口（search/serve/index/status）
│   ├── indexer/           # Tantivy 索引引擎
│   ├── search/            # 搜索管线
│   ├── chunker/           # tree-sitter AST 感知切块
│   ├── embed/             # ONNX embedding（可选特征）
│   ├── server/            # MCP HTTP 服务
│   ├── postprocess.rs     # 代码后处理（高亮行 · 上下文控制）
│   ├── cache.rs           # 跨平台缓存目录管理
│   └── types/             # 数据结构 + AI 输出格式
├── assets/                # 图标资源
├── scripts/               # 构建脚本
└── index.html             # 宣传页
```

---

## 为什么选择 STS-X？

| 对比维度 | grep/ripgrep | IDE 内置搜索 | Everything | **STS-X** |
|---------|-------------|-------------|-----------|----------|
| AI 原生输出 | ❌ 纯文本 | ❌ 纯文本 | ❌ 纯文本 | **✅ 结构化 JSON** |
| MCP 协议 | ❌ | ❌ | ❌ | **✅ 内置** |
| 工具自发现 | ❌ | ❌ | ❌ | **✅ GET /tools** |
| AST 感知切块 | ❌ 零散行 | ❌ 零散行 | ❌ | **✅ 函数/类完整块** |
| 自动索引 | ❌ | ❌ | ❌ | **✅ 系统缓存** |
| 项目根自动探测 | ❌ | ❌ | ❌ | **✅ 13种标记** |
| 跨平台 | ✅ | ❌ | ❌ Windows only | **✅ macOS + Windows** |
| 零依赖单二进制 | ❌ 需系统 | ❌ 需IDE | ❌ 需安装 | **✅ 18MB** |
| 搜索延迟 | 毫秒级 | 秒级 | 毫秒级 | **0–2ms** |

---

## 作者与联系

- **作者**：星TAP实验室软件
- **邮箱**：cscb603@qq.com
- **项目**：https://github.com/cscb603/sts-x
- 免费开源，欢迎集成到任何 AI Agent / 编码工具；bug 反馈、功能建议、合作均可来信。

---

## 许可证

MIT License. Copyright © 2026 星TAP实验室软件 &lt;cscb603@qq.com&gt;
