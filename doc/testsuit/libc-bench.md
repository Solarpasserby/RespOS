5. **`/proc/self/smaps` fake 文件**

6. **`tmpfile` 依赖的文件语义**

---

## 动态链接后续待补充事项

当前已完成的 aux 向量完善（2026-06-01）为动态链接打好了基础，但尚有若干工作需要在后续完成才能使文件系统中的动态链接 ELF（如 `/glibc/` 下的程序）正常运行。

### 1. 文件系统 ELF 的动态链接器加载

**现状**：`MemorySet::from_elf_data(&[u8])` 只能从内置应用数据（link_app.S 嵌入的二进制）中查找动态链接器。当 `PT_INTERP` 指向文件系统路径（如 `/lib/ld-linux-riscv64-lp64d.so.1`）时，若该链接器未嵌入内核镜像，会回退为静态链接（`need_dl = false`），此时 aux 向量中不含 `AT_BASE`，且 `entry_point` 保持为原始 ELF 入口，动态链接会失败。

**需要做的**：在 `sys_execve`（`os/src/syscall/process.rs:136`）中，从文件读取 ELF 后先解析程序头，若检测到 `PT_INTERP` 且 `.interp` 指向文件系统路径，则：

```text
1. 用 path_open 打开动态链接器文件
2. 读取其全部数据
3. 将主 ELF 数据和链接器数据一起传给一个新的 from_elf_data 变体
   （或扩展 from_elf_data 接受 Option<&[u8]> interp_data 参数）
4. 在 from_elf_data 中将链接器的 LOAD 段映射到 DL_INTERP_OFFSET 偏移
5. 将 entry_point 设为链接器入口 + DL_INTERP_OFFSET
```

关键代码路径：
- `os/src/syscall/process.rs:148` — `path_open(AT_FDCWD, &path, ...)` 成功后调用 `task.execve(all_data.as_slice(), ...)`
- 需要在此处插入 PT_INTERP 解析和链接器加载逻辑

### 2. LoongArch 架构的配置常量

**现状**：`os/src/arch/mod.rs` 目前仅 `#[cfg(target_arch = "riscv64")]` 下有 `pub mod rv64;`，LoongArch 没有对应的 config 模块。`DL_INTERP_OFFSET` 和 `CLK_TCK` 目前只定义在 `os/src/arch/rv64/config/mm.rs` 中。

**需要做的**：在实现 LoongArch 架构层时，需要创建对应的 config 模块（如 `os/src/arch/loongarch64/config/mm.rs`），其中同样定义：

```rust
pub const DL_INTERP_OFFSET: usize = 0x30_0000_0000; // 与 RISC-V 相同值
pub const CLK_TCK: usize = 100;
pub const PAGE_SIZE: usize = 0x4000; // LA64 页大小为 16KB
pub const PAGE_SIZE_BITS: usize = 14;
// ... 以及其他必要常量
```

同时在 `os/src/arch/mod.rs` 中添加：
```rust
#[cfg(target_arch = "loongarch64")]
pub mod loongarch64;
#[cfg(target_arch = "loongarch64")]
pub use loongarch64::*;
```

### 3. LoongArch 用户栈写入的 VA/PA 差异

**现状**：`init_user_stack`（`os/src/task/task.rs`）直接通过虚拟地址写入用户栈：

```rust
let ptr = *stack_ptr as *mut u8;
ptr.copy_from_nonoverlapping(...);
```

这在 RISC-V 内核态下可以正常工作（内核使用用户页表可直接访问用户虚拟地址），但 LoongArch 可能需要在写入前将 VA 转换为 PA（参考 RocketOS 的做法：`memory_set.translate_va_to_pa(VirtAddr::from(*stack_ptr))`）。

**需要做的**：在 `init_user_stack` 的 `push_strings_to_stack`、`push_usize_to_stack`、`push_pointers_to_stack` 等辅助函数中，根据目标架构选择直接使用 VA 还是先转 PA。可以在 `memory_set` 上添加一个辅助方法：

```rust
/// 获取用户栈虚拟地址对应的可写物理地址
fn user_stack_ptr(&self, va: usize) -> *mut u8 {
    #[cfg(target_arch = "riscv64")]
    { va as *mut u8 }
    #[cfg(target_arch = "loongarch64")]
    { self.translate_va_to_pa(VirtAddr::from(va)).unwrap() as *mut u8 }
}
```

### 4. 随机数源（AT_RANDOM）

**现状**：`init_user_stack` 中 `AT_RANDOM` 指向的 16 字节区域当前为全零：

```rust
*user_sp -= 16; // 占位，未填充实际随机数
```

musl/glibc 的栈保护（stack canary）和 `AT_SECURE` 判断依赖此随机数。全零的随机数会削弱栈保护效果，但不影响正常运行。

**需要做的**：接入硬件随机数源（如 RISC-V 的 `seed` CSR 或 VirtIO entropy 设备），至少在初始化时填充一次随机字节。作为临时方案，可以用当前时间戳和 CPU 周期计数拼凑一个低质量的随机数。

### 5. AT_SECURE 与 setuid 语义

**现状**：aux 向量中 `AT_UID`/`AT_EUID`/`AT_GID`/`AT_EGID` 当前硬编码为 0。如果将来支持 setuid 程序，`AT_SECURE` 需要根据实际 uid 和 euid 是否匹配来设置，否则动态链接器不会启用安全模式（`LD_LIBRARY_PATH` 忽略等）。

**当前影响**：无。RespOS 暂无用户/权限管理，所有进程以 uid=0 运行，无需 `AT_SECURE`。此项仅为远期规划备忘。
