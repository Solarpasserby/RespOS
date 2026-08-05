//! RV64 SMP 的早期启动阶段。
//!
//! 此阶段负责让次 hart 使用独立栈进入共享内核地址空间，并建立本地
//! trap/timer、IPI 与 scheduler 接入。共享地址空间的 active-mask 与
//! request/ack TLB shootdown 尚未完成，因此不能把当前实现视为通用 SMP 完成态。

use crate::{arch, mm};
use core::{
    arch::asm,
    hint::spin_loop,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

pub const MAX_HARTS: usize = 8;
const BOOT_COLD: u8 = 0;
const BOOT_READY: u8 = 1;
const RV64_ENTRY_PHYS: usize = 0x8020_0000;

static BOOT_STATE: AtomicU8 = AtomicU8::new(BOOT_COLD);
static ONLINE_HART_MASK: AtomicUsize = AtomicUsize::new(0);
static IDLE_HART_MASK: AtomicUsize = AtomicUsize::new(0);
static IPI_COUNT: [AtomicUsize; MAX_HARTS] = [const { AtomicUsize::new(0) }; MAX_HARTS];
// 不能放在 BSS：主 hart 需要在清零 BSS 之前认领启动职责。
static BOOT_HART: AtomicUsize = AtomicUsize::new(usize::MAX);

/// user trap 最早期可访问的本 CPU 状态。
///
/// 汇编从 `sscratch` 取得它，并在保存用户寄存器前用 `kernel_sp` 切到当前
/// task 的内核栈。因此首字段的偏移必须保持为 0。
#[repr(C)]
pub struct TrapScratch {
    kernel_sp: AtomicUsize,
    user_sp: AtomicUsize,
}

impl TrapScratch {
    pub const fn new() -> Self {
        Self {
            kernel_sp: AtomicUsize::new(0),
            user_sp: AtomicUsize::new(0),
        }
    }
}

#[repr(C)]
pub struct PerCpu {
    scratch: TrapScratch,
    hart_id: AtomicUsize,
}

impl PerCpu {
    pub const fn new() -> Self {
        Self {
            scratch: TrapScratch::new(),
            hart_id: AtomicUsize::new(usize::MAX),
        }
    }
}

static PER_CPUS: [PerCpu; MAX_HARTS] = [const { PerCpu::new() }; MAX_HARTS];

/// 初始化本 hart 的内核态身份。用户态 tp 仍由 trap context 保存/恢复，
/// 仅内核执行期间将 tp 固定为 `PerCpu` 指针。
pub fn init_current_hart(hart_id: usize) {
    assert!(hart_id < MAX_HARTS);
    let per_cpu = &PER_CPUS[hart_id];
    per_cpu.hart_id.store(hart_id, Ordering::Release);
    let ptr = per_cpu as *const PerCpu as usize;
    unsafe {
        asm!("mv tp, {ptr}", ptr = in(reg) ptr, options(nostack, preserves_flags));
        asm!("csrw sscratch, {ptr}", ptr = in(reg) ptr, options(nostack, preserves_flags));
    }
}

/// 当前 CPU 的 `PerCpu` 指针；只允许由已经建立内核态身份的路径调用。
#[inline]
pub fn current_per_cpu_ptr() -> usize {
    let ptr: usize;
    unsafe {
        asm!("mv {ptr}, tp", ptr = out(reg) ptr, options(nostack, preserves_flags));
    }
    ptr
}

#[inline]
pub fn current_hart_id() -> usize {
    let ptr = current_per_cpu_ptr() as *const PerCpu;
    unsafe { (*ptr).hart_id.load(Ordering::Acquire) }
}

/// 任务切换前发布下一任务的内核栈顶，供下一次从 user trap 进入时使用。
#[inline]
pub fn set_current_kernel_sp(kernel_sp: usize) {
    let ptr = current_per_cpu_ptr() as *const PerCpu;
    unsafe {
        (*ptr).scratch.kernel_sp.store(kernel_sp, Ordering::Release);
    }
}

/// 由最先进入 S-mode 的 hart 认领一次全局初始化职责。
pub fn claim_boot_hart(hart_id: usize) -> bool {
    BOOT_HART
        .compare_exchange(usize::MAX, hart_id, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

pub fn boot_hart() -> usize {
    let hart_id = BOOT_HART.load(Ordering::Acquire);
    assert!(hart_id != usize::MAX, "boot hart was not claimed");
    hart_id
}

/// 全局 timeout/posix timer 队列在早期 SMP 阶段仍由 boot hart 串行服务。
/// 这样 idle secondary 的本地 tick 不会并发取得单核时期的全局 timer 锁。
#[inline]
pub fn is_timer_service_hart() -> bool {
    current_hart_id() == boot_hart()
}

pub fn online_hart_mask() -> usize {
    ONLINE_HART_MASK.load(Ordering::Acquire)
}

/// 发布当前 hart 正在 idle loop 中。调用者随后必须再次检查 ready queue，
/// 以覆盖 enqueue 恰好发生在首次空队列检查与这里之间的窗口。
#[inline]
pub fn enter_idle() {
    let hart = current_hart_id();
    IDLE_HART_MASK.fetch_or(1 << hart, Ordering::Release);
}

#[inline]
pub fn leave_idle() {
    let hart = current_hart_id();
    IDLE_HART_MASK.fetch_and(!(1 << hart), Ordering::Release);
}

/// 在 ready task 已发布后唤醒一个已 online 的 idle hart。
///
/// IPI 是提示而非 task ownership：hart 醒来后仍必须在 scheduler lock 内
/// claim task。SBI 失败只在启动异常时记录，不能使 enqueue 回滚。
pub fn kick_one_idle_hart() {
    let mask = IDLE_HART_MASK.load(Ordering::Acquire) & online_hart_mask();
    if mask == 0 {
        return;
    }
    let hart = mask.trailing_zeros() as usize;
    if let Err(error) = arch::sbi::send_ipi(1 << hart, 0) {
        println!("[smp] IPI kick hart {} failed: {}", hart, error);
    }
}

/// 清除本 hart 的 software interrupt pending 位并记录一次 IPI。
///
/// 该函数只可从 supervisor software interrupt trap 调用；不得获取 scheduler
/// 或驱动锁，以便后续可以安全地把它用作 idle CPU 的唤醒通知。
pub fn acknowledge_ipi() {
    unsafe {
        asm!("csrc sip, {mask}", mask = const 1 << 1, options(nostack, preserves_flags));
    }
    let hart_id = current_hart_id();
    IPI_COUNT[hart_id].fetch_add(1, Ordering::Relaxed);
}

#[inline]
fn ipi_count(hart_id: usize) -> usize {
    IPI_COUNT[hart_id].load(Ordering::Acquire)
}

/// 发布主 hart 已完成的全局初始化；之后的次 hart 只可读取这些状态。
pub fn publish_boot_ready(boot_hart: usize) {
    assert!(boot_hart < MAX_HARTS);
    ONLINE_HART_MASK.store(1 << boot_hart, Ordering::Release);
    BOOT_STATE.store(BOOT_READY, Ordering::Release);
}

/// 请求 OpenSBI 启动配置范围内的次 hart。
///
/// QEMU 不提供一个稳定的 S-mode hart 数查询接口；未配置的 hart 会返回
/// `SBI_ERR_INVALID_PARAM` (-3)，这里静默跳过。BuildStorm RV64 的目标为 8。
pub fn start_secondary_harts(boot_hart: usize) {
    for hart_id in 0..MAX_HARTS {
        if hart_id == boot_hart {
            continue;
        }
        match arch::sbi::hart_start(hart_id, RV64_ENTRY_PHYS, 0) {
            Ok(()) => {}
            Err(-3) => {}
            Err(error) => {
                println!("[smp] hart {} start failed: {}", hart_id, error);
            }
        }
    }
}

/// 次 hart 的入口。
///
/// 它只在 boot-ready 后读取全局状态，建立本核页表/trap/timer 后进入本 CPU
/// idle scheduler。kernel timer handler 会识别无 current task 的 idle CPU。
pub fn secondary_main(hart_id: usize, _opaque: usize) -> ! {
    if hart_id >= MAX_HARTS {
        arch::idle();
    }
    while BOOT_STATE.load(Ordering::Acquire) != BOOT_READY {
        spin_loop();
    }
    init_current_hart(hart_id);
    mm::activate_kernel_space();
    arch::trap::init();
    ONLINE_HART_MASK.fetch_or(1 << hart_id, Ordering::Release);
    arch::trap::enable_timer_interrupt();
    arch::timer::set_next_ti_trigger();
    // 初始 sender 不能假定 HSM start-pending hart 已可接收 IPI。用已在线的
    // 本 hart 进行一次 self-IPI，验证 SBI IPI、SSIP 清除和 trap 返回路径；
    // 后续 scheduler 会只向 ONLINE_HART_MASK 中的目标 hart 发送远端 IPI。
    if let Err(error) = arch::sbi::send_ipi(1 << hart_id, 0) {
        println!("[smp] hart {} self IPI failed: {}", hart_id, error);
    }
    arch::wait_for_interrupt();
    println!(
        "[smp] hart {} online (per-cpu timer idle, ipi={})",
        hart_id,
        ipi_count(hart_id)
    );
    crate::task::run_tasks();
}
