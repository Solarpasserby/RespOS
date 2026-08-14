//! TCP 套接字实现。
//!
//! 在 smoltcp 的 TCP 状态机之上封装了一层粗粒度状态机：
//!
//! ```text
//! CLOSED ──connect()──→ BUSY ──→ CONNECTING ──poll──→ CONNECTED
//! CLOSED ──listen()───→ BUSY ──→ LISTENING
//! CONNECTED ──shutdown()──→ BUSY ──→ CLOSED
//! ```
//!
//! 所有状态转换通过 `AtomicU8` 的 CAS 操作实现，确保单核环境下的并发安全。
//! 阻塞操作使用 `block_on` 模式：poll 接口 → 尝试操作 → EAGAIN 则等待网络事件后重试。

use core::{
    cell::UnsafeCell,
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicU64, Ordering},
};
use smoltcp::{
    iface::SocketHandle,
    socket::tcp::{self, ConnectError, RecvError, SendError, State},
    wire::{IpEndpoint, IpListenEndpoint, Ipv4Address},
};
use spin::Mutex;

use crate::{
    net::addr::{LOOP_BACK_IP, UNSPECIFIED_ENDPOINT, from_sockaddr_to_ipendpoint, is_unspecified},
    syscall::{Errno, SysResult, finish_task_timeout, register_task_timeout_us},
    task::{
        current_task, prepare_current_task_blocked, remove_task, switch_to_next_task,
        yield_current_task,
    },
    timer::get_timeout_us,
};

use super::{
    LISTEN_TABLE, SocketSetWrapper, poll_interfaces, register_tcp_waiter, service_task_timers,
    socket_set, unregister_tcp_waiter,
};

/// 描述 socket 的当前可读/可写状态。
pub struct PollState {
    pub readable: bool,
    pub writeable: bool,
}

/// TCP 套接字。
///
/// 内部包含 smoltcp socket 句柄（`SocketHandle`），所有协议操作委托给 smoltcp。
/// 状态字段使用 `AtomicU8` 以确保跨 yield 的一致性。
pub struct TcpSocket {
    /// 粗粒度连接状态：CLOSED / BUSY / CONNECTED / CONNECTING / LISTENING。
    state: AtomicU8,
    /// smoltcp socket 句柄，通过 `SOCKET_SET` 间接访问。
    handle: UnsafeCell<Option<SocketHandle>>,
    /// 绑定的本地地址。
    local_addr: UnsafeCell<IpEndpoint>,
    /// 连接的对端地址。
    remote_addr: UnsafeCell<IpEndpoint>,
    /// 是否非阻塞模式。
    nonblock: AtomicBool,
    /// 是否允许端口复用（SO_REUSEADDR）。
    reuse_addr: AtomicBool,
    /// 用户态 shutdown(SHUT_RD/SHUT_WR) 的半关闭状态。
    recv_shutdown: AtomicBool,
    send_shutdown: AtomicBool,
    /// 异步 connect 完成后由 SO_ERROR 读取并消费的正 errno；0 表示无 pending error。
    pending_error: AtomicIsize,
    /// TCP keepalive 参数。
    tcp_keepidle: AtomicU64,
    tcp_keepintvl: AtomicU64,
    tcp_keepcnt: AtomicU64,
}

/// 状态常量。
const STATE_CLOSED: u8 = 0;
const STATE_BUSY: u8 = 1;
const STATE_CONNECTED: u8 = 2;
const STATE_CONNECTING: u8 = 3;
const STATE_LISTENING: u8 = 4;
const STATE_FAILED: u8 = 5;

const SHUT_RD: usize = 0;
const SHUT_WR: usize = 1;
const SHUT_RDWR: usize = 2;

// SAFETY: TcpSocket 的字段访问受 `update_state` / `block_on` 控制，
// 在单核协作式调度下不存在真正的并发问题。
unsafe impl Sync for TcpSocket {}

// ——— 私有方法 ———
impl TcpSocket {
    fn get_state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    fn set_state(&self, state: u8) {
        self.state.store(state, Ordering::Release);
    }

    pub fn is_closed(&self) -> bool {
        self.get_state() == STATE_CLOSED
    }

    pub fn is_connected(&self) -> bool {
        self.get_state() == STATE_CONNECTED
    }

    fn reset_shutdown_flags(&self) {
        self.recv_shutdown.store(false, Ordering::Release);
        self.send_shutdown.store(false, Ordering::Release);
    }

    fn is_connecting(&self) -> bool {
        self.get_state() == STATE_CONNECTING
    }

    fn is_listening(&self) -> bool {
        self.get_state() == STATE_LISTENING
    }

    pub fn get_tcp_keepcnt(&self) -> u64 {
        self.tcp_keepcnt.load(Ordering::Acquire)
    }

    pub fn get_tcp_keepidle(&self) -> u64 {
        self.tcp_keepidle.load(Ordering::Acquire)
    }

    pub fn get_tcp_keepintvl(&self) -> u64 {
        self.tcp_keepintvl.load(Ordering::Acquire)
    }

    /// 原子状态转换：current → BUSY → 执行 f → new（成功）或 current（失败）。
    ///
    /// 如果当前状态不是 `current`（被其他操作抢占），返回 `EISCONN`。
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

    /// 构造 `IpListenEndpoint`，若地址未指定则默认使用回环地址。
    fn bound_endpoint(&self) -> IpListenEndpoint {
        let local_addr = unsafe { self.local_addr.get().read() };
        let port = if local_addr.port != 0 {
            local_addr.port
        } else {
            get_ephemeral_port()
        };
        debug_assert!(port != 0);
        let addr = if is_unspecified(local_addr.addr) {
            Some(smoltcp::wire::IpAddress::Ipv4(Ipv4Address::new(
                127, 0, 0, 1,
            )))
        } else {
            Some(local_addr.addr)
        };
        IpListenEndpoint { addr, port }
    }

    /// 检查 connect 后的 smoltcp 状态：SynSent → 未就绪，Established → 已连接。
    fn poll_connect(&self, handle: SocketHandle) -> PollState {
        let writable = socket_set()
            .lock()
            .with_socket::<_, tcp::Socket, _>(handle, |socket| match socket.state() {
                State::SynSent => false,
                State::Established => {
                    self.reset_shutdown_flags();
                    self.pending_error.store(0, Ordering::Release);
                    self.set_state(STATE_CONNECTED);
                    true
                }
                _ => {
                    self.pending_error
                        .store(Errno::ECONNREFUSED.code(), Ordering::Release);
                    unsafe {
                        self.local_addr.get().write(UNSPECIFIED_ENDPOINT);
                        self.remote_addr.get().write(UNSPECIFIED_ENDPOINT);
                    }
                    self.set_state(STATE_FAILED);
                    true
                }
            });
        PollState {
            readable: false,
            writeable: writable,
        }
    }

    fn reset_failed_connect(&self) -> Result<(), Errno> {
        self.state
            .compare_exchange(
                STATE_FAILED,
                STATE_BUSY,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| Errno::EALREADY)?;
        if let Some(handle) = unsafe { self.handle.get().read() } {
            socket_set().lock().remove(handle);
        }
        let handle = socket_set().lock().add(SocketSetWrapper::new_tcp_socket());
        unsafe {
            self.handle.get().write(Some(handle));
            self.local_addr.get().write(UNSPECIFIED_ENDPOINT);
            self.remote_addr.get().write(UNSPECIFIED_ENDPOINT);
        }
        self.pending_error.store(0, Ordering::Release);
        self.set_state(STATE_CLOSED);
        Ok(())
    }

    /// 检查已连接 socket 的可读写状态。
    fn poll_stream(&self, _isread: bool) -> PollState {
        let handle = unsafe { self.handle.get().read().unwrap() };
        let mut readable = false;
        let mut writeable = false;
        socket_set()
            .lock()
            .with_socket::<_, tcp::Socket, _>(handle, |socket| {
                readable = !socket.may_recv() || socket.can_recv();
                writeable = !socket.may_send() || socket.can_send();
            });
        readable |= self.recv_shutdown.load(Ordering::Acquire);
        writeable |= self.send_shutdown.load(Ordering::Acquire);
        if !readable && !writeable {
            readable = true;
        }
        PollState {
            readable,
            writeable,
        }
    }

    /// 检查监听 socket 是否有已完成握手的连接。
    fn poll_listening(&self) -> PollState {
        let port = unsafe { self.local_addr.get().read().port };
        let readable = LISTEN_TABLE.lock().can_accept(port);
        PollState {
            readable,
            writeable: false,
        }
    }

    /// 阻塞循环：poll → 尝试操作 → 成功返回 / EAGAIN 则等待网络事件后重试。
    fn block_on<F, T>(
        &self,
        per_call_nonblocking: bool,
        deadline_us: Option<usize>,
        mut f: F,
    ) -> Result<T, Errno>
    where
        F: FnMut() -> Result<T, Errno>,
    {
        if self.is_nonblocking() || per_call_nonblocking {
            f()
        } else {
            let task = current_task().ok_or(Errno::ESRCH)?;
            task.set_interruptible(true);
            let result = loop {
                service_task_timers();
                poll_interfaces();
                task.check_real_timer();
                if task.check_signal_interrupt() || task.is_interrupted() {
                    task.clear_interrupted();
                    break Err(Errno::EINTR);
                }
                match f() {
                    Ok(res) => break Ok(res),
                    Err(res) => {
                        if res == Errno::EAGAIN {
                            // Publish the waiter before the final condition
                            // check. A peer may enqueue data between the first
                            // check and blocking; the second poll/check closes
                            // that lost-wakeup window.
                            register_tcp_waiter(task.tid());
                            poll_interfaces();
                            match f() {
                                Ok(res) => {
                                    unregister_tcp_waiter(task.tid());
                                    break Ok(res);
                                }
                                Err(e) if e != Errno::EAGAIN => {
                                    unregister_tcp_waiter(task.tid());
                                    break Err(e);
                                }
                                Err(_) => {}
                            }
                            if deadline_us.is_some_and(|deadline| get_timeout_us() >= deadline) {
                                unregister_tcp_waiter(task.tid());
                                break Err(Errno::EAGAIN);
                            }
                            // Network progress wakes this task immediately.
                            // Keep the existing short deadline as a fallback
                            // for an idle listener: timer processing in the
                            // current scheduler still relies on a periodic
                            // runnable task while every userspace task sleeps.
                            let fallback_deadline = get_timeout_us().saturating_add(1000);
                            let wait_deadline = deadline_us
                                .map(|deadline| deadline.min(fallback_deadline))
                                .unwrap_or(fallback_deadline);
                            register_task_timeout_us(task.tid(), wait_deadline);
                            if prepare_current_task_blocked() {
                                if task.is_ready()
                                    || task.is_interrupted()
                                    || task.check_signal_interrupt()
                                {
                                    remove_task(task.tid());
                                    task.set_running();
                                } else {
                                    switch_to_next_task();
                                }
                            } else {
                                crate::perf::net_yield(1);
                                crate::perf::tcp_wait_yield(1);
                                yield_current_task();
                            }
                            let _ = finish_task_timeout(task.tid());
                            unregister_tcp_waiter(task.tid());
                            task.check_real_timer();
                            if task.check_signal_interrupt() || task.is_interrupted() {
                                task.clear_interrupted();
                                break Err(Errno::EINTR);
                            }
                        } else {
                            break Err(res);
                        }
                    }
                }
            };
            task.set_interruptible(false);
            result
        }
    }
}

// ——— 公有方法 ———
impl TcpSocket {
    /// 创建一个处于 CLOSED 状态的 TCP 套接字。
    pub const fn new() -> Self {
        TcpSocket {
            state: AtomicU8::new(STATE_CLOSED),
            handle: UnsafeCell::new(None),
            local_addr: UnsafeCell::new(UNSPECIFIED_ENDPOINT),
            remote_addr: UnsafeCell::new(UNSPECIFIED_ENDPOINT),
            nonblock: AtomicBool::new(false),
            reuse_addr: AtomicBool::new(false),
            recv_shutdown: AtomicBool::new(false),
            send_shutdown: AtomicBool::new(false),
            pending_error: AtomicIsize::new(0),
            tcp_keepidle: AtomicU64::new(0),
            tcp_keepintvl: AtomicU64::new(0),
            tcp_keepcnt: AtomicU64::new(0),
        }
    }

    /// 用已有的 smoltcp 句柄创建一个已连接的套接字（用于 accept 返回）。
    pub fn new_connected(
        handle: SocketHandle,
        local_endpoint: IpEndpoint,
        remote_endpoint: IpEndpoint,
    ) -> Self {
        TcpSocket {
            state: AtomicU8::new(STATE_CONNECTED),
            handle: UnsafeCell::new(Some(handle)),
            local_addr: UnsafeCell::new(local_endpoint),
            remote_addr: UnsafeCell::new(remote_endpoint),
            nonblock: AtomicBool::new(false),
            reuse_addr: AtomicBool::new(false),
            recv_shutdown: AtomicBool::new(false),
            send_shutdown: AtomicBool::new(false),
            pending_error: AtomicIsize::new(0),
            tcp_keepidle: AtomicU64::new(0),
            tcp_keepintvl: AtomicU64::new(0),
            tcp_keepcnt: AtomicU64::new(0),
        }
    }

    pub fn local_addr(&self) -> Result<IpEndpoint, Errno> {
        match self.get_state() {
            STATE_CONNECTED | STATE_LISTENING | STATE_CLOSED => {
                Ok(unsafe { self.local_addr.get().read() })
            }
            _ => Err(Errno::ENOTCONN),
        }
    }

    pub fn remote_addr(&self) -> Result<IpEndpoint, Errno> {
        match self.get_state() {
            STATE_CONNECTED => Ok(unsafe { self.remote_addr.get().read() }),
            _ => Err(Errno::ENOTCONN),
        }
    }

    pub fn is_block(&self) -> bool {
        !self.is_nonblocking()
    }

    pub fn is_nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Acquire)
    }

    pub fn set_nonblocking(&self, block: bool) {
        self.nonblock.store(block, Ordering::Release);
    }

    pub fn is_reuse_addr(&self) -> bool {
        self.reuse_addr.load(Ordering::Acquire)
    }

    pub fn set_reuse_addr(&self, reuse: bool) {
        self.reuse_addr.store(reuse, Ordering::Release);
    }

    /// 发起 TCP 连接。
    ///
    /// 1. CAS CLOSED → CONNECTING
    /// 2. 通过 smoltcp 发送 SYN
    /// 3. `block_on` 等待 smoltcp 状态变为 Established
    pub fn connect(
        &self,
        remote_addr: SocketAddr,
        deadline_us: Option<usize>,
    ) -> Result<(), Errno> {
        match self.get_state() {
            STATE_CONNECTING => {
                poll_interfaces();
                let handle = unsafe { self.handle.get().read().ok_or(Errno::ENOTCONN)? };
                self.poll_connect(handle);
                return match self.get_state() {
                    STATE_CONNECTING => Err(Errno::EALREADY),
                    STATE_CONNECTED => Err(Errno::EISCONN),
                    STATE_FAILED => Err(Errno::ECONNREFUSED),
                    _ => Err(Errno::EALREADY),
                };
            }
            STATE_CONNECTED | STATE_LISTENING => return Err(Errno::EISCONN),
            STATE_FAILED => {
                let error = self.pending_error.load(Ordering::Acquire);
                if error != 0 {
                    return Err(Errno::ECONNREFUSED);
                }
                self.reset_failed_connect()?;
                return Err(Errno::ECONNABORTED);
            }
            STATE_BUSY => return Err(Errno::EALREADY),
            STATE_CLOSED => {}
            _ => return Err(Errno::EINVAL),
        }

        self.update_state(STATE_CLOSED, STATE_CONNECTING, || {
            let handle = unsafe { self.handle.get().read() }
                .unwrap_or_else(|| socket_set().lock().add(SocketSetWrapper::new_tcp_socket()));
            unsafe {
                self.handle.get().write(Some(handle));
            }
            self.reset_shutdown_flags();
            let bound_endpoint = self.bound_endpoint();
            let remote_ipendpoint = from_sockaddr_to_ipendpoint(remote_addr);

            if bound_endpoint.port == remote_ipendpoint.port {
                return Err(Errno::ECONNREFUSED);
            }

            let iface = &*LOOPBACK_IFACE;
            let (local_endpoint, remote_endpoint) = socket_set()
                .lock()
                .with_socket_mut::<_, tcp::Socket, Result<(IpEndpoint, IpEndpoint), Errno>>(
                    handle,
                    |socket| {
                        socket
                            .connect(iface.lock().context(), remote_ipendpoint, bound_endpoint)
                            .map_err(|e| match e {
                                ConnectError::InvalidState => Errno::ECONNREFUSED,
                                ConnectError::Unaddressable => Errno::ECONNREFUSED,
                            })?;
                        Ok((
                            socket.local_endpoint().unwrap(),
                            socket.remote_endpoint().unwrap(),
                        ))
                    },
                )?;
            unsafe {
                self.local_addr.get().write(local_endpoint);
                self.remote_addr.get().write(remote_endpoint);
            }
            Ok(())
        })?;

        if self.is_nonblocking() {
            return Err(Errno::EINPROGRESS);
        }

        crate::perf::net_yield(1);
        crate::perf::tcp_connect_yield(1);
        yield_current_task();

        let result = self
            .block_on(false, deadline_us, || {
                let handle = unsafe { self.handle.get().read().unwrap() };
                let PollState { writeable, .. } = self.poll_connect(handle);
                if !writeable {
                    Err(Errno::EAGAIN)
                } else if self.get_state() == STATE_CONNECTED {
                    Ok(())
                } else {
                    Err(Errno::ECONNREFUSED)
                }
            })
            .map_err(|err| {
                if err == Errno::EAGAIN && deadline_us.is_some() {
                    Errno::EINPROGRESS
                } else {
                    err
                }
            });
        if result.is_err() {
            // Blocking connect reports the failure directly; there is no
            // asynchronous error left for SO_ERROR to consume.
            self.pending_error.store(0, Ordering::Release);
        }
        result
    }

    /// 绑定本地地址和端口。
    pub fn bind(&self, mut local_addr: SocketAddr) -> SysResult {
        self.update_state(STATE_CLOSED, STATE_CLOSED, || {
            let old_addr = unsafe { self.local_addr.get().read() };
            if old_addr.port != 0 {
                return Err(Errno::EINVAL);
            }
            if local_addr.port() == 0 {
                local_addr.set_port(get_ephemeral_port());
            }
            if !self.is_reuse_addr() {
                let l = from_sockaddr_to_ipendpoint(local_addr);
                socket_set().lock().tcp_bind_check(l.addr, l.port)?;
            }
            unsafe {
                self.local_addr
                    .get()
                    .write(from_sockaddr_to_ipendpoint(local_addr));
            }
            Ok(())
        })
    }

    /// 开始监听绑定的端口。将端点注册到 `LISTEN_TABLE`。
    pub fn listen(&self, backlog: usize) -> SysResult {
        self.update_state(STATE_CLOSED, STATE_LISTENING, || {
            let bound_endpoint = self.bound_endpoint();
            let existing_handle = unsafe { self.handle.get().read() };
            let listen_handle = if let Some(handle) = existing_handle {
                socket_set()
                    .lock()
                    .with_socket_mut::<_, tcp::Socket, Result<(), Errno>>(handle, |socket| {
                        socket.listen(bound_endpoint).map_err(|_| Errno::EADDRINUSE)
                    })?;
                handle
            } else {
                let mut listen_socket = SocketSetWrapper::new_tcp_socket();
                listen_socket
                    .listen(bound_endpoint)
                    .map_err(|_| Errno::EADDRINUSE)?;
                socket_set().lock().add(listen_socket)
            };
            unsafe {
                (*self.local_addr.get()).port = bound_endpoint.port;
            }
            if let Err(err) = LISTEN_TABLE
                .lock()
                .listen(bound_endpoint, listen_handle, backlog)
            {
                if existing_handle.is_none() {
                    socket_set().lock().remove(listen_handle);
                }
                return Err(err);
            }
            unsafe {
                self.handle.get().write(None);
            }
            Ok(())
        })
    }

    /// 接受一个已完成的入站连接，返回新的已连接 `TcpSocket`。
    pub fn accept(&self, deadline_us: Option<usize>) -> Result<TcpSocket, Errno> {
        if !self.is_listening() {
            return Err(Errno::EINVAL);
        }
        let local_port = unsafe { self.local_addr.get().read().port };

        self.block_on(false, deadline_us, || {
            match LISTEN_TABLE.lock().accept(local_port) {
                Ok((handle, (local_endpoint, remote_endpoint))) => Ok(TcpSocket::new_connected(
                    handle,
                    local_endpoint,
                    remote_endpoint,
                )),
                Err(e) => {
                    if e == Errno::ECONNRESET || e == Errno::ECONNREFUSED {
                        self.close_all();
                    }
                    Err(e)
                }
            }
        })
    }

    /// 处理用户态 shutdown(2)。smoltcp 的 close() 正好对应关闭 TCP 发送半边。
    pub fn shutdown(&self, how: usize) -> SysResult {
        if how > SHUT_RDWR {
            return Err(Errno::EINVAL);
        }
        if self.is_connecting() {
            return Err(Errno::ENOTCONN);
        }
        if self.is_listening() {
            return Err(Errno::ENOTCONN);
        }
        if !self.is_connected() {
            return Err(Errno::ENOTCONN);
        }

        match how {
            SHUT_RD => {
                self.recv_shutdown.store(true, Ordering::Release);
            }
            SHUT_WR => {
                self.shutdown_write();
            }
            SHUT_RDWR => {
                self.recv_shutdown.store(true, Ordering::Release);
                self.shutdown_write();
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn shutdown_write(&self) {
        if self.send_shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        let Some(handle) = (unsafe { self.handle.get().read() }) else {
            return;
        };
        socket_set()
            .lock()
            .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| {
                socket.close();
            });
        socket_set().lock().poll_interfaces();
    }

    /// 释放连接或停止监听，用于 Drop 和内部错误恢复。
    pub fn close_all(&self) {
        // 已连接 socket：关闭 smoltcp socket，清除绑定地址
        let _ = self.update_state(STATE_CONNECTED, STATE_CLOSED, || {
            let handle = unsafe { self.handle.get().read().unwrap() };
            socket_set()
                .lock()
                .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| {
                    socket.close();
                });
            unsafe {
                self.local_addr.get().write(UNSPECIFIED_ENDPOINT);
            }
            self.recv_shutdown.store(true, Ordering::Release);
            self.send_shutdown.store(true, Ordering::Release);
            socket_set().lock().poll_interfaces();
            Ok(())
        });

        // 监听 socket：从 LISTEN_TABLE 注销
        let _ = self.update_state(STATE_LISTENING, STATE_CLOSED, || {
            let local_port = unsafe { self.local_addr.get().read().port };
            unsafe {
                self.local_addr.get().write(UNSPECIFIED_ENDPOINT);
            }
            self.recv_shutdown.store(true, Ordering::Release);
            self.send_shutdown.store(true, Ordering::Release);
            LISTEN_TABLE.lock().unlisten(local_port);
            socket_set().lock().poll_interfaces();
            Ok(())
        });
    }

    /// 关闭 smoltcp socket（不清除状态，由 Drop 或 shutdown 调用）。
    pub fn close(&self) {
        let handle = match unsafe { self.handle.get().read() } {
            Some(h) => h,
            None => return,
        };
        socket_set().lock().poll_interfaces();
        socket_set()
            .lock()
            .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| {
                socket.close();
            });
    }

    /// 从 socket 接收数据。
    fn recv_once(
        &self,
        buf: &mut [u8],
        per_call_nonblocking: bool,
        deadline_us: Option<usize>,
        peek: bool,
        peek_min_len: usize,
    ) -> Result<usize, Errno> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.is_connecting() {
            return Err(Errno::EAGAIN);
        } else if !self.is_connected() {
            return Err(Errno::ENOTCONN);
        } else if self.recv_shutdown.load(Ordering::Acquire) {
            return Ok(0);
        }
        let handle = unsafe { self.handle.get().read().unwrap() };
        self.block_on(per_call_nonblocking, deadline_us, || {
            socket_set()
                .lock()
                .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| {
                    if socket.recv_queue() > 0 {
                        if peek && socket.recv_queue() < peek_min_len && socket.may_recv() {
                            return Err(Errno::EAGAIN);
                        }
                        let result = if peek {
                            socket.peek_slice(buf)
                        } else {
                            socket.recv_slice(buf)
                        };
                        let len = result.map_err(|e| match e {
                            RecvError::Finished => Errno::ECONNRESET,
                            RecvError::InvalidState => Errno::ENOTCONN,
                        })?;
                        Ok(len)
                    } else if !socket.may_recv() {
                        Ok(0)
                    } else if !socket.is_active() {
                        Err(Errno::ECONNREFUSED)
                    } else {
                        Err(Errno::EAGAIN)
                    }
                })
        })
    }

    /// 从 TCP 字节流接收数据，并实现 MSG_PEEK / MSG_WAITALL 的流语义。
    pub fn recv(
        &self,
        buf: &mut [u8],
        per_call_nonblocking: bool,
        deadline_us: Option<usize>,
        peek: bool,
        waitall: bool,
    ) -> Result<usize, Errno> {
        let waitall = waitall && !per_call_nonblocking && !self.is_nonblocking();
        if !waitall {
            return self.recv_once(buf, per_call_nonblocking, deadline_us, peek, 1);
        }

        if peek {
            let result = self.recv_once(buf, false, deadline_us, true, buf.len());
            return match result {
                Err(err @ (Errno::EAGAIN | Errno::EINTR)) => {
                    match self.recv_once(buf, true, None, true, 1) {
                        Ok(len) if len > 0 => Ok(len),
                        _ => Err(err),
                    }
                }
                other => other,
            };
        }

        let mut total = 0usize;
        while total < buf.len() {
            match self.recv_once(&mut buf[total..], false, deadline_us, false, 1) {
                Ok(0) => break,
                Ok(len) => total += len,
                Err(_) if total > 0 => break,
                Err(err) => return Err(err),
            }
        }
        Ok(total)
    }

    /// 向 socket 发送数据。
    pub fn send(
        &self,
        buf: &[u8],
        per_call_nonblocking: bool,
        deadline_us: Option<usize>,
    ) -> Result<usize, Errno> {
        if self.is_connecting() {
            return Err(Errno::EAGAIN);
        } else if !self.is_connected() {
            return Err(Errno::ENOTCONN);
        } else if self.send_shutdown.load(Ordering::Acquire) {
            return Err(Errno::EPIPE);
        }
        let handle = unsafe { self.handle.get().read().unwrap() };
        self.block_on(per_call_nonblocking, deadline_us, || {
            socket_set()
                .lock()
                .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| {
                    if !socket.is_active() || !socket.may_send() {
                        Err(Errno::EPIPE)
                    } else if socket.can_send() {
                        let len = socket.send_slice(buf).map_err(|e| match e {
                            SendError::InvalidState => Errno::EPIPE,
                        })?;
                        Ok(len)
                    } else if socket.send_queue() == socket.send_capacity() {
                        Err(Errno::EAGAIN)
                    } else {
                        Err(Errno::EAGAIN)
                    }
                })
        })
    }

    /// 查询可读/可写状态。
    pub fn poll(&self, isread: bool) -> PollState {
        match self.get_state() {
            STATE_LISTENING => self.poll_listening(),
            STATE_CONNECTING => {
                let handle = unsafe { self.handle.get().read().unwrap() };
                self.poll_connect(handle)
            }
            STATE_CONNECTED => self.poll_stream(isread),
            STATE_FAILED => PollState {
                readable: false,
                writeable: true,
            },
            _ => PollState {
                writeable: false,
                readable: false,
            },
        }
    }

    /// 返回并消费异步 connect 的 pending error，供 SO_ERROR 使用。
    pub fn take_error(&self) -> i32 {
        if self.is_connecting() {
            poll_interfaces();
            if let Some(handle) = unsafe { self.handle.get().read() } {
                self.poll_connect(handle);
            }
        }
        self.pending_error.swap(0, Ordering::AcqRel) as i32
    }

    pub fn has_pending_error(&self) -> bool {
        if self.is_connecting() {
            poll_interfaces();
            if let Some(handle) = unsafe { self.handle.get().read() } {
                self.poll_connect(handle);
            }
        }
        self.pending_error.load(Ordering::Acquire) != 0
    }

    /// 设置 Nagle 算法开关。
    pub fn set_nagle_enabled(&self, enable: bool) {
        let handle = unsafe {
            self.handle.get().read().unwrap_or_else(|| {
                let handle = socket_set().lock().add(SocketSetWrapper::new_tcp_socket());
                self.handle.get().write(Some(handle));
                handle
            })
        };
        socket_set()
            .lock()
            .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| {
                socket.set_nagle_enabled(enable);
            });
    }

    /// 启用 TCP keepalive。
    pub fn set_keep_alive(&self) {
        let handle = unsafe {
            self.handle.get().read().unwrap_or_else(|| {
                let handle = socket_set().lock().add(SocketSetWrapper::new_tcp_socket());
                self.handle.get().write(Some(handle));
                handle
            })
        };
        socket_set()
            .lock()
            .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| {
                socket.keep_alive();
            });
    }

    pub fn nagle_enabled(&self) -> bool {
        let Some(handle) = (unsafe { self.handle.get().read() }) else {
            return true;
        };
        socket_set()
            .lock()
            .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| socket.nagle_enabled())
    }

    pub fn with_socket<F, T>(&self, f: F) -> T
    where
        F: FnOnce(&tcp::Socket) -> T,
    {
        let handle = unsafe { self.handle.get().read().unwrap() };
        socket_set()
            .lock()
            .with_socket::<_, tcp::Socket, _>(handle, |socket| f(socket))
    }

    pub fn with_socket_mut<R>(&self, f: impl FnOnce(Option<&mut tcp::Socket>) -> R) -> R {
        let handle = unsafe { self.handle.get().read() };
        match handle {
            Some(handle) => socket_set()
                .lock()
                .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| f(Some(socket))),
            None => f(None),
        }
    }

    pub fn set_hop_limit(&self, limit: u8) -> SysResult {
        let handle = unsafe {
            self.handle.get().read().unwrap_or_else(|| {
                let handle = socket_set().lock().add(SocketSetWrapper::new_tcp_socket());
                self.handle.get().write(Some(handle));
                handle
            })
        };
        socket_set()
            .lock()
            .with_socket_mut::<_, tcp::Socket, _>(handle, |socket| {
                socket.set_hop_limit(Some(limit));
            });
        Ok(())
    }
}

/// 分配临时端口（范围 0xc000–0xffff），跳过已被 LISTEN_TABLE 占用的端口。
fn get_ephemeral_port() -> u16 {
    const PORT_START: u16 = 0xc000;
    const PORT_END: u16 = 0xffff;
    static CURR: Mutex<u16> = Mutex::new(PORT_START);

    let mut curr = CURR.lock();
    let mut tries = 0;
    while tries <= PORT_END - PORT_START {
        let port = *curr;
        if *curr == PORT_END {
            *curr = PORT_START;
        } else {
            *curr += 1;
        }
        if LISTEN_TABLE.lock().can_listen(port)
            && socket_set()
                .lock()
                .tcp_bind_check(LOOP_BACK_IP, port)
                .is_ok()
        {
            return port;
        }
        tries += 1;
    }
    panic!("no available port");
}

impl Drop for TcpSocket {
    fn drop(&mut self) {
        self.close_all();
        if let Some(handle) = unsafe { self.handle.get().read() } {
            socket_set().lock().remove(handle);
        }
    }
}

use super::LOOPBACK_IFACE;
