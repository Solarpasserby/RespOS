1. **`clone` 的线程语义**
   
   源码里 `malloc.c` 和 `pthread.c` 大量用 `pthread_create/join/mutex`。musl pthread 会走 `clone`，而你现在忽略了 `_tls/_ctid/_ptid`。

   ```text
   CLONE_SETTLS        -> 子线程 x4(tp) = tls
   CLONE_CHILD_SETTID  -> 在子线程地址空间 *ctid = tid
   CLONE_PARENT_SETTID -> 在父线程地址空间 *ptid = tid
   CLONE_CHILD_CLEARTID -> 线程退出时 *ctid = 0，并 futex wake
   ```

2. **`futex` syscall 98**

   ```text
   FUTEX_WAIT: 如果 *uaddr == val，就 yield/阻塞；否则返回 EAGAIN
   FUTEX_WAKE: 唤醒等待在 uaddr 上的任务，返回唤醒数
   ```

3. **`set_tid_address` syscall 96**

   musl 初始化线程库时会用它设置 clear-child-tid 地址。你的反汇编也看到 syscall 96。它和 `CLONE_CHILD_CLEARTID` 配合，用于线程退出唤醒 joiner。

5. **`/proc/self/smaps` fake 文件**

6. **`tmpfile` 依赖的文件语义**
