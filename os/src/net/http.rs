//! In-kernel HTTP server that renders live system status.
//!
//! This is a diagnostic and demo service rather than a general-purpose web
//! server: it binds TCP port 80 on the non-loopback smoltcp interface (when
//! present) and answers every request with an HTML page generated from the
//! kernel's live counters. It reuses the existing [`ListenTable`] so the
//! listener pool is replenished by the ordinary `poll_interfaces` path, and it
//! is driven from the timer-service hart's idle loop so it keeps progressing
//! even when no user task performs socket syscalls.

use alloc::{format, string::String, vec::Vec};
use core::fmt::Write;

use smoltcp::{iface::SocketHandle, socket::tcp, wire::IpListenEndpoint};

use super::{LISTEN_TABLE, SocketSetWrapper, socket_set};

/// TCP port the HTTP server listens on.
pub const HTTP_PORT: u16 = 80;
/// Backlog for the HTTP listener pool.
const HTTP_BACKLOG: usize = 16;
/// Upper bound on accumulated request bytes per connection.
const RX_BUF_CAP: usize = 8192;
/// Upper bound on concurrent connections handled by this diagnostic server.
const MAX_CONNS: usize = 16;

pub struct HttpServer {
    port: u16,
    conns: Vec<HttpConn>,
}

struct HttpConn {
    handle: SocketHandle,
    rx: Vec<u8>,
    response: Option<Vec<u8>>,
    sent: usize,
    closing: bool,
}

impl HttpServer {
    /// Create the listener and register it in the shared listen table.
    ///
    /// Returns `None` when the port is already taken or a listener socket
    /// cannot be created.
    pub fn new() -> Option<Self> {
        let endpoint = IpListenEndpoint {
            addr: None,
            port: HTTP_PORT,
        };
        let mut listen_socket = SocketSetWrapper::new_tcp_socket();
        listen_socket.listen(endpoint).ok()?;
        let handle = socket_set().lock().add(listen_socket);
        if LISTEN_TABLE
            .lock()
            .listen(endpoint, handle, HTTP_BACKLOG)
            .is_err()
        {
            socket_set().lock().remove(handle);
            return None;
        }
        Some(Self {
            port: HTTP_PORT,
            conns: Vec::new(),
        })
    }

    /// Advance the server state machine. `poll_interfaces` must have run first
    /// so newly arrived packets and handshakes are already reflected in the
    /// socket states.
    pub fn poll(&mut self) {
        // Accept every completed handshake.
        while self.conns.len() < MAX_CONNS {
            match LISTEN_TABLE.lock().accept(self.port) {
                Ok((handle, _endpoints)) => {
                    self.conns.push(HttpConn {
                        handle,
                        rx: Vec::with_capacity(1024),
                        response: None,
                        sent: 0,
                        closing: false,
                    });
                }
                Err(_) => break,
            }
        }

        self.conns.retain_mut(|conn| conn.poll());
    }
}

impl HttpConn {
    /// Process one connection. Returns `false` when it should be removed.
    fn poll(&mut self) -> bool {
        if self.closing {
            let finished = socket_set()
                .lock()
                .with_socket::<_, tcp::Socket, _>(self.handle, |socket| !socket.is_active());
            if finished {
                socket_set().lock().remove(self.handle);
                return false;
            }
            return true;
        }

        // 1. Read whatever is available.
        if self.response.is_none() {
            let mut buf = [0u8; 1024];
            let (read, peer_closed) =
                socket_set()
                    .lock()
                    .with_socket_mut::<_, tcp::Socket, _>(self.handle, |socket| {
                        if !socket.may_recv() {
                            return (0usize, true);
                        }
                        match socket.recv_slice(&mut buf) {
                            Ok(n) => (n, false),
                            Err(_) => (0usize, true),
                        }
                    });

            if read > 0 && self.rx.len() < RX_BUF_CAP {
                let take = read.min(RX_BUF_CAP - self.rx.len());
                self.rx.extend_from_slice(&buf[..take]);
            }

            let request_done =
                peer_closed || request_headers_complete(&self.rx) || self.rx.len() >= RX_BUF_CAP;
            if request_done {
                self.response = Some(build_http_response());
            }
        }

        // 2. Send the response once it exists.
        if let Some(response) = &self.response {
            while self.sent < response.len() {
                let written = socket_set().lock().with_socket_mut::<_, tcp::Socket, _>(
                    self.handle,
                    |socket| {
                        if !socket.can_send() {
                            return 0usize;
                        }
                        socket.send_slice(&response[self.sent..]).unwrap_or(0)
                    },
                );
                if written == 0 {
                    break;
                }
                self.sent += written;
            }

            if self.sent >= response.len() {
                // Send FIN and let the peer drain it before removing the
                // socket, so the browser reliably sees the end of the body.
                socket_set()
                    .lock()
                    .with_socket_mut::<_, tcp::Socket, _>(self.handle, |socket| socket.close());
                self.closing = true;
            }
        }

        true
    }
}

/// Detect the end of the HTTP request headers.
fn request_headers_complete(rx: &[u8]) -> bool {
    rx.windows(4).any(|w| w == b"\r\n\r\n")
}

fn build_http_response() -> Vec<u8> {
    let body = status_html();
    let mut response = String::with_capacity(body.len() + 128);
    let _ = write!(response, "HTTP/1.1 200 OK\r\n");
    let _ = write!(response, "Content-Type: text/html; charset=utf-8\r\n");
    let _ = write!(response, "Content-Length: {}\r\n", body.len());
    let _ = write!(response, "Connection: close\r\n");
    let _ = write!(response, "Cache-Control: no-store\r\n");
    let _ = write!(response, "\r\n");
    let _ = write!(response, "{body}");
    response.into_bytes()
}

fn arch_name() -> &'static str {
    if cfg!(target_arch = "riscv64") {
        "RISC-V 64"
    } else {
        "LoongArch 64"
    }
}

fn fmt_bytes(bytes: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if bytes >= MIB {
        format!("{}.{:02} MiB", bytes / MIB, bytes % MIB * 100 / MIB)
    } else if bytes >= 1024 {
        format!("{} KiB", bytes / 1024)
    } else {
        format!("{bytes} B")
    }
}

fn fmt_duration_us(us: usize) -> String {
    let total_s = us / 1_000_000;
    let h = total_s / 3600;
    let m = total_s % 3600 / 60;
    let s = total_s % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn status_html() -> String {
    let uptime_us = crate::timer::get_timeout_us();
    let idle_us = crate::task::system_idle_time_us();
    let tasks = crate::task::TASK_MANAGER.len();
    let online_harts = crate::arch::smp::online_hart_mask().count_ones();
    let max_harts = crate::arch::smp::MAX_HARTS;

    let page_size = crate::config::PAGE_SIZE;
    let mem_total = crate::config::physical_memory_size();
    let mem_free = crate::mm::free_frame_count() * page_size;
    let mem_cached = crate::fs::page_cache_page_count() * page_size;
    let mem_dirty = crate::fs::page_cache_dirty_page_count() * page_size;
    let heap_used = crate::mm::heap_allocated();

    let guest_ip = super::guest_ip();
    let mac = super::eth_mac()
        .map(|m| {
            format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                m[0], m[1], m[2], m[3], m[4], m[5]
            )
        })
        .unwrap_or_else(|| String::from("n/a"));

    let mut out = String::with_capacity(2048);
    let _ = write!(
        out,
        "<!DOCTYPE html>\n<html lang=\"zh\"><head>\n\
         <meta charset=\"utf-8\">\n\
         <meta http-equiv=\"refresh\" content=\"2\">\n\
         <title>RespOS 内核内 HTTP 服务器</title>\n\
         <style>\
         body{{font-family:system-ui,monospace;margin:2rem auto;max-width:52rem;padding:0 1rem;}}\
         h1{{font-size:1.4rem;}}\
         .tag{{color:#2563eb;}}\
         table{{border-collapse:collapse;width:100%;margin-top:1rem;}}\
         td,th{{border:1px solid #d1d5db;padding:.4rem .6rem;text-align:left;}}\
         th{{background:#f3f4f6;}}\
         </style></head><body>\n\
         <h1>🦀 <span class=\"tag\">RespOS</span> 内核内 HTTP 服务器</h1>\n\
         <p>这个网页运行在 <strong>内核里</strong>：由 virtio-net + smoltcp 驱动，\
         <em>不经过任何用户态进程</em>，每次请求都实时渲染内核状态。</p>\n"
    );
    let _ = write!(out, "<table>\n<tr><th>项目</th><th>数值</th></tr>\n");
    let _ = writeln!(out, "<tr><td>架构</td><td>{}</td></tr>", arch_name());
    let _ = writeln!(out, "<tr><td>Guest IP</td><td>{}</td></tr>", guest_ip);
    let _ = writeln!(out, "<tr><td>MAC 地址</td><td>{}</td></tr>", mac);
    let _ = writeln!(
        out,
        "<tr><td>系统启动时间</td><td>{} (idle {})</td></tr>",
        fmt_duration_us(uptime_us),
        fmt_duration_us(idle_us)
    );
    let _ = writeln!(
        out,
        "<tr><td>在线 CPU</td><td>{} / {}</td></tr>",
        online_harts, max_harts
    );
    let _ = writeln!(out, "<tr><td>任务数</td><td>{}</td></tr>", tasks);
    let _ = writeln!(
        out,
        "<tr><td>物理内存总量</td><td>{}</td></tr>",
        fmt_bytes(mem_total)
    );
    let _ = writeln!(
        out,
        "<tr><td>空闲内存</td><td>{}</td></tr>",
        fmt_bytes(mem_free)
    );
    let _ = writeln!(
        out,
        "<tr><td>PageCache 缓存</td><td>{} (脏 {})</td></tr>",
        fmt_bytes(mem_cached),
        fmt_bytes(mem_dirty)
    );
    let _ = writeln!(
        out,
        "<tr><td>内核堆占用</td><td>{}</td></tr>",
        fmt_bytes(heap_used)
    );
    let _ = write!(out, "</table>\n</body></html>\n");
    out
}
