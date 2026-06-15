# LTP 文件系统与 Linux ABI 兼容层设计草案

本文记录 RespOS 为通过 oscamp LTP 测例时，文件系统、路径解析和 Linux ABI 兼容层应采用的长期设计方向。当前实现中已有一些为了快速通过测例的兜底逻辑，例如 `fchmodat`、`fchownat`、`mkdirat`、ext4 create 后的 synthetic inode，以及部分 lwext4 资源释放修复。这些逻辑可以作为阶段性方案保留，但后续应逐步收束到清晰、可审查、可替换的抽象中。

## 当前问题

LTP 测例不只是直接测试某个 syscall。许多用例在真正执行测试前，会由 glibc、LTP harness、shell、busybox 和动态加载器完成一组初始化动作，包括创建临时目录、调整权限、修改属主、写结果文件、读取 `/proc` 或检查环境能力。因此，一个看似简单的 `getpid01` 也可能依赖 `mkdirat`、`fchmodat`、`fchownat`、`stat`、`openat`、`unlink` 等文件系统语义。

当前比较脆弱的点主要有：

1. syscall 层容易混入测例特化逻辑，例如根据路径前缀跳过真实 chmod。
2. ext4 元数据语义不完整，mode、uid、gid、ctime 等字段没有统一的缓存和覆盖规则。
3. lwext4 在 create 后立刻重新 open 或 lookup 新对象时可能阻塞，导致 VFS 无法稳定拿到真实 inode。
4. synthetic inode 能解决阶段性卡死，但可能带来 inode number、page cache、dentry cache 一致性问题。
5. `fchownat` 等 syscall 当前处于单用户 root 模型，和 Linux 完整权限语义差距较大。

这些问题不适合继续通过零散补丁解决。更稳的方向是把 Linux ABI 语义、VFS 路径语义、文件系统后端语义分层。

## 设计原则

### syscall 层保持薄

syscall 层只负责 Linux ABI 入口处必须处理的事情：

- 从用户空间复制参数；
- 校验 flags；
- 处理 `dirfd`、空路径、`AT_EMPTY_PATH`、`AT_SYMLINK_NOFOLLOW` 等路径解析选项；
- 调用 VFS 或 inode 操作；
- 返回 Linux errno。

syscall 层不应该直接包含 ext4 特例、LTP 路径前缀、临时目录特判或 lwext4 资源释放细节。

理想调用关系如下：

```text
sys_fchmodat
  -> resolve_path_at(dirfd, path, lookup_flags)
  -> inode.set_mode(abs_path, mode)

sys_fchownat
  -> resolve_path_at(dirfd, path, lookup_flags)
  -> inode.set_owner(abs_path, uid, gid)

sys_mkdirat
  -> filename_create(dirfd, path, Directory, mode)
  -> inode.create(parent_path, name, Directory)
  -> inode.set_mode(child_path, mode)
```

### VFS 层负责路径和 dentry 一致性

`namei`/VFS 层应该统一处理：

- `dirfd` 相对路径；
- `.`、`..`；
- symlink follow/no-follow；
- mount crossing；
- dentry cache 命中和失效；
- create、unlink、rename 后的 dentry 树更新；
- open file 与 path/dentry/inode 的生命周期关系。

文件系统后端不应该知道 syscall 的 flags，也不应该参与复杂路径解析。后端只接收已解析出的父目录、名字、绝对路径和操作类型。

### ext4 后端负责底层存储语义

ext4 后端应该负责：

- 与 lwext4 交互；
- 维护真实 inode cache；
- 管理 page cache；
- 将 chmod、chown、utimensat、stat 等元数据操作映射到底层；
- 在底层能力不足时提供集中、显式的兼容兜底。

底层兼容兜底必须集中放置，使用清晰命名和注释，例如：

```rust
// TODO[LWEXT4]: ...
fn synthetic_created_inode(...)

// TODO[ABI-COMPAT]: ...
fn set_cached_mode(...)
```

## 元数据缓存设计

当前 `Ext4Inode` 中已经有时间缓存和 mode override，但长期应收敛为统一结构：

```rust
struct InodeMetaOverride {
    mode: Option<u32>,
    uid: Option<u32>,
    gid: Option<u32>,
    atime: Option<TimeSpec>,
    mtime: Option<TimeSpec>,
    ctime: Option<TimeSpec>,
}
```

推荐规则：

1. `stat()` 优先读取底层 ext4 真实元数据。
2. 如果存在 override，则用 override 覆盖底层结果中的对应字段。
3. `chmod` 更新 mode override，并更新 ctime。
4. `chown` 更新 uid/gid override，并更新 ctime。
5. `utimensat` 更新 atime/mtime override，并更新 ctime。
6. 当底层 ext4 写回能力补齐后，可以把 override 逐步改为 write-through。

这样可以避免 `mode_override`、`times`、uid/gid 语义分散在不同位置，也便于后续实现权限检查。

## create 后 inode 获取设计

长期目标应是：

```text
create object in lwext4
  -> close all temporary handles
  -> lookup new child
  -> get real inode number
  -> install real inode into dentry cache
```

当前如果 create 后立刻重新 open/lookup 会触发 lwext4 阻塞，可以保留 synthetic inode 作为阶段性方案，但需要遵守以下约束：

1. synthetic inode 只用于“create 已成功，但新对象立即 lookup 不可靠”的路径。
2. 普通路径 lookup 必须继续走真实 ext4 inode cache。
3. synthetic inode 必须安装进新 dentry，确保同一路径的连续 open/read/write 在 dentry 生命周期内一致。
4. synthetic inode 的 page cache 风险要被明确标注，后续应通过真实 inode number 或 path-backed cache 替代。
5. unlink/rename 时需要同步清理 dentry cache，避免旧 synthetic inode 残留。

理想修复方向不是永久保留 synthetic inode，而是定位并修复 lwext4 的句柄释放、锁、目录项缓存或 open 状态问题。

## chown/chmod 权限语义

当前内核是单用户 root 模型，可以先接受 `fchownat` 路径存在即成功，但要明确这是兼容层行为。

后续语义补全顺序建议：

1. `fchownat` 支持 `AT_EMPTY_PATH` 和 `AT_SYMLINK_NOFOLLOW` 的路径规则。
2. inode 元数据中记录 uid/gid override。
3. `stat` 返回 uid/gid。
4. `chmod` 正确维护 mode 与 ctime。
5. 引入 umask，影响 `open(O_CREAT)` 和 `mkdirat` 初始 mode。
6. 实现基础权限检查：owner/group/other 的 read/write/execute。
7. 再考虑 Linux 更复杂的 capability、sticky bit、S_ISGID 清理、ACL 等规则。

两周竞赛目标下，不建议一开始就追求完整 Linux 权限模型。优先保证 LTP 初始化、临时目录、结果文件、普通 open/stat/chmod/chown 行为稳定。

## TODO 标记分类

建议后续代码中把临时兼容点分成三类：

```text
TODO[ABI-COMPAT]
Linux ABI 语义未完整实现，例如 uid/gid、权限检查、flags 细节。

TODO[LWEXT4]
底层 lwext4 行为异常或能力不足，例如 create 后立即 lookup 阻塞、句柄释放问题。

TODO[VFS-CONSISTENCY]
dentry、inode、page cache、rename/unlink 生命周期存在一致性欠账。
```

这样审查时可以快速判断问题属于 Linux 语义、底层文件系统 bug，还是 VFS 结构问题。

## 推荐重构路线

### 第一阶段：稳定 oscamp LTP 目标列表

目标是让当前优先级测例稳定运行，允许保留少量封装清楚的兼容兜底。

建议要求：

- 所有兜底必须集中在 syscall、VFS、ext4 后端的合理层次；
- 不在 syscall 层写 LTP 路径特判；
- 所有临时行为必须带 TODO 分类；
- 每次改动后至少运行 `cargo check`、LoongArch `cargo check` 和目标架构 LTP 子集。

### 第二阶段：收束高风险兜底

优先处理会污染后续测例的语义：

- create 后 synthetic inode；
- chmod/chown/stat 元数据一致性；
- unlink/rename 后 dentry cache 清理；
- open 后写入、重新 open 后读取的 page cache 一致性；
- symlink follow/no-follow。

这一阶段不一定扩展更多测例，而是减少“当前能过但以后会反噬”的实现。

### 第三阶段：补齐 Linux ABI 细节

重点补：

- `AT_EMPTY_PATH`；
- `AT_SYMLINK_NOFOLLOW`；
- `O_CREAT` + symlink；
- `umask`；
- uid/gid；
- mode type bits 与 permission bits；
- `/proc` 能力探测文件；
- errno 精确性。

LTP 的很多失败并不一定是目标 syscall 本身，而是初始化过程中的能力探测和 errno 不匹配。

## 最小回归用例建议

建议后续在内核或用户态补一些小型回归用例，不依赖完整 LTP：

1. `mkdir -> chmod -> stat`：确认目录 mode 可见。
2. `open(O_CREAT) -> write -> close -> open -> read`：确认新建文件数据一致。
3. `open(O_CREAT) -> chmod -> stat`：确认文件 mode 可见。
4. `mkdir -> chown -> stat`：确认 uid/gid 兜底或 override 行为明确。
5. `create -> unlink -> create same name`：确认 dentry cache 不残留。
6. `rename old new -> open new -> open old`：确认 rename 后路径状态正确。
7. `symlink -> fchmodat(..., AT_SYMLINK_NOFOLLOW)`：确认 no-follow 语义不误伤目标文件。

这些小用例比直接跑完整 LTP 更适合定位语义回归。

## 当前阶段的判断

现在的实现方向可以继续推进，但必须承认它仍是基础兼容层，不是完整 Linux 文件系统语义。比较合理的工程策略是：

1. 先让 oscamp 目标列表稳定通过；
2. 把所有临时语义集中和标注；
3. 每通过一批测例后审查一次 VFS/ext4 语义债务；
4. 优先修复 create、metadata、dentry cache、rename/unlink 这些会影响大量测例的基础行为。

不要把每个 LTP 失败都当成独立 syscall bug。很多失败实际是 VFS、libc 初始化路径、临时目录、权限元数据、动态链接器环境共同作用的结果。
