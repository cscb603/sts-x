#!/bin/bash
# download-models.sh
# Project: sts-x
# Description: Download the embedding model for sts-x semantic search (P0-2).
#
# 推荐模型：bge-small-zh-v1.5（中文版，512 维）—— 专治「中文 NL → 英文代码」
# 的跨语言语义鸿沟。模型存到 <sts-x-root>/models/bge-small-zh-v1.5/。
#
# 运行期查找（embed/mod.rs::find_model_files，目录名不限）：
#   1. $STX_MODEL_DIR
#   2. <可执行文件同目录>/models/
#   3. 系统缓存根 models/
#   4. config.model_path（默认相对 "models"）
# 布局：model.onnx + tokenizer.json 平铺，或一层子目录（任意目录名）。

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MODELS_DIR="$SCRIPT_DIR/../models"
MODEL_SUBDIR="bge-small-zh-v1.5"
EMBED_DIR="$MODELS_DIR/$MODEL_SUBDIR"
mkdir -p "$MODELS_DIR"

echo "📦 STS-X Semantic Model Downloader"
echo "=================================="
echo "Model:   bge-small-zh-v1.5 (中文 embedding, 512d, 23MB)"
echo "Target:  $EMBED_DIR"
echo ""

if [ -f "$EMBED_DIR/model.onnx" ] && [ -f "$EMBED_DIR/tokenizer.json" ]; then
    echo "✅ Embedding model already exists: $EMBED_DIR"
else
    echo "⬇️  Downloading bge-small-zh-v1.5 (ONNX)..."
    mkdir -p "$EMBED_DIR"

    # Option 1: huggingface-cli（最稳）
    if command -v huggingface-cli >/dev/null 2>&1; then
        huggingface-cli download \
            Xenova/bge-small-zh-v1.5 \
            onnx/model_quantized.onnx \
            --local-dir "$EMBED_DIR" 2>/dev/null && \
            mv "$EMBED_DIR/onnx/model_quantized.onnx" "$EMBED_DIR/model.onnx" 2>/dev/null; \
            rm -rf "$EMBED_DIR/onnx" 2>/dev/null
        if [ ! -f "$EMBED_DIR/tokenizer.json" ]; then
            huggingface-cli download \
                Xenova/bge-small-zh-v1.5 \
                tokenizer.json \
                --local-dir "$EMBED_DIR" 2>/dev/null || true
        fi
    fi

    # Option 2: 直链下载（aria2 / curl）
    if [ ! -f "$EMBED_DIR/model.onnx" ]; then
        echo "   Fallback: direct URL download..."
        BASE="https://huggingface.co/Xenova/bge-small-zh-v1.5/resolve/main/onnx"
        (aria2c -x 5 -s 5 --continue=true -d "$EMBED_DIR" -o "model.onnx" "$BASE/model_quantized.onnx" 2>/dev/null \
            || curl -L --retry 3 -o "$EMBED_DIR/model.onnx" "$BASE/model_quantized.onnx" 2>/dev/null) || true
        [ ! -s "$EMBED_DIR/model.onnx" ] && rm -f "$EMBED_DIR/model.onnx"
    fi
    if [ ! -f "$EMBED_DIR/tokenizer.json" ]; then
        (aria2c -x 5 -s 5 --continue=true -d "$EMBED_DIR" -o "tokenizer.json" "$(dirname "$BASE")/tokenizer.json" 2>/dev/null \
            || curl -L --retry 3 -o "$EMBED_DIR/tokenizer.json" "https://huggingface.co/Xenova/bge-small-zh-v1.5/resolve/main/tokenizer.json" 2>/dev/null) || true
    fi

    if [ -f "$EMBED_DIR/model.onnx" ] && [ -f "$EMBED_DIR/tokenizer.json" ]; then
        echo "   ✅ Embedding model downloaded"
    else
        echo "   ⚠️  Could not download ONNX model. Convert manually:"
        echo "      pip install optimum[exporters]"
        echo "      optimum-cli export onnx --model BAAI/bge-small-zh-v1.5 $EMBED_DIR"
    fi
fi

echo ""
echo "═══════════════════════════════════════════════════"
echo "Model directory: $MODELS_DIR"
echo ""
echo "Usage (semantic build only, cargo build --release --features semantic):"
echo "  export STX_MODEL_DIR=$MODELS_DIR"
echo "  export ORT_DYLIB_PATH=/path/to/libonnxruntime.<dylib|dll>   # macOS/Linux"
echo "  # Windows: 把 onnxruntime.dll 放 sts-x.exe 同目录或 lib/ 即可"
echo "  export STX_SEMANTIC=1"
echo "  sts-x ai \"缓存\" -p <项目>   # 中文 NL → 英文代码语义直达"
echo ""
echo "无 STX_SEMANTIC 时仍走 BM25 + P0-1 自动重试链（零依赖，无需模型）。"
