= 12. 总结与展望
<12-总结与展望>
#quote(block: true)[
本章回答：RespOS 的最终贡献是什么？当前限制有哪些？如何复现？未来方向是什么？
]

== 12.1 本队最终贡献摘要
<121-本队最终贡献摘要>
【待人工填写------从前面各章提炼 3-6 条核心贡献，每条绑定代码路径和测试状态】

#quote(block: true)[
【建议写什么】

- 双架构 HAL 抽象层的完整实现（RISC-V64 + LoongArch64）
- 统一进程/线程模型的 TCB 设计，含细粒度字段分类、SMP 多核支持、exec sibling quiescence 协议
- 完整虚拟地址空间模型（MemorySet→VMA→PTE），含 COW、lazy allocation、file-backed ELF、FDT 动态内存识别
- 类 Linux VFS 架构，含 dentry 缓存、页缓存、挂载子系统、ext4 适配
- 每条贡献绑定代码路径和测试证据

【建议准备的表】贡献---代码---证据---状态摘要表
]

== 12.2 当前限制与未完成项
<122-当前限制与未完成项>
内核的已知边界按模块列出：

#figure(
  align(center)[#table(
    columns: 2,
    align: (auto,auto,),
    table.header([模块], [当前限制],),
    table.hline(),
    [进程管理], [vfork 地址空间共享语义未完整实现（仅有同步语义）；非 leader exec 未支持；LA64 SMP 未经过动态验证；动态链接器仍为 eager load],
    [内存管理], [无 swap 和页面回收机制；LA64 RAM 上限仍为静态配置；私有文件页 fault 的锁外 I/O 未完成；`MS_INVALIDATE` 与 `SIGBUS` 语义不完整],
    [文件系统], [单 ext4 超块实例，无多设备支持；写密集场景下 NAMEI\_MUTATION\_LOCK 串行化；全路径存储 dentry 模型 rename 和缓存淘汰有额外开销；页缓存使用 Vec 而非物理页框，mmap 路径有多余拷贝],
    [信号系统], [`SA_RESTART` 未实现；实时信号排队不完整（同种信号连续到达时合并）；Core dump 未实现],
    [时钟模块], [线程 CPU clock、TAI、wakeup alarm 未支持；`times()` 的用户/系统时间仅为近似记账],
    [网络模块], [TCP 重传/拥塞控制依赖 smoltcp 内部实现；物理网卡驱动未完成；select 系统调用未实现],
    [IPC], [消息队列和 semaphore 未实现；跨 shmat 的 futex key 未统一],
    [测试], [BuildStorm 完整 minibuild/compile 未通过；LA64 多核测试覆盖不足],
  )]
  , kind: table
  )

#quote(block: true)[
【建议准备的表】已验证/部分验证/未实现/待验证四分类表

【容易出现的问题】不能用\"理论上支持\"填入已验证列；限制不是道歉段落，而是结果可解释性的一部分
]

== 12.3 测试结果与性能数据
<123-测试结果与性能数据>
【待人工填写------补充最终测试数据】

#quote(block: true)[
【建议写什么】

- LTP 等测例通过情况（按测试层级分组：basic / busybox / libc / LTP / CAgent / 自研 probe）
- SMP 并发压力测试结果（按 SMP=1/2/4/8、debug/release、256M/8G 分列）
- 关键性能对比数据（如与前版本或参考内核的对比）

【建议准备的表】测试层级---测例数---通过数---主要失败原因表
]

== 12.4 复现入口
<124-复现入口>
【待人工填写------最终提交版本确认后补充】

#quote(block: true)[
【建议写什么】

- 构建命令：`make all` 或 `make build-rv` / `make build-la`
- 镜像准备：`bash scripts/get_img.sh`
- 运行命令：`make rv` / `make la`
- 日志路径：`rv-output.txt` / `la-output.txt`
]

== 12.5 未来工作
<125-未来工作>
基于当前限制（§12.2），下一步工作的优先级排序：

+ #strong[BuildStorm 完整通过]。当前最重要的未闭环问题，涉及 pipe 引用生命周期、wait/wakeup 竞争、exit 延迟回收的综合调试
+ #strong[LA64 多核验证]。当前 LA64 仍走单核路径，需要完成 LA64 的 SMP 启动、IPI 和共享 MM 验证
+ #strong[内存管理完善]。swap / 页面回收 / 动态链接器 file-backed 加载
+ #strong[性能优化]。per-CPU run queue、锁竞争优化、页缓存零拷贝 mmap
+ #strong[功能补全]。`SA_RESTART`、消息队列、物理网卡驱动

#quote(block: true)[
【建议写什么】每项未来工作必须与 §12.2 的限制一一对应；区分\"已计划\"与\"方向性建议\"
]
