# RespOS 设计决策记录

这里只收录能解释当前代码形态或避免重复踩坑的决策。日期是当前证据最后核验时间，不一定是
最初提出时间。

## 共享高半区内核模型

- 状态：已确认
- 适用范围：RV64/LA64 虚拟内存、启动、linker、页表
- 最后验证：2026-08-01
- 证据：`os/src/linker_riscv.ld`、`os/src/linker_loongarch.ld`、
  `os/src/arch/*/config/mm.rs`、`os/src/main.rs`
- 内容：两个架构保持 `0xffffffc0...` 高半区共享内核布局。LoongArch 从低物理入口建立早期
  分页再跳转高半区，不采用与公共内核假设割裂的独立低地址模型。
- 后续影响：LoongArch 修复应尽量收敛在 arch 层，不能未经全局审计改变公共地址空间模型。

## syscall 保持薄层，领域对象拥有状态机

- 状态：已确认
- 适用范围：MM、task、FS、net syscall
- 最后验证：2026-08-01
- 证据：`os/src/syscall/mm.rs`、`os/src/mm/memory_set.rs`、Git `15fe1a5`；A/B/C 文档
- 内容：syscall 负责 ABI 参数解析、fd/用户指针获取和 errno 映射；VMA、调度、namei、socket
  等状态转换由领域模块维护。
- 后续影响：发现 syscall 文件重复维护缓存、映射表或对象生命周期时，应优先下沉到所有者。

## 用户指针通过页表逐页访问

- 状态：已确认
- 适用范围：copyin/copyout、字符串、iovec、futex 预检查
- 最后验证：2026-08-01
- 证据：`os/src/mm/mod.rs`、Git `15fe1a5`
- 内容：用户地址可能跨页、跨相邻 VMA、处于 lazy 或 COW 状态。通用 copy 先检查整段权限并
  确保页可访问，再通过 PTE 对应 frame 复制，避免 kernel page fault。
- 后续影响：不要在 syscall 中直接构造用户 slice 或逐字节裸解引用。

## 高风险状态修改采用 prepare/copyout/commit

- 状态：已确认
- 适用范围：wait4、shmat、timer、prlimit、rename 等可失败操作
- 最后验证：2026-08-01
- 证据：Git `3aa1fb5`、`15fe1a5`、`cba8e24`；
  `docs/四天内核重构-ABC-整合审查.md`
- 内容：先验证并准备资源，用户 copyout 成功后再提交不可逆状态；失败不能重复累计、提前删除
  zombie、破坏旧映射或泄露半成品对象。
- 后续影响：review 时要逐项列出失败点和其前后的可见状态，不能只检查成功路径。

## descriptor flags 与 open-file status flags 分离

- 状态：已确认
- 适用范围：fdtable、dup、fcntl、exec、CLOEXEC
- 最后验证：2026-08-01
- 证据：`os/src/fs/fdtable.rs`、`os/src/fs/file.rs`、Git `cba8e24`
- 内容：CLOEXEC 属于每个 `FdEntry`；offset 和 O_APPEND/O_NONBLOCK 等 open-file 状态属于共享
  `File`。dup 共享后者，不共享前者。
- 后续影响：任何 fd 复制、进程复制和 fcntl 实现都必须保持这一身份关系。

## 未实现状态型 ABI 必须诚实失败

- 状态：已确认
- 适用范围：mmap/madvise/special fd/fs flags 等
- 最后验证：2026-08-01
- 证据：A/B/C 重构文档、Git `cba8e24`/`15fe1a5`
- 内容：会改变长期状态或让用户依赖后续语义的能力，未实现时返回明确 errno，不能无条件 0。
  纯 hint/no-op 只有在 ABI 允许且完成参数校验时才可受限接受。
- 后续影响：恢复旧测例不能以重新引入假成功为代价；应修实现或证明用例依赖非目标行为。

## 暂时拒绝 writable file `MAP_SHARED`

- 状态：已确认，但需要替换
- 适用范围：文件 mmap/msync/munmap/writeback
- 最后验证：2026-08-01
- 证据：`os/src/fs/file.rs::mmap_allowed`、`os/src/mm/memory_set.rs`、Git `cba8e24`；
  当前 RV/LA 完整日志
- 内容：为隔离持有 `MemorySet` 写锁执行后端 I/O、munmap 吞写回错误等风险，当前明确返回
  `EOPNOTSUPP`。该决定提高了 ABI 诚实性，但阻断 basic、lmbench 和 LTP 框架，不能作为长期
  完成状态。
- 后续影响：正确替代方案是设计锁外 prepare/writeback/commit、错误传播和 truncate 失效
  契约；不能仅删除检查后恢复旧的隐患实现。

## 关键回归必须双架构并分析日志

- 状态：已确认
- 适用范围：进入集成/main 的测试门禁
- 最后验证：2026-08-01
- 证据：顶层 `Makefile`、A/B/C 验收文档、当前 `make rv`/`make la` 结果
- 内容：构建门禁至少覆盖 RV/LA；运行门禁使用仓库根目录真实入口。QEMU 正常关机和 make 退出
  0 只表示 runner 结束，必须解析测试 summary 和失败标记。
- 后续影响：提交说明应分别陈述 build、boot、专项 probe、完整 workload，不使用笼统“测试通过”。

## 优化时保持原有有效测例兼容

- 状态：暂定
- 适用范围：`dev` 后续重构
- 最后验证：2026-08-01
- 证据：当前维护目标；尚待后续提交和回归矩阵固化
- 内容：结构优化应尽量保持已有有效 ABI 测例；如果旧测例依赖取巧实现，可以排除，但必须
  给出源码级原因和替代验证，不能仅因失败就标记为“无效”。
- 后续影响：下一阶段先恢复 LTP harness 可运行性，再以真实失败驱动窄范围修复。
