# RespOS 当前状态

本文件是快速变化的状态页。更新测试结论时必须同时更新日期、提交和命令。

## 基线

### 当前开发提交

- 状态：已确认
- 适用范围：本地 `dev`
- 最后验证：2026-08-01
- 证据：`git log -1`、`git status --short --branch`
- 内容：`dev` 指向 `44430df`（`fix(integration): tighten mmap and unlink lifecycle`），包含
  A/B/C 整合。tracked 工作树在本轮文档工作前无业务源码修改。
- 后续影响：后续结果必须注明是否仍基于该提交；新增 Codex 文档尚未提交。

## 2026-08-01 双架构完整运行

### release 构建、启动和关机

- 状态：已确认
- 适用范围：RV64/LA64，单核，默认 256 MiB 比赛镜像
- 最后验证：2026-08-01
- 证据：`make rv` → `rv-output.txt`；`make la` → `la-output.txt`
- 内容：两条命令都成功构建 user/kernel、启动 QEMU、运行到 testrunner 结束并主动关机；外层
  make 退出码均为 0。未观察到 kernel panic 或整机死锁。
- 后续影响：这只证明运行流程完成，不代表测试通过；结果必须从日志内的分组和 summary 判定。

### LTP 当前被 writable file `MAP_SHARED` 阻断

- 状态：已确认
- 适用范围：musl/glibc LTP，RV64/LA64
- 最后验证：2026-08-01
- 证据：`os/src/fs/file.rs::FileOp::mmap_allowed`；两份运行日志中的
  `mmap(... PROT_READ | PROT_WRITE, MAP_SHARED, fd=3 ...) failed: EOPNOTSUPP`
- 内容：LTP 框架建立控制页时失败，绝大多数用例在测试主体之前 `TBROK`：

| 架构 | libc | passed | failed | skipped | selected |
| --- | --- | ---: | ---: | ---: | ---: |
| RV64 | musl | 18 | 664 | 17 | 699 |
| RV64 | glibc | 18 | 667 | 14 | 699 |
| LA64 | musl | 18 | 665 | 16 | 699 |
| LA64 | glibc | 18 | 668 | 13 | 699 |

- 后续影响：不能用这轮数字评价几百个 syscall 的真实语义；首要任务是实现安全的 writable
  shared file mapping/writeback 协议，再重跑完整 LTP。

### basic 与 lmbench 的 mmap 回归

- 状态：已确认
- 适用范围：两架构、两种 libc
- 最后验证：2026-08-01
- 证据：`rv-output.txt`、`la-output.txt`
- 内容：basic 的文件 mmap/munmap 失败，munmap 返回 `-EINVAL`；RV mmap 测试出现用户态
  segmentation fault。lmbench musl/glibc 均报告 mmap/msync 不支持。
- 后续影响：这是跨架构策略回归，不是单架构偶发故障。

### 其余工作负载可运行但尚非全绿

- 状态：已确认
- 适用范围：当前完整日志
- 最后验证：2026-08-01
- 证据：`rv-output.txt`、`la-output.txt`
- 内容：BusyBox、libcbench、Lua 和 iozone 均运行到分组结束，日志未显示内核崩溃；但
  libctest 的 static/dynamic wrapper 在两架构均报告退出码 256。网络存在非确定性建连失败：
  RV 的 iperf musl parallel TCP 与 netperf musl UDP_STREAM 各失败一次，LA 的 iperf glibc
  parallel TCP 失败一次。
- 后续影响：libctest 返回值要单独定位；网络项应增加服务端 ready/retry 诊断后再判断内核缺陷。

## A/B/C 整合时的专项验证

### 代表性 runtime/MM/FS probe 曾通过双架构

- 状态：已确认（历史集成证据）
- 适用范围：提交 `44430df` 形成前后的整合验证
- 最后验证：2026-08-01
- 证据：`docs/四天内核重构-ABC-整合审查.md`、Git `2f736d4`/`e0d69fd`/`cba8e24`
- 内容：文档记录双架构 debug/release build、MM split/invariant、task A probe、FS Day2～Day4
  回归通过；futex cmp-requeue 使用专项强制竞态构建验证，之后恢复默认构建。
- 后续影响：这些结果证明合并接口未明显破坏专项语义，但不能替代今天被阻断的完整 LTP。

## 历史信息与待验证项

### README 的“LTP 600 余项”是历史状态

- 状态：已过期
- 适用范围：README 所描述的早期稳定版本
- 最后验证：2026-08-01
- 证据：`README.md`；当前完整 RV/LA 日志与其不一致
- 内容：项目历史上曾记录本地 LTP 可通过 600 余项、评测稳定版本约 2350 分。这些数字不能
  代表当前 `dev`。
- 后续影响：发布说明或汇报引用成绩时必须注明对应 commit、镜像和测试日期。

### 尚未完成的高风险验证

- 状态：待验证
- 适用范围：进入 main 前的回归门禁
- 最后验证：2026-08-01
- 证据：`docs/四天内核重构-ABC-整合审查.md`
- 内容：真实 SMP；MAP_FIXED/mremap 极端 ENOMEM 回滚；truncate 与 resident mapped page；
  rename+多硬链接事务；epoll 跨进程最后关闭；pipe/poll/epoll 与 close/signal/timeout 联合竞争。
- 后续影响：这些问题不能因当前 QEMU 完成整轮运行而视为关闭。

## 工作区注意事项

### 本地存在刻意未提交的资料和回归程序

- 状态：已确认
- 适用范围：当前容器工作区
- 最后验证：2026-08-01
- 证据：`git status --short`
- 内容：两份 `docs/RespOS*.md` 与 `user/src/bin/fs_day{2,3,4}_*.rs` 未跟踪；此前约定不纳入
  提交。本目录新增文件也将保持未提交，直到维护者主动审查。
- 后续影响：`user/Makefile` 会通过 wildcard 构建所有 `user/src/bin/*.rs`，所以未跟踪测试仍会
  影响本地构建产物；复现实验时要记录它们是否存在。
