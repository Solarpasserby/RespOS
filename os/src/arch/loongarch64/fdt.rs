//! LoongArch 2K1000LA 扁平设备树（FDT）解析。
//!
//! U-Boot 通过 `go ${entry} ${fdt_addr}` 启动内核时，按 C 调用约定传入
//! `a0 = argc`、`a1 = argv`（每个参数都是十六进制地址字符串）。这里从 argv 提取
//! DTB 物理地址，并解析 `/memory` reg 得到实际 DDR 末址。
//!
//! 参考：
//! - StarryOS `platforms/someboot/src/arch/loongarch64/entry.rs` 的 `uboot_go_fdt_arg`
//! - RespOS `os/src/arch/rv64/config/board.rs` 的 `fdt_memory_end`

// FDT 结构常量（小端，与 RV64 解析一致）
const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 1;
const FDT_END_NODE: u32 = 2;
const FDT_PROP: u32 = 3;
const FDT_NOP: u32 = 4;
const FDT_END: u32 = 9;
const FDT_MAX_SIZE: usize = 2 * 1024 * 1024;

// U-Boot `go` 参数上限（与 StarryOS 一致）
const MAX_UBOOT_GO_ARGS: usize = 16;
const MAX_UBOOT_GO_ARG_LEN: usize = 32;
// UHI boot 协议：a0 == usize::MAX-1 时 a1 直接是 DTB 物理地址
const UHI_FDT_ARG0: usize = usize::MAX - 1;

/// 从启动参数（a0=argc, a1=argv）提取 DTB 物理地址。
///
/// 支持两种 U-Boot 启动协议：
/// - UHI：a0 == `UHI_FDT_ARG0` 时，a1 即 DTB 物理地址。
/// - `go ${entry} ${fdt_addr}`：a0 = argc（1..=16），a1 = argv（`char*` 数组）；
///   DTB 地址是 argv[1..] 里的十六进制字符串。
pub fn fdt_addr_from_boot_args(argc: usize, argv: usize) -> Option<usize> {
    if argc == UHI_FDT_ARG0 {
        return Some(argv);
    }
    if !(1..=MAX_UBOOT_GO_ARGS).contains(&argc) || argv == 0 {
        return None;
    }
    let argv = argv as *const usize;
    for idx in 1..argc {
        let arg = unsafe { *argv.add(idx) };
        if let Some(addr) = parse_hex_addr(arg) {
            return Some(addr);
        }
    }
    None
}

/// 解析十六进制地址字符串（如 "0x8f000000" 或 "8f000000"）。
fn parse_hex_addr(arg: usize) -> Option<usize> {
    if arg == 0 {
        return None;
    }
    let ptr = arg as *const u8;
    let mut idx = 0;
    if unsafe { *ptr } == b'0' && matches!(unsafe { *ptr.add(1) }, b'x' | b'X') {
        idx = 2;
    }
    let mut value = 0usize;
    let mut has_digit = false;
    while idx < MAX_UBOOT_GO_ARG_LEN {
        let byte = unsafe { *ptr.add(idx) };
        if byte == 0 {
            return has_digit.then_some(value);
        }
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        } as usize;
        value = value.checked_mul(16)?.checked_add(digit)?;
        has_digit = true;
        idx += 1;
    }
    None
}

/// 解析 FDT `/memory` 节点的 `reg`，返回所有内存区间（可能有多段）的最大末址。
///
/// 2K1000LA 的 DDR 是否分段（低段 + 高窗口）尚未确认；本函数取最大末址，使
/// `physical_memory_end()` 至少覆盖全部 DDR。多段不连续区间对 frame allocator /
/// direct map 的影响在 Stage 2 后续提交处理。
///
/// # Safety
/// `fdt_addr` 必须是有效的物理地址，且其指向的内存可读（本函数在早期 DMW0
/// identity 直映期间调用，物理地址可直接访问）。
pub unsafe fn memory_end_from_fdt(fdt_addr: usize) -> Option<usize> {
    // U-Boot 可能给带高位段标记的地址，掩到 48-bit 物理地址。
    let fdt_addr = fdt_addr & ((1usize << 48) - 1);
    let base = fdt_addr as *const u8;

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

    let align4 = |v: usize| v.checked_add(3).map(|v| v & !3);
    let mut cursor = struct_start;
    let mut depth = 0usize;
    let mut memory_depth = None;
    let mut address_cells = 2usize;
    let mut size_cells = 1usize;
    let mut max_end = 0usize;

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
                    // reg 可含多个 (addr, size) 对；遍历所有，取最大 end。
                    let mut off = 0usize;
                    while off.checked_add(cells * 4)? <= len {
                        let mut address = 0u64;
                        for cell in 0..address_cells {
                            address = (address << 32) | read_be32(cursor + off + cell * 4)? as u64;
                        }
                        // 2K1000LA 的 FDT /memory 用 0x9000 窗口 VA（0x90000000_...），
                        // 掩到 48-bit 得到物理地址（对齐 StarryOS 的 to_phys）。
                        address &= (1u64 << 48) - 1;
                        let mut size = 0u64;
                        for cell in 0..size_cells {
                            size = (size << 32)
                                | read_be32(cursor + off + (address_cells + cell) * 4)? as u64;
                        }
                        if let Some(end) = address.checked_add(size) {
                            max_end = max_end.max(usize::try_from(end).ok()?);
                        }
                        off += cells * 4;
                    }
                }
                cursor = align4(data_end)?;
            }
            FDT_NOP => {}
            FDT_END => break,
            _ => return None,
        }
    }
    (max_end != 0).then_some(max_end)
}
