= 5. 文件系统
<5-文件系统>
#quote(block: true)[
本章回答：RespOS 如何把文件描述符、路径、目录项、inode 和具体文件系统连接起来？ext4、procfs、devfs 等后端如何复用同一套接口？页缓存和写回如何与文件 I/O、内存映射协作？
]

RespOS 的文件系统参考并实现了类 Linux 的 VFS 分层模型。VFS 的基本思想是把"用户如何访问文件"和"文件数据实际存在哪里"分开：上层面向统一的文件、目录和路径对象，下层再把这些操作转换为 ext4 或虚拟文件系统的具体实现。系统调用层负责文件描述符和用户缓冲区，路径查找、目录项缓存、文件偏移、页缓存以及后端 I/O 分别由对应的抽象对象维护。文件系统主要以 ext4 文件系统为持久化后端，同时接入 procfs、devfs、tmpfile 以及 pipe、socket 等文件对象。

本文件系统的核心设计可以概括为两点：首先，`Path`、`Dentry`、`InodeOp` 和 `FileOp` 分别承担位置、命名、后端对象和打开实例的职责；其次，文件后端 I/O 与地址空间管理相互配合，文件映射写回采用"锁内取得快照、锁外执行 I/O、失败向上返回"的协议。前者保证 VFS 模型清晰，后者保证文件系统和内存管理能够在多种访问路径下协同工作。

== 5.1 VFS 对象模型
<51-vfs-对象模型>
=== 5.1.1 从文件描述符到后端存储
<511-从文件描述符到后端存储>
在进入具体实现前，需要先区分文件系统中的几个基本概念。\*\*文件描述符（file descriptor）\*\*是进程访问一个打开对象的整数索引；\*\*打开文件对象（open-file description）\*\*表示一次打开操作产生的状态，例如当前偏移和打开状态；\*\*路径（path）\*\*表示命名空间中的位置；\*\*目录项（dentry）\*\*记录路径树中的名称和父子关系；#strong[inode]表示文件本身的属性和后端操作。多个文件描述符可以指向同一个打开文件对象，但不同路径也可能通过硬链接指向同一个 inode。

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([对象], [主要回答的问题], [典型生命周期],),
    table.hline(),
    [`FdEntry`], [这个进程的哪个 fd 指向什么对象？], [随描述符槽位创建、复制和关闭],
    [`FileOp` / `File`], [这一次打开的偏移和状态是什么？], [从 open/pipe/socket 创建到最后一个引用释放],
    [`Path`], [对象位于哪个挂载实例和目录项？], [随 cwd、root、打开文件或路径操作引用],
    [`Dentry`], [路径树中的这个名字和父子关系是什么？], [由目录项缓存和活动引用共同维持],
    [`InodeOp`], [文件属性和数据由哪个后端管理？], [由文件系统后端和打开对象共同使用],
  )]
  , kind: table
  )

#strong[表 5-1 VFS 基本对象的职责与生命周期]

RespOS 的一次普通文件访问可以沿着如下对象链路展开。图 5-1 中，自上而下的每一层承担不同生命周期的状态：

```text
用户态 fd
    │
    ▼
FdTable slot → FdEntry { Arc<dyn FileOp>, descriptor flags }
    │
    ▼
FileOp → File { offset, Path, open-file status flags, page cache }
    │
    ▼
Path { VfsMount, Dentry }
    │
    ▼
Dentry { parent, children, inode, rename identity }
    │
    ▼
InodeOp → Ext4Inode / ProcInode / DevInode / other backend inode
    │
    ▼
SuperBlockOp → ext4 / procfs / devfs superblock
```

#strong[图 5-1 RespOS 文件访问对象链路]

这条链路并不是简单的函数调用栈，而是多个对象之间的所有权关系。`FdEntry` 只属于某个文件描述符表中的一个槽位；`FileOp` 是可以被 `dup`、`fork` 后的多个描述符共享的打开对象；`Path` 将挂载实例和目录项绑定起来；`Dentry` 保持路径命名空间中的身份；`InodeOp` 把统一的 VFS 操作转换为具体后端操作。这样的分层使目录查找、打开状态和数据存储可以分别演化，也使 pipe、socket 等特殊对象能够复用 fd 接口而不必伪装成磁盘文件。

因此，两个进程分别 `open` 同一个文件时，通常得到不同的 `File` 和不同的文件偏移；而通过 `dup` 或 `fork` 复制描述符时，复制的是指向同一个 `FileOp` 的 `Arc`，所以偏移和打开状态继续共享。描述符自身的 close-on-exec 属性保存在 `FdEntry` 中，不会因为共享 `FileOp` 而被错误地传播。

=== 5.1.2 `InodeOp`：后端文件对象的最小接口
<512-inodeop后端文件对象的最小接口>
`InodeOp` 位于 VFS 与具体文件系统之间，提供节点类型、属性、按偏移读写、截断、目录查找、创建、链接和删除等操作。接口还包含符号链接、扩展属性、时间更新以及页缓存获取等可选能力：

```rust
pub trait InodeOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn node_type(&self) -> InodeType;
    fn stat(&self, path: &str) -> SysResult<KStat>;
    fn read_at(&self, path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize>;
    fn write_at(&self, path: &str, off: usize, buf: &[u8]) -> SysResult<usize>;
    fn truncate(&self, path: &str, size: usize) -> SysResult<usize>;
    fn lookup(&self, parent_path: &str, name: &str)
        -> SysResult<Arc<dyn InodeOp>>;
    fn readdir(&self, path: &str) -> SysResult<Vec<LinuxDirent64>>;
    fn create(&self, parent_path: &str, name: &str, ty: InodeType)
        -> SysResult<Arc<dyn InodeOp>>;
    fn link(&self, old_path: &str, dentry: Arc<Dentry>) -> SysResult;
    fn unlink(&self, dentry: &Arc<Dentry>) -> SysResult;
}
```

#strong[代码片段 5-1 `InodeOp` 的核心接口]

接口中的 `path` 参数反映了当前 ext4 适配层的现实：lwext4 的访问入口以路径为主要参数，`Ext4Inode` 因而同时维护 inode 身份和后端存储路径。VFS 上层不直接依赖这一实现细节，而是通过 `Dentry::current_abs_path()` 生成当前路径；rename 后，已打开文件和子目录项仍沿 dentry 父链观察新的路径。

`get_page_cache` 默认返回 `None`，只有支持普通文件页缓存的后端才覆盖它。时间、权限、扩展属性等接口采用默认失败或空实现，意味着 procfs、devfs 等后端可以只实现自身语义，而不会因为没有磁盘属性而伪造成功。这样的默认策略也让"未实现"和"无意义操作"能够以明确 errno 暴露给用户态。

=== 5.1.3 `FileOp`：打开实例与特殊文件的统一入口
<513-fileop打开实例与特殊文件的统一入口>
`FileOp` 面向的是一次打开实例，而不是抽象意义上的 inode。普通文件 `File`、管道、标准输入输出、epoll、socket 和 memfd 都通过这个 trait 接入文件描述符表。其核心操作包括顺序读写、指定偏移读写、seek、状态标志、读写权限、poll 就绪判断、截断和同步：

```rust
pub trait FileOp: Any + Send + Sync {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize>;
    fn write(&self, buf: &[u8]) -> SysResult<usize>;
    fn read_at_offset(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize>;
    fn write_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize>;
    fn can_seek(&self) -> SysResult;
    fn seek(&self, offset: isize) -> SysResult<usize>;
    fn get_offset(&self) -> usize;
    fn get_flags(&self) -> OpenFlags;
    fn set_status_flags(&self, flags: OpenFlags) -> SysResult;
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn read_ready(&self) -> bool;
    fn write_ready(&self) -> bool;
    fn register_poll_waiter(&self, tid: usize, events: PollEvents) -> bool;
    fn fsync(&self) -> SysResult<usize>;
}
```

#strong[代码片段 5-2 `FileOp` 的打开实例接口]

普通文件的 `read` 和 `write` 会推进共享 offset；`pread`/`pwrite` 通过指定偏移接口访问文件而不改变该 offset。普通文件通常总是处于读就绪和写就绪状态，pipe 则根据环形缓冲区是否为空或已满返回结果，并在阻塞路径登记等待者。不可 seek 的对象默认返回 `ESPIPE`，特殊文件可以按自身语义覆盖同步、截断和 poll 行为。

`set_status_flags` 修改的是打开对象共享的状态，例如追加写和非阻塞访问；close-on-exec 则属于描述符自身。这种分层是 `dup`、`fcntl`、`fork` 和 `exec` 正确实现 Linux ABI 语义的基础。对于普通、无特殊设计的打开标志，本章不逐项罗列，重点只讨论它们是否改变对象身份、共享范围或 I/O 状态。

=== 5.1.4 `Dentry`、`Path` 与缓存身份
<514-dentrypath-与缓存身份>
`Path` 是 `(VfsMount, Dentry)` 二元组，表示一个路径在整个挂载命名空间中的位置。`Dentry` 内部包含 inode、父目录和子目录缓存：

```rust
pub struct Dentry {
    pub abs_path: String,
    pub inner: Mutex<DentryInner>,
}

pub struct DentryInner {
    pub inode: Option<Arc<dyn InodeOp>>,
    pub parent: Option<Arc<Dentry>>,
    pub children: HashMap<String, Weak<Dentry>>,
    // rename 后用于覆盖旧路径的路径别名
    alias_path: Option<String>,
}
```

#strong[代码片段 5-3 `Dentry` 的路径树状态]

`inode == None` 表示负目录项，可缓存"该名字不存在"的查找结果。父节点使用强引用，保证仍被使用的 dentry 可以沿父链回溯并重新计算路径；子节点使用 `Weak`，避免父子双向强引用形成永久环。子缓存失效时，弱引用升级失败的项会被清理。

从生命周期看，路径查找首先得到或创建 dentry，随后由 `Path`、当前工作目录、打开的 `File` 或缓存继续持有它。unlink 删除的是目录项到名字空间的连接，不必立即销毁 inode；如果文件仍被打开，后端还要保留数据直到最后一个 `File` 释放。rename 也不是简单地创建一个新对象，而是尽量保持已有 dentry 的身份并更新其父链。这正是 dentry 与 inode 不能合并成一个对象的原因：前者描述"通过哪个名字找到"，后者描述"找到的文件是什么"。

RespOS 选择保存初始绝对路径并通过父链计算当前路径，而不是只保存 Linux 风格的短文件名。这样做简化了路径拼接，也使打开文件、当前工作目录和 rename 后的后代路径能够继续指向同一个 dentry 身份；代价是 rename 和缓存失效必须同时维护 `alias_path`、父子关系以及全局 dentry cache。`remove_dentry_cache_tree` 会在 unlink、rename 和卸载等命名空间变化时清理受影响的缓存树。

== 5.2 挂载树与路径解析
<52-挂载树与路径解析>
=== 5.2.1 `VfsMount` 与 `Mount`
<521-vfsmount-与-mount>
RespOS 将"一个文件系统实例"和"它挂载到哪里"分开表示。`VfsMount` 保存该实例的根 dentry、超级块和挂载 flags；`Mount` 保存挂载点、父挂载和子挂载，形成全局 mount tree：

```rust
pub struct VfsMount {
    pub root: Arc<Dentry>,
    pub fs: Arc<dyn SuperBlockOp>,
    flags: AtomicI32,
}

pub struct Mount {
    pub mountpoint: Arc<Dentry>,
    pub vfs_mount: Arc<VfsMount>,
    pub parent: Option<Weak<Mount>>,
    pub children: Mutex<Vec<Arc<Mount>>>,
}
```

#strong[代码片段 5-4 文件系统实例与挂载树节点]

路径解析遇到挂载点时，会把游标中的 `mnt` 和 `dentry` 一起切换到子文件系统根目录。反向解析 `..` 时，如果当前 dentry 已经是子文件系统根，则沿 `Mount.parent` 返回父文件系统中的挂载点。这样，路径语义既能在每个文件系统内部保持独立，又能在全局命名空间中正确跨越挂载边界。

当前初始化流程把 ext4 根文件系统作为主存储后端，再将 procfs 和 devfs 以独立的 `VfsMount` 挂载到 `/proc` 和 `/dev`。tmpfs/shm 等接口也通过 VFS 和特殊文件对象提供相应语义。部分挂载使用根文件系统中的隐藏 backing 目录保存后端数据，以便在现有 ext4 镜像上提供隔离的挂载根；这属于当前实现的适配方案，不应误解为每个虚拟文件系统都直接拥有独立块设备。

=== 5.2.2 namei 路径遍历
<522-namei-路径遍历>
路径查找由 `Nameidata` 驱动。它保存当前挂载实例、当前 dentry、待处理的路径分量和遍历深度。相对路径从当前工作目录或 `dirfd` 对应的目录开始，绝对路径从任务 root 开始；`dirfd`、空路径、符号链接跟随等路径操作选项由 syscall 层传入并在 namei 中解释。本章只展开这些选项如何改变查找起点和遍历过程，不逐项罗列所有 ABI 常量。

一次典型的 `link_path_walk` 过程如下：

```text
校验 PATH_MAX / NAME_MAX 和 dirfd
        │
        ▼
确定起点：root、cwd 或 dirfd 对应目录
        │
        ▼
逐分量查找 dentry cache
        ├─ 命中：检查挂载穿越并继续
        └─ 未命中：调用当前 inode.lookup，创建 dentry 并缓存
        │
        ▼
处理 .、..、符号链接和挂载点
        │
        ▼
得到 Path { mnt, dentry }
```

#strong[图 5-2 namei 路径解析流程]

符号链接展开采用重新组织剩余路径的迭代流程，并限制最多跟随 40 次，避免深层链接造成无限循环或内核栈增长。对 create、unlink、rename、link 和 symlink 等修改命名空间的操作，当前实现使用 `NAMEI_MUTATION_LOCK` 串行化关键路径，以简化 dentry cache、后端路径和失败回滚之间的一致性维护。

这一设计将路径修改、后端操作和 dentry 缓存维护纳入同一条可控的更新路径，便于保证 create、unlink 和 rename 的可观察结果一致。它也是当前 VFS 适配路径型 lwext4 接口时采用的工程取舍：先保证命名空间状态和失败回滚的正确性，再根据实际并发负载进一步细化锁粒度。

== 5.3 ext4 与虚拟文件系统后端
<53-ext4-与虚拟文件系统后端>
=== 5.3.1 ext4 适配层
<531-ext4-适配层>
RespOS 通过 `lwext4_rust` 的 FFI 接入 ext4。`Ext4Inode` 将 VFS 的 `InodeOp` 操作翻译成 lwext4 调用，并在其上维护内核侧状态，包括 inode 号、属性覆盖、时间、打开计数和页缓存。ext4 后端内部使用全局 inode cache 保存弱引用，避免仅因为缓存项存在就无限延长 inode 生命周期。

当前适配层特别处理了三个 lwext4 不直接替 VFS 完成的语义：

+ #strong[打开计数。] 创建 `File` 时增加 `open_files`，释放最后一个 `File` 时减少计数。这个计数表示 open-file description 的生命周期，不能用 `Arc::strong_count` 代替。
+ #strong[unlink 后继续访问。] 对仍有打开者、且链接数将降为零的普通文件，先改名为隐藏的 `.respos_orphan_<ino>` 路径，并在内存中将链接数视为零；最后一个打开对象释放时再删除隐藏文件。
+ #strong[页缓存接入。] 普通 ext4 文件返回共享 `PageCache`，读写优先访问缓存，必要时再向 lwext4 读取或写回。

因此，unlink 并不立即破坏已经打开的 `File`。与此同时，rename 保持已有 dentry 身份并更新路径覆盖，使 cwd、已打开文件和后代 dentry 不会因为后端路径变化而失去关联。rename 的替换路径还会暂存被覆盖的普通文件或目录；主操作提交后，清理暂存项失败不能再向用户报告 rename 失败，否则会出现"返回失败但命名空间已经改变"的状态矛盾。

=== 5.3.2 procfs、devfs 与特殊文件
<532-procfsdevfs-与特殊文件>
procfs 和 devfs 都实现 `SuperBlockOp`，并通过独立挂载实例提供自己的根 inode。它们的 inode 通常动态生成内容，而不是从 ext4 数据块读取。例如 `/proc/cpuinfo`、`/proc/meminfo`、`/proc/self/maps` 等文件在 read 时根据内核状态生成文本；`/dev/null`、`/dev/zero`、`/dev/random`、tty、shm 等设备则由各自的 `FileOp` 或 `InodeOp` 实现读写语义。

pipe 不依赖磁盘 inode。两个端点共享一个带有读写关闭状态、FIFO 等待队列和 poll waiter 的环形缓冲区；当没有读者或写者时，端点生命周期决定 EOF 和 `SIGPIPE` 等可观察行为。socket、eventfd、epoll、timerfd 等对象也直接以 `FileOp` 形式进入 `FdTable`。这些对象复用文件描述符、poll 和 close 机制，但不应被强行套入 ext4 的页缓存或路径生命周期。

== 5.4 页缓存与持久化写回
<54-页缓存与持久化写回>
=== 5.4.1 PageCache 的职责
<541-pagecache-的职责>
`PageCache` 以页为单位缓存普通文件内容，并维护缓存长度、页内容、脏状态和写回版本等信息。`File` 构造时从 inode 获取共享页缓存；顺序读、指定偏移读和普通写都优先通过 `PageCache` 完成。首次读取尚未驻留的区间时，缓存向后端发起按偏移读取；写入则先修改缓存页，再根据同步标志或显式同步请求执行写回。

页缓存把用户可见的文件长度和后端磁盘长度分开管理：写入扩大缓存后，`stat` 可以观察到新的长度，真正的后端更新由写回链路提交。`O_SYNC`、`O_DSYNC` 和 `fsync` 会提高写回强度；普通 close 路径也会尝试刷新自身的脏页，最后一个 ext4 打开对象关闭时还会触发孤儿文件清理。

=== 5.4.2 三层写回链
<542-三层写回链>
当前普通文件的持久化路径可以概括为：

```text
FileOp::write / mmap 写入
        │
        ▼
PageCache：修改页、标记脏页、更新逻辑长度
        │
        ▼
InodeOp::write_at：按路径写入 lwext4 或其他后端
        │
        ▼
SuperBlockOp::sync：提交文件系统级缓存和块设备状态
```

#strong[图 5-3 文件写入与持久化写回链路]

`File::fsync` 会先刷新关联页缓存，再调用当前路径所属 `VfsMount` 的超级块同步操作。对普通 ext4 文件，缓存写回和后端同步是两个层次：前者把内存中的脏页交给 inode 后端，后者推动文件系统和设备完成更底层的提交。不能只调用其中一层就宣称数据已经持久化。

=== 5.4.3 与 `mmap(MAP_SHARED)` 的协作
<543-与-mmapmap_shared-的协作>
文件映射的写回由 MM 和 FS 共同完成。建立可写共享映射前，`FileOp::mmap_allowed` 检查文件是否可读，并要求 `MAP_SHARED|PROT_WRITE` 对应的 open-file description 可写。映射建立后，页面内容由地址空间管理；在 `msync`、`munmap`、MAP\_FIXED 替换、mremap 收缩/覆盖、mprotect 或进程退出等路径中，MM 在地址空间锁内取得 resident frame 的快照，然后释放 `MemorySet` 锁，再通过 `FileOp` 写入页缓存或后端，最后按需要调用 `fsync`。

这种"快照---锁外 I/O---同步"的协议有两个目的：避免文件后端 I/O 持有地址空间写锁导致锁序反转，也避免后端阻塞把所有地址空间操作串在一起。由于当前硬件路径没有统一提供 dirty bit，系统对 resident writable shared file pages 采用保守写回，从而以更明确的同步行为换取实现可靠性。

== 5.5 文件描述符表与生命周期
<55-文件描述符表与生命周期>
=== 5.5.1 `FdTable` 与 `FdEntry`
<551-fdtable-与-fdentry>
`FdTable` 是进程持有的描述符槽位集合，内部使用自旋锁保护槽位数组，并维护下一个分配位置及 `RLIMIT_NOFILE` 限制。每个槽位保存一个 `FdEntry`：

```rust
pub struct FdEntry {
    pub file: Arc<dyn FileOp>,
    pub flags: OpenFlags,
}
```

#strong[代码片段 5-5 描述符槽位与打开对象的关系]

`FdEntry::flags` 保存每个描述符独有的标志，当前最重要的是 close-on-exec。`file` 指向共享的 `FileOp`，其内部保存 offset、追加写和非阻塞等 open-file 状态。关闭描述符时，先从表中移除槽位，再由 `Arc` 引用计数决定何时释放 `FileOp`；普通 `File` 的析构会尝试同步页缓存，并减少 ext4 inode 的打开计数。

=== 5.5.2 dup、fork、exec 与 close
<552-dupforkexec-与-close>
- `dup` 和 `fork` 复制 `Arc<dyn FileOp>`，因此共享文件偏移和打开状态，但为新描述符创建独立的 `FdEntry`，可以单独设置 close-on-exec。
- `exec` 根据每个 `FdEntry` 的 close-on-exec 状态关闭描述符；共享的 `FileOp` 只有在没有其他引用时才析构。
- `close` 从 `FdTable` 移除槽位后，pipe、socket、特殊 fd 和普通文件分别执行自己的最后引用清理；pipe 的最后一个写端关闭才会让读端观察到 EOF，最后一个读端关闭则影响写端错误和阻塞唤醒。
- `fcntl` 修改描述符标志和打开状态时必须区分上述两个对象，不能把 close-on-exec 写入 `File` 的共享状态。

这一分层也解释了为什么文件系统章节不能只讨论磁盘 inode：用户态可观察的文件语义同时由命名空间身份、打开实例状态和描述符生命周期决定。

== 5.6 功能与设计总结
<56-功能与设计总结>
文件系统并不是孤立的存储模块，而是连接系统调用、进程、内存和设备的基础设施。表 5-2 总结了 RespOS 已形成的主要功能和对应设计。

| 功能或设计 | 主要实现 | | -\-\- | -\-\- | -\-\- | | VFS 对象抽象 | 通过 `Path`、`Dentry`、`InodeOp`、`FileOp` 和 `SuperBlockOp` 分离路径、命名、文件对象、打开状态与后端存储 | | 磁盘文件系统 | 通过 `lwext4_rust` 接入 ext4，支持普通文件、目录、符号链接、硬链接、rename、unlink 和属性操作 | | 虚拟文件系统 | 以独立挂载实例接入 procfs、devfs，并提供 `/proc`、`/dev`、shm 等内核和设备接口 | | 路径与挂载 | 支持 cwd/root/dirfd 起点、dentry cache、符号链接、`.`/`..` 以及跨挂载点路径遍历 | | 文件访问性能 | 通过 ext4 inode 级共享页缓存、目录项缓存和目录读取缓存减少后端访问 | | 数据一致性 | 通过页缓存写回、`fsync`、unlink 孤儿文件和 rename 回滚维护文件数据与命名空间的一致性 | | 内核模块协作 | 通过 file-backed VMA、`mmap` 写回、pipe、socket、poll 和统一 fd 生命周期连接 MM、task、IPC、network 与 drivers |

#strong[表 5-2 文件系统主要功能与设计对应关系]

从整体上看，RespOS 文件系统形成了从用户态 fd 到 ext4 块设备的完整调用链：fd 首先定位到打开对象，打开对象持有路径，路径连接挂载实例和 dentry，dentry 再通过 `InodeOp` 调用具体文件系统。围绕这条主线，系统同时支持磁盘文件、虚拟文件、设备文件和无路径特殊文件，并将缓存、同步、内存映射和进程生命周期纳入统一的文件对象模型。

这种设计既保持了 Linux VFS 模型对用户态的可理解性，又利用 Rust 的 `Arc`、trait 和错误传播机制管理了对象生命周期、后端替换和失败路径，为后续扩展更多文件系统或设备类型保留了清晰接口。
