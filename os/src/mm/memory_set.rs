// os/src/mm/memory_set.rs

use super::address::{PhysPageNum, StepByOne, VPNRange, VirtAddr, VirtPageNum};
use super::frame_allocator::{FrameTracker, frame_alloc};
use super::{PTEFlags, PageTable, PageTableEntry};
use crate::arch::{sfence, write_mmu_token};
use crate::config::{
    CLK_TCK, DL_INTERP_OFFSET, KERNEL_BASE, KERNEL_STACK_SIZE, MEMORY_END, MMAP_MAX_ADDR,
    MMAP_MIN_ADDR, PAGE_SIZE, PAGE_SIZE_BITS, TRAMPOLINE, TRAMPOLINE_CODE, USER_STACK_SIZE,
    VIRTIO_MMIO,
};
use crate::fs::{AT_FDCWD, File, FileOp, path_open};
use crate::syscall::{Errno, SysResult};
use crate::task::{
    AT_BASE, AT_CLKTCK, AT_EGID, AT_ENTRY, AT_EUID, AT_GID, AT_PAGESZ, AT_PHDR, AT_PHENT, AT_PHNUM,
    AT_UID, AuxHeader,
};
use crate::trap::PageFaultCause;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use bitflags::bitflags;
use lazy_static::lazy_static;
use spin::Mutex;

unsafe extern "C" {
    fn stext();
    fn etext();
    fn srodata();
    fn erodata();
    fn sdata();
    fn edata();
    fn sbss_with_stack();
    fn ebss();
    fn ekernel();
}

lazy_static! {
    /// 内核地址空间，内核地址空间在内核初始化后创建
    ///
    /// 内核采用恒等映射，因而开启虚拟地址后访问内核空间的地址不变
    ///
    /// 由于内核空间被所有用户空间共享，所以使用 `Arc` 来实现共享，使用 `Mutex` 来实现内部可变性
    pub static ref KERNEL_SPACE: Arc<Mutex<MemorySet>> =
        Arc::new(Mutex::new(MemorySet::new_kernel()));
    static ref SHARED_FILE_PAGES: Mutex<BTreeMap<SharedFilePageKey, Weak<FrameTracker>>> =
        Mutex::new(BTreeMap::new());
}

const ET_DYN: u16 = 3;
const PIE_LOAD_OFFSET: usize = 0x40_0000;

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
struct SharedFilePageKey {
    dev: u64,
    ino: u64,
    page_index: usize,
}

const KERNEL_STACK_EAGER_SIZE: usize = KERNEL_STACK_SIZE;

fn read_dynamic_linker(interp: &str) -> Option<Vec<u8>> {
    if let Ok(file) = path_open(AT_FDCWD, interp, 0, 0) {
        if let Ok(data) = file.read_all() {
            info!("[from_elf_data] dynamic linker {} loaded", interp);
            return Some(data);
        }
    }
    None
}

fn read_file_at(file: Arc<dyn FileOp>, offset: usize, buf: &mut [u8]) -> SysResult<usize> {
    if let Some(file) = file.as_any().downcast_ref::<File>() {
        return file.read_at_offset(offset, buf);
    }

    let origin_offset = file.get_offset();
    file.seek(offset as isize)?;
    let mut done = 0usize;
    let read_result = loop {
        if done >= buf.len() {
            break Ok(done);
        }
        match file.read(&mut buf[done..]) {
            Ok(0) => break Ok(done),
            Ok(n) => done += n,
            Err(err) => break Err(err),
        }
    };
    let restore_result = file.seek(origin_offset as isize);
    match read_result {
        Ok(n) => {
            restore_result?;
            Ok(n)
        }
        Err(err) => {
            let _ = restore_result;
            Err(err)
        }
    }
}

fn write_file_at(file: Arc<dyn FileOp>, offset: usize, buf: &[u8]) -> SysResult<usize> {
    if let Some(file) = file.as_any().downcast_ref::<File>() {
        return file.write_at_offset(offset, buf);
    }

    let origin_offset = file.get_offset();
    file.seek(offset as isize)?;
    let mut done = 0usize;
    let write_result = loop {
        if done >= buf.len() {
            break Ok(done);
        }
        match file.write(&buf[done..]) {
            Ok(0) => break Ok(done),
            Ok(n) => done += n,
            Err(err) => break Err(err),
        }
    };
    let restore_result = file.seek(origin_offset as isize);
    match write_result {
        Ok(n) => {
            restore_result?;
            Ok(n)
        }
        Err(err) => {
            let _ = restore_result;
            Err(err)
        }
    }
}

fn shared_file_frame(backing: &FileBacking, page_offset: usize) -> SysResult<Arc<FrameTracker>> {
    let stat = backing.file.get_stat()?;
    let file_offset = backing.offset.checked_add(page_offset).ok_or(Errno::EIO)?;
    let key = SharedFilePageKey {
        dev: stat.dev,
        ino: stat.ino,
        page_index: file_offset / PAGE_SIZE,
    };

    if let Some(frame) = SHARED_FILE_PAGES
        .lock()
        .get(&key)
        .and_then(|weak| weak.upgrade())
    {
        return Ok(frame);
    }

    let frame = Arc::new(frame_alloc().ok_or(Errno::ENOMEM)?);
    if page_offset < backing.len {
        let read_len = (backing.len - page_offset).min(PAGE_SIZE);
        read_file_at(
            backing.file.clone(),
            file_offset,
            &mut frame.ppn().get_bytes_array()[..read_len],
        )?;
    }

    let mut pages = SHARED_FILE_PAGES.lock();
    if let Some(existing) = pages.get(&key).and_then(|weak| weak.upgrade()) {
        return Ok(existing);
    }
    pages.insert(key, Arc::downgrade(&frame));
    Ok(frame)
}

/// 地址空间
///
/// 一系列有关联的逻辑段 [`MapArea`]，地址不一定连续
pub struct MemorySet {
    // 堆分配
    pub brk: usize,
    pub heap_bottom: usize,
    // mmap 起始地址
    pub mmap_start: usize,
    // 页表和各逻辑段
    pub page_table: PageTable,
    areas: Vec<MapArea>,
}

impl Drop for MemorySet {
    fn drop(&mut self) {
        self.recycle_data_pages();
    }
}

struct MmapRequest {
    addr: Option<usize>,
    map_len: usize,
    map_perm: MapPermission,
    replace: bool,
    noreplace: bool,
    locked: bool,
}

struct MmapPlacement {
    start: usize,
    end: usize,
    map_perm: MapPermission,
    locked: bool,
    auto_addr: bool,
}

pub(crate) enum MmapBacking<'a> {
    LazyAnonymous,
    SharedAnonymous,
    SharedFrames {
        attach_id: usize,
        frames: &'a [Arc<FrameTracker>],
    },
    PrivateFile {
        file: Arc<dyn FileOp>,
        offset: usize,
        len: usize,
    },
    SharedFile {
        file: Arc<dyn FileOp>,
        offset: usize,
        len: usize,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct SharedFutexKey {
    pub owner: usize,
    pub page_index: usize,
    pub offset: usize,
}

pub(crate) fn mmap_file_backing(
    file: Arc<dyn FileOp>,
    offset: usize,
    _len: usize,
    map_len: usize,
    shared: bool,
) -> SysResult<MmapBacking<'static>> {
    let file_len = file.get_stat()?.size.saturating_sub(offset).min(map_len);
    if shared {
        Ok(MmapBacking::SharedFile {
            file,
            offset,
            len: file_len,
        })
    } else {
        Ok(MmapBacking::PrivateFile {
            file,
            offset,
            len: file_len,
        })
    }
}

impl MemorySet {
    #[cfg(target_arch = "loongarch64")]
    fn initial_mmap_start() -> usize {
        MMAP_MIN_ADDR
    }

    #[cfg(target_arch = "riscv64")]
    fn initial_mmap_start() -> usize {
        MMAP_MIN_ADDR
    }

    #[cfg(target_arch = "loongarch64")]
    fn record_auto_mmap(&mut self, _start: usize, end: usize, map_perm: MapPermission) {
        let prot_bits = MapPermission::READ | MapPermission::WRITE | MapPermission::EXECUTE;
        self.mmap_start = if map_perm.intersection(prot_bits).is_empty() {
            end.checked_add(PAGE_SIZE)
                .filter(|next| *next <= MMAP_MAX_ADDR)
                .unwrap_or(end)
        } else {
            end
        };
    }

    #[cfg(target_arch = "riscv64")]
    fn record_auto_mmap(&mut self, _start: usize, end: usize, _map_perm: MapPermission) {
        self.mmap_start = end;
    }

    /// 将一段空的逻辑段加入地址空间，在内部完成映射关系的建立
    fn push_empty_map_area(
        &mut self,
        map_area: MapArea,
        data: Option<&[u8]>,
        data_offset: usize,
    ) {
        self.try_push_empty_map_area(map_area, data, data_offset)
            .expect("failed to map area");
    }

    fn try_push_empty_map_area(
        &mut self,
        mut map_area: MapArea,
        data: Option<&[u8]>,
        data_offset: usize,
    ) -> SysResult {
        map_area.map(&mut self.page_table)?;
        if let Some(data) = data {
            map_area.copy_data(&self.page_table, data, data_offset);
        }
        self.areas.push(map_area); // 转移所有权
        Ok(())
    }
    /// 惰性添加逻辑段——只记录虚拟地址范围，不分配物理页不建立映射
    fn push_map_area_lazy(&mut self, map_area: MapArea) {
        self.areas.push(map_area);
    }

    /// 对外暴露的添加内核栈段的接口
    pub fn insert_stack_area(&mut self, stack_top: usize) -> SysResult {
        let stack_bottom_va = VirtAddr::from(stack_top - KERNEL_STACK_EAGER_SIZE);
        let stack_bottom_vpn = VirtPageNum::from(stack_bottom_va);
        if self
            .areas
            .iter()
            .any(|area| area.vpn_range.get_start() == stack_bottom_vpn)
        {
            return Ok(());
        }
        let mut area = MapArea::new(
            (stack_top - KERNEL_STACK_EAGER_SIZE).into(),
            stack_top.into(),
            MapType::Framed,
            MapPermission::READ | MapPermission::WRITE,
        );
        area.map(&mut self.page_table)?;
        self.areas.push(area);
        Ok(())
    }
    /// 对外暴露的删除内核栈段的接口
    pub fn remove_stack_area(&mut self, stack_top: usize) {
        let stack_bottom_va = VirtAddr::from(stack_top - KERNEL_STACK_EAGER_SIZE);
        let stack_bottom_vpn = VirtPageNum::from(stack_bottom_va);
        if let Some((idx, area)) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| area.vpn_range.get_start() == stack_bottom_vpn)
        {
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
        }
    }
    /// 插入已有逻辑段，由调用者保证插入逻辑段没有冲突
    pub fn insert_framed_area_va(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_perm: MapPermission,
    ) {
        self.push_empty_map_area(
            MapArea::new(start_va, end_va, MapType::Framed, map_perm),
            None,
            0,
        );
    }

    /// 将 sigreturn 跳板页映射到用户地址空间（TRAMPOLINE 虚拟地址）。
    ///
    /// 用户态信号处理函数返回后，会跳转到该页执行架构相关的 sigreturn
    /// 系统调用入口代码。每个用户进程都需要此映射。
    pub fn map_trampoline(&mut self) {
        // 在用户空间分配一页并映射
        self.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(TRAMPOLINE),
                VirtAddr::from(TRAMPOLINE + PAGE_SIZE),
                MapType::Framed,
                MapPermission::READ | MapPermission::EXECUTE | MapPermission::USER,
            ),
            Some(TRAMPOLINE_CODE),
            0,
        );
    }

    pub fn try_map_trampoline(&mut self) -> SysResult {
        self.try_push_empty_map_area(
            MapArea::new(
                VirtAddr::from(TRAMPOLINE),
                VirtAddr::from(TRAMPOLINE + PAGE_SIZE),
                MapType::Framed,
                MapPermission::READ | MapPermission::EXECUTE | MapPermission::USER,
            ),
            Some(TRAMPOLINE_CODE),
            0,
        )
    }

    /// 惰性插入逻辑段，只预留虚拟地址空间，不分配物理页（由 page fault handler 按需分配）
    pub fn insert_framed_area_va_lazy(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_perm: MapPermission,
    ) {
        self.push_map_area_lazy(MapArea::new(start_va, end_va, MapType::Framed, map_perm));
    }

    /// 插入匿名共享映射。
    ///
    /// TODO[ABI-COMPAT]: 这里选择立即分配物理页，避免 fork 时父子进程分别
    /// fault 出不同页帧。它还不是完整的 Linux shmem/tmpfs 语义，但能保证
    /// `MAP_SHARED | MAP_ANONYMOUS` 在现有 COW fork 模型下真正共享内存。
    pub fn insert_shared_framed_area_va(
        &mut self,
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_perm: MapPermission,
    ) {
        self.push_empty_map_area(MapArea::new_shared(start_va, end_va, map_perm), None, 0);
    }
    /// 根据首虚拟页号删除对应逻辑段
    pub fn remove_area_with_start_vpn(&mut self, vpn_start: VirtPageNum) -> SysResult {
        let (idx, area) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| area.vpn_range.get_start() == vpn_start)
            .ok_or(Errno::EINVAL)?;

        area.notify_mmap_close();
        area.unmap(&mut self.page_table);
        self.areas.remove(idx);
        Ok(())
    }

    /// 删除与给定虚拟页区间重叠的映射，必要时裁剪或切分原逻辑段
    pub fn remove_area_with_overlap_range(&mut self, vpn_range: VPNRange) -> SysResult {
        let mut idx = 0;
        while idx < self.areas.len() {
            if !self.areas[idx].vpn_range.intersect_with(&vpn_range) {
                idx += 1;
                continue;
            }

            let area = self.areas.remove(idx);
            let remove_start = vpn_range.get_start();
            let remove_end = vpn_range.get_end();
            let overlap_start = area.vpn_range.get_start().max(remove_start);
            let overlap_end = area.vpn_range.get_end().min(remove_end);
            let split = area.split_by_overlap(overlap_start, overlap_end);

            let mut middle = split.middle;
            middle.notify_mmap_close();
            middle.unmap(&mut self.page_table);

            if let Some(left) = split.left {
                self.areas.insert(idx, left);
                idx += 1;
            }
            if let Some(right) = split.right {
                self.areas.insert(idx, right);
                idx += 1;
            }
        }
        Ok(())
    }

    /// 判断给定虚拟页区间是否与当前任一逻辑段重叠。
    pub fn area_intersects(&self, vpn_range: &VPNRange) -> bool {
        self.areas
            .iter()
            .any(|area| area.vpn_range.intersect_with(vpn_range))
    }

    pub fn shm_attach_ids_for_frames(&self, frames: &[Arc<FrameTracker>]) -> Vec<usize> {
        let mut ids = Vec::new();
        if frames.is_empty() {
            return ids;
        }
        for area in self.areas.iter() {
            let Some(attach_id) = area.shm_attach_id else {
                continue;
            };
            if ids.contains(&attach_id) {
                continue;
            }
            if area
                .data_frames
                .values()
                .any(|frame| frames.iter().any(|target| Arc::ptr_eq(frame, target)))
            {
                ids.push(attach_id);
            }
        }
        ids
    }

    pub fn remove_shm_attachment(&mut self, vpn_start: VirtPageNum) -> SysResult<usize> {
        let attach_id = self
            .areas
            .iter()
            .rev()
            .find(|area| area.vpn_range.contain(&vpn_start))
            .and_then(|area| area.shm_attach_id)
            .ok_or(Errno::EINVAL)?;

        let mut idx = 0;
        let mut removed = false;
        while idx < self.areas.len() {
            if self.areas[idx].shm_attach_id == Some(attach_id) {
                let mut area = self.areas.remove(idx);
                area.unmap(&mut self.page_table);
                removed = true;
            } else {
                idx += 1;
            }
        }
        if removed {
            Ok(attach_id)
        } else {
            Err(Errno::EINVAL)
        }
    }

    fn is_private_writable_anonymous(area: &MapArea) -> bool {
        area.map_type == MapType::Framed
            && !area.shared
            && area.file_backing.is_none()
            && area
                .map_perm
                .contains(MapPermission::WRITE | MapPermission::USER)
    }

    /// 确保一段 brk 增长区间可作为私有匿名堆使用。
    ///
    /// 已存在的私有匿名可写 VMA 直接复用，空洞补成 lazy 匿名 VMA；
    /// 一旦遇到文件映射、共享映射或不可写 VMA，就按 Linux brk 失败语义拒绝。
    pub fn ensure_private_writable_anonymous_range(&mut self, vpn_range: VPNRange) -> SysResult {
        for area in self
            .areas
            .iter()
            .filter(|area| area.vpn_range.intersect_with(&vpn_range))
        {
            if !Self::is_private_writable_anonymous(area) {
                return Err(Errno::ENOMEM);
            }
        }

        let mut cursor = vpn_range.get_start();
        let end = vpn_range.get_end();
        while cursor < end {
            if let Some(area) = self
                .areas
                .iter()
                .find(|area| area.vpn_range.contain(&cursor))
            {
                cursor = area.vpn_range.get_end().min(end);
                continue;
            }

            let next = self
                .areas
                .iter()
                .filter_map(|area| {
                    let start = area.vpn_range.get_start();
                    (start > cursor && start < end).then_some(start)
                })
                .min()
                .unwrap_or(end);
            self.insert_framed_area_va_lazy(
                VirtAddr::from(cursor),
                VirtAddr::from(next),
                MapPermission::READ | MapPermission::WRITE | MapPermission::USER,
            );
            cursor = next;
        }
        Ok(())
    }

    fn prepare_mmap(&mut self, request: MmapRequest) -> SysResult<MmapPlacement> {
        let mut start = self.choose_mmap_start(request.addr, request.map_len)?;
        if request.addr.is_some()
            && !request.replace
            && !request.noreplace
            && (start < MMAP_MIN_ADDR
                || start
                    .checked_add(request.map_len)
                    .filter(|end| *end <= MMAP_MAX_ADDR)
                    .is_none())
        {
            start = self.choose_mmap_start(None, request.map_len)?;
        }
        if start < MMAP_MIN_ADDR {
            return Err(Errno::EINVAL);
        }
        let mut end = start.checked_add(request.map_len).ok_or(Errno::ENOMEM)?;
        if end > MMAP_MAX_ADDR {
            return Err(Errno::ENOMEM);
        }

        let mut vpn_range =
            VPNRange::new(VirtAddr::from(start).floor(), VirtAddr::from(end).ceil());
        let mut intersects = self.area_intersects(&vpn_range);
        if request.addr.is_some() && !request.replace && !request.noreplace && intersects {
            start = self.choose_mmap_start(None, request.map_len)?;
            end = start.checked_add(request.map_len).ok_or(Errno::ENOMEM)?;
            if end > MMAP_MAX_ADDR {
                return Err(Errno::ENOMEM);
            }
            vpn_range = VPNRange::new(VirtAddr::from(start).floor(), VirtAddr::from(end).ceil());
            intersects = self.area_intersects(&vpn_range);
        }
        if request.noreplace && intersects {
            return Err(Errno::EEXIST);
        } else if request.replace {
            self.remove_area_with_overlap_range(vpn_range.clone())?;
        } else if intersects {
            return Err(Errno::EINVAL);
        }

        Ok(MmapPlacement {
            start,
            end,
            map_perm: request.map_perm,
            locked: request.locked,
            auto_addr: request.addr.is_none(),
        })
    }

    fn finish_mmap(&mut self, placement: &MmapPlacement) {
        if placement.auto_addr {
            self.record_auto_mmap(placement.start, placement.end, placement.map_perm);
        }
    }

    /// 按 mmap 语义选择地址并插入指定 backing 的用户映射。
    pub(crate) fn mmap_area(
        &mut self,
        addr: Option<usize>,
        map_len: usize,
        map_perm: MapPermission,
        replace: bool,
        noreplace: bool,
        locked: bool,
        backing: MmapBacking<'_>,
    ) -> SysResult<usize> {
        if let MmapBacking::SharedFrames { frames, .. } = &backing {
            if frames.len() != map_len / PAGE_SIZE {
                return Err(Errno::EINVAL);
            }
        }

        let placement = self.prepare_mmap(MmapRequest {
            addr,
            map_len,
            map_perm,
            replace,
            noreplace,
            locked,
        })?;

        match backing {
            MmapBacking::LazyAnonymous => {
                self.push_map_area_lazy(MapArea::new_with_flags(
                    VirtAddr::from(placement.start),
                    VirtAddr::from(placement.end),
                    MapType::Framed,
                    placement.map_perm,
                    false,
                    placement.locked,
                ));
            }
            MmapBacking::SharedAnonymous => {
                let mut area = MapArea::new_shared(
                    VirtAddr::from(placement.start),
                    VirtAddr::from(placement.end),
                    placement.map_perm,
                );
                area.locked = placement.locked;
                self.push_empty_map_area(area, None, 0);
            }
            MmapBacking::SharedFrames { attach_id, frames } => {
                let mut area = MapArea::new_shared(
                    VirtAddr::from(placement.start),
                    VirtAddr::from(placement.end),
                    placement.map_perm,
                );
                area.locked = placement.locked;
                area.shm_attach_id = Some(attach_id);
                for (vpn, frame) in area.vpn_range.into_iter().zip(frames.iter()) {
                    self.page_table
                        .map(vpn, frame.ppn(), PTEFlags::from(placement.map_perm))?;
                    area.data_frames.insert(vpn, frame.clone());
                }
                self.areas.push(area);
            }
            MmapBacking::PrivateFile { file, offset, len } => {
                self.push_map_area_lazy(MapArea::new_file_backed(
                    VirtAddr::from(placement.start),
                    VirtAddr::from(placement.end),
                    placement.map_perm,
                    false,
                    placement.locked,
                    file,
                    offset,
                    len,
                ));
            }
            MmapBacking::SharedFile { file, offset, len } => {
                let mut area = MapArea::new_file_backed(
                    VirtAddr::from(placement.start),
                    VirtAddr::from(placement.end),
                    placement.map_perm,
                    true,
                    placement.locked,
                    file,
                    offset,
                    len,
                );
                area.map(&mut self.page_table)?;
                area.notify_mmap_open();
                self.areas.push(area);
            }
        }

        self.finish_mmap(&placement);
        Ok(placement.start)
    }

    /// 将内核缓冲区写入已经映射好的用户虚拟地址范围。
    ///
    /// 该函数通过页表找到物理页后写内核直映地址，不受用户页 PTE 的
    /// 读写权限影响，适合用于 `mmap` 初始化只读文件页。
    /// 将字节数据写入用户地址空间中已映射的虚拟地址范围。
    ///
    /// execve 初始化用户栈（argv/envp/auxv）时使用：新地址空间的用户栈页
    /// 已在 MemorySet 中建立映射，通过页表翻译到物理页直接写入，避免依赖尚未设置的当前页表
    pub fn write_bytes_to_mapped_range(&mut self, start: usize, data: &[u8]) -> SysResult {
        let end = start.checked_add(data.len()).ok_or(Errno::EFAULT)?;
        let mut copied = 0usize;
        let mut cur = start;

        while cur < end {
            let va = VirtAddr::from(cur);
            let vpn = va.floor();
            let page_offset = cur & (PAGE_SIZE - 1);
            let copy_len = (PAGE_SIZE - page_offset).min(end - cur);
            let pte = self.page_table.translate(vpn).ok_or(Errno::EFAULT)?;
            if !pte.is_valid() {
                return Err(Errno::EFAULT);
            }

            let dst = &mut pte.ppn().get_bytes_array()[page_offset..page_offset + copy_len];
            dst.copy_from_slice(&data[copied..copied + copy_len]);
            copied += copy_len;
            cur += copy_len;
        }
        Ok(())
    }

    #[cfg(target_arch = "loongarch64")]
    fn choose_mmap_start(&self, addr: Option<usize>, map_len: usize) -> SysResult<usize> {
        const PMD_SIZE: usize = PAGE_SIZE * 512;

        fn align_for_loongarch(start: usize, map_len: usize) -> SysResult<usize> {
            let end = start.checked_add(map_len).ok_or(Errno::ENOMEM)?;
            if map_len < PMD_SIZE && start / PMD_SIZE != (end - 1) / PMD_SIZE {
                Ok((start + PMD_SIZE - 1) & !(PMD_SIZE - 1))
            } else {
                Ok(start)
            }
        }

        match addr {
            Some(start) => Ok(start),
            None => {
                let mut start = align_for_loongarch(self.mmap_start, map_len)?;
                loop {
                    let end = start.checked_add(map_len).ok_or(Errno::ENOMEM)?;
                    if end > MMAP_MAX_ADDR {
                        return Err(Errno::ENOMEM);
                    }
                    let vpn_range =
                        VPNRange::new(VirtAddr::from(start).floor(), VirtAddr::from(end).ceil());
                    if let Some(area) = self
                        .areas
                        .iter()
                        .find(|area| area.vpn_range.intersect_with(&vpn_range))
                    {
                        start = usize::from(VirtAddr::from(area.vpn_range.get_end()))
                            .checked_add(PAGE_SIZE)
                            .ok_or(Errno::ENOMEM)?;
                        start = align_for_loongarch(start, map_len)?;
                        continue;
                    }
                    return Ok(start);
                }
            }
        }
    }

    #[cfg(target_arch = "riscv64")]
    fn choose_mmap_start(&self, addr: Option<usize>, map_len: usize) -> SysResult<usize> {
        match addr {
            Some(start) => Ok(start),
            None => {
                let mut start = self.mmap_start;
                loop {
                    let end = start.checked_add(map_len).ok_or(Errno::ENOMEM)?;
                    if end > MMAP_MAX_ADDR {
                        return Err(Errno::ENOMEM);
                    }
                    let vpn_range =
                        VPNRange::new(VirtAddr::from(start).floor(), VirtAddr::from(end).ceil());
                    if let Some(area) = self
                        .areas
                        .iter()
                        .find(|area| area.vpn_range.intersect_with(&vpn_range))
                    {
                        start = usize::from(VirtAddr::from(area.vpn_range.get_end()));
                        continue;
                    }
                    return Ok(start);
                }
            }
        }
    }

    /// 删除 mmap 区域，并在释放的是当前 mmap 尾部时回退下一次分配的起点。
    pub fn munmap_range(&mut self, addr: usize, map_len: usize) -> SysResult {
        let end = addr.checked_add(map_len).ok_or(Errno::EINVAL)?;
        if end > MMAP_MAX_ADDR {
            return Err(Errno::ENOMEM);
        }
        let vpn_range = VPNRange::new(VirtAddr::from(addr).floor(), VirtAddr::from(end).ceil());
        self.remove_area_with_overlap_range(vpn_range)?;
        #[cfg(target_arch = "loongarch64")]
        if self.mmap_start == end || self.mmap_start == end.saturating_add(PAGE_SIZE) {
            self.mmap_start = addr;
        }
        #[cfg(target_arch = "riscv64")]
        if self.mmap_start == end {
            self.mmap_start = addr;
        }
        Ok(())
    }

    pub fn mremap_range(
        &mut self,
        old_addr: usize,
        old_len: usize,
        new_len: usize,
        maymove: bool,
        fixed_addr: Option<usize>,
    ) -> SysResult<usize> {
        let old_end = old_addr.checked_add(old_len).ok_or(Errno::ENOMEM)?;
        let old_range = VPNRange::new(
            VirtAddr::from(old_addr).floor(),
            VirtAddr::from(old_end).floor(),
        );
        let new_pages = new_len / PAGE_SIZE;
        let old_pages = old_len / PAGE_SIZE;
        if new_pages == 0 || old_pages == 0 {
            return Err(Errno::EINVAL);
        }

        if let Some(new_addr) = fixed_addr {
            let new_end = new_addr.checked_add(new_len).ok_or(Errno::ENOMEM)?;
            if new_addr < MMAP_MIN_ADDR || new_end > MMAP_MAX_ADDR {
                return Err(Errno::ENOMEM);
            }
            let new_start = VirtAddr::from(new_addr).floor();
            let new_range = VPNRange::new(new_start, VirtAddr::from(new_end).floor());
            if old_range.intersect_with(&new_range) {
                return Err(Errno::EINVAL);
            }
            let mut middle = self.take_exact_area(old_range)?;
            middle.notify_mmap_close();
            middle.unmap_ptes_only(&mut self.page_table);
            self.remove_area_with_overlap_range(new_range)?;
            middle.rebase(new_start, new_pages);
            middle.remap_existing_frames(&mut self.page_table)?;
            middle.notify_mmap_open();
            self.areas.push(middle);
            return Ok(new_addr);
        }

        if new_len == old_len {
            return Ok(old_addr);
        }

        if new_len < old_len {
            let keep_end = old_addr.checked_add(new_len).ok_or(Errno::ENOMEM)?;
            self.remove_area_with_overlap_range(VPNRange::new(
                VirtAddr::from(keep_end).floor(),
                VirtAddr::from(old_end).floor(),
            ))?;
            return Ok(old_addr);
        }

        let new_end = old_addr.checked_add(new_len).ok_or(Errno::ENOMEM)?;
        if new_end <= MMAP_MAX_ADDR {
            let extension = VPNRange::new(
                VirtAddr::from(old_end).floor(),
                VirtAddr::from(new_end).floor(),
            );
            if !self.area_intersects(&extension) {
                self.extend_area_end(old_range, VirtAddr::from(new_end).floor())?;
                return Ok(old_addr);
            }
        }

        if !maymove {
            return Err(Errno::ENOMEM);
        }

        let new_addr = self.choose_mmap_start(None, new_len)?;
        let new_end = new_addr.checked_add(new_len).ok_or(Errno::ENOMEM)?;
        let new_start = VirtAddr::from(new_addr).floor();
        let new_range = VPNRange::new(new_start, VirtAddr::from(new_end).floor());
        if self.area_intersects(&new_range) {
            return Err(Errno::ENOMEM);
        }
        let mut middle = self.take_exact_area(old_range)?;
        middle.notify_mmap_close();
        middle.unmap_ptes_only(&mut self.page_table);
        middle.rebase(new_start, new_pages);
        middle.remap_existing_frames(&mut self.page_table)?;
        middle.notify_mmap_open();
        self.areas.push(middle);
        Ok(new_addr)
    }

    fn take_exact_area(&mut self, vpn_range: VPNRange) -> SysResult<MapArea> {
        let mut idx = 0;
        while idx < self.areas.len() {
            if !self.areas[idx].vpn_range.contain_range(&vpn_range) {
                idx += 1;
                continue;
            }

            let area = self.areas.remove(idx);
            let split = area.split_by_overlap(vpn_range.get_start(), vpn_range.get_end());
            if let Some(left) = split.left {
                self.areas.insert(idx, left);
                idx += 1;
            }
            if let Some(right) = split.right {
                self.areas.insert(idx, right);
            }
            return Ok(split.middle);
        }
        Err(Errno::EFAULT)
    }

    fn extend_area_end(&mut self, old_range: VPNRange, new_end: VirtPageNum) -> SysResult {
        let (idx, area) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| area.vpn_range.contain_range(&old_range))
            .ok_or(Errno::EFAULT)?;

        if area.vpn_range.get_start() != old_range.get_start()
            || area.vpn_range.get_end() != old_range.get_end()
        {
            let area = self.areas.remove(idx);
            let split = area.split_by_overlap(old_range.get_start(), old_range.get_end());
            let mut middle = split.middle;
            middle.resize_end(new_end, &mut self.page_table);
            if let Some(left) = split.left {
                self.areas.insert(idx, left);
                self.areas.insert(idx + 1, middle);
            } else {
                self.areas.insert(idx, middle);
            }
            if let Some(right) = split.right {
                self.areas.push(right);
            }
            return Ok(());
        }

        area.resize_end(new_end, &mut self.page_table);
        Ok(())
    }

    /// 惰性重映射：只修改 VPN 范围，不分配/释放物理页（由 page fault handler 按需处理）
    pub fn remap_area_lazy(
        &mut self,
        vpn_start: VirtPageNum,
        new_vpn_end: VirtPageNum,
    ) -> SysResult {
        let (idx, area) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| area.vpn_range.get_start() == vpn_start)
            .ok_or(Errno::EINVAL)?;

        if vpn_start == new_vpn_end {
            area.notify_mmap_close();
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
            return Ok(());
        }

        let old_vpn_end = area.vpn_range.get_end();
        if new_vpn_end < old_vpn_end {
            let vpn_range = VPNRange::new(new_vpn_end, old_vpn_end);
            for vpn in vpn_range {
                area.unmap_one(&mut self.page_table, vpn);
            }
        }
        area.vpn_range = VPNRange::new(vpn_start, new_vpn_end);
        Ok(())
    }

    /// 调整以指定页为结尾的可写用户区域，供 brk 在遇到前置保护页后继续维护堆尾。
    pub fn remap_writable_area_lazy_from_end(
        &mut self,
        old_vpn_end: VirtPageNum,
        new_vpn_end: VirtPageNum,
    ) -> SysResult {
        let (idx, area) = self
            .areas
            .iter_mut()
            .enumerate()
            .rev()
            .find(|(_, area)| {
                area.vpn_range.get_end() == old_vpn_end && Self::is_private_writable_anonymous(area)
            })
            .ok_or(Errno::EINVAL)?;

        let vpn_start = area.vpn_range.get_start();
        if vpn_start == new_vpn_end {
            area.notify_mmap_close();
            area.unmap(&mut self.page_table);
            self.areas.remove(idx);
            return Ok(());
        }

        if new_vpn_end < old_vpn_end {
            for vpn in VPNRange::new(new_vpn_end, old_vpn_end) {
                area.unmap_one(&mut self.page_table, vpn);
            }
        }
        area.vpn_range = VPNRange::new(vpn_start, new_vpn_end);
        Ok(())
    }

    /// 修改与给定虚拟页区间重叠的映射权限，必要时裁剪或切分原逻辑段
    pub fn remap_area_with_overlap_range(
        &mut self,
        vpn_range: VPNRange,
        map_perm: MapPermission,
    ) -> SysResult {
        let new_map_perm = map_perm | MapPermission::USER;

        for vpn in vpn_range {
            let area = self
                .areas
                .iter()
                .rev()
                .find(|area| area.vpn_range.contain(&vpn))
                .ok_or(Errno::ENOMEM)?;
            if !area.map_perm.contains(MapPermission::USER) {
                return Err(Errno::EFAULT);
            }
        }

        if new_map_perm.contains(MapPermission::WRITE) {
            for area in self
                .areas
                .iter()
                .filter(|area| area.vpn_range.intersect_with(&vpn_range))
            {
                if area.shared {
                    if let Some(backing) = &area.file_backing {
                        backing.file.mmap_allowed(true, true)?;
                    }
                }
            }
        }

        let mut idx = 0;
        while idx < self.areas.len() {
            if !self.areas[idx].vpn_range.intersect_with(&vpn_range) {
                idx += 1;
                continue;
            }

            let area = self.areas.remove(idx);
            let overlap_start = area.vpn_range.get_start().max(vpn_range.get_start());
            let overlap_end = area.vpn_range.get_end().min(vpn_range.get_end());
            let split = area.split_by_overlap(overlap_start, overlap_end);
            let mut middle = split.middle;
            let old_writable = middle.map_perm.contains(MapPermission::WRITE);
            let new_writable = new_map_perm.contains(MapPermission::WRITE);
            if old_writable && !new_writable {
                middle.notify_mmap_close();
            }

            // 修改中间段已映射页的 PTE 权限
            let mapped_vpns: Vec<_> = middle.data_frames.keys().copied().collect();
            for vpn in mapped_vpns {
                self.modify_user_pte_perm(vpn, new_map_perm);
            }
            middle.map_perm = new_map_perm;
            if !old_writable && new_writable {
                middle.notify_mmap_open();
            }

            if let Some(left) = split.left {
                self.areas.insert(idx, left);
                idx += 1;
            }

            self.areas.insert(idx, middle);
            idx += 1;

            if let Some(right) = split.right {
                self.areas.insert(idx, right);
                idx += 1;
            }
        }
        Ok(())
    }

    pub fn advise_fork_behavior(&mut self, vpn_range: VPNRange, wipe_on_fork: bool) -> SysResult {
        for vpn in vpn_range {
            let area = self
                .areas
                .iter()
                .rev()
                .find(|area| area.vpn_range.contain(&vpn))
                .ok_or(Errno::ENOMEM)?;
            if !area.map_perm.contains(MapPermission::USER) {
                return Err(Errno::EFAULT);
            }
            if area.shared || area.file_backing.is_some() {
                return Err(Errno::EINVAL);
            }
        }

        let mut idx = 0;
        while idx < self.areas.len() {
            if !self.areas[idx].vpn_range.intersect_with(&vpn_range) {
                idx += 1;
                continue;
            }

            let area = self.areas.remove(idx);
            let overlap_start = area.vpn_range.get_start().max(vpn_range.get_start());
            let overlap_end = area.vpn_range.get_end().min(vpn_range.get_end());
            let split = area.split_by_overlap(overlap_start, overlap_end);
            let mut middle = split.middle;
            middle.wipe_on_fork = wipe_on_fork;

            if let Some(left) = split.left {
                self.areas.insert(idx, left);
                idx += 1;
            }
            self.areas.insert(idx, middle);
            idx += 1;
            if let Some(right) = split.right {
                self.areas.insert(idx, right);
                idx += 1;
            }
        }
        Ok(())
    }

    pub fn set_locked_range(&mut self, vpn_range: VPNRange, locked: bool) -> SysResult {
        for vpn in vpn_range {
            let area = self
                .areas
                .iter()
                .rev()
                .find(|area| area.vpn_range.contain(&vpn))
                .ok_or(Errno::ENOMEM)?;
            if !area.map_perm.contains(MapPermission::USER) {
                return Err(Errno::ENOMEM);
            }
        }

        let mut idx = 0;
        while idx < self.areas.len() {
            if !self.areas[idx].vpn_range.intersect_with(&vpn_range) {
                idx += 1;
                continue;
            }

            let area = self.areas.remove(idx);
            let overlap_start = area.vpn_range.get_start().max(vpn_range.get_start());
            let overlap_end = area.vpn_range.get_end().min(vpn_range.get_end());
            let split = area.split_by_overlap(overlap_start, overlap_end);
            let mut middle = split.middle;
            middle.locked = locked;

            if let Some(left) = split.left {
                self.areas.insert(idx, left);
                idx += 1;
            }
            self.areas.insert(idx, middle);
            idx += 1;
            if let Some(right) = split.right {
                self.areas.insert(idx, right);
                idx += 1;
            }
        }
        Ok(())
    }

    /// 校验 madvise 对指定地址范围的基础 Linux 语义。
    ///
    /// 对当前内核暂不产生实际页回收/预取行为的 advice，也必须先完成地址范围
    /// 和 advice 适用对象校验，不能把非法入参当作成功。
    pub fn check_madvise_range(&self, vpn_range: VPNRange, advice: i32) -> SysResult {
        for vpn in vpn_range {
            let area = self
                .areas
                .iter()
                .rev()
                .find(|area| area.vpn_range.contain(&vpn))
                .ok_or(Errno::ENOMEM)?;
            if !area.map_perm.contains(MapPermission::USER) {
                return Err(Errno::ENOMEM);
            }

            match advice {
                4 => {
                    // MADV_DONTNEED is rejected for locked mappings.
                    if area.locked {
                        return Err(Errno::EINVAL);
                    }
                }
                8 => {
                    // MADV_FREE applies only to private anonymous mappings.
                    if !Self::is_private_writable_anonymous(area) {
                        return Err(Errno::EINVAL);
                    }
                }
                _ => {}
            }
        }

        match advice {
            5 | 12 | 13 => Err(Errno::EINVAL),
            _ => Ok(()),
        }
    }

    fn modify_user_pte_perm(&mut self, vpn: VirtPageNum, map_perm: MapPermission) {
        let Some(pte) = self.page_table.translate(vpn) else {
            return;
        };
        if !pte.is_valid() {
            return;
        }

        let mut flags = PTEFlags::from(map_perm);
        if pte.is_cow() {
            flags |= PTEFlags::COW;
            flags.remove(PTEFlags::WRITE);
        }
        self.page_table.modify_pte(vpn, flags);
    }

    /// 修改页表基址寄存器，切换页表
    pub fn flush_tlb(&self) {
        sfence();
    }

    /// 激活地址空间
    #[cfg(target_arch = "riscv64")]
    pub fn activate(&self) {
        let token = self.page_table.token();
        let vpn = VirtAddr::from(stext as *const () as usize).floor();
        let pte = self.page_table.translate(vpn).unwrap();
        assert!(pte.is_valid());
        assert!(pte.readable() || pte.executable() || pte.writable());

        write_mmu_token(token);
        self.flush_tlb();
    }

    #[cfg(target_arch = "loongarch64")]
    pub fn activate(&self) {
        let token = self.page_table.token();
        write_mmu_token(token);
        if !crate::arch::paging_enabled() {
            crate::arch::enable_mmu();
        }
        self.flush_tlb();
    }

    /// 生成页表对应 `stap` 寄存器值
    pub fn token(&self) -> usize {
        self.page_table.token()
    }
    /// 转译虚拟页号为物理页号
    pub fn translate(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        self.page_table.translate(vpn)
    }

    /// 遍历所有映射区域，供外部观察者（如 /proc/self/smaps）使用。
    ///
    /// 闭包参数依次为：起始虚拟地址、结束虚拟地址、权限。
    pub fn each_area(&self, mut f: impl FnMut(usize, usize, MapPermission, bool, bool)) {
        for area in self.areas.iter() {
            let start = area.vpn_range.get_start().0 << PAGE_SIZE_BITS;
            let end = area.vpn_range.get_end().0 << PAGE_SIZE_BITS;
            f(start, end, area.map_perm, area.shared, area.locked);
        }
    }

    /// 回收内部地址空间
    pub fn recycle_data_pages(&mut self) {
        for area in self.areas.iter_mut() {
            area.notify_mmap_close();
            area.unmap(&mut self.page_table);
        }
        self.areas.clear();
        // 退出路径可能仍短暂运行在当前用户页表上，页表页帧不能立刻
        // 归还给通用分配器。各架构的 retire_owned_frames() 会把页表帧
        // 放入有限隔离队列，过一段时间再释放。
        self.page_table.retire_owned_frames();
    }
}

impl MemorySet {
    /// 创建一个新的地址空间，内部没有逻辑段
    pub fn new() -> Self {
        Self {
            brk: 0,
            heap_bottom: 0,
            mmap_start: Self::initial_mmap_start(),
            page_table: PageTable::new(),
            areas: Vec::new(),
        }
    }
    /// 创建一个拥有内核空间根页表页信息的地址空间，主要用于用户进程
    pub fn from_kernel_page_table() -> SysResult<Self> {
        Ok(Self {
            brk: 0,
            heap_bottom: 0,
            mmap_start: Self::initial_mmap_start(),
            page_table: PageTable::from_kernel()?,
            areas: Vec::new(),
        })
    }

    /// 创建内核地址空间
    ///
    /// 为内核地址建立虚拟地址，使其在虚拟地址开启时仍能正常访问内核空间，内核采用恒等映射
    pub fn new_kernel() -> Self {
        let mut memory_set = Self::new();
        // 尝试移除跳板映射
        // memory_set.map_trampoline();
        // 内核各段作为逻辑段加入地址空间
        // .text段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(stext as *const () as usize),
                VirtAddr::from(etext as *const () as usize),
                MapType::Direct,
                MapPermission::READ | MapPermission::EXECUTE,
            ),
            None,
            0,
        );
        // .rodata段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(srodata as *const () as usize),
                VirtAddr::from(erodata as *const () as usize),
                MapType::Direct,
                MapPermission::READ,
            ),
            None,
            0,
        );
        // .data段
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(sdata as *const () as usize),
                VirtAddr::from(edata as *const () as usize),
                MapType::Direct,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None,
            0,
        );
        // .bss段和栈段（该栈段指的是初始分配的栈空间，初始化在.bss段）
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(sbss_with_stack as *const () as usize),
                VirtAddr::from(ebss as *const () as usize),
                MapType::Direct,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None,
            0,
        );
        // 内核剩余部分
        memory_set.push_empty_map_area(
            MapArea::new(
                VirtAddr::from(ekernel as *const () as usize),
                VirtAddr::from(KERNEL_BASE + MEMORY_END),
                MapType::Direct,
                MapPermission::READ | MapPermission::WRITE,
            ),
            None,
            0,
        );
        // 设备 MMIO 区域
        for (start, len) in VIRTIO_MMIO.iter().copied() {
            memory_set.push_empty_map_area(
                MapArea::new(
                    VirtAddr::from(KERNEL_BASE + start),
                    VirtAddr::from(KERNEL_BASE + start + len),
                    MapType::Direct,
                    MapPermission::READ | MapPermission::WRITE,
                ),
                None,
                0,
            );
        }

        memory_set
    }

    /// 根据 elf 格式的用户程序文件数据，创建用户程序内核空间
    ///
    /// 内部完成对elf文件的解析，当前内核对堆栈地址的处理能力不完善
    pub fn from_elf_data(elf_data: &[u8]) -> (Self, usize, usize, usize, Vec<AuxHeader>) {
        Self::try_from_elf_data(elf_data).expect("failed to load elf data")
    }

    pub fn try_from_elf_data(
        elf_data: &[u8],
    ) -> SysResult<(Self, usize, usize, usize, Vec<AuxHeader>)> {
        let mut memory_set =
            Self::from_kernel_page_table().map_err(|_| Errno::ENOMEM)?;

        // 在用户空间映射 sigreturn 跳板页
        memory_set.try_map_trampoline()?;
        // 由于传入的是 elf 格式的数据，所以需要读取文件头来得到各段的地址，之后再做分配映射
        let elf = xmas_elf::ElfFile::new(elf_data).map_err(|_| Errno::ENOEXEC)?;
        let elf_header = elf.header;
        let magic = elf_header.pt1.magic;
        if magic != [0x7f, 0x45, 0x4c, 0x46] || elf_data.len() < 18 {
            return Err(Errno::ENOEXEC);
        }
        let ph_count = elf_header.pt2.ph_count();
        let ph_entsize = elf_header.pt2.ph_entry_size() as usize;
        let has_interp = (0..ph_count).any(|i| {
            elf.program_header(i).ok().and_then(|ph| ph.get_type().ok())
                == Some(xmas_elf::program::Type::Interp)
        });
        let elf_type = u16::from_le_bytes([elf_data[16], elf_data[17]]);
        let load_bias = if elf_type == ET_DYN && has_interp {
            PIE_LOAD_OFFSET
        } else {
            0
        };
        let app_entry_point = elf_header.pt2.entry_point() as usize + load_bias;
        let mut entry_point = app_entry_point;

        let mut max_vpn_end = VirtPageNum(0);
        let mut ph_va: usize = 0;
        let mut first_load: bool = true;
        let mut need_dl: bool = false;
        let mut interp_path: Option<alloc::string::String> = None;

        for i in 0..ph_count {
            let ph = elf.program_header(i).map_err(|_| Errno::ENOEXEC)?;
            let ph_type = ph.get_type().map_err(|_| Errno::ENOEXEC)?;

            if ph_type == xmas_elf::program::Type::Load {
                if ph.file_size() > ph.mem_size() {
                    return Err(Errno::ENOEXEC);
                }
                let start_va = VirtAddr::from(ph.virtual_addr() as usize + load_bias);
                let end_va =
                    VirtAddr::from(ph.virtual_addr() as usize + load_bias + ph.mem_size() as usize);
                if first_load {
                    // 第一个 LOAD 段的起始地址减去 ELF 头中的 ph_offset 即为程序头表虚拟地址
                    ph_va = start_va.0 + elf_header.pt2.ph_offset() as usize;
                    first_load = false;
                }

                let mut map_perm = MapPermission::USER;
                let ph_flags = ph.flags();
                if ph_flags.is_read() {
                    map_perm |= MapPermission::READ;
                }
                if ph_flags.is_write() {
                    map_perm |= MapPermission::WRITE;
                }
                if ph_flags.is_execute() {
                    map_perm |= MapPermission::EXECUTE;
                }

                let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                max_vpn_end = max_vpn_end.max(map_area.vpn_range.get_end());
                let data_offset = start_va.0 & (PAGE_SIZE - 1);
                let file_start = ph.offset() as usize;
                let file_end = file_start
                    .checked_add(ph.file_size() as usize)
                    .ok_or(Errno::ENOEXEC)?;
                let file_data = elf.input.get(file_start..file_end).ok_or(Errno::ENOEXEC)?;
                memory_set.try_push_empty_map_area(
                    map_area,
                    Some(file_data),
                    data_offset,
                )?;
            }

            if ph_type == xmas_elf::program::Type::Interp {
                need_dl = true;
                let start = ph.offset() as usize;
                let end = start
                    .checked_add(ph.file_size() as usize)
                    .ok_or(Errno::ENOEXEC)?;
                let interp_data = elf.input.get(start..end).ok_or(Errno::ENOEXEC)?;
                if let Ok(s) = core::str::from_utf8(interp_data) {
                    interp_path = Some(alloc::string::String::from(s.trim_end_matches('\0')));
                }
            }
        }

        // —— 兼容未剥离 ELF：若 PT_INTERP 未能解析，再尝试 .interp section ——
        if need_dl && interp_path.is_none() {
            if let Some(section) = elf.find_section_by_name(".interp") {
                let raw = section.raw_data(&elf);
                if let Ok(s) = core::str::from_utf8(raw) {
                    interp_path = Some(alloc::string::String::from(s.trim_end_matches('\0')));
                }
            }
        }

        // —— 加载动态链接器 ——
        if let Some(ref interp) = interp_path {
            let fs_interp_data = read_dynamic_linker(interp);
            let interp_data = fs_interp_data
                .as_deref()
                .or_else(|| crate::loader::get_app_data_by_name(interp));

            if let Some(interp_data) = interp_data {
                let interp_elf =
                    xmas_elf::ElfFile::new(interp_data).map_err(|_| Errno::ENOEXEC)?;
                let interp_head = interp_elf.header;
                let interp_ph_count = interp_head.pt2.ph_count();
                entry_point = interp_head.pt2.entry_point() as usize + DL_INTERP_OFFSET;

                for i in 0..interp_ph_count {
                    let ph = interp_elf.program_header(i).map_err(|_| Errno::ENOEXEC)?;
                    if ph.get_type().map_err(|_| Errno::ENOEXEC)?
                        == xmas_elf::program::Type::Load
                    {
                        if ph.file_size() > ph.mem_size() {
                            return Err(Errno::ENOEXEC);
                        }
                        let start_va =
                            VirtAddr::from(ph.virtual_addr() as usize + DL_INTERP_OFFSET);
                        let end_va = VirtAddr::from(
                            ph.virtual_addr() as usize + DL_INTERP_OFFSET + ph.mem_size() as usize,
                        );
                        let mut map_perm = MapPermission::USER;
                        let ph_flags = ph.flags();
                        if ph_flags.is_read() {
                            map_perm |= MapPermission::READ;
                        }
                        if ph_flags.is_write() {
                            map_perm |= MapPermission::WRITE;
                        }
                        if ph_flags.is_execute() {
                            map_perm |= MapPermission::EXECUTE;
                        }
                        let map_area = MapArea::new(start_va, end_va, MapType::Framed, map_perm);
                        max_vpn_end = max_vpn_end.max(map_area.vpn_range.get_end());
                        let data_offset = start_va.0 & (PAGE_SIZE - 1);
                        let file_start = ph.offset() as usize;
                        let file_end = file_start
                            .checked_add(ph.file_size() as usize)
                            .ok_or(Errno::ENOEXEC)?;
                        let file_data = interp_data
                            .get(file_start..file_end)
                            .ok_or(Errno::ENOEXEC)?;
                        memory_set.try_push_empty_map_area(
                            map_area,
                            Some(file_data),
                            data_offset,
                        )?;
                    }
                }

                info!(
                    "[from_elf_data] loaded dynamic linker: {} at {:#x}",
                    interp, DL_INTERP_OFFSET
                );
            } else {
                warn!(
                    "[from_elf_data] dynamic linker '{}' not found in embedded apps, proceeding without it",
                    interp
                );
                need_dl = false;
            }
        }

        // —— 构建 aux 向量 ——
        let mut aux_vec: Vec<AuxHeader> = alloc::vec![
            AuxHeader {
                aux_type: AT_PHDR,
                value: ph_va
            },
            AuxHeader {
                aux_type: AT_PHENT,
                value: ph_entsize
            },
            AuxHeader {
                aux_type: AT_PHNUM,
                value: ph_count as usize
            },
            AuxHeader {
                aux_type: AT_PAGESZ,
                value: PAGE_SIZE
            },
            AuxHeader {
                aux_type: AT_ENTRY,
                value: app_entry_point
            },
            AuxHeader {
                aux_type: AT_UID,
                value: 0
            },
            AuxHeader {
                aux_type: AT_EUID,
                value: 0
            },
            AuxHeader {
                aux_type: AT_GID,
                value: 0
            },
            AuxHeader {
                aux_type: AT_EGID,
                value: 0
            },
            AuxHeader {
                aux_type: AT_CLKTCK,
                value: CLK_TCK
            },
        ];

        if need_dl {
            aux_vec.push(AuxHeader {
                aux_type: AT_BASE,
                value: DL_INTERP_OFFSET,
            });
        }

        // 映射其余段
        let max_va_end = VirtAddr::from(max_vpn_end);
        let mut user_stack_bottom = usize::from(max_va_end);
        user_stack_bottom += PAGE_SIZE; // 上移栈底，将空白页作为守护页，当栈溢出时将访问守护页而发生段错误
        let user_stack_top = user_stack_bottom + USER_STACK_SIZE;
        // 映射栈段
        memory_set.try_push_empty_map_area(
            MapArea::new(
                VirtAddr::from(user_stack_bottom),
                VirtAddr::from(user_stack_top),
                MapType::Framed,
                MapPermission::READ | MapPermission::WRITE | MapPermission::USER,
            ),
            None,
            0,
        )?;
        // 映射堆段，初始不分配堆内存。当前用户栈不是放在固定高地址，
        // 因此 brk 区必须放在栈之后，避免静态程序中堆和栈起点重叠。
        let heap_bottom = user_stack_top + PAGE_SIZE;
        memory_set.heap_bottom = heap_bottom;
        memory_set.brk = heap_bottom;

        // 不再映射异常上下文到用户空间，发生异常时，切换到内核将用户上下文存入内核栈

        let token = memory_set.page_table.token();
        Ok((
            memory_set, // 用户程序地址空间
            token,
            user_stack_top, // 用户程序栈顶地址
            entry_point,    // 用户程序入口地址（动态链接时指向 ld-linux）
            aux_vec,        // auxiliary vector
        ))
    }

    /// 基于已有用户地址空间创建新地址空间（用于 fork）
    ///
    /// 使用 COW（写时复制）策略：
    /// - 可写页：父子共享物理帧，两边 PTE 标记为只读 + COW
    /// - 只读页：直接共享物理帧，保持只读
    /// - 惰性未分配页：只复制 VPN 范围，各自按需分配
    pub fn from_existed_user(user_space: &mut MemorySet) -> SysResult<Self> {
        let mut memory_set = Self::from_kernel_page_table()?;
        memory_set.brk = user_space.brk;
        memory_set.heap_bottom = user_space.heap_bottom;
        memory_set.mmap_start = user_space.mmap_start;

        let mut copied_vpns = BTreeSet::new();
        for area in user_space.areas.iter_mut() {
            let mut new_area = MapArea::from_another(area);
            let is_writable = area.map_perm.contains(MapPermission::WRITE);

            for vpn in area.vpn_range {
                if area.wipe_on_fork {
                    continue;
                }
                if !area.data_frames.contains_key(&vpn) {
                    // 惰性未分配页：PTE 无效，data_frames 无记录
                    // 子进程也跳过，各自在访问时按需分配
                    continue;
                }
                if !copied_vpns.insert(vpn) {
                    // mmap/mremap/fork 压力下若父地址空间残留了重叠 area，
                    // 同一 VPN 的真实 PTE 仍然只有一个。fork 复制时按父页表的
                    // 首次可见映射复制，避免对子页表二次 map() 返回 EEXIST。
                    continue;
                }

                let shared_frame = area.data_frames.get(&vpn).unwrap().clone();
                let ppn = shared_frame.ppn();
                let parent_pte = user_space.page_table.translate(vpn).unwrap();

                if area.shared {
                    let child_flags = parent_pte.flags();
                    if let Err(err) = memory_set.page_table.map(vpn, ppn, child_flags) {
                        println!(
                            "[fork] duplicate shared vpn={:#x}, area=[{:#x}, {:#x}), err={:?}",
                            vpn.0,
                            area.vpn_range.get_start().0,
                            area.vpn_range.get_end().0,
                            err
                        );
                        return Err(err);
                    }
                } else if is_writable {
                    // COW 共享：先建立子进程 PTE，成功后再修改父进程 PTE。
                    // 这样 fork 中途因 ENOMEM 等错误失败时，不会把父地址空间
                    // 留在半 COW 状态。
                    let mut flags = parent_pte.flags();
                    flags.remove(PTEFlags::WRITE | PTEFlags::DIRTY);

                    let mut child_flags = parent_pte.flags();
                    child_flags.remove(PTEFlags::WRITE | PTEFlags::DIRTY);
                    if let Err(err) = memory_set.page_table.map(vpn, ppn, child_flags) {
                        println!(
                            "[fork] duplicate cow vpn={:#x}, area=[{:#x}, {:#x}), err={:?}",
                            vpn.0,
                            area.vpn_range.get_start().0,
                            area.vpn_range.get_end().0,
                            err
                        );
                        return Err(err);
                    }
                    memory_set.page_table.make_pte_cow(vpn);

                    user_space.page_table.modify_pte(vpn, flags);
                    user_space.page_table.make_pte_cow(vpn);
                } else {
                    // 只读页直接共享，无需 COW
                    let child_flags = parent_pte.flags();
                    if let Err(err) = memory_set.page_table.map(vpn, ppn, child_flags) {
                        println!(
                            "[fork] duplicate readonly vpn={:#x}, area=[{:#x}, {:#x}), err={:?}",
                            vpn.0,
                            area.vpn_range.get_start().0,
                            area.vpn_range.get_end().0,
                            err
                        );
                        return Err(err);
                    }
                }
                new_area.data_frames.insert(vpn, shared_frame);
            }
            new_area.notify_mmap_open();
            memory_set.areas.push(new_area);
        }
        user_space.flush_tlb();
        Ok(memory_set)
    }
}

impl MemorySet {
    /// 检查用户传进来的虚拟地址的合法性
    ///
    /// 使用 `MapArea` 做检查，而不是查页表
    /// 要保证 `MapArea` 与页表的一致性，也就是说，页表中的映射都在MapArea中
    pub fn check_valid_user_vpn(
        &self,
        vpn: VirtPageNum,
        wanted_map_perm: MapPermission,
    ) -> SysResult<VPNRange> {
        let wanted = wanted_map_perm | MapPermission::USER;

        for area in self.areas.iter().rev() {
            if area.vpn_range.contain(&vpn) {
                if area.map_perm.contains(wanted) {
                    return Ok(area.vpn_range.clone());
                } else {
                    return Err(Errno::EFAULT);
                }
            }
        }
        Err(Errno::EFAULT)
    }
    pub fn check_valid_user_vpn_range(
        &self,
        vpn_range: VPNRange,
        wanted_map_perm: MapPermission,
    ) -> SysResult<VPNRange> {
        let wanted = wanted_map_perm | MapPermission::USER;

        for area in self.areas.iter().rev() {
            if area.vpn_range.contain_range(&vpn_range) {
                if area.map_perm.contains(wanted) {
                    return Ok(area.vpn_range.clone());
                } else {
                    return Err(Errno::EFAULT);
                }
            }
        }
        Err(Errno::EFAULT)
    }

    /// 检查一段用户虚拟页都属于用户映射区。
    ///
    /// 这里逐页检查，允许范围跨过相邻的多个用户 VMA。
    pub fn check_user_mapped_range(&self, vpn_range: VPNRange) -> SysResult {
        for vpn in vpn_range {
            self.check_valid_user_vpn(vpn, MapPermission::empty())?;
        }
        Ok(())
    }

    /// 检查一段用户虚拟页都属于满足权限要求的用户映射区。
    ///
    /// 与 `check_valid_user_vpn_range` 不同，这里允许范围跨过多个相邻 VMA。
    pub fn check_user_access_range(
        &self,
        vpn_range: VPNRange,
        wanted_map_perm: MapPermission,
    ) -> SysResult {
        for vpn in vpn_range {
            self.check_valid_user_vpn(vpn, wanted_map_perm)?;
        }
        Ok(())
    }

    pub(crate) fn shared_futex_key(&self, vaddr: VirtAddr) -> SysResult<SharedFutexKey> {
        let vpn = vaddr.floor();
        let offset = vaddr.page_offset();
        let area = self
            .areas
            .iter()
            .rev()
            .find(|area| area.vpn_range.contain(&vpn))
            .ok_or(Errno::EFAULT)?;
        if !area.shared {
            return Err(Errno::EFAULT);
        }

        let page_offset = (vpn - area.vpn_range.get_start()) * PAGE_SIZE;
        if let Some(backing) = &area.file_backing {
            let stat = backing.file.get_stat()?;
            let file_offset = backing.offset.checked_add(page_offset).ok_or(Errno::EIO)?;
            return Ok(SharedFutexKey {
                owner: (stat.dev as usize).rotate_left(17) ^ stat.ino as usize ^ 0x6675_7465_7866,
                page_index: file_offset / PAGE_SIZE,
                offset,
            });
        }

        if let Some(attach_id) = area.shm_attach_id {
            return Ok(SharedFutexKey {
                owner: attach_id ^ 0x7368_6d66_7574_6578usize,
                page_index: page_offset / PAGE_SIZE,
                offset,
            });
        }

        let frame = area.data_frames.get(&vpn).ok_or(Errno::EFAULT)?;
        Ok(SharedFutexKey {
            owner: usize::from(frame.ppn()) ^ 0x616e_6f6e_6675_7478usize,
            page_index: page_offset / PAGE_SIZE,
            offset,
        })
    }

    /// 尝试处理用户态页错误，解决 COW 或惰性分配
    pub fn handle_page_fault(&mut self, cause: PageFaultCause, stval: usize) -> SysResult {
        let vpn = VirtAddr::from(stval).floor();

        let area_idx = match self.areas.iter().position(|a| a.vpn_range.contain(&vpn)) {
            Some(idx) => idx,
            None => return Err(Errno::EFAULT),
        };

        let area_perm = self.areas[area_idx].map_perm;
        if !area_perm.contains(MapPermission::USER) {
            return Err(Errno::EFAULT);
        }

        let pte = self.page_table.translate(vpn);
        let is_store = matches!(cause, PageFaultCause::Store);

        // COW 写入：PTE 有效 + COW 标记 + area 允许写
        if is_store && pte.is_some_and(|p| p.is_valid() && p.is_cow()) {
            if !area_perm.contains(MapPermission::WRITE) {
                return Err(Errno::EFAULT);
            }

            let old_frame = self.areas[area_idx]
                .data_frames
                .get(&vpn)
                .ok_or(Errno::EFAULT)?;
            let count = Arc::strong_count(old_frame);

            if count == 1 {
                // 无其他进程共享，直接恢复可写
                let flags = PTEFlags::from(area_perm);
                self.page_table.modify_pte(vpn, flags);
                self.page_table.clear_pte_cow(vpn);
            } else {
                // 多进程共享时，换入一页新物理页并复制旧页内容
                let old_data = old_frame.ppn().get_bytes_array();
                self.areas[area_idx].remap_one_with_data(&mut self.page_table, vpn, old_data)?;
            }
            self.flush_tlb();
            return Ok(());
        }

        // 惰性分配：PTE 无效 + 在有效 area 内
        if pte.is_none() || !pte.unwrap().is_valid() {
            let needed_perm = if is_store {
                MapPermission::WRITE
            } else {
                match cause {
                    PageFaultCause::Instruction => MapPermission::EXECUTE,
                    _ => MapPermission::READ,
                }
            };

            if !area_perm.contains(needed_perm) {
                return Err(Errno::EFAULT);
            }

            self.areas[area_idx].map_one(&mut self.page_table, vpn)?;
            self.flush_tlb();
            return Ok(());
        }

        Err(Errno::EFAULT)
    }

    pub fn ensure_user_page_access(
        &mut self,
        vpn_range: VPNRange,
        perm: MapPermission,
    ) -> SysResult {
        for vpn in vpn_range {
            let pte = self.page_table.translate(vpn);
            let needs_fault = match pte {
                Some(pte) if pte.is_valid() => {
                    perm.contains(MapPermission::WRITE) && (!pte.writable() || pte.is_cow())
                }
                _ => true,
            };
            if !needs_fault {
                continue;
            }

            let cause = if perm.contains(MapPermission::WRITE) {
                PageFaultCause::Store
            } else if perm.contains(MapPermission::EXECUTE) {
                PageFaultCause::Instruction
            } else {
                PageFaultCause::Load
            };
            let va = usize::from(VirtAddr::from(vpn));
            self.handle_page_fault(cause, va)?;
        }
        Ok(())
    }
}

/// 逻辑段
///
/// 一段连续地址 [`VPNRange`] 的虚拟内存
struct MapArea {
    vpn_range: VPNRange,
    data_frames: BTreeMap<VirtPageNum, Arc<FrameTracker>>,
    map_type: MapType,
    map_perm: MapPermission,
    shared: bool,
    locked: bool,
    wipe_on_fork: bool,
    file_backing: Option<FileBacking>,
    shm_attach_id: Option<usize>,
}

#[derive(Clone)]
struct FileBacking {
    file: Arc<dyn FileOp>,
    offset: usize,
    len: usize,
}

#[derive(Clone)]
struct MapAreaMeta {
    map_type: MapType,
    map_perm: MapPermission,
    shared: bool,
    locked: bool,
    wipe_on_fork: bool,
    file_backing: Option<FileBacking>,
    shm_attach_id: Option<usize>,
}

struct MapAreaSplit {
    left: Option<MapArea>,
    middle: MapArea,
    right: Option<MapArea>,
}

impl MapArea {
    /// 创建空逻辑段
    ///
    /// 只指定了一段虚拟内存，内部没有实际的映射的页帧
    pub fn new(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
    ) -> Self {
        let start_vpn: VirtPageNum = start_va.floor();
        let end_vpn: VirtPageNum = end_va.ceil();
        Self {
            vpn_range: VPNRange::new(start_vpn, end_vpn),
            data_frames: BTreeMap::new(),
            map_type,
            map_perm,
            shared: false,
            locked: false,
            wipe_on_fork: false,
            file_backing: None,
            shm_attach_id: None,
        }
    }

    pub fn new_with_flags(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_type: MapType,
        map_perm: MapPermission,
        shared: bool,
        locked: bool,
    ) -> Self {
        let mut area = Self::new(start_va, end_va, map_type, map_perm);
        area.shared = shared;
        area.locked = locked;
        area
    }

    pub fn new_shared(start_va: VirtAddr, end_va: VirtAddr, map_perm: MapPermission) -> Self {
        let mut area = Self::new(start_va, end_va, MapType::Framed, map_perm);
        area.shared = true;
        area
    }

    pub fn new_file_backed(
        start_va: VirtAddr,
        end_va: VirtAddr,
        map_perm: MapPermission,
        shared: bool,
        locked: bool,
        file: Arc<dyn FileOp>,
        offset: usize,
        len: usize,
    ) -> Self {
        let mut area = Self::new(start_va, end_va, MapType::Framed, map_perm);
        area.shared = shared;
        area.locked = locked;
        area.file_backing = Some(FileBacking { file, offset, len });
        area
    }

    fn meta(&self) -> MapAreaMeta {
        MapAreaMeta {
            map_type: self.map_type,
            map_perm: self.map_perm,
            shared: self.shared,
            locked: self.locked,
            wipe_on_fork: self.wipe_on_fork,
            file_backing: self.file_backing.clone(),
            shm_attach_id: self.shm_attach_id,
        }
    }

    fn from_parts(
        vpn_range: VPNRange,
        data_frames: BTreeMap<VirtPageNum, Arc<FrameTracker>>,
        meta: MapAreaMeta,
    ) -> Self {
        Self {
            vpn_range,
            data_frames,
            map_type: meta.map_type,
            map_perm: meta.map_perm,
            shared: meta.shared,
            locked: meta.locked,
            wipe_on_fork: meta.wipe_on_fork,
            file_backing: meta.file_backing,
            shm_attach_id: meta.shm_attach_id,
        }
    }

    fn split_by_overlap(
        mut self,
        overlap_start: VirtPageNum,
        overlap_end: VirtPageNum,
    ) -> MapAreaSplit {
        let area_start = self.vpn_range.get_start();
        let area_end = self.vpn_range.get_end();
        debug_assert!(area_start <= overlap_start);
        debug_assert!(overlap_start < overlap_end);
        debug_assert!(overlap_end <= area_end);

        let meta = self.meta();
        let mut middle_and_right = self.data_frames.split_off(&overlap_start);
        let right_frames = middle_and_right.split_off(&overlap_end);
        let left_meta = Self::meta_for_subrange(&meta, area_start, area_start, overlap_start);
        let middle_meta = Self::meta_for_subrange(&meta, area_start, overlap_start, overlap_end);
        let right_meta = Self::meta_for_subrange(&meta, area_start, overlap_end, area_end);

        let left = if overlap_start > area_start {
            Some(Self::from_parts(
                VPNRange::new(area_start, overlap_start),
                self.data_frames,
                left_meta,
            ))
        } else {
            None
        };

        let middle = Self::from_parts(
            VPNRange::new(overlap_start, overlap_end),
            middle_and_right,
            middle_meta,
        );

        let right = if overlap_end < area_end {
            Some(Self::from_parts(
                VPNRange::new(overlap_end, area_end),
                right_frames,
                right_meta,
            ))
        } else {
            None
        };

        MapAreaSplit {
            left,
            middle,
            right,
        }
    }

    fn meta_for_subrange(
        meta: &MapAreaMeta,
        area_start: VirtPageNum,
        range_start: VirtPageNum,
        range_end: VirtPageNum,
    ) -> MapAreaMeta {
        let mut meta = meta.clone();
        if let Some(backing) = &mut meta.file_backing {
            let delta = (range_start - area_start) * PAGE_SIZE;
            let range_len = (range_end - range_start) * PAGE_SIZE;
            backing.offset = backing.offset.saturating_add(delta);
            backing.len = backing.len.saturating_sub(delta).min(range_len);
        }
        meta
    }

    /// 复制构造空逻辑段
    ///
    /// 只指定了一段与传入逻辑段一致的虚拟内存，内部没有实际的映射的页帧
    pub fn from_another(another: &MapArea) -> Self {
        Self::from_parts(
            VPNRange::new(another.vpn_range.get_start(), another.vpn_range.get_end()),
            BTreeMap::new(),
            another.meta(),
        )
    }

    fn page_count(&self) -> usize {
        self.vpn_range.get_end() - self.vpn_range.get_start()
    }

    fn notify_mmap_open(&self) {
        if let Some(backing) = &self.file_backing {
            backing.file.mmap_open(
                self.shared,
                self.map_perm.contains(MapPermission::WRITE),
                self.page_count(),
            );
        }
    }

    fn notify_mmap_close(&self) {
        if let Some(backing) = &self.file_backing {
            backing.file.mmap_close(
                self.shared,
                self.map_perm.contains(MapPermission::WRITE),
                self.page_count(),
            );
        }
    }

    fn resize_end(&mut self, new_end: VirtPageNum, page_table: &mut PageTable) {
        let old_end = self.vpn_range.get_end();
        if new_end < old_end {
            for vpn in VPNRange::new(new_end, old_end) {
                self.unmap_one(page_table, vpn);
            }
        }
        self.vpn_range = VPNRange::new(self.vpn_range.get_start(), new_end);
        let byte_len = self.page_count() * PAGE_SIZE;
        if let Some(backing) = &mut self.file_backing {
            backing.len = byte_len;
        }
    }

    fn unmap_ptes_only(&self, page_table: &mut PageTable) {
        for vpn in self.data_frames.keys().copied() {
            page_table.try_unmap(vpn);
        }
    }

    fn rebase(&mut self, new_start: VirtPageNum, new_pages: usize) {
        let old_start = self.vpn_range.get_start();
        let mut data_frames = BTreeMap::new();
        for (vpn, frame) in core::mem::take(&mut self.data_frames) {
            let offset = vpn - old_start;
            if offset < new_pages {
                data_frames.insert(VirtPageNum(new_start.0 + offset), frame);
            }
        }
        self.data_frames = data_frames;
        self.vpn_range = VPNRange::new(new_start, VirtPageNum(new_start.0 + new_pages));
        if let Some(backing) = &mut self.file_backing {
            backing.len = new_pages * PAGE_SIZE;
        }
    }

    fn remap_existing_frames(&self, page_table: &mut PageTable) -> SysResult {
        let flags = PTEFlags::from(self.map_perm);
        for (vpn, frame) in self.data_frames.iter() {
            page_table.map(*vpn, frame.ppn(), flags)?;
        }
        Ok(())
    }

    /// 为逻辑段上所有虚拟页创建物理页帧并建立映射
    ///
    /// 传入页表的可变借用，以修改传入页表的内容
    pub fn map(&mut self, page_table: &mut PageTable) -> SysResult {
        for vpn in self.vpn_range {
            if let Err(err) = self.map_one(page_table, vpn) {
                let cleanup_end = VirtPageNum(vpn.0 + 1);
                for cleanup_vpn in VPNRange::new(self.vpn_range.get_start(), cleanup_end) {
                    self.unmap_one(page_table, cleanup_vpn);
                }
                return Err(err);
            }
        }
        Ok(())
    }
    /// 为逻辑段上所有虚拟页销毁物理页帧并消除映射
    pub fn unmap(&mut self, page_table: &mut PageTable) {
        for vpn in self.vpn_range {
            self.unmap_one(page_table, vpn);
        }
    }

    /// 复制数据到逻辑段的实际物理页帧上
    pub fn copy_data(&mut self, page_table: &PageTable, data: &[u8], offset: usize) {
        assert_eq!(self.map_type, MapType::Framed);
        let mut start: usize = 0;
        let mut current_vpn = self.vpn_range.get_start();
        let len = data.len();
        // 数据长度不超过逻辑段长度
        assert!(
            len <= PAGE_SIZE * (self.vpn_range.get_end() - current_vpn),
            "[kernel] MapArea Panic: Copy data is out of vpn range!"
        );
        // 数据在段内有偏移，对第一页做特殊处理
        if offset != 0 {
            let src = &data[0..len.min(PAGE_SIZE - offset)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[offset..offset + src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE - offset;
            current_vpn.step();
        }
        loop {
            if start >= len {
                break;
            }
            let src = &data[start..len.min(start + PAGE_SIZE)];
            let dst = &mut page_table
                .translate(current_vpn)
                .unwrap()
                .ppn()
                .get_bytes_array()[..src.len()];
            dst.copy_from_slice(src);
            start += PAGE_SIZE;
            current_vpn.step();
        }
    }

    /// 依据逻辑段的不同映射策略，为虚拟页分配物理页帧，并建立映射关系
    pub fn map_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) -> SysResult {
        let ppn: PhysPageNum;
        match self.map_type {
            // 直接映射，物理页号和虚拟页号存在线性偏移，一般用于内核，无需分配页帧管理，因为内存地址固定
            MapType::Direct => {
                ppn = PhysPageNum(vpn - VirtAddr::from(KERNEL_BASE).floor());
            }
            // 随机映射，物理页号和虚拟页号无关，用于用户程序，分配页帧统一管理
            MapType::Framed => {
                let page_offset = (vpn - self.vpn_range.get_start()) * PAGE_SIZE;
                let frame = if self.shared {
                    if let Some(backing) = &self.file_backing {
                        shared_file_frame(backing, page_offset)?
                    } else {
                        Arc::new(frame_alloc().ok_or(Errno::ENOMEM)?)
                    }
                } else {
                    let frame = frame_alloc().ok_or(Errno::ENOMEM)?;
                    let frame_ppn = frame.ppn();
                    if let Some(backing) = &self.file_backing {
                        if page_offset < backing.len {
                            let file_offset =
                                backing.offset.checked_add(page_offset).ok_or(Errno::EIO)?;
                            let read_len = (backing.len - page_offset).min(PAGE_SIZE);
                            read_file_at(
                                backing.file.clone(),
                                file_offset,
                                &mut frame_ppn.get_bytes_array()[..read_len],
                            )?;
                        }
                    }
                    Arc::new(frame)
                };
                ppn = frame.ppn();
                self.data_frames.insert(vpn, frame);
            }
        }
        let pte_flags = PTEFlags::from(self.map_perm);
        if let Err(err) = page_table.map(vpn, ppn, pte_flags) {
            if self.map_type == MapType::Framed {
                self.data_frames.remove(&vpn);
            }
            return Err(err);
        }
        Ok(())
    }

    /// 为已有映射换入一页新的物理页，并把给定数据复制进去
    pub fn remap_one_with_data(
        &mut self,
        page_table: &mut PageTable,
        vpn: VirtPageNum,
        data: &[u8],
    ) -> SysResult {
        let frame = frame_alloc().ok_or(Errno::ENOMEM)?;
        let ppn = frame.ppn();
        // 由调用者保证 data 合法
        ppn.get_bytes_array().copy_from_slice(data);

        page_table.unmap(vpn);
        self.data_frames.insert(vpn, Arc::new(frame));
        page_table.map(vpn, ppn, PTEFlags::from(self.map_perm))?;
        Ok(())
    }

    /// 消除虚拟页与物理页帧的映射关系，自动销毁失去连接的物理页帧
    pub fn unmap_one(&mut self, page_table: &mut PageTable, vpn: VirtPageNum) {
        match self.map_type {
            MapType::Framed => {
                if self.shared {
                    if let (Some(backing), Some(frame)) =
                        (&self.file_backing, self.data_frames.get(&vpn))
                    {
                        let page_offset = (vpn - self.vpn_range.get_start()) * PAGE_SIZE;
                        if page_offset < backing.len {
                            let file_offset = backing.offset.saturating_add(page_offset);
                            let write_len = (backing.len - page_offset).min(PAGE_SIZE);
                            let _ = write_file_at(
                                backing.file.clone(),
                                file_offset,
                                &frame.ppn().get_bytes_array()[..write_len],
                            );
                        }
                    }
                }
                self.data_frames.remove(&vpn);
            }
            _ => {}
        }
        page_table.try_unmap(vpn);
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum MapType {
    Direct, // 直接映射——线性偏移
    Framed, // 随机映射
}

bitflags! {
    pub struct MapPermission: u16 {
        const READ     = 1 << 1;
        const WRITE    = 1 << 2;
        const EXECUTE  = 1 << 3;
        const USER     = 1 << 4;
        const GLOBAL   = 1 << 5;
        const ACCESSED = 1 << 6;
        const DIRTY    = 1 << 7;
    }
}
