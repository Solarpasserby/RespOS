#!/usr/bin/env bash
set -euo pipefail

DOC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DOC_DIR"

echo "[1/2] 从 Markdown 生成章节 Typst..."
bash generate.sh

command -v typst >/dev/null 2>&1 || {
  echo "错误: 未找到 typst（建议使用 0.11.1）" >&2
  exit 1
}

FONT_PATH="../assets/fonts"
if [[ -d /mnt/c/Windows/Fonts ]]; then
  FONT_PATH="../assets/fonts:/mnt/c/Windows/Fonts"
fi

echo "[2/2] 编译决赛文档 PDF..."
typst compile --root .. --font-path "$FONT_PATH" main.typ 决赛文档.pdf
echo "✓ 编译成功: $DOC_DIR/决赛文档.pdf"
