//! Measurement prototype: OS backing beneath the existing per-agent block pool.
//! Linux uses one reserved range and persistent in-band descriptors.
//! Apple maps one tagged region per block and Windows one VirtualAlloc
//! reservation per block; they and the other targets keep the process-wide
//! registry. Other targets retain allocator backing.
//! Production in-band descriptors and byte-store ownership are in REGION-DESIGN.

use super::HeapGeneration;

pub(crate) mod reservation;

pub(crate) use reservation::contains;







pub(super) const ALIGN: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    NurseryBlock,
    OldBlock,
    /// O1: collectible old backing excluded from every relocation policy.
    CollectingHolder,
    LargeObject,
    #[allow(dead_code)] // Reserved for the byte-store integration slice.
    ByteStore,
    #[allow(dead_code)] // Reserved for native payload-region integration.
    Payload,
}

pub(super) fn kind_for(generation: HeapGeneration, len: usize) -> Kind {
    match generation {
        HeapGeneration::Old | HeapGeneration::Longlived if len > super::BLOCK_SIZE => {
            Kind::LargeObject
        }
        HeapGeneration::Old | HeapGeneration::Longlived => Kind::OldBlock,
        _ => Kind::NurseryBlock,
    }
}

fn mapped_len(len: usize) -> Option<usize> {
    if len == 0 || len % 4096 != 0 {
        return None;
    }
    len.checked_add(ALIGN - 1).map(|n| n & !(ALIGN - 1))
}

/// Charge the pool for the owned OS extent, including rounded tails. Other
/// targets retain the allocator backend and its exact requested length.
pub(super) fn backing_len(len: usize) -> usize {
    {
        mapped_len(len).expect("invalid region extent")
    }
}

/// Complete payload units are dense backing in every generation, including
/// refilled nursery blocks. Sub-unit extents and partial tails stay on base
/// pages. Advice precedes writes; neither prefault nor collapse.
pub(super) unsafe fn advise(data: *mut u8, len: usize, kind: Kind) {
    {
        let mapped = mapped_len(len).expect("invalid region extent");
        let _ = kind;
        // Fork: a program held to a memory allowance by its host is a small
        // one, and a huge page makes the first byte written to a 2 MB unit
        // cost 2 MB of resident memory. Measured on a counter app: 8 MB of
        // its 20 MB were four such pages, mostly unused. Base pages there.
        let dense = if crate::host::limit() != 0 {
            0
        } else {
            len / ALIGN * ALIGN
        };
        if dense != 0 {
            advise_thp(data.cast(), dense, libc::MADV_HUGEPAGE);
        }
        if dense < mapped {
            advise_thp(
                data.add(dense).cast(),
                mapped - dense,
                libc::MADV_NOHUGEPAGE,
            );
        }
    }
}

/// THP advice is a hint: kernels built without CONFIG_TRANSPARENT_HUGEPAGE
/// reject it with EINVAL and the mapping stays usable (#12863). Any other
/// failure still means a bad range.
unsafe fn advise_thp(data: *mut libc::c_void, len: usize, advice: libc::c_int) {
    if libc::madvise(data, len, advice) != 0 {
        let err = std::io::Error::last_os_error();
        assert_eq!(
            err.raw_os_error(),
            Some(libc::EINVAL),
            "madvise failed: {err}"
        );
    }
}

/// Fresh private anonymous backing, or null on refusal. This entry never
/// collects. The existing caller owns emergency reclaim outside arena borrows.
///
/// A GC block owns whole metadata pages: its start and its end are multiples of
/// `GENERATION_PAGE_SIZE`, on every target and for every [`Kind`]. The page is
/// the unit of old-gen metadata (live, dead and pinned bytes, the object index,
/// dirty slots) and the block is the unit of release, so each page belongs to
/// exactly one block and a page's base address finds it (old-page defrag and
/// idle compaction select by it). Blocks change generation and keep their
/// backing, so the rule covers every kind, not only large regions.
#[inline(always)]
pub(super) unsafe fn map(kind: Kind, len: usize) -> *mut u8 {
    let data = map_extent(kind, len);
    debug_assert!(
        data.is_null() || (data as usize | len) % super::GENERATION_PAGE_SIZE == 0,
        "a GC block owns whole metadata pages: {data:p} + {len:#x}"
    );
    data
}

#[inline(always)]
unsafe fn map_extent(kind: Kind, len: usize) -> *mut u8 {
    {
        let Some(mapped) = mapped_len(len) else {
            return std::ptr::null_mut();
        };
        let data = reservation::map(kind, mapped);
        if !data.is_null() {
            advise(data, len, kind);
        }
        data
    }
}



/// Reactivate released pages before they are handed out again.
#[inline]
pub(super) unsafe fn reuse(data: *mut u8, len: usize) {
    let _ = (data, len);
}


/// Physical destruction, used by pool overflow, pool drain and TLS teardown.
/// Callers remove page metadata and external owners before destruction.
pub(super) unsafe fn unmap(data: *mut u8, len: usize) {
    reservation::unmap(data, mapped_len(len).expect("invalid extent"));
    // Releasing the reservation frees committed and decommitted pages alike.
}

/// Matching backing for fixtures that seed the real region pool.
#[cfg(test)]
pub(crate) unsafe fn new_test_block_backing(size: usize) -> *mut u8 {
    map(Kind::NurseryBlock, size)
}



/// Every target, every kind: the backing `map` returns starts and ends on a
/// metadata page boundary (see [`map`]).
#[cfg(test)]
mod page_alignment_tests {
    use super::*;

    #[test]
    fn every_block_kind_owns_whole_metadata_pages() {
        let page = super::super::GENERATION_PAGE_SIZE;
        for kind in [
            Kind::NurseryBlock,
            Kind::OldBlock,
            Kind::CollectingHolder,
            Kind::LargeObject,
        ] {
            for len in [super::super::BLOCK_SIZE, 2 * super::super::BLOCK_SIZE] {
                unsafe {
                    let data = map(kind, len);
                    assert!(!data.is_null(), "{kind:?} backing of {len:#x}");
                    let start = data as usize;
                    unmap(data, len);
                    assert_eq!(start % page, 0, "{kind:?} block start {start:#x}");
                    assert_eq!((start + len) % page, 0, "{kind:?} block end");
                }
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn flags_for(data: *mut u8) -> String {
        let smaps = std::fs::read_to_string("/proc/self/smaps").unwrap();
        let mut found = false;
        for line in smaps.lines() {
            if let Some(range) = line.split_whitespace().next().filter(|v| v.contains('-')) {
                let (start, end) = range.split_once('-').unwrap();
                if let (Ok(start), Ok(end)) = (
                    usize::from_str_radix(start, 16),
                    usize::from_str_radix(end, 16),
                ) {
                    found = (start..end).contains(&(data as usize));
                }
            }
            if found && line.starts_with("VmFlags:") {
                return line.to_owned();
            }
        }
        panic!("region missing from smaps");
    }

    #[test]
    fn aligned_os_backing_has_whole_unit_thp_and_releases_pages() {
        // After `unmap`, the registry must no longer contain the range. The
        // kernel hands a just-unmapped range to the very next `mmap` in the
        // process, and every concurrently running GC test maps through this
        // same registry, so in a shared process the range can be legitimately
        // re-registered between the `unmap` and the check: only an
        // exclusively owned process can assert it.
        let Some(_isolated) = crate::test_support::isolated_process_guard() else {
            return;
        };
        unsafe {
            for kind in [Kind::NurseryBlock, Kind::OldBlock, Kind::LargeObject] {
                let data = map(kind, 2 * ALIGN);
                assert!(!data.is_null());
                assert_eq!(data as usize & (ALIGN - 1), 0);
                let flags = flags_for(data);
                let expected = "hg";
                assert!(flags.split_whitespace().any(|v| v == expected), "{flags}");
                std::ptr::write_bytes(data, 0xa5, 2 * ALIGN);
                let mut resident = vec![0u8; 2 * ALIGN / 4096];
                assert_eq!(
                    libc::mincore(data.cast(), 2 * ALIGN, resident.as_mut_ptr()),
                    0
                );
                assert!(resident.iter().all(|v| v & 1 == 1));
                assert_eq!(
                    libc::madvise(data.cast(), 2 * ALIGN, libc::MADV_DONTNEED),
                    0
                );
                assert_eq!(
                    libc::mincore(data.cast(), 2 * ALIGN, resident.as_mut_ptr()),
                    0
                );
                assert!(resident.iter().all(|v| v & 1 == 0));
                *data = 42;
                assert_eq!(*data, 42);
                unmap(data, 2 * ALIGN);
                assert!(!contains(data as usize, 4096));
                assert_eq!(libc::mincore(data.cast(), 4096, resident.as_mut_ptr()), 0);
                assert_eq!(resident[0] & 1, 0);
            }
        }
    }

    #[test]
    fn reused_whole_region_keeps_dense_advice_in_nursery() {
        unsafe {
            let data = map(Kind::OldBlock, ALIGN);
            advise(data, ALIGN, Kind::NurseryBlock);
            assert!(flags_for(data).split_whitespace().any(|v| v == "hg"));
            unmap(data, ALIGN);
        }
    }

    #[test]
    fn invalid_extents_are_refused_without_collection() {
        unsafe {
            assert!(map(Kind::NurseryBlock, 0).is_null());
            assert!(map(Kind::NurseryBlock, ALIGN - 1).is_null());
            assert!(map(Kind::NurseryBlock, usize::MAX & !(ALIGN - 1)).is_null());
        }
    }

    #[test]
    fn dense_nursery_advice_survives_speculative_and_committed_promotion() {
        crate::arena::tests::run_with_fresh_arenas(|| {
            let user = crate::arena::arena_alloc_gc(24, 8, crate::gc::GC_TYPE_STRING);
            let base = (user as usize & !(ALIGN - 1)) as *mut u8;
            assert!(flags_for(base).split_whitespace().any(|v| v == "hg"));
            let speculative = crate::arena::retag_young_for_in_place_promotion(true);
            assert!(
                !speculative.is_empty(),
                "LIVE SUBJECT: retagged nursery block"
            );
            assert!(flags_for(base).split_whitespace().any(|v| v == "hg"));
            crate::arena::undo_in_place_promotion_retag(&speculative);
            assert!(flags_for(base).split_whitespace().any(|v| v == "hg"));
            let promotion = crate::arena::retag_young_for_in_place_promotion(false);
            let stats = crate::arena::finish_in_place_promotion(
                promotion,
                crate::arena::PromotionLiveness::AssumeAllLive,
            );
            assert!(stats.objects > 0);
            assert!(crate::arena::pointer_in_old_gen(user as usize));
            assert!(flags_for(base).split_whitespace().any(|v| v == "hg"));
        });
    }

    #[test]
    fn mature_complete_units_keep_partial_tails_on_base_pages() {
        unsafe {
            let len = ALIGN + 4096;
            let data = map(Kind::LargeObject, len);
            assert!(!data.is_null());
            assert!(flags_for(data).split_whitespace().any(|v| v == "hg"));
            assert!(flags_for(data.add(ALIGN))
                .split_whitespace()
                .any(|v| v == "nh"));
            unmap(data, len);
        }
    }

    #[test]
    fn nursery_whole_units_are_huge_but_small_extents_and_tails_are_not() {
        unsafe {
            for len in [4096, ALIGN - 4096, ALIGN, ALIGN + 4096, 2 * ALIGN + 4096] {
                let data = map(Kind::NurseryBlock, len);
                assert!(!data.is_null(), "LIVE SUBJECT: mapped nursery backing");
                let dense = len / ALIGN * ALIGN;
                for offset in (0..backing_len(len)).step_by(4096) {
                    // The kernel's VMA advice is the subject, independently of
                    // whether THP allocation succeeds on this machine.
                    let flags = flags_for(data.add(offset));
                    let expected = if offset < dense { "hg" } else { "nh" };
                    assert!(
                        flags.split_whitespace().any(|v| v == expected),
                        "len={len} offset={offset} expected={expected}: {flags}"
                    );
                }
                unmap(data, len);
            }
        }
    }

    fn page() -> *mut libc::c_void {
        let page = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                4096,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(page, libc::MAP_FAILED);
        page
    }

    #[test]
    fn thp_advice_tolerates_einval() {
        // An unaligned start is EINVAL on every kernel, the same errno a
        // kernel without THP returns for the advice itself.
        let page = page();
        unsafe {
            advise_thp(page.cast::<u8>().add(1).cast(), 1, libc::MADV_HUGEPAGE);
            advise_thp(page.cast::<u8>().add(1).cast(), 1, libc::MADV_NOHUGEPAGE);
            libc::munmap(page, 4096);
        }
    }

    #[test]
    #[should_panic(expected = "madvise failed")]
    fn thp_advice_still_rejects_an_unmapped_range() {
        let page = page();
        unsafe {
            libc::munmap(page, 4096);
            // MADV_NORMAL keeps this ENOMEM on kernels without THP too.
            advise_thp(page, 4096, libc::MADV_NORMAL);
        }
    }
}


