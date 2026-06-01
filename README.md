# RespOS

一个用 Rust 编写的 RISC-V 64 位操作系统内核，面向教学与竞赛

**目标平台:** riscv64gc-unknown-none-elf
**语言:** Rust

## 构建与运行

```bash
make MODE=debug         # 调试构建
make MODE=release       # 发布构建
make clean              # 清理构建产物
make check-submit       # 竞赛提交前检查
```

生成的 kernel 二进制为 `kernel-rv`，可通过 QEMU 加载运行

## 目录结构

```
RespOS/
├── Makefile
├── bootloader/          # RustSBI 引导固件
├── scripts/             # 镜像获取 / 工具链安装脚本
├── doc/                 # 设计文档与开发日志
├── user/                # 用户态程序
│
└── os/                  # 内核源码
    ├── build.rs         # 构建脚本
    └── src/
        ├── main.rs      # 内核入口
        ├── arch/rv64/   # 架构相关
        │   ├── config/  #   内核常量
        │   ├── entry/   #   启动入口
        │   ├── mm/      #   页表实现
        │   ├── trap/    #   异常/中断处理
        │   └── task/    #   任务上下文切换
        ├── drivers/     # 设备驱动
        ├── fs/          # 文件系统
        ├── mm/          # 内存管理
        ├── mutex/       # 锁机制
        ├── syscall/     # 系统调用分发
        └── task/        # 任务管理
```

## 许可证

[GNU General Public License v2.0](LICENSE)
