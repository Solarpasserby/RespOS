# C 组 Day3：锁、缓存与路径一致性

## 1. 完成状态

Day3 已完成可在 C 组边界内独立闭环的三条主线：

1. positioned I/O 与 `fsync` 不再持有 `FileInner` 自旋锁执行 page-cache/ext4 慢路径；
2. rename/unlink/create 的 dentry、打开文件和后端路径 identity 得到收紧；
3. page-cache truncate/writeback 增加长度代次保护，并完成双架构生命周期回归和基准。

A 组 waiter/scheduler 与 B 组文件 mmap/VMA 接口尚未在当前工作树提供稳定契约，因此相关
跨组项目仍保留为显式边界，没有用 C 组私有接口越界实现。

## 2. 持锁慢操作盘点

| 锁 | 原锁内操作 | Day3 处理 | 当前边界 |
| --- | --- | --- | --- |
| `FileInner` | `read_at_offset` 触发 page load/ext4 read | 锁内快照 path/cache/write-back，锁外读取 | 已修复 |
| `FileInner` | `write_at_offset` 触发 page load、同步写回和时间戳更新 | 锁内快照，锁外写入/flush，最后短锁提交时间覆盖 | 已修复 |
| `FileInner` | `fsync` 执行 page-cache writeback 和 superblock flush | 锁内快照 cache/path/superblock，锁外完整同步 | 已修复 |
| Page map/Page | dirty page writeback | 复制页数据和 `write_version` 后释放页锁，再执行 inode I/O | 原有正向基线继续保持 |
| `NAMEI_MUTATION_LOCK` | ext4 create/unlink/rename | 后端成功后才提交 dentry cache；覆盖 rename 使用可回滚暂存 | 仍串行化 namespace mutation |
| `FileInner` | 普通 `read/write/truncate` | 暂未缩短 | offset、append、truncate 需要新的 inode/open-description 排序原语后才能安全拆锁 |

这里没有为了缩短临界区牺牲共享 offset 或 append 原子语义。普通 read/write 的进一步拆锁需要
把 offset 保留/提交和 inode 级 size mutation 分离，属于后续结构性优化。

## 3. rename、dentry 与打开文件 identity

### 3.1 后端提交与失败回滚

- `RENAME_NOREPLACE` 现在在目标存在时返回 `EEXIST`，不再被静默忽略；
- 覆盖普通文件时，先将目标移动到 inode orphan 名字；源 rename 失败时恢复原目标；
- 覆盖空目录时，先移动到备份名字；源 rename 失败时恢复；
- dentry tree/cache 只在后端源 rename 成功后提交；
- rename 已成功后，备份清理失败不会再反向报告 syscall 失败；
- ext4 inode 记录可用后端路径，rename 前已打开的 File 可继续读、写和 fsync；
- orphan 清理由 inode 级打开 File 计数控制，独立打开同一 inode 时不会因先关闭一个 File 而过早删除。

### 3.2 dentry identity

rename 不再为源对象创建替代 dentry，而是保留原 `Arc<Dentry>` 并更新父链/名字别名。
`Path::abs_path()` 和全局路径通过当前父链计算，因此：

- 打开的 File 保持原 dentry/inode identity；
- cwd 指向被 rename 的目录时，`getcwd` 返回新路径；
- 在已 rename cwd 下继续进行相对 create/lookup 可使用新后端路径；
- 子 dentry 自动继承父目录的新路径前缀。

### 3.3 固定生命周期序列

用户态回归覆盖了：

1. create → lookup → open；
2. create → unlink → create same name，并验证新内容；
3. rename old/new，旧路径 `ENOENT`；
4. open → rename/unlink → 原 fd 继续读写与 fsync；
5. close → reopen；
6. hard link → unlink 一个名字 → 另一个名字继续可用；
7. symlink follow 与 readlink no-follow；
8. `RENAME_NOREPLACE` 和覆盖已有普通文件；
9. cwd 目录 rename → getcwd → 相对 create → 新绝对路径 lookup。

挂载点附近 `..` 已有 namei/mount crossing 实现，但本次没有在可销毁的独立挂载镜像上加入 mutation
回归，因此不把固定序列第 10 项宣称为完成。

## 4. page cache / writeback 不变量

- dirty page 仍以 `write_version` 快照判断是否可以清 dirty；
- inode 写回失败通过 `?` 返回，清 dirty 发生在成功完整写入之后；
- `fsync` 遍历目标 cache 的所有 dirty page；
- `fdatasync` 当前采用比最低要求更强的完整 `fsync` fallback，不假装提供独立元数据等级；
- truncate 缩小时移除 EOF 外页面并清零尾页不可见区域；
- 尾页清零递增 `write_version`，使旧写回快照失效；
- page cache 增加 `size_version`。写回若与 truncate/extend 竞争且越过新 EOF，会恢复当前长度并保留 dirty 状态；
- truncate 扩展区域读取为零；
- cache 挂在 inode 上，关闭一个 File 不会丢弃其他 File 仍共享的 cache。

文件 mmap 的 read/write 一致性、msync/munmap 写回责任和 truncate 映射页处理仍等待 B 组 VMA
快照/回调契约；当前 `msync` 继续明确返回 `EOPNOTSUPP`。

## 5. 回归与基准

回归程序：`user/src/bin/fs_day3_regression.rs`。

| 架构 | debug build | QEMU Day3 lifecycle | Day2 防回退 |
| --- | --- | --- | --- |
| RISC-V | PASS | PASS | Day3 覆盖 positioned/fsync 主路径；此前完整 Day2 PASS |
| LoongArch | PASS | PASS | 完整 `fs_day2_io_regression` PASS |

最终一轮 QEMU 基准，单位为毫秒：

| 架构 | 字节数 | 顺序写 | fsync 后首次读 | cache hit 再读 |
| --- | ---: | ---: | ---: | ---: |
| RISC-V | 4 KiB | 1 | 1 | 0 |
| RISC-V | 64 KiB | 6 | 4 | 4 |
| RISC-V | 1 MiB | 191 | 44 | 63 |
| LoongArch | 4 KiB | 4 | 1 | 1 |
| LoongArch | 64 KiB | 4 | 3 | 2 |
| LoongArch | 1 MiB | 145 | 26 | 24 |

数据表明当前 syscall/page 循环在 1 MiB 顺序写上仍是明显热点，但 cache-hit 读取没有出现数量级异常。
Day3 没有在缺少修改前同配置基线的情况下声称性能提升，也没有引入直接借用用户页等高风险优化；
这组数据作为 Day4/后续优化基线。

## 6. 尚未收口的边界

- A：pipe/poll waiter 的 blocked 顺序、signal interrupt 和 close 唤醒需要 scheduler owner 契约；
- B：file mmap backing offset、shared dirty、msync/munmap 和 truncate mapped-page 需要 VMA owner 契约；
- 普通共享-offset read/write 仍持 `FileInner` 跨 I/O，需先引入不会破坏 append/offset 的排序模型；
- 覆盖多硬链接目标时仍沿用后端 unlink 路径，完整的“任意别名都可作为 inode 后端句柄”需要 inode-handle 化，超出四天方案范围；
- 当前没有后台 writeback 线程，也不承诺崩溃一致性。
