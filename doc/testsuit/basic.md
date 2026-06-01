### 测例完成情况

| 测试文件 | 依赖的系统调用 | 是否成功 |
| --- | --- | --- |
| `brk` | `brk` | ☑ |
| `chdir` | `chdir`, `mkdir` | ☑ |
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
| `mkdir_` | `mkdirat` / `mkdir` | ☑ |
| `mmap` | `mmap` | ☑ |
| `mount` | `mount`, `umount2` | ☑ |
| `munmap` | `mmap`, `munmap` | ☑ |
| `openat` | `openat`, `close` | ☑ |
| `open` | `open`, `read`, `close` | ☑ |
| `pipe` | `pipe2` / `pipe`, `read`, `write`, `close` | ☑ |
| `read` | `open`, `read`, `close` | ☑ |
| `sleep` | `nanosleep` | ☑ |
| `times` | `times` | ☑ |
| `umount` | `mount`, `umount2` | ☑ |
| `uname` | `uname` | ☑ |
| `unlink` | `unlinkat` / `unlink` | ☑ |
| `wait` | `fork`, `wait4` / `wait` | ☑ |
| `waitpid` | `fork`, `wait4` / `waitpid` | ☑ |
| `write` | `write` | ☑ |
| `yield` | `sched_yield` | ☑ |


### 阶段总结

花了一周的时间完成了 basic 测例，超出预计的时间，但到底也算是对内核的完善的吧。实现线程模型确实是挺不容易的。
但需要说明的是，部分系统调用只做了最基础的实现，还需后续完善。
