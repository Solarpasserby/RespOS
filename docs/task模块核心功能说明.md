# Task 模块核心功能说明

## task 子模块文件职责总览

```text
os/src/task
├── mod.rs         组织各子模块，导出调度接口，实现 suspend/exit 两个任务切换方法
├── task.rs        定义 TaskControlBlock 和 TaskControlBlockInner，管理任务资源、父子关系、fork/exec 辅助逻辑
├── manager.rs     管理就绪队列 TASK_MANAGER，负责把任务加入队列和从队列取出任务
├── processor.rs   管理 PROCESSOR，记录当前 CPU 正在运行的任务和空闲任务，负责 run_tasks/schedule
├── context.rs     定义 TaskContext，即 __switch 保存和恢复的任务上下文
├── switch.rs      引入 switch.S 中的 __switch，并声明任务上下文切换接口
├── switch.S       真正执行寄存器保存、寄存器恢复、satp 切换和 ret 返回
├── kstack.rs      管理每个任务的内核栈，包括地址计算、映射插入和生命周期回收
└── pid.rs         分配和回收任务 pid，PidHandle 释放时自动归还 pid
```

简单来说，`manager.rs` 决定“下一个可运行任务是谁”，`processor.rs` 记录“当前 CPU 正在运行谁”，`task.rs` 保存“任务自身拥有哪些资源和状态”，`switch.S` 则完成最终的硬件上下文切换。

## 第一部分：任务运行的整体调用链

一次普通的任务切换可以理解为：**当前任务让出 CPU，空闲任务接管调度，再从队列里选出下一个任务运行**。

```text
timer trap
    -> trap_handler
    -> suspend_current_and_run_next
    -> schedule
    -> __switch(当前任务, 空闲任务)
    -> run_tasks
    -> fetch_task
    -> __switch(空闲任务, 下一个任务)
```

关键函数：

- [`trap_handler`](../os/src/trap/mod.rs#L50)：处理异常和中断；计时器中断会进入 [`SupervisorTimer` 分支](../os/src/trap/mod.rs#L85)。
- [`suspend_current_and_run_next`](../os/src/task/mod.rs#L71)：暂停当前任务，改为 `Ready`，重新放回就绪队列，然后切回空闲任务。
- [`schedule`](../os/src/task/processor.rs#L112)：调用 [`__switch`](../os/src/task/switch.S#L12)，保存当前任务上下文，恢复空闲任务上下文。
- [`run_tasks`](../os/src/task/processor.rs#L84)：空闲任务中的调度循环，不断从 [`TASK_MANAGER`](../os/src/task/manager.rs#L11) 取任务运行。

这部分最重要的理解是：普通任务不直接选择下一个任务，它只负责切回空闲任务；真正从队列取任务的是 `run_tasks`。

稍微展开看，`suspend_current_and_run_next` 主要做三件事：

1. 通过 [`take_current_task`](../os/src/task/processor.rs#L56) 从 `PROCESSOR.current` 取出当前任务；
2. 把任务状态从 `Running` 改为 `Ready`，再调用 `add_task` 放回就绪队列；
3. 取出当前任务的 `task_context` 指针，调用 `schedule` 切换到空闲任务。

## 第二部分：任务模块的内存布局与内核栈切换

每个任务都有自己的内核栈。用户程序平时在用户栈上运行，进入内核后使用对应任务的内核栈。

### 1. 内核栈地址规则

内核栈常量在 [`os/src/config/mm.rs`](../os/src/config/mm.rs#L14)：

- `KERNEL_STACK_TOP = 0xffff_ffff_ffff_f000`
- `PAGE_SIZE = 0x1000`
- `KERNEL_STACK_SIZE = 15 * PAGE_SIZE`

每个任务的栈顶由 [`get_kernel_stack_top`](../os/src/task/kstack.rs#L49) 计算：

```text
kernel_stack_top(pid) = KERNEL_STACK_TOP - pid * (KERNEL_STACK_SIZE + PAGE_SIZE)
```

因此内核栈从高地址向低地址排列，每个任务栈之间留出一个未映射的 guard page。如果内核栈溢出，就会访问到这片未映射区域并触发错误，而不是直接覆盖相邻任务的内核栈。

### 2. 内核栈上放了什么

新任务创建时，[`TaskControlBlock::new`](../os/src/task/task.rs#L34) 会：

1. 分配 pid 和 [`KernelStack`](../os/src/task/task.rs#L37)；
2. 创建用户地址空间；
3. 在内核栈顶部写入 [`TrapContext`](../os/src/task/task.rs#L41)；
4. 初始化 [`TaskContext`](../os/src/task/task.rs#L51)，其中 `sp` 指向内核栈上的 `TrapContext`，`ra` 指向 `__restore`。

这样任务第一次被调度时，`__switch` 恢复 `TaskContext` 后会跳到 `__restore`，再从 `TrapContext` 恢复用户态上下文。

这里容易混淆的是 `TaskContext` 和 `TrapContext`：前者服务于内核中的任务切换，保存 `__switch` 需要的寄存器；后者服务于从内核返回用户态，保存用户程序的寄存器、`sstatus` 和 `sepc`。一个任务从“被调度运行”到“回到用户态”，实际会先恢复 `TaskContext`，再由 `__restore` 使用内核栈上的 `TrapContext`。

### 3. __switch 的“无感”切换

[`__switch`](../os/src/task/switch.S#L12) 保存当前任务的 `ra`、`sp`、`s0-s11`、`satp`，再恢复下一个任务的同类寄存器。

关键是：

- `ra` 决定 `ret` 后回到哪里；
- `sp` 决定继续使用哪一个内核栈；
- `satp` 决定切换到哪个任务的地址空间。

所以任务调用 `__switch` 后会像“暂停”一样离开 CPU；之后再次被调度时，`__switch` 返回，任务从原来的位置继续执行。

换句话说，`__switch` 对当前任务来说像一个普通函数调用，只是这个函数中途把 CPU 交给了别的任务。当前任务被换出时，自己的返回地址和内核栈指针已经保存到 `TaskContext`；之后再次换入时，`ret` 会回到当初调用 `__switch` 之后的位置。这就是任务切换看起来“无感”的原因。

### 4. 为什么切换页表后还能访问内核栈

用户地址空间由 [`MemorySet::from_kernel_page_table`](../os/src/mm/memory_set.rs#L123) 创建，会复制内核高地址映射。任务创建时先把内核栈插入 `KERNEL_SPACE`，再创建用户地址空间，因此任务页表里也能访问自己的内核栈。

这个顺序很重要：如果先创建用户地址空间，再映射任务内核栈，那么该任务页表里可能没有对应的内核栈映射。后续 `__switch` 恢复该任务的 `satp` 后，内核还要继续使用这个任务的内核栈执行；如果页表中缺少映射，就无法正常访问。因此代码在创建任务时先创建 `KernelStack`，再创建 `MemorySet`。

> 为什么要给每个任务设计独立内核栈
> 
> 多个内核栈的核心想法是：**每个任务进入内核后，都有一块属于自己的内核执行现场**。如果任务在内核中途被切走，它的内核栈内容不会被其他任务覆盖；之后切回来时，只要恢复 `sp`，就能沿着原来的内核调用链继续执行。
> 
> 这和抢占式调度有关，但当前内核没有完整实现，它的优点主要有：
> 
> - 隔离性更好：不同任务的内核调用链、局部变量和异常上下文不会混在同一个栈里。
> - 切换更自然：`__switch` 只需要保存/恢复 `sp`，就能回到对应任务自己的内核执行现场。
> - 支持中途暂停：任务可以在内核执行过程中被切走，之后再从原来的内核栈继续。
> - 更容易暴露栈错误：配合 guard page，内核栈溢出时更容易触发错误，而不是直接覆盖其他任务的栈。

## 第三部分：任务控制块与系统调用支持

[`TaskControlBlock`](../os/src/task/task.rs#L22) 是任务的核心数据结构，外层包含：

- `pid`：任务 ID；
- `kernel_stack`：任务内核栈；
- `inner`：可变任务数据，使用 `Mutex<TaskControlBlockInner>` 保护。

[`TaskControlBlockInner`](../os/src/task/task.rs#L178) 主要包含：

- `task_status`：任务状态，`Ready` / `Running` / `Exited`；
- `task_context`：任务切换上下文；
- `memory_set`：任务地址空间；
- `fd_table`：文件描述符表；
- `cwd`：当前工作目录；
- `parent` / `children`：父子任务关系；
- `base_size`：用户内存边界相关信息；
- `exit_code`：任务退出码。

为了支持任务相关系统调用，`task.rs` 额外实现了这些接口：

- [`fork`](../os/src/task/task.rs#L67)：复制当前任务，创建子任务；
- [`exec`](../os/src/task/task.rs#L102)：替换当前任务的地址空间和用户态上下文；
- [`get_trap_cx`](../os/src/task/task.rs#L121)：获取内核栈上的异常上下文，供 `fork` 修改子任务返回值；
- [`pid`](../os/src/task/task.rs#L140)：返回任务 pid；
- [`inner_exclusive_access`](../os/src/task/task.rs#L117)：访问任务内部可变状态，供 `waitpid` 查询子任务和退出码；
- [`alloc_fd`](../os/src/task/task.rs#L145)、[`set_fd`](../os/src/task/task.rs#L148)、[`get_fd_entry`](../os/src/task/task.rs#L151)：为文件相关系统调用访问 fd 表；
- [`cwd`](../os/src/task/task.rs#L156)、[`set_cwd`](../os/src/task/task.rs#L159)：为路径相关系统调用维护当前目录。

> 参考 Unix/Linux 的使用方式，可以把这组系统调用理解成“父任务启动一个新程序”的流程：比如 shell 想运行 `hello_world`，它先 `fork` 出一个子任务；父任务继续保留 shell 的执行现场，子任务则在 `fork` 返回后调用 `exec`，把自己的地址空间替换成 `hello_world`。程序运行结束时调用 `exit` 保存退出码，父任务再通过 `waitpid` 取得这个退出码并回收子任务。
>
> 这里的重点是：`fork` 只负责复制当前执行现场，`exec` 才负责装载新程序；`exec` 并不是创建新任务，而是让当前任务执行另一个程序。Linux `execve(2)` 手册也强调，`execve` 会让当前进程执行新程序，PID 等很多属性保持不变。`exit` / `waitpid` 则对应程序结束后的父子同步：子任务退出后留下退出状态，父任务通过 wait 系列调用取得这个状态。
>
> 参考资料：[`execve(2)`](https://man7.org/linux/man-pages/man2/execve.2.html)、[`_exit(2)`](https://www.man7.org/linux/man-pages/man2/exit.2.html)、[`wait(2)` / `waitpid(2)`](https://man7.org/linux/man-pages/man2/waitpid.2.html)。
