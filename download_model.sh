#!/bin/bash
# Download bge-small-en-v1.5 model weights for StackIntercept
set -euo pipefail

MODEL_DIR="model"
mkdir -p "$MODEL_DIR"

echo "Downloading bge-small-en-v1.5 model weights..."
curl -L -o "$MODEL_DIR/config.json" \
    "https://huggingface.co/BAAI/bge-small-en-v1.5/raw/main/config.json"
curl -L -o "$MODEL_DIR/tokenizer.json" \
    "https://huggingface.co/BAAI/bge-small-en-v1.5/raw/main/tokenizer.json"
curl -L -o "$MODEL_DIR/model.safetensors" \
    "https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/model.safetensors"

echo "Done. Model files in $MODEL_DIR/:"
ls -lh "$MODEL_DIR/"
