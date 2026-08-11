// RISC-V QEMU virt 机器时钟频率。
//
// 目前三类时间使用同一硬件尺度；保留拆分命名是为了和 LoongArch 的
// bench-facing wall clock / timeout / accounting 设计保持一致。
pub const HARDWARE_CLOCK_FREQ: usize = 10_000_000;
pub const USER_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
pub const ACCOUNTING_CLOCK_FREQ: usize = HARDWARE_CLOCK_FREQ;
pub const MEMORY_START: usize = 0x8020_0000;
pub const MEMORY_END: usize = 0x9000_0000;
/// End of the QEMU virt RAM window for the supported 16 GiB configuration.
pub const MAX_PHYSICAL_MEMORY_END: usize = 0x4_8000_0000;

use core::sync::atomic::{AtomicUsize, Ordering};

static PHYSICAL_MEMORY_END: AtomicUsize = AtomicUsize::new(MEMORY_END);

pub fn physical_memory_end() -> usize {
    PHYSICAL_MEMORY_END.load(Ordering::Acquire)
}

pub fn physical_memory_size() -> usize {
    physical_memory_end().saturating_sub(MEMORY_START)
}

/// Read the RAM extent from the flattened device tree supplied by OpenSBI.
/// The early page table maps the supported RAM window before this runs.
pub fn init_physical_memory_end(fdt_pa: usize) {
    if let Some(end) = unsafe { fdt_memory_end(fdt_pa) } {
        PHYSICAL_MEMORY_END.store(
            end.clamp(MEMORY_END, MAX_PHYSICAL_MEMORY_END),
            Ordering::Release,
        );
    }
}

unsafe fn fdt_memory_end(fdt_pa: usize) -> Option<usize> {
    const FDT_MAGIC: u32 = 0xd00d_feed;
    const FDT_BEGIN_NODE: u32 = 1;
    const FDT_END_NODE: u32 = 2;
    const FDT_PROP: u32 = 3;
    const FDT_NOP: u32 = 4;
    const FDT_END: u32 = 9;
    const FDT_MAX_SIZE: usize = 2 * 1024 * 1024;

    let base = (crate::config::KERNEL_BASE + fdt_pa) as *const u8;
    let read_be32 = |offset: usize| -> Option<u32> {
        let bytes = unsafe { core::slice::from_raw_parts(base.add(offset), 4) };
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    };
    if read_be32(0)? != FDT_MAGIC {
        return None;
    }
    let total = read_be32(4)? as usize;
    if !(40..=FDT_MAX_SIZE).contains(&total) {
        return None;
    }
    let struct_start = read_be32(8)? as usize;
    let strings_start = read_be32(12)? as usize;
    let struct_size = read_be32(36)? as usize;
    let struct_end = struct_start.checked_add(struct_size)?.min(total);
    if struct_start >= struct_end || strings_start >= total {
        return None;
    }

    let align4 = |value: usize| value.checked_add(3).map(|value| value & !3);
    let mut cursor = struct_start;
    let mut depth = 0usize;
    let mut memory_depth = None;
    let mut address_cells = 2usize;
    let mut size_cells = 1usize;
    while cursor.checked_add(4)? <= struct_end {
        let token = read_be32(cursor)?;
        cursor += 4;
        match token {
            FDT_BEGIN_NODE => {
                let name_start = cursor;
                while cursor < struct_end && unsafe { *base.add(cursor) } != 0 {
                    cursor += 1;
                }
                if cursor >= struct_end {
                    return None;
                }
                let name = unsafe {
                    core::slice::from_raw_parts(base.add(name_start), cursor - name_start)
                };
                depth += 1;
                if depth == 2 && (name == b"memory" || name.starts_with(b"memory@")) {
                    memory_depth = Some(depth);
                }
                cursor = align4(cursor + 1)?;
            }
            FDT_END_NODE => {
                if memory_depth == Some(depth) {
                    memory_depth = None;
                }
                depth = depth.checked_sub(1)?;
            }
            FDT_PROP => {
                if cursor.checked_add(8)? > struct_end {
                    return None;
                }
                let len = read_be32(cursor)? as usize;
                let name_offset = read_be32(cursor + 4)? as usize;
                cursor += 8;
                let data_end = cursor.checked_add(len)?;
                if data_end > struct_end || strings_start.checked_add(name_offset)? >= total {
                    return None;
                }
                let prop_name_start = strings_start + name_offset;
                let mut prop_name_end = prop_name_start;
                while prop_name_end < total && unsafe { *base.add(prop_name_end) } != 0 {
                    prop_name_end += 1;
                }
                let prop_name = unsafe {
                    core::slice::from_raw_parts(
                        base.add(prop_name_start),
                        prop_name_end - prop_name_start,
                    )
                };
                if depth == 1 && len >= 4 {
                    if prop_name == b"#address-cells" {
                        address_cells = read_be32(cursor)? as usize;
                    } else if prop_name == b"#size-cells" {
                        size_cells = read_be32(cursor)? as usize;
                    }
                } else if memory_depth == Some(depth) && prop_name == b"reg" {
                    if !(1..=2).contains(&address_cells) || !(1..=2).contains(&size_cells) {
                        return None;
                    }
                    let cells = address_cells.checked_add(size_cells)?;
                    if len < cells * 4 {
                        return None;
                    }
                    let mut address = 0u64;
                    for cell in 0..address_cells {
                        address = (address << 32) | read_be32(cursor + cell * 4)? as u64;
                    }
                    let mut size = 0u64;
                    for cell in 0..size_cells {
                        size =
                            (size << 32) | read_be32(cursor + (address_cells + cell) * 4)? as u64;
                    }
                    let end = address.checked_add(size)?;
                    return usize::try_from(end).ok();
                }
                cursor = align4(data_end)?;
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => return None,
        }
    }
    None
}

pub const VIRTIO_MMIO: &[(usize, usize)] = &[
    (0x1000_1000, 0x00_1000), // virtio-mmio-bus.0
    (0x1000_2000, 0x00_1000), // virtio-mmio-bus.1
];
