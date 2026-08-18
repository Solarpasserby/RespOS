# RespOS

RespOS 是一个使用 Rust 编写的教学与竞赛型操作系统内核，主要面向全国大学生操作系统比赛。当前项目支持 RISC-V 64 与 LoongArch 64 两个架构，能够在比赛镜像中运行主要用户态测试程序，并围绕 Linux ABI 兼容、双架构移植和复杂测例支撑做了较完整的工程实现。

- [决赛文档](docs/决赛文档/决赛文档.pdf)
- [决赛答辩 PPT](docs/决赛文档/RespOS决赛答辩-初版.pptx)

## 设计理念与完成情况

RespOS 以 Linux 用户态兼容为主要目标，内核提供接近 Linux ABI 的系统调用接口，并通过比赛镜像中的 musl/glibc 程序持续验证语义。项目在结构上保持内核、用户态运行时与测例入口分离：内核负责进程、内存、文件系统、网络和中断等基础能力，用户态部分提供系统调用封装、基础运行时和 `testrunner`。

在实现上，RespOS 尽量保持跨架构公共逻辑复用，将 RISC-V 64 与 LoongArch 64 的差异收敛到 HAL、启动、陷入处理和页表等架构相关层。初赛及本地兼容性回归由内嵌 `testrunner` 组织；决赛评分绕过 `testrunner`，由 `contest_launcher` 直接执行官方镜像中的 CAgent 和 BuildStorm 脚本。

| 模块 | 完成情况 |
| --- | --- |
| HAL 模块 | 实现了项目自有的 HAL 抽象与架构适配代码，支持 RISC-V 64 和 LoongArch 64 双架构启动、陷入处理、上下文切换与页表相关操作。 |
| 进程管理 | 实现统一的进程和线程数据结构，支持无栈协程式调度、全局统一 executor 调度器、多线程资源回收、进程等待与退出清理。 |
| 文件系统 | 实现基于 dentry 的目录树构建，支持 ext4、procfs、devfs、tmpfs/shm 等文件系统，并通过页缓存和 dentry 缓存加快读写与路径查找。 |
| 内存管理 | 实现物理页管理、地址空间管理、页表映射、用户缓冲区检查等基础能力，并支持 CoW、lazy allocation、mmap、文件映射和页面回收等优化。 |
| 时钟模块 | 实现基于时间轮和最小堆混合结构的定时任务管理，支持 sleep、interval timer、实时定时器与阻塞任务唤醒机制。 |
| IPC 系统 | 支持用户自定义信号、信号屏蔽与 `sigreturn` 机制，实现支持读者/写者同步的管道机制，并支持 System V 共享内存。 |
| 网络模块 | 初步完成 virtio-net、loopback 与基础 socket 相关代码，能够运行网络类测试，并通过了 netperf 网络测例。 |
| 中断模块 | 支持时钟中断和外部中断处理，包含 QEMU 环境下的中断分发，并支持上板场景的串口中断处理。 |
| 用户态与测例 | 实现用户态运行时、系统调用封装和 `testrunner`，支持 basic、busybox、libc-bench、libctest、LTP、iozone、iperf、netperf、lmbench 等测例的本地运行与评测日志输出。 |

## 构建与运行准备

```bash
make all                      # 线上评测入口：构建双架构 QEMU 内核和自动识别辅助盘
make build-qemu-rv64          # 构建 QEMU RV64 内核
make build-jh7110             # 构建 JH7110/VisionFive 2 真机镜像
make build-qemu-loongarch64   # 构建 QEMU LoongArch64 内核
make build-ls2k1000           # 构建 LS2K1000 真机镜像
make check-submit             # 检查提交产物
make help                     # 查看全部入口及默认资源配置
make clean                    # 清理构建产物
```

四个平台统一使用 `build-<平台名>` 形式。旧命令 `build-rv`、`build-vf2`、`build-la`、
`build-la-ls2k1000` 仍作为兼容别名保留。

构建完成后，仓库根目录会生成：

- `kernel-rv`
- `kernel-la`
- `disk.img`
- `disk-la.img`

线上平台执行 `make all`。两个 `kernel-*` 是 ELF 内核，两个 `disk*.img` 是包含
`mode=auto` 的小型 ext4 辅助盘；平台提供大型官方根镜像并按比赛 QEMU 参数启动。
`contest_launcher` 会优先检查 CAgent/BuildStorm 决赛脚本，再检查初赛 basic 脚本，从而让同一份
提交同时适配决赛评分和后续初赛复测。

第一次本地运行前可下载官方初赛和决赛镜像：

```bash
bash scripts/get_img.sh                 # 保持原行为：初赛 + 当前决赛，两架构
bash scripts/get_img.sh software rv     # 2025 软件兼容性 Alpine RV64 镜像
bash scripts/get_img.sh software both   # 2025 软件兼容性 Alpine 双架构镜像
```

脚本会把镜像放到 `img/` 并保留压缩包；`all` 可一次获取全部三组镜像。软件兼容性镜像会校验 Release
记录的 SHA-256。初赛全量镜像用下面的命令恢复到不会和 128 MiB 决赛子集混淆的新名字：

```bash
make prepare-pre-images
```

## 运行

```bash
make run-rv-pre         # RISC-V 初赛全量测例，进入 testrunner
make run-la-pre         # LoongArch 初赛全量测例，进入 testrunner
make run-rv-final       # RISC-V 决赛 CAgent + BuildStorm
make run-la-final       # LoongArch 决赛 CAgent + BuildStorm
make run-rv-diagnostic  # 决赛镜像 + 交互式 shell
make run-la-diagnostic
make run-rv-software    # 2025 Alpine 软件兼容性镜像 + 专项辅助盘
make run-la-software
```

所有本地 QEMU 入口都使用 `-snapshot`。`make rv`/`make la` 暂时保留为初赛入口别名，
`make run-rv-pub`/`make run-la-pub` 保留为决赛入口别名；新脚本应使用上述明确名称。

## 目录结构

```text
RespOS/
├── Makefile              # 四平台构建与 QEMU 运行入口
├── auxfs/                # 辅助盘 profile 与 software/bootstrap payload
├── bootloader/           # RISC-V 启动镜像与引导相关文件
├── os/                   # 内核源码
│   ├── src/
│   │   ├── arch/         # RISC-V / LoongArch 架构适配、启动、陷入与上下文切换
│   │   ├── platform/     # QEMU RV64、JH7110、QEMU LA64、LS2K1000 平台实现
│   │   ├── drivers/      # virtio-blk、virtio-net 等设备驱动
│   │   ├── fs/           # VFS、ext4、procfs、devfs、dentry 与 mount tree
│   │   ├── mm/           # 物理页、地址空间、COW、lazy allocation 与 mmap
│   │   ├── task/         # 进程/线程、调度、等待、退出回收与 futex
│   │   ├── syscall/      # Linux ABI 风格系统调用实现
│   │   ├── signal/       # 信号递送、屏蔽、sigreturn 与默认动作
│   │   └── net/          # socket、loopback、virtio-net 与协议栈适配
│   ├── cargo/            # RISC-V / LoongArch Cargo 配置模板
│   └── Makefile          # 内核目录下的构建、运行与 GDB 调试入口
├── user/                 # 用户态库、测试程序与 testrunner
│   ├── src/
│   │   ├── lib.rs        # 用户态运行时与系统调用封装
│   │   └── bin/          # testrunner、shell 工具与各类测试入口程序
│   ├── build.rs          # 用户程序打包与 LTP 清单生成逻辑
│   └── oscomp_ltp_list.txt
├── img/                  # 本地测试镜像
├── judge/                # LTP 日志解析、报告生成与 baseline 对比工具
├── docs/                 # 设计记录、调试文档与比赛文档
├── scripts/              # 镜像下载、报告生成和辅助检查脚本
├── testsuit/             # 本地测例源码或资料
├── vendor/               # 第三方依赖源码
└── .devcontainer/        # Dev Container 开发环境配置
```

## 许可证

[GNU General Public License v2.0](LICENSE)
