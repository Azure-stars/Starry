#![no_std]
#![feature(btree_cursors)]

#[macro_use]
extern crate log;
extern crate alloc;
use axerrno::LinuxResult;
use mm::VmAreaStruct;
use memory_addr::{is_aligned_4k, align_down_4k, PAGE_SIZE_4K, PAGE_SHIFT};
use memory_addr::align_up_4k;
use core::ops::Bound;
use axhal::mem::{phys_to_virt, virt_to_phys};
use axhal::arch::TASK_UNMAPPED_BASE;
pub use mm::FileRef;

/// Interpret addr exactly.
pub const MAP_FIXED: usize = 0x10;
/// Don't use a file.
pub const MAP_ANONYMOUS: usize = 0x20;

pub fn mmap(
    va: usize, len: usize, prot: usize, flags: usize,
    fd: usize, offset: usize
) -> LinuxResult<usize> {
    let current = task::current();
    let filetable = current.filetable.lock();
    let file = if (flags & MAP_ANONYMOUS) != 0 {
        None
    } else {
        filetable.get_file(fd)
    };
    _mmap(va, len, prot, flags, file, offset)
}

pub fn _mmap(
    mut va: usize, mut len: usize, _prot: usize, flags: usize,
    file: Option<FileRef>, offset: usize
) -> LinuxResult<usize> {
    assert!(is_aligned_4k(va));
    len = align_up_4k(len);
    debug!("mmap va {:#X} offset {:#X}", va, offset);

    if (flags & MAP_FIXED) == 0 {
        va = get_unmapped_vma(va, len);
        debug!("Get unmapped vma {:#X}", va);
    }

    let mm = task::current().mm();
    if let Some(mut overlap) = find_overlap(va, len, flags) {
        info!("find overlap {:#X}-{:#X}", overlap.vm_start, overlap.vm_end);
        assert!(va >= overlap.vm_start && va+len <= overlap.vm_end,
            "{:#X}-{:#X}; overlap {:#X}-{:#X}",
            va, va+len, overlap.vm_start, overlap.vm_end);

        if va + len < overlap.vm_end {
            let bias = (va + len - overlap.vm_start) >> PAGE_SHIFT;
            let mut new = overlap.clone();
            new.vm_start = va + len;
            new.vm_pgoff += bias;
            mm.lock().vmas.insert(va+len, new);
        }
        if va > overlap.vm_start {
            overlap.vm_end = va;
            mm.lock().vmas.insert(overlap.vm_start, overlap);
        }
    }

    info!("mmap region: {:#X} - {:#X}, flags: {:#X}", va, va + len, flags);
    let vma = VmAreaStruct::new(va, va + len, offset >> PAGE_SHIFT, file, flags);
    mm.lock().vmas.insert(va, vma);

    Ok(va)
}

fn find_overlap(va: usize, len: usize, flags: usize) -> Option<VmAreaStruct> {
    info!("find_overlap: va {:#X} len {}, flags {:#X}", va, len, flags);

    let mm = task::current().mm();
    let locked_mm = mm.lock();
    let ret = locked_mm.vmas.iter().find(|(_, vma)| {
        in_vma(va, va+len, vma) ||
        in_range(vma.vm_start, vma.vm_end, va, va+len)
    });

    if let Some((key, _)) = ret {
        warn!("### Removed!!!");
        mm.lock().vmas.remove(&key)
    } else {
        None
    }
}

#[inline]
const fn in_range(start: usize, end: usize, r_start: usize, r_end: usize) -> bool {
    (start >= r_start && start < r_end) || (end > r_start && end <= r_end)
}

#[inline]
const fn in_vma(start: usize, end: usize, vma: &VmAreaStruct) -> bool {
    in_range(start, end, vma.vm_start, vma.vm_end)
}

pub fn get_unmapped_vma(_va: usize, len: usize) -> usize {
    let mm = task::current().mm();
    let locked_mm = mm.lock();
    let mut gap_end = TASK_UNMAPPED_BASE;
    for (_, vma) in locked_mm.vmas.iter().rev() {
        debug!("get_unmapped_vma iterator: {:#X} {:#X} {:#X}",
            vma.vm_start, vma.vm_end, gap_end);
        if vma.vm_end > gap_end {
            continue;
        }
        if gap_end - vma.vm_end >= len {
            info!("get_unmapped_vma: {:#X} {:#X} {:#X}",
                vma.vm_start, vma.vm_end, gap_end);
            return gap_end - len;
        }
        gap_end = vma.vm_start;
    }
    unimplemented!("NO available unmapped vma!");
}

pub fn faultin_page(va: usize) -> usize {
    debug!("faultin_page... va {:#X}", va);
    let mm = task::current().mm();
    let locked_mm = mm.lock();

    let vma = locked_mm.vmas.upper_bound(Bound::Included(&va)).value().unwrap();
    assert!(
        va >= vma.vm_start && va < vma.vm_end,
        "va {:#X} in {:#X} - {:#X}",
        va,
        vma.vm_start,
        vma.vm_end
    );
    let va = align_down_4k(va);
    let delta = va - vma.vm_start;
    //let flags = vma.vm_flags;
    let offset = (vma.vm_pgoff << PAGE_SHIFT) + delta;

    let direct_va: usize = axalloc::global_allocator()
        .alloc_pages(1, PAGE_SIZE_4K).unwrap();

    // Todo: check whether we need to zero it.
    let buf = unsafe {
        core::slice::from_raw_parts_mut(direct_va as *mut u8, PAGE_SIZE_4K)
    };
    buf.fill(0);

    let pa = virt_to_phys(direct_va.into()).into();

    if vma.vm_file.get().is_some() {
        let f = vma.vm_file.get().unwrap().clone();
        locked_mm.fill_cache(pa, PAGE_SIZE_4K, &mut f.lock(), offset);
    }
    let _ = locked_mm.map_region(va, pa, PAGE_SIZE_4K, 1);
    phys_to_virt(pa.into()).into()
}

pub fn set_brk(va: usize) -> usize {
    // Have a guard for mm to lock this whole function,
    // because mm.brk() and mm.set_brk() should be in a atomic context.
    let mm = task::current().mm();
    let brk = mm.lock().brk();

    assert!(is_aligned_4k(brk));
    debug!("brk!!! {:#x}, {:#x}", va, brk);

    if va == 0 {
        brk
    } else {
        assert!(va > brk);
        let offset = va - brk;
        assert!(is_aligned_4k(offset));
        _mmap(brk, offset, 0, MAP_FIXED|MAP_ANONYMOUS, None, 0).unwrap();
        let _ = faultin_page(brk);
        mm.lock().set_brk(va);
        va
    }
}
