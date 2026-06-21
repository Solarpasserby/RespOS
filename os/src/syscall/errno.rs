pub type SysResult<T = ()> = Result<T, Errno>;

#[repr(isize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Errno {
    /// 操作不允许，例如权限不足或当前状态不允许执行该操作。
    EPERM = 1,
    /// 没有这个文件或目录。
    ENOENT = 2,
    /// 没有这个进程。
    ESRCH = 3,
    /// 系统调用被信号中断。
    EINTR = 4,
    /// 输入输出错误，常用于底层块设备或文件系统读写失败。
    EIO = 5,
    /// 没有这个设备或地址。
    ENXIO = 6,
    /// 参数列表过长。
    E2BIG = 7,
    /// 可执行文件格式错误。
    ENOEXEC = 8,
    /// 无效的文件描述符。
    EBADF = 9,
    /// 没有子进程。
    ECHILD = 10,
    /// 资源暂时不可用，稍后可重试。
    EAGAIN = 11,
    /// 内存不足，无法拓展/分配页帧。
    ENOMEM = 12,
    /// 权限不足。
    EACCES = 13,
    /// 用户地址无效，或者无法访问用户传入的指针。
    EFAULT = 14,
    /// 设备或资源正忙。
    EBUSY = 16,
    /// 文件已经存在。
    EEXIST = 17,
    /// 跨设备链接。
    EXDEV = 18,
    /// 没有这个设备。
    ENODEV = 19,
    /// 路径中的某一项不是目录。
    ENOTDIR = 20,
    /// 目标是目录，但当前操作要求普通文件。
    EISDIR = 21,
    /// 参数无效。
    EINVAL = 22,
    /// 系统范围内打开的文件过多。
    ENFILE = 23,
    /// 当前进程打开的文件过多。
    EMFILE = 24,
    /// 不是终端设备，常用于不支持终端相关 ioctl 的文件。
    ENOTTY = 25,
    /// 设备或文件系统空间不足。
    ENOSPC = 28,
    /// 非法 seek，例如对 pipe、socket 等不可 seek 对象执行 lseek。
    ESPIPE = 29,
    /// 只读文件系统。
    EROFS = 30,
    /// 硬链接数过多。
    EMLINK = 31,
    /// 管道破裂，例如向没有读端的 pipe 写入。
    EPIPE = 32,
    /// 文件描述符不是 socket。
    ENOTSOCK = 88,
    /// 结果过大，常用于用户缓冲区太小。
    ERANGE = 34,
    /// 文件名过长。
    ENAMETOOLONG = 36,
    /// 系统调用或功能尚未实现。
    ENOSYS = 38,
    /// 目录非空。
    ENOTEMPTY = 39,
    /// 符号链接层数过多，通常表示符号链接循环。
    ELOOP = 40,
    /// 需要目标地址（socket 未指定 peer 地址）。
    EDESTADDRREQ = 89,
    /// 消息过长（UDP 数据报超过 MTU）。
    EMSGSIZE = 90,
    /// 协议选项不可用。
    ENOPROTOOPT = 92,
    /// 协议不支持，常用于 socket domain/type/protocol 组合无效。
    EPROTONOSUPPORT = 93,
    /// 操作不支持，例如给 preadv2/pwritev2 传了内核未实现的 flags。
    EOPNOTSUPP = 95,
    /// 不支持该 socket address family。
    EAFNOSUPPORT = 97,
    /// 地址已被占用（bind 端口冲突）。
    EADDRINUSE = 98,
    /// 无法分配请求的地址（bind 到非本机地址）。
    EADDRNOTAVAIL = 99,
    /// 网络不可达。
    ENETUNREACH = 101,
    /// 网络连接丢失。
    ENETRESET = 102,
    /// 连接被对端中止。
    ECONNABORTED = 103,
    /// 连接被对端重置。
    ECONNRESET = 104,
    /// 传输端点已连接（connect 已连接过的 socket）。
    EISCONN = 106,
    /// 传输端点未连接（send/recv 未连接的 socket）。
    ENOTCONN = 107,
    /// 操作超时。
    ETIMEDOUT = 110,
    /// 连接被拒绝。
    ECONNREFUSED = 111,
    /// 操作已在进行中。
    EALREADY = 114,
    /// 操作正在进行中（非阻塞 connect 等）。
    EINPROGRESS = 115,
    /// 我不知道。
    EIDONTKNONW = 114514,
}

impl Errno {
    /// 返回正的 errno 编号，例如 EINVAL -> 22
    pub fn code(self) -> isize {
        self as isize
    }

    /// 返回系统调用约定中的错误返回值，例如 EINVAL -> -22
    pub fn as_ret(self) -> isize {
        -(self as isize)
    }

    /// 返回英文错误说明，主要用于内核日志和调试输出
    /// 描述文本参考 Linux 内核源码及 strerror 标准实现 —— 千问
    pub fn message(self) -> &'static str {
        match self {
            Errno::EPERM => "Operation not permitted",
            Errno::ENOENT => "No such file or directory",
            Errno::ESRCH => "No such process",
            Errno::EINTR => "Interrupted system call",
            Errno::EIO => "Input/output error",
            Errno::ENXIO => "No such device or address",
            Errno::E2BIG => "Argument list too long",
            Errno::ENOEXEC => "Exec format error",
            Errno::EBADF => "Bad file descriptor",
            Errno::ECHILD => "No child processes",
            Errno::EAGAIN => "Resource temporarily unavailable",
            Errno::ENOMEM => "Cannot allocate memory",
            Errno::EACCES => "Permission denied",
            Errno::EFAULT => "Bad address",
            Errno::EBUSY => "Device or resource busy",
            Errno::EEXIST => "File exists",
            Errno::EXDEV => "Cross-device link",
            Errno::ENODEV => "No such device",
            Errno::ENOTDIR => "Not a directory",
            Errno::EISDIR => "Is a directory",
            Errno::EINVAL => "Invalid argument",
            Errno::ENFILE => "Too many open files in system",
            Errno::EMFILE => "Too many open files",
            Errno::ENOTTY => "Not a typewriter", // 历史遗留术语，指代 TTY 设备，现在指代终端
            Errno::ENOSPC => "No space left on device",
            Errno::ESPIPE => "Illegal seek",
            Errno::ERANGE => "Result out of range",
            Errno::EROFS => "Read-only file system",
            Errno::EMLINK => "Too many links",
            Errno::EPIPE => "Broken pipe",
            Errno::ENOTSOCK => "Socket operation on non-socket",
            Errno::ENAMETOOLONG => "File name too long",
            Errno::ENOSYS => "Function not implemented",
            Errno::ENOTEMPTY => "Directory not empty",
            Errno::ELOOP => "Too many levels of symbolic links",
            Errno::EDESTADDRREQ => "Destination address required",
            Errno::EMSGSIZE => "Message too long",
            Errno::ENOPROTOOPT => "Protocol not available",
            Errno::EPROTONOSUPPORT => "Protocol not supported",
            Errno::EOPNOTSUPP => "Operation not supported",
            Errno::EAFNOSUPPORT => "Address family not supported by protocol",
            Errno::EADDRINUSE => "Address already in use",
            Errno::EADDRNOTAVAIL => "Cannot assign requested address",
            Errno::ENETUNREACH => "Network is unreachable",
            Errno::ENETRESET => "Network dropped connection on reset",
            Errno::ECONNABORTED => "Software caused connection abort",
            Errno::ECONNRESET => "Connection reset by peer",
            Errno::EISCONN => "Transport endpoint is already connected",
            Errno::ENOTCONN => "Transport endpoint is not connected",
            Errno::ETIMEDOUT => "Connection timed out",
            Errno::ECONNREFUSED => "Connection refused",
            Errno::EALREADY => "Operation already in progress",
            Errno::EINPROGRESS => "Operation now in progress",
            Errno::EIDONTKNONW => "I don't know which is proper",
        }
    }
}
