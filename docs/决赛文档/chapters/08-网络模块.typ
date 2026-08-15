= 8. 网络模块
<8-网络模块>
#quote(block: true)[
本章介绍 smoltcp 与 Linux socket ABI 的连接方式，以及 TCP、UDP、UNIX 域 socket 和并发 `listen/accept` 的实现。
]

RespOS 采用"内核管理 ABI、对象生命周期和阻塞语义，smoltcp 管理 TCP/IP 协议状态机"的分层设计。这一边界使项目无需重新实现 TCP 重传、滑动窗口和报文解析，而能把工程重点放在用户态真正可见的部分：fd 身份、`bind/connect/listen/accept`、阻塞与信号中断、backlog 并发容量、`poll/epoll` 就绪性以及 close 时的句柄回收。

当前数据面明确聚焦于本地回环通信。内核已实现 IPv4 loopback 上的 TCP/UDP，并为 `AF_INET6` 的 `::`/`::1` 提供回环 ABI 兼容，同时实现基于内存队列的 UNIX 域 socket。源码中没有将 virtio-net 设备接入 `Interface`，因此本章不将真实以太网收发或物理网卡列为已完成能力。

== 8.1 分层架构与责任边界
<81-分层架构与责任边界>
RespOS 从 syscall 到数据包共分为五层。如图 8-1 所示，`syscall/net.rs` 负责 Linux ABI 结构解析和用户拷贝，`Socket` 负责 fd/VFS 统一抽象，`TcpSocket`/`UdpSocket` 管理内核可见状态，smoltcp 管理协议细节，`LoopbackDev` 则完成纯软件收发。

```text
用户程序：socket / bind / connect / listen / accept / read / write / poll
                                  ↓
os/src/syscall/net.rs：sockaddr、msghdr/iovec、sockopt、copyin/copyout、errno
                                  ↓
Socket : FileOp：fd 身份、O_NONBLOCK、read/write/readiness
                 ├─ TcpSocket → ListenTable
                 ├─ UdpSocket
                 └─ UnixSocket（内存队列）
                         ↓
            smoltcp SocketSet + Interface::poll()
                         ↓
                 LoopbackDev（127.0.0.1）
```

#strong[图 8-1 RespOS 网络模块分层调用链]

表 8-1 进一步明确各层拥有的状态，以及不应越界处理的内容。

#strong[表 8-1 网络各层的所有者与职责]

#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([层次], [主要对象], [负责的状态], [不在该层处理的内容],),
    table.hline(),
    [syscall ABI], [`SockAddrIn/SockAddrIn6/SockAddrUn`、`MsgHdr`、`MMsgHdr`], [参数长度、字节序、用户指针、fd 分配与 errno], [TCP 状态转换、句柄回收],
    [fd/VFS 抽象], [`Socket : FileOp`], [domain/kind、非阻塞状态、统一 read/write/readiness], [TCP 报文和 UDP 队列算法],
    [传输层适配], [`TcpSocket`、`UdpSocket`、`ListenTable`], [连接粗粒度状态、本地/对端地址、backlog、阻塞等待], [拥塞控制、重传算法],
    [协议栈], [smoltcp `SocketSet`、TCP/UDP socket], [TCP/IP 协议状态机、收发缓冲、报文处理], [Linux fd 和进程语义],
    [设备], [`LoopbackDev`], [待接收数据包队列和可复用缓冲池], [真实 NIC 中断、DMA 和 virtqueue],
  )]
  , kind: table
  )

这样分层后，更换协议库或增加真实网卡时，应优先保持 `Socket : FileOp` 和 syscall ABI 层不变；而修改 `accept` 并发性时，则必须同时审计 `ListenTable`、smoltcp handle 和 fd 的唯一所有权。

== 8.2 `Socket` 对象与 fd 生命周期
<82-socket-对象与-fd-生命周期>
RespOS 通过让 `Socket` 实现 `FileOp` 将套接字纳入统一 fd 模型。这使 `read/write/close/fcntl/ppoll/epoll` 无需在文件和 socket 之间维护两套描述符表；对用户态而言，socket fd 与普通 fd 遵循相同的分配、复制和关闭边界，但通过 `ESPIPE` 明确拒绝 seek。

#strong[代码片段 8-1 用户可见 socket 的协议分派]

```rust
// os/src/net/socket.rs（节选）
enum SocketInner {
    Tcp(TcpSocket),
    Udp(UdpSocket),
    Unix(UnixSocket),
}

pub struct Socket {
    pub domain: SocketDomain,
    pub kind: SocketKind,
    inner: SocketInner,
    nonblock: AtomicBool,
    cloexec: AtomicBool,
    send_buf_size: AtomicU64,
    recv_buf_size: AtomicU64,
}

impl FileOp for Socket {
    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> { /* 按协议分派 */ }
    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> { /* 按协议分派 */ }
    fn read_ready(&self) -> bool { self.tcp_poll(true) }
    fn write_ready(&self) -> bool { self.tcp_poll(false) }
}
```

代码片段 8-1 中的 `Socket` 是 open-file description：`dup` 或 `fork` 后的 fd 可以共享同一 socket 对象和连接状态。`SOCK_NONBLOCK` 在创建时传入对象，后续 `fcntl` 也经 `FileOp::set_status_flags` 更新相同状态。`SOCK_CLOEXEC` 进入 fd 旗标语义，使 `exec` 能经统一 fd table 路径关闭描述符。

socket 对象的创建与用户 copyout 还遵循失败原子性。例如 `socketpair` 会先检查用户结果数组，再依次分配两个 fd；第二个 fd 分配或最终 copyout 失败时，已分配的 fd 会被回收。`accept4` 创建新 socket 并分配 fd 后，若写回对端地址失败，也会关闭新 fd，不向进程留下不可达的连接对象。

== 8.3 回环设备与协议轮询
<83-回环设备与协议轮询>
RespOS 将回环通信实现为符合 smoltcp `Device` trait 的纯软件设备。`LoopbackDev` 不经过 virtqueue 或 MMIO：发送 token 从缓冲池取出 `Vec<u8>`，smoltcp 填充报文后将其推入接收队列；下一次 interface poll 再通过接收 token 取出同一缓冲。接收 token 销毁时将缓冲归还池中，池最多保留 64 个缓冲，避免高频回环报文不断重新分配堆对象。

```text
smoltcp TxToken::consume
      → 从 pool 取缓冲/按需分配
      → 填充 IP + TCP/UDP 报文
      → 进入 LoopbackDev.queue
      → Interface::poll()
      → RxToken::consume
      → 投递到对端 smoltcp socket
      → RxToken drop，缓冲返回 pool
```

#strong[图 8-2 回环设备的纯软件数据路径]

图 8-2 中的全局 `SocketSet`、loopback `Interface`、`LoopbackDev` 和 `ListenTable` 均由锁保护。`poll_interfaces()` 以当前内核时间驱动 `Interface::poll()`，使 smoltcp 处理新报文、TCP 状态转换和缓冲投递；随后立即把已完成握手的 listener handle 转入 accept queue 并补充新 listener。该 poll 入口由 TCP/UDP 阻塞循环、socket readiness 查询和 `/proc/net/tcp` 生成路径调用，因而当前协议栈是协作式驱动，而不是由独立 NIC 中断线程推进。

== 8.4 TCP 套接字与并发监听
<84-tcp-套接字与并发监听>
RespOS 在 smoltcp TCP 状态机之上增加了面向 syscall 并发操作的粗粒度状态。`CLOSED`、`CONNECTING`、`CONNECTED` 和 `LISTENING` 表达用户可见生命周期，`BUSY` 则是临时 CAS 中间态，用于防止同一 socket 被两条路径同时执行不可并发的状态转换。

#strong[代码片段 8-2 TCP 粗粒度状态的 CAS 提交]

```rust
// os/src/net/tcp.rs（节选）
fn update_state<F, T>(&self, current: u8, new: u8, f: F) -> Result<T, Errno>
where
    F: FnOnce() -> Result<T, Errno>,
{
    match self
        .state
        .compare_exchange(current, STATE_BUSY, Ordering::Acquire, Ordering::Acquire)
    {
        Ok(_) => {
            let res = f();
            if res.is_ok() {
                self.set_state(new);
            } else {
                self.set_state(current);
            }
            res
        }
        Err(_old) => Err(Errno::EISCONN),
    }
}
```

代码片段 8-2 的关键在于失败时恢复原状态。`bind`、`connect`、`listen`、`close` 都通过同一入口提交粗粒度转换，从而避免协议句柄已创建而用户状态仍留在另一阶段。

```text
客户端：socket → [bind] → connect → CONNECTING
                                      ↓ poll
                                  CONNECTED → send/recv → shutdown/close

服务端：socket → [bind] → listen → LISTENING
                                      ↓
                              accept → 新 CONNECTED socket
```

#strong[图 8-3 TCP 客户端与服务端生命周期]

图 8-3 展示的状态转换由 syscall 操作触发，而协议握手的实际推进依赖下一节所述的协作式轮询。

=== 8.4.1 阻塞、非阻塞与信号中断
<841-阻塞非阻塞与信号中断>
TCP 的 `connect/accept/send/recv` 共用 `block_on` 模式：先调用 `poll_interfaces()` 推进协议栈，再尝试一次操作；如果返回 `EAGAIN`，非阻塞 socket 立即向用户返回，阻塞 socket 则将当前任务标记为可中断等待，登记约 1 ms 的短期 timeout 后切换任务，醒来再重试。

之所以不在 syscall 中持续忙轮询，是因为 listener 等待第一个连接时可能暂时没有其他 ready 任务；如果内核一直在 TCP 循环中占用 CPU，调度器 idle 路径无法推进 `sleep/timeout`，无关定时任务也会被饥饿。短期阻塞让出 CPU，同时保留轮询式协议栈的实现简洁性。

等待期间会在切换前后检查 real timer、pending signal 和 `interrupted` 状态，被信号中断时返回 `EINTR`。注册 blocked 与真正切换之间的信号窗口也会二次检查，避免发送方已标记中断、但尚找不到 blocked queue 条目时产生丢失唤醒。

`shutdown(SHUT_RD)` 记录接收半边关闭，后续读取返回 EOF；`SHUT_WR` 记录发送半边关闭并调用 smoltcp `close()` 推进 FIN 路径，后续写入返回 `EPIPE`。`Drop` 是所有权最终落点：普通连接从全局 `SocketSet` 移除 handle，监听 socket 还由 `unlisten` 回收 listener 池和 accept queue 中尚未交给用户的所有 handle。

=== 8.4.2 backlog listener 池
<842-backlog-listener-池>
RespOS 不把 smoltcp 的单个 listening socket 直接等同于 Linux backlog，因为 smoltcp 中一个 listener 完成握手后会变成该条已连接 socket。在用户态调用 `accept` 前，原端口就暂时失去可承接后续 SYN 的 listener；多个 client 在同一轮 poll 中到达时，后续连接可能收到 reset。

RespOS 因此将 `listen(backlog)` 实现为受限 listener 池。backlog 被限制到 1～128，每个端口的 `ListenTableEntry` 同时维护"仍在监听/握手中"的 `listen_handles` 和"已连接、等待 accept"的 `accept_queue`。两个队列总长度不超过 backlog，既预留并发握手容量，又防止恶意大 backlog 无界占用每个 64 KiB 收发缓冲的 TCP socket。

#strong[代码片段 8-3 listener 池的容量不变量]

```rust
// os/src/net/listen.rs（节选）
fn replenish_listeners(entry: &mut ListenTableEntry) {
    while entry.listen_handles.len() + entry.accept_queue.len() < entry.queue_limit {
        let mut socket = SocketSetWrapper::new_tcp_socket();
        if socket.listen(entry.listen_endpoint).is_err() {
            return;
        }
        let handle = socket_set().lock().add(socket);
        entry.listen_handles.push_back(handle);
    }
}
```

如代码片段 8-3 所示，池中每移出一个已连接 handle，就尽快补入新 listener。这一补充不等用户态 `accept` 才发生：每次协议 poll 后，`promote_ready_listeners` 会把 `Established` handle 转入 accept queue，然后立即补位。

```text
listen(backlog=N)
      → 预建 N 个 listener handle（N 限制为 1..128）
      → Interface::poll 完成一条握手
      → Established handle 从 listen_handles 移入 accept_queue
      → 立即补建新 listener
      → accept 取出 handle，所有权转入新 TcpSocket
```

#strong[图 8-4 backlog listener 池与 accept queue 的句柄转移]

图 8-4 中的 handle 只是 `SocketSet` 索引，本身不拥有协议对象，因此每条移交路径都明确唯一所有者：监听期属于 `ListenTableEntry`，`accept` 成功后属于新 `TcpSocket`，`unlisten` 则显式 remove 两个队列中的所有剩余 handle，从而避免重复 remove 和未接受连接泄漏。

该设计来自对真实并发失败的修复。旧实现只有一个 listener，多个 client 并发连接时会随机 reset。2026-08-03 的 RV64 CAgent 回归中，修复后连续三轮不再出现 `Connection failed to 127.0.0.1:8080`，包括 network 在内的网络责任项稳定通过；当时整体 8/10 的剩余失败已由日志定位到文件系统路径，不被用来扩大网络结论。

== 8.5 UDP 与 UNIX 域 socket
<85-udp-与-unix-域-socket>
=== 8.5.1 UDP 数据报
<851-udp-数据报>
UDP 不需要 TCP 的连接状态机，但仍然管理本地端点、默认对端、端口冲突、阻塞语义和数据报边界。`UdpSocket::new` 在全局 `SocketSet` 中创建一个带 256 个 metadata slot、64 KiB 接收字节区和 64 KiB 发送字节区的 smoltcp socket。

`bind` 检查同地址/端口是否已被现有 UDP socket 占用；端口为 0 时，从 `0xc000..=0xffff` 循环取候选端口并扫描 `SocketSet` 验证冲突。因而，当前源码中没有独立 `PortManager`，端口所有权由存活的协议 socket 直接体现。

表 8-2 对比无连接与保存默认对端两种 UDP 用法。

#strong[表 8-2 UDP 两种用法的数据路径]

#figure(
  align(center)[#table(
    columns: 4,
    align: (auto,auto,auto,auto,),
    table.header([用法], [发送], [接收], [地址语义],),
    table.hline(),
    [无连接], [`sendto(buf, remote)`], [`recvfrom(buf)`], [每个数据报单独指定/返回对端],
    [已 `connect`], [`send(buf)`], [`recv(buf)`], [socket 保存默认对端，接收时过滤非匹配端点],
  )]
  , kind: table
  )

未显式 `bind` 的 socket 在首次发送或 `connect` 时自动绑定回环临时端口。`recvfrom` 先使用固定 1528 字节的内核临时缓冲接收，再按用户缓冲长度复制并写回来源地址；协议栈报告数据报超过该临时缓冲时，当前路径将 `Truncated` 转换为 `EAGAIN`，因此不能把它表述为任意长度数据报的完整接收。阻塞 UDP 会在 `EAGAIN` 时让出 CPU 并检查信号，但当前使用 cooperative yield，没有 TCP 路径的 1 ms blocked-timeout 协议。

=== 8.5.2 UNIX 域 socket
<852-unix-域-socket>
RespOS 为本机 IPC 提供不经过 smoltcp 的 UNIX 域 socket，因为 `socketpair` 和基于 pathname 的本地服务是 libc、shell 和多进程工具的常见依赖，不需要 IP 报文开销。每个 `UnixSocket` 持有一个接收字节队列和对端队列引用，单向队列容量上限为 64 KiB；`socketpair` 直接交叉连接两个对象的队列。

pathname `bind` 会通过 VFS 创建 `InodeType::Socket` 节点，同时把路径 key 注册到内核 `UNIX_LISTENERS` 表；抽象地址只使用内核 key，不创建路径节点。`listen` 为 key 建立 pending queue，`connect` 创建与客户队列交叉引用的服务端 `UnixSocket` 并推入队列，`accept` 取出该对象。pending queue 上限为 128，监听 socket 关闭时删除对应注册项。

当前 `SOCK_STREAM`、`SOCK_DGRAM` 和 `SOCK_SEQPACKET` 的 UNIX 域数据面都复用字节队列，因而已提供本地传输和 `socketpair`，但 `DGRAM/SEQPACKET` 还没有独立保留消息边界的队列语义。这是当前实现的精确边界，因此不将其表述为完整 UNIX datagram/seqpacket 实现。

== 8.6 网络系统调用与就绪性
<86-网络系统调用与就绪性>
RespOS 的网络 syscall 层保持"解析 ABI、获取 socket 对象、调用领域方法、写回结果"的边界。表 8-3 展示已接入的主要接口及其实现重点。

#strong[表 8-3 主要 socket 系统调用]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([类别], [系统调用], [RespOS 实现重点],),
    table.hline(),
    [创建], [`socket`、`socketpair`], [domain/type/protocol 校验，`SOCK_NONBLOCK/CLOEXEC`，多 fd 失败回滚],
    [端点与连接], [`bind`、`connect`、`listen`、`accept/accept4`], [sockaddr 转换，低端口权限，backlog 传递，新连接 fd 所有权],
    [数据], [`sendto/recvfrom`、`sendmsg/recvmsg`、`sendmmsg/recvmmsg`], [通过 MM 逐页 copyin/copyout，iovec 聚合/分散，部分成功返回],
    [查询与控制], [`getsockname/getpeername`、`setsockopt/getsockopt`、`shutdown`], [长度受限写回，`TCP_NODELAY`、IP TTL 等有效选项，半关闭语义],
    [等待], [`read/write`、`ppoll/pselect6`、`epoll`], [通过 `FileOp::read_ready/write_ready` 统一扫描 socket 可读/可写状态],
  )]
  , kind: table
  )

`sendmsg/recvmsg` 会先检查 `IOV_MAX=1024` 和累计长度溢出，再经 MM 的用户页拷贝路径处理每个 iovec；`sendmmsg/recvmmsg` 按消息顺序处理，中途失败时，若前面已有成功项就返回已完成数量，保留 Linux 多消息接口的部分进度语义。控制消息、out-of-band 数据和 error queue 尚未实现，相关 flag 返回明确 errno，而不伪造附加数据。

socket readiness 由 `Socket::tcp_poll` 先驱动一轮协议 poll，再查询 TCP/UDP/UNIX 对象。TCP 已连接 socket 根据 `can_recv/can_send/may_recv/may_send` 计算可读可写，listener 根据 accept queue 是否非空可读，UDP 根据数据报缓冲状态判断，UNIX socket 则检查字节队列或 pending queue。这使 socket 能被标准 `ppoll/pselect6/epoll` 扫描；当前 socket 没有覆盖 `FileOp::register_poll_waiter` 建立事件推送，因而统一等待层对这类 fd 使用协作式重试，不将其宣称为完全事件驱动的 socket 唤醒。

`/proc/net/tcp` 也复用同一实时状态。procfs 读取时先 poll 协议栈，再同时收集 `ListenTable` 中的 listener/未 accept 连接和 `SocketSet` 中的 TCP socket，转换为 Linux 工具可识别的端点与状态表。这使 `ss` 等用户工具能观察内核网络状态，无需一份独立的 procfs 镜像缓存。

=== 8.6.1 回环 IPv6 兼容边界
<861-回环-ipv6-兼容边界>
RespOS 接受 `AF_INET6` TCP/UDP socket，但当前只对 `::` 和 `::1` 建立回环兼容：syscall 层将两者分别归一为 IPv4 `0.0.0.0` 和 `127.0.0.1` 再进入现有回环路径，写回 `sockaddr_in6` 时再还原为全零或 `::1`。其他 IPv6 地址返回 `EADDRNOTAVAIL`。

这里的目标是满足只需本地回环的 libc 和应用探测，并不是建立完整的 IPv6 接口地址、邻居发现和路由。smoltcp 启用 `proto-ipv6` 特性只说明协议代码可编译，不代表 RespOS 已经把它接入设备、地址配置和 ABI 的完整路径。

== 8.7 与其他模块的协作
<87-与其他模块的协作>
网络模块的可用性依赖内存、VFS、调度、时钟和 procfs 的共同语义。这些交互是 socket 在竞赛负载中能正确阻塞、被中断并随进程回收的前提，具体边界见表 8-4。

#strong[表 8-4 网络模块的跨模块协作]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([协作模块], [网络使用的能力], [关键约束],),
    table.hline(),
    [MM], [sockaddr、iovec、数据缓冲和 sockopt 的 copyin/copyout], [用户页可能跨页或 lazy/COW，不在 net syscall 中直接解引用],
    [VFS / fd table], [`Socket : FileOp`、fd 分配、`CLOEXEC`、`O_NONBLOCK`], [descriptor flag 与共享 socket 状态必须维持既有分层],
    [task / signal], [可中断阻塞、yield、blocked registration、`EINTR`], [检查与入队之间不得丢失 signal wakeup],
    [timer], [TCP 阻塞轮询的短 timeout、smoltcp poll timestamp], [不在 socket syscall 忙等导致全局 timeout 饥饿],
    [procfs], [`/proc/net/tcp` 的动态端点和状态], [从实际 listener/socket 生成，不维护会过期的镜像],
  )]
  , kind: table
  )

== 8.8 功能与设计成果总结
<88-功能与设计成果总结>
RespOS 网络模块的项目贡献主要位于 smoltcp 之上和之下的工程连接：向上建立 Linux socket ABI、fd/VFS 身份和可中断阻塞语义，向下建立可复用缓冲的 loopback `Device`，并在中间为 smoltcp 单 listener 建立符合 backlog 用户语义的受限句柄池。表 8-5 汇总这些成果及其当前边界。

#strong[表 8-5 网络模块成果与当前边界]

#figure(
  align(center)[#table(
    columns: 3,
    align: (auto,auto,auto,),
    table.header([方向], [已实现的设计成果], [证据或边界],),
    table.hline(),
    [TCP loopback], [`connect/listen/accept/send/recv/shutdown`，CAS 状态机，可中断阻塞], [`net_loopback_smoke` 覆盖 fork 客户端与请求/回显；真实外网不在当前数据面内],
    [并发监听], [backlog 限制为 1～128，listener 池、accept queue 和 poll 后立即补位], [2026-08-03 RV64 CAgent 连续三轮的并发 connect 失败消失；不用 client 硬编码重试掩盖 `ECONNREFUSED`],
    [UDP loopback], [`bind/sendto/recvfrom/connect/send/recv`，自动临时端口], [`net_loopback_smoke` 覆盖数据报发送、内容和来源地址],
    [UNIX 域], [pathname/抽象 key、`socketpair`、listen/connect/accept、64 KiB 内存通道], [`DGRAM/SEQPACKET` 当前与 stream 共用字节队列，不宣称完整消息边界],
    [Linux ABI], [sockaddr v4/v6/unix、msg/mmsg、主要 sockopt、fd readiness、`/proc/net/tcp`], [ancillary data、OOB/error queue、raw socket 未实现；socket poll waiter 仍为协作式重试],
    [网络层与设备], [IPv4 loopback 与 `AF_INET6 ::/::1` 兼容入口], [其他 IPv6 地址、路由、virtio-net 与物理网卡数据路径尚未接入],
  )]
  , kind: table
  )

就当前实现而言，网络模块已经形成了一条本地通信闭环：套接字由 fd 持有，协议状态由 smoltcp 驱动，阻塞过程与调度和信号协作，并发握手由 listener 池维持 backlog 容量，close 后再回收协议句柄。章节中列出的边界对应当前实际接入的数据路径，而不是第三方库本身的功能列表。
