# RespOS

RespOS 是一个使用 Rust 编写的教学与竞赛型操作系统内核，主要面向全国大学生操作系统比赛内核赛道的 QEMU 评测环境。

当前项目支持 RISC-V 64 与 LoongArch 64 两个架构，能够在比赛镜像中运行基础测例、BusyBox、libc-bench、LTP、iozone、netperf、lmbench 等用户态测试程序。

## 特性

- Rust 内核与用户态程序构建流程，内核与用户程序分离构建
- RISC-V 64 / LoongArch 64 双架构支持
- ELF 用户程序加载、动态链接程序运行支持
- Linux ABI 风格系统调用、信号、线程、futex 与进程资源管理
- 虚拟内存、COW、lazy allocation、mmap、文件映射与页缓存
- VFS、ext4、procfs、devfs、tmpfs/shm 与挂载管理
- virtio-blk、virtio-net、loopback 与基础 socket 支持
- 面向 OSComp 测例的 testrunner、资源清理与 LTP 清单化运行
- 本地日志解析、Linux baseline 对比与 CSV 报告生成工具

## 设计概览

RespOS 以 Linux 用户态兼容为主要目标，内核提供接近 Linux ABI 的系统调用接口，并通过比赛镜像中的 musl/glibc 程序持续验证语义。

内核主要模块包括：

- **arch**：RISC-V 与 LoongArch 的启动、陷入处理、页表与上下文切换。
- **mm**：地址空间、页表映射、COW、mmap、用户缓冲区检查与页面回收。
- **task**：进程/线程模型、调度、等待、退出回收、futex 与资源限制。
- **fs**：VFS、ext4、procfs、devfs、tmpfs、page cache 和 mount tree。
- **syscall**：按功能拆分的 Linux ABI 系统调用实现。
- **net**：基于 virtio-net 与协议栈的基础网络能力。

用户态部分提供系统调用封装、基础运行时、测试入口程序和 `testrunner`。比赛环境下，内核启动后由 `testrunner` 按顺序运行各组测例并输出评测机可识别的日志。

## 构建

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

## 运行

```bash
make rv           # 构建并运行 RISC-V 版本
make la           # 构建并运行 LoongArch 版本
make rv MEM=1G    # 指定 QEMU 内存
make la SMP=1     # 指定 QEMU CPU 数量
```

默认测试镜像路径：

- RISC-V: `img/sdcard-rv.img`
- LoongArch: `img/sdcard-la.img`

运行日志默认写入：

- RISC-V: `rv-output.txt`
- LoongArch: `la-output.txt`

## 目录结构

```text
RespOS/
├── Makefile              # 顶层构建与 QEMU 运行入口
├── os/                   # 内核源码
│   ├── src/arch/         # 架构相关代码
│   ├── src/drivers/      # 设备驱动
│   ├── src/fs/           # 文件系统与 VFS
│   ├── src/mm/           # 内存管理
│   ├── src/syscall/      # Linux ABI 风格系统调用
│   └── src/task/         # 进程、线程与调度
├── user/                 # 用户态库、测试程序与 testrunner
│   └── oscomp_ltp_list.txt
├── img/                  # 本地测试镜像
├── judge/                # LTP 日志解析与对比工具
├── docs/                 # 设计记录与调试文档
├── scripts/              # 辅助脚本
├── testsuit/             # 本地测例源码或资料
└── vendor/               # 第三方依赖源码
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
- iperf / netperf
- iozone
- lmbench
- LTP

其中 LTP 使用清单化运行方式，便于在不同阶段启用高收益测例、跳过高风险或高耗时测例。

## 开发提示

- 顶层 `Makefile` 是本地和评测环境的主要入口。
- 修改用户态程序后，需要重新构建对应架构，`user/build.rs` 会将用户程序打包进内核。
- RV 与 LA 构建会切换不同 Cargo 配置，不建议并行执行 `make build-rv` 和 `make build-la`。
- `docs/` 中保存了部分模块说明、性能优化记录和调试方案。
- `judge/` 中的脚本用于将 LTP 输出转换为表格，适合做阶段性回归检查。

## 文档

- `docs/mm模块基础说明.md`
- `docs/task模块核心功能说明.md`
- `docs/ltp-fs-abi-design.md`
- `docs/ltp-performance-optimization.md`
- `docs/比赛环境配置.md`

## 许可证

[GNU General Public License v2.0](LICENSE)
