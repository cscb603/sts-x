#!/usr/bin/env python3
# pack-win-semantic-332.py — sts-x v3.3.2 Windows 语义版打包
# 输入：target/x86_64-pc-windows-msvc/release/sts-x.exe（semantic 构建）
#      + _scratch/win-dll/onnxruntime.dll + models/bge-small-zh-v1.5/
# 输出：dist/sts-x-win-x64-semantic-v3.3.2.zip
import zipfile, os, shutil, sys

ROOT = "/Users/xtap/Documents/AI/sts-x"
VERSION = "3.3.2"
EXE = f"{ROOT}/target/x86_64-pc-windows-msvc/release/sts-x.exe"
DLL = f"{ROOT}/_scratch/win-dll/onnxruntime.dll"
MODEL = f"{ROOT}/models/bge-small-zh-v1.5"
OUT = f"{ROOT}/dist/sts-x-win-x64-semantic-v{VERSION}.zip"
PKG = f"{ROOT}/dist/pkg_win_sem"

# 前置检查
for p in (EXE, DLL, f"{MODEL}/model.onnx", f"{MODEL}/tokenizer.json"):
    if not os.path.isfile(p):
        print(f"❌ 缺 {p}"); sys.exit(1)

# 重建打包目录
shutil.rmtree(PKG, ignore_errors=True)
os.makedirs(f"{PKG}/lib")
os.makedirs(f"{PKG}/models/bge-small-zh-v1.5")
shutil.copy(EXE, f"{PKG}/sts-x.exe")
shutil.copy(DLL, f"{PKG}/lib/onnxruntime.dll")
shutil.copy(f"{MODEL}/model.onnx", f"{PKG}/models/bge-small-zh-v1.5/model.onnx")
shutil.copy(f"{MODEL}/tokenizer.json", f"{PKG}/models/bge-small-zh-v1.5/tokenizer.json")

note = f"""首次打开必看 - sts-x {VERSION}（Windows 64 位 语义检索版）

一、这是什么
  sts-x 是给 AI Agent 用的代码+文件搜索引擎（AST 切块 + BM25）。
  本包是「语义检索增强版」：内置中文 embedding 模型（bge-small-zh-v1.5），
  中文自然语言描述也能命中英文代码（如「缓存目录在哪里」→ cache.rs）。
  v{VERSION} 新增 glob 子命令：按模式列文件（替代手写 Glob），AI 友好输出。

二、部署
  1. 把整个文件夹解压到任意目录（如 C:\\tools\\sts-x）
  2. 添加环境变量：
     STX_SEMANTIC=1        （开启语义检索；不设则走 BM25+自动重试，无需模型）
     STX_MODEL_DIR=...     （可选；默认自动找 exe 同目录 models/）
  3. 确认目录结构：
     C:\\tools\\sts-x\\
     |- sts-x.exe
     |- lib\\onnxruntime.dll     <- ORT 动态库，必须与 exe 同级 lib 下
     `- models\\bge-small-zh-v1.5\\model.onnx + tokenizer.json

三、使用
  sts-x ai "缓存目录在哪里" -p C:\\你的项目
  sts-x search "McpServer" -p C:\\你的项目 --locate
  sts-x glob "*.rs,*.toml" -p C:\\你的项目   （按模式找文件，替代 Glob）
  sts-x mcp -p C:\\你的项目        （MCP stdio 服务，配 WorkBuddy/Claude）

四、常见问题
  - 提示缺 onnxruntime.dll / 语义自动降级：确认 lib\\onnxruntime.dll 在 exe 同目录 lib 下
    （或把 dll 直接放 exe 同目录）。缺 dll 时程序自动退回 BM25，不会崩溃。
  - 首次搜索会自动建索引（缓存到 %LOCALAPPDATA%\\sts-x\\cache），稍等片刻。
  - MCP 接入：command 指向 sts-x.exe，args 填 ["mcp","-p","<项目根>"]。

五、版本
  sts-x v{VERSION}（semantic）· bge-small-zh-v1.5 · onnxruntime 1.28.0 · MIT License

祝你用得开心！
"""
with open(f"{PKG}/首次打开必看.txt", "w", encoding="utf-8") as f:
    f.write(note)

# 打包（统一 UTF-8，全部加可读权限）
with zipfile.ZipFile(OUT, "w", zipfile.ZIP_DEFLATED) as z:
    for root, dirs, files in os.walk(PKG):
        for fn in files:
            p = os.path.join(root, fn)
            zi = zipfile.ZipInfo.from_file(p, p[len(PKG)+1:])
            zi.compress_type = zipfile.ZIP_DEFLATED
            zi.external_attr = 0o644 << 16
            with open(p, "rb") as fh:
                z.writestr(zi, fh.read())

shutil.rmtree(PKG, ignore_errors=True)
print(f"✅ {os.path.basename(OUT)}  {os.path.getsize(OUT)/1048576:.1f}MB")
