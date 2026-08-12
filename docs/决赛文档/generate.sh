#!/usr/bin/env bash
set -euo pipefail

DOC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="$DOC_DIR/markdown"
CHAPTER_DIR="$DOC_DIR/chapters"
MANIFEST="$DOC_DIR/chapters.typ"

command -v pandoc >/dev/null 2>&1 || {
  echo "错误: 未找到 pandoc（需要 Pandoc 3.x 的 typst writer）" >&2
  exit 1
}

mkdir -p "$CHAPTER_DIR"
find "$CHAPTER_DIR" -maxdepth 1 -type f -name '*.typ' -delete

{
  echo "// 此文件由 generate.sh 自动生成，请勿直接编辑。"
  for source in "$SOURCE_DIR"/*.md; do
    name="$(basename "$source" .md)"
    output="$CHAPTER_DIR/$name.typ"
    pandoc \
      --from=gfm \
      --to=typst \
      --wrap=none \
      --output="$output" \
      "$source"
    printf '#include "chapters/%s.typ"\n' "$name"
  done
} > "$MANIFEST"

echo "生成 $(find "$CHAPTER_DIR" -maxdepth 1 -type f -name '*.typ' | wc -l | tr -d ' ') 个章节。"
