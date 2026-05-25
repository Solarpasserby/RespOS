| 测试文件 | 依赖的系统调用 | 是否成功 |
| --- | --- | --- |
| `brk` | `brk` | ☑ |
| `chdir` | `chdir`, `mkdir` | ☐ |
| `clone` | `clone` | ☑ |
| `close` | `open`, `close` | ☑ |
| `dup2` | `dup2` | ☑ |
| `dup` | `dup` | ☑ |
| `execve` | `execve` | ☑ |
| `exit` | `exit` | ☑ |
| `fork` | `fork`, `wait` | ☑ |
| `fstat` | `open`, `fstat`, `close` | ☑ |
| `getcwd` | `getcwd` | ☑ |
| `getdents` | `open`, `getdents64`, `close` | ☑ |
| `getpid` | `getpid` | ☑ |
| `getppid` | `getppid` | ☑ |
| `gettimeofday` | `gettimeofday` | ☑ |
| `mkdir_` | `mkdirat` / `mkdir` | ☐ |
| `mmap` | `mmap` | ☑ |
| `mount` | `mount`, `umount2` | ☐ |
| `munmap` | `mmap`, `munmap` | ☑ |
| `openat` | `openat`, `close` | ☑ |
| `open` | `open`, `read`, `close` | ☑ |
| `pipe` | `pipe2` / `pipe`, `read`, `write`, `close` | ☐ |
| `read` | `open`, `read`, `close` | ☑ |
| `sleep` | `nanosleep` | ☑ |
| `times` | `times` | ☑ |
| `umount` | `mount`, `umount2` | ☐ |
| `uname` | `uname` | ☑ |
| `unlink` | `unlinkat` / `unlink` | ☑ |
| `wait` | `fork`, `wait4` / `wait` | ☑ |
| `waitpid` | `fork`, `wait4` / `waitpid` | ☑ |
| `write` | `write` | ☑ |
| `yield` | `sched_yield` | ☑ |

### P2：最后补，测试面窄或实现代价偏高

| 系统调用 / 语义缺口 | 当前状态 | 为什么可以后放 |
| --- | --- | --- |
| `mount` / `umount2` | `TODO[UNIMPLEMENTED]` | 依赖完整挂载模型，和当前 ext4 / VFS 设计耦合较深。 |
| `pipe2` 的 `flags` 语义 | `TODO[ABI-COMPAT]` | 如果 basic 只用普通 `pipe` 行为，可先延后。 |
