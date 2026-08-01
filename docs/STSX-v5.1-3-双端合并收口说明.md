# STS-X v5.1-3 双端合并收口（Win 侧推送）

> 时间：2026-08-01 09:11（Win 侧推送）
> 状态：**双端同源收口** —— Mac 侧已同步编译部署，Win 侧 v5.1-3 已部署，两端合并确认

## 双端状态

| 端 | 版本 | 二进制 | MCP | 说明 |
|---|---|---|---|---|
| Win | 3.2.0 v5.1-3 | `C:\tools\sts-x.exe`（31.4MB） | 原生 stdio `sts-x mcp -p f:/trae-cn` | 本机编译部署 ✅ |
| Mac | 3.2.0 v5.1-3 | `/usr/local/bin/sts-x` | 原生 stdio `sts-x mcp -p <项目根>` | 用户确认已同步 ✅ |

## v5.1-3 核心变更（两边都要一致）

1. **有命中砍 `_ai_instructions`**（741 字指南不再附，仅 0 命中给中/英自救）——expand 省 38%、file 省 27%
2. **`search_time_ms` 移到 JSON 末尾**——波动字段不打断 LLM 前缀缓存（DeepSeek 0.02¥/M 档）
3. **输出字节级稳定**——同参数重复查询一致，利于缓存
4. **`--max-tokens` 中文加权**（CJK 按 1.5/字符）——中文项目预算不超量
5. v5.1-2 已修：根探测爬根（package.json 需 node_modules 配套 + 深度上限 8）
6. v5.1-1 已修：path-filter locate 失效（live-grep 兜底遵守过滤）

## 本包内容（3 个文件）

- `STSX-v5.1-3-AGENTS.md` — AI 接口契约（v5.1-3，双端一致版本）
- `STSX-v5.1-3-更新说明.md` — 变更明细 + 安装方法
- `STSX-v5.1-3-使用说明.txt` — 人类/小白说明

## 合并动作（Mac 侧确认）

- 若 Mac 侧 AGENTS.md / 文档还是 v5.1-2 或更早，请用本包 3 个文件覆盖更新
- 确认 `sts-x --version` 为 3.2.0，且 expand 有命中输出**不含** `_ai_instructions` 字段
- 验收命令：
  ```bash
  sts-x search "McpServer" -p <项目> --expand | grep -c _ai_instructions  # 应为 0
  sts-x search "zzqq_nonsense_xyw98" -p <项目> --expand | grep -c "No direct match"  # 应为 1
  ```

## 下一步

两端合并后即毕业收口。后续版本演进以源码包为准，双端同源编译。
