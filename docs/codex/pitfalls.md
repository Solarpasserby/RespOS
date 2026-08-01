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

## 网络 `Connection refused` 可能是 ready 时序而非 ABI

- 状态：暂定
- 适用范围：iperf/netperf loopback
- 最后验证：2026-08-01
- 证据：RV 在 musl 组失败、LA 在 glibc 组失败，而相邻/对应组可成功
- 内容：当前失败没有固定在单一架构或 libc，形态为并行 client 建连时服务端尚未接受连接。
- 后续影响：先给 runner 增加 server-ready handshake、有限重试和进程清理日志，再判断是否为 listen
  backlog/socket 状态机缺陷。

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
