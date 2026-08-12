# MM 模块基础说明

这份文档会刻意弱化虚拟地址到物理地址的翻译细节，也不会展开多级页表的实现，而是重点理解三件事：

1. 内核和用户任务的地址空间大致怎么分布；
2. 内核如何组织需要动态映射的内存段；
3. 页帧分配器和内核堆分配器分别解决什么问题。

注意：非常抱歉，由于**内核访问用户数据的接口**目前在新分支进行了补全和完善（甚至 bug 还没改完），这里先留出位置，不作为当前讲解重点。

## mm 子模块文件职责总览

```text
os/src/mm
├── mod.rs              组织内存管理子模块，提供 mm::init 初始化入口
├── memory_set.rs       定义 MemorySet 和 MapArea，负责描述地址空间和逻辑段
├── frame_allocator.rs  管理可分配的物理页帧，返回带自动回收能力的 FrameTracker
├── heap_allocator.rs   初始化内核全局堆，使 Vec, Arc 等分配在堆上的数据结构可以使用
├── page_table.rs       封装页表创建、映射和查询；本指南只把它当作底层工具
└── address.rs          封装地址、页号和页范围等基础类型
```

简单来说，`memory_set.rs` 回答“一个地址空间里有哪些段”，`frame_allocator.rs` 回答“需要新页帧时从哪里拿”，`heap_allocator.rs` 回答“内核里 `Vec` / `Arc` 这些堆对象从哪里分配”。

## 第一部分：内存模块初始化发生在哪里

内核进入 Rust 后，初始化顺序在 [`rust_main`](../os/src/main.rs#L37) 中：

```text
clear_bss
    -> mm::init
    -> task::add_initproc
    -> trap::init
    -> task::run_tasks
```

关键点是：内存模块在创建第一个用户任务之前初始化。[`mm::init`](../os/src/mm/mod.rs#L23) 里面做三件事：

1. [`init_heap`](../os/src/mm/mod.rs#L24)：先启用内核堆；
2. [`init_frame_allocator`](../os/src/mm/mod.rs#L25)：再初始化页帧分配器；
3. [`KERNEL_SPACE.lock().activate`](../os/src/mm/mod.rs#L26)：最后激活正式的内核地址空间。

这里先初始化堆，是因为后续内存管理结构会用到 `Vec`、`BTreeMap`、`Arc` 等需要动态分配的类型。再初始化页帧分配器，是为了之后创建页表页、用户程序数据页、内核栈页等都能从统一入口申请物理页帧。

在进入 `rust_main` 之前，汇编启动代码已经准备了过渡用的临时启动页表和启动栈。[`entry.asm`](../os/src/entry.asm#L6) 设置 `boot_stack_top` 作为初始栈，[`boot_pagetable`](../os/src/entry.asm#L26) 则用两个 1GB 大页保证早期代码能跑起来。正式进入 `mm::init` 后，内核会构建自己的地址空间 `KERNEL_SPACE` 并切换过去。

## 第二部分：MemorySet 和 MapArea 怎么描述地址空间

[`MemorySet`](../os/src/mm/memory_set.rs#L43) 是“一个地址空间”的抽象，内部主要有两部分：

- `page_table`：底层页表对象；
- `areas`：一组 [`MapArea`](../os/src/mm/memory_set.rs#L280)，每个 `MapArea` 描述一段连续虚拟页。

学习时可以先把 `page_table` 当作底层工具，把重点放在 `areas`：地址空间不是一整块连续内存，而是由多个逻辑段拼起来的。例如内核地址空间有 `.text`、`.rodata`、`.data`、`.bss`、剩余内存区；用户地址空间有 ELF 加载段和用户栈。

新增逻辑段统一经过 [`push_empty_map_area`](../os/src/mm/memory_set.rs#L50)：

```text
MapArea::new(...)
    -> map_area.map(...)
    -> 可选 copy_data(...)
    -> areas.push(map_area)
```

其中 [`MapArea::map_one`](../os/src/mm/memory_set.rs#L359) 根据映射类型分成两类：

- `MapType::Direct`：用于内核固定区域，虚拟页号和物理页号之间是线性偏移，不从页帧分配器拿数据页；
- `MapType::Framed`：用于非线性映射区域，每一页都调用 [`frame_alloc`](../os/src/mm/memory_set.rs#L368) 获取独立物理页帧。

用户程序的 ELF 段、用户栈、任务内核栈都属于 `Framed`；内核镜像段和 `ekernel..KERNEL_BASE + MEMORY_END` 这类固定偏移区域属于 `Direct`。

## 第三部分：地址空间的整体分布

对于 64 位系统，64 位的地址可以描述极大的空间。但是我们**使用虚拟地址**来描述我们的地址空间。对于我们采用的 SV39 的实现机制，它可以描述 $2^{39}$ byte 也就是 512 GB 大小的空间，我们一般把地址空间分成两大块：

- 低地址区域（64 位地址中最低的 256 G 空间）：用户程序自己的代码段、数据段、用户栈等；
- 高地址区域（64 位地址中最高的 256 G 空间）：内核地址空间，包括内核镜像、内核堆、可分配物理内存的线性映射、每个任务的内核栈等。

我们将内核映射到高地址，因此这里有几个常量完成这件事

内核高地址的基准常量 [`KERNEL_BASE`](../os/src/config/mm.rs#L6)，它用于转换内核中数据的物理地址和虚拟地址

```text
KERNEL_BASE = 0xffff_ffc0_0000_0000
```

内核镜像本身在编译时产生的地址应为高地址，这样在激活页表后，可以正常运行内核。这个部分写在链接脚本里的 [`BASE_ADDRESS`](../os/src/linker.ld#L3) 

```text
BASE_ADDRESS = 0xffffffc080200000
```

内核地址空间的创建：[`MemorySet::new_kernel`](../os/src/mm/memory_set.rs#L132) 。它不是一次性粗暴映射，而是按段加入。这些段都是链接脚本中暴露的符号，链接脚本负责把内核镜像切成 `.text`、`.rodata`、`.data`、`.bss` 等段，根据这些符号对应的地址我们可以访问对应的数据并将映射关系填充到页表：

- `.text`：[`stext..etext`](../os/src/mm/memory_set.rs#L138)，权限为 `READ | EXECUTE`；
- `.rodata`：[`srodata..erodata`](../os/src/mm/memory_set.rs#L148)，权限为 `READ`；
- `.data`：[`sdata..edata`](../os/src/mm/memory_set.rs#L158)，权限为 `READ | WRITE`；
- `.bss` 和启动栈：[`sbss_with_stack..ebss`](../os/src/mm/memory_set.rs#L168)，权限为 `READ | WRITE`；
- 内核剩余可用区域：[`ekernel..KERNEL_BASE + MEMORY_END`](../os/src/mm/memory_set.rs#L178)，权限为 `READ | WRITE`。

## 第四部分：用户地址空间的大致形状

用户程序地址空间由 [`MemorySet::from_elf_data`](../os/src/mm/memory_set.rs#L193) 创建。它做的事情可以按“加载程序 + 放置用户栈”理解：

1. 先通过 [`from_kernel_page_table`](../os/src/mm/memory_set.rs#L122) 创建一个带内核高地址映射的新地址空间，这样的设计使得发生异常时，可以直接切换到内核运行，同时内核也可以直接访问用户空间的数据；
2. 解析 ELF 文件，把每个 `Load` 段加入用户地址空间，这主要依靠外部库进行解析。这部分可能有些抽象因为没有显示展示各段的映射逻辑，实际上一个用户程序也和内核程序一样在编译后会有`.text`、`.rodata`、`.data`、`.bss` 等段，把这部分的映射和内核部分段的映射类比就行；
3. 根据 ELF 段权限设置 `READ`、`WRITE`、`EXECUTE` 和 `USER`；
4. 在最后一个 ELF 段后留一页 guard page，保证栈不溢出；
5. 再映射用户栈，大小为 [`USER_STACK_SIZE`](../os/src/config/mm.rs#L12)。

用图表示大致是：

```text
低地址
  用户 ELF Load 段：代码、只读数据、可写数据等
  未映射 guard page
  用户栈 USER_STACK_SIZE
  ...
高地址
  共享的内核高地址映射
```

这里暂时没有实现用户堆段，代码中也留下了 [`TODO: 映射堆段`](../os/src/mm/memory_set.rs#L245)

还有一个容易忽略的顺序问题：创建任务时，[`TaskControlBlock::new`](../os/src/task/task.rs#L31) 会先创建 [`KernelStack`](../os/src/task/task.rs#L34)，再创建用户地址空间。原因是 `from_kernel_page_table` 会复制当前内核高地址映射，如果内核栈还没有插入 `KERNEL_SPACE`，新任务地址空间里就可能缺少自己的内核栈映射。

## 第五部分：内核栈在高地址中的位置

每个任务都有自己的内核栈。相关常量在 [`config/mm.rs`](../os/src/config/mm.rs#L15)：

```text
KERNEL_STACK_TOP  = 0xffff_ffff_ffff_f000
KERNEL_STACK_SIZE = 15 * PAGE_SIZE
PAGE_SIZE         = 0x1000
```

内核栈顶由 [`get_kernel_stack_top`](../os/src/task/kstack.rs#L49) 计算：

```text
kernel_stack_top(pid) = KERNEL_STACK_TOP - pid * (KERNEL_STACK_SIZE + PAGE_SIZE)
```

这意味着内核栈从高地址向低地址排列，每两个栈之间留出一页未映射空间作为 guard page。`KernelStack::new` 会调用 [`insert_stack_area`](../os/src/task/kstack.rs#L22)，把这段内核栈插入 `KERNEL_SPACE`；`KernelStack` 被释放时，`Drop` 会调用 [`remove_stack_area`](../os/src/task/kstack.rs#L40) 移除映射。

内核栈的插入本质上也是 `MapType::Framed`：[`insert_stack_area`](../os/src/mm/memory_set.rs#L58) 使用 `Framed` 和 `READ | WRITE` 权限，所以内核栈实际占用的物理页帧也来自页帧分配器。

## 第六部分：内核访问用户数据的接口

这一部分先留空，但这部分主要解决这些问题

- 用户指针如何被内核安全读取；
- 跨页用户缓冲区如何处理；
- 访问不同类型的数据如何封装；
- 系统调用如何和这些接口配合。

当前主分支里 [`page_table.rs`](../os/src/mm/page_table.rs#L231) 之后保留了旧的 `translate_byte_buffer`、`translate_str`、`translated_refmut` 注释代码，他们是原来的实现

## 第七部分：页帧分配器

页帧分配器在 [`frame_allocator.rs`](../os/src/mm/frame_allocator.rs#L1) 中实现。它管理的是固定大小的物理页帧，页大小为 [`PAGE_SIZE = 0x1000`](../os/src/config/mm.rs#L21)。

初始化入口是 [`init_frame_allocator`](../os/src/mm/frame_allocator.rs#L115)。它把可分配范围设置为：

```text
ekernel 去掉 KERNEL_BASE 后向上取整
    到
MEMORY_END 向下取整
```

也就是说，内核镜像本身占用的物理内存不会再被分配；页帧分配器只管理内核镜像结束之后、物理内存上界之前的页帧。

当前具体实现是 [`StackFrameAllocator`](../os/src/mm/frame_allocator.rs#L64)，这个结构体内部的变量有点唬人，但其实分配原则就是**栈的形式**管理页帧，优先从栈顶拿取页帧（物理页号），结束使用的页帧（物理页号）放回栈顶。

对外使用时一般不直接拿 `PhysPageNum`，而是通过 [`frame_alloc`](../os/src/mm/frame_allocator.rs#L125) 得到 [`FrameTracker`](../os/src/mm/frame_allocator.rs#L20)。`FrameTracker` 有两个关键效果：

- 创建时调用 [`clear`](../os/src/mm/frame_allocator.rs#L39) 清空整个页帧，避免旧数据残留；
- 生命周期结束时通过 [`Drop`](../os/src/mm/frame_allocator.rs#L46) 自动归还页帧。

所以这里可以强调 Rust 风格的资源管理（**RAII**）：页帧不是“申请后随手记得释放”，而是绑定到 `FrameTracker` 这个对象；对象离开生命周期，页帧自动回收。

页帧分配器的典型使用点包括：

- [`PageTable::new`](../os/src/mm/page_table.rs#L20)：创建根页表页；
- [`find_pte_create`](../os/src/mm/page_table.rs#L58)：缺少中间页表页时分配新页表页；
- [`MapArea::map_one`](../os/src/mm/memory_set.rs#L367)：`Framed` 数据页分配；
- [`insert_stack_area`](../os/src/mm/memory_set.rs#L58)：任务内核栈页分配；
- [`from_elf_data`](../os/src/mm/memory_set.rs#L193)：用户 ELF 段和用户栈页分配。

## 第八部分：内核堆分配器

这部分没啥内容，不用看，只要知道我们内核可以使用堆数据是因为在这里进行了实现。
