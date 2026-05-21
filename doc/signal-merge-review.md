# feature/signal 合并后检查记录

本次检查基于 `feature/signal` 合并 `main` 后的当前工作区。整体结论是：`main` 分支合入部分没有看到明显的编译级冲突残留，主要风险集中在信号机制的运行时语义。

已执行的基础验证：

```bash
cargo check # in os/
cargo check # in user/
```

两边均可以通过编译检查；仓库中也没有发现 `<<<<<<<` / `=======` / `>>>>>>>` 这类冲突标记。

## 主要问题

### P0：会直接影响信号正确性

1. `SIGKILL` 目前不会真正终止任务。

   位置：`os/src/task/mod.rs`

   `call_kernel_signal_handler` 对多数内核信号只设置 `task_inner.killed = true`，但后续 `handle_signals` 看到 `killed` 后只是跳出循环，并没有调用 `exit_current_and_run_next` 或其他退出路径。因此 `kill(pid, SIGKILL)` 很可能只是标记任务，然后继续返回用户态。

   建议：在信号处理流程中让不可捕获的终止类信号进入明确的退出路径，并设置合理退出码。

2. 默认 signal action 没有清除 pending signal，也没有执行默认动作。

   位置：`os/src/task/mod.rs`

   `call_user_signal_handler` 在 `handler == 0` 时只打印日志，没有清除 `signals` 中对应的 pending bit，也没有按默认语义终止或忽略。像 `SIGTERM` 这种没有用户 handler 的信号会一直留在 pending 集合里，之后每次 trap 都可能重复处理。

   建议：为默认 action 建立明确策略。至少需要区分“默认忽略”和“默认终止”，并在处理后清除对应 pending bit。

### P1：边界和 ABI 细节

3. `sigaction` 拒绝了 31 号信号。

   位置：`os/src/syscall/process.rs`

   当前判断为：

   ```rust
   if signum < 0 || signum as usize >= MAX_SIG
   ```

   但 `MAX_SIG = 31`，信号表长度也是 `MAX_SIG + 1`，并且 `SIGSYS = 31` 已定义。这里会导致 31 号信号不可注册。

   建议：改为 `signum as usize > MAX_SIG`。

4. `sigaction` 强制 `action` 和 `old_action` 都非空。

   位置：`os/src/syscall/process.rs`

   `check_sigaction_error` 现在把 `action == 0` 或 `old_action == 0` 都当成错误。但用户态封装已经设计成：

   ```rust
   sigaction(signum, Option<&SignalAction>, Option<&mut SignalAction>)
   ```

   正常语义中“只设置新 action”或“只读取旧 action”都应该允许。当前实现会让这些用法返回 `EINVAL`。

   建议：只在需要读写用户指针时检查对应指针；空指针本身不应直接作为错误。

5. `kill` 对 `signum` 的边界检查太晚。

   位置：`os/src/syscall/process.rs`

   当前实现先做：

   ```rust
   SignalFlags::from_bits(1 << signum)
   ```

   如果 `signum` 是负数或超出范围，存在 shift panic 或未定义预期的问题。

   建议：先判断 `signum < 0 || signum as usize > MAX_SIG`，再计算 bit。

6. 用户态和内核态的 `SignalAction` ABI 没有完全对齐。

   位置：

   - `os/src/task/action.rs`
   - `user/src/lib.rs`

   内核侧结构体是：

   ```rust
   #[repr(C, align(16))]
   pub struct SignalAction { ... }
   ```

   用户侧结构体没有 `#[repr(C)]`。当前字段简单时可能暂时能跑，但 syscall 直接跨地址空间拷贝结构体，最好让两侧布局显式一致。

   建议：用户侧也加上 `#[repr(C, align(16))]`，或双方统一成同一套 ABI 定义。

### P2：后续可顺手整理

7. `fork` 后没有继承父进程的 `signal_mask`。

   位置：`os/src/task/task.rs`

   当前子任务初始化为 `SignalFlags::empty()`。Linux 语义下 fork 后子进程应继承父进程 signal mask；不过如果当前项目只做最小信号能力，可以先标成 ABI 兼容性缺口。

8. `sigreturn` 对 `trap_ctx_backup` 直接 `unwrap()`。

   位置：`os/src/syscall/process.rs`

   如果用户程序在没有进入 handler 的情况下主动调用 `sigreturn`，内核会 panic。

   建议：没有 backup 时返回 `EINVAL` 或按项目约定返回错误码。

## main 合入情况

从本轮检查结果看，`main` 合入后的架构重构、文件系统补充、syscall 扩展等内容没有暴露出直接的合并冲突残留。需要注意的是，当前工作区还有两个未提交修改：

- `os/src/mm/mod.rs`
- `os/src/syscall/process.rs`

其中 `os/src/mm/mod.rs` 删除了 `translated_ref` / `translated_refmut` 的 re-export；当前仓库里没有发现其他模块继续依赖它们，所以暂时不是问题。

`os/src/syscall/process.rs` 删除了旧的 `sys_get_time`；当前用户态已经通过 `gettimeofday` 封装 `time_get`，没有发现旧接口调用残留。

## 建议修复顺序

建议先修 P0，因为它们会直接决定信号测试是否能稳定通过：

```text
1. 明确 SIGKILL / 默认终止类信号的退出路径
2. 修复默认 action 的 pending bit 清理和终止/忽略策略
3. 修复 sigaction 的 signum 边界和空指针语义
4. 修复 kill 的 signum 边界检查
5. 对齐用户态和内核态 SignalAction ABI
6. 处理 sigreturn 无 backup 时的错误返回
7. 再考虑 fork 是否继承 signal_mask
```

