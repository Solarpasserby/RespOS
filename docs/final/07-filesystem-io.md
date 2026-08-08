# 7. 文件系统与 I/O

## 7.1 VFS 对象与路径解析

【本节目的】说明 Path、Mount、Dentry、Inode、File 的身份和 namei 的责任边界。

【建议写什么】用对象关系和一次 open/path walk 流程说明 `dirfd`、`.`/`..`、symlink、mount crossing、rename/unlink 约束；不把对象简单合并成“文件”。

【建议检查的 RespOS 代码】`os/src/fs/{path.rs,namei.rs,mount.rs,dentry_cache.rs}`；`os/src/fs/vfs/{dentry.rs,inode.rs,super_block.rs}`。

【建议查看的 Git 历史】`cba8e24`、`44430df`、`40d745a`；查看 namei/create/unlink 生命周期修复。

【建议准备的图 / 表】VFS 对象关系图；path walk 状态表；后端能力限制表。

【建议准备的测试 / 数据】basic FS、LTP namei/link/rename/unlink、CAgent fs-create；失败 errno 与镜像状态。

【容易出现的问题】不能用 open-file 引用数推断 inode 是否已删除；path-based ext4 后端限制要如实列出。

## 7.2 ext4、页缓存与挂载持久化

【本节目的】解释比赛镜像中的 ext4 后端、页缓存和关机写回边界。

【建议写什么】填写 ext4 superblock/inode、block device、page cache、mount/unmount、flush；区分内核实现与 vendor `lwext4_rust` 能力。

【建议检查的 RespOS 代码】`os/src/fs/ext4/`；`page_cache.rs`；`drivers/virtio/block_dev.rs`；`vendor/lwext4_rust/`；`scripts/get_img.sh`。

【建议查看的 Git 历史】`269a94a`、`8169793`、`cba8e24`；核对正常关机和镜像 journal 相关修复。

【建议准备的图 / 表】VFS→ext4→virtio block 调用图；guest 关机与 host fsck 时序。

【建议准备的测试 / 数据】正常 guest quit 后 `e2fsck -fn`；写入、fsync、重启前后文件内容和镜像 hash。

【容易出现的问题】QEMU 持有 raw 镜像时不能 host 写入；异常终止后的镜像不能作为干净基线。

## 7.3 FdTable、FdEntry 与 open-file description

【本节目的】说明 fd 槽位、descriptor flags、共享 open-file 状态和 dup/exec/clone/close 的生命周期。

【建议写什么】重点写 CLOEXEC 与 offset/status flags 的分层、`Arc<dyn FileOp>`、跨线程/跨进程共享、关闭最后引用与失败路径。

【建议检查的 RespOS 代码】`os/src/fs/{fdtable.rs,fdset.rs,file.rs}`；`os/src/syscall/{fs.rs,special_fd.rs,process.rs}`。

【建议查看的 Git 历史】`cba8e24`、`17dcd4e`、`b785262`；核对 CLONE_FILES 和退出清表判断。

【建议准备的图 / 表】FdTable slot→FdEntry→FileOp→File 对象图；dup/clone/exec/close 引用变化表。

【建议准备的测试 / 数据】dup/fcntl/CLOEXEC、CLONE_FILES、pipe EOF、Rust stdout/stderr 捕获。

【容易出现的问题】不能把 CLOEXEC 放进共享 File；不能用 TCB 总引用数直接判断跨进程共享。

## 7.4 pipe、poll/epoll 与 EOF 生命周期

【本节目的】把 pipe buffer、读写端、阻塞唤醒和退出/exec 引用释放讲清楚。

【建议写什么】写 pipe 创建、读写、poll notification、端点 drop、EOF、waiter 唤醒和 open-file 引用；把 BuildStorm 中的已验证边界与未验证 blocker 分开。

【建议检查的 RespOS 代码】`os/src/fs/{pipe.rs,poll.rs,file.rs}`；`os/src/syscall/special_fd.rs`；`os/src/task/task.rs`。

【建议查看的 Git 历史】`269a94a`、`cf30f64`、`b785262`；关注 pipe trace 仅为临时诊断证据。

【建议准备的图 / 表】pipe 端点/引用/EOF 状态图；poll→wake 时序。

【建议准备的测试 / 数据】`pipetest`、poll/epoll、Rust cargo 捕获输出、`[pipelifetrace]`；未出现 Pipe::drop 的样本标为待验证。

【容易出现的问题】子进程退出不等于所有写端立即释放；不要把某一轮日志中的强引用变化直接写成最终根因。

## 7.5 procfs、devfs 与特殊文件

【本节目的】说明比赛用户态如何观察 CPU、内存、进程和设备，以及这些接口如何接入真实内核状态。

【建议写什么】覆盖 proc cpuinfo/meminfo/stat/uptime/maps/smaps、dev null/zero/random/tty/shm、special fd；标明动态 CPU/RAM 输出来源。

【建议检查的 RespOS 代码】`os/src/fs/proc/`；`os/src/fs/dev/`；`os/src/fs/special.rs`；`os/src/syscall/system.rs`。

【建议查看的 Git 历史】`f326ac8`、`17dcd4e`、`40d745a`；核对 nproc、MemTotal 和 procfs 变化。

【建议准备的图 / 表】proc/dev 树；用户观察接口—内核数据源表。

【建议准备的测试 / 数据】`nproc`、`cat /proc/cpuinfo`、`/proc/meminfo`、health/shell smoke；RV64 2/8 核与 256M/8G 对照。

【容易出现的问题】固定常数不能冒充真实 online CPU/RAM；procfs 结果必须与 QEMU 参数和 FDT 证据一致。

## 7.6 socket、loopback 与网络 I/O

【本节目的】记录当前 smoltcp socket/loopback 实现及其对 CAgent 的实际影响。

【建议写什么】说明 socket FileOp、SocketSet、TCP/UDP、loopback device、listen table/backlog、accept queue 和 poll；解释连接失败如何区分内核 ABI 与 server 排队。

【建议检查的 RespOS 代码】`os/src/net/{mod.rs,listen.rs,socket.rs,tcp.rs,udp.rs,loopback.rs}`；`os/src/syscall/net.rs`。

【建议查看的 Git 历史】`5f77068`、`40d745a`、`269a94a`；核对并发 SYN/listener 池修复。

【建议准备的图 / 表】socket 生命周期；listen backlog→accept queue→userspace accept 时序。

【建议准备的测试 / 数据】`net_loopback_smoke`、CAgent 单项/三轮并发 TCP、UDP loopback、server 串行排队对照。

【容易出现的问题】Connection refused 可能是 listener ready 时序或测试 server 串行处理；不能只凭 CAgent reject 修改固定 syscall。

