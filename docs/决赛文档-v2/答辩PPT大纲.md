# RespOS 决赛答辩 PPT 大纲

> 建议版本：24 页主讲，约 18 分钟；另备 6 页答疑附录。  
> 内容重点：进程管理、内存管理、文件系统、IPC 与信号。  
> 核心叙事：RespOS 不是系统调用的堆叠，而是一套围绕“对象所有权、并发提交和跨模块生命周期”建立的 Linux 兼容内核。

## 一、参考 PPT 后的编排原则

两份参考材料各有一种值得借鉴的表达方式：

- NPUcore-IMPACT 以“问题—攻关—成果”组织内容，项目辨识度强；
- StarryX 以“整体架构—模块展开—结果总结”组织内容，结构清晰；
- RespOS 建议结合两者：先给出整体对象关系，再用真实工程难题解释关键设计，最后用有边界的测试证据收束。

制作时遵循以下约束：

1. 每页只回答一个问题，标题尽量直接写成结论。
2. 主讲页以对象图、状态机、时序图为主，不大段粘贴源码或文档表格。
3. 每个模块至少包含一页“为什么这样设计”，避免成为功能清单。
4. 测试结果必须同时写明架构、核数、日期或测试范围，不使用无来源的总分和通过率。
5. 对尚未完成的能力明确写边界，体现工程判断，而不是隐藏问题。

## 二、主讲大纲

### 第 1 页　封面

**标题：** RespOS：面向真实 Linux 负载的双架构 Rust 内核  
**副标题：** 进程、内存、文件与 IPC 的统一生命周期设计

页面内容：

- 队伍、学校、成员、指导教师；
- RISC-V 64 / LoongArch 64；
- Rust、Linux ABI、ext4、SMP 四个关键词。

建议画面：简洁的 RespOS 标志，背景用“用户程序 → syscall → 四个内核模块 → 双架构”的淡化框图。

讲解重点：一句话说明项目目标——让真实 libc、shell 和工具链能够在自主内核上形成闭环，而不是只通过孤立测例。

建议时间：20 秒。

### 第 2 页　我们解决的不是四个模块，而是一个运行闭环

**核心结论：** 真实程序运行要求任务、地址空间、fd 和异步事件在同一生命周期内保持一致。

页面内容：

- `TaskControlBlock`：谁在执行；
- `MemorySet`：它能访问什么；
- `FdTable / FileOp`：它持有什么 I/O 对象；
- signal / pipe / futex / shm：它如何与其他任务协作；
- VFS / ext4 / page cache：数据如何命名、缓存和持久化。

建议图示：以 TCB 为中心的环形对象关系图，四个重点模块围绕它连接；不要先列 syscall 数量。

讲解重点：四章内容不是并列功能，`fork/exec/exit`、COW、CLOEXEC、pipe EOF 和信号中断都依赖同一条资源引用链。

建议时间：45 秒。

### 第 3 页　贯穿 RespOS 的三条设计原则

**标题直接写结论：** 所有权确定释放点，prepare/commit 保证失败原子性，single-winner 解决并发竞争

页面内容：

| 原则 | 代表对象 | 解决的问题 |
| --- | --- | --- |
| 明确所有权 | `Arc/Weak`、TCB、Frame、FileOp | 谁持有、何时释放、如何共享 |
| prepare/commit | exec、wait4、共享 mmap 写回、shmat | 失败不能留下半完成状态 |
| single-winner | futex、signal、timeout、exit | 竞争事件不能重复唤醒或永久丢失 |

建议图示：三个相互咬合的齿轮；每个齿轮只放一个原则和两个代表案例。

讲解重点：后续每个模块都回扣这三条原则，让评委形成稳定记忆点。

建议时间：45 秒。

### 第 4 页　系统总览：一次真实程序执行经过哪些层

**核心结论：** RespOS 用统一 syscall 与对象抽象连接用户态、VFS、任务、内存和双架构后端。

建议图示：只画一张横向调用链。

```text
glibc / BusyBox / cargo
        ↓ Linux ABI
syscall：参数解析、用户拷贝、errno
        ↓
TCB ── MemorySet ── FdTable/FileOp ── Signal/IPC
        ↓                 ↓
    PageTable          VFS / page cache / ext4
        ↓                 ↓
 RV64 / LA64 arch      virtio block
```

页面右下角放四个后续问题：任务如何活？页面何时驻留？文件如何持久化？等待如何被安全唤醒？

建议时间：55 秒。

### 第 5 页　统一任务模型：进程和线程都是 TCB

**核心结论：** 调度器只调度 TCB，进程语义由 tid/tgid 与资源共享边界组合得到。

页面内容：

- 每个 TCB 独占 tid、内核栈、trap context、调度状态和线程信号状态；
- 同一线程组共享 tgid、`MemorySet`、`FdTable` 和 signal handler；
- `children` 强引用保留 wait status，全局索引和线程组使用 `Weak` 避免引用环。

建议图示：使用文档“进程、线程与资源对象关系图”的简化版；用实线表示 `Arc`，虚线表示 `Weak`。

讲解重点：强调“共享与独占由对象关系决定”，而不是在每个 syscall 中临时判断进程或线程。

建议时间：55 秒。

### 第 6 页　fork、exec、exit 是三套提交协议

**核心结论：** 创建、替换和退出都先准备资源，再在唯一提交点改变用户可见状态。

页面内容：

- fork/clone：按 flag 选择共享 `MemorySet/FdTable`，普通 fork 通过 COW 派生；
- exec：准备新映像 → 静止 sibling → 替换地址空间与 trap context → 处理 CLOEXEC/信号；
- exit/wait：逻辑退出 → 保留 zombie → `wait4` copyout 成功后提交回收；
- TCB 与当前内核栈在 idle 栈上延迟析构。

建议图示：三条平行的短时序线，共同标出“prepare”和“commit”两种颜色。

讲解重点：重点讲多线程 exec 不能只替换当前线程地址空间，以及 wait4 为什么必须先 copyout 再删除 child。

建议时间：70 秒。

### 第 7 页　多核切换：上下文保存完成后，任务才能重新发布

**核心结论：** `handoff + cpu_owner` 关闭了“任务尚未保存就被另一 CPU 恢复”的竞态窗口。

页面内容：

```text
CPU A：Running → 写入 per-CPU handoff → __switch 保存 context → idle
                                                              ↓
idle：清除 cpu_owner → 根据最终状态发布 Ready              全局队列
                                                              ↓
CPU B：fetch_for_cpu → claim cpu_owner → 恢复任务
```

- ready/blocked/index 保持唯一归属；
- affinity 同时约束取任务和定向 IPI；
- 退出任务在安全栈上进入延迟回收。

建议图示：双 CPU 泳道图，突出“保存完成”这条栅栏。

建议时间：65 秒。

### 第 8 页　内存对象模型：VMA 是承诺，PTE 是现状，Frame 是所有权

**核心结论：** `MemorySet` 同时管理虚拟范围语义、硬件映射和驻留页所有权。

页面内容：

- VMA：范围、权限、shared/private、file backing；
- PTE：CPU 当前可见的 VPN → PPN 与权限；
- `FrameTracker`：4 KiB 页帧，`Arc` 表达共享，Drop 自动回收；
- lazy VMA 可以暂时没有 PTE 和 frame。

建议图示：三层堆叠图“VMA → PTE → Frame”，旁边标出 RV64/LA64 共用的 39-bit 高半区模型。

讲解重点：纠正“内存管理就是分配器”的印象；物理帧使用栈式 frame allocator，buddy 只负责内核堆。

建议时间：55 秒。

### 第 9 页　一次缺页统一完成 lazy、文件页与 COW

**核心结论：** 不同映射类型共享一个缺页入口，差异由 VMA 权限和 backing 决定。

建议图示：缺页决策树。

```text
page fault
  ├─ 无合法 VMA / 权限不符 → EFAULT / signal
  ├─ PTE 缺失 + anonymous → 清零页
  ├─ PTE 缺失 + file backing → 按页读取
  ├─ COW 写 + 唯一 owner → 恢复 W
  └─ COW 写 + 多 owner → 分配、复制、替换
```

页面补充：用户 copyin/copyout 复用同一套补页和权限检查，支持跨页、lazy 与 COW 缓冲区。

讲解重点：COW 失败不能在父地址空间留下空洞；尚未驻留的页在 fork 时只复制 VMA 元数据。

建议时间：70 秒。

### 第 10 页　共享 mmap：锁内快照，锁外写回

**核心结论：** MM 不持地址空间锁进入文件系统，从结构上避免 MM—FS 锁序环。

页面内容：

```text
MemorySet 锁内：定位 writable shared VMA → 快照 resident frame
                                           ↓ 解锁
文件系统：按 offset 写入 page cache → MS_SYNC 时 fsync
                                           ↓
内存系统：提交 unmap / replace / shrink
```

- `(dev, ino, page_index)` 标识共享文件页；
- 对可返回错误的 syscall，写回成功后才改变映射；
- 当前 `MS_INVALIDATE` 与 truncate 后完整 `SIGBUS` 仍是边界。

建议图示：MM 与 FS 两个泳道，I/O 全部位于 MM 锁外。

建议时间：60 秒。

### 第 11 页　从 45 MiB cargo 到按需 ELF 装载

**核心结论：** 主程序只读有界 ELF 元数据，`PT_LOAD` 页面首次访问时才进入物理内存。

页面内容：

- 旧路径：整文件进入固定内核堆，再建立用户页；
- 新路径：header/program header/解释器元数据 ≤ 1 MiB；
- 主 ELF 使用 private file-backed VMA，BSS 依靠新页清零；
- 45,559,552 字节 cargo 已在 RV64 `-smp 8 -m 8G` 运行到 `BUILDSTORM_TOOLCHAIN ok`。

建议图示：左右对比内存占用，不展示大段 loader 代码；下方放一行真实日志截图。

讲解边界：该证据不代表 BuildStorm minibuild 或最终编译已经通过；动态链接器仍使用整文件读取。

建议时间：60 秒。

### 第 12 页　多核页表：只向真正使用地址空间的 hart 发 RFENCE

**核心结论：** `active_hart_mask` 将 PTE 修改、远端 TLB 失效和旧页回收排成安全顺序。

页面内容：

```text
修改 PTE → 本地 sfence.vma → 读取 active_hart_mask
         → SBI remote SFENCE → 确认完成 → 回收旧 frame
```

- 调度进入用户页表前发布 hart bit；切回 idle/kernel 页表后清除；
- 避免向所有 CPU 无差别广播；
- 证据限于当前 RV64 QEMU/OpenSBI，LA64 SMP 不沿用该结论。

建议图示：一张 4 核示意图，只高亮当前加载该 `MemorySet` 的两个核。

建议时间：55 秒。

### 第 13 页　VFS：从 fd 到 ext4 的五层对象链

**核心结论：** descriptor、打开实例、路径、目录项和 inode 分层后，不同文件后端可以共用 syscall。

建议图示：全页纵向对象链。

```text
fd → FdEntry → FileOp → Path{mount,dentry} → InodeOp → SuperBlockOp
```

页面内容：

- `FdEntry` 保存 per-fd 的 CLOEXEC；
- `FileOp` 保存 open-file description 状态，如 offset 和 status flags；
- `InodeOp` 表达文件身份与按偏移操作；
- ext4、procfs、devfs、pipe 和匿名文件按能力接入相应层。

讲解重点：fork 复制 fd table，但 `Arc<FileOp>` 仍共享，所以父子共享文件偏移；`CLONE_FILES` 才共享整张表。

建议时间：60 秒。

### 第 14 页　路径与挂载：同一棵命名空间中的跨文件系统行走

**核心结论：** `Nameidata` 同时携带 mount 与 dentry，才能正确处理挂载点、`..` 和符号链接。

页面内容：

- dentry 命中：HashMap 查找 + mount 检查；
- miss：调用 `inode.lookup` 并缓存；
- symlink：迭代重启，最多跟随 40 次；
- mount root 上的 `..` 退回父 mount 的挂载点；
- parent 使用强引用，children 使用弱引用以打破循环。

建议图示：`/` 下挂载 `/proc` 的小树，画出进入挂载和 `..` 返回两条箭头。

讲解边界：当前命名空间修改由全局 `NAMEI_MUTATION_LOCK` 串行化，正确性清晰，但并发创建/删除会受限。

建议时间：55 秒。

### 第 15 页　ext4 适配：在 path-based 后端上补齐 Unix 文件语义

**核心结论：** VFS 不只是把 trait 转发给 lwext4，还补偿了后端缺失的对象生命周期语义。

页面内容：

- `Ext4Inode` 将 `InodeOp` 翻译为 lwext4 C FFI；
- 后端按完整路径定位，而不是按 inode 号；
- unlink 但 fd 仍打开时：rename 到隐藏孤儿路径；
- `nlink_override` 对用户呈现链接数为 0；
- 最后一个 open file 释放时真正删除孤儿文件。

建议图示：`name → inode/file → hidden orphan → final close → delete` 生命周期图。

讲解重点：用“已 unlink 文件仍可通过旧 fd 读写”这个用户可见语义解释设计，不讲 FFI 适配流水账。

建议时间：60 秒。

### 第 16 页　页缓存：按需驻留、版本化写回、代际回收

**核心结论：** 稀疏缓存降低大文件占用，版本号避免并发写回错误清除新脏数据。

页面内容：

- per-inode `BTreeMap<page_index, Page>`，只缓存访问过的页；
- write 标记 dirty 并增加 `write_version`；
- sync 快照版本、批量写回，仅清除版本未变化的 dirty；
- generational LRU 只回收干净且无外部引用的旧页；
- `fsync` 完成 page cache → lwext4 → block device 的三级持久化链。

建议图示：左侧页面状态机 `Absent → Clean → Dirty → Clean/Evict`，右侧三层写回箭头。

讲解重点：区分“写入页缓存”和“数据已落盘”，避免将普通 write 说成持久化完成。

建议时间：70 秒。

### 第 17 页　IPC 的共同问题：等待者由谁完成

**核心结论：** 信号、管道和 futex 的核心不是传输格式，而是阻塞登记与竞争完成。

页面内容：

| 机制 | 对象身份 | 阻塞条件 | 完成者 |
| --- | --- | --- | --- |
| signal | tid/tgid + pending | signal wait 或打断其他等待 | signal delivery |
| pipe | 共享 ring buffer | 读空或写满 | peer / close / signal |
| futex | backing-aware key | 用户值仍等于期望值 | wake / timeout / signal / exit |
| shm | segment + attach | 映射本身不阻塞 | detach / RMID |

建议图示：四种机制汇入 scheduler 的 `Blocked → Ready` 状态边。

建议时间：45 秒。

### 第 18 页　信号闭环：从产生到 sigreturn

**核心结论：** 信号只在返回用户态前提交，确保能安全构造和恢复完整用户现场。

页面内容：

```text
kill/timer/fault/SIGPIPE/SIGCHLD
  → 选择目标 tid → pending + SigInfo
  → 打断可中断等待，或等待下一次 trap return
  → 默认动作 / 构造 SigFrame(SigRTFrame)
  → handler → trampoline → sigreturn 恢复寄存器与 mask
```

- pending/mask/alt stack 每线程独立；handler 表线程组共享；
- 支持 `SA_SIGINFO/ONSTACK/NODEFER/RESETHAND`；
- Core 类暂不生成 core 文件，`SA_RESTART` 尚未自动重启 syscall。

建议图示：完整闭环时序图，突出 trap return 和 sigreturn 两个边界。

建议时间：65 秒。

### 第 19 页　pipe 与 futex：关闭 lost-wakeup 窗口

**核心结论：** “锁内登记 + 发布后复查”避免丢失唤醒，“single-winner”避免重复完成。

页面左半：pipe。

```text
reader 持 buffer 锁：检查为空 → 登记 tid → 标记 Blocked → 解锁
writer 持同一锁：写入 → 取出 waiter → wakeup
reader 切换前：复查是否已经 Ready
```

页面右半：futex。

```text
Pending ──wake────> Woken
        ├─timeout─> TimedOut
        ├─signal──> Interrupted
        └─exit────> cleanup
```

讲解重点：所有事件争夺同一个完成状态；只有赢家删除 waiter 并发布任务，后到路径不重复入队。

建议时间：75 秒。

### 第 20 页　System V 共享内存：segment 所有权与 attach 事务

**核心结论：** 共享 frame 属于 segment，每个地址空间只拥有一条可回滚的 attach 关系。

页面内容：

- `shmget` 创建 segment 和共享 frames；
- `shmat` 预留 attach id，再向 `MemorySet` 建图，失败回滚 owner；
- `IPC_RMID` 有 attach 时仅标记删除；
- 最后一次 `shmdt` 后释放 segment 和 frame。

建议图示：`segment → attach A / attach B → MemorySet A / B`，下方画出延迟删除时序。

讲解边界：当前不支持 System V 消息队列和 semaphore；跨进程独立 shmat 的 futex key 身份仍需进一步统一。

建议时间：50 秒。

### 第 21 页　一条真实跨模块链：shell 启动并等待子程序

**核心结论：** 用户看到的一次命令执行，会穿过本次答辩的全部四个模块。

建议图示：全页时序图。

```text
shell
  → pipe + fork(COW、共享 FileOp)
  → child exec(file-backed ELF、CLOEXEC、signal reset)
  → page fault(代码页按需读取)
  → read/write(page cache / pipe)
  → futex 或 signal/timeout 参与等待
  → exit_group(SIGCHLD、关闭 fd、共享 mmap 写回)
  → parent wait4(copyout 后回收 zombie)
  → idle 栈延迟释放 TCB 与内核栈
```

讲解重点：这一页不是复述流程，而是证明四个模块共享所有权和提交协议，形成真实应用闭环。

建议时间：70 秒。

### 第 22 页　验证证据：结论与适用范围一起展示

**核心结论：** RespOS 用专项并发测试和真实负载验证设计，同时保留未完成边界。

建议内容：三张证据卡片。

1. **共享 mmap：** 2026-08-02；RV64 musl/glibc 各 20 passed、2 个已知边界失败；LA64 各 15 passed、0 failed。
2. **RV64 共享地址空间：** 2026-08-06；2/8 核专项压力通过，8 核 1000 轮 remap 竞争共 10/10 通过。
3. **大 ELF：** 2026-08-06；RV64 8 核、8 GiB，45.6 MiB cargo 运行到 `BUILDSTORM_TOOLCHAIN ok`。

页面底部用较小但清晰的文字标注：完整 LTP、BuildStorm minibuild/final compile、LA64 SMP 不由上述结果代替。

建议图示：只使用真实日志中最短、最有辨识度的三行；不要放整屏终端截图。

建议时间：60 秒。

### 第 23 页　总结：RespOS 建立了四个可组合的内核协议

页面内容：

- 进程：统一 TCB、资源共享边界、多核安全切换和分阶段回收；
- 内存：VMA/PTE/Frame 所有权、lazy/COW/file-backed 和定向 TLB shootdown；
- 文件：分层 VFS、path-based ext4 语义补偿、稀疏页缓存与持久化链；
- IPC/信号：完整信号返回链、无丢失管道等待、futex single-winner 和共享内存事务。

中间大字总结：

> 让复杂 Linux 语义在失败、并发和多核条件下仍有唯一的状态提交点。

建议图示：回到第 2 页的对象闭环图，但把已讲解的四条链全部点亮，形成首尾呼应。

建议时间：40 秒。

### 第 24 页　结束页

**标题：** 谢谢各位专家，请批评指正

页面内容：仓库二维码、项目名称、队伍联系方式。不要在此页继续塞功能总结。

建议时间：10 秒。

## 三、15 分钟裁剪方案

若现场主讲严格限制为 15 分钟，建议压缩为 19 页：

- 第 3 页“三条原则”并入第 2 页；
- 第 6、7 页合并为“任务生命周期与多核切换”；
- 第 11、12 页合并为“大 ELF 与 SMP 地址空间”；
- 第 14、15 页合并为“路径解析与 ext4 语义补偿”；
- 第 17 页并入第 19 页；
- 第 20 页 System V shm 移入答疑附录；
- 保留第 21 页跨模块链和第 22 页证据页，不应因时间紧张删除。

## 四、建议答疑附录

### 附录 A1　clone flag 与资源共享矩阵

列出 `CLONE_VM`、`CLONE_FILES`、`CLONE_SIGHAND`、`CLONE_THREAD`、`CLONE_VFORK` 的实际语义和当前 vfork 边界。

### 附录 A2　五类 VMA 的驻留与 fork 行为

对比私有匿名、私有文件、共享匿名、共享文件和 ELF `PT_LOAD`，回答“哪些 lazy、哪些 COW、哪些写回”。

### 附录 A3　VFS 对象所有权与 fd 复制语义

画出 `FdTable → FdEntry → Arc<FileOp> → Path/Dentry/Inode`，回答 dup、fork、`CLONE_FILES` 和 CLOEXEC 的差别。

### 附录 A4　页缓存并发写回细节

展示 `write_version`、`size_version`、dirty 计数和 generational LRU，回答“写回期间再次写入为什么不会丢 dirty”。

### 附录 A5　信号 flag 与默认动作支持范围

列出 `SA_SIGINFO/ONSTACK/NODEFER/RESETHAND`、Stop/Continue/Term/Core，以及 `SA_RESTART` 和实时信号排队边界。

### 附录 A6　当前限制与后续演进

- 全局 run queue 与全局 namei mutation lock 的可扩展性；
- 动态链接器 file-backed 改造；
- dirty bit、truncate-`SIGBUS`、`MS_INVALIDATE`；
- LA64 SMP；
- System V 消息队列、semaphore 与稳定跨进程 shm futex key。

## 五、视觉与讲解规范

- 四个模块固定配色：进程蓝、内存紫、文件橙、IPC/信号绿；跨模块页同时使用四色。
- 主标题建议 30～34 pt，正文不低于 20 pt；一页正文尽量不超过 6 行。
- 源码只用于答疑页；主讲页把源码翻译成对象图、状态机或时序图。
- 流程图中的动词统一使用“准备、登记、发布、提交、回滚、释放”。
- 结果页使用“能力—证据—边界”三段式，不把历史成绩当作当前结果。
- 演讲时每页先说结论，再解释设计原因，最后指出用户可见效果或验证证据。

