#!/usr/bin/env bash
# pack-release.sh — sts-x 全平台发布打包（商业级）
#
# 产物（dist/）：
#   STS-X-<V>-Mac版(给AI搜代码).zip          Mac 默认版（单文件，零依赖）
#   STS-X-<V>-Mac版-语义检索(给AI搜代码).zip  Mac 语义版（+ dylib + 模型 + 启动器）
#   STS-X-<V>-Win版(给AI搜代码).zip          Win 默认版（单 exe，crt-static）
#   STS-X-<V>-Win版-语义检索(给AI搜代码).zip  Win 语义版（+ dll + 模型）
#   STS-X-<V>-源码.zip                        源码包（git ls-files，含编译指引）
#
# 铁律：R1 说明四段式 / R2 Python zipfile（中文名 UTF-8）/ R3 crt-static（构建时）
# 用法：先跑完构建矩阵，再 ./scripts/pack-release.sh
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$SCRIPT_DIR"

VERSION="$(grep '^version' Cargo.toml | head -1 | sed 's/.*= *"\(.*\)"/\1/')"
DIST_ABS="$(cd dist && pwd)"
MAC_BIN="target/release/sts-x"
WIN_BIN="target/x86_64-pc-windows-msvc/release/sts-x.exe"
MAC_DYLIB_SRC="/Users/xtap/Documents/AI/trueskin-rs/target/release/libonnxruntime.1.28.0.dylib"
WIN_DLL_SRC="_scratch/win-dll/onnxruntime.dll"
MODEL_DIR="models/bge-small-zh-v1.5"
PY="/Users/xtap/.workbuddy/binaries/python/versions/3.13.12/bin/python3"

echo "=== sts-x ${VERSION} 全平台发布打包 ==="
mkdir -p dist
for f in "$MAC_BIN" "$WIN_BIN" "$MODEL_DIR/model.onnx" "$MODEL_DIR/tokenizer.json"; do
    [ -f "$f" ] || { echo "❌ 缺 $f"; exit 1; }
done
[ -f "$WIN_DLL_SRC" ] || { echo "❌ 缺 $WIN_DLL_SRC（先跑 pack-win-semantic.sh 提取）"; exit 1; }
[ -f "$MAC_DYLIB_SRC" ] || echo "⚠️ 缺 Mac dylib $MAC_DYLIB_SRC（Mac 语义包将不带 dylib）"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ── 小白说明生成（R1 四段式 + 标准安装 + MCP 接入）──────────
gen_note() { # $1=平台 mac|win  $2=是否语义 yes|no  $3=输出路径
    local plat="$1" sem="$2" out="$3"
    local sem_label sem_note
    if [ "$sem" = "yes" ]; then sem_label="语义检索版"; else sem_label="默认版"; fi
    {
        echo "首次打开必看 - sts-x ${VERSION}（${plat} ${sem_label}）"
        echo "=================================================="
        echo "1. 先别慌：这是免费分享版，没做付费签名，第一次运行系统可能拦你，"
        echo "   不是病毒，不会自动删。"
        echo ""
        echo "2. 标准安装（CLI，一次装好全局可用）："
        if [ "$plat" = "win" ]; then
            echo "   解压到任意目录（如 C:\\tools\\sts-x），然后把该目录加入系统 PATH："
            echo "     设置 → 系统 → 关于 → 高级系统设置 → 环境变量 → Path → 新建"
            echo "     填 C:\\tools\\sts-x（或你解压的目录）→ 确定。"
            echo "   之后新开的命令行窗口里直接输 sts-x 即可。"
        else
            echo "   解压到任意目录，然后："
            echo "     sudo mv sts-x /usr/local/bin/          # 装成全局命令"
            echo "   之后任何终端里直接输 sts-x 即可。"
        fi
        echo ""
        echo "3. 怎么用："
        echo "     sts-x --version                    # 应显示 ${VERSION}"
        echo "     sts-x ai \"缓存\" -p <你的项目路径>  # 中文查询代码（0 命中自动换英文词重试）"
        echo "     sts-x search \"McpServer\" -p <项目> --locate"
        echo "     sts-x file \"Cargo.toml\" -p <目录>  # 任意目录零索引文件搜索"
        if [ "$sem" = "yes" ]; then
            echo "     STX_SEMANTIC=1 sts-x ai \"缓存目录在哪里\" -p <项目>  # 中文 NL → 英文代码语义直达"
        fi
        echo ""
        echo "4. MCP 接入（AI 客户端，推荐方式 A 原生 stdio）："
        echo "   在 AI 客户端的 MCP 配置里加（如 ~/.workbuddy/mcp.json）："
        if [ "$plat" = "win" ]; then
            echo "   { \"mcpServers\": { \"sts-x\": { \"command\": \"C:\\\\tools\\\\sts-x\\\\sts-x.exe\","
            echo "       \"args\": [\"mcp\", \"-p\", \"C:\\\\你的项目根\"], \"env\": {} } } }"
        else
            echo "   { \"mcpServers\": { \"sts-x\": { \"command\": \"/usr/local/bin/sts-x\","
            echo "       \"args\": [\"mcp\", \"-p\", \"/你的项目根\"], \"env\": {} } } }"
        fi
        if [ "$sem" = "yes" ]; then
            echo "   语义版建议在 env 里加：\"env\": { \"STX_SEMANTIC\": \"1\" }（或命令行 export/set）"
        fi
        echo "   客户端将看到工具：search（代码搜索）/ file（文件搜索）。"
        echo ""
        echo "5. 第一次被拦怎么办："
        if [ "$plat" = "win" ]; then
            echo "   Windows：双击 exe 被 SmartScreen 拦 → 点「更多信息 → 仍要运行」；"
            echo "   或右键 exe → 属性 → 底部「解除锁定」→ 确定。"
        else
            echo "   macOS：终端跑二进制被拦（无法验证开发者）→ 先执行一次："
            echo "     xattr -dr com.apple.quarantine <解压目录>"
            echo "   再运行即可，以后不再提示。"
        fi
        echo ""
        echo "6. 支持系统：macOS 12+（Apple Silicon/Intel）/ Windows 10/11 64 位。"
        if [ "$sem" = "yes" ]; then
            echo "   别删旁边的依赖："
            echo "   - lib/   onnxruntime 动态库（语义检索必需；缺失时自动退回 BM25，不崩）"
            echo "   - models/ 中文 embedding 模型 bge-small-zh-v1.5"
            echo "   语义检索：设 STX_SEMANTIC=1 后查询即启用向量召回"
            echo "   （中文描述 → 英文代码语义直达，如「缓存目录在哪里」→ cache.rs）。"
            if [ "$plat" = "mac" ]; then
                echo "   ${VERSION} 起自动探测 dylib（同目录 lib/、/usr/local/lib、/opt/homebrew/lib），"
                echo "   直接 ./sts-x 即可零配置使用；sts-x.sh 为兜底启动器。"
            else
                echo "   ${VERSION} 起自动探测 dll（同目录 lib/、同目录），直接 sts-x.exe 即可；"
                echo "   sts-x-semantic.bat 为兜底启动器（自动开 STX_SEMANTIC）。"
            fi
        else
            echo "   默认版零依赖，单文件即用。"
        fi
        echo ""
        echo "祝你用得开心 🎉"
    } > "$out"
}

# ── Mac 默认版 ────────────────────────────────────────────
echo "── Mac 默认版 ──"
PKG="$TMP/mac-default"; mkdir -p "$PKG"
cp "$MAC_BIN" "$PKG/sts-x"
gen_note mac no "$PKG/首次打开必看-sts-x.txt"

# ── Mac 语义版 ────────────────────────────────────────────
echo "── Mac 语义版 ──"
PKG="$TMP/mac-sem"; mkdir -p "$PKG/lib" "$PKG/models"
cp "$MAC_BIN" "$PKG/sts-x"
if [ -f "$MAC_DYLIB_SRC" ]; then cp "$MAC_DYLIB_SRC" "$PKG/lib/"; fi
cp -R "$MODEL_DIR" "$PKG/models/"
cat > "$PKG/sts-x.sh" <<'SH'
#!/usr/bin/env bash
# sts-x 语义检索启动器（Mac）——自动定位同目录 lib 下的 onnxruntime dylib
DIR="$(cd "$(dirname "$0")" && pwd)"
DYLIB=$(ls "$DIR"/lib/libonnxruntime*.dylib 2>/dev/null | head -1)
if [ -n "$DYLIB" ] && [ -z "${ORT_DYLIB_PATH:-}" ]; then
  export ORT_DYLIB_PATH="$DYLIB"
fi
if [ -z "${STX_SEMANTIC:-}" ]; then export STX_SEMANTIC=1; fi
exec "$DIR/sts-x" "$@"
SH
chmod +x "$PKG/sts-x.sh"
gen_note mac yes "$PKG/首次打开必看-sts-x.txt"

# ── Win 默认版 ────────────────────────────────────────────
echo "── Win 默认版 ──"
PKG="$TMP/win-default"; mkdir -p "$PKG"
cp "$WIN_BIN" "$PKG/sts-x.exe"
gen_note win no "$PKG/首次打开必看-sts-x.txt"

# ── Win 语义版 ────────────────────────────────────────────
echo "── Win 语义版 ──"
PKG="$TMP/win-sem"; mkdir -p "$PKG/lib" "$PKG/models"
cp "$WIN_BIN" "$PKG/sts-x.exe"
cp "$WIN_DLL_SRC" "$PKG/lib/"
cp -R "$MODEL_DIR" "$PKG/models/"
# sts-x-semantic.bat 启动器（自动开 STX_SEMANTIC；纯 ASCII + CRLF 防编码坑）
cat > "$PKG/sts-x-semantic.bat" <<'BAT'
@echo off
rem sts-x semantic launcher (Windows) - enables STX_SEMANTIC automatically
setlocal
if not defined STX_SEMANTIC set STX_SEMANTIC=1
"%~dp0sts-x.exe" %*
BAT
# 转 CRLF（bat 必须 CRLF）
if command -v perl >/dev/null 2>&1; then
    perl -pi -e 's/\r?\n/\r\n/g' "$PKG/sts-x-semantic.bat"
else
    sed -i '' 's/$/\r/' "$PKG/sts-x-semantic.bat"
fi
gen_note win yes "$PKG/首次打开必看-sts-x.txt"

# ── 打包（Python zipfile，R2 中文名 UTF-8；保留 sts-x.sh 可执行位）────
"$PY" - "$TMP" "$DIST_ABS" "$VERSION" <<'PY'
import os, sys, zipfile
tmp, dist, ver = sys.argv[1], sys.argv[2], sys.argv[3]

def zip_dir(src, out, exec_arcs=()):
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for root, _, files in os.walk(src):
            for f in files:
                p = os.path.join(root, f)
                arc = os.path.relpath(p, src)
                zi = zipfile.ZipInfo.from_file(p, arc)
                zi.compress_type = zipfile.ZIP_DEFLATED  # writestr 默认不压缩，必须显式
                if arc in exec_arcs:
                    zi.external_attr = (0o755 & 0xFFFF) << 16  # -rwxr-xr-x
                with open(p, "rb") as fh:
                    z.writestr(zi, fh.read())

jobs = [
    (f"{tmp}/mac-default", f"{dist}/STS-X-{ver}-Mac版(给AI搜代码).zip", ()),
    (f"{tmp}/mac-sem",     f"{dist}/STS-X-{ver}-Mac版-语义检索(给AI搜代码).zip", ("sts-x.sh",)),
    (f"{tmp}/win-default", f"{dist}/STS-X-{ver}-Win版(给AI搜代码).zip", ()),
    (f"{tmp}/win-sem",     f"{dist}/STS-X-{ver}-Win版-语义检索(给AI搜代码).zip", ()),
]
for src, out, execs in jobs:
    zip_dir(src, out, execs)
    print(f"✅ {os.path.basename(out)} ({os.path.getsize(out)/1048576:.1f}MB)")
PY

# ── 源码包（git ls-files + 未跟踪新源文件；排除模型/二进制）──
echo "── 源码包 ──"
SRC_TMP="$TMP/src"
mkdir -p "$SRC_TMP"
# 已跟踪文件
git ls-files -z | while IFS= read -r -d '' f; do
    mkdir -p "$SRC_TMP/$(dirname "$f")"
    cp "$f" "$SRC_TMP/$f"
done
# 未跟踪的新源文件（?? 且不在 _scratch/dist/target/models）
git status --porcelain | awk '$1=="??"{print $2}' | grep -vE '^(_scratch|dist|target|models)/' | while IFS= read -r f; do
    [ -f "$f" ] || continue
    mkdir -p "$SRC_TMP/$(dirname "$f")"
    cp "$f" "$SRC_TMP/$f"
done
# .cargo/config.toml 若未跟踪则手动带（crt-static 关键）
if [ -f .cargo/config.toml ] && ! git ls-files --error-unmatch .cargo/config.toml >/dev/null 2>&1; then
    mkdir -p "$SRC_TMP/.cargo"; cp .cargo/config.toml "$SRC_TMP/.cargo/"
fi
cat > "$SRC_TMP/首次打开必看-源码编译.txt" <<'SRC'
sts-x 源码编译指引
=================
依赖：Rust 1.85+；core_lib 位于 ../rust_master_workspace/libs/core_lib（或按 Cargo.toml 调整路径）。

默认版（零依赖）：
  cargo build --release
  # 产物 target/release/sts-x（macOS）/ sts-x.exe（Windows）

语义检索版（中文向量召回，推荐）：
  # 1) 下载模型（bge-small-zh-v1.5，中文 512 维）：
  ./scripts/download-models.sh
  # 2) 构建：
  cargo build --release --features semantic
  # 3) 运行时动态库（v3.3.1 起自动探测，一般无需手动）：
  #    macOS: 把 libonnxruntime.1.28.0.dylib 放 exe 同目录 lib/、/usr/local/lib 或 /opt/homebrew/lib
  #    Windows: 把 onnxruntime.dll 放 exe 同目录 lib/ 或同目录
  #    或显式 export ORT_DYLIB_PATH=/path/to/libonnxruntime.(dylib|dll)
  # 4) 开启语义：
  export STX_SEMANTIC=1
  sts-x ai "缓存" -p <项目>

Windows 交叉编译（macOS 上出 Win exe，crt-static 单文件）：
  RUSTC_WRAPPER= SDKROOT= cargo xwin build --release --target x86_64-pc-windows-msvc [--features semantic]

门禁（发布前必跑，全绿才可发）：
  cargo fmt
  cargo test --all-targets --features semantic
  cargo clippy --all-targets --features semantic -- -D warnings

模型与动态库不进 git（.gitignore），源码包不含模型——语义功能需自行 download-models.sh。
SRC
(cd "$SRC_TMP" && "$PY" - "$DIST_ABS" "$VERSION" <<'PY'
import os, sys, zipfile
dist, ver = sys.argv[1], sys.argv[2]
out = f"{dist}/STS-X-{ver}-源码.zip"
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
    for root, _, files in os.walk("."):
        for f in files:
            p = os.path.join(root, f)
            z.write(p, os.path.relpath(p, "."))
print(f"✅ STS-X-{ver}-源码.zip ({os.path.getsize(out)/1048576:.1f}MB)")
PY
)

echo ""
echo "=== 全部产物（dist/）==="
ls -lh dist/STS-X-${VERSION}*
