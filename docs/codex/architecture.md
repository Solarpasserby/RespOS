# RespOS 架构与不变量

本文只记录当前源码可验证的结构和长期约束。更细的专题设计可继续阅读
`docs/mm模块基础说明.md`、`docs/task模块核心功能说明.md`、
`docs/ltp-fs-abi-design.md` 与 A/B/C 重构文档。

## 启动与执行链

```text
架构入口 / linker
  → rust_main：清 BSS；LA 建立早期分页并跳转高半区
  → rust_main_high
  → trap::init
  → mm::init
  → net::init
  → task::add_initproc
  → 开启并设置下一次时钟中断
  → task::run_tasks
  → initproc / testrunner / 比赛镜像程序
```

### 初始化顺序不可随意交换

- 状态：已确认
- 适用范围：`os/src/main.rs` 与各全局子系统
- 最后验证：2026-08-01
- 证据：`os/src/main.rs`、`os/src/mm/mod.rs`、`os/src/net/mod.rs`
- 内容：MM 初始化先于网络全局对象和 initproc，initproc 入队后才开启周期调度。LoongArch 在
  进入公共高半区路径前还有早期分页和架构扩展初始化。
- 后续影响：新增依赖 allocator、页表或 timer 的全局对象时，要确认首次访问发生在相应子系统
  初始化之后。

## 内核边界

| 边界 | 当前所有者 | 不应放入调用方的内容 |
| --- | --- | --- |
| syscall → MM | `MemorySet`、VMA/PTE/frame helper | VMA 切分、COW、lazy fault、file backing 状态机 |
| syscall → task | TCB、thread group、scheduler、futex wait state | 退出回收和 waiter single-winner 规则 |
| syscall → FS | namei/VFS/FileOp/FdTable | path walk、descriptor/open-file 状态混用 |
| syscall → net | socket FileOp 与 `os/src/net` | smoltcp socket 生命周期和 listen table 细节 |
| 公共代码 → arch | `os/src/arch/{rv64,loongarch64}` | 直接散落的 CSR/寄存器、页表和 trap-frame 假设 |

## MM 模型

### `MemorySet` 拥有地址空间语义

- 状态：已确认
- 适用范围：mmap、munmap、mprotect、mremap、COW、lazy allocation、用户拷贝
- 最后验证：2026-08-01
- 证据：`os/src/mm/memory_set.rs`、`os/src/mm/mod.rs`、`os/src/syscall/mm.rs`、
  Git `15fe1a5`
- 内容：VMA 由 `MemorySet` 管理；`MmapBacking` 表达匿名/文件和 shared/private backing；
  `MapArea::split_by_overlap` 统一切分。用户拷贝逐页检查 VMA 权限、确保 lazy/COW 页可访问，
  再通过 PTE 对应物理页复制，不能直接解引用用户虚拟地址。
- 后续影响：所有地址范围必须检查加法、页对齐和用户上界；任何可能失败的替换应先准备资源，
  再原子更新 PTE/VMA，避免留下空洞。

### VMA/PTE/frame 不变量

- 状态：已确认
- 适用范围：`MemorySet` 修改路径
- 最后验证：2026-08-01
- 证据：`os/src/mm/memory_set.rs` debug invariant 与 split self-test；Git `15fe1a5`
- 内容：VMA 有序、非空且不重叠；`data_frames` 的 VPN 属于对应 VMA 并具有有效 PTE；用户
  PTE 带 USER；VMA/PTE 权限相容；私有 COW 页不同时可写；shared mapping 不进入私有 COW。
- 后续影响：debug 启动自检失败应视为结构性错误，不能通过关闭断言完成验收。

### RV64 `MemorySet` active CPU 与 shootdown

- 状态：已实现，目标 QEMU/OpenSBI 专项压力已通过
- 适用范围：RV64 SMP、共享 `MemorySet`、PTE 修改与 frame 回收前 fence
- 最后验证：2026-08-06
- 证据：`os/src/mm/memory_set.rs`、`os/src/task/processor.rs`、`os/src/task/task.rs`；
  `/tmp/respos-active-mask-shared-mm-smp{2,8}.log`
- 内容：task 恢复前在 `MemorySet` 读锁内设置当前 hart active bit；切回 per-CPU idle/kernel
  页表后才清除。页表修改持写锁，完成本地 fence 后，只向 active remote hart 发 SBI RFENCE。
  `exec` 与 clone 的临时 `activate()` 必须显式撤销旧/临时地址空间 bit。当前 OpenSBI RFENCE
  实现会等待远端 TLB 请求同步计数归零后返回。
- 后续影响：不得在仍执行旧 `satp` 时提前清 bit，也不得绕过 `MemorySet` 锁修改 PTE。移植到
  非 OpenSBI firmware 时必须重新确认 RFENCE completion 语义；未知实现不能沿用当前验收结论。

## task、scheduler 与 futex

### 调度状态只有一个所有者提交

- 状态：已确认
- 适用范围：ready/block/wakeup/exit/context switch
- 最后验证：2026-08-01
- 证据：`os/src/task/scheduler.rs`、`os/src/task/processor.rs`、Git `3aa1fb5`
- 内容：scheduler 使用 RT、normal、idle 多级队列并维护 `task_index`/blocked 集合；状态先准备，
  再由调度路径提交。退出任务通过 `DEAD_TASKS` 延迟 drop，避免在自身内核栈上释放自身。
- 后续影响：不要从多个路径重复入队或重复唤醒；close/signal/timeout 竞争必须保持 single-winner。

### SMP ready 选择与唤醒必须同时遵守 affinity

- 状态：已实现，RV64 8 核定向烟测已通过，退出压力仍有独立 blocker
- 适用范围：全局 ready queue、TCB CPU owner、RV64 idle hart IPI
- 最后验证：2026-08-06
- 证据：`os/src/task/scheduler.rs`、`os/src/task/task.rs`、`os/src/task/processor.rs`、
  `os/src/arch/rv64/smp.rs`；`/tmp/respos-affinity-smp8.log`
- 内容：每个 CPU 从最高优先级开始，在队列内选择第一个 affinity 允许且 owner 已释放的
  task；不兼容的队首保留原位。任务发布或 owner 释放后，IPI 目标只从 affinity 允许的
  online idle hart 中选择。
- 后续影响：不能只在 dequeue 中过滤 affinity。若 enqueue 仍唤醒任意 idle CPU，只允许高编号
  CPU 的任务可能永久留队。owner 在 handoff 后释放时还必须补一次定向 kick，覆盖第一次
  IPI 早于 context save 完成的窗口。

### 全局 heap 和 kernel timer work 的中断边界

- 状态：已实现，RV64 2/4/8 核退出压力已验证
- 适用范围：全局内核 heap、RV64 user/kernel timer trap、timeout/signal registry
- 最后验证：2026-08-06
- 证据：`os/src/mm/heap_allocator.rs`、`os/src/arch/rv64/trap/mod.rs`；
  `/tmp/respos-smp8-gdb-bt1.txt`、`/tmp/respos-smp2-dynamic-bt.txt`
- 内容：`LockedHeap` 的跨 CPU 自旋锁不能防止同 CPU 中断重入；所有 heap alloc/dealloc 及直接
  heap guard 都必须在本地 `InterruptGuard` 内执行，解锁顺序为先 heap、后恢复中断。
  `check_all_task_timers()` 会进入 task/signal/timer 高层锁，不得从中断任意 kernel 临界区的
  timer trap 重入。RV64 当前安全点是 user-mode timer trap，以及 boot hart 上
  `current_task == None` 的 per-CPU idle context。
- 后续影响：新增中断内工作前要列出其触及的所有锁；若需在长 syscall 期间精确处理
  timeout，应建立 lock-free pending + 安全点延迟工作，不能直接恢复 kernel trap 里的高层扫描。

### futex 锁内不触发用户缺页

- 状态：已确认
- 适用范围：futex wait/wake/requeue
- 最后验证：2026-08-01
- 证据：`os/src/task/futex/wait.rs`、`os/src/mm/mod.rs`、Git `3aa1fb5`
- 内容：可能 fault 的用户页检查在 futex queue 临界区之外完成；比较重排锁内使用固定 4 字节
  no-fault PTE 读取。wait completion 区分 Pending/Woken/TimedOut/Interrupted。
- 后续影响：不能在全局 futex queue lock 下调用一般 `copy_from_user` 或分配 frame。

## FS、VFS 与 fd 模型

```text
FdTable slot (FdEntry: descriptor flags)
  → Arc<dyn FileOp> (open-file description: offset/status)
  → File (Path + InodeOp + page cache)
  → Path (VfsMount + Dentry)
  → Dentry (name/parent/inode)
  → InodeOp / ext4 backend
```

### descriptor 与 open-file description 分层

- 状态：已确认
- 适用范围：dup/fcntl/close/exec/CLOEXEC/offset/status flags
- 最后验证：2026-08-01
- 证据：`os/src/fs/fdtable.rs`、`os/src/fs/file.rs`、Git `cba8e24`
- 内容：`FdEntry` 保存 descriptor flags；`FileInner` 保存共享 offset、path 和 open-file status
  flags。dup 后 descriptor 可独立设置 CLOEXEC，但共享同一个 open-file offset/status。
- 后续影响：实现新 fcntl 或 clone/exec 行为时，不得把 CLOEXEC 写进共享 `File` flags。

### filesystem ELF 使用按需 private file backing

- 状态：已实现，BuildStorm toolchain 阶段已验证
- 适用范围：filesystem exec、动态程序、大 ELF、kernel heap
- 最后验证：2026-08-06；RV64 release、8 核、8 GiB
- 证据：`os/src/mm/memory_set.rs::try_from_elf_file()`、
  `os/src/task/task.rs::execve_file()`；`/tmp/respos-buildstorm-rv8-file-backed-exec.log`
- 内容：exec 只把 ELF/program header 与 PT_INTERP 名称读入 kernel heap；主 ELF 的 PT_LOAD VMA
  持有 `Arc<dyn FileOp>`、page-aligned file offset 和有效文件长度，private fault 时分配独立 frame
  并按页读取，BSS 尾部保持零。文件式 loader 将元数据前缀限制为 1 MiB，并在安装 VMA 前校验
  ELF64 program-header 尺寸和 PT_LOAD 文件边界；嵌入式 app 仍使用完整 slice 的 eager loader。
- 后续影响：PT_LOAD offset 与 virtual address 的页内偏移必须同余；不能重新用 `read_all()` 或
  简单放大 kernel heap 规避大 ELF。动态链接器本身目前仍整文件读取，后续可用同一抽象继续收敛。

### vfork 父任务必须先登记 blocked 再发布子任务

- 状态：已实现，完整 BuildStorm 仍待验证
- 适用范围：RV64 SMP `CLONE_VFORK`、posix_spawn、Rust Command/cargo
- 最后验证：2026-08-06；RV64 release、8 核受控 trace
- 证据：`os/src/syscall/process.rs::sys_clone()`；
  `/tmp/respos-buildstorm-cloneflags.log`、`/tmp/respos-buildstorm-exittrace.log`
- 内容：vfork parent 的 blocked registration 是 wakeup 协议的一部分，必须发生在 child 加入全局
  ready queue 之前。发布后 child 可在任意 CPU 立即 exec；exec/exit 的一次性 wake 此时必能从
  blocked 表取回 parent。父任务登记后再发布 child，随后直接切到 idle/下一任务。
- 后续影响：任何“发布对象后再登记 waiter”的一次性事件都要审计同类 lost-wakeup 窗口；单核
  测试因 child 无法提前运行，不能覆盖该顺序错误。

### path、dentry、inode 与 mount 各有身份

- 状态：已确认
- 适用范围：namei、rename/unlink、mount crossing、proc/dev/ext4
- 最后验证：2026-08-01
- 证据：`os/src/fs/path.rs`、`os/src/fs/vfs/dentry.rs`、`os/src/fs/namei.rs`、
  `os/src/fs/mount.rs`
- 内容：`Path` 是 mount+dentry；dentry 表达目录项身份和父子关系；inode 表达文件对象；路径
  查找由 namei 处理 `dirfd`、`.`/`..`、symlink 与 mount crossing。
- 后续影响：不能用 `Arc::strong_count` 推断 inode 是否仍被打开；当前 ext4 使用明确 open-file
  计数。多硬链接与 rename 仍受后端 path API 限制，见 `pitfalls.md`。

## 网络模型

### smoltcp 回环路径是当前主要实现

- 状态：已确认
- 适用范围：TCP/UDP socket 与本地网络 benchmark
- 最后验证：2026-08-03
- 证据：`os/src/net/mod.rs`、`os/src/net/listen.rs`、`os/src/net/socket.rs`、
  `os/src/syscall/net.rs`；RV64 CAgent 三轮并发回归
- 内容：socket syscall 经 FileOp socket 对象进入 TCP/UDP 实现，全局 `SocketSet`、loopback
  interface/device 和 listen table 由锁保护，`poll_interfaces` 驱动协议栈。smoltcp 的单个
  listener 完成握手后会成为连接 socket，因此 listen table 按受限 `listen(backlog)` 预建
  listener 池；poll 将已连接 handle 转入 accept queue 并及时补位，避免同一轮并发 SYN 在
  userspace `accept` 前收到 reset。
- 后续影响：网络失败应先区分 ABI、协议状态和测试服务端启动时序，不能只凭一次
  `Connection refused` 判断内核网络语义损坏。修改 poll/accept/close 时必须同时维护 listener
  池、accept queue 和 socket handle 的唯一所有权，避免泄漏或重复 remove。

## 双架构差异

| 项目 | RISC-V 64 | LoongArch 64 |
| --- | --- | --- |
| 目标 | `riscv64gc-unknown-none-elf` | `loongarch64-unknown-none` |
| QEMU 启动 | OpenSBI/`-bios`，物理入口 `0x80200000` | `-kernel` ELF，物理入口 `0x00200000` |
| block 设备 | virtio-mmio | virtio-pci |
| 高半区 | `0xffffffc0...` | 同一共享高半区模型 |
| 早期分页 | 启动路径已进入公共高半区模型 | `rust_main` 显式启用 boot paging 后跳高半区 |
| 内核栈/堆 | 约 60 KiB 栈、64 MiB 堆 | 32 KiB 栈、48 MiB 堆 |

### 当前不是已验证 SMP 内核

- 状态：待验证
- 适用范围：多核调度、全局锁、TLB shootdown
- 最后验证：2026-08-01
- 证据：顶层 `Makefile` 默认 `SMP=1`；整合审查明确缺少真实 SMP 压力证据
- 内容：构建和 QEMU 参数允许设置 `SMP`，但当前验收只覆盖单核，不能据此宣称多核正确。
- 后续影响：任何 SMP 工作要单独设计 per-CPU 状态、跨核唤醒和 TLB 同步测试。
