# RespOS 一周内核语义训练项目

## 1. 训练安排概述

这一周让两名队员各自独立完成两个 Rust 用户态小项目：

1. **进程退出、Zombie 与 wait 模型**；
2. **fd、dup、fork 与共享文件偏移模型**。

项目允许使用 Rust 标准库，不要求 `no_std`，也不要求模拟真实硬件。重点是理解内核对象、状态变化、共享关系和资源生命周期，并把这些认识映射回 RespOS。

建议每个队员建立两个独立 Cargo 工程：

```text
respos-training/
├── process-wait-model/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       └── main.rs        # 可选，仅用于演示状态输出
└── fd-sharing-model/
    ├── Cargo.toml
    └── src/
        ├── lib.rs
        └── main.rs        # 可选，仅用于演示对象关系
```

两人使用同一份需求，但分别设计和实现。可以讨论内核概念和 Rust 编译错误，不要直接复制对方的结构体和函数实现。完成核心功能后再交换 review。

不要求精确控制代码行数。一般每个项目 300～600 行已经足够；如果明显超过 800 行，通常说明加入了过多非核心功能。

## 2. 项目 A1：进程退出、Zombie 与 wait

### 2.1 要训练的能力

这个项目训练：

- 用状态机描述进程生命周期；
- 同时维护父进程和子进程两侧的对象关系；
- 区分“进程停止执行”“运行资源释放”“父进程回收 Zombie 记录”；
- 区分正常结果、暂时不可用和真正的错误；
- 为生命周期代码编写能够复现错误的测试；
- 根据状态快照定位第一次错误变化。

### 2.2 需要查阅的内核知识

队员不需要先系统学习整本操作系统教材。开始项目前，查清下面几个概念即可。

#### 进程退出与 Zombie

进程调用 exit 后不会继续执行，但父进程仍需要获得它的 pid 和退出状态。因此内核通常先释放大部分运行资源，再保留一份较小的 Zombie 记录。父进程调用 wait 后，这份记录才被最终回收。

建议查阅：

- `man 2 exit`；
- `man 2 waitpid`；
- `man 2 waitid`；
- 搜索关键词：`Linux zombie process reaping`、`process exit wait lifecycle`。

需要回答：

1. exit 和 wait 分别由谁调用？
2. 为什么 exit 后不能立即丢掉 pid 和 exit code？
3. 为什么父进程不 wait 会产生 Zombie？

#### 父子关系和孤儿进程托管

父进程可能先于子进程退出。内核需要为仍然存在的 child 找到新的 parent，使其将来仍能被回收。RespOS 当前把这类 child 转交给 init 进程。

建议查阅：

- `man 2 getppid`；
- 搜索关键词：`orphan process reparent init`；
- RespOS 的 `reparent_children_to_init`。

需要回答：修改 parent 时，为什么还要同时修改新旧 parent 的 children 集合？

#### wait 的三种不同结果

下面三种情况不能混在一起：

1. 当前没有符合条件的 child：类似 `ECHILD`；
2. 有 child，但它还没有退出，且使用非阻塞 wait：类似返回 0；
3. 有 child，但它还没有退出，且使用阻塞 wait：真实内核会让当前任务睡眠。

这个用户态模型不做真实调度，因此第三种情况只返回 `WouldBlock`，表示真实内核下一步应该阻塞。

#### 内核对象生命周期

本项目把资源分成两类：

- **运行资源**：模拟地址空间、fd 表等，在 exit 阶段释放；
- **Zombie 记录**：包含 pid、parent、exit code，在 wait 阶段删除。

这是一种简化模型，但比“所有资源都等到 wait 才释放”更接近 RespOS。

### 2.3 对应 RespOS 代码

只要求阅读下列位置：

| 知识 | RespOS 位置 | 阅读重点 |
| --- | --- | --- |
| 进程对象 | `os/src/task/task.rs::TaskControlBlock` | `parent`、`children`、`exited_children`、`task_status`、`exit_code` |
| 创建 child | `TaskControlBlock::clone_` | 普通进程何时加入 parent.children；线程为何不加入 |
| 进程退出 | `task_group_exit` | 哪些资源在 exit 清理，哪些状态保留 |
| 通知父进程 | `notify_parent_exit` | exited_children、SIGCHLD、wakeup 的关系 |
| 孤儿托管 | `reparent_children_to_init` | 如何同时更新 child 和 init |
| 等待回收 | `os/src/syscall/process.rs::sys_wait4` | child 选择、WNOHANG、ECHILD、status copyout、remove 顺序 |

当前 RespOS 的线程组退出语义并不等同于完整 Linux。这个项目只训练普通父子进程，不要求模拟线程组。

### 2.4 项目范围

必须实现：

- 初始化一个 init 进程；
- 从指定 parent 创建 child；
- 进程正常 exit 并保存退出码；
- exit 时释放一次模拟运行资源，但保留 Zombie 记录；
- 按指定 pid 等待；
- 等待任意一个已退出 child；
- 非阻塞 wait；
- 成功 wait 后删除 Zombie 记录；
- 父进程退出后，把它的 children 转交 init；
- 状态快照和基本不变量检查。

不实现：

- 线程、线程组和 clone flags；
- 信号、stop/continue 和 core dump；
- 真实阻塞、唤醒和调度；
- 地址空间和真实 fd；
- pgid、rusage 和完整 Linux wait status 编码；
- 多核和锁。

### 2.5 行为约定

#### 状态

```text
Running
   │ exit(code)
   ▼
Zombie(code)
   │ parent wait 成功
   ▼
Reaped（从进程表删除）
```

exit 阶段：

- `Running → Zombie(code)`；
- 释放模拟运行资源；
- 保留 pid、parent、children 和 exit code；
- 把自己的 children 转交 init；
- 不能直接把自己从 parent.children 删除。

wait 阶段：

- 只有 parent 可以回收自己的 child；
- 只有 Zombie child 可以成功回收；
- 回收后从 parent.children 和全局进程表删除；
- 同一 pid 不能成功 wait 两次。

#### 建议返回结果

```rust
pub enum WaitOutcome {
    Reaped { pid: Pid, exit_code: i32 },
    NotReady,   // 有匹配 child，但 nohang=true 且尚未退出
    WouldBlock, // 有匹配 child，但阻塞式 wait 目前需要睡眠
}
```

完全没有匹配 child 时建议返回 `ModelError::NoChild`，不要也返回 `NotReady`。

### 2.6 示例代码框架

下面只提供对象边界和函数入口。核心状态修改留给队员完成。

```rust
use std::collections::{BTreeMap, BTreeSet};

pub type Pid = u32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessState {
    Running,
    Zombie { exit_code: i32 },
}

#[derive(Debug, Clone)]
pub struct RuntimeResource {
    pub id: u32,
    pub released: bool,
    pub release_count: u32,
}

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: Pid,
    pub parent: Option<Pid>,
    pub children: BTreeSet<Pid>,
    pub state: ProcessState,
    pub resource: RuntimeResource,
}

#[derive(Debug, Clone, Copy)]
pub enum WaitSelector {
    Any,
    Pid(Pid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitOutcome {
    Reaped { pid: Pid, exit_code: i32 },
    NotReady,
    WouldBlock,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    NoSuchProcess,
    NoChild,
    AlreadyExited,
    CannotExitInit,
    BrokenInvariant(String),
}

pub struct KernelModel {
    next_pid: Pid,
    init_pid: Pid,
    processes: BTreeMap<Pid, Process>,
}

impl KernelModel {
    pub fn new() -> Self {
        todo!("创建 init 和初始资源")
    }

    pub fn init_pid(&self) -> Pid {
        self.init_pid
    }

    pub fn spawn(&mut self, parent: Pid) -> Result<Pid, ModelError> {
        todo!("创建 child，并维护父子双方关系")
    }

    pub fn exit(&mut self, pid: Pid, code: i32) -> Result<(), ModelError> {
        todo!("释放运行资源、进入 Zombie、托管 children")
    }

    pub fn wait(
        &mut self,
        parent: Pid,
        selector: WaitSelector,
        nohang: bool,
    ) -> Result<WaitOutcome, ModelError> {
        todo!("选择 child，区分 NoChild/NotReady/WouldBlock/Reaped")
    }

    pub fn process(&self, pid: Pid) -> Option<&Process> {
        self.processes.get(&pid)
    }

    pub fn check_invariants(&self) -> Result<(), ModelError> {
        todo!("检查 pid、父子双向关系、状态和资源释放次数")
    }

    pub fn snapshot(&self) -> String {
        todo!("稳定输出 pid、parent、children、state、resource")
    }
}
```

如果借用检查导致一个函数同时修改 parent 和 child 很困难，允许先收集 pid 和需要进行的变化，再分成几个短的可变借用阶段。不要通过 clone 整个 Process 后修改副本来绕开问题。

### 2.7 示例事件和预期状态

```text
init=1
spawn(1) -> 2
spawn(2) -> 3

exit(2, 7)
  pid 2: Zombie(7), runtime resource released
  pid 3: parent 从 2 改为 1
  init.children: [2, 3]

wait(1, pid=2, nohang=false)
  -> Reaped(pid=2, exit_code=7)
  pid 2 从进程表删除
  pid 3 仍由 init 管理
```

### 2.8 示例测试框架

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_then_wait_reaps_once() {
        let mut kernel = KernelModel::new();
        let init = kernel.init_pid();
        let child = kernel.spawn(init).unwrap();

        kernel.exit(child, 7).unwrap();

        // exit 后仍能找到 Zombie 记录，但运行资源已经释放。
        let child_state = kernel.process(child).unwrap();
        assert_eq!(child_state.state, ProcessState::Zombie { exit_code: 7 });
        assert!(child_state.resource.released);

        let result = kernel
            .wait(init, WaitSelector::Pid(child), false)
            .unwrap();
        assert_eq!(
            result,
            WaitOutcome::Reaped {
                pid: child,
                exit_code: 7,
            }
        );
        assert!(kernel.process(child).is_none());
        assert_eq!(
            kernel.wait(init, WaitSelector::Pid(child), false),
            Err(ModelError::NoChild)
        );
    }

    #[test]
    fn running_child_distinguishes_nohang_and_blocking_wait() {
        let mut kernel = KernelModel::new();
        let init = kernel.init_pid();
        let child = kernel.spawn(init).unwrap();

        assert_eq!(
            kernel.wait(init, WaitSelector::Pid(child), true).unwrap(),
            WaitOutcome::NotReady
        );
        assert_eq!(
            kernel.wait(init, WaitSelector::Pid(child), false).unwrap(),
            WaitOutcome::WouldBlock
        );
        kernel.check_invariants().unwrap();
    }
}
```

这些测试只展示需求，不展示 `spawn/exit/wait` 的实现。

### 2.9 必做测试清单

- child exit 后 parent 得到正确 pid 和 exit code；
- child 未退出时，nohang 返回 NotReady；
- child 未退出时，阻塞式 wait 返回 WouldBlock，状态不变；
- wait 非 child pid 返回 NoChild；
- 成功 wait 后重复 wait 返回 NoChild；
- 两个 child 按不同顺序退出，`WaitSelector::Any` 能分别回收；
- 父进程先退出，Running child 转交 init；
- 父进程先退出，已有 Zombie child 也转交 init并可被回收；
- exit 只释放一次运行资源；
- 所有操作后 `check_invariants()` 通过。

### 2.10 项目完成后的 RespOS 阅读任务

每名队员用自己的模型回答：

1. `task_group_exit` 中哪些操作对应“释放运行资源”？
2. 为什么 leader 在 `set_exited` 后仍然能被 `sys_wait4` 找到？
3. `notify_parent_exit` 为什么既记录 exited child 又调用 wakeup？
4. `sys_wait4` 为什么要先完成 status copyout，再从 children 删除 child？
5. `reparent_children_to_init` 需要维护哪些双向关系？

不要求此时修改内核。能从模型走回真实代码，就是这个项目最重要的结果。

## 3. 项目 B1：fd、dup、fork 与共享文件偏移

### 3.1 要训练的能力

这个项目训练：

- 区分用户 fd、fd table entry、open file description 和 inode；
- 判断 clone 一个 Rust 对象时，究竟复制了值还是共享了底层状态；
- 正确放置共享状态和描述符私有状态；
- 使用 `Rc<RefCell<T>>` 表达单线程共享对象；
- 通过稳定 object id 和 offset 快照诊断共享错误；
- 理解 close、fork 和 exec 对 fd 生命周期的影响。

### 3.2 需要查阅的内核知识

#### fd 与 open file description

用户程序使用整数 fd。进程的 fd table 把这个整数映射到一个打开文件对象。打开文件对象保存当前 offset 和部分状态标志，再指向 inode/文件内容。

建议查阅：

- `man 2 open`，重点阅读 open file description；
- `man 2 read`；
- `man 2 lseek`；
- 搜索关键词：`file descriptor table open file description inode`。

需要回答：为什么对同一个路径调用两次 open，通常不会共享 offset？

#### dup 的共享语义

dup 创建新的 fd table entry，但两个 fd 指向同一个 open file description，所以共享 offset。

建议查阅：

- `man 2 dup`；
- `man 2 fcntl` 中 `F_DUPFD`、`F_GETFD`、`F_SETFD`、`F_GETFL`、`F_SETFL`；
- 搜索关键词：`dup shared file offset close-on-exec`。

需要回答：dup 后关闭原 fd，为什么新 fd 仍能使用？

#### fork 与 fd table

普通 fork 为 child 创建自己的 fd table，但其中的表项仍引用父进程原有的 open file descriptions。因此父子关闭 fd 的动作相互独立，读写 offset 却可能共享。

建议查阅：

- `man 2 fork` 的 open file description 部分；
- `man 2 close`。

需要回答：父进程 close(3) 后，为什么 child 的 fd 3 仍然有效？

#### descriptor flag 与 status flag

- `FD_CLOEXEC` 属于单个 fd table entry；
- `O_APPEND`、`O_NONBLOCK` 等 file status flags 通常属于共享 open file description；
- dup 默认清除新 fd 的 close-on-exec；
- exec 关闭带 CLOEXEC 的 fd，保留其他 fd。

建议查阅：

- `man 2 fcntl`；
- `man 2 execve`；
- 搜索关键词：`descriptor flags file status flags difference`。

RespOS 当前的 flags 存放和 `F_SETFL` 共享行为值得单独实测。本项目先按照上述对象分层建立正确模型，不要求顺手修改内核。

#### Rust 所有权知识

开始前至少知道：

- `Rc::clone` 只增加共享引用，不深拷贝内部对象；
- `RefCell` 把借用规则放到运行时检查；
- `Rc::strong_count` 可用于调试，但不应代替业务对象关系；
- `Drop` 在最后一个强引用消失时运行。

可查阅 Rust 标准库文档中的 `std::rc::Rc`、`std::cell::RefCell` 和 The Rust Book 的 smart pointers 章节。

### 3.3 对应 RespOS 代码

| 知识 | RespOS 位置 | 阅读重点 |
| --- | --- | --- |
| fd table | `os/src/fs/fdtable.rs::FdTable` | table、next_fd、alloc_fd、close |
| fd entry | `os/src/fs/fdtable.rs::FdEntry` | entry 自身是值，内部 file 是 `Arc<dyn FileOp>` |
| fork 复制 | `FdTable::from_existed_user` | clone table entry 后为何仍共享 File |
| 打开文件对象 | `os/src/fs/file.rs::{File,FileInner}` | offset 在 FileInner，而不是 fd 数字中 |
| dup | `os/src/syscall/fs.rs::{sys_dup,sys_dup3}` | 新表项、共享 file、CLOEXEC 处理 |
| fcntl | `os/src/syscall/fs.rs::sys_fcntl` | descriptor flag 和 status flag 的当前处理位置 |
| fork | `TaskControlBlock::clone_` | 普通 fork、线程、CLONE_FILES 的差别 |
| exec | `TaskControlBlock::execve`、`FdTable::close_on_exec` | 哪些 fd 被关闭 |

### 3.4 项目范围

必须实现：

- 内存中的 inode/文件内容；
- open 创建新的 OpenFile 和 fd；
- read 和 seek 更新 OpenFile.offset；
- close 删除一个 fd table entry；
- dup 创建新 entry，但共享 OpenFile；
- 对同一 inode 独立 open 两次，offset 相互独立；
- 普通 fork 复制 fd table，但共享 OpenFile；
- 父子 close 相互独立；
- descriptor-local CLOEXEC；
- exec 关闭 CLOEXEC fd；
- 稳定的 fd→OpenFile→Inode 快照。

不实现：

- 路径解析、VFS、mount 和权限；
- write、truncate 和文件扩容；
- pipe、socket、设备；
- page cache 和磁盘；
- 多线程和锁；
- dup3、F_DUPFD、完整 fcntl；
- CLONE_FILES。

### 3.5 四层对象模型

```text
Process
  └── FdTable
        └── fd -> FdEntry { cloexec, Rc<RefCell<OpenFile>> }
                                      └── OpenFile { id, offset, status_flags, Rc<Inode> }
                                                                                  └── Inode { id, data }
```

状态应放在下面这些层：

| 状态 | 所属对象 | 是否在 dup/fork 后共享 |
| --- | --- | --- |
| 文件内容 | Inode | 共享 |
| 当前 offset | OpenFile | 共享 |
| status flags | OpenFile | 共享 |
| CLOEXEC | FdEntry | 不共享，属于单个 fd |
| fd 数字和空槽 | FdTable | 普通 fork 后两张表相互独立 |

### 3.6 示例代码框架

```rust
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

pub type Pid = u32;
pub type Fd = usize;
pub type InodeId = u32;
pub type OpenFileId = u32;

#[derive(Debug)]
pub struct Inode {
    pub id: InodeId,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StatusFlags {
    pub append: bool,
    pub nonblock: bool,
}

#[derive(Debug)]
pub struct OpenFile {
    pub id: OpenFileId,
    pub inode: Rc<Inode>,
    pub offset: usize,
    pub status_flags: StatusFlags,
}

#[derive(Debug, Clone)]
pub struct FdEntry {
    pub file: Rc<RefCell<OpenFile>>,
    pub cloexec: bool,
}

#[derive(Debug, Clone)]
pub struct FdTable {
    pub entries: Vec<Option<FdEntry>>,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: Pid,
    pub fds: FdTable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    NoSuchProcess,
    NoSuchInode,
    BadFd,
    InvalidSeek,
    TooManyFiles,
}

pub struct FileModel {
    init_pid: Pid,
    next_pid: Pid,
    next_inode_id: InodeId,
    next_open_file_id: OpenFileId,
    inodes: BTreeMap<InodeId, Rc<Inode>>,
    processes: BTreeMap<Pid, Process>,
}

impl FileModel {
    pub fn new() -> Self {
        todo!("创建初始进程和对象 id 分配器")
    }

    pub fn create_inode(&mut self, data: &[u8]) -> InodeId {
        todo!("创建内存 inode")
    }

    pub fn init_pid(&self) -> Pid {
        self.init_pid
    }

    pub fn open(&mut self, pid: Pid, inode: InodeId) -> Result<Fd, ModelError> {
        todo!("每次 open 创建新的 OpenFile，再分配 fd")
    }

    pub fn read(&mut self, pid: Pid, fd: Fd, len: usize) -> Result<Vec<u8>, ModelError> {
        todo!("从共享 OpenFile.offset 开始读取并推进 offset")
    }

    pub fn seek(&mut self, pid: Pid, fd: Fd, offset: usize) -> Result<(), ModelError> {
        todo!("更新 OpenFile.offset，并检查范围")
    }

    pub fn dup(&mut self, pid: Pid, old_fd: Fd) -> Result<Fd, ModelError> {
        todo!("clone FdEntry 中的 Rc，分配最小可用 fd，清除新 fd 的 CLOEXEC")
    }

    pub fn close(&mut self, pid: Pid, fd: Fd) -> Result<(), ModelError> {
        todo!("只删除当前 FdTable 的一个 entry")
    }

    pub fn fork(&mut self, parent: Pid) -> Result<Pid, ModelError> {
        todo!("创建新的 FdTable，但 entry 中的 Rc<OpenFile> 继续共享")
    }

    pub fn set_cloexec(
        &mut self,
        pid: Pid,
        fd: Fd,
        enabled: bool,
    ) -> Result<(), ModelError> {
        todo!("只修改一个 FdEntry")
    }

    pub fn exec(&mut self, pid: Pid) -> Result<(), ModelError> {
        todo!("删除当前进程中 cloexec=true 的 entries")
    }

    pub fn snapshot(&self, pid: Pid) -> Result<String, ModelError> {
        todo!("稳定输出 fd、cloexec、open-file id、offset、inode id")
    }
}
```

这份框架故意没有提供 fd 分配、借用拆分和 read 的实现。这些正是本项目要训练的部分。

为降低范围，模型可以规定 seek 只接受 `0..=inode.data.len()`；真实 Linux 允许普通文件 seek 到 EOF 之后，这一差异应写在 README 中，但本周不要求模拟稀疏文件。

### 3.7 示例事件和预期关系

假设 inode 内容是 `abcdef`：

```text
fd3 = open(inode)       -> fd3 -> open#1(offset=0) -> inode#1
fd4 = dup(fd3)          -> fd4 -> open#1(offset=0) -> inode#1
read(fd3, 2) == "ab"   -> open#1.offset=2
read(fd4, 2) == "cd"   -> open#1.offset=4

fd5 = open(inode)       -> fd5 -> open#2(offset=0) -> inode#1
read(fd5, 2) == "ab"   -> open#2.offset=2，open#1 仍为 4
```

fork 后：

```text
parent.fd3 ─┐
            ├── open#1(offset=4) ── inode#1
child.fd3 ──┘

parent.close(fd3)
  parent.fd3 失效
  child.fd3 仍指向 open#1
```

### 3.8 示例测试框架

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dup_shares_offset_but_independent_open_does_not() {
        let mut model = FileModel::new();
        let pid = model.init_pid();
        let inode = model.create_inode(b"abcdef");

        let fd1 = model.open(pid, inode).unwrap();
        let fd2 = model.dup(pid, fd1).unwrap();

        assert_eq!(model.read(pid, fd1, 2).unwrap(), b"ab");
        assert_eq!(model.read(pid, fd2, 2).unwrap(), b"cd");

        let fd3 = model.open(pid, inode).unwrap();
        assert_eq!(model.read(pid, fd3, 2).unwrap(), b"ab");
    }

    #[test]
    fn fork_shares_open_file_but_close_is_per_fd_table() {
        let mut model = FileModel::new();
        let parent = model.init_pid();
        let inode = model.create_inode(b"abcdef");
        let fd = model.open(parent, inode).unwrap();
        let child = model.fork(parent).unwrap();

        assert_eq!(model.read(parent, fd, 2).unwrap(), b"ab");
        assert_eq!(model.read(child, fd, 2).unwrap(), b"cd");

        model.close(parent, fd).unwrap();
        assert_eq!(model.read(parent, fd, 1), Err(ModelError::BadFd));
        assert_eq!(model.read(child, fd, 1).unwrap(), b"e");
    }
}
```

### 3.9 必做测试清单

- 一次 open 后连续 read 正确推进 offset；
- 两次独立 open 同一 inode，offset 独立；
- dup 后交错 read，共享 offset；
- dup 后任一 fd seek，另一个 fd 看到相同 offset；
- close 原 fd 后 dup fd 仍有效；
- 关闭中间 fd 后，下一次 open/dup 复用最小空槽；
- fork 后父子交错 read，共享 offset；
- 父 close 不影响 child 的同号 fd；
- 设置一个 fd 的 CLOEXEC 不影响另一个 dup fd；
- exec 只关闭 CLOEXEC fd；
- dup 一个 CLOEXEC fd，新 fd 默认不带 CLOEXEC；
- BadFd、InvalidSeek 等错误不改变 offset 和 fd table。

OpenFile 最后一个引用释放的观测可以作为加分项，不强制要求实现复杂的 release registry。能够通过 close/fork 测试证明“有引用就仍然可用”已经足够。

### 3.10 项目完成后的 RespOS 阅读任务

每名队员回答：

1. `FdTable::from_existed_user` clone 了什么，没有 clone 什么？
2. 为什么父子拥有不同 fd table，却仍共享 `FileInner.offset`？
3. `sys_dup` 为什么只需要 clone FdEntry 就能共享 File？
4. `sys_dup` 为什么清除新 fd 的 CLOEXEC？
5. `FdTable::close_on_exec` 为什么不能直接清空整张表？
6. 当前 `sys_fcntl(F_SETFL)` 修改的 flags 位于哪一层？dup 后是否共享需要怎样的 user test 才能证明？

最后设计一个 RespOS user binary 测试，包含三组对照：

```text
dup：共享 offset
fork：共享 offset，但 close 动作独立
两次 open：offset 独立
```

只要求写测试方案和预期输出；本周不要求修改内核。

## 4. 队长如何提供帮助

这两个项目不需要采用严格教学流程。队长主要在队员卡住时判断问题属于哪一类。

### Rust 语法或借用问题

可以直接解释：

- `Rc::clone`、`RefCell::borrow_mut` 的使用方式；
- 如何缩短可变借用范围；
- 如何拆模块和暴露测试接口；
- 编译错误具体在说什么。

这类帮助不会破坏训练目标。

### 对象关系问题

先让队员画图，不直接说字段应该放在哪里。可以问：

- 这个状态是某个 fd 私有的，还是多个 fd 共同观察的？
- exit 后父进程还需要读取哪些信息？
- 当前操作修改后，关系的另一侧是否也需要更新？
- 如果这个对象被 clone，期望复制内容还是共享身份？

### 调试问题

要求先提供：

1. 最小失败操作序列；
2. 操作前后的 snapshot；
3. 预期状态；
4. 实际第一次出现差异的位置。

不要接受只有最终 panic 或一大段无边界日志的报告。

## 5. 一周结束时的简单验收

不需要复杂评分表。每个项目检查四件事：

1. **能运行**：核心测试大部分通过，重复操作和错误路径没有明显破坏状态；
2. **能画图**：A1 能画进程状态和父子树，B1 能画 fd 四层对象关系；
3. **能解释**：能说出三个关键不变量以及一个常见错误；
4. **能回到 RespOS**：能找到对应结构和函数，讲清模型删掉了哪些复杂因素。

建议保留以下交付物：

- 两个独立 Cargo 项目；
- 每个项目一份简短 README；
- 自动测试；
- 一张状态图或对象图；
- 一页 RespOS 映射说明；
- 数个按实现阶段划分的小 Git 提交。

一周训练完成的判断不是代码是否漂亮，而是队员以后遇到进程生命周期或 fd 共享问题时，知道应该先找哪些对象、画什么关系、检查什么状态，并能用测试证明自己的理解。
