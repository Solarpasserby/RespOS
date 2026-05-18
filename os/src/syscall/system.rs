// os/src/syscall/utils.rs

use crate::mm::copy_to_user;
use super::SysResult;

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
        Self {
            sysname: Self::from_str("RespOS"),
            nodename: Self::from_str("LAPTOP"),
            release: Self::from_str("0.1.0-dev"),
            version: Self::from_str("Resp0S 0.1.0"),
            machine: Self::from_str("riscv64"),
            domainname: Self::from_str("localdomain"),
        }
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
