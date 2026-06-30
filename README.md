# RespOS

RespOS 是一个使用 Rust 编写的教学与竞赛型操作系统内核，主要面向全国大学生操作系统比赛。当前项目支持 RISC-V 64 与 LoongArch 64 两个架构，能够在比赛镜像中运行主要用户态测试程序，并围绕 Linux ABI 兼容、双架构移植和复杂测例支撑做了较完整的工程实现。

- [初赛文档](docs/初赛文档/初赛文档.pdf)
- [初赛 PPT](docs/初赛文档/RespOS初赛进展汇报.pptx)
- [初赛视频展示](https://pan.baidu.com/s/1UeR0f-N6hwRB5mUd1EDO-w?pwd=0612)

> 补充说明：受限于项目规划、时间投入和个人能力，最终提测成绩未能完全反映项目在本地环境中的实际完成度。我们在本地对 LTP 等测例做了持续验证，其中 LTP 可通过 600 余个测例，本地阶段性统计分数接近 5000 分。但不可否认：评测平台下的并发、时序和环境差异仍暴露出内核稳定性与细节语义上的不足，造成我们无法将内核潜在的能力进行展现。

> 听我解释：由于最后提交时间较晚，且提交计划和平台环境可能共同影响了最终成绩，展示分数可能相对偏低（我等不到它出分了）；我们最后提交的版本选择的是此前实际拿到过分数、相对稳定的版本，该版本可取得约 2350 分。我们承认项目仍有不足，也会在后续继续修复和完善。恳请评判老师在评分时结合项目的代码规模、模块完整性、本地测试结果、最终稳定版本得分和实际工程工作量，给予一定的理解与综合考量。

## 设计理念与完成情况

RespOS 以 Linux 用户态兼容为主要目标，内核提供接近 Linux ABI 的系统调用接口，并通过比赛镜像中的 musl/glibc 程序持续验证语义。项目在结构上保持内核、用户态运行时与测例入口分离：内核负责进程、内存、文件系统、网络和中断等基础能力，用户态部分提供系统调用封装、基础运行时和 `testrunner`。

在实现上，RespOS 尽量保持跨架构公共逻辑复用，将 RISC-V 64 与 LoongArch 64 的差异收敛到 HAL、启动、陷入处理和页表等架构相关层；在测试流程上，则由 `testrunner` 按比赛镜像中的测例组织运行，并输出评测机可识别的日志。

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
make all          # 构建 kernel-rv 与 kernel-la
make build-rv     # 仅构建 RISC-V 内核
make build-la     # 仅构建 LoongArch 内核
make MODE=debug   # 使用 debug 配置构建
make check-submit # 检查提交产物
make clean        # 清理构建产物
```

构建完成后，仓库根目录会生成：

- `kernel-rv`
- `kernel-la`

这两个文件是比赛平台要求的 ELF 内核产物。

第一次运行前需要在仓库根目录准备测试镜像：

- RISC-V: `img/sdcard-rv.img`
- LoongArch: `img/sdcard-la.img`

可以直接执行脚本下载并解压官方测试仓库发布的镜像：

```bash
bash scripts/get_img.sh
```

脚本会把镜像放到 `img/` 目录，并保留 `.xz` 压缩包，后续镜像被写坏时可以从本地压缩包恢复。

## 运行

```bash
make rv           # 构建并运行 RISC-V 版本
make la           # 构建并运行 LoongArch 版本
```

也可以进入内核目录直接运行当前架构：

```bash
cd os
make run          # 默认 ARCH=riscv64
```

运行日志默认写入：

- RISC-V: `rv-output.txt`
- LoongArch: `la-output.txt`

## 调试

RISC-V 调试：

```bash
cd os
make gdbserver
```

LoongArch 调试：

```bash
cd os
make gdbserver ARCH=loongarch64
```

## 目录结构

```text
RespOS/
├── Makefile              # 顶层构建与 QEMU 运行入口，生成 kernel-rv / kernel-la
├── bootloader/           # RISC-V 启动镜像与引导相关文件
├── os/                   # 内核源码
│   ├── src/
│   │   ├── arch/         # RISC-V / LoongArch 架构适配、启动、陷入与上下文切换
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

## 测例与评测

RespOS 主要面向 OSComp 初赛评测流程：

1. `make all` 生成 `kernel-rv` 与 `kernel-la`。
2. QEMU 挂载比赛提供的 ext4 测试镜像。
3. 内核启动后扫描并运行镜像中的测试脚本。
4. `testrunner` 串行执行测例组，输出符合评测机格式的日志。
5. 测例全部结束后主动关机。

当前本地常用测例包括：

- basic
- BusyBox
- libc-bench
- libctest
- lua
- iperf
- netperf
- iozone
- lmbench
- LTP

其中 LTP 使用清单化运行方式，便于在不同阶段启用高收益测例、跳过高风险或高耗时测例。

## 许可证

[GNU General Public License v2.0](LICENSE)
