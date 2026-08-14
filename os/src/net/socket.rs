//! 套接字抽象层。
//!
//! `Socket` 是用户态可见的套接字对象，实现了 RespOS 的 `FileOp` trait，
//! 从而可以通过标准文件描述符接口（read/write/poll）操作。
//! 内部根据 `SocketKind` 分派到 `TcpSocket` 或 `UdpSocket`。

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::{
    net::SocketAddr,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use lazy_static::lazy_static;
use spin::Mutex;

use crate::{
    fs::vfs::InodeType,
    fs::{FileOp, KStat, OpenFlags, POLL_HUP, POLL_READ, POLL_WRITE, PollEvents, PollWaiters},
    mutex::SpinLock,
    syscall::{Errno, SysResult, finish_task_timeout, register_task_timeout_us},
    task::{
        current_task, prepare_current_task_blocked, remove_task, switch_to_next_task, wakeup_task,
    },
    timer::get_timeout_us,
};

use super::{
    addr::{UNSPECIFIED_ENDPOINT, from_ipendpoint_to_socketaddr},
    poll_interfaces,
    tcp::TcpSocket,
    udp::UdpSocket,
};

const UNIX_SOCKET_BUFFER_LIMIT: usize = 64 * 1024;
const UNIX_LISTEN_QUEUE_LIMIT: usize = 128;

lazy_static! {
    static ref UNIX_LISTENERS: Mutex<Vec<(Vec<u8>, Arc<UnixListener>)>> = Mutex::new(Vec::new());
}

// ——— 类型枚举 ———

/// 套接字地址族。
#[allow(non_camel_case_types)]
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SocketDomain {
    /// UNIX 域套接字。
    AF_UNIX = 1,
    /// IPv4 套接字。
    AF_INET = 2,
    /// IPv6 套接字（当前复用 loopback TCP/UDP 传输）。
    AF_INET6 = 10,
}

/// 套接字类型。
#[allow(non_camel_case_types)]
#[derive(Clone, PartialEq, Eq, Debug, Copy)]
pub enum SocketKind {
    /// 流式套接字（TCP）。
    SOCK_STREAM = 1,
    /// 数据报套接字（UDP）。
    SOCK_DGRAM = 2,
    /// 原始套接字（暂不支持）。
    SOCK_RAW = 3,
    /// UNIX seqpacket 在本地队列中按保序流处理。
    SOCK_SEQPACKET = 5,
}

// ——— SocketInner ———

/// 内部协议套接字，分派到 TCP 或 UDP。
enum SocketInner {
    Tcp(TcpSocket),
    Udp(UdpSocket),
    Unix(UnixSocket),
}

struct UnixSocket {
    rx: Arc<SpinLock<UnixBuffer>>,
    peer_rx: SpinLock<Option<Arc<SpinLock<UnixBuffer>>>>,
    peer_credentials: SpinLock<Option<UnixPeerCredentials>>,
    closed: Arc<AtomicBool>,
    peer_closed: SpinLock<Option<Arc<AtomicBool>>>,
    read_shutdown: Arc<AtomicBool>,
    write_shutdown: Arc<AtomicBool>,
    peer_read_shutdown: SpinLock<Option<Arc<AtomicBool>>>,
    peer_write_shutdown: SpinLock<Option<Arc<AtomicBool>>>,
    bound_key: Mutex<Option<Vec<u8>>>,
    peer_key: Mutex<Option<Vec<u8>>>,
    listener: SpinLock<Option<Arc<UnixListener>>>,
    nonblock: AtomicBool,
}

struct UnixListener {
    pending: SpinLock<UnixPending>,
    poll_waiters: Arc<PollWaiters>,
    credentials: UnixPeerCredentials,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnixPeerCredentials {
    pub pid: usize,
    pub uid: usize,
    pub gid: usize,
}

struct UnixBuffer {
    data: VecDeque<u8>,
    read_waiters: VecDeque<usize>,
    write_waiters: VecDeque<usize>,
    poll_waiters: Arc<PollWaiters>,
}

struct UnixPending {
    sockets: VecDeque<UnixSocket>,
    accept_waiters: VecDeque<usize>,
}

impl UnixBuffer {
    fn new() -> Self {
        Self {
            data: VecDeque::new(),
            read_waiters: VecDeque::new(),
            write_waiters: VecDeque::new(),
            poll_waiters: Arc::new(PollWaiters::new()),
        }
    }
}

impl UnixListener {
    fn new(credentials: UnixPeerCredentials) -> Self {
        Self {
            pending: SpinLock::new(UnixPending {
                sockets: VecDeque::new(),
                accept_waiters: VecDeque::new(),
            }),
            poll_waiters: Arc::new(PollWaiters::new()),
            credentials,
        }
    }
}

fn current_unix_credentials() -> SysResult<UnixPeerCredentials> {
    let task = current_task().ok_or(Errno::ESRCH)?;
    Ok(UnixPeerCredentials {
        pid: task.tgid(),
        uid: task.uid(),
        gid: task.gid(),
    })
}

impl UnixSocket {
    fn new() -> Self {
        Self {
            rx: Arc::new(SpinLock::new(UnixBuffer::new())),
            peer_rx: SpinLock::new(None),
            peer_credentials: SpinLock::new(None),
            closed: Arc::new(AtomicBool::new(false)),
            peer_closed: SpinLock::new(None),
            read_shutdown: Arc::new(AtomicBool::new(false)),
            write_shutdown: Arc::new(AtomicBool::new(false)),
            peer_read_shutdown: SpinLock::new(None),
            peer_write_shutdown: SpinLock::new(None),
            bound_key: Mutex::new(None),
            peer_key: Mutex::new(None),
            listener: SpinLock::new(None),
            nonblock: AtomicBool::new(false),
        }
    }

    fn pair(credentials: UnixPeerCredentials) -> (Self, Self) {
        let left = Self::new();
        let right = Self::new();
        *left.peer_rx.lock() = Some(right.rx.clone());
        *right.peer_rx.lock() = Some(left.rx.clone());
        *left.peer_closed.lock() = Some(right.closed.clone());
        *right.peer_closed.lock() = Some(left.closed.clone());
        *left.peer_read_shutdown.lock() = Some(right.read_shutdown.clone());
        *right.peer_read_shutdown.lock() = Some(left.read_shutdown.clone());
        *left.peer_write_shutdown.lock() = Some(right.write_shutdown.clone());
        *right.peer_write_shutdown.lock() = Some(left.write_shutdown.clone());
        *left.peer_credentials.lock() = Some(credentials);
        *right.peer_credentials.lock() = Some(credentials);
        (left, right)
    }

    fn set_nonblocking(&self, nonblock: bool) {
        self.nonblock.store(nonblock, Ordering::Release);
    }

    fn bind_path(&self, key: &[u8]) -> SysResult {
        let mut bound_key = self.bound_key.lock();
        if bound_key.is_some() {
            return Err(Errno::EINVAL);
        }
        *bound_key = Some(key.to_vec());
        Ok(())
    }

    fn ensure_unbound(&self) -> SysResult {
        if self.bound_key.lock().is_some() {
            return Err(Errno::EINVAL);
        }
        Ok(())
    }

    fn is_nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Acquire)
    }

    fn finish_interruptible_wait(&self, deadline_us: Option<usize>) -> SysResult<bool> {
        let task = current_task().ok_or(Errno::ESRCH)?;
        if task.is_ready() {
            // The producer won the race after we published Blocked but before
            // this CPU switched away.  Consume the ready publication locally.
            remove_task(task.tid());
            task.set_running();
        } else {
            switch_to_next_task();
        }
        let timed_out = if deadline_us.is_some() {
            finish_task_timeout(task.tid())
        } else {
            false
        };
        task.check_real_timer();
        if task.check_signal_interrupt() || task.is_interrupted() {
            task.clear_interrupted();
            task.set_interruptible(false);
            return Err(Errno::EINTR);
        }
        task.set_interruptible(false);
        Ok(timed_out || deadline_us.is_some_and(|deadline| get_timeout_us() >= deadline))
    }

    fn read(
        &self,
        buf: &mut [u8],
        per_call_nonblocking: bool,
        deadline_us: Option<usize>,
        peek: bool,
        waitall: bool,
    ) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.peer_rx.lock().is_none() {
            return Err(Errno::ENOTCONN);
        }
        let peer_closed = self.peer_closed.lock().clone().ok_or(Errno::ENOTCONN)?;
        let peer_write_shutdown = self
            .peer_write_shutdown
            .lock()
            .clone()
            .ok_or(Errno::ENOTCONN)?;
        let task = current_task().ok_or(Errno::ESRCH)?;
        let waitall = waitall && !per_call_nonblocking && !self.is_nonblocking();
        loop {
            let mut rx = self.rx.lock();
            let peer_eof = self.read_shutdown.load(Ordering::Acquire)
                || peer_closed.load(Ordering::Acquire)
                || peer_write_shutdown.load(Ordering::Acquire);
            let timed_out = deadline_us.is_some_and(|deadline| get_timeout_us() >= deadline);
            if !rx.data.is_empty()
                && (!waitall || rx.data.len() >= buf.len() || peer_eof || timed_out)
            {
                let read_len = rx.data.len().min(buf.len());
                if peek {
                    for (out, value) in buf.iter_mut().zip(rx.data.iter()).take(read_len) {
                        *out = *value;
                    }
                    return Ok(read_len);
                }
                for out in buf.iter_mut().take(read_len) {
                    *out = rx.data.pop_front().expect("read length checked");
                }
                let wake_writer = rx.write_waiters.pop_front();
                let poll_waiters = rx.poll_waiters.clone();
                drop(rx);
                poll_waiters.notify(POLL_WRITE);
                if let Some(tid) = wake_writer {
                    wakeup_task(tid);
                }
                return Ok(read_len);
            }
            // A stream socket reports EOF once the last reference to the peer
            // endpoint is closed and all bytes already sent have been drained.
            if peer_eof {
                return Ok(0);
            }
            if self.is_nonblocking() || per_call_nonblocking {
                return Err(Errno::EAGAIN);
            }
            if timed_out {
                return Err(Errno::EAGAIN);
            }
            task.set_interruptible(true);
            task.check_real_timer();
            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.set_interruptible(false);
                if !rx.data.is_empty() {
                    drop(rx);
                    return self.read(buf, true, None, peek, false);
                }
                return Err(Errno::EINTR);
            }
            rx.read_waiters.push_back(task.tid());
            if let Some(deadline) = deadline_us {
                register_task_timeout_us(task.tid(), deadline);
            }
            let should_block = prepare_current_task_blocked();
            if !should_block {
                if deadline_us.is_some() {
                    finish_task_timeout(task.tid());
                }
                task.set_interruptible(false);
                rx.read_waiters.retain(|tid| *tid != task.tid());
                return Err(Errno::ESRCH);
            }
            let interrupted = task.check_signal_interrupt() || task.is_interrupted();
            drop(rx);
            if interrupted {
                wakeup_task(task.tid());
            }
            let result = self.finish_interruptible_wait(deadline_us);
            self.rx.lock().read_waiters.retain(|tid| *tid != task.tid());
            match result {
                Ok(false) => {}
                Ok(true) => return self.read(buf, true, None, peek, false),
                Err(err) => {
                    return match self.read(buf, true, None, peek, false) {
                        Ok(len) => Ok(len),
                        Err(_) => Err(err),
                    };
                }
            }
        }
    }

    fn write(
        &self,
        buf: &[u8],
        per_call_nonblocking: bool,
        deadline_us: Option<usize>,
    ) -> SysResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let peer_rx = self.peer_rx.lock().clone().ok_or(Errno::ENOTCONN)?;
        let peer_closed = self.peer_closed.lock().clone().ok_or(Errno::ENOTCONN)?;
        let peer_read_shutdown = self
            .peer_read_shutdown
            .lock()
            .clone()
            .ok_or(Errno::ENOTCONN)?;
        let task = current_task().ok_or(Errno::ESRCH)?;
        loop {
            if self.write_shutdown.load(Ordering::Acquire)
                || peer_closed.load(Ordering::Acquire)
                || peer_read_shutdown.load(Ordering::Acquire)
            {
                return Err(Errno::EPIPE);
            }
            let mut rx = peer_rx.lock();
            let available = UNIX_SOCKET_BUFFER_LIMIT.saturating_sub(rx.data.len());
            if available > 0 {
                let write_len = available.min(buf.len());
                rx.data.extend(buf[..write_len].iter().copied());
                let wake_reader = rx.read_waiters.pop_front();
                let poll_waiters = rx.poll_waiters.clone();
                drop(rx);
                poll_waiters.notify(POLL_READ);
                if let Some(tid) = wake_reader {
                    wakeup_task(tid);
                }
                return Ok(write_len);
            }
            if self.is_nonblocking() || per_call_nonblocking {
                return Err(Errno::EAGAIN);
            }
            if deadline_us.is_some_and(|deadline| get_timeout_us() >= deadline) {
                return Err(Errno::EAGAIN);
            }
            task.set_interruptible(true);
            task.check_real_timer();
            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }
            rx.write_waiters.push_back(task.tid());
            if let Some(deadline) = deadline_us {
                register_task_timeout_us(task.tid(), deadline);
            }
            let should_block = prepare_current_task_blocked();
            if !should_block {
                if deadline_us.is_some() {
                    finish_task_timeout(task.tid());
                }
                task.set_interruptible(false);
                rx.write_waiters.retain(|tid| *tid != task.tid());
                return Err(Errno::ESRCH);
            }
            let interrupted = task.check_signal_interrupt() || task.is_interrupted();
            drop(rx);
            if interrupted {
                wakeup_task(task.tid());
            }
            let result = self.finish_interruptible_wait(deadline_us);
            peer_rx
                .lock()
                .write_waiters
                .retain(|tid| *tid != task.tid());
            if result? {
                return Err(Errno::EAGAIN);
            }
        }
    }

    fn read_ready(&self) -> bool {
        self.read_shutdown.load(Ordering::Acquire)
            || !self.rx.lock().data.is_empty()
            || self
                .peer_closed
                .lock()
                .as_ref()
                .is_some_and(|closed| closed.load(Ordering::Acquire))
            || self
                .peer_write_shutdown
                .lock()
                .as_ref()
                .is_some_and(|closed| closed.load(Ordering::Acquire))
    }

    fn write_ready(&self) -> bool {
        self.write_shutdown.load(Ordering::Acquire)
            || self
                .peer_closed
                .lock()
                .as_ref()
                .is_some_and(|closed| closed.load(Ordering::Acquire))
            || self
                .peer_read_shutdown
                .lock()
                .as_ref()
                .is_some_and(|closed| closed.load(Ordering::Acquire))
            || self
                .peer_rx
                .lock()
                .as_ref()
                .is_some_and(|rx| rx.lock().data.len() < UNIX_SOCKET_BUFFER_LIMIT)
    }

    fn listen(&self) -> SysResult {
        let key = self.bound_key.lock().clone().ok_or(Errno::EINVAL)?;
        let credentials = current_unix_credentials()?;
        let mut registry = UNIX_LISTENERS.lock();
        if registry.iter().any(|(item_key, _)| item_key == &key) {
            return Err(Errno::EADDRINUSE);
        }
        let listener = Arc::new(UnixListener::new(credentials));
        *self.listener.lock() = Some(listener.clone());
        registry.push((key, listener));
        Ok(())
    }

    fn connect_path(&self, key: &[u8]) -> SysResult {
        if self.peer_rx.lock().is_some() {
            return Err(Errno::EISCONN);
        }
        let connector_credentials = current_unix_credentials()?;
        let connector_key = self.bound_key.lock().clone();
        let listener = UNIX_LISTENERS
            .lock()
            .iter()
            .find(|(item_key, _)| item_key.as_slice() == key)
            .map(|(_, listener)| listener.clone())
            .ok_or(Errno::ECONNREFUSED)?;

        let server = UnixSocket::new();
        *server.bound_key.lock() = Some(key.to_vec());
        *server.peer_key.lock() = connector_key;
        *self.peer_key.lock() = Some(key.to_vec());
        *server.peer_rx.lock() = Some(self.rx.clone());
        *self.peer_rx.lock() = Some(server.rx.clone());
        *server.peer_closed.lock() = Some(self.closed.clone());
        *self.peer_closed.lock() = Some(server.closed.clone());
        *server.peer_read_shutdown.lock() = Some(self.read_shutdown.clone());
        *self.peer_read_shutdown.lock() = Some(server.read_shutdown.clone());
        *server.peer_write_shutdown.lock() = Some(self.write_shutdown.clone());
        *self.peer_write_shutdown.lock() = Some(server.write_shutdown.clone());
        *server.peer_credentials.lock() = Some(connector_credentials);
        *self.peer_credentials.lock() = Some(listener.credentials);

        let mut pending = listener.pending.lock();
        if pending.sockets.len() >= UNIX_LISTEN_QUEUE_LIMIT {
            *self.peer_rx.lock() = None;
            *self.peer_closed.lock() = None;
            *self.peer_read_shutdown.lock() = None;
            *self.peer_write_shutdown.lock() = None;
            *self.peer_credentials.lock() = None;
            *self.peer_key.lock() = None;
            return Err(Errno::EAGAIN);
        }
        pending.sockets.push_back(server);
        let wake_accept = pending.accept_waiters.pop_front();
        drop(pending);
        if let Some(tid) = wake_accept {
            wakeup_task(tid);
        }
        listener.poll_waiters.notify(POLL_READ);
        Ok(())
    }

    fn accept(&self, deadline_us: Option<usize>) -> SysResult<UnixSocket> {
        let listener = self.listener.lock().clone().ok_or(Errno::EINVAL)?;
        let task = current_task().ok_or(Errno::ESRCH)?;
        loop {
            let mut pending = listener.pending.lock();
            if let Some(socket) = pending.sockets.pop_front() {
                return Ok(socket);
            }
            if self.is_nonblocking() {
                return Err(Errno::EAGAIN);
            }
            if deadline_us.is_some_and(|deadline| get_timeout_us() >= deadline) {
                return Err(Errno::EAGAIN);
            }
            task.set_interruptible(true);
            task.check_real_timer();
            if task.check_signal_interrupt() || task.is_interrupted() {
                task.clear_interrupted();
                task.set_interruptible(false);
                return Err(Errno::EINTR);
            }
            pending.accept_waiters.push_back(task.tid());
            if let Some(deadline) = deadline_us {
                register_task_timeout_us(task.tid(), deadline);
            }
            let should_block = prepare_current_task_blocked();
            if !should_block {
                if deadline_us.is_some() {
                    finish_task_timeout(task.tid());
                }
                task.set_interruptible(false);
                pending.accept_waiters.retain(|tid| *tid != task.tid());
                return Err(Errno::ESRCH);
            }
            let interrupted = task.check_signal_interrupt() || task.is_interrupted();
            drop(pending);
            if interrupted {
                wakeup_task(task.tid());
            }
            let result = self.finish_interruptible_wait(deadline_us);
            listener
                .pending
                .lock()
                .accept_waiters
                .retain(|tid| *tid != task.tid());
            if result? {
                return Err(Errno::EAGAIN);
            }
        }
    }

    fn bound_key(&self) -> Option<Vec<u8>> {
        self.bound_key.lock().clone()
    }

    fn peer_key(&self) -> Option<Vec<u8>> {
        self.peer_key.lock().clone()
    }

    fn is_connected(&self) -> bool {
        self.peer_rx.lock().is_some()
    }

    fn peer_credentials(&self) -> SysResult<UnixPeerCredentials> {
        self.peer_credentials
            .lock()
            .as_ref()
            .copied()
            .ok_or(Errno::ENOTCONN)
    }

    fn shutdown(&self, how: usize) -> SysResult {
        if self.peer_rx.lock().is_none() {
            return Err(Errno::ENOTCONN);
        }
        if how == 0 || how == 2 {
            self.read_shutdown.store(true, Ordering::Release);
            let mut rx = self.rx.lock();
            rx.data.clear();
            let mut wake = core::mem::take(&mut rx.write_waiters);
            wake.append(&mut rx.read_waiters);
            let poll_waiters = rx.poll_waiters.clone();
            drop(rx);
            poll_waiters.notify(POLL_READ | POLL_WRITE | POLL_HUP);
            for tid in wake.drain(..) {
                wakeup_task(tid);
            }
        }
        if how == 1 || how == 2 {
            self.write_shutdown.store(true, Ordering::Release);
            if let Some(peer_rx) = self.peer_rx.lock().clone() {
                let mut peer_rx = peer_rx.lock();
                let mut wake = core::mem::take(&mut peer_rx.read_waiters);
                wake.append(&mut peer_rx.write_waiters);
                let poll_waiters = peer_rx.poll_waiters.clone();
                drop(peer_rx);
                poll_waiters.notify(POLL_READ | POLL_WRITE | POLL_HUP);
                for tid in wake.drain(..) {
                    wakeup_task(tid);
                }
            }
        }
        Ok(())
    }

    fn poll_hup(&self) -> bool {
        self.peer_closed
            .lock()
            .as_ref()
            .is_some_and(|closed| closed.load(Ordering::Acquire))
    }

    fn register_poll_waiter(&self, tid: usize, events: PollEvents) {
        self.rx
            .lock()
            .poll_waiters
            .register(tid, events & (POLL_READ | POLL_HUP));
        if let Some(peer_rx) = self.peer_rx.lock().clone() {
            peer_rx
                .lock()
                .poll_waiters
                .register(tid, events & POLL_WRITE);
        }
        if events & POLL_READ != 0 {
            if let Some(listener) = self.listener.lock().clone() {
                listener.poll_waiters.register(tid, POLL_READ);
            }
        }
    }

    fn unregister_poll_waiter(&self, tid: usize) {
        self.rx.lock().poll_waiters.unregister(tid);
        if let Some(peer_rx) = self.peer_rx.lock().clone() {
            peer_rx.lock().poll_waiters.unregister(tid);
        }
        if let Some(listener) = self.listener.lock().clone() {
            listener.poll_waiters.unregister(tid);
        }
    }

    fn close(&self) {
        let Some(listener) = self.listener.lock().take() else {
            return;
        };
        UNIX_LISTENERS
            .lock()
            .retain(|(_, item)| !Arc::ptr_eq(item, &listener));
    }
}

impl Drop for UnixSocket {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        self.read_shutdown.store(true, Ordering::Release);
        self.write_shutdown.store(true, Ordering::Release);
        let mut wake = VecDeque::new();
        if let Some(peer_rx) = self.peer_rx.lock().clone() {
            let mut peer_rx = peer_rx.lock();
            wake.append(&mut peer_rx.read_waiters);
            let poll_waiters = peer_rx.poll_waiters.clone();
            drop(peer_rx);
            poll_waiters.notify(POLL_READ | POLL_WRITE | POLL_HUP);
        }
        let mut rx = self.rx.lock();
        wake.append(&mut rx.write_waiters);
        let poll_waiters = rx.poll_waiters.clone();
        drop(rx);
        poll_waiters.notify(POLL_READ | POLL_WRITE | POLL_HUP);
        for tid in wake {
            wakeup_task(tid);
        }
        self.close();
    }
}

// ——— Socket ———

/// 用户态可见的套接字对象。
///
/// 实现 `FileOp`，可存入 fd_table 并通过 read/write 等系统调用操作。
pub struct Socket {
    /// 地址族（AF_INET / AF_UNIX / AF_INET6）。
    pub domain: SocketDomain,
    /// 套接字类型（SOCK_STREAM / SOCK_DGRAM）。
    pub kind: SocketKind,
    /// 内部协议实现。
    inner: SocketInner,
    /// 非阻塞标志。
    nonblock: AtomicBool,
    /// close-on-exec 标志。
    cloexec: AtomicBool,
    /// SO_SNDBUF 值。
    #[allow(dead_code)]
    send_buf_size: AtomicU64,
    /// SO_RCVBUF 值。
    #[allow(dead_code)]
    recv_buf_size: AtomicU64,
    /// SO_RCVTIMEO 值。
    #[allow(dead_code)]
    recvtimeout_us: Mutex<Option<usize>>,
    /// SO_SNDTIMEO 值。
    #[allow(dead_code)]
    sendtimeout_us: Mutex<Option<usize>>,
}

// SAFETY: 单核协作式调度，方法调用在系统调用路径上串行化。
unsafe impl Send for Socket {}
unsafe impl Sync for Socket {}

impl Socket {
    /// 创建一个新的套接字。
    pub fn new(
        domain: SocketDomain,
        socket_type: SocketKind,
        _protocol: usize,
    ) -> Result<Self, Errno> {
        let inner = match (&domain, socket_type) {
            (
                SocketDomain::AF_UNIX,
                SocketKind::SOCK_STREAM | SocketKind::SOCK_DGRAM | SocketKind::SOCK_SEQPACKET,
            ) => SocketInner::Unix(UnixSocket::new()),
            (SocketDomain::AF_INET | SocketDomain::AF_INET6, SocketKind::SOCK_STREAM) => {
                SocketInner::Tcp(TcpSocket::new())
            }
            (SocketDomain::AF_INET | SocketDomain::AF_INET6, SocketKind::SOCK_DGRAM) => {
                SocketInner::Udp(UdpSocket::new())
            }
            (_, SocketKind::SOCK_RAW | SocketKind::SOCK_SEQPACKET) => {
                return Err(Errno::EPROTONOSUPPORT);
            }
        };
        Ok(Socket {
            domain,
            kind: socket_type,
            inner,
            nonblock: AtomicBool::new(false),
            cloexec: AtomicBool::new(false),
            send_buf_size: AtomicU64::new(64 * 1024),
            recv_buf_size: AtomicU64::new(64 * 1024),
            recvtimeout_us: Mutex::new(None),
            sendtimeout_us: Mutex::new(None),
        })
    }

    pub fn new_unix_pair(socket_type: SocketKind) -> Result<(Self, Self), Errno> {
        if !matches!(
            socket_type,
            SocketKind::SOCK_STREAM | SocketKind::SOCK_DGRAM | SocketKind::SOCK_SEQPACKET
        ) {
            return Err(Errno::EINVAL);
        }
        let credentials = current_unix_credentials()?;
        let (left, right) = UnixSocket::pair(credentials);
        let make = |inner| Socket {
            domain: SocketDomain::AF_UNIX,
            kind: socket_type,
            inner: SocketInner::Unix(inner),
            nonblock: AtomicBool::new(false),
            cloexec: AtomicBool::new(false),
            send_buf_size: AtomicU64::new(64 * 1024),
            recv_buf_size: AtomicU64::new(64 * 1024),
            recvtimeout_us: Mutex::new(None),
            sendtimeout_us: Mutex::new(None),
        };
        Ok((make(left), make(right)))
    }

    pub fn set_nonblocking(&self, block: bool) {
        self.nonblock.store(block, Ordering::Release);
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.set_nonblocking(block),
            SocketInner::Udp(udp) => udp.set_nonblocking(block),
            SocketInner::Unix(unix) => unix.set_nonblocking(block),
        }
    }

    pub fn is_nonblocking(&self) -> bool {
        self.nonblock.load(Ordering::Acquire)
    }

    /// 设置 FD_CLOEXEC 标志。
    pub fn set_close_on_exec(&self, is_set: bool) {
        self.cloexec.store(is_set, Ordering::Release);
    }

    pub fn socket_type_value(&self) -> i32 {
        self.kind as i32
    }

    pub fn set_reuse_addr(&self, reuse: bool) {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.set_reuse_addr(reuse),
            SocketInner::Udp(udp) => udp.set_reuse_addr(reuse),
            SocketInner::Unix(_) => {}
        }
    }

    pub fn set_send_buf_size(&self, size: u64) {
        self.send_buf_size.store(size, Ordering::Release);
    }

    pub fn send_buf_size(&self) -> u64 {
        self.send_buf_size.load(Ordering::Acquire)
    }

    pub fn set_recv_buf_size(&self, size: u64) {
        self.recv_buf_size.store(size, Ordering::Release);
    }

    pub fn recv_buf_size(&self) -> u64 {
        self.recv_buf_size.load(Ordering::Acquire)
    }

    pub fn set_recv_timeout_us(&self, timeout_us: Option<usize>) {
        *self.recvtimeout_us.lock() = timeout_us;
    }

    pub fn recv_timeout_us(&self) -> Option<usize> {
        *self.recvtimeout_us.lock()
    }

    pub fn set_send_timeout_us(&self, timeout_us: Option<usize>) {
        *self.sendtimeout_us.lock() = timeout_us;
    }

    pub fn send_timeout_us(&self) -> Option<usize> {
        *self.sendtimeout_us.lock()
    }

    pub fn take_socket_error(&self) -> i32 {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.take_error(),
            SocketInner::Udp(_) | SocketInner::Unix(_) => 0,
        }
    }

    fn recv_deadline_us(&self, per_call_nonblocking: bool) -> Option<usize> {
        if self.is_nonblocking() || per_call_nonblocking {
            None
        } else {
            self.recv_timeout_us()
                .map(|timeout| get_timeout_us().saturating_add(timeout))
        }
    }

    fn send_deadline_us(&self, per_call_nonblocking: bool) -> Option<usize> {
        if self.is_nonblocking() || per_call_nonblocking {
            None
        } else {
            self.send_timeout_us()
                .map(|timeout| get_timeout_us().saturating_add(timeout))
        }
    }

    pub fn set_tcp_nodelay(&self, enabled: bool) -> SysResult {
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                tcp.set_nagle_enabled(!enabled);
                Ok(())
            }
            _ => Err(Errno::ENOPROTOOPT),
        }
    }

    pub fn tcp_nodelay(&self) -> SysResult<bool> {
        match &self.inner {
            SocketInner::Tcp(tcp) => Ok(!tcp.nagle_enabled()),
            _ => Err(Errno::ENOPROTOOPT),
        }
    }

    pub fn set_hop_limit(&self, limit: u8) -> SysResult {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.set_hop_limit(limit),
            SocketInner::Udp(udp) => {
                udp.set_socket_ttl(limit);
                Ok(())
            }
            SocketInner::Unix(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    /// 获取绑定地址（getsockname）。
    pub fn get_bound_address(&self) -> Result<SocketAddr, Errno> {
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                let local_addr = tcp.local_addr()?;
                Ok(from_ipendpoint_to_socketaddr(local_addr))
            }
            SocketInner::Udp(udp) => udp.local_addr(),
            SocketInner::Unix(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    pub fn get_bound_unix_key(&self) -> SysResult<Option<Vec<u8>>> {
        match &self.inner {
            SocketInner::Unix(unix) => Ok(unix.bound_key()),
            SocketInner::Tcp(_) | SocketInner::Udp(_) => Err(Errno::EAFNOSUPPORT),
        }
    }

    pub fn get_peer_unix_key(&self) -> SysResult<Option<Vec<u8>>> {
        match &self.inner {
            SocketInner::Unix(unix) if unix.is_connected() => Ok(unix.peer_key()),
            SocketInner::Unix(_) => Err(Errno::ENOTCONN),
            SocketInner::Tcp(_) | SocketInner::Udp(_) => Err(Errno::EAFNOSUPPORT),
        }
    }

    pub fn ensure_unix_connected(&self) -> SysResult {
        match &self.inner {
            SocketInner::Unix(unix) if unix.is_connected() => Ok(()),
            SocketInner::Unix(_) => Err(Errno::ENOTCONN),
            SocketInner::Tcp(_) | SocketInner::Udp(_) => Err(Errno::EAFNOSUPPORT),
        }
    }

    pub fn unix_peer_credentials(&self) -> SysResult<UnixPeerCredentials> {
        match &self.inner {
            SocketInner::Unix(unix) => unix.peer_credentials(),
            SocketInner::Tcp(_) | SocketInner::Udp(_) => Err(Errno::ENOPROTOOPT),
        }
    }

    /// 获取对端地址（getpeername）。
    pub fn get_remote_addr(&self) -> Result<SocketAddr, Errno> {
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                let remote_addr = tcp.remote_addr()?;
                Ok(from_ipendpoint_to_socketaddr(remote_addr))
            }
            SocketInner::Udp(udp) => udp.remote_addr(),
            SocketInner::Unix(_) => Err(Errno::ENOTCONN),
        }
    }

    /// 绑定到本地地址。
    pub fn bind(&self, local_addr: SocketAddr) -> SysResult {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.bind(local_addr),
            SocketInner::Udp(udp) => udp.bind(local_addr),
            SocketInner::Unix(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    pub fn bind_unix_path(&self, key: &[u8]) -> SysResult {
        match &self.inner {
            SocketInner::Unix(unix) => unix.bind_path(key),
            SocketInner::Tcp(_) | SocketInner::Udp(_) => Err(Errno::EAFNOSUPPORT),
        }
    }

    pub fn ensure_unix_unbound(&self) -> SysResult {
        match &self.inner {
            SocketInner::Unix(unix) => unix.ensure_unbound(),
            SocketInner::Tcp(_) | SocketInner::Udp(_) => Err(Errno::EAFNOSUPPORT),
        }
    }

    /// 开始监听（仅 TCP）。
    pub fn listen(&self, backlog: usize) -> SysResult {
        if !matches!(
            self.kind,
            SocketKind::SOCK_STREAM | SocketKind::SOCK_SEQPACKET
        ) {
            return Err(Errno::EOPNOTSUPP);
        }
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.listen(backlog),
            SocketInner::Unix(unix) => unix.listen(),
            SocketInner::Udp(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    /// 接受入站连接（仅 TCP），返回新的已连接 Socket 和对端地址。
    pub fn accept(&self) -> Result<(Self, SocketAddr), Errno> {
        if !matches!(
            self.kind,
            SocketKind::SOCK_STREAM | SocketKind::SOCK_SEQPACKET
        ) {
            return Err(Errno::EOPNOTSUPP);
        }
        let deadline_us = self.recv_deadline_us(false);
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                let new_tcp = tcp.accept(deadline_us)?;
                let remote_addr = match new_tcp.remote_addr() {
                    Ok(a) => a,
                    Err(_) => UNSPECIFIED_ENDPOINT,
                };
                Ok((
                    Socket {
                        domain: self.domain.clone(),
                        kind: self.kind,
                        inner: SocketInner::Tcp(new_tcp),
                        nonblock: AtomicBool::new(false),
                        cloexec: AtomicBool::new(false),
                        send_buf_size: AtomicU64::new(64 * 1024),
                        recv_buf_size: AtomicU64::new(64 * 1024),
                        recvtimeout_us: Mutex::new(None),
                        sendtimeout_us: Mutex::new(None),
                    },
                    from_ipendpoint_to_socketaddr(remote_addr),
                ))
            }
            SocketInner::Unix(unix) => Ok((
                Socket {
                    domain: self.domain.clone(),
                    kind: self.kind,
                    inner: SocketInner::Unix(unix.accept(deadline_us)?),
                    nonblock: AtomicBool::new(false),
                    cloexec: AtomicBool::new(false),
                    send_buf_size: AtomicU64::new(64 * 1024),
                    recv_buf_size: AtomicU64::new(64 * 1024),
                    recvtimeout_us: Mutex::new(None),
                    sendtimeout_us: Mutex::new(None),
                },
                from_ipendpoint_to_socketaddr(UNSPECIFIED_ENDPOINT),
            )),
            SocketInner::Udp(_) => Err(Errno::EOPNOTSUPP),
        }
    }

    /// 连接到远程地址。
    pub fn connect(&self, addr: SocketAddr) -> Result<(), Errno> {
        let deadline_us = self.send_deadline_us(false);
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.connect(addr, deadline_us),
            SocketInner::Udp(udp) => udp.connect(addr),
            SocketInner::Unix(_) => Err(Errno::ENOENT),
        }
    }

    pub fn connect_unix_path(&self, key: &[u8]) -> SysResult {
        match &self.inner {
            SocketInner::Unix(unix) => unix.connect_path(key),
            SocketInner::Tcp(_) | SocketInner::Udp(_) => Err(Errno::EAFNOSUPPORT),
        }
    }

    /// 关闭套接字的一端或两端。
    pub fn shutdown(&self, how: usize) -> SysResult {
        if how > 2 {
            return Err(Errno::EINVAL);
        }
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                tcp.shutdown(how)?;
            }
            SocketInner::Udp(udp) => {
                udp.shutdown();
            }
            SocketInner::Unix(unix) => unix.shutdown(how)?,
        }
        Ok(())
    }

    /// 向指定地址发送数据（sendto）。
    pub fn send_to(
        &self,
        buf: &[u8],
        addr: SocketAddr,
        per_call_nonblocking: bool,
    ) -> Result<usize, Errno> {
        let deadline_us = self.send_deadline_us(per_call_nonblocking);
        match &self.inner {
            SocketInner::Udp(udp) => udp.send_to(buf, addr, per_call_nonblocking, deadline_us),
            SocketInner::Tcp(tcp) => tcp.send(buf, per_call_nonblocking, deadline_us),
            SocketInner::Unix(unix) => unix.write(buf, per_call_nonblocking, deadline_us),
        }
    }

    /// 向已连接的对端发送数据。
    pub fn send(&self, buf: &[u8], per_call_nonblocking: bool) -> Result<usize, Errno> {
        let deadline_us = self.send_deadline_us(per_call_nonblocking);
        match &self.inner {
            SocketInner::Udp(udp) => udp.send(buf, per_call_nonblocking, deadline_us),
            SocketInner::Tcp(tcp) => tcp.send(buf, per_call_nonblocking, deadline_us),
            SocketInner::Unix(unix) => unix.write(buf, per_call_nonblocking, deadline_us),
        }
    }

    /// 接收数据并返回发送方地址（recvfrom）。
    pub fn recv_from(
        &self,
        buf: &mut [u8],
        per_call_nonblocking: bool,
        peek: bool,
        waitall: bool,
    ) -> Result<(usize, SocketAddr), Errno> {
        let deadline_us = self.recv_deadline_us(per_call_nonblocking);
        match &self.inner {
            SocketInner::Udp(udp) => udp.recv_from(buf, per_call_nonblocking, deadline_us, peek),
            SocketInner::Tcp(tcp) => {
                let len = tcp.recv(buf, per_call_nonblocking, deadline_us, peek, waitall)?;
                let remote_addr = tcp.remote_addr().unwrap_or(UNSPECIFIED_ENDPOINT);
                Ok((len, from_ipendpoint_to_socketaddr(remote_addr)))
            }
            SocketInner::Unix(unix) => {
                let len = unix.read(buf, per_call_nonblocking, deadline_us, peek, waitall)?;
                Ok((len, from_ipendpoint_to_socketaddr(UNSPECIFIED_ENDPOINT)))
            }
        }
    }

    /// 查询可读或可写状态（内部 poll + 状态检查）。
    fn tcp_poll(&self, isread: bool) -> bool {
        poll_interfaces();
        match &self.inner {
            SocketInner::Tcp(tcp) => {
                let state = tcp.poll(isread);
                if isread {
                    state.readable
                } else {
                    state.writeable
                }
            }
            SocketInner::Udp(udp) => {
                let state = udp.poll();
                if isread {
                    state.readable
                } else {
                    state.writeable
                }
            }
            SocketInner::Unix(unix) => {
                if isread {
                    unix.read_ready()
                        || unix
                            .listener
                            .lock()
                            .as_ref()
                            .is_some_and(|listener| !listener.pending.lock().sockets.is_empty())
                } else {
                    unix.write_ready()
                }
            }
        }
    }
}

// ——— impl FileOp for Socket ———
// Socket 通过实现 FileOp 接入内核的 VFS 层，可以存入 fd_table，
// 并通过 read / write / poll 等标准接口操作。

impl FileOp for Socket {
    fn as_any(&self) -> &dyn core::any::Any {
        self
    }

    fn splice_supported(&self) -> bool {
        true
    }

    fn validate_splice_read(&self) -> SysResult {
        match &self.inner {
            SocketInner::Unix(unix) if !unix.is_connected() => Err(Errno::EINVAL),
            SocketInner::Tcp(_) | SocketInner::Udp(_) | SocketInner::Unix(_) => Ok(()),
        }
    }

    fn read<'a>(&'a self, buf: &'a mut [u8]) -> SysResult<usize> {
        let deadline_us = self.recv_deadline_us(false);
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.recv(buf, false, deadline_us, false, false),
            SocketInner::Udp(udp) => {
                let (len, _addr) = udp.recv_from(buf, false, deadline_us, false)?;
                Ok(len)
            }
            SocketInner::Unix(unix) => unix.read(buf, false, deadline_us, false, false),
        }
    }

    fn write<'a>(&'a self, buf: &'a [u8]) -> SysResult<usize> {
        let deadline_us = self.send_deadline_us(false);
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.send(buf, false, deadline_us),
            SocketInner::Udp(udp) => udp.send(buf, false, deadline_us),
            SocketInner::Unix(unix) => unix.write(buf, false, deadline_us),
        }
    }

    /// 非阻塞可读：poll 网络接口后检查 socket 是否有数据。
    fn read_ready(&self) -> bool {
        self.tcp_poll(true)
    }

    /// 非阻塞可写：poll 网络接口后检查 socket 是否可写。
    fn write_ready(&self) -> bool {
        self.tcp_poll(false)
    }

    fn poll_hup(&self) -> bool {
        match &self.inner {
            SocketInner::Unix(unix) => unix.poll_hup(),
            SocketInner::Tcp(_) | SocketInner::Udp(_) => false,
        }
    }

    fn poll_error(&self) -> bool {
        match &self.inner {
            SocketInner::Tcp(tcp) => tcp.has_pending_error(),
            SocketInner::Udp(_) | SocketInner::Unix(_) => false,
        }
    }

    fn register_poll_waiter(&self, tid: usize, events: PollEvents) -> bool {
        match &self.inner {
            SocketInner::Unix(unix) => {
                unix.register_poll_waiter(tid, events);
                true
            }
            SocketInner::Tcp(_) | SocketInner::Udp(_) => false,
        }
    }

    fn unregister_poll_waiter(&self, tid: usize) {
        if let SocketInner::Unix(unix) = &self.inner {
            unix.unregister_poll_waiter(tid);
        }
    }

    fn readable(&self) -> bool {
        true
    }

    fn writable(&self) -> bool {
        true
    }

    /// 套接字不支持 seek。
    fn can_seek(&self) -> SysResult {
        Err(Errno::ESPIPE)
    }

    fn seek(&self, _offset: isize) -> SysResult<usize> {
        Err(Errno::ESPIPE)
    }

    fn get_offset(&self) -> usize {
        0
    }

    fn get_flags(&self) -> OpenFlags {
        let mut flags = OpenFlags::O_RDWR;
        if self.is_nonblocking() {
            flags |= OpenFlags::O_NONBLOCK;
        }
        if self.cloexec.load(Ordering::Acquire) {
            flags |= OpenFlags::O_CLOEXEC;
        }
        flags
    }

    fn set_status_flags(&self, flags: OpenFlags) -> SysResult {
        self.set_nonblocking(flags.contains(OpenFlags::O_NONBLOCK));
        Ok(())
    }

    fn get_stat(&self) -> SysResult<KStat> {
        Ok(KStat::minimal(0, InodeType::Socket))
    }

    fn fsync(&self) -> SysResult<usize> {
        Err(Errno::EINVAL)
    }
}

// ——— 辅助函数 ———

/// 将系统调用中的 domain 参数解析为 `SocketDomain`。
pub fn parse_domain(domain: usize) -> Result<SocketDomain, Errno> {
    match domain {
        1 => Ok(SocketDomain::AF_UNIX),
        2 => Ok(SocketDomain::AF_INET),
        10 => Ok(SocketDomain::AF_INET6),
        _ => Err(Errno::EAFNOSUPPORT),
    }
}

/// 将系统调用中的 type 参数解析为 `SocketKind`。
pub fn parse_kind(kind: usize) -> Result<SocketKind, Errno> {
    match kind & 0xFF {
        1 => Ok(SocketKind::SOCK_STREAM),
        2 => Ok(SocketKind::SOCK_DGRAM),
        3 => Ok(SocketKind::SOCK_RAW),
        5 => Ok(SocketKind::SOCK_SEQPACKET),
        _ => Err(Errno::EINVAL),
    }
}

/// `socket()` 系统调用的 type 参数中的 SOCK_NONBLOCK 标志位。
pub const SOCK_NONBLOCK: usize = 0x800;
/// `socket()` 系统调用的 type 参数中的 SOCK_CLOEXEC 标志位。
pub const SOCK_CLOEXEC: usize = 0x80000;
