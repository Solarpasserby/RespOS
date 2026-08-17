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
        // LoongArch `cacop`：rj=$zero 时对整个缓存级别做索引操作（broadcast）。
        // 0x9 = Index Writeback-Invalidate L1D（LEAF1）、0xa = L2（LEAF2）。
        // 2K1000LA 的 SATA DMA 与 CPU 缓存不是一致的，必须在 DMA 前后刷/失效缓存，
        // 否则控制器读不到 CPU 刚写的命令/数据，或 CPU 读到 DMA 前的旧数据。
        unsafe {
            core::arch::asm!(
                "cacop 0x9, $zero, 0",
                "cacop 0xa, $zero, 0",
                "dbar 0",
                options(nostack)
            );
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
        // 诊断用 early_print（无锁、uncached、Stage 1 已验证），避免 println! 的
        // console 自旋锁/格式化在 AHCI 初始化早期引入额外变量。
        #[cfg(feature = "board_ls2k1000")]
        crate::arch::sbi::early_print("[kernel] AHCI: probing...\n");

        let mut driver = unsafe { AhciDriver::try_new(base) }.ok_or(DevError::BadState)?;

        #[cfg(feature = "board_ls2k1000")]
        {
            crate::arch::sbi::early_print("[kernel] AHCI: try_new returned, capacity=0x");
            crate::arch::sbi::early_print_hex(driver.capacity() as usize);
            crate::arch::sbi::early_print(", block_size=0x");
            crate::arch::sbi::early_print_hex(driver.block_size());
            crate::arch::sbi::early_print("\n");

            // 诊断：读 block2（ext4 superblock @offset1024）验证 DMA 读数据。
            let mut sb = [0u8; 512];
            if driver.read(2, &mut sb) {
                let magic = u16::from_le_bytes([sb[56], sb[57]]);
                crate::arch::sbi::early_print("[kernel] AHCI: block2 magic=0x");
                crate::arch::sbi::early_print_hex(magic as usize);
                crate::arch::sbi::early_print(" (ext4=0xef53)\n");
            } else {
                crate::arch::sbi::early_print("[kernel] AHCI: read block2 failed\n");
            }
        }
        #[cfg(not(feature = "board_ls2k1000"))]
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
