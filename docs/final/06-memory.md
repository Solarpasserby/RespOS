# 6. 内存管理

RespOS 的内存管理由三类资源共同构成：物理页帧分配器管理真实 RAM，内核堆为 Rust 动态对象提供小块内存，`MemorySet` 则统一维护进程的 VMA、页表项和驻留页帧。系统调用层只负责检查 Linux ABI 参数；地址空间的切分、缺页、COW 和映射生命周期均由 `MemorySet` 完成。

## 6.1 物理内存、内核堆与 direct map

物理页帧与内核堆是两套相互独立的分配机制。`StackFrameAllocator` 以 4 KiB 页为单位，从内核镜像结束位置与 `MEMORY_START` 的较大者开始分配，首次分配沿地址递增，释放页进入回收栈。`FrameTracker` 创建时清零页面，并在最后一个所有者释放时通过 `Drop` 自动归还页帧。它并不是 buddy allocator；`buddy_system_allocator::LockedHeap` 只服务于 `Vec`、`Arc`、`BTreeMap` 等内核动态对象。

| 资源 | 管理对象 | 所有权与回收 | 当前边界 |
| --- | --- | --- | --- |
| `StackFrameAllocator` | 物理页、页表页、用户驻留页 | `FrameTracker`/`Arc<FrameTracker>` 最后引用释放时回收 | 全局锁，按页分配，无换出 |
| `IrqSafeHeap<32>` | 内核动态对象 | Rust allocator 的 alloc/dealloc | RV64 为 64 MiB，LA64 为 48 MiB |
| kernel direct map | RAM 与 MMIO 的高半区线性映射 | 内核页表长期持有 | 可访问窗口不等于可分配 RAM |

普通自旋锁只能排除其他 CPU，不能阻止同一 CPU 在持锁期间被 timer 中断后再次进入 allocator。为避免这种自锁，`IrqSafeHeap` 在所有分配、释放及直接取得 heap guard 的路径上先关闭本地中断，并保证先释放 heap lock、后恢复中断。该规则是 SMP 下内核堆的必要不变量。

RV64 启动时的内存发现分为“先建立最大可达窗口”和“再确定实际可分配上限”两步：

```text
entry.asm 建立 early page table（最多覆盖 8 GiB QEMU RAM）
        ↓
boot hart 解析 OpenSBI 传入的 FDT /memory reg
        ↓
physical_memory_end = clamp(FDT 末址, 256 MiB 基线, 8 GiB 上限)
        ↓
frame allocator 仅管理 [kernel_end, physical_memory_end)
        ↓
kernel MemorySet 建立相同实际范围的高半区 direct map
```

首个 RAM GiB 使用 4 KiB 页映射，以保持 `.text`、`.rodata`、`.data/.bss` 的细粒度执行和写权限；其后的完整 GiB 在 RV64 Sv39 页表中使用 1 GiB 叶页，避免 8 GiB direct map 消耗数千个页表页。LA64 当前没有对应的动态 FDT 路径，`physical_memory_end()` 固定返回 256 MiB。因而 early 页表能够覆盖 8 GiB，只说明 CPU 能访问该窗口，绝不意味着 frame allocator 可以把窗口中的全部地址当作真实 RAM。

动态内存路径已在 RV64 上分别验证：`-m 8G` 时 `/proc/meminfo` 报告 `MemTotal: 8386560 kB`，`-m 256M -smp 1` 时报告 `MemTotal: 260096 kB`，后者还能正常退出 QEMU。这些数字包含内核保留造成的差异，不应直接等同于空闲 frame 数。

## 6.2 MemorySet、VMA、PTE 与用户地址空间

`MemorySet` 是一个地址空间的语义所有者，包含页表、按地址有序的 `areas: Vec<MapArea>`、`brk/heap_bottom`、`mmap_start`，以及 RV64 上用于 TLB shootdown 的 active-hart mask。每个 `MapArea` 表示一段连续虚拟页范围，同时记录权限、映射类型、shared/locked/fork 标志、可选文件 backing，以及仅针对已驻留页面的 `data_frames`。

```text
MemorySet
 ├─ PageTable: VPN → PTE → PPN
 └─ MapArea (VMA): 地址范围 + 权限 + backing 语义
       ├─ 尚未访问的 lazy 页：只有 VMA，没有 PTE/frame
       └─ 已驻留页：data_frames[VPN] → Arc<FrameTracker>
                                    └→ 与 PTE 指向同一 PPN
```

这一区分使“大虚拟范围、少量实际访问”成为常态：VMA 描述用户可见的映射承诺，PTE 表示当前硬件可访问状态，`data_frames` 则承担物理页所有权。释放 lazy framed VMA 时只遍历 resident `data_frames`，而不是扫描整个虚拟跨度；只有恒等映射的 `Direct` area 才按范围逐页解除映射。

地址空间修改遵守以下结构不变量：

- VMA 非空、按起始地址有序且互不重叠；
- `data_frames` 中的 VPN 必须属于本 VMA，并存在有效 PTE；
- 用户 VMA 的 PTE 带 `USER`，PTE 读写执行权限与 VMA 相容；
- private COW 页不能同时保持可写，shared mapping 不进入 private COW；
- 所有长度加法、向上页对齐和地址上界检查必须在构造 `VirtAddr` 前完成，用户范围不得越过 `TRAMPOLINE`。

`munmap`、`mprotect`、`mremap` 与 `MAP_FIXED` 都可能只覆盖 VMA 的中间部分。`MapArea::split_by_overlap` 将原 area 拆成 left/middle/right，并同步调整 resident frame 集合与文件 offset/len，避免 syscall 层维护另一套映射状态。debug 内核在修改后运行结构检查，并带有 VMA split 自检。

失败原子性同样由 MM 层保证。例如 COW 复制先申请并填充新 frame，再以不会分配内存的 `replace_pte` 替换旧 PTE；若申请失败，旧映射仍保持有效。`mmap` 放置、重映射和范围删除也先校验完整范围及冲突，再提交 VMA/PTE 变化。

RV64 多核下，task 恢复用户地址空间前在 `MemorySet` 读锁内设置本 hart 的 active bit，切回 per-CPU idle/kernel 页表后才清除。页表写入者持写锁，提交 PTE 后先执行本地 fence，再仅向仍 active 的远端 hart 发出 SBI RFENCE。该协议已通过 2 核 100 轮和 8 核 1000 轮固定地址反复 `munmap + MAP_FIXED mmap` 的共享地址空间测试；其同步完成语义目前只对所验证的 QEMU/OpenSBI 环境成立。

## 6.3 缺页、lazy allocation 与 COW

缺页：CPU 访问虚拟地址时，页表无法给其提供合法的可访问的物理页映射。

Lazy allocation：虚拟地址是合法的（在某个 VMA 范围内），但对应的物理页还没有分配，PTE 也没有建立成有效映射。

```
一个进程 → 虚拟地址空间 → VMA（内核数据结构）描述哪些区域合法 → 页表 → 物理内存 RAM
```

Page fault 的几种情况：

| 情况 | 说明 |
|---|---|
| PTE 不存在/无效 | lazy allocation，按需分配物理页 |
| 物理页尚未加载 | demand paging，从磁盘/文件读入 |
| 写只读页面 | COW，fork 后父子共享页被写入 |
| 用户访问内核页 | 权限不足，SIGSEGV |
| 执行不可执行页面 | 权限不足，SIGSEGV |
| 地址不属于任何 VMA | 非法访问，杀进程 |


缺页处理的全流程：

```
CPU 访问 VA → 查页表失败 → Page fault → 内核检查 VMA → 决定这个访问是否合法
→ frame_alloc() → 拿到一个新的物理页 → 把数据放进去 → map(vpn, ppn)
→ 创建/修改 PTE → VA→PA 建立映射 → 返回用户态 → 重新执行刚才失败的指令
```

### trap 入口

RV64 的 trap handler 把用户页错误转成 `PageFaultCause::{Instruction, Load, Store}` 三种，然后在 `MemorySet` 的写锁里调用 `handle_page_fault`。

发生 page fault 时 RISC-V 硬件把出错的虚拟地址写到 `stval` CSR，内核读出来转成 vpn（`memory_set.rs:2421`）：

```rust
let vpn = VirtAddr::from(stval).floor();
```

### handle_page_fault 处理顺序

`handle_page_fault`（`memory_set.rs:2417-2503`）先定位覆盖 fault VPN 的 VMA，再检查 USER 和本次访问需要的 R/W/X 权限。不存在 VMA、访问 trampoline 以上地址或权限不符都返回 `EFAULT`，最终由 trap 层变成发给进程的 `SIGSEGV`。

A. 找到这个虚拟页属于哪个 VMA

```rust
let area_idx = self.areas.iter().position(|a| a.vpn_range.contain(&vpn))?;
```

一个进程（`MemorySet`）有多个 `MapArea`：

```
self.areas = [
    代码段 VMA,
    数据段 VMA,
    heap VMA,
    mmap VMA,
    stack VMA,
]
```

每个 `MapArea` 描述一段连续的虚拟地址范围。`self.areas.iter()` 一个一个检查，`.position(...)` 找到满足条件的那个元素的位置（索引）。没有 VMA 覆盖就返回 `EFAULT`。

B. 检查 VMA 权限

```rust
let area_perm = self.areas[area_idx].map_perm;
if !area_perm.contains(MapPermission::USER) {
    return Err(Errno::EFAULT);
}
```

`area_perm` 是 VMA 的权限位，必须包含 `USER`，否则是内核页，用户态无权访问。

C. 检查 PTE 并判断 fault 类型

```rust
let pte = self.page_table.translate(vpn);
let is_store = matches!(cause, PageFaultCause::Store);
let needed_perm = if is_store {
    MapPermission::WRITE
} else {
    match cause {
        PageFaultCause::Instruction => MapPermission::EXECUTE,
        _ => MapPermission::READ,
    }
};
```

`self.page_table` 是当前进程的页表。`translate(vpn)` 走 SV39 页表 walk 找到对应的 PTE。`is_store` 判断这次 page fault 是不是写内存导致的。`needed_perm` 是本次访问需要的最小权限。

D. 三个分支，按顺序匹配

```
用户 page fault
  ├─ 无覆盖 VMA / 越界 / VMA 权限不允许 → EFAULT
  ├─ 有效 PTE + store + COW         → 分支一（COW 写入）
  ├─ PTE 不存在或无效               → 分支二（惰性分配）
  ├─ 并发修改后 PTE 已具备所需权限   → 分支三（竞态重试）
  └─ 其他权限 fault                → EFAULT
```

**分支一：COW 写入**（`memory_set.rs:2445-2471`）

```rust
if is_store && pte.is_some_and(|p| p.is_valid() && p.is_cow()) {
    // 写操作 + PTE 有效 + PTE 标记为 COW
    let count = Arc::strong_count(old_frame);
    // count 表示现在还有多少地方共同持有这个物理页

    if count == 1 {
        // 最后一个引用：只需恢复写权限，无需复制
        self.page_table.modify_pte(vpn, flags);
        self.page_table.clear_pte_cow(vpn);
    } else {
        // 仍有共享者：分配新页，复制旧数据
        // frame_alloc() → 分配一个新的物理页 B
        // 把旧物理页 A 的内容复制到新物理页 B
        // 修改当前进程的页表，让当前 vpn 不再映射 A，改成映射 B
        // 新 PTE 设置 W=1，COW 清掉
        self.areas[area_idx].remap_one_with_data(&mut self.page_table, vpn, old_data)?;
    }
    self.flush_tlb(); // 刷新 TLB
    return Ok(());
}
```

COW 的所有权由 `Arc<FrameTracker>` 表达。`strong_count == 1` 说明另一个进程已经退出或之前触发过自己的 COW fault 拿到了新页，这时只需要 `modify_pte` 恢复 `W=1`、`clear_pte_cow` 清除 COW 位——不分配、不复制，只改 PTE 标志位。`modify_pte` 只改标志位不改 PPN（`page_table.rs:267`），专门给 COW 恢复用的。

`strong_count > 1` 说明还有别人共享这个页。`remap_one_with_data`（`memory_set.rs:2945`）做的事：`frame_alloc()` 拿一个新物理页 B → 把旧页 A 的内容 copy 到 B → `replace_pte` 用新 PTE 原子替换旧 PTE，新 PTE 设 `W=1`，COW 清掉 → 旧帧引用递减。

关键设计：旧 PTE 和 frame 一直保留到新页分配和复制全部成功之后。如果 `frame_alloc()` 失败返回 `ENOMEM`，旧映射原封不动，地址空间里不会留一个洞。

**分支二：惰性分配**（`memory_set.rs:2475-2483`）

```rust
if pte.is_none() || !pte.unwrap().is_valid() {
    if !area_perm.contains(needed_perm) {
        return Err(Errno::EFAULT);
    }
    self.areas[area_idx].map_one(&mut self.page_table, vpn)?;
    // 给这个 VMA 里面的 vpn 真正分配一个物理页，并建立 VPN→PPN 的页表映射
    self.flush_tlb();
    return Ok(());
}
```

`map_one`（`memory_set.rs:2897-2942`）做的事：
1. `frame_alloc()` 从物理页帧分配器拿一个新物理页
2. 如果是文件映射（`file_backing`），从文件读数据填进新页，不足部分保持零
3. `page_table.map(vpn, ppn, flags)` 建立 PTE 映射
4. 把 `FrameTracker` 插入 `data_frames`
5. 如果 PTE 映射失败，从 `data_frames` 里删掉 frame 做清理

**分支三：竞态重试**（`memory_set.rs:2491-2499`）

```rust
// CPU 的 TLB 还保存着旧的 PTE
if area_perm.contains(needed_perm)
    && pte.is_some_and(|pte| match cause {
        PageFaultCause::Instruction => pte.executable(),
        PageFaultCause::Load => pte.readable(),
        PageFaultCause::Store => pte.writable(),
    })
{
    sfence();  // 仅刷新本地 TLB，重试故障指令
    return Ok(());
}
```

多核上 fault 被本 hart 锁存了，但拿到 `MemorySet` 写锁之前另一个 hart 已经把页补好了。等到本 hart 拿到锁一看，PTE 已经是有效的。这时候只要 VMA 允许、PTE 也允许，直接 `sfence` 刷一下本 hart 的 TLB 然后重试就行，不用误报非法访问。这个修复帮 RV64 8 核 BuildStorm 越过了工具链启动，但不等于完整的 COW/mprotect 并发验证。

### COW 完整生命周期

#### COW PTE 位

RV64 用 PTE 第 8 位标记 COW（`page_table.rs:332`）：

```
| 63..54 | 53..10 |  9 |  8 | 7 | 6 | 5 | 4 | 3 | 2 | 1 | 0 |
| 保留   |  PPN   | RSW| COW| D | A | G | U | X | W | R | V |
```

硬件不认识这个位——纯粹是内核的软件约定。`MapPermission` 不含 COW，COW 只存在于 arch 层的 `PTEFlags`。

#### fork 时建立 COW

`MemorySet::from_existed_user`（`memory_set.rs:2215-2307`）是 fork 时构造子进程地址空间的函数，COW 就是在这建立的。

fork 前：父进程独占可写页（PTE 里 `W=1, COW=0`，引用计数=1）。

对每个私有可写 VMA 里的每个已分配物理页：

```rust
// 1. 先给子进程建 PTE，指向同一个物理页，去掉 WRITE|DIRTY
let mut child_flags = parent_pte.flags();
child_flags.remove(PTEFlags::WRITE | PTEFlags::DIRTY);
memory_set.page_table.map(vpn, ppn, child_flags)?;

// 2. 子进程 PTE 设 COW 位
memory_set.page_table.make_pte_cow(vpn);

// 3. 修改父进程 PTE，同样去掉 WRITE|DIRTY
let mut flags = parent_pte.flags();
flags.remove(PTEFlags::WRITE | PTEFlags::DIRTY);
user_space.page_table.modify_pte(vpn, flags);

// 4. 父进程 PTE 设 COW 位
user_space.page_table.make_pte_cow(vpn);
```

fork 后，父子共享同一个物理帧：

```
父 PTE: V=1, R=1, W=0, COW=1
子 PTE: V=1, R=1, W=0, COW=1
Arc<FrameTracker>::strong_count = 2
```

关键设计：子进程的 PTE 先建。如果子进程因为 `ENOMEM` 失败，父进程的地址空间还是原样，不会留在半 COW 状态。

三种特殊情况：
- **惰性未分配页**（`data_frames` 没记录，PTE 无效）：父子都跳过，各自以后按需分配。
- **只读页**：直接共享，保留原始标志位，不做 COW。
- **共享映射**（`area.shared`）：直接共享，不做 COW。

#### COW 写入时

写一个 COW 页的时候触发 `handle_page_fault` 分支一，上面已经讲过了。两种情况对比：

| | count == 1 | count > 1 |
|---|---|---|
| 分配新页 | 不需要 | frame_alloc() 分一个 |
| 复制数据 | 不需要 | 旧页内容 copy 到新页 |
| PTE 操作 | modify_pte（改标志位，PPN 不变） | replace_pte（原子替换成新 PPN） |
| 旧帧引用 | 不变 | 减 1 |
| 新帧引用 | — | = 1 |

两种情况最后都 `flush_tlb()`。

### 不同 VMA 类型的缺页行为

`map_one` 里 `MapType` 和 `self.shared` 的组合决定了对每种 VMA 缺页时怎么处理：

| VMA 类型 | 首次访问 | fork 后写入 | 页内容来源 |
|---|---|---|---|
| private anonymous | lazy 分配清零页 | COW | 零页语义 |
| private file | lazy 分配私有 frame | COW，不写回原文件 | 按 backing offset 读文件，不足部分保持零 |
| shared anonymous | 共享 `Arc<FrameTracker>` | 直接写共享页，不做 COW | 初始清零 |
| shared file | 建映射前取得全局共享 file frame | 直接写共享页 | (dev, ino, page index) 标识的共享页 |


## 6.4 文件映射、共享写回与用户拷贝

`sys_mmap` 先验证长度、页对齐、用户上界、`MAP_SHARED`/`MAP_PRIVATE` 互斥关系、文件描述符和 offset，再把请求交给 `MemorySet::mmap_area`。匿名 private 映射保留为 lazy VMA；匿名 shared 和文件 shared 映射使用共享 frame；private file mapping 保存 file、offset 和有效长度，在首次 fault 时填页。`MAP_FIXED` 会替换原范围，`MAP_FIXED_NOREPLACE` 则在冲突时失败。

可写 `MAP_SHARED` 的关键不是“允许建立 PTE”，而是完整的写回生命周期。RespOS 采用锁外 I/O 协议：

```text
建立 shared file mapping
  锁外按 (dev, ino, page index) 取得/读取共享 frames
       → 持 MemorySet 写锁提交 VMA/PTE

msync / munmap / MAP_FIXED / mremap / mprotect / exit
  持 MemorySet 读锁复制 resident writable shared 页快照
       → 释放 MM 锁
       → FileOp write；MS_SYNC 再 fsync
       → 成功后执行相应的地址空间变更
```

当前硬件抽象未利用 dirty bit，因此所有 resident、writable、shared file pages 都会被保守写回。写回前重新读取文件长度，只写到当前 EOF，避免文件被 truncate 后由旧映射再次扩展。短写返回 `EIO`。`MS_INVALIDATE` 因缺少 inode-wide 共享 frame 失效协议而明确返回 `EOPNOTSUPP`；truncate 后访问越界页应产生 `SIGBUS` 的完整语义也尚未实现。

上述“锁外 I/O”只覆盖当前 shared file 建图和写回协议。private file-backed page fault（包括主程序 PT_LOAD 的按需装页）仍会在持有 `MemorySet` 写锁时调用文件读取，是后续需要继续拆分为 prepare/revalidate/commit 的锁序边界，不能据此宣称 MM 锁内已不存在后端 I/O。

用户拷贝不直接把用户指针转成 Rust slice 后解引用，因为该地址可能跨页、跨相邻 VMA，或正处于 lazy/COW 状态。`copy_from_user`/`copy_to_user` 先用 checked arithmetic 得到完整 VPNRange，在 `MemorySet` 写锁内检查整段 VMA 权限并调用 `ensure_user_page_access` 补齐 lazy/COW 页，然后逐页翻译 PTE，通过对应物理页复制。这样 kernel copy 不会因直接访问尚未映射的用户虚拟地址而触发不可恢复的 kernel page fault。futex 锁内的固定 4 字节比较则使用 no-fault 读取，可能缺页的预检查必须在 futex 全局锁外完成。

## 6.5 exec 的 file-backed ELF 与内存压力

旧的 filesystem exec 路径先用 `File::read_all()` 把完整 ELF 放入固定内核堆，再解析和复制每个 `PT_LOAD`。面对约 45 MiB 的 cargo 可执行文件时，这一临时 `Vec` 会直接挤压 64 MiB 的 RV64 kernel heap，而且程序页随后还要占用物理 frame，造成重复峰值。RespOS 没有通过扩大固定 heap 回避问题，而是增加了 `MemorySet::try_from_elf_file`：

1. 先定长读取 64 字节 ELF64 header，验证 magic、位数与小端格式；
2. 用 checked arithmetic 验证 program-header offset、56 字节 entry size、数量和文件边界；
3. 只保留 program headers 与 `PT_INTERP` 字符串所需的元数据前缀，硬上限为 1 MiB；
4. 对主程序的每个 `PT_LOAD` 建立 private file-backed lazy VMA，记录页内偏移、文件 offset 和 file size；
5. fault 时只读取被访问的页，frame 初始清零，因此 `p_memsz - p_filesz` 的 BSS 部分自然保持为零。

| 指标 | eager `read_all` | 当前 file-backed 主程序 |
| --- | --- | --- |
| exec 临时 heap | 随 ELF 文件大小增长 | 随 header/`PT_INTERP` 元数据增长，最多 1 MiB |
| `PT_LOAD` frame | exec 时全部建立 | 首次访问时逐页建立 |
| 大 ELF 峰值 | 完整文件副本与程序页叠加 | 元数据与实际工作集为主 |
| 畸形 ELF | 容易放大分配 | 越界、溢出或超限返回 `ENOEXEC` |

`execve_file` 在新 `MemorySet` 和用户栈准备完成后，先在旧地址空间仍安装时停止并清理 sibling thread，再替换进程映像。这一点与内存生命周期直接相关：robust list 和 `clear_child_tid` 保存的是旧用户地址，若先换 MM，清理动作可能写坏新程序映像。

该路径已经让 45,559,552 字节的 cargo 在 RV64 `-smp 8 -m 8G` 环境中完成 `BUILDSTORM_TOOLCHAIN ok`，并消除了此前由固定 256 MiB 管理上限和整文件加载导致的 `ENOMEM` 边界。但证据只能说明工具链发现和启动阶段已越过：BuildStorm minibuild/full compile 仍未完成，不能写成完整比赛负载通过。

当前还有一个明确限制：动态链接器仍由 `read_dynamic_linker()` 整文件读取后 eager 建立 `PT_LOAD`，尚未复用主程序的 file-backed loader。因此本次设计显著降低了大主程序的内存峰值，但没有消除 interpreter 路径的整文件内核堆占用；后续应把文件身份、offset 和长度继续传入统一的 lazy ELF 映射流程。
