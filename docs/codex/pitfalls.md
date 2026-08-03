# RespOS 已确认易错点

## `make rv/la` 返回 0 不等于测试通过

- 状态：已确认
- 适用范围：完整 QEMU 测试
- 最后验证：2026-08-01
- 证据：当前 `make rv`/`make la` 均退出 0，但 LTP 各有 600 余项失败
- 内容：顶层命令通过 pipefail 运行 QEMU 并 tee 日志；testrunner 即使记录用例失败仍会跑完并主动
  关机，因此外层成功主要表示 QEMU 正常结束。
- 后续影响：验收必须解析 summary、TBROK、segfault、wrapper exit code 和分组 fail。

## writable file `MAP_SHARED` 拒绝造成全局级联

- 状态：已确认
- 适用范围：basic、lmbench、LTP 以及依赖共享控制页的用户程序
- 最后验证：2026-08-01
- 证据：`os/src/fs/file.rs::mmap_allowed`、`os/src/syscall/mm.rs`、当前双架构日志
- 内容：当前策略返回 `EOPNOTSUPP`。LTP 框架自身使用 fd-backed writable shared page，所以看似
  无关的 getpid/fcntl/fs 等用例在初始化时一起 TBROK。
- 后续影响：不要逐个修数百个 LTP case；先修 shared file mapping 协议。也不要直接删除检查，
  因为旧实现仍有锁内 I/O 和写回错误传播风险。

## MM 锁内后端 I/O 是长期锁序风险

- 状态：已确认
- 适用范围：file fault、shared unmap/writeback
- 最后验证：2026-08-01
- 证据：`os/src/mm/memory_set.rs`、`docs/四天内核重构-ABC-整合审查.md`
- 内容：部分 file backing fault/writeback 仍可能持有 `MemorySet` 写锁访问文件后端；munmap 写回
  错误也没有完整传播协议。
- 后续影响：设计 writable shared 时把文件读写移到地址空间锁外，并在重新加锁后校验映射版本
  或身份，再 commit。

## LTP 的首个框架错误会污染后续结论

- 状态：已确认
- 适用范围：LTP harness 调试
- 最后验证：2026-08-01
- 证据：当前 mmap 初始化级联；`user/src/bin/testrunner.rs`；历史 `pipe2_02` helper 级联记录
- 内容：测试可能在 `tst_test.c` 资源准备、mount、helper exec 或共享控制页阶段失败，尚未进入目标
  syscall assertion。后续大量相同错误不是大量独立内核缺陷。
- 后续影响：先定位第一个不同的失败点，并从 testsuit/image 脚本确认它属于 setup 还是测试主体。

## 未跟踪用户程序仍会进入本地构建

- 状态：已确认
- 适用范围：`user/src/bin/` 与内核内嵌应用
- 最后验证：2026-08-01
- 证据：`user/Makefile` 的 `$(wildcard src/bin/*.rs)`；当前构建日志编译了未跟踪 FS probe
- 内容：Git 未跟踪不等于构建不可见。任何放在 `user/src/bin` 的 `.rs` 都会被构建并可能嵌入内核，
  影响大小、编译时间或应用清单。
- 后续影响：报告可复现结果时附 `git status --short`；干净 CI 与本地 dirty tree 可能产物不同。

## RV/LA 构建共享活动 Cargo config

- 状态：已确认
- 适用范围：顶层和子目录构建
- 最后验证：2026-08-01
- 证据：`Makefile`、`os/Makefile`、`user/Makefile`
- 内容：构建目标把架构模板复制到同一个 `.cargo/config.toml`。并行运行两个架构构建可能在
  config 被覆盖时使用错误 target/linker 设置。
- 后续影响：双架构顺序构建；若未来需要 `make -j`，先改成互不共享的配置/target dir 协议。

## 比赛镜像是可变测试状态

- 状态：已确认
- 适用范围：mount、文件、LTP、benchmark
- 最后验证：2026-08-01
- 证据：顶层 QEMU 以 raw 可写方式挂载 `img/sdcard-*.img`；`scripts/get_img.sh`
- 内容：测试会创建 `/tmp`、`/etc`、symlink、benchmark 文件和 mount backing。前一轮异常退出可能
  改变后一轮失败形态。
- 后续影响：难以解释的非确定性首先用保留的 `.xz` 恢复镜像，再比较；记录镜像版本和 hash。

## glibc 工具会暴露未覆盖的合法 open flag

- 状态：已确认
- 适用范围：glibc pub 镜像中的 coreutils/CAgent 文件命令
- 最后验证：2026-08-02
- 证据：`os/src/fs/file.rs::OpenFlags`、`os/src/syscall/fs.rs::validate_open_flags()`、
  glibc `/usr/bin/touch` 的 `EINVAL` 复现
- 内容：BusyBox 和 glibc 工具的等价操作不一定使用相同的 open flag。此次 glibc `touch`
  携带 Linux ABI 规定的 `O_NOCTTY`，旧实现因未声明该位在进入 namei 前返回 `EINVAL`。
- 后续影响：遇到用户态工具特定失败时，先保存实际 syscall flags 并对照 Linux ABI；对纯兼容、
  不改变普通文件状态的合法 flag 可接受为 no-op，不能按测试名添加特判。

## 网络 `Connection refused` 可能是 ready 时序而非 ABI

- 状态：已确认并修复（当前工作树）
- 适用范围：多个 client 同时连接同一个 smoltcp TCP listener
- 最后验证：2026-08-03
- 证据：CAgent debug 的 agent exit 255 与 `Connection failed to 127.0.0.1:8080`；
  `os/src/net/listen.rs`；修复后 RV64 三轮官方 CAgent 日志
- 内容：服务端已经 listen 不代表单个 smoltcp listener 能承接同一轮 poll 中的全部 SYN。旧实现等
  userspace `accept` 时才补 listener，第一个握手占用 listener 后，其余并发 SYN 会被 reset。该错误
  会随机落到任意 agent，因而可能伪装成 `kernel`、`cpu` 或 FS 命令失败。
- 后续影响：先保存 agent 原始日志和退出码；若是 connect 失败，不要修改 `uname`、`nproc` 等固定
  命令。监听实现必须按 backlog 提前提供容量，而不是在客户端硬编码重试。

## 在待恢复 journal 的镜像上写入 debug 文件会丢失

- 状态：已确认
- 适用范围：从 `.gz` 恢复 pub ext4 镜像后使用 `debugfs -w`
- 最后验证：2026-08-03
- 证据：RV64 镜像注入的 `/glibc/cagent_debug.sh` host/image SHA-256 一致，但首次启动 journal
  recovery 后 guest 中变为 0 字节；先运行 `e2fsck -pf` 后重新注入可正常执行
- 内容：压缩包中的 ext4 可能带 `needs journal recovery`。在回放旧 journal 前直接写新 inode，启动
  时的恢复会用旧元数据覆盖该写入。
- 后续影响：只读查看不受影响；临时写镜像前先恢复 journal 并再次校验内容。正式干净回归不要
  注入 debug 文件。

## lwext4 的 CMake 生成目录也跨架构共享

- 状态：已确认
- 适用范围：RV64/LA64 快速切换构建
- 最后验证：2026-08-03
- 证据：`vendor/lwext4_rust/build.rs`、`c/lwext4/Makefile`；失败日志中 LA C compiler 配合 RV
  `ar/ranlib`，手动顺序执行目标架构 `make musl-generic ARCH=...` 后重试成功
- 内容：除了活动 Cargo config，lwext4 两架构还共用 `c/lwext4/build_musl-generic`。切换架构时
  可能出现 C compiler 已切换但 archive 工具仍来自上一架构的混合配置。
- 后续影响：不要并行构建两架构。若顶层构建在 archive 阶段出现交叉工具混用，先顺序执行
  `make musl-generic ARCH=riscv64|loongarch64 -C vendor/lwext4_rust/c/lwext4` 再重试；长期应把
  CMake build dir 按架构拆分。

## libctest wrapper 的 256 需要解码

- 状态：待验证
- 适用范围：musl libctest static/dynamic
- 最后验证：2026-08-01
- 证据：RV/LA 日志均显示 `run-static.sh`、`run-dynamic.sh exited with code 256`
- 内容：可见子用例大量显示 `Pass!`，但 runner 打印的是 wait status/封装值，尚未确认是脚本真实
  exit 1、状态解码问题还是隐藏子用例失败。
- 后续影响：检查 waitpid ABI 和脚本最终命令，不能把“屏幕上都是 Pass”当成 wrapper 成功。

## 高半区模型与 LoongArch 2 MiB 边界历史问题

- 状态：待验证
- 适用范围：LoongArch mmap、TLS、pthread、页表边界
- 最后验证：2026-08-01
- 证据：当前高半区源码已确认；2 MiB TLS 故障仅来自 2026-06 历史 Codex 记忆，当前源码未重现
- 内容：历史调试曾观察到小匿名 mmap 跨 2 MiB 页表边界与 pthread/TLS 故障相关，并采用 LA
  特定 mmap placement 缓解。当前是否仍需要该缓解尚未专项复验。
- 后续影响：若 LA-only pthread fault 重新出现，优先记录 fault VA、TP/TLS、PMD 边界和 mmap
  placement；在复现前不要把旧解释写成当前缺陷。

## rename/unlink 仍受 path-based ext4 后端限制

- 状态：已确认
- 适用范围：多硬链接、rename 覆盖、unlink 后打开文件
- 最后验证：2026-08-01
- 证据：`os/src/fs/ext4/inode.rs`、`os/src/fs/namei.rs`、整合审查
- 内容：open-file 计数已替代 `Arc::strong_count` 猜测，但后端仍通过 renamed/orphan path 兼容
  path API；多别名的完整 inode-handle/事务语义尚未建立。
- 后续影响：不要用更多路径特判宣称完整 POSIX 语义；复杂 rename 需要统一 inode identity/backup
  设计和失败注入。

## 历史测试成绩会快速过期

- 状态：已确认
- 适用范围：README、汇报和分支比较
- 最后验证：2026-08-01
- 证据：README 历史“600 余 LTP”与当前 LTP 初始化失败并存
- 内容：同一仓库的不同 commit、镜像、内存配置、libc 和 runner 清单会产生完全不同的结果。
- 后续影响：任何成绩必须携带 commit、日期、架构、镜像、命令和 summary；旧记忆只用于寻找线索。
