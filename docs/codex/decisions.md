# RespOS 设计决策记录

这里只收录能解释当前代码形态或避免重复踩坑的决策。日期是当前证据最后核验时间，不一定是
最初提出时间。

## 普通 close 不执行全文件系统持久化屏障

- 状态：已采用，完整 BuildStorm 计时待验证
- 适用范围：普通文件 close、PageCache writeback、fsync/sync、ext4 shutdown
- 最后验证：2026-08-09
- 证据：`os/src/fs/file.rs`、`os/src/fs/ext4/super_block.rs`；RV64 8 核
  `buildstorm_file_probe` 与 `/proc/respos_perf`
- 内容：普通 `close(2)` 不等价于 `fsync(2)`，不得在每个 `File::drop()` 中调用
  `ext4_cache_flush("/")` 或 block-device FLUSH。当前 inode cache 仍以 weak reference 为主，因此
  close 会把该文件现有脏页提交给 lwext4，避免最后 inode/PageCache 消失时丢数据；全文件系统和
  设备持久化只由显式 fsync/sync 及正常卸载触发。
- 后续影响：若改为强引用 inode/page cache 和后台 writeback，可进一步取消 close 数据写回；在此之前
  不能以性能为由直接丢弃脏页。崩溃一致性和正常 shutdown 的 virtio FLUSH 门禁保持不变。

## lwext4 连续对齐块使用 multi-block VirtIO 请求

- 状态：已采用，完整 BuildStorm 计时待验证
- 适用范围：lwext4 `KernelDevOp` 到 `BlockDevice` 的读写
- 最后验证：2026-08-09
- 证据：`os/src/drivers/disk.rs`、`os/src/drivers/virtio/block_dev.rs`；RV64 8 核
  `buildstorm_file_probe`
- 内容：对齐且连续的 512-byte blocks 合并为一次 `read_blocks`/`write_blocks` 调用；只有非对齐
  头尾继续单块读取或 read-modify-write。块层计数记录实际 VirtIO 请求数和字节数。
- 后续影响：不得为了继续合并而跨越非连续 LBA 或跳过 partial-block 保留内容；异步队列和多队列属于
  后续独立优化。

## 高频诊断设施必须由 kernel feature 静态隔离

- 状态：已采用
- 适用范围：BuildStorm 性能计数器、进程/退出/pipe/futex/timer 等详细串口 trace
- 最后验证：2026-08-09
- 证据：`os/src/perf.rs`、`os/src/console.rs`、`os/Cargo.toml`；RV64/LA64 feature 组合构建及
  RV64 `/proc/respos_perf`/trace 烟测
- 内容：聚合原子计数只由 `perf_counters` 启用，高频串口路径只由 `debug_traces` 启用；正式计时两者
  均关闭。业务调用统一使用编译为空操作的计数函数和 `debug_trace!`，不得在热点中以运行时布尔值
  保留格式化、timer 采样或原子更新成本。
- 后续影响：临时定位问题时优先扩展现有门控和 proc 汇总，不再新增无条件串口输出或平行计数框架；
  panic、启动和不可恢复错误输出不属于该高频诊断边界。

## 共享高半区内核模型

- 状态：已确认
- 适用范围：RV64/LA64 虚拟内存、启动、linker、页表
- 最后验证：2026-08-01
- 证据：`os/src/linker_riscv.ld`、`os/src/linker_loongarch.ld`、
  `os/src/arch/*/config/mm.rs`、`os/src/main.rs`
- 内容：两个架构保持 `0xffffffc0...` 高半区共享内核布局。LoongArch 从低物理入口建立早期
  分页再跳转高半区，不采用与公共内核假设割裂的独立低地址模型。
- 后续影响：LoongArch 修复应尽量收敛在 arch 层，不能未经全局审计改变公共地址空间模型。

## syscall 保持薄层，领域对象拥有状态机

- 状态：已确认
- 适用范围：MM、task、FS、net syscall
- 最后验证：2026-08-01
- 证据：`os/src/syscall/mm.rs`、`os/src/mm/memory_set.rs`、Git `15fe1a5`；A/B/C 文档
- 内容：syscall 负责 ABI 参数解析、fd/用户指针获取和 errno 映射；VMA、调度、namei、socket
  等状态转换由领域模块维护。
- 后续影响：发现 syscall 文件重复维护缓存、映射表或对象生命周期时，应优先下沉到所有者。

## 用户指针通过页表逐页访问

- 状态：已确认
- 适用范围：copyin/copyout、字符串、iovec、futex 预检查
- 最后验证：2026-08-01
- 证据：`os/src/mm/mod.rs`、Git `15fe1a5`
- 内容：用户地址可能跨页、跨相邻 VMA、处于 lazy 或 COW 状态。通用 copy 先检查整段权限并
  确保页可访问，再通过 PTE 对应 frame 复制，避免 kernel page fault。
- 后续影响：不要在 syscall 中直接构造用户 slice 或逐字节裸解引用。

## 高风险状态修改采用 prepare/copyout/commit

- 状态：已确认
- 适用范围：wait4、shmat、timer、prlimit、rename 等可失败操作
- 最后验证：2026-08-01
- 证据：Git `3aa1fb5`、`15fe1a5`、`cba8e24`；
  `docs/四天内核重构-ABC-整合审查.md`
- 内容：先验证并准备资源，用户 copyout 成功后再提交不可逆状态；失败不能重复累计、提前删除
  zombie、破坏旧映射或泄露半成品对象。
- 后续影响：review 时要逐项列出失败点和其前后的可见状态，不能只检查成功路径。

## descriptor flags 与 open-file status flags 分离

- 状态：已确认
- 适用范围：fdtable、dup、fcntl、exec、CLOEXEC
- 最后验证：2026-08-01
- 证据：`os/src/fs/fdtable.rs`、`os/src/fs/file.rs`、Git `cba8e24`
- 内容：CLOEXEC 属于每个 `FdEntry`；offset 和 O_APPEND/O_NONBLOCK 等 open-file 状态属于共享
  `File`。dup 共享后者，不共享前者。
- 后续影响：任何 fd 复制、进程复制和 fcntl 实现都必须保持这一身份关系。

## 未实现状态型 ABI 必须诚实失败

- 状态：已确认
- 适用范围：mmap/madvise/special fd/fs flags 等
- 最后验证：2026-08-01
- 证据：A/B/C 重构文档、Git `cba8e24`/`15fe1a5`
- 内容：会改变长期状态或让用户依赖后续语义的能力，未实现时返回明确 errno，不能无条件 0。
  纯 hint/no-op 只有在 ABI 允许且完成参数校验时才可受限接受。
- 后续影响：恢复旧测例不能以重新引入假成功为代价；应修实现或证明用例依赖非目标行为。

## writable file `MAP_SHARED` 使用锁外快照写回

- 状态：已验证（受限 ABI 子集）
- 适用范围：文件 mmap/msync/munmap/writeback；RV64/LA64
- 最后验证：2026-08-02
- 证据：`os/src/mm/memory_set.rs`、`os/src/syscall/mm.rs`、`os/src/syscall/fs.rs`、
  `os/src/task/task.rs`；RV64/LA64 LTP `mmap001` 与 mmap/munmap 子集回归
- 内容：可写 `MAP_SHARED` 不再被无条件拒绝。共享文件页在建立映射前锁外预取；写回时在
  `MemorySet` 锁内复制 resident frame 快照，释放锁后通过 `FileOp` 写入页缓存；`MS_SYNC`
  再执行 `fsync`。munmap、MAP_FIXED 替换、mremap 覆盖/收缩、mprotect 和进程退出复用同一
  写回协议，写回失败可返回给 syscall，且不会在 MM 锁内执行后端 I/O。当前没有硬件 dirty bit，
  所以对 resident writable shared file pages 保守写回。
- 后续影响：`MS_INVALIDATE` 仍明确返回 `EOPNOTSUPP`，因为共享文件 frame 全局缓存没有
  inode-wide 失效协议；文件截断后的 mapped-page `SIGBUS` 规则仍需单独实现，写回时不会因旧
  映射重新扩展当前 EOF。

## 关键回归必须双架构并分析日志

- 状态：已确认
- 适用范围：进入集成/main 的测试门禁
- 最后验证：2026-08-01
- 证据：顶层 `Makefile`、A/B/C 验收文档、当前 `make rv`/`make la` 结果
- 内容：构建门禁至少覆盖 RV/LA；运行门禁使用仓库根目录真实入口。QEMU 正常关机和 make 退出
  0 只表示 runner 结束，必须解析测试 summary 和失败标记。
- 后续影响：提交说明应分别陈述 build、boot、专项 probe、完整 workload，不使用笼统“测试通过”。

## SMP kernel timer 只在无任务锁的安全点执行高层工作

- 状态：已确认
- 适用范围：RV64 SMP timer、timeout、signal、futex/timerfd/POSIX timer registry
- 最后验证：2026-08-06
- 证据：`os/src/arch/rv64/trap/mod.rs`；2 核 `ACTIVE_ITIMER_TASKS` 中断重入 GDB 栈
  `/tmp/respos-smp2-dynamic-bt.txt`；2/4/8 核各三轮退出压力
- 内容：普通 kernel-mode timer trap 只确认并重编程 tick，不调用会获取 task/signal/timer
  锁的 `check_all_task_timers()`。高层 timer work 当前由 user-mode timer trap 和 boot hart 的无 current
  idle context 串行服务。
- 后续影响：不以“把某一把锁换成 NoIrq”逐个遮掩高层中断重入；需更强及时性时改为
  中断只发布 pending，再在明确安全点消费。

## RV64 shootdown 复用目标 OpenSBI 的同步 RFENCE

- 状态：已确认（仅限当前 QEMU/OpenSBI 目标）
- 适用范围：RV64 SMP `MemorySet::flush_tlb`
- 最后验证：2026-08-06
- 证据：RISC-V SBI RFENCE 规范；OpenSBI 1.5.1 `sbi_tlb_request`、`tlb_sync` 实现；
  `smp_shared_mm_probe` 2/8 核日志
- 内容：OS 维护真实 active hart mask；PTE 发布后调用 SBI remote SFENCE。当前 OpenSBI 的
  TLB IPI event 带 per-source sync counter，发起方在远端执行 fence 并 ack 前不返回，因此不再
  叠加一套重复的 S-mode shootdown IPI 队列。
- 后续影响：这是平台契约，不是 SBI 文本对所有 firmware 的无条件完成性推论。更换 firmware、
  绕过 SBI 或加入 ASID 后必须重新审计 request/ack、mask 与 frame 回收顺序。

## filesystem exec 不以扩大固定 kernel heap 支持大 ELF

- 状态：已采用
- 适用范围：filesystem ELF loader、BuildStorm、kernel heap budget
- 最后验证：2026-08-06
- 证据：`os/src/mm/memory_set.rs::try_from_elf_file()`；45,559,552 字节 cargo 的
  `BUILDSTORM_TOOLCHAIN ok` 日志 `/tmp/respos-buildstorm-rv8-file-backed-exec.log`
- 内容：大 ELF 通过 file-backed PT_LOAD 按需装页；不提高 `KERNEL_HEAP_SIZE`、不放宽
  `read_all()` 的半堆保护来承受完整 executable Vec。这样 exec 临时内存随 header 大小而非文件大小
  增长，并把段页生命周期纳入已有 VMA/frame 模型。文件式 exec 的 header/PT_INTERP 元数据前缀
  上限为 1 MiB；超出上限或 PT_LOAD 越过文件末尾的 ELF 返回 `ENOEXEC`，不得触发无界内核分配。
- 后续影响：若扩展到 interpreter 或其他架构，继续传递 file identity/offset/length；不得为单个
  workload 设置路径特判或无界整文件 allocation。

## 优化时保持原有有效测例兼容

- 状态：暂定
- 适用范围：`dev` 后续重构
- 最后验证：2026-08-01
- 证据：当前维护目标；尚待后续提交和回归矩阵固化
- 内容：结构优化应尽量保持已有有效 ABI 测例；如果旧测例依赖取巧实现，可以排除，但必须
  给出源码级原因和替代验证，不能仅因失败就标记为“无效”。
- 后续影响：下一阶段先恢复 LTP harness 可运行性，再以真实失败驱动窄范围修复。

## smoltcp TCP 使用受限 backlog listener 池

- 状态：已验证（当前工作树）
- 适用范围：loopback TCP listen/accept 与并发连接
- 最后验证：2026-08-03
- 证据：`os/src/net/listen.rs`、`os/src/net/mod.rs`、`os/src/net/tcp.rs`；
  `/tmp/cagent-a-rv-run{1,2,3}.log`
- 内容：不在 `connect` 上用测试相关重试掩盖 `ECONNREFUSED`。由于一个 smoltcp listener 同时
  只能承接一个握手，内核按传入 backlog（上限 128、下限 1）预建 listener handle；完成握手的
  handle 进入 accept queue，协议 poll 和 accept 路径负责补充空位。
- 后续影响：backlog 会占用 TCP 收发缓冲内存，不能无限接受用户值；close/unlisten 必须回收仍在
  listener 池和 accept queue 中的所有 handle。若以后替换为原生 SYN queue，应保持相同的
  userspace 可观察并发语义。
