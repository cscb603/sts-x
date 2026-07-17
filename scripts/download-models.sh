#!/bin/bash
# download-models.sh
# Project: sts-x
# Description: Download ONNX models for sts-x (embedding + reranker)
#
# Uses huggingface-cli for reliable model downloads.
# Models are stored in <sts-x-root>/models/ directory.
#
# Models downloaded:
# - BGE-small-en-v1.5 → text embedding (384d, ~100MB ONNX quantized)
# - BGE-Reranker-v2-m3 → cross-encoder reranker (~500MB ONNX)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MODELS_DIR="$SCRIPT_DIR/../models"
mkdir -p "$MODELS_DIR"

echo "📦 STS-X Model Downloader"
echo "========================="
echo "Target: $MODELS_DIR"
echo ""

# ── BGE-small-en-v1.5 (Embedding) ──────────────────────
EMBED_DIR="$MODELS_DIR/bge-small-en-v1.5"
if [ -f "$EMBED_DIR/model.onnx" ] && [ -f "$EMBED_DIR/tokenizer.json" ]; then
    echo "✅ Embedding model already exists: $EMBED_DIR"
else
    echo "⬇️  Downloading BGE-small-en-v1.5 (embedding, 384d)..."
    mkdir -p "$EMBED_DIR"

    # Try to find ONNX export of BGE-small
    # Option 1: Use ONNX export from HuggingFace
    echo "   Attempting download via huggingface-cli..."
    huggingface-cli download \
        Xenova/bge-small-en-v1.5 \
        onnx/model_quantized.onnx \
        --local-dir "$EMBED_DIR" 2>/dev/null && \
        mv "$EMBED_DIR/onnx/model_quantized.onnx" "$EMBED_DIR/model.onnx" 2>/dev/null; \
        rm -rf "$EMBED_DIR/onnx" 2>/dev/null

    # Option 2: Download tokenizer separately
    if [ ! -f "$EMBED_DIR/tokenizer.json" ]; then
        huggingface-cli download \
            Xenova/bge-small-en-v1.5 \
            tokenizer.json \
            --local-dir "$EMBED_DIR" 2>/dev/null || true
    fi

    # Option 3: Fallback to direct URL with aria2
    if [ ! -f "$EMBED_DIR/model.onnx" ]; then
        echo "   Fallback: downloading via direct URL (converted ONNX)..."
        aria2c -x 5 -s 5 --continue=true \
            -d "$EMBED_DIR" \
            -o "model.onnx" \
            "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main/onnx/model_quantized.onnx" 2>/dev/null || \
        aria2c -x 5 -s 5 --continue=true \
            -d "$EMBED_DIR" \
            -o "model.onnx" \
            "https://huggingface.co/Xenova/bge-small-en-v1.5/resolve/main/onnx/model.onnx" 2>/dev/null || true
    fi

    if [ -f "$EMBED_DIR/model.onnx" ]; then
        echo "   ✅ Embedding model downloaded"
    else
        echo "   ⚠️  Could not download ONNX model. You can convert manually:"
        echo "      pip install optimum[exporters]"
        echo "      optimum-cli export onnx --model BAAI/bge-small-en-v1.5 $EMBED_DIR"
    fi
fi

# ── BGE-Reranker-v2-m3 (Reranker) ─────────────────────
RERANK_DIR="$MODELS_DIR/bge-reranker-v2-m3"
if [ -f "$RERANK_DIR/model.onnx" ] && [ -f "$RERANK_DIR/tokenizer.json" ]; then
    echo "✅ Reranker model already exists: $RERANK_DIR"
else
    echo ""
    echo "⬇️  Downloading BGE-Reranker-v2-m3 (cross-encoder)..."
    mkdir -p "$RERANK_DIR"

    # Try huggingface-cli
    echo "   Attempting download via huggingface-cli..."
    huggingface-cli download \
        Xenova/bge-reranker-v2-m3 \
        onnx/model_quantized.onnx \
        --local-dir "$RERANK_DIR" 2>/dev/null && \
        mv "$RERANK_DIR/onnx/model_quantized.onnx" "$RERANK_DIR/model.onnx" 2>/dev/null; \
        rm -rf "$RERANK_DIR/onnx" 2>/dev/null

    if [ ! -f "$RERANK_DIR/tokenizer.json" ]; then
        huggingface-cli download \
            Xenova/bge-reranker-v2-m3 \
            tokenizer.json \
            --local-dir "$RERANK_DIR" 2>/dev/null || true
    fi

    # Fallback
    if [ ! -f "$RERANK_DIR/model.onnx" ]; then
        echo "   Fallback: downloading via direct URL..."
        aria2c -x 5 -s 5 --continue=true \
            -d "$RERANK_DIR" \
            -o "model.onnx" \
            "https://huggingface.co/Xenova/bge-reranker-v2-m3/resolve/main/onnx/model_quantized.onnx" 2>/dev/null || true
    fi

    if [ -f "$RERANK_DIR/model.onnx" ]; then
        echo "   ✅ Reranker model downloaded"
    else
        echo "   ⚠️  Could not download ONNX model. You can convert manually:"
        echo "      pip install optimum[exporters]"
        echo "      optimum-cli export onnx --model BAAI/bge-reranker-v2-m3 $RERANK_DIR"
    fi
fi

echo ""
echo "══════════════════════════════════════════════"
echo "Model directory: $MODELS_DIR"
echo ""
echo "Usage:"
echo "  sts-x index /path/to/project --model $MODELS_DIR/bge-small-en-v1.5"
echo "  sts-x search <query>"
echo "  sts-x serve"
echo ""
