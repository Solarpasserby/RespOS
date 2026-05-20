| 测试文件 | 依赖的系统调用 | 是否成功 |
| --- | --- | --- |
| `brk` | `brk` | ☐ |
| `chdir` | `chdir`, `mkdir` | ☐ |
| `clone` | `clone` | ☐ |
| `close` | `open`, `close` | ☑ |
| `dup2` | `dup2` | ☑ |
| `dup` | `dup` | ☑ |
| `execve` | `execve` | ☐ |
| `exit` | `exit` | ☑ |
| `fork` | `fork`, `wait` | ☐ |
| `fstat` | `open`, `fstat`, `close` | ☑ |
| `getcwd` | `getcwd` | ☑ |
| `getdents` | `open`, `getdents64`, `close` | ☑ |
| `getpid` | `getpid` | ☑ |
| `getppid` | `getppid` | ☑ |
| `gettimeofday` | `gettimeofday` | ☑ |
| `mkdir_` | `mkdirat` / `mkdir` | ☐ |
| `mmap` | `mmap` | ☐ |
| `mount` | `mount`, `umount2` | ☐ |
| `munmap` | `mmap`, `munmap` | ☐ |
| `openat` | `openat`, `close` | ☐ |
| `open` | `open`, `read`, `close` | ☐ |
| `pipe` | `pipe2` / `pipe`, `read`, `write`, `close` | ☐ |
| `read` | `open`, `read`, `close` | ☑ |
| `sleep` | `nanosleep` | ☑ |
| `times` | `times` | ☑ |
| `umount` | `mount`, `umount2` | ☐ |
| `uname` | `uname` | ☑ |
| `unlink` | `unlinkat` / `unlink` | ☐ |
| `wait` | `fork`, `wait4` / `wait` | ☐ |
| `waitpid` | `fork`, `wait4` / `waitpid` | ☐ |
| `write` | `write` | ☑ |
| `yield` | `sched_yield` | ☐ |

## 系统调用补全优先级

下面的排序不是按 syscall number 排，而是按当前项目推进 basic 测试时更实际的三个因素综合决定：

1. 是否直接对应已有 basic 测试项；
2. 是否容易基于现有内核能力补完；
3. 是否会成为后续多个测试的公共依赖。

### P1：随后补，直接影响一批核心测试

| 系统调用 / 语义缺口 | 当前状态 | 为什么排在第二梯队 |
| --- | --- | --- |
| `brk` | `TODO[UNIMPLEMENTED]` | 直接对应 `brk` 测试，也是后续用户堆的基础能力。 |
| `mmap` / `munmap` | `TODO[UNIMPLEMENTED]` | 直接对应 `mmap`、`munmap` 测试，但会进入内存管理核心，复杂度明显高于 P0。 |
| `openat` 的 `dirfd` 语义 | `TODO[ABI-COMPAT]` | 现在只兼容 `AT_FDCWD`；若 basic 测试出现相对目录 fd，这里会暴露问题。 |
| `mkdirat` 的 `dirfd` 语义 | `TODO[ABI-COMPAT]` | 与 `openat` 同类，建议和路径解析能力一起补。 |
| `wait4` 的完整语义 | `TODO[ABI-COMPAT]` | 目前只够覆盖 `waitpid` 子集；如果测试检查 `options`，需要继续补。 |

这一组开始会触及更真实的 Unix 语义，通常需要同时查看 `mm`、路径解析或任务状态，而不是只写一个短 handler。

### P2：最后补，测试面窄或实现代价偏高

| 系统调用 / 语义缺口 | 当前状态 | 为什么可以后放 |
| --- | --- | --- |
| `mount` / `umount2` | `TODO[UNIMPLEMENTED]` | 依赖完整挂载模型，和当前 ext4 / VFS 设计耦合较深。 |
| `setpriority` | `TODO[UNIMPLEMENTED]` | 当前调度器若尚未使用优先级，先做它对整体通过率帮助有限。 |
| `pipe2` 的 `flags` 语义 | `TODO[ABI-COMPAT]` | 如果 basic 只用普通 `pipe` 行为，可先延后。 |
| `clone` 的完整语义 | `TODO[ABI-COMPAT]` | 目前只是借用 `fork` 子集；真正支持 `stack`、线程式共享语义会牵涉任务模型。 |
| `execve` 的 `envp` 语义 | `TODO[ABI-COMPAT]` | basic 往往先关注程序替换和 argv，环境变量支持通常可后补。 |
