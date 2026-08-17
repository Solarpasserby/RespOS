//! 2K1000LA AHCI（SATA）块设备驱动，封装 StarryOS `simple-ahci` crate。
//!
//! 2K1000LA 的 AHCI 控制器在 `0x400e0000`（设备树 `sata@400e0000`，irq 19）。真机 MMIO
//! 必须经 uncached DMW0 窗口（VSEG=0x8000）访问（见 keypoints 坑 2）；DMA 数据/命令表
//! 由内核堆分配，`virt_to_phys` 把高半区 VA 转成物理地址供控制器使用。

use crate::config::BLOCK_SIZE;
use crate::drivers::{BlockDevice, DevError, DevResult, Device, DeviceType};
use simple_ahci::{AhciDriver, Hal};
use spin::Mutex;

/// 真机 uncached 直映窗口基址。
const UNCACHE_BASE: usize = 0x8000_0000_0000_0000;
/// 2K1000LA AHCI 控制器 MMIO 基址（物理）。
const AHCI_BASE: usize = 0x400e_0000;

struct RespOsAhciHal;

impl Hal for RespOsAhciHal {
    fn virt_to_phys(va: usize) -> usize {
        crate::arch::kernel_virt_to_phys(va as *const ())
    }

    fn current_ms() -> u64 {
        crate::timer::get_time_ms() as u64
    }

    fn flush_dcache() {
        // LoongArch `dbar 0` 数据屏障。这里假设 2K1000LA 的 SATA DMA 与 CPU 缓存
        // 走一致性互连（Loongson SoC 常见）；若出现数据损坏，需改成 cacop 显式
        // 刷/失效缓存（Stage 5 后续验证点）。
        unsafe {
            core::arch::asm!("dbar 0", options(nostack));
        }
    }
}

pub struct AhciBlockDevice {
    inner: Mutex<AhciDriver<RespOsAhciHal>>,
}

unsafe impl Send for AhciBlockDevice {}
unsafe impl Sync for AhciBlockDevice {}

impl AhciBlockDevice {
    /// 从 2K1000LA 的 AHCI 基址初始化（MMIO 经 uncached 窗口访问）。
    pub fn new() -> DevResult<Self> {
        let base = UNCACHE_BASE | AHCI_BASE;
        let driver = unsafe { AhciDriver::try_new(base) }.ok_or(DevError::BadState)?;
        println!(
            "[kernel] AHCI disk: {} blocks x {} bytes",
            driver.capacity(),
            driver.block_size()
        );
        Ok(Self {
            inner: Mutex::new(driver),
        })
    }
}

impl Device for AhciBlockDevice {
    fn device_name(&self) -> &str {
        "ahci"
    }

    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
}

impl BlockDevice for AhciBlockDevice {
    fn num_blocks(&self) -> usize {
        self.inner.lock().capacity() as usize
    }

    fn block_size(&self) -> usize {
        BLOCK_SIZE
    }

    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> DevResult {
        if self.inner.lock().read(block_id as u64, buf) {
            Ok(())
        } else {
            Err(DevError::Io)
        }
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) -> DevResult {
        if self.inner.lock().write(block_id as u64, buf) {
            Ok(())
        } else {
            Err(DevError::Io)
        }
    }

    fn flush(&self) -> DevResult {
        // AHCI 写直接落盘，暂无需额外 flush（可后续加 FUA）。
        Ok(())
    }
}
