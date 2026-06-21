// os/src/syscall/utils.rs

use super::{Errno, SysResult};
use crate::arch::sbi;
use crate::config::{MEMORY_END, MEMORY_START, PAGE_SIZE};
use crate::fs::ext4;
use crate::mm::{copy_from_user, copy_to_user, free_frame_count};
use crate::mutex::SpinLock;
use crate::task::{TASK_MANAGER, current_task};
use crate::timer::get_time_ms;
use lazy_static::lazy_static;

lazy_static! {
    static ref UTS_STATE: SpinLock<UtsState> = SpinLock::new(UtsState::default());
}

struct UtsState {
    nodename: [u8; 65],
    domainname: [u8; 65],
}

impl Default for UtsState {
    fn default() -> Self {
        Self {
            nodename: UtsName::from_str("LAPTOP"),
            domainname: UtsName::from_str("localdomain"),
        }
    }
}

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

impl UtsName {
    fn linux_default() -> Self {
        #[cfg(target_arch = "riscv64")]
        let machine = "riscv64";
        #[cfg(target_arch = "loongarch64")]
        let machine = "loongarch64";
        let state = UTS_STATE.lock();

        Self {
            sysname: Self::from_str("Linux"),
            nodename: state.nodename,
            release: Self::from_str("6.10.0-dev"), // 为运行 glibc 程序所设
            version: Self::from_str("Resp0S 0.1.0"),
            machine: Self::from_str(machine),
            domainname: state.domainname,
        }
    }

    fn with_personality(persona: usize) -> Self {
        const UNAME26: usize = 0x0002_0000;
        let mut uts = Self::linux_default();
        if persona & UNAME26 != 0 {
            // Linux 的 UNAME26 兼容位会把 3.x+ 内核版本伪装成 2.6.40+。
            uts.release = Self::from_str("2.6.40-dev");
        }
        uts
    }
}

impl Default for UtsName {
    fn default() -> Self {
        Self::linux_default()
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
pub fn sys_uname(buf: *mut UtsName) -> SysResult<usize> {
    let persona = current_task().map_or(0, |task| task.personality());
    let utsname = UtsName::with_personality(persona);
    copy_to_user(buf, &utsname as *const UtsName, 1)?;
    Ok(0)
}

fn set_uts_field(name: *const u8, len: usize, is_domainname: bool) -> SysResult<usize> {
    const MAX_UTS_FIELD_LEN: usize = 64;
    if len > MAX_UTS_FIELD_LEN {
        return Err(Errno::EINVAL);
    }

    let task = current_task().expect("[kernel] current task is None.");
    if task.euid() != 0 {
        return Err(Errno::EPERM);
    }

    let mut field = [0u8; 65];
    if len != 0 {
        copy_from_user(field.as_mut_ptr(), name, len)?;
    }

    let mut state = UTS_STATE.lock();
    if is_domainname {
        state.domainname = field;
    } else {
        state.nodename = field;
    }
    Ok(0)
}

pub fn sys_sethostname(name: *const u8, len: usize) -> SysResult<usize> {
    set_uts_field(name, len, false)
}

pub fn sys_setdomainname(name: *const u8, len: usize) -> SysResult<usize> {
    set_uts_field(name, len, true)
}

/// 系统调用 sys-personality。
///
pub fn sys_personality(persona: usize) -> SysResult<usize> {
    const GET_PERSONA: usize = usize::MAX;
    const PER_MASK: usize = 0x00ff;
    const ADDR_NO_RANDOMIZE: usize = 0x0040_0000;
    const UNAME26: usize = 0x0002_0000;
    const SUPPORTED_FLAGS: usize = ADDR_NO_RANDOMIZE | UNAME26;

    let task = current_task().expect("[kernel] current task is None.");
    let old = task.personality();
    if persona == GET_PERSONA {
        return Ok(old);
    }
    if persona & !(PER_MASK | SUPPORTED_FLAGS) != 0 {
        return Err(Errno::EINVAL);
    }
    task.set_personality(persona);
    Ok(old)
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
