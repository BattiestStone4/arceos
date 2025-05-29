use core::iter;

use alloc::sync::Arc;
use axhal::paging::{MappingFlags, PageSize, PageTable};
use memory_addr::{PAGE_SIZE_4K, PhysAddr, VirtAddr};

use super::{Backend, SharedPages, alloc::alloc_frame};

impl Backend {
    /// Creates a new allocation mapping backend.
    pub fn new_shared(page_num: usize, source: Option<Arc<SharedPages>>) -> Self {
        let pages = if let Some(source) = source {
            assert_eq!(source.len(), page_num);
            source
        } else {
            Arc::new(SharedPages(
                iter::repeat_with(|| alloc_frame(true).unwrap())
                    .take(page_num)
                    .collect(),
            ))
        };
        Self::Shared { pages }
    }

    pub(crate) fn map_shared(
        start: VirtAddr,
        pages: &[PhysAddr],
        flags: MappingFlags,
        pt: &mut PageTable,
    ) -> bool {
        debug!(
            "map_shared: [{:#x}, {:#x}) {:?}",
            start,
            start + pages.len() * PAGE_SIZE_4K,
            flags,
        );
        // allocate all possible physical frames for populated mapping.
        for (i, frame) in pages.iter().enumerate() {
            let addr = start + i * PAGE_SIZE_4K;
            if let Ok(tlb) = pt.map(addr, *frame, PageSize::Size4K, flags) {
                tlb.ignore(); // TLB flush on map is unnecessary, as there are no outdated mappings.
            } else {
                return false;
            }
        }
        true
    }

    pub(crate) fn unmap_shared(
        start: VirtAddr,
        pages: &Arc<SharedPages>,
        pt: &mut PageTable,
    ) -> bool {
        debug!(
            "unmap_shared: [{:#x}, {:#x})",
            start,
            start + pages.len() * PAGE_SIZE_4K
        );
        for i in 0..pages.len() {
            let addr = start + i * PAGE_SIZE_4K;
            if let Ok((_, page_size, tlb)) = pt.unmap(addr) {
                // Deallocate the physical frame if there is a mapping in the
                // page table.
                if page_size.is_huge() {
                    return false;
                }
                tlb.flush();
            } else {
                // Deallocation is needn't if the page is not mapped.
            }
        }
        true
    }
}
