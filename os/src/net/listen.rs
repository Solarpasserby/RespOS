use alloc::{boxed::Box, collections::vec_deque::VecDeque, vec::Vec};
use smoltcp::{
    iface::SocketHandle,
    socket::tcp::{self, State},
    wire::{IpAddress, IpEndpoint, IpListenEndpoint},
};

use crate::mutex::SpinLock;
use crate::syscall::{Errno, SysResult};

use super::{SocketSetWrapper, TcpProcEntry, socket_set};

const LISTEN_QUEUE_SIZE: usize = 128;

struct ListenTableEntry {
    listen_endpoint: IpListenEndpoint,
    #[allow(dead_code)]
    task_id: usize,
    listen_handles: VecDeque<SocketHandle>,
    accept_queue: VecDeque<SocketHandle>,
    queue_limit: usize,
}

impl ListenTableEntry {
    pub fn new(
        listen_endpoint: IpListenEndpoint,
        listen_handle: SocketHandle,
        backlog: usize,
    ) -> Self {
        let queue_limit = backlog.clamp(1, LISTEN_QUEUE_SIZE);
        let mut listen_handles = VecDeque::with_capacity(queue_limit);
        listen_handles.push_back(listen_handle);
        ListenTableEntry {
            listen_endpoint,
            task_id: 0,
            listen_handles,
            accept_queue: VecDeque::with_capacity(queue_limit),
            queue_limit,
        }
    }
}

/// 端口监听表。每项用 SpinLock 包装以实现内部可变性（&self 方法可修改）。
pub struct ListenTable {
    table: Box<[SpinLock<Option<Box<ListenTableEntry>>>]>,
    active_ports: Vec<u16>,
}

impl ListenTable {
    pub fn new() -> Self {
        let mut v: Vec<SpinLock<Option<Box<ListenTableEntry>>>> = Vec::with_capacity(65536);
        for _ in 0..65536 {
            v.push(SpinLock::new(None));
        }
        ListenTable {
            table: v.into_boxed_slice(),
            active_ports: Vec::new(),
        }
    }

    pub fn can_listen(&self, port: u16) -> bool {
        self.table[port as usize].lock().is_none()
    }

    pub fn listen(
        &mut self,
        listen_endpoint: IpListenEndpoint,
        listen_handle: SocketHandle,
        backlog: usize,
    ) -> SysResult {
        let port = listen_endpoint.port;
        debug_assert!(port != 0);
        let mut guard = self.table[port as usize].lock();
        if guard.is_none() {
            *guard = Some(Box::new(ListenTableEntry::new(
                listen_endpoint,
                listen_handle,
                backlog,
            )));
            if let Some(entry) = guard.as_mut() {
                replenish_listeners(entry);
            }
            self.active_ports.push(port);
            Ok(())
        } else {
            Err(Errno::EADDRINUSE)
        }
    }

    pub fn unlisten(&mut self, port: u16) {
        let mut guard = self.table[port as usize].lock();
        if let Some(entry) = guard.as_ref() {
            for &handle in entry.listen_handles.iter() {
                socket_set().lock().remove(handle);
            }
            for &handle in entry.accept_queue.iter() {
                socket_set().lock().remove(handle);
            }
        }
        *guard = None;
        self.active_ports.retain(|active_port| *active_port != port);
    }

    pub fn can_accept(&self, port: u16) -> bool {
        self.promote_listener(port);
        let guard = self.table[port as usize].lock();
        if let Some(entry) = guard.as_ref() {
            !entry.accept_queue.is_empty()
        } else {
            false
        }
    }

    pub fn accept(&self, port: u16) -> Result<(SocketHandle, (IpEndpoint, IpEndpoint)), Errno> {
        self.promote_listener(port);
        let mut guard = self.table[port as usize].lock();
        let entry = guard.as_mut().ok_or(Errno::ECONNREFUSED)?;
        while let Some(handle) = entry.accept_queue.pop_front() {
            if is_closed(handle) {
                socket_set().lock().remove(handle);
                continue;
            }
            if let Some(addr_tuple) = get_addr_tuple(handle) {
                replenish_listeners(entry);
                return Ok((handle, addr_tuple));
            }
            socket_set().lock().remove(handle);
        }
        Err(Errno::EAGAIN)
    }

    fn promote_listener(&self, port: u16) {
        let mut guard = self.table[port as usize].lock();
        let Some(entry) = guard.as_mut() else {
            return;
        };
        let mut pending = VecDeque::with_capacity(entry.queue_limit);
        while let Some(handle) = entry.listen_handles.pop_front() {
            if entry.accept_queue.len() < entry.queue_limit && is_connected(handle) {
                entry.accept_queue.push_back(handle);
            } else if is_pending_listener(handle) {
                pending.push_back(handle);
            } else {
                socket_set().lock().remove(handle);
            }
        }
        entry.listen_handles = pending;
        replenish_listeners(entry);
    }

    /// Move every newly established listening socket into its accept queue and
    /// install a replacement listener immediately.  smoltcp represents a
    /// listener with one socket, so waiting until userspace calls accept leaves
    /// a window where concurrent SYNs see no listening socket and are reset.
    pub fn promote_ready_listeners(&self) {
        for &port in self.active_ports.iter() {
            self.promote_listener(port);
        }
    }

    pub fn take_handle(&self, port: u16, handle: SocketHandle) {
        let mut guard = self.table[port as usize].lock();
        if let Some(entry) = guard.as_mut() {
            entry.accept_queue.retain(|&item| item != handle);
            entry.listen_handles.retain(|&item| item != handle);
        }
    }

    pub(crate) fn proc_entries(&self) -> Vec<TcpProcEntry> {
        let mut entries = Vec::new();
        for slot in self.table.iter() {
            let guard = slot.lock();
            let Some(entry) = guard.as_ref() else {
                continue;
            };
            let local_addr = entry
                .listen_endpoint
                .addr
                .unwrap_or(IpAddress::v4(0, 0, 0, 0));
            entries.push(TcpProcEntry::new(
                IpEndpoint::new(local_addr, entry.listen_endpoint.port),
                IpEndpoint::new(IpAddress::v4(0, 0, 0, 0), 0),
                State::Listen,
            ));
            for &handle in entry.accept_queue.iter() {
                if let Some((local, remote)) = get_addr_tuple(handle) {
                    entries.push(TcpProcEntry::new(local, remote, State::Established));
                }
            }
        }
        entries
    }
}

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

// SocketHandle does not own the corresponding SocketSet entry.  unlisten()
// therefore removes every handle explicitly before dropping this table entry.

fn is_connected(handle: SocketHandle) -> bool {
    super::socket_set()
        .lock()
        .with_socket::<_, tcp::Socket, _>(handle, |socket| {
            matches!(socket.state(), State::Established)
        })
}

fn is_pending_listener(handle: SocketHandle) -> bool {
    super::socket_set()
        .lock()
        .with_socket::<_, tcp::Socket, _>(handle, |socket| {
            matches!(socket.state(), State::Listen | State::SynReceived)
        })
}

fn is_closed(handle: SocketHandle) -> bool {
    super::socket_set()
        .lock()
        .with_socket::<_, tcp::Socket, _>(handle, |socket| matches!(socket.state(), State::Closed))
}

fn get_addr_tuple(handle: SocketHandle) -> Option<(IpEndpoint, IpEndpoint)> {
    super::socket_set()
        .lock()
        .with_socket::<_, tcp::Socket, _>(handle, |socket| {
            Some((socket.local_endpoint()?, socket.remote_endpoint()?))
        })
}
