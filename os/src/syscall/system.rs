// os/src/syscall/utils.rs

use super::SysResult;
use crate::arch::sbi;
use crate::config::{MEMORY_END, MEMORY_START, PAGE_SIZE};
use crate::fs::ext4;
use crate::mm::{copy_to_user, free_frame_count};
use crate::task::TASK_MANAGER;
use crate::timer::get_time_ms;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct UtsName {
    // 系统名
    pub sysname: [u8; 65],
    // 网络主机名
    pub nodename: [u8; 65],
    // 发行编号
    pub release: [u8; 65],
    // 发行版本
    pub version: [u8; 65],
    // 硬件架构标识符
    pub machine: [u8; 65],
    // 域名
    pub domainname: [u8; 65],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct SysInfo {
    pub uptime: usize,
    pub loads: [usize; 3],
    pub totalram: usize,
    pub freeram: usize,
    pub sharedram: usize,
    pub bufferram: usize,
    pub totalswap: usize,
    pub freeswap: usize,
    pub procs: u16,
    pub totalhigh: usize,
    pub freehigh: usize,
    pub mem_unit: u32,
}

impl UtsName {
    fn from_str(info: &str) -> [u8; 65] {
        let mut field = [0u8; 65];
        let bytes = info.as_bytes();
        let len = bytes.len().min(64);
        field[..len].copy_from_slice(&bytes[..len]);
        field
    }
}

impl Default for UtsName {
    fn default() -> Self {
        #[cfg(target_arch = "riscv64")]
        let machine = "riscv64";
        #[cfg(target_arch = "loongarch64")]
        let machine = "loongarch64";

        Self {
            sysname: Self::from_str("RespOS"),
            nodename: Self::from_str("LAPTOP"),
            release: Self::from_str("6.10.0-dev"), // 为运行 glibc 程序所设
            version: Self::from_str("Resp0S 0.1.0"),
            machine: Self::from_str(machine),
            domainname: Self::from_str("localdomain"),
        }
    }
}

/// 系统调用 sys-syslog
pub fn sys_syslog(action: usize, buf: *mut u8, len: isize) -> SysResult<usize> {
    const SYSLOG_ACTION_CLOSE: usize = 0;
    const SYSLOG_ACTION_OPEN: usize = 1;
    const SYSLOG_ACTION_READ: usize = 2;
    const SYSLOG_ACTION_READ_ALL: usize = 3;
    const SYSLOG_ACTION_READ_CLEAR: usize = 4;
    const SYSLOG_ACTION_CLEAR: usize = 5;
    // const SYSLOG_ACTION_CONSOLE_OFF: usize = 6;
    // const SYSLOG_ACTION_CONSOLE_ON: usize = 7;
    // const SYSLOG_ACTION_CONSOLE_LEVEL: usize = 8;
    // const SYSLOG_ACTION_SIZE_UNREAD: usize = 9;
    const SYSLOG_ACTION_SIZE_BUFFER: usize = 10;

    match action {
        SYSLOG_ACTION_CLOSE | SYSLOG_ACTION_OPEN => Ok(0),
        SYSLOG_ACTION_READ | SYSLOG_ACTION_READ_ALL | SYSLOG_ACTION_READ_CLEAR => {
            if buf.is_null() || len <= 0 {
                return Ok(0);
            }
            let msg = b"<5>RespOS kernel log buffer\n\0";
            let n = (msg.len() - 1).min(len as usize);
            copy_to_user(buf, msg.as_ptr(), n)?;
            Ok(n)
        }
        SYSLOG_ACTION_CLEAR => Ok(0),
        SYSLOG_ACTION_SIZE_BUFFER => Ok(4096),
        _ => Err(super::Errno::ENOSYS),
    }
}

/// 系统调用 sys-uname
///
/// TODO：目前只做固定实现
pub fn sys_uname(buf: *mut UtsName) -> SysResult<usize> {
    let utsname = UtsName::default();
    copy_to_user(buf, &utsname as *const UtsName, 1)?;
    Ok(0)
}

pub fn sys_reboot() -> SysResult<usize> {
    ext4::shutdown();
    sbi::shutdown(false);
}

pub fn sys_sysinfo(buf: *mut SysInfo) -> SysResult<usize> {
    let totalram = MEMORY_END.saturating_sub(MEMORY_START);
    let info = SysInfo {
        uptime: get_time_ms() / 1000,
        loads: [0, 0, 0],
        totalram,
        freeram: free_frame_count() * PAGE_SIZE,
        sharedram: 0,
        bufferram: 0,
        totalswap: 0,
        freeswap: 0,
        procs: TASK_MANAGER.len() as u16,
        totalhigh: 0,
        freehigh: 0,
        mem_unit: 1,
    };
    copy_to_user(buf, &info as *const SysInfo, 1)?;
    Ok(0)
}
