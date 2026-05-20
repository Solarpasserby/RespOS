use riscv::register::sstatus;

/// 本 CPU 中断状态守卫。
///
/// 创建时保存当前 `sstatus.SIE`，随后关闭 supervisor interrupt enable；
/// 析构时只在原本开启中断的情况下重新开启它。这样可以支持嵌套的关中断临界区：
/// 内层 guard 释放时不会错误打开外层已经关闭的中断。
pub struct InterruptGuard {
    sie_before_lock: bool,
}

impl InterruptGuard {
    #[inline(always)]
    pub fn new() -> Self {
        let sie_before_lock = sstatus::read().sie();
        unsafe {
            sstatus::clear_sie();
        }
        Self { sie_before_lock }
    }
}

impl Drop for InterruptGuard {
    #[inline(always)]
    fn drop(&mut self) {
        if self.sie_before_lock {
            unsafe {
                sstatus::set_sie();
            }
        }
    }
}
