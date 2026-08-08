# 4. 进程与线程管理

## 4.1 Task、Process、Thread 与线程组身份

【本节目的】说明 RespOS 中 TCB、tid/tgid、父子关系、线程组和共享资源的实际关系。

【建议写什么】回答对象由谁创建、谁拥有、谁共享、谁修改、谁销毁；区分进程地址空间、FdTable、内核栈、signal/futex 状态和线程组成员。

【建议检查的 RespOS 代码】`os/src/task/task.rs`；`manager.rs`；`tid.rs`；`kstack.rs`；`context.rs`；`os/src/syscall/process.rs`。

【建议查看的 Git 历史】`00c6822`、`3aa1fb5`、`17dcd4e`；`git log -S'TaskControlBlock'`。

【建议准备的图 / 表】Process—Thread—TCB—MemorySet—FdTable—kernel stack 关系图；所有权/共享矩阵。

【建议准备的测试 / 数据】`task_a_*_probe`、`smp_phase3_probe`、`CLONE_VM/CLONE_FILES` 专项（若最终存在）；记录 tid/tgid。

【容易出现的问题】不能用 `Arc::strong_count` 单独推断语义所有权；同组延迟回收 TCB 会影响引用计数观察。

## 4.2 创建与地址空间/文件描述符共享

【本节目的】解释 fork/clone 的共享与复制边界，以及失败时父子状态如何保持一致。

【建议写什么】分别覆盖 `CLONE_VM`、`CLONE_FILES`、`CLONE_VFORK`、普通 fork/clone；重点写 MemorySet、FdTable、CLOEXEC 和 vfork waiter 的生命周期。

【建议检查的 RespOS 代码】`os/src/syscall/process.rs::sys_clone`；`os/src/task/task.rs`；`os/src/fs/fdtable.rs`；`os/src/mm/memory_set.rs`。

【建议查看的 Git 历史】`cf30f64`、`17dcd4e`、`f326ac8`、`b785262`；关注 vfork 顺序与 fd-table 归属修复。

【建议准备的图 / 表】clone flags → 共享对象矩阵；父子创建时序图。

【建议准备的测试 / 数据】clone/fork/exec/wait、`CLONE_VFORK`、`CLONE_FILES` 和共享 MM probe；至少双架构跑可用子集。

【容易出现的问题】单核通过不能证明 vfork 发布顺序正确；child 可能在父 blocked 登记前运行，必须保留 SMP 证据。

## 4.3 exec 的映像替换与 sibling 线程处理

【本节目的】讲清 exec 前后地址空间、线程组和旧用户地址资源的安全顺序。

【建议写什么】围绕 argv/envp、ELF metadata、PT_LOAD backing、CLOEXEC、robust-list、clear-child-tid、sibling quiescence 和失败回滚填写。

【建议检查的 RespOS 代码】`os/src/task/task.rs::execve_file`、`close_other_threads_for_exec`；`os/src/mm/mod.rs`；`os/src/mm/memory_set.rs`。

【建议查看的 Git 历史】`f326ac8`、`b785262`；查看 `exec`、ARG_MAX 和 quiescence 相关 diff。

【建议准备的图 / 表】exec 四阶段时序；旧 MemorySet/新 MemorySet 与 sibling owner 状态图。

【建议准备的测试 / 数据】大 ELF、长 argv/envp、动态程序、vfork/posix_spawn、远端 sibling trace；未跑出的 BuildStorm 阶段标待验证。

【容易出现的问题】不能在新映像安装后再写旧地址；不能把“构建成功”与完整 BuildStorm 成功混为一谈。

## 4.4 exit、wait 与延迟回收

【本节目的】说明逻辑退出、父子通知、wait 回收、内核栈和 DEAD_TASKS 的不同时间点。

【建议写什么】区分 `exit`/`exit_group`、zombie/回收、waiter 唤醒、FdTable 清理、当前栈不能自释放、所有 CPU idle 时的 deferred cleanup。

【建议检查的 RespOS 代码】`os/src/task/task.rs`；`processor.rs`；`manager.rs`；`syscall/process.rs`；`fs/pipe.rs`。

【建议查看的 Git 历史】`57046f1`、`3aa1fb5`、`17dcd4e`、`f326ac8`、`b785262`。

【建议准备的图 / 表】退出—通知—回收时序；资源释放责任表；waiter 所在线程组关系图。

【建议准备的测试 / 数据】wait4 probe、并发 exit/wait、pipe EOF、所有 CPU idle 的 exit 压力；保存第一失败日志。

【容易出现的问题】不能只看 parent leader；同组实际 waiter 可能是其他 tid；延迟 TCB 仍可能持有旧资源。

