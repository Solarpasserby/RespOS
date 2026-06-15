#!/bin/bash
# RespOS 初赛文档编译脚本
cd "$(dirname "$0")"
echo "编译 RespOS 初赛文档..."

# 字体路径优先级：本地 fonts/ > Windows 系统字体
FONT_PATH="--font-path fonts"
if [ -d "/mnt/c/Windows/Fonts" ]; then
    FONT_PATH="--font-path fonts:/mnt/c/Windows/Fonts"
fi

typst compile $FONT_PATH main.typ 初赛文档.pdf
if [ $? -eq 0 ]; then
    echo "✓ 编译成功: 初赛文档.pdf"
else
    echo "✗ 编译失败"
    exit 1
fi
