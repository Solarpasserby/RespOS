//! JH7110 dw_mmc SD 卡驱动（Stage 5 最小轮询版 v1）。
//!
//! 目标控制器：sdio1 @ `0x16020000`（本板 microSD 枚举为 `mmc 1`）。
//! 寄存器表与初始化序列参考 U-Boot `drivers/mmc/dw_mmc.c` + `include/dwmmc.h`
//! （该驱动已在本板 `fatload mmc 1:3` 实测可用），以及 Linux `dw_mmc-starfive.c`。
//!
//! v1 范围：1-bit、默认速度、FIFO 轮询、单块读写。时钟/复位(syscrg)与电压
//! (sys_syscon) 复用 U-Boot 已配置状态，后续再自建。

#![allow(dead_code)] // 驱动保留完整寄存器/命令表供后续 stage 使用

use crate::arch::timer::get_time_ms;
use crate::config::{BLOCK_SIZE, KERNEL_BASE};
use crate::drivers::{BlockDevice, DevError, DevResult, Device, DeviceType};
use core::ptr::{read_volatile, write_volatile};

const SDMMC_BASE: usize = 0x1602_0000;

// 寄存器偏移（U-Boot include/dwmmc.h）
const CTRL: usize = 0x000;
const PWREN: usize = 0x004;
const CLKDIV: usize = 0x008;
const CLKSRC: usize = 0x00c;
const CLKENA: usize = 0x010;
const TMOUT: usize = 0x014;
const CTYPE: usize = 0x018;
const BLKSIZ: usize = 0x01c;
const BYTCNT: usize = 0x020;
const INTMASK: usize = 0x024;
const CMDARG: usize = 0x028;
const CMD: usize = 0x02c;
const RESP0: usize = 0x030;
const RINTSTS: usize = 0x044;
const STATUS: usize = 0x048;
const FIFOTH: usize = 0x04c;
const VERID: usize = 0x06c;
const BMOD: usize = 0x080;
const DATA: usize = 0x200;

// CTRL 位
const CTRL_RESET: u32 = 1 << 0;
const CTRL_FIFO_RESET: u32 = 1 << 1;
const CTRL_DMA_RESET: u32 = 1 << 2;
const CTRL_INT_ENABLE: u32 = 1 << 4;

// CMD 位
const CMD_RESP_EXP: u32 = 1 << 6;
const CMD_RESP_LONG: u32 = 1 << 7;
const CMD_CHECK_CRC: u32 = 1 << 8;
const CMD_DATA_EXP: u32 = 1 << 9;
const CMD_WRITE: u32 = 1 << 10; // 1=写（host→card），0=读（card→host）
const CMD_PRV_DAT_WAIT: u32 = 1 << 13;
const CMD_UPD_CLK: u32 = 1 << 21;
const CMD_USE_HOLD_REG: u32 = 1 << 29;
const CMD_START: u32 = 1 << 31;

// RINTSTS 位
const RINT_CD: u32 = 1 << 2; // command done
const RINT_DTO: u32 = 1 << 3; // data transfer over
const RINT_RXDR: u32 = 1 << 5; // rx data ready
const RINT_TXDR: u32 = 1 << 4; // tx data ready
const RINT_ERR: u32 = (1 << 6) | (1 << 7) | (1 << 8) | (1 << 9) | (1 << 10);

const CTYPE_1BIT: u32 = 0;
const CTYPE_4BIT: u32 = 1;
// FIFOTH 布局（JH7110）：MSIZE bits[30:28]、RX_WMark bits[27:16]、TX_WMark bits[11:0]
const FIFOTH_MSIZE_SHIFT: u32 = 28;
const FIFOTH_RX_WM_SHIFT: u32 = 16;
const FIFOTH_RX_WM_MASK: u32 = 0xfff << 16;
// STATUS 寄存器（0x48）：FIFO 字数在 bits[29:17]
const STATUS_FIFO_COUNT_SHIFT: u32 = 17;
const STATUS_FIFO_COUNT_MASK: u32 = 0x1fff;
const STATUS_BUSY: u32 = 1 << 9;

// MMC 命令号
const CMD_GO_IDLE_STATE: u32 = 0;
const CMD_SEND_IF_COND: u32 = 8;
const CMD_APP_CMD: u32 = 55;
const ACMD_SEND_OP_COND: u32 = 41;
const CMD_ALL_SEND_CID: u32 = 2;
const CMD_SEND_RELATIVE_ADDR: u32 = 3;
const CMD_SEND_CSD: u32 = 9;
const CMD_SELECT_CARD: u32 = 7;
const ACMD_SET_BUS_WIDTH: u32 = 6;
const CMD_SET_BLOCKLEN: u32 = 16;
const CMD_READ_SINGLE_BLOCK: u32 = 17;
const CMD_WRITE_BLOCK: u32 = 24;

#[inline]
fn rd(off: usize) -> u32 {
    unsafe { read_volatile((KERNEL_BASE + SDMMC_BASE + off) as *const u32) }
}
#[inline]
fn wr(off: usize, val: u32) {
    unsafe { write_volatile((KERNEL_BASE + SDMMC_BASE + off) as *mut u32, val) }
}

fn delay_ms(ms: usize) {
    let start = get_time_ms();
    while get_time_ms().saturating_sub(start) < ms {
        core::hint::spin_loop();
    }
}

/// 等待命令完成（轮询 RINTSTS.CD 或错误位），超时返回 Err。
fn wait_cmd_done(timeout_ms: usize) -> DevResult {
    let start = get_time_ms();
    loop {
        let st = rd(RINTSTS);
        if st & RINT_ERR != 0 {
            wr(RINTSTS, RINT_ERR); // 清错误位
            return Err(DevError::Io);
        }
        if st & RINT_CD != 0 {
            wr(RINTSTS, RINT_CD);
            return Ok(());
        }
        if get_time_ms().saturating_sub(start) > timeout_ms {
            return Err(DevError::Again);
        }
    }
}

/// 发送一条命令。`resp_type` 决定 CMD 寄存器的响应位。
fn send_cmd(cmd_index: u32, arg: u32, resp_type: RespType, timeout_ms: usize) -> DevResult<u32> {
    wr(CMDARG, arg);
    let mut cmd = cmd_index & 0x3f;
    match resp_type {
        RespType::None => {}
        RespType::R1 | RespType::R3 | RespType::R6 | RespType::R7 => {
            cmd |= CMD_RESP_EXP;
            if !matches!(resp_type, RespType::R3) {
                cmd |= CMD_CHECK_CRC;
            }
        }
        RespType::R2 => {
            cmd |= CMD_RESP_EXP | CMD_RESP_LONG | CMD_CHECK_CRC;
        }
    }
    wr(CMD, cmd | CMD_USE_HOLD_REG | CMD_PRV_DAT_WAIT | CMD_START);
    wait_cmd_done(timeout_ms)?;
    Ok(rd(RESP0))
}

#[derive(Clone, Copy)]
enum RespType {
    None,
    R1,
    R3,
    R6,
    R7,
    R2,
}

/// 控制器软复位 + 初始化 FIFO 阈值（时钟/电压沿用 U-Boot 配置）。
fn controller_init() -> DevResult {
    // 软复位：控制器 + FIFO + DMA
    wr(CTRL, CTRL_RESET | CTRL_FIFO_RESET | CTRL_DMA_RESET | CTRL_INT_ENABLE);
    let start = get_time_ms();
    while rd(CTRL) & (CTRL_RESET | CTRL_FIFO_RESET | CTRL_DMA_RESET) != 0 {
        if get_time_ms().saturating_sub(start) > 100 {
            return Err(DevError::BadState);
        }
    }
    wr(PWREN, 1);
    wr(CTYPE, CTYPE_1BIT);
    wr(TMOUT, 0xffff_ffff);
    // FIFOTH：读默认 RX_WMark 得 FIFO 深度，再设 RX_WMark=深度/2-1、TX_WMark=深度/2、
    // MSIZE=2（与 U-Boot dwmci_init 一致）。
    let default_fifoth = rd(FIFOTH);
    let fifo_depth = ((default_fifoth & FIFOTH_RX_WM_MASK) >> FIFOTH_RX_WM_SHIFT) + 1;
    wr(
        FIFOTH,
        (2 << FIFOTH_MSIZE_SHIFT)
            | ((fifo_depth / 2 - 1) << FIFOTH_RX_WM_SHIFT)
            | (fifo_depth / 2),
    );
    wr(INTMASK, 0); // 纯轮询，不用中断
    wr(BMOD, 0); // 不用 IDMAC，走 FIFO
    // 清所有中断状态
    wr(RINTSTS, 0xffff_ffff);
    Ok(())
}

/// 使能卡时钟（CLKDIV 分频 + UPD_CLK 序列）。
/// UPD_CLK 是特殊时钟更新命令，完成标志是 CMD.START 清零，不会置 RINTSTS.CD。
fn enable_clock(div: u32) -> DevResult {
    fn wait_start_clear() -> DevResult {
        for _ in 0..200_000 {
            if rd(CMD) & CMD_START == 0 {
                return Ok(());
            }
        }
        Err(DevError::Again)
    }
    wr(CLKENA, 0);
    wr(CLKSRC, 0);
    wr(CLKDIV, div);
    wr(CMD, CMD_PRV_DAT_WAIT | CMD_UPD_CLK | CMD_START);
    wait_start_clear()?;
    wr(CLKENA, 1);
    wr(CMD, CMD_PRV_DAT_WAIT | CMD_UPD_CLK | CMD_START);
    wait_start_clear()?;
    Ok(())
}

fn card_init() -> DevResult<(u32, usize)> {
    // CMD0: GO_IDLE_STATE
    send_cmd(CMD_GO_IDLE_STATE, 0, RespType::None, 100)?;
    delay_ms(2);

    // CMD8: SEND_IF_COND（arg = 0x1AA，支持 SDHC/SDXC 2.0+）
    let if_cond = send_cmd(CMD_SEND_IF_COND, 0x1aa, RespType::R7, 100);
    let is_sdhc = if_cond.is_ok();

    // ACMD41: SD_SEND_OP_COND，循环直到卡就绪或超时
    let start = get_time_ms();
    loop {
        send_cmd(CMD_APP_CMD, 0, RespType::R1, 100)?;
        let arg = 0x40ff_8000 | if is_sdhc { 1 << 30 } else { 0 };
        let resp = send_cmd(ACMD_SEND_OP_COND, arg, RespType::R3, 100)?;
        if resp & (1 << 31) != 0 {
            break; // 卡就绪
        }
        if get_time_ms().saturating_sub(start) > 1000 {
            return Err(DevError::Again);
        }
        delay_ms(1);
    }

    // CMD2: ALL_SEND_CID（R2 长响应，忽略内容）
    send_cmd(CMD_ALL_SEND_CID, 0, RespType::R2, 100)?;

    // CMD3: SEND_RELATIVE_ADDR（R6，响应高 16 位是 RCA）
    let r6 = send_cmd(CMD_SEND_RELATIVE_ADDR, 0, RespType::R6, 100)?;
    let rca = (r6 >> 16) & 0xffff;

    // CMD7: SELECT_CARD（R1）
    send_cmd(CMD_SELECT_CARD, rca << 16, RespType::R1, 100)?;

    // CMD9: SEND_CSD（R2 长响应），解析 C_SIZE 得块数（CSD v2.0）
    send_cmd(CMD_SEND_CSD, rca << 16, RespType::R2, 100)?;
    // R2 128 位：CSD[127:96]=RESP3, [95:64]=RESP2, [63:32]=RESP1, [31:0]=RESP0。
    // C_SIZE = CSD[69:48] = ((RESP2 & 0x3f) << 16) | (RESP1 >> 16)。
    let resp1 = rd(0x034);
    let resp2 = rd(0x038);
    let c_size = ((resp2 & 0x3f) << 16) | (resp1 >> 16);
    let num_blocks = (c_size as usize + 1) * 1024; // 每块 512 字节

    // ACMD6: SET_BUS_WIDTH（保持 1-bit，先不切 4-bit 求稳）
    send_cmd(CMD_APP_CMD, rca << 16, RespType::R1, 100)?;
    send_cmd(ACMD_SET_BUS_WIDTH, 0, RespType::R1, 100)?;

    // CMD16: SET_BLOCKLEN = 512
    send_cmd(CMD_SET_BLOCKLEN, BLOCK_SIZE as u32, RespType::R1, 100)?;

    Ok((rca, num_blocks))
}

pub struct SdCard {
    rca: u32,
    num_blocks: usize,
    block_size: usize,
}

impl SdCard {
    pub fn new() -> DevResult<Self> {
        controller_init()?;
        crate::println!("[vf2] SD: controller reset ok (verid=0x{:08x})", rd(VERID));
        enable_clock(63)?; // 初始化阶段用最慢分频（~400kHz，SD 规范要求）
        crate::println!("[vf2] SD: clock enabled");
        let (rca, num_blocks) = card_init()?;
        let card = Self { rca, num_blocks, block_size: BLOCK_SIZE };
        // 从快往慢逐档试，读根分区超级块（ROOT_DISK_BASE_BLOCK + 2）并校验 magic 0xEF53，
        // 与挂载真实读取路径一致。ciu 具体值未精确定，div=0/1/2 会超 25MHz 导致读错。
        // lwext4 风格 CRC32c（init 0xffff_ffff，无 final XOR），与 ext4 superblock checksum 算法一致。
        fn crc32c(data: &[u8]) -> u32 {
            let mut crc: u32 = 0xffff_ffff;
            for &b in data {
                crc ^= b as u32;
                for _ in 0..8 {
                    crc = (crc >> 1) ^ (0x82f6_3b78 & (0u32.wrapping_sub(crc & 1)));
                }
            }
            crc
        }
        let super_block = (crate::platform::ROOT_DISK_BASE_BLOCK + 2) as usize;
        let mut clock_div = 63u32;
        for div in [4u32, 8, 16, 32, 63] {
            enable_clock(div)?;
            let mut probe = [0u8; 1024];
            let read_ok = card.read_block(super_block, &mut probe).is_ok();
            let stored = u32::from_le_bytes([probe[1020], probe[1021], probe[1022], probe[1023]]);
            let ok = read_ok
                && probe[56] == 0x53
                && probe[57] == 0xef
                && crc32c(&probe[..1020]) == stored;
            if ok {
                clock_div = div;
                break;
            }
            crate::println!("[vf2] SD: div={} superblock checksum mismatch, lowering clock", div);
        }
        crate::println!(
            "[vf2] SD: card identified, rca=0x{:x}, num_blocks={} ({} MiB), clock_div={}",
            rca,
            num_blocks,
            num_blocks * 512 / 1024 / 1024,
            clock_div
        );
        Ok(card)
    }

    /// 单块读。block 为 512 字节块号。
    fn read_single(&self, block: u32, buf: &mut [u8; 512]) -> DevResult {
        // 等控制器空闲（STATUS.BUSY 清零），再清中断（同 U-Boot dwmci_send_cmd）
        let busy_start = get_time_ms();
        while rd(STATUS) & STATUS_BUSY != 0 {
            if get_time_ms().saturating_sub(busy_start) > 100 {
                return Err(DevError::Again);
            }
        }
        wr(RINTSTS, 0xffff_ffff);

        wr(BLKSIZ, 512);
        wr(BYTCNT, 512);
        // 数据搬移前复位 FIFO 并等复位完成
        wr(CTRL, rd(CTRL) | CTRL_FIFO_RESET);
        let rst_start = get_time_ms();
        while rd(CTRL) & CTRL_FIFO_RESET != 0 {
            if get_time_ms().saturating_sub(rst_start) > 100 {
                return Err(DevError::Again);
            }
        }

        let arg = block * (BLOCK_SIZE as u32 / 512);
        // CMD17: READ_SINGLE_BLOCK（R1 + data），读方向 bit10(RW)=0
        wr(CMDARG, arg);
        wr(
            CMD,
            CMD_READ_SINGLE_BLOCK
                | CMD_RESP_EXP
                | CMD_CHECK_CRC
                | CMD_DATA_EXP
                | CMD_USE_HOLD_REG
                | CMD_PRV_DAT_WAIT
                | CMD_START,
        );

        // 轮询数据：读 STATUS 里的 FIFO 字数，一次读走所有可用字（同 U-Boot）
        let mut off = 0usize;
        let start = get_time_ms();
        while off < 512 {
            let st = rd(RINTSTS);
            if st & RINT_ERR != 0 {
                wr(RINTSTS, RINT_ERR);
                return Err(DevError::Io);
            }
            if st & (RINT_RXDR | RINT_DTO) != 0 {
                wr(RINTSTS, st & (RINT_RXDR | RINT_DTO));
                let status = rd(STATUS);
                let fifo_words = (status >> STATUS_FIFO_COUNT_SHIFT) & STATUS_FIFO_COUNT_MASK;
                let remaining = (512 - off) / 4;
                let n = remaining.min(fifo_words as usize);
                for _ in 0..n {
                    let word = rd(DATA);
                    buf[off..off + 4].copy_from_slice(&word.to_ne_bytes());
                    off += 4;
                }
            }
            if get_time_ms().saturating_sub(start) > 2000 {
                crate::println!(
                    "[vf2] SD read timeout, block={}, off={}, rintsts=0x{:08x}",
                    block,
                    off,
                    rd(RINTSTS)
                );
                return Err(DevError::Again);
            }
        }
        // 等数据传输结束（DTO），保证控制器空闲后再返回，避免连续读时抢跑
        let dto_start = get_time_ms();
        loop {
            let st = rd(RINTSTS);
            if st & RINT_ERR != 0 {
                wr(RINTSTS, RINT_ERR);
                return Err(DevError::Io);
            }
            if st & RINT_DTO != 0 {
                wr(RINTSTS, RINT_DTO);
                break;
            }
            if get_time_ms().saturating_sub(dto_start) > 2000 {
                return Err(DevError::Again);
            }
        }
        Ok(())
    }

    /// 单块写。block 为 512 字节块号。
    fn write_single(&self, block: u32, buf: &[u8; 512]) -> DevResult {
        // 等控制器空闲（STATUS.BUSY 清零）再清中断，连续写第二个块时必需
        let busy_start = get_time_ms();
        while rd(STATUS) & STATUS_BUSY != 0 {
            if get_time_ms().saturating_sub(busy_start) > 100 {
                return Err(DevError::Again);
            }
        }
        wr(RINTSTS, 0xffff_ffff);

        wr(BLKSIZ, 512);
        wr(BYTCNT, 512);
        // 数据搬移前复位 FIFO 并等复位完成
        wr(CTRL, rd(CTRL) | CTRL_FIFO_RESET);
        let rst_start = get_time_ms();
        while rd(CTRL) & CTRL_FIFO_RESET != 0 {
            if get_time_ms().saturating_sub(rst_start) > 100 {
                return Err(DevError::Again);
            }
        }

        let arg = block * (BLOCK_SIZE as u32 / 512);
        // CMD24: WRITE_BLOCK，写方向 bit10(RW)=1
        wr(CMDARG, arg);
        wr(
            CMD,
            CMD_WRITE_BLOCK
                | CMD_RESP_EXP
                | CMD_CHECK_CRC
                | CMD_DATA_EXP
                | CMD_WRITE
                | CMD_USE_HOLD_REG
                | CMD_PRV_DAT_WAIT
                | CMD_START,
        );

        // 轮询写：TXDR 触发时写 4 字节进 FIFO
        let mut off = 0usize;
        let start = get_time_ms();
        while off < 512 {
            let st = rd(RINTSTS);
            if st & RINT_ERR != 0 {
                wr(RINTSTS, RINT_ERR);
                return Err(DevError::Io);
            }
            if st & RINT_TXDR != 0 {
                wr(RINTSTS, RINT_TXDR);
                let mut word = [0u8; 4];
                let n = (512 - off).min(4);
                word[..n].copy_from_slice(&buf[off..off + n]);
                wr(DATA, u32::from_ne_bytes(word));
                off += n;
            }
            if get_time_ms().saturating_sub(start) > 2000 {
                return Err(DevError::Again);
            }
        }
        // 等 data transfer over（DTO）
        let start = get_time_ms();
        loop {
            let st = rd(RINTSTS);
            if st & RINT_ERR != 0 {
                wr(RINTSTS, RINT_ERR);
                return Err(DevError::Io);
            }
            if st & RINT_DTO != 0 {
                wr(RINTSTS, RINT_DTO);
                break;
            }
            if get_time_ms().saturating_sub(start) > 2000 {
                return Err(DevError::Again);
            }
        }
        Ok(())
    }
}

impl Device for SdCard {
    fn device_name(&self) -> &str {
        "jh7110-sd"
    }
    fn device_type(&self) -> DeviceType {
        DeviceType::Block
    }
}

impl BlockDevice for SdCard {
    fn num_blocks(&self) -> usize {
        self.num_blocks
    }
    fn block_size(&self) -> usize {
        self.block_size
    }
    fn read_block(&self, block_id: usize, buf: &mut [u8]) -> DevResult {
        for (i, chunk) in buf.chunks_mut(512).enumerate() {
            let mut tmp = [0u8; 512];
            self.read_single((block_id + i) as u32, &mut tmp)?;
            let n = chunk.len().min(512);
            chunk.copy_from_slice(&tmp[..n]);
        }
        Ok(())
    }
    fn write_block(&self, block_id: usize, buf: &[u8]) -> DevResult {
        for (i, chunk) in buf.chunks(512).enumerate() {
            let mut tmp = [0u8; 512];
            let n = chunk.len().min(512);
            tmp[..n].copy_from_slice(&chunk[..n]);
            self.write_single((block_id + i) as u32, &tmp)?;
        }
        Ok(())
    }
    fn flush(&self) -> DevResult {
        Ok(())
    }
}
