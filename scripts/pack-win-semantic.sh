#!/usr/bin/env bash
# pack-win-semantic.sh — sts-x Windows 语义版打包（Mac 上交叉编译后使用）
#
# 产出：dist/sts-x-win-x64-semantic-v3.2.0.zip
#   内容：
#     sts-x.exe                        （semantic 构建）
#     lib/onnxruntime.dll              （ORT 动态库，nuget/官方 zip 提取）
#     models/bge-small-zh-v1.5/        （中文 embedding 模型 24MB）
#     首次打开必看.txt                 （部署说明）
#
# 前置：
#   1. cargo xwin build --release --target x86_64-pc-windows-msvc --features semantic
#   2. onnxruntime.dll 已就位（本脚本自动从 _scratch/win-dll/*.nupkg 提取，或手动放 _scratch/win-dll/）
#
# 用法：./scripts/pack-win-semantic.sh

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$SCRIPT_DIR"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')"
EXE="target/x86_64-pc-windows-msvc/release/sts-x.exe"
DLL_SRC="_scratch/win-dll/onnxruntime.dll"
MODEL_DIR="models/bge-small-zh-v1.5"
OUT_DIR="dist"
ZIP_NAME="sts-x-win-x64-semantic-v${VERSION}.zip"

echo "=== sts-x Win 语义包打包 (v${VERSION}) ==="
[ -f "$EXE" ] || { echo "❌ 缺 $EXE —— 先跑 cargo xwin build --release --target x86_64-pc-windows-msvc --features semantic"; exit 1; }
[ -f "$MODEL_DIR/model.onnx" ] && [ -f "$MODEL_DIR/tokenizer.json" ] || { echo "❌ 缺模型 $MODEL_DIR"; exit 1; }

# ── 提取 onnxruntime.dll ──
if [ ! -f "$DLL_SRC" ]; then
    echo "提取 onnxruntime.dll 从 nupkg..."
    NUPKG=$(ls _scratch/win-dll/*.nupkg 2>/dev/null | head -1 || true)
    [ -n "$NUPKG" ] || { echo "❌ 缺 nupkg（_scratch/win-dll/）—— 手动放 onnxruntime.dll 到该目录"; exit 1; }
    rm -rf _scratch/win-dll/x && mkdir -p _scratch/win-dll/x
    unzip -q -o "$NUPKG" -d _scratch/win-dll/x
    DLL=$(find _scratch/win-dll/x -name "onnxruntime.dll" | head -1)
    [ -n "$DLL" ] || { echo "❌ nupkg 里没找到 onnxruntime.dll"; exit 1; }
    cp "$DLL" "$DLL_SRC"
    echo "  ✅ onnxruntime.dll ($(ls -lh "$DLL_SRC" | awk '{print $5}'))"
fi

# ── 组装打包目录 ──
mkdir -p "$OUT_DIR/pkg/lib" "$OUT_DIR/pkg/models"
cp "$EXE" "$OUT_DIR/pkg/sts-x.exe"
cp "$DLL_SRC" "$OUT_DIR/pkg/lib/"
cp -R "$MODEL_DIR" "$OUT_DIR/pkg/models/"

cat > "$OUT_DIR/pkg/首次打开必看.txt" <<'TXT'
STS-X v3.2.0 语义检索版（Windows x64）使用说明
================================================

一、这是什么
  sts-x 是给 AI Agent 用的代码+文件搜索引擎（AST 切块 + BM25）。
  本包为「语义检索增强版」：内置中文 embedding 模型（bge-small-zh-v1.5），
  中文自然语言描述也能命中英文代码（如「缓存目录在哪里」→ cache.rs）。

二、部署
  1. 把整个文件夹解压到任意目录（如 C:\tools\sts-x）
  2. 添加环境变量：
     STX_SEMANTIC=1        （开启语义检索；不设则走 BM25+自动重试，无需模型）
     STX_MODEL_DIR=...     （可选；默认自动找 exe 同目录 models/）
  3. 确认目录结构：
     C:\tools\sts-x\
     ├─ sts-x.exe
     ├─ lib\onnxruntime.dll     ← ORT 动态库，必须与 exe 同级 lib 下
     └─ models\bge-small-zh-v1.5\model.onnx + tokenizer.json

三、使用
  sts-x ai "缓存目录在哪里" -p C:\你的项目
  sts-x search "McpServer" -p C:\你的项目 --locate
  sts-x mcp -p C:\你的项目        （MCP stdio 服务，配 WorkBuddy/Claude）

四、常见问题
  - 提示缺 onnxruntime.dll / 语义自动降级：确认 lib\onnxruntime.dll 在 exe 同目录的 lib\ 下
    （或把 dll 直接放 exe 同目录）。缺 dll 时程序自动退回 BM25，不会崩溃。
  - 首次搜索会自动建索引（缓存到 %LOCALAPPDATA%\sts-x\cache），稍等片刻。
  - MCP 接入：command 指向 sts-x.exe，args 填 ["mcp","-p","<项目根>"]。

五、版本
  sts-x v3.2.0（semantic）· bge-small-zh-v1.5 · onnxruntime 1.28.0 · MIT License
TXT

# ── 打包（Python zipfile 保证中文名 UTF-8）──
cd "$OUT_DIR/pkg"
/Users/xtap/.workbuddy/binaries/python/versions/3.13.12/bin/python3 - "$ZIP_NAME" <<'PY'
import zipfile, os, sys
out = sys.argv[1]
with zipfile.ZipFile(f"../{out}", "w", zipfile.ZIP_DEFLATED) as z:
    for root, dirs, files in os.walk("."):
        for f in files:
            p = os.path.join(root, f)
            z.write(p, p[2:] if p.startswith("./") else p)
print(f"✅ {out}")
PY
cd "$SCRIPT_DIR"
rm -rf "$OUT_DIR/pkg"
ls -lh "$OUT_DIR/$ZIP_NAME"
echo "=== 完成: $OUT_DIR/$ZIP_NAME ==="
