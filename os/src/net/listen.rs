use alloc::{boxed::Box, collections::vec_deque::VecDeque, vec::Vec};
use smoltcp::{
    iface::SocketHandle,
    socket::tcp::{self, State},
    wire::{IpEndpoint, IpListenEndpoint},
};

use crate::mutex::SpinLock;
use crate::syscall::{Errno, SysResult};

use super::{socket_set, SocketSetWrapper};

const LISTEN_QUEUE_SIZE: usize = 512;

struct ListenTableEntry {
    listen_endpoint: IpListenEndpoint,
    #[allow(dead_code)]
    task_id: usize,
    listen_handle: SocketHandle,
    accept_queue: VecDeque<SocketHandle>,
}

impl ListenTableEntry {
    pub fn new(listen_endpoint: IpListenEndpoint, listen_handle: SocketHandle) -> Self {
        ListenTableEntry {
            listen_endpoint,
            task_id: 0,
            listen_handle,
            accept_queue: VecDeque::with_capacity(LISTEN_QUEUE_SIZE),
        }
    }
}

/// 端口监听表。每项用 SpinLock 包装以实现内部可变性（&self 方法可修改）。
pub struct ListenTable {
    table: Box<[SpinLock<Option<Box<ListenTableEntry>>>]>,
}

impl ListenTable {
    pub fn new() -> Self {
        let mut v: Vec<SpinLock<Option<Box<ListenTableEntry>>>> = Vec::with_capacity(65536);
        for _ in 0..65536 {
            v.push(SpinLock::new(None));
        }
        ListenTable {
            table: v.into_boxed_slice(),
        }
    }

    pub fn can_listen(&self, port: u16) -> bool {
        self.table[port as usize].lock().is_none()
    }

    pub fn listen(&self, listen_endpoint: IpListenEndpoint, listen_handle: SocketHandle) -> SysResult {
        let port = listen_endpoint.port;
        debug_assert!(port != 0);
        let mut guard = self.table[port as usize].lock();
        if guard.is_none() {
            *guard = Some(Box::new(ListenTableEntry::new(listen_endpoint, listen_handle)));
            Ok(())
        } else {
            Err(Errno::EADDRINUSE)
        }
    }

    pub fn unlisten(&self, port: u16) {
        let mut guard = self.table[port as usize].lock();
        if let Some(entry) = guard.as_ref() {
            socket_set().lock().remove(entry.listen_handle);
            for &handle in entry.accept_queue.iter() {
                socket_set().lock().remove(handle);
            }
        }
        *guard = None;
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
        let handle = entry.accept_queue.pop_front().ok_or(Errno::EAGAIN)?;
        if is_closed(handle) {
            Err(Errno::ECONNRESET)
        } else {
            Ok((handle, get_addr_tuple(handle)))
        }
    }

    fn promote_listener(&self, port: u16) {
        let mut guard = self.table[port as usize].lock();
        let Some(entry) = guard.as_mut() else {
            return;
        };
        if entry.accept_queue.len() >= LISTEN_QUEUE_SIZE {
            return;
        }
        if !is_connected(entry.listen_handle) {
            return;
        }

        let old_handle = entry.listen_handle;
        let mut socket = SocketSetWrapper::new_tcp_socket();
        if socket.listen(entry.listen_endpoint).is_err() {
            return;
        }
        let new_handle = socket_set().lock().add(socket);
        entry.listen_handle = new_handle;
        entry.accept_queue.push_back(old_handle);
    }

    pub fn take_handle(&self, port: u16, handle: SocketHandle) {
        let mut guard = self.table[port as usize].lock();
        if let Some(entry) = guard.as_mut() {
            entry.accept_queue.retain(|&item| item != handle);
            if entry.listen_handle == handle {
                return;
            }
        }
    }
}

// Drop for ListenTableEntry is handled automatically — when guard is dropped,
// Box<ListenTableEntry> is dropped, and the syn_queue's SocketHandles are
// cleaned up via the SOCKET_SET. But we don't hook into that here since
// the Drop impl would need to lock the global socket set.

fn is_connected(handle: SocketHandle) -> bool {
    super::socket_set()
        .lock()
        .with_socket::<_, tcp::Socket, _>(handle, |socket| {
            matches!(socket.state(), State::Established)
        })
}

fn is_closed(handle: SocketHandle) -> bool {
    super::socket_set()
        .lock()
        .with_socket::<_, tcp::Socket, _>(handle, |socket| {
            matches!(socket.state(), State::Closed)
        })
}

fn get_addr_tuple(handle: SocketHandle) -> (IpEndpoint, IpEndpoint) {
    super::socket_set()
        .lock()
        .with_socket::<_, tcp::Socket, _>(handle, |socket| {
            (
                socket.local_endpoint().unwrap(),
                socket.remote_endpoint().unwrap(),
            )
        })
}
