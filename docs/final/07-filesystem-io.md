# 第七章 文件系统

RespOS 的文件系统采用分层抽象的 VFS（Virtual File System）架构，通过 dentry、inode、super_block 三层核心 trait 实现了文件访问与后端存储的解耦。这一设计使得上层的 open/read/write 系统调用能在不感知 ext4、procfs、devfs 差异的情况下完成文件操作，同时为未来的文件系统后端扩展保留了清晰的接口边界。

## 7.1 VFS 对象模型

### 7.1.1 五层抽象：从 fd 到磁盘

RespOS 的文件系统栈从用户态文件描述符到最终磁盘数据共跨越五层抽象。这一设计与 Linux 的 `fd → struct file → struct path { vfsmount, dentry } → struct inode` 模型在结构上完全对齐，但在 Rust 的所有权语义下采用了 `Arc` 引用计数而非手动 `atomic_inc/dec` 管理生命周期。

```
int fd（用户态）
    │  FdTable[fd]
    ▼
FdEntry { file: Arc<dyn FileOp>, flags }     ← per-fd 标志（仅 CLOEXEC 有意义）
    │
    ▼
FileOp trait（open-file description）        ← offset、O_APPEND、O_NONBLOCK、写回策略
    │  File / Pipe / SpecialFd
    ▼
Path { mnt: Arc<VfsMount>, dentry: Arc<Dentry> }  ← 文件系统实例 + 目录缓存项
    │
    ▼
Dentry（目录项缓存）                          ← abs_path + parent(Arc) + children(Weak)
    │  通过 InodeOp trait 访问 inode
    ▼
InodeOp trait（inode 操作）                   ← read_at / write_at / lookup / create ...
    │  Ext4Inode / ProcInode / DevInode
    ▼
SuperBlockOp trait（超级块）                  ← root_inode() / sync() / statfs()
```

这一架构的核心洞察在于将两个不同生命周期的概念拆分为独立的 trait。`InodeOp` 管理的是文件自身的属性和数据——它在文件存续期间保持不变，无论该文件被打开多少次，底层的 inode 始终是同一个。`FileOp` 管理的是打开实例的状态——offset（当前读写位置）、O_APPEND 语义、O_NONBLOCK 阻塞策略——这些状态属于"这一次打开"，不同进程打开同一文件拥有相互独立的 offset，而 fork 或 dup 复制的 fd 则共享同一个 FileOp 实例和 offset。

```rust
// vfs/inode.rs:13 — 文件系统后端实现的最小接口
pub trait InodeOp: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;                              // 类型擦除逃生口
    fn node_type(&self) -> InodeType;                          // Regular/Directory/SymLink/...
    fn stat(&self, path: &str) -> SysResult<KStat>;            // 元信息
    fn read_at(&self, path: &str, off: usize, buf: &mut [u8]) -> SysResult<usize>;
    fn write_at(&self, path: &str, off: usize, buf: &[u8]) -> SysResult<usize>;
    fn truncate(&self, path: &str, size: usize) -> SysResult<usize>;
    fn lookup(&self, parent_path: &str, name: &str) -> SysResult<Arc<dyn InodeOp>>;
    fn readdir(&self, path: &str) -> SysResult<Vec<LinuxDirent64>>;
    fn create(&self, parent_path: &str, name: &str, ty: InodeType) -> SysResult<Arc<dyn InodeOp>>;
    fn link(&self, old_path: &str, bare_dentry: Arc<Dentry>) -> SysResult;
    fn unlink(&self, valid_dentry: &Arc<Dentry>) -> SysResult;
    // 8 个可覆盖的默认方法: get_page_cache / set_times / set_mode / set_owner / xattr...
}
```

InodeOp trait 共 20 个方法，其中 10 个提供默认实现。默认方法返回 `Errno::EINVAL` 或 `Errno::ENOSYS`——这种设计允许后端文件系统只实现自己支持的语义而忽略无关操作。例如 procfs 的 read_at 是动态生成字符串而非读取磁盘块，其 set_times 和 set_mode 继承默认的 EINVAL 拒绝；ext4 则几乎全覆盖。这种"按需覆盖"的策略在功能完整性和代码简洁性之间取得了平衡。

```rust
// file.rs:46 — per-open-file 操作的统一接口
pub trait FileOp: Any + Send + Sync {
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize>;
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize>;
    fn read_at_offset(&self, offset: usize, buf: &mut [u8]) -> SysResult<usize>;  // pread 不碰 offset
    fn write_at_offset(&self, offset: usize, buf: &[u8]) -> SysResult<usize>;     // pwrite 不碰 offset
    fn seek(&self, offset: isize) -> SysResult<usize>;
    fn get_flags(&self) -> OpenFlags;
    fn get_stat(&self) -> SysResult<KStat>;
    fn readable(&self) -> bool;
    fn writable(&self) -> bool;
    fn read_ready(&self) -> bool;        // 非阻塞模式下的数据可用性
    fn write_ready(&self) -> bool;       // 同上，写空间可用性
    fn fsync(&self) -> SysResult<usize>; // 脏页 → inode → 超级块刷盘
    fn truncate(&self, size: usize) -> SysResult<usize>;
    fn mmap_allowed(&self, shared: bool, writable: bool) -> SysResult;
    fn register_poll_waiter(&self, tid: usize, events: PollEvents) -> bool;
    // ...
}
```

FileOp trait 的默认实现体现了清晰的分类策略。普通文件默认 `read_ready()` / `write_ready()` 永远返回 true——数据立即可得，无需阻塞等待。pipe 覆盖这两个方法为"缓冲区非空/非满时返回 true"，并覆盖 `register_poll_waiter` 注册 epoll 通知。pread/pwrite 方法默认返回 `ESPIPE`——仅支持 seek 的 fd 类型才实现。这种"不做不该做的事"的默认策略使得添加新的 fd 类型变得安全。

### 7.1.2 Dentry：路径到 inode 的缓存映射

Dentry 不仅是缓存——它是 VFS 层的路径命名空间的核心数据结构。其设计面临 Rust 所有权模型下的一个根本张力：如何在保证内存安全的前提下维护一棵带有双向引用的树结构。

```rust
// vfs/dentry.rs:19
pub struct Dentry {
    pub abs_path: String,                       // 文件全路径（非 Linux 的分量名 d_name）
    pub inner: Mutex<DentryInner>,
}

struct DentryInner {
    pub inode: Option<Arc<dyn InodeOp>>,        // None = 负目录项（缓存"文件不存在"）
    pub parent: Option<Arc<Dentry>>,            // 父目录（强引用）
    pub children: HashMap<String, Weak<Dentry>>,// 子目录（弱引用，打破循环）
    alias_path: Option<String>,                 // rename 后的新路径覆盖
}
```

parent 选择强引用（`Arc`）而 children 选择弱引用（`Weak`）的决策源于 `current_abs_path()` 的需求。完整路径通过沿 parent 链递归拼接分量名获得——如果 parent 是弱引用，每次向上追溯都可能因缓存淘汰而中断，需要从根目录重新走 namei 解析，这会引入不可预料的延迟。强引用保证了"只要子 dentry 存活，父 dentry 一定活着"的不变性，路径追溯永不失败。这是一个典型"以弱引用打破循环、以强引用保证路径可靠"的折中方案。

children 用 `Weak` 的代价是 `remove_dentry_cache_tree`（unlink/rename/umount 时清除子树缓存）只能通过 O(n) 全局前缀扫描实现——因为无法沿弱引用链可靠地遍历所有子孙。Linux 的做法是双向使用手动引用计数（C 语言的灵活性），在删除 dentry 时按特定顺序减计数来手动断环。Rust 的安全模型不允许这种操作，因此 Weak 成为唯一的无 unsafe 解法。

`alias_path` 字段是路径模型选择"全路径存储"而非"分量名存储"的直接后果。Linux 的每个 dentry 只记录自己的分量名（`d_name: qstr`），rename 时只需修改目标 dentry 的 `d_name` 和 `d_parent`。RespOS 的 dentry 记录完整绝对路径（`abs_path: String`），rename 后旧路径失效，alias_path 作为"覆盖值"记录新路径。`current_abs_path()` 优先读取 alias_path，并将分量名沿 parent 链递归拼接——后代 dentry 无需单独更新，通过继承父 dentry 的新路径前缀而自动修正。

### 7.1.3 后端实现矩阵

| 后端 | 实现文件 | 覆盖的 InodeOp 方法 | 特点 |
|------|---------|-------------------|------|
| ext4 | `os/src/fs/ext4/inode.rs` (1019 行) | 几乎所有方法 | path-based：通过 lwext4_rust C FFI 访问真实 ext4 磁盘 |
| proc | `os/src/fs/proc/` (dirs.rs 2032 行) | read_at, readdir, stat | 动态生成：/proc/cpuinfo/meminfo/stat 等从内核全局计数器拼接文本 |
| dev | `os/src/fs/dev/` (mod.rs 366 行) | read_at, write_at | 字符/块设备：null/zero/random/tty/shm |
| pipe | `os/src/fs/pipe.rs` (598 行) | 不实现 InodeOp | 仅通过 FileOp trait 暴露读写接口，数据在环形缓冲区而非 inode |
| special | `os/src/fs/special.rs` (271 行) | read_at, write_at | tmpfile / memfd：匿名文件，无磁盘持久化 |

ext4 后端的 path-based 特性值得关注——所有 inode 操作（read_at、write_at、lookup、create）都需要传递完整的文件路径字符串。这是因为底层 lwext4_rust 通过路径而非 inode 号在磁盘上定位文件。这一限制导致硬链接的实现需要额外的 dentry 层协调：同一个 inode 在两条不同路径下被访问时，必须确保 dentry 层正确跟踪了路径别名。

### 7.1.4 open 完整调用链

以 `open("/mnt/ext4/foo", O_RDWR | O_CREAT)` 为例，一次文件打开共经历六个阶段：

1. **系统调用入口**：`sys_openat`（`syscall/fs.rs:1142`）验证 flags 合法性（access mode 互斥、未知 flag 拒绝），从用户态拷贝路径字符串到内核堆
2. **路径解析**：`path_open` → `Nameidata::new_at` 构造路径游标 → `link_path_walk` 逐分量 walk → 每步 `lookup_dentry` 查 dentry 缓存或调 `inode.lookup(name)`
3. **文件创建**（O_CREAT）：最后一级分量不存在时，`open_last_lookups` 调 `filename_create` → `inode.create(parent_path, name, type)`
4. **File 对象构造**：`File::new(path, inode, flags)` — 调用 `inode.get_page_cache()` 获取共享页缓存；O_TRUNC 时调用 `inode.truncate(0)` + `pc.resize(0)`；O_APPEND 时初始化 offse̲t = stat.size
5. **FdEntry 分配**：`task.alloc_fd(FdEntry::new(file, open_flags))` 在进程 fd 表中找空位，返回 `int fd`
6. **用户态返回**：fd 整数传回用户程序，后续 read/write/close 通过 fd 索引到 FdEntry

### 7.1.5 设计边界

ext4 后端的所有 inode 操作都依赖路径字符串，而非通过 inode 号的直接索引。这限制了硬链接的完整性——同一个 inode 出现在两条不同的路径下时，VFS 层的 dentry 必须承担路径别名的维护工作。SuperBlockOp trait 当前仅包含 `root_inode()`、`sync()`、`statfs()` 三个方法，不包含 Linux 超级块所负责的 inode 分配与回收回调（`alloc_inode`/`evict_inode`）。这是因为当前 ext4 的 inode 生命周期由 lwext4_rust C 库在内部管理，VFS 层无需介入。proc 和 dev 文件系统共享同一个 ext4 磁盘镜像，它们的 InodeOp 实现通过内存中的动态生成逻辑模拟文件内容，不产生磁盘 I/O。

---

## 7.2 挂载与文件系统实例

### 7.2.1 VfsMount 与 Mount：挂载语义的双对象分离

挂载操作涉及两个正交语义——"被挂载的文件系统是什么"和"这个文件系统挂在树上的哪个位置"。RespOS 通过 VfsMount 和 Mount 两个独立结构实现这种分离。这一设计直接类比 Linux 的 `struct vfsmount` 和 `struct mount`，在 Rust 的 Arc/Weak 所有权限制下实现了 mount namespace 的基本语义。

```rust
// mount.rs:51 — "什么文件系统被挂载"
pub struct VfsMount {
    pub root: Arc<Dentry>,              // 该文件系统的根 dentry
    pub fs: Arc<dyn SuperBlockOp>,      // 超级块（根 inode 句柄 + sync 回调 + statfs）
    flags: AtomicI32,                   // MS_RDONLY / MS_NOATIME / MS_NOSYMFOLLOW ...
}

// mount.rs:60 — "这个文件系统挂在何处"
pub struct Mount {
    pub mountpoint: Arc<Dentry>,        // 挂载点目录（属于父文件系统）
    pub vfs_mount: Arc<VfsMount>,       // 指向被挂载的文件系统
    pub parent: Option<Weak<Mount>>,    // 父 Mount 节点（弱引用，打破循环）
    pub children: Mutex<Vec<Arc<Mount>>>, // 子 Mount 列表
    backing_root: Option<MountBackingRoot>, // 块设备挂载的容量隔离目录
}
```

VfsMount 解决"这是个什么文件系统"——通过 `root` 字段提供文件系统内路径解析的起点，通过 `fs` 字段提供超级块操作（获取根 inode、刷缓存、查询文件系统统计）。Mount 解决"挂在哪"——通过 `mountpoint` 记录父文件系统中被覆盖的目录，通过 `parent`/`children` 形成一棵挂载树，使得 `..` 穿越和挂载卸载能沿树结构安全遍历。

全局挂载树 `MOUNT_TREE` 用 `Vec<Mount>` 扁平存储所有 Mount 节点，同时两套查找接口服务于不同场景：

- `get_mount_by_dentry(dentry)`：O(n) 线性扫描，指针相等比较。路径解析的每一步都调用它——"当前 dentry 是挂载点吗？" 如果是，Nameidata 的 mnt 和 dentry 同时切换到被挂载文件系统的根
- `get_mount_by_vfsmount(vfs_mount)`：O(n) 线性扫描。umount 和 readdir 修补 `..` inode 时使用——"这个文件系统挂在哪个目录下？"

Mount 的 parent 使用 `Weak` 而 children 使用 `Arc` 的引用方向与 Dentry 相反。Dentry 的 parent 是强引用以支持可靠的向上路径追溯，而 Mount 的 parent 是弱引用以避免挂载树形成循环引用环。这一差异的根源在于两种树结构的访问模式不同——Dentry 的访问从子向父（`current_abs_path()`），Mount 的遍历从父向子（umount 递归清理子树）。

### 7.2.2 do_mount 六路分发

`do_mount`（`mount.rs:269`）是 mount 系统调用的后端入口，按六种语义分支处理：

| 分支 | 用户态命令 | 操作 |
|------|-----------|------|
| `MS_REMOUNT` | `mount -o remount,ro /mnt` | 仅修改已有挂载点的 flags（当前拒绝 MS_RDONLY 切换） |
| `MS_PRIVATE` | `mount --make-private` | no-op：挂载传播尚未实现 |
| `MS_MOVE` | `mount --move /old /new` | 旧 Mount 从树中摘除，新 Mount 指向同一 VfsMount 挂入新位置 |
| `MS_BIND` | `mount --bind /src /dst` | 新 VfsMount.root 指向 source 的 dentry，共享同一 SuperBlock |
| ext4 块设备 | `mount -t ext4 /dev/loop0 /mnt` | 创建 backing_root（带容量限制）→ 标准挂载 |
| tmpfs | `mount -t tmpfs none /tmp` | 复用 ext4 SuperBlock，无容量限制 |

所有分支共享统一的后续流程：① 解析 target 路径（确认类型为目录、未被其他文件系统占用）→ ② 创建 `VfsMount` 或读取已有实例 → ③ 创建 `Mount` 节点并加入全局挂载树 → ④ `remove_dentry_cache_descendants` 清除目标路径下的 dentry 缓存（旧内容被新文件系统遮盖，缓存的 dentry 指向已不可见的 inode）。

### 7.2.3 挂载穿越与 `..` 跨边界修补

路径解析每步的挂载穿越逻辑嵌入在 `lookup_dentry_maybe_follow_mount`（`namei.rs:110`）中：

```rust
if let Some(mount) = get_mount_by_dentry(&child_dentry) {
    nd.mnt = mount.vfs_mount.clone();         // 切换当前文件系统
    nd.dentry = mount.vfs_mount.root.clone();  // 进入被挂载文件系统的根
}
```

穿越后所有剩余的路径分量都在新文件系统内部解析。`path_global_abs_path`（`mount.rs:234`）执行反向操作——递归沿 Mount parent 链向上，将文件系统内部路径（如 ext4#2 的 `/foo`）拼接到全局命名空间（`/mnt/ext4/foo`）。这一递归穿越的复杂度是"有多个文件系统命名空间"本身的固有复杂性，与具体实现无关——Linux 的 `d_path` 同样需要沿 mount 树递归。

被挂载文件系统的根目录面临一个 `..` 语义的二义性问题。对 ext4#2 自身而言，根目录的 `..` 就是根本身（ext4 实现中 `..` == `.`）。但在全局命名空间中，`..` 应指向父文件系统中的挂载点目录。`follow_dotdot`（`namei.rs:149`）在 Nameidata 的当前 dentry 等于所在文件系统根时，沿 `Mount.parent` 链退回父 Mount 的挂载点，而非沿 Dentry.parent 链（那只会停在文件系统内部根上）。`readdir_uncached`（`file.rs:290`）额外修补了 `..` 的 inode 号——当列出目录的 dentry 等于 `path.mnt.root` 时，从父 Mount 的挂载点目录读取正确的 inode 号覆盖 ext4 返回的值。

### 7.2.4 当前限制与单 ext4 实例

内核当前仅有一个 ext4 超块实例（`SUPER_BLOCK` lazy_static，`ext4/mod.rs:14`），所有挂载（包括 procfs、devfs、tmpfs 的"虚拟"挂载）最终共享同一个磁盘镜像上的 ext4 目录树。块设备挂载通过 `MountBackingRoot` 机制实现隔离——在真实 ext4 根下创建隐藏目录 `.respos_mount_N` 作为每个 mount 的文件系统根，`check_mount_file_growth` 递归统计该隐藏目录下的总文件大小来计算磁盘空间使用率。

这一设计是"单磁盘镜像"选择的直接后果。Linux 中每个块设备文件系统拥有独立的超级块和独立的块设备，数据天然隔离。RespOS 的 `MountBackingRoot` 本质上是在单个 ext4 内部模拟多文件系统的隔离——这种妥协在比赛环境中足够工作，不需要额外的磁盘镜像支持。如果需要真正支持多 ext4 实例，需要从全局单例 `SUPER_BLOCK` 迁移到按需创建 `Ext4SuperBlock`，并为 procfs、devfs、tmpfs 实现独立的 `SuperBlockOp` trait 实现，从而消除 MountBackingRoot 机制。

---

## 7.3 路径解析（namei）

### 7.3.1 Nameidata：路径解析的状态游标

namei（Name Index）是将用户提供的路径字符串逐层解析为 `Path { mnt, dentry }` 的完整引擎。其核心结构 `Nameidata`（`namei.rs:38`）是解析过程中的状态游标——记录当前所在的文件系统、目录项，以及尚未解析的路径分量列表：

```rust
pub struct Nameidata {
    pub mnt: Arc<VfsMount>,           // 当前文件系统实例
    pub dentry: Arc<Dentry>,          // 当前所在的目录 dentry
    path_segments: Vec<String>,       // 路径字符串的 '/' split 分量
    depth: usize,                     // 已解析到第几个分量
}
```

Nameidata 的构造（`new_from_path`）完成了两件事：将路径字符串按 `/` 分割为分量向量，以及确定解析的起点——绝对路径从进程的 root 开始（chroot 边界），相对路径从 dirfd 指向的目录或当前工作目录开始。

### 7.3.2 link_path_walk：逐分量解析主循环

```rust
// namei.rs:1136
pub fn link_path_walk(nd: &mut Nameidata) -> SysResult {
    while nd.depth < nd.path_segments.len() - 1 {
        match name {
            "."  => skip,
            ".." => follow_dotdot(nd),    // 可能跨挂载边界退回父文件系统
            _    => {
                lookup_dentry(nd)?;        // 查缓存 → miss → inode.lookup
                if symlink { read_link → restart; }  // 递归展开
            }
        }
        nd.depth += 1;
    }
}
```

循环的最外层有一个 `'restart` 标签——这是一个重要的设计选择。当解析过程遇到符号链接展开时，剩余分量需要和链接目标拼接成全新的路径，解析必须从新的起点重新开始。使用 `continue 'restart` 而非递归调用避免了深层目录嵌套下的栈溢出风险，同时自然复用了 symlink 跟随计数（`MAX_SYMLINK_FOLLOWS = 40`）的深度限制。

`lookup_dentry` 的缓存命中路径和缺失路径形成了两个不同的开销剖面。缓存命中时仅需一次全局锁获取（`DENTRY_CACHE.lock()`），然后 O(1) HashMap 查找，再加上一次挂载点检查（`get_mount_by_dentry` 的 O(n) 线性扫描——n 为当前挂载点数）。缓存缺失时额外触发 `inode.lookup(name)`（对 ext4 而言是一次 lwext4 C FFI 调用，开销远大于内存操作），然后创建新的 `Dentry(abs_path, parent, inode)`、插入 parent.children 表和全局缓存、触发 LRU 回收检查。

### 7.3.3 符号链接展开

符号链接的展开是一个递归过程，但通过 `link_path_walk` 的循环 restart 实现了迭代式的执行：

1. `inode.read_link("link")` 读回目标路径字符串（如 `"/mnt/ext4/foo"`）
2. `join_symlink_target(target, remaining_segments)` 将目标路径和分量向量剩余部分拼接
3. 绝对目标从进程根重新开始 → 相对目标从 symlink 所在目录继续
4. 重新构造 Nameidata → `continue 'restart` 重新进入主循环

symbolic link 跟随计数 `symlink_follows` 在每次 restart 时递增，超 40 次返回 `ELOOP`。这一设计和 Linux 的行为完全一致——`link_path_walk` 中的 `nd->depth` 和 `total_link_count` 防止符号链接循环或过深递归导致的栈溢出。

### 7.3.4 AT_* 标志位的精细化控制

RespOS 实现了四个 `AT_*` 标志位，对标 Linux 的 `*at` 系统调用族：

| 常量 | 值 | 语义 | 影响的系统调用 |
|------|-----|------|--------------|
| `AT_FDCWD` | -100 | 相对路径从当前工作目录开始 | 所有 openat/statat/unlinkat 等 |
| `AT_SYMLINK_NOFOLLOW` | 0x100 | 最后一级分量不展开符号链接 | lstat, readlink, O_NOFOLLOW open |
| `AT_EMPTY_PATH` | 0x1000 | 空路径操作 dirfd 自身 | fstatat, linkat + AT_EMPTY_PATH |
| `AT_NO_AUTOMOUNT` | 0x800 | 不触发 autofs 自动挂载 | 当前仅纳入白名单，内核不实现 autofs |

### 7.3.5 全路径存储的设计取舍

路径模型的根本设计选择——存储全路径 `abs_path` 还是分量名 `d_name`——在 RespOS 的代码库中留下了连锁痕迹。

**选择全路径**的动机是简化路径拼接：`child_abs_path = parent.current_abs_path() + "/" + name`，而不需要像 Linux 那样从根目录逐层向上遍历收集分量名。这一选择适用于 dentry 缓存命中率高时——路径一旦构造完成就缓存在 `abs_path` 中，后续读取无需重新计算。

**但这一选择引入了三个伴随代价**。第一，rename 操作导致旧路径失效，需要 `alias_path` 作为覆盖值，`relocate()` 方法在 rename 完成后更新目标 dentry 的 `alias_path` 和 parent 指针。第二，dentry 缓存中的 parent 必须使用强引用（`Arc`）以保证向上追溯路径的可靠性——`Weak` 可能在缓存淘汰时失效，导致 `current_abs_path()` 的递归追溯中断。第三，parent 的强引用迫使 children 使用弱引用（`Weak`）以打破引用循环，而弱引用链条使 `remove_dentry_cache_tree` 无法从父目录递归遍历子孙——只能退化为 O(n) 全局前缀扫描。

这些代价共同指向一个结论：全路径存储在 rename 不频繁的系统中是简洁高效的，但在生产级系统中，Linux 的分量名模型通过消除 alias_path、支持双向遍历（parent 和 children 都是有效的访问路径）、将路径拼接推迟到真正需要时才计算，获得了更好的整体性能和维护性。

### 7.3.6 并发控制

全局 `NAMEI_MUTATION_LOCK`（`namei.rs:29`）将 create、unlink、rename、symlink 等修改路径命名空间的操作完全串行化。这一设计简洁且无死锁风险——所有修改操作共享一个互斥锁，避免了多锁排序的复杂性——但代价是写密集场景下的多核并行度完全丧失。在 buildstorm 等并发创建/删除文件的负载下，多个 CPU 核心上的进程实际上在互斥锁上排队，多核优势被抵消。

---

## 7.4 ext4 后端与页缓存

### 7.4.1 ext4 后端：C 库封装与内核适配

RespOS 通过 Rust FFI 绑定 C 库 `lwext4` 实现 ext4 文件系统的读写。这一架构选择意味着内核的文件系统层是一个适配层——将 VFS 的 `InodeOp` trait 方法翻译为 lwext4 的 C 函数调用——而非直接操作 ext4 磁盘数据结构的原生实现。

```rust
// ext4/super_block.rs:17
pub struct Ext4SuperBlock {
    inner: Mutex<Option<Ext4BlockWrapper<Disk>>>,  // lwext4 块设备实例封装
    root: Arc<dyn InodeOp>,                         // 根 inode (ino=2)
}
```

`Ext4SuperBlock` 在构造时创建 `Ext4BlockWrapper<Disk>` 并调用 lwext4 的 `ext4_mount` 初始化块设备。其三个核心操作映射了超级块的最小接口：`root_inode()` 返回缓存的 ino=2（ext4 固定根 inode 号）；`sync()` 调用 `ext4_cache_flush` 将 lwext4 内部缓冲区刷到块设备；`statfs()` 调用 `ext4_mount_point_stats` 获取块计数、inode 计数、空闲空间等文件系统统计信息，并以 `STATFS64` 格式返回。

`Ext4Inode` 在 lwext4 基础上维护了额外的内核状态层，覆盖了 lwext4 不直接管理的语义：

- `open_files: AtomicUsize`：文件打开计数。每次 `File::new` 时递增（`open_file()`），File drop 时递减（`close_file()`），用于判断 unlink 时是否需要转为孤儿文件
- `nlink_override: Mutex<Option<u32>>`：inode 链接数的内存覆盖值。unlink 后设为 0（告知 stat 该文件已无链接），但被打开的 fd 仍然可以读写
- `orphan_path: Mutex<Option<String>>`：孤儿文件的磁盘路径。unlink 时调用 `orphan_regular_file` 将文件 rename 为 `.respos_orphan_<ino>` 隐藏名，最后一个 fd 关闭时 `cleanup_orphan` 真正删除

这个"孤儿文件 = rename 到隐藏名 + nlink_override 模拟"的机制是对 Linux 孤儿 inode 模型的模拟。Linux 通过 `i_nlink == 0 && i_count > 0` 的双计数器语义自然支持"已 unlink 但仍在使用的 inode"，而 lwext4 的 API 缺乏这一抽象——它只提供 `file_remove`（直接删除目录项和文件），不提供"仅减少 nlink 但保留 inode"的中间状态。RespOS 的 rename-to-hidden 策略在应用层实现了等价语义，但如果在异常关机（未等到 cleanup_orphan 执行）后重启，会留下 `.respos_orphan_*` 文件残留。

### 7.4.2 页缓存设计：按需加载与脏页标记

页缓存（`page_cache.rs`，489 行）是文件数据在内存中的缓冲区，其设计核心是在低 I/O 延迟和有限内存之间取得平衡。

```
Page（单页）：
  data: Vec<u8>（PAGE_SIZE 字节）     ← 注意：堆分配而非直接使用物理页框
  dirty: bool                         ← 被写过但未刷盘
  write_version: usize                ← sync 时的并发写检测版本号
  generation: usize                   ← 全局 LRU 代数

PageCache（per-inode）：
  pages: Mutex<BTreeMap<usize, Arc<Mutex<Page>>>>  ← 稀疏存储：只存实际访问过的页
  file_size: Mutex<usize>
  size_version: AtomicUsize           ← 文件大小被修改的次数（sync 用它检测并发 truncate）
  dirty_pages: AtomicUsize            ← 该缓存中脏页总数
```

页缓存的存储方式是**稀疏的**——一个 1GB 的文件只被访问了前两页时，BTreeMap 中只有两个 Page 对象。这种按需分配策略避免了在文件打开时全量分配缓存页的内存浪费。

四个操作构成了页缓存的完整生命周期：

**read_at**：逐页遍历 → 查 pages BTreeMap（先拿页锁、缓存命中则直接 memcpy）→ miss 时构造新 Page → 通过 `lower` 回调（`(inode, path)`）从 ext4 读入数据 → 插入 BTreeMap → touch_page 推入全局 LRU → `reclaim_global` 检查全局内存配额。

**write_at**：分两条路径处理——整页覆盖或写超出文件末尾的新页时直接分配零页（不需要磁盘 I/O 来补原数据），部分覆盖现有页时先调 `get_or_load` 读入旧数据再覆盖写入区域。写入后标记 `page.dirty = true` 并递增 `write_version`，但此时数据仍在内存中，未触及磁盘。

**sync**：fsync 或 O_SYNC 写触发的脏页写回。先快照脏页的 `write_version`（用于并发写检测），然后按连续脏页分组（每组最多 `WRITEBACK_BATCH_PAGES = 32` 页），对每组调用 `inode.write_at(path, offset, &data)` 批量写回。写回后双重检查：`size_version` 是否变化（并发 truncate/extend 修改了文件长度）→ 若变了则恢复当前长度并跳过本组；`write_version` 是否与快照一致（并发写重新弄脏了页）→ 仅清理版本号未变的页的 dirty 标记。

**reclaim_global**：内存压力触发的全局回收。采用 generational LRU 算法——全局队列 `PAGE_CACHE_LRU` 记录所有页面的 (cache_id, page_idx, generation) 元组。回收时从队首弹出最老条目，比较 generation 与 Page 中记录的当前 generation：不匹配说明页面在此期间被重新访问过（已 touch 到队尾），跳过；匹配且 `page.dirty == false` 且 `Arc::strong_count == 2`（仅 BTreeMap 和 LRU 持有，无外部引用）才真正从 BTreeMap 中删除。脏页永远不会被 LRU 回收——脏数据的释放机制是 `sync` 而非回收。

### 7.4.3 页缓存的 Vec<u8> 与物理页框

Page 的数据存储使用 `Vec<u8>`（堆分配）而非直接使用内核的物理页框（frame_allocator 分配的 4KB 物理页）。这一选择的根源是 lwext4_rust C 库有自己的内部缓冲区管理，读写路径绕过了内核的物理页管理。

这一架构带来了每 I/O 路径多一次内存拷贝的代价：read 路径的数据流为"lwext4 内部缓冲 → Page.data (Vec<u8>) → copy_to_user → 用户态 buf"（两次拷贝），而 Linux 原生 ext4 驱动将物理页框直接作为 page cache，数据流为"DMA → 物理页框 → copy_to_user → 用户态 buf"（一次拷贝）。mmap 路径的代价更为显著——需要从 Page.data 再拷贝到物理页框才能映射进用户页表，比 Linux 的零拷贝 mmap 多了两次拷贝。

这是使用外部 C 库而非原生 Rust 文件系统实现的选择带来的代价。在比赛环境的 I/O 量级下，多一次拷贝不是主要瓶颈；但这一设计限制了未来 mmap 性能的优化空间。

### 7.4.4 持久化写回的三层链

文件数据的持久化经过三级缓存，每一级都有独立的刷盘触发条件：

```
用户 write()
  → page_cache.write_at()           ← 内存标脏，不碰盘
    （脏页积累直到：O_SYNC/O_DSYNC 触发或 fsync 触发或内存压力）
  → page_cache.sync(inode, path)
      → inode.write_at(path, off, data)  ← 页缓存 → lwext4 内部写缓冲区
        （数据在 ext4 内部，但可能仍被 lwext4 缓冲）
  → superblock.sync()
      → ext4_cache_flush()               ← lwext4 缓冲区 → virtio 块设备
        （数据真正到达磁盘镜像）
```

普通 `write()` 停留在第一级。`O_DSYNC`/`O_SYNC` 写推至第二级。只有显式 `fsync`、`File::drop`（关闭最后一个 fd 的引用）或 `sync` 系统调用才将三级全部打穿。`File::write_back` 布尔标志控制 File 是否负责写回——tmpfile（无持久化语义）设为 false，ext4 普通文件设为 true。这一标志在 `read_at` 中也作为 `lower` 回调的开关：write_back=true 时页缓存 miss 从 ext4 读入数据，false 时保持零页。

### 7.4.5 块设备接口

`Ext4BlockWrapper<Disk>` 通过 lwext4 的 `ext4_blockdev_iface` 回调表连接内核的 virtio 块设备驱动。五个回调（`dev_open`、`dev_read`、`dev_write`、`dev_close`、`dev_seek`）由 RespOS 的 `Disk` 实现，对接 `BlockDeviceImpl`（`drivers/virtio/block_dev.rs`）的 virtio-blk 协议。ext4 磁盘操作完全透明于上层 VFS——路径解析和 inode 操作都通过路径字符串路由，ext4 内部维护路径到磁盘块地址的完整映射（inode 表、间接块、extent 索引），这些复杂性由 lwext4 C 库封装。
