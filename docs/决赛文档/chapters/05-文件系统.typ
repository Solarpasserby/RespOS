= 5. 文件系统
<5-文件系统>
#quote(block: true)[
本章回答：RespOS 如何把文件描述符、路径、目录项、inode 和具体文件系统连接起来？ext4、procfs、devfs 等后端如何复用同一套接口？页缓存和写回如何与文件 I/O、内存映射协作？
]

RespOS 的文件系统参考并实现了类 Linux 的 VFS 分层模型。它首先把三个容易混在一起的问题分开：文件在目录树中的位置是什么，文件本身具有什么属性和操作，某一次打开之后又处于什么状态。路径查找解决第一个问题，`InodeOp` 解决第二个问题，`FileOp` 解决第三个问题，`FdTable` 再把这些内核对象映射为用户态看到的整数 fd。

可以用三句话区分这些层次：`Dentry/Path` 表示"从哪里找到它"，`InodeOp` 表示"找到的对象是什么"，`FileOp` 表示"这次打开怎样使用它"。因此，`InodeOp` 不保存某个调用者的文件偏移，也不直接对应用户态 fd；`FileOp` 才保存一次打开实例的 offset、status flags 和 poll 状态。一个 inode 可以对应多个打开实例，一个打开实例又可以被多个 fd 共享。

文件系统主要以 ext4 作为持久化后端，同时接入 procfs、devfs、tmpfile、pipe、socket 等对象。它们不需要都具有磁盘 inode 或路径，但只要实现相应的 `FileOp`，就可以复用 `read`、`write`、`poll`、`fcntl` 和 `close` 等文件描述符语义。文件后端 I/O 与地址空间管理则通过 `FileOp` 的按偏移访问和 mmap 写回接口连接起来。

== 5.1 VFS 对象模型
<51-vfs-对象模型>
=== 5.1.1 `File`：一次打开文件的运行时状态
<511-file一次打开文件的运行时状态>
文件系统对象进入用户态前，会经历"路径找到 inode、根据 inode 建立打开对象、把打开对象放入 fd 表"的过程。RespOS 中，普通文件的打开实例由 `File` 表示，并实现 `FileOp` trait。`File` 自身保存稳定的 inode 引用，动态变化的打开状态则集中放在受互斥锁保护的 `FileInner` 中：

```rust
pub struct File {
    inode: Arc<dyn InodeOp>,
    inner: Mutex<FileInner>,
}

struct FileInner {
    offset: usize,
    path: Arc<Path>,
    flags: OpenFlags,
    dirent_cache: Option<Arc<Vec<LinuxDirent64>>>,
    page_cache: Option<Arc<PageCache>>,
    write_back: bool,
    // 省略 tmpfile 与时间覆盖字段
}
```

代码片段 5-1 `File` 与 `FileInner` 的核心状态

`FileInner` 中的 `offset` 以字节为单位，表示该 open-file description 当前的访问位置；`path` 将这次打开与挂载实例中的 dentry 绑定；`flags` 保存打开状态；`page_cache` 和 `write_back` 则把普通文件的 I/O 与缓存写回连接起来。`File` 构造时会增加 ext4 inode 的打开计数，并根据 inode 类型取得共享页缓存。

打开标志会直接影响初始状态。若指定 `O_TRUNC` 且以写模式打开，构造阶段先将文件截断并同步调整缓存长度；若指定 `O_APPEND`，普通文件的初始 offset 设置为当前文件大小，否则从零开始。之后 `read`/`write` 在 `FileInner` 锁内推进 offset，因此两个独立的 `open` 拥有独立访问位置，而 `dup` 或 fork 复制同一个 `Arc<dyn FileOp>` 时会共享该位置。

=== 5.1.2 `FileOp`：统一不同类型文件的使用方式
<512-fileop统一不同类型文件的使用方式>
`File` 实现 `FileOp`，这个 trait 描述用户通过 fd 使用一个打开对象时需要的行为，而不是描述磁盘上的文件身份。普通文件、管道、标准输入输出、socket、epoll 和 memfd 都可以以 `Arc<dyn FileOp>` 进入 `FdTable`：

```rust
pub trait FileOp: Any + Send + Sync {
    fn read(&self, buf: &mut [u8]) -> SysResult<usize>;
    fn write(&self, buf: &[u8]) -> SysResult<usize>;
    fn read_at_offset(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize>;
    fn write_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize>;
    fn can_seek(&self) -> SysResult;
    fn seek(&self, offset: isize) -> SysResult<usize>;
    fn get_offset(&self) -> usize;
    fn set_status_flags(&self, flags: OpenFlags) -> SysResult;
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn read_ready(&self) -> bool;
    fn write_ready(&self) -> bool;
    fn register_poll_waiter(&self, tid: usize, events: PollEvents) -> bool;
    fn fsync(&self) -> SysResult<usize>;
}
```

代码片段 5-2 `FileOp` 的核心操作接口

其中，`read`、`write` 和 `seek` 使用并更新打开实例的共享 offset；`read_at_offset` 和 `write_at_offset` 则提供不改变 offset 的无状态访问，供 `pread`、`pwrite` 和文件映射等路径使用。`read_ready`、`write_ready` 与 `register_poll_waiter` 表达阻塞和就绪语义：普通文件通常直接报告可读写，pipe 根据缓冲区和端点状态判断，socket 则根据协议缓冲区判断。不可 seek 的对象保留默认的 `ESPIPE` 行为，特殊文件可以覆盖相应操作。

`FileOp` 的价值在于把"如何使用文件"从"文件实际存储在哪里"中分离出来。pipe 和 socket 不需要伪造 ext4 inode，也可以通过同一接口接入 `read`、`write`、poll 和 close；普通文件则在 `FileOp` 实现内部继续调用 `InodeOp`、`Path` 和 `PageCache`。描述符自身的 `CLOEXEC` 位保存在 `FdEntry` 中，不属于共享的 `FileOp` 状态。

=== 5.1.3 `InodeOp`：文件对象的后端能力边界
<513-inodeop文件对象的后端能力边界>
如果说 `FileOp` 回答"这次打开怎么用"，`InodeOp` 回答的就是"这个文件对象是什么、后端能够对它做什么"。它不保存某个打开实例的 offset，也不直接对应用户态 fd，而是提供节点类型、属性、按偏移 I/O、截断、目录查找、创建、链接、删除和可选的页缓存等能力：

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

代码片段 5-3 `InodeOp` 的后端能力接口

`read_at` 和 `write_at` 使用显式偏移，因而同一 inode 可以被多个打开实例访问而不会共享打开位置；`lookup`、`create` 和 `unlink` 则表达目录在命名空间中的操作能力。`get_page_cache` 默认返回 `None`，只有支持普通文件缓存的后端覆盖它；时间、权限和扩展属性接口在不适用的后端上返回明确错误或空结果，procfs、devfs 等对象不需要伪造磁盘文件语义。

当前接口中的 `path` 参数来自 lwext4 的路径型访问方式。VFS 通过 `Path` 和 `Dentry::current_abs_path()` 生成当前后端路径，再由具体 inode 实现把统一操作转换成底层调用。这个参数是当前 ext4 适配策略的实现细节，不改变 `InodeOp` 的职责：它仍然表示文件对象的后端能力，而不是一次打开的状态。

=== 5.1.4 `Ext4Inode`：InodeOp 的具体实现
<514-ext4inodeinodeop-的具体实现>
`Ext4Inode` 是 RespOS 对 ext4 普通文件和目录的 VFS 适配对象。它没有把 lwext4 的临时打开句柄直接暴露给上层，而是长期保存 inode 身份、路径变化和内核侧缓存状态；每次执行具体 I/O 时，再以当前有效路径创建短生命周期的 `Ext4File` 调用 lwext4：

```rust
pub struct Ext4Inode {
    pub ino: u64,
    ty: Ext4InodeTypes,
    times: Mutex<Option<InodeTimes>>,
    mode_override: Mutex<Option<u32>>,
    owner_override: Mutex<Option<(u32, u32)>>,
    nlink_override: Mutex<Option<u32>>,
    orphan_path: Mutex<Option<String>>,
    renamed_path: Mutex<Option<String>>,
    open_files: AtomicUsize,
    page_cache: Arc<PageCache>,
    xattrs: Mutex<HashMap<String, Vec<u8>>>,
}
```

代码片段 5-4 `Ext4Inode` 的核心状态

这些字段分别承担四类职责：`ino` 和 `ty` 标识后端对象；时间、权限、所有者和链接数覆盖字段保存 VFS 需要但当前后端不能直接稳定提供的元数据；`orphan_path` 与 `renamed_path` 处理路径变化期间仍然存活的 inode；`open_files` 区分真正的打开实例数量；`page_cache` 使同一个 inode 的多个 `File` 共享文件数据缓存。

ext4 inode 的创建经过 inode cache：`get_or_create` 先按 inode 号查找全局缓存，缓存中只保存 `Weak` 引用；已有 `Arc<Ext4Inode>` 仍被使用时，新的路径查找会复用同一 inode 对象，所有打开实例也就共享同一份页缓存和内核侧元数据。缓存项不拥有 inode 的生命周期，最后一个强引用释放后，失效的弱引用可以被清理，避免缓存本身阻止 inode 回收。

具体操作需要同时处理 VFS 语义和 lwext4 的错误、锁与路径约束。`lookup` 通过 ext4 目录遍历获得子 inode 号和类型，再调用 `get_or_create`；`read_at`、`write_at`、`stat` 和 `truncate` 使用 `storage_path` 选择当前后端路径，创建临时 `Ext4File` 完成底层操作，并通过 `map_lwext4_err` 把 lwext4 错误转换为 Linux 风格 errno。对共享的 lwext4 入口，适配层使用 `EXT4_OP_LOCK` 串行化 FFI 调用，避免底层全局状态被并发破坏。

`Ext4Inode` 还承担普通 inode 生命周期中最难由单次路径调用解决的两类语义：

- #strong[rename。] dentry 可以保持自身身份并更新父链，但 lwext4 后端需要访问新的路径。RespOS 将新路径记录到 `renamed_path`，使已经打开的 `File` 后续通过 `storage_path` 不再访问旧路径。
- #strong[unlink 与延迟清理。] 如果文件仍有打开者且链接数归零，适配层先把后端文件改名为 `.respos_orphan_<ino>`，将内核侧链接数覆盖为零；最后一个 `File` 关闭时，`open_files` 归零并删除这个隐藏路径。这样目录项可以先从命名空间消失，已有 fd 仍然可以继续访问文件内容。

因此，`Ext4Inode` 并不是简单的"lwext4 inode 指针包装"，而是 VFS 语义与路径型 ext4 后端之间的适配层：它统一缓存 inode 身份，维护打开计数和元数据覆盖，转换错误码，协调 rename/unlink 生命周期，并把普通文件读写接入共享页缓存。

=== 5.1.5 `Dentry`、`Path` 与对象定位
<515-dentrypath-与对象定位>
`Path` 是 `(VfsMount, Dentry)` 二元组，表示一个对象在整个挂载命名空间中的位置；`Dentry` 维护名字、父目录、子目录缓存以及可选的 `Arc<dyn InodeOp>`：

```rust
pub struct DentryInner {
    pub inode: Option<Arc<dyn InodeOp>>,
    pub parent: Option<Arc<Dentry>>,
    pub children: HashMap<String, Weak<Dentry>>,
    alias_path: Option<String>,
}
```

代码片段 5-5 `Dentry` 的路径树状态

`inode == None` 表示负目录项，可缓存"该名字不存在"的结果。父节点使用强引用以支持沿父链重建路径，子节点使用 `Weak` 避免形成永久引用环。rename 时 dentry 保持对象身份，通过父链和 `alias_path` 计算新的后端路径；unlink 删除名字空间中的 dentry 连接，但不必立即销毁仍被打开的 inode。

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

#strong[代码片段 5-6 文件系统实例与挂载树节点]

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
RespOS 通过 `lwext4_rust` 的 FFI 接入 ext4，`Ext4Inode` 负责把 `InodeOp` 的统一语义翻译为 lwext4 的路径型接口。VFS 层不直接操作 lwext4 的临时文件句柄，而是由 inode 根据当前 `storage_path` 创建短生命周期的 `Ext4File`，完成读写、属性查询、目录遍历和命名空间修改；底层错误再由适配层转换为 Linux 风格 errno。

这一适配层还补齐了 lwext4 不直接替 VFS 管理的语义：`open_files` 单独记录 `File` 数量，不能用 `Arc::strong_count` 代替；普通 ext4 inode 通过 `get_page_cache` 接入共享页缓存；rename 后通过 `renamed_path` 保证旧的打开对象继续访问正确的后端路径；unlink 后通过 orphan 路径延迟清理，使名字从目录树中消失后，已有 fd 仍能继续访问文件。具体的 inode 状态和这些生命周期协议见 5.1.4，本节只说明它们如何作为 ext4 后端能力对外提供。

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

#strong[代码片段 5-7 描述符槽位与打开对象的关系]

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
