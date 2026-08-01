# sts-x v5.1 Windows 侧 bat / PowerShell 适配补充（Win→Mac 收口用）

> 配套 `v5.1-migration-note.md`。Mac 端 note 已覆盖 Python/Rust/JS/jq 示例，本文件补 **Windows 原生场景**：
> bat 没有原生 JSON 解析，PowerShell 是首选；jq 本机已装但非系统默认。
> 核心风险同源：**`_ai_instructions` / `hint` 为 `Option<String>`，`None` 时 key 直接从 JSON 消失**，硬取值必崩。

---

## 一、PowerShell（推荐，Windows 原生，无额外依赖）

```powershell
# 调用 sts-x 取 JSON（--no-hint 可省 token，但演示保留默认）
$resp = & C:\tools\sts-x.exe search "main" -p F:\trae-cn --expand --json 2>$null

# 安全解析：None 时 key 不存在 -> ConvertFrom-Json 后属性为 $null
$j = $resp | ConvertFrom-Json
$instr = if ($null -ne $j._ai_instructions) { $j._ai_instructions } else { "" }
$hint  = if ($null -ne $j.hint) { $j.hint } else { "" }

# 用法：拼接进提示词 / 写文件
if ($instr) { "AI 指令: $instr" | Out-File -Append guide.txt -Encoding utf8 }
```

⚠️ 坑：`ConvertFrom-Json` 对超大 JSON（>2MB）在 PS 5.1 有深度限制，可加 `-Depth 20`；本机 sts-x 输出通常很小，无碍。

---

## 二、bat 里调 PowerShell 内联（无 jq 时的标准做法）

```bat
@echo off
set "QRY=main"
set "PROJ=F:\trae-cn"
for /f "usebackq delims=" %%o in (` ^
  powershell -NoProfile -Command " ^
    $r = & C:\tools\sts-x.exe search '%QRY%' -p '%PROJ%' --expand --json 2>$null ^| ConvertFrom-Json; ^
    $i = if($null -ne $r._ai_instructions){$r._ai_instructions}else{''}; ^
    $h = if($null -ne $r.hint){$r.hint}else{''}; ^
    Write-Output ('INSTR=' + $i.Length + ' HINT=' + $h.Length) ^
  " ^
`) do set "RESULT=%%o"
echo %RESULT%
```

要点：
- bat 不能直接解析 JSON，**一律委托 PowerShell `-Command` 内联**。
- `^|`（caret+pipe）是 bat 里转义的管道，写在一行或续行都要 care。
- 别用 `for /f "tokens=..."` 直接 cut 文本取字段——中文/空格会截断，交给 PowerShell。

---

## 三、bat 里用 jq（本机已装时，最简洁）

```bat
@echo off
set "RESP=%TEMP%\stsx.json"
C:\tools\sts-x.exe search "main" -p F:\trae-cn --expand --json > "%RESP%" 2>nul

:: jq // "" 保证 None(key 消失) 时返回空串，exit 0 不崩
for /f "usebackq delims=" %%i in (`jq -r "._ai_instructions // empty" "%RESP%"`) do set "INSTR=%%i"
for /f "usebackq delims=" %%h in (`jq -r ".hint // empty" "%RESP%"`) do set "HINT=%%h"

if defined INSTR echo AI指令长度=%INSTR%
if defined HINT echo 自救提示=%HINT%
```

⚠️ 坑：jq 在本机 `C:\tools` 或 scoop 里，但**不是 Windows 系统默认组件**。跨机器分发的 bat 不能假设 jq 存在——优先用方案二（纯 PowerShell）。

---

## 四、WorkBuddy / agent 调用侧（我这边已落地，供参考）

我（Win 端 WorkBuddy）调 `sts-x` 走 Rust 原生 stdio MCP，反序列化用 `serde_json::Value` + `.get(key).and_then(|v| v.as_str()).unwrap_or("")` —— 已天然 null 安全。0 命中自救链读 `hint` 字段，无 `_ai_instructions` 硬取值。无需改动。

---

## 五、验证清单（Windows 侧追加项）

- [ ] `sts-x search <词> -p <proj> --expand --json` 后，PowerShell `ConvertFrom-Json` 不报错 ✓
- [ ] 0 命中时，`$j.hint` 非空（自救串）、`$j._ai_instructions` 仍非空（AI_HINT 指南）✓
- [ ] locate 模式任意查询：`$j._ai_instructions` 为 `$null`（字段不存在，不崩）✓
- [ ] `jq -r '._ai_instructions // ""'` 在 None 时输出空串且 exit 0 ✓
- [ ] bat 内 `for /f` 不做字符串 cut 取字段，全委托 PowerShell ✓

---

*Win 端 AI 起草（2026-08-01 01:30）。与 Mac 端 `v5.1-migration-note.md` 配套，两边各 commit 一份收口。*
