#!/bin/bash
# RespOS 初赛文档编译脚本
# 用法：bash build.sh

cd "$(dirname "$0")"
echo "正在编译 Typst 文档..."
typst compile --font-path /mnt/c/Windows/Fonts main.typ 初赛文档.pdf
if [ $? -eq 0 ]; then
    echo "编译成功: doc/初赛文档/初赛文档.pdf"
else
    echo "编译失败"
    exit 1
fi
