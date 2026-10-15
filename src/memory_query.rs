//! Current-operation memory protection checks. Never caches object data or
//! ownership, never keeps region permissions across a native call/frame.
use std::{
    cell::{Cell, RefCell},
    sync::OnceLock,
};
use windows::Win32::System::{
    Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery},
    ProcessStatus::{K32QueryWorkingSetEx, PSAPI_WORKING_SET_EX_INFORMATION},
    SystemInformation::{GetSystemInfo, SYSTEM_INFO},
    Threading::GetCurrentProcess,
};

thread_local! {
    static QUERIES: Cell<u64> = const { Cell::new(0) };
    static CACHE: RefCell<Cache> = const { RefCell::new(Cache {
        depth: 0, entries: [None; 128], next: 0, pages: [None; 256]
    }) };
}

#[derive(Clone, Copy)]
struct Region {
    start: usize,
    end: usize,
    readable: bool,
    writable: bool,
}

struct Cache {
    depth: usize,
    entries: [Option<Region>; 128],
    next: usize,
    pages: [Option<Region>; 256],
}

impl Cache {
    fn clear(&mut self) {
        self.entries.fill(None);
        self.pages.fill(None);
        self.next = 0;
    }
}

struct ScopeGuard;
impl Drop for ScopeGuard {
    fn drop(&mut self) {
        CACHE.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.clear();
            cache.depth -= 1;
        });
    }
}

pub fn query_count() -> u64 {
    QUERIES.with(Cell::get)
}

/// Validate only the requested bytes; callers needing full region boundaries
/// must use accessible_region instead.
pub fn accessible_span(address: usize, length: usize, write: bool) -> bool {
    let Some(end) = address
        .checked_add(length)
        .filter(|_| address != 0 && length != 0)
    else {
        return false;
    };
    static PAGE_SIZE: OnceLock<usize> = OnceLock::new();
    let page_size = *PAGE_SIZE.get_or_init(|| {
        let mut info = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut info) };
        info.dwPageSize as usize
    });
    if !page_size.is_power_of_two() {
        return accessible_region(address, length, write).is_some();
    }
    let first = address & !(page_size - 1);
    let pages = (end - 1 - first) / page_size + 1;
    // Keep temporary storage and query work bounded. Full-region callers and
    // very large buffers retain the original VirtualQuery path.
    if pages > 64 {
        return accessible_region(address, length, write).is_some();
    }
    let cached = CACHE.with(|cache| {
        let cache = cache.borrow();
        if cache.depth == 0 {
            return None;
        }
        if let Some(region) = cache
            .entries
            .iter()
            .flatten()
            .find(|r| address >= r.start && end <= r.end)
        {
            return Some(if write {
                region.writable
            } else {
                region.readable
            });
        }
        for i in 0..pages {
            let start = first + i * page_size;
            let region = cache.pages[(start / page_size) % cache.pages.len()]
                .filter(|r| r.start == start)?;
            if !(if write {
                region.writable
            } else {
                region.readable
            }) {
                return Some(false);
            }
        }
        Some(true)
    });
    if let Some(allowed) = cached {
        return allowed;
    }
    let mut info = [PSAPI_WORKING_SET_EX_INFORMATION::default(); 64];
    for (i, page) in info[..pages].iter_mut().enumerate() {
        page.VirtualAddress = (first + i * page_size) as *mut _;
    }
    QUERIES.with(|n| n.set(n.get().saturating_add(1)));
    let ok = unsafe {
        K32QueryWorkingSetEx(
            GetCurrentProcess(),
            info.as_mut_ptr().cast(),
            (pages * size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32,
        )
    }
    .as_bool();
    if !ok
        || info[..pages].iter().any(|page| unsafe {
            page.VirtualAttributes.Flags & 1 == 0 || page.VirtualAttributes.Flags & (1 << 31) != 0
        })
    {
        // Nonresident/guard/unmapped pages must not be touched to fault them in.
        // Reuse the original committed/protection checks without altering them.
        return accessible_region(address, length, write).is_some();
    }
    let mut allowed = true;
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        for page in &info[..pages] {
            // Documented PSAPI_WORKING_SET_EX_BLOCK: Valid bit 0, Win32Protection
            // bits 4..14. Protection is meaningful only after Valid was checked.
            let protection = (unsafe { page.VirtualAttributes.Flags } >> 4) & 0x7ff;
            let (readable, writable) = permissions(protection as u32);
            allowed &= if write { writable } else { readable };
            if cache.depth > 0 {
                let start = page.VirtualAddress as usize;
                let index = (start / page_size) % cache.pages.len();
                cache.pages[index] = Some(Region {
                    start,
                    end: start + page_size,
                    readable,
                    writable,
                });
            }
        }
    });
    allowed
}

fn permissions(protection: u32) -> (bool, bool) {
    if protection & 0x100 != 0 {
        return (false, false);
    }
    let protection = protection & 0xff;
    (
        matches!(protection, 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x40 | 0x80),
        matches!(protection, 0x04 | 0x08 | 0x40 | 0x80),
    )
}

/// The operation must not invoke native game code or change target page
/// mappings. Each nested operation starts fresh, and invalidates its parent on
/// return too. Data/identity reads are never cached. This is not a lifetime lock:
/// engine phase/ownership checks remain responsible for object lifetime, just
/// as with the original check-then-dereference path.
pub fn scoped<T>(operation: impl FnOnce() -> T) -> T {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.clear();
        cache.depth += 1;
    });
    let _guard = ScopeGuard;
    operation()
}

pub fn accessible_region(address: usize, length: usize, write: bool) -> Option<(usize, usize)> {
    if address == 0 || length == 0 {
        return None;
    }
    let end = address.checked_add(length)?;
    if let Some(region) = CACHE.with(|cache| {
        let cache = cache.borrow();
        if cache.depth == 0 {
            return None;
        }
        cache
            .entries
            .iter()
            .flatten()
            .copied()
            .find(|r| address >= r.start && end <= r.end)
    }) {
        return (if write {
            region.writable
        } else {
            region.readable
        })
        .then_some((region.start, region.end));
    }
    let mut info = MEMORY_BASIC_INFORMATION::default();
    QUERIES.with(|n| n.set(n.get().saturating_add(1)));
    let result = unsafe {
        VirtualQuery(
            Some(address as *const _),
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if result == 0 || info.State != MEM_COMMIT || info.Protect.0 & 0x100 != 0 {
        return None;
    }
    let (readable, writable) = permissions(info.Protect.0);
    let start = info.BaseAddress as usize;
    let stop = start.checked_add(info.RegionSize)?;
    if address < start || end > stop {
        return None;
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.depth > 0 {
            let index = cache.next;
            cache.entries[index] = Some(Region {
                start,
                end: stop,
                readable,
                writable,
            });
            cache.next = (index + 1) % cache.entries.len();
        }
    });
    (if write { writable } else { readable }).then_some((start, stop))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Memory::{
        MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_GUARD, PAGE_PROTECTION_FLAGS, PAGE_READONLY,
        PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
    };
    struct Allocation(*mut std::ffi::c_void);
    impl Allocation {
        fn new() -> Self {
            let ptr =
                unsafe { VirtualAlloc(None, 0x3000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
            assert!(!ptr.is_null());
            Self(ptr)
        }
        fn address(&self) -> usize {
            self.0 as usize
        }
    }
    impl Drop for Allocation {
        fn drop(&mut self) {
            unsafe { VirtualFree(self.0, 0, MEM_RELEASE).unwrap() };
        }
    }
    #[test]
    fn page_checks_reject_guard_readonly_and_decommitted_middle_pages() {
        let allocation = Allocation::new();
        let address = allocation.address();
        for offset in [0, 0x1000, 0x2000] {
            unsafe { ((address + offset) as *mut u8).write_volatile(3) };
        }
        let mut old = PAGE_PROTECTION_FLAGS::default();
        scoped(|| assert!(accessible_span(address + 0xfff, 0x1002, true)));
        unsafe {
            VirtualProtect(
                (address + 0x1000) as *const _,
                0x1000,
                PAGE_READONLY,
                &mut old,
            )
            .unwrap()
        };
        scoped(|| {
            assert!(accessible_span(address + 0xfff, 0x1002, false));
            assert!(!accessible_span(address + 0xfff, 0x1002, true));
            assert!(accessible_span(address, 4, true));
        });
        unsafe {
            VirtualProtect(
                (address + 0x1000) as *const _,
                0x1000,
                PAGE_READWRITE | PAGE_GUARD,
                &mut old,
            )
            .unwrap()
        };
        scoped(|| assert!(!accessible_span(address + 0xfff, 0x1002, false)));
        let mut info = MEMORY_BASIC_INFORMATION::default();
        unsafe {
            VirtualQuery(
                Some((address + 0x1000) as *const _),
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        assert_ne!(
            info.Protect.0 & PAGE_GUARD.0,
            0,
            "permission queries consumed the guard page"
        );
        unsafe {
            VirtualProtect(
                (address + 0x1000) as *const _,
                0x1000,
                PAGE_READWRITE,
                &mut old,
            )
            .unwrap()
        };
        unsafe { VirtualFree((address + 0x1000) as *mut _, 0x1000, MEM_DECOMMIT).unwrap() };
        scoped(|| {
            assert!(accessible_span(address, 4, true));
            assert!(accessible_span(address + 0x2000, 4, true));
            assert!(!accessible_span(address + 0xfff, 0x1002, false));
            assert!(!accessible_span(usize::MAX - 1, 4, false));
            assert!(!accessible_span(0, 4, false));
            assert!(!accessible_span(address, 0, false));
        });
    }

    #[test]
    fn page_checks_fallback_without_faulting_and_invalidate_at_each_operation() {
        let allocation = Allocation::new();
        let address = allocation.address();
        let attributes = || {
            let mut info = PSAPI_WORKING_SET_EX_INFORMATION {
                VirtualAddress: allocation.0,
                ..Default::default()
            };
            assert!(
                unsafe {
                    K32QueryWorkingSetEx(
                        GetCurrentProcess(),
                        (&mut info as *mut PSAPI_WORKING_SET_EX_INFORMATION).cast(),
                        size_of::<PSAPI_WORKING_SET_EX_INFORMATION>() as u32,
                    )
                }
                .as_bool()
            );
            unsafe { info.VirtualAttributes.Flags }
        };
        assert_eq!(attributes() & 1, 0);
        scoped(|| assert!(accessible_span(address, 4, true)));
        assert_eq!(
            attributes() & 1,
            0,
            "fallback must not touch an unresident page"
        );
        unsafe { (address as *mut u32).write_volatile(7) };
        let before = query_count();
        scoped(|| {
            for _ in 0..1000 {
                assert!(accessible_span(address, 4, true));
            }
            unsafe { (address as *mut u32).write_volatile(9) };
            assert_eq!(unsafe { (address as *const u32).read_volatile() }, 9);
            scoped(|| assert!(accessible_span(address, 4, true)));
            assert!(accessible_span(address, 4, true));
        });
        assert_eq!(query_count() - before, 3);
        assert!(
            std::panic::catch_unwind(|| scoped(|| {
                assert!(accessible_span(address, 4, true));
                panic!("fixture");
            }))
            .is_err()
        );
        assert_eq!(CACHE.with(|c| c.borrow().depth), 0);
        let mut old = PAGE_PROTECTION_FLAGS::default();
        unsafe { VirtualProtect(allocation.0, 0x1000, PAGE_READONLY, &mut old).unwrap() };
        assert!(!accessible_span(address, 4, true));
        assert!(accessible_span(address, 4, false));
        scoped(|| {
            assert!(accessible_span(address, 4, false));
            std::thread::spawn(move || {
                let before = query_count();
                assert!(accessible_span(address, 4, false));
                assert_eq!(query_count() - before, 1);
            })
            .join()
            .unwrap();
        });
    }
    #[test]
    fn resident_metadata_query_cost_is_independent_of_surrounding_heap() {
        let size = 128 * 1024 * 1024;
        let ptr = unsafe { VirtualAlloc(None, size, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        assert!(!ptr.is_null());
        let allocation = Allocation(ptr);
        // A resident large region reproduces the live game, unlike the small
        // heap fixtures which made VirtualQuery appear effectively constant.
        for page in (0..size).step_by(4096) {
            unsafe { (ptr.cast::<u8>().add(page)).write_volatile(0) };
        }
        let address = allocation.address() + 64;
        let measure = |fast| {
            let clock = std::time::Instant::now();
            for _ in 0..128 {
                scoped(|| {
                    assert!(if fast {
                        accessible_span(address, 8, false)
                    } else {
                        accessible_region(address, 8, false).is_some()
                    });
                });
            }
            clock.elapsed()
        };
        let full = measure(false);
        let requested = measure(true);
        println!(
            "RESIDENT-HEAP iterations=128 full_us={} requested_us={}",
            full.as_micros(),
            requested.as_micros()
        );
        assert!(
            requested * 4 < full,
            "metadata checks still pay for the surrounding 128 MiB region"
        );
    }
    #[test]
    fn deduplicates_queries_but_never_field_reads_or_operation_epochs() {
        let allocation = Allocation::new();
        let address = allocation.address();
        let before = query_count();
        scoped(|| {
            for i in 0..1000 {
                assert!(accessible_region(address + i, 4, i % 2 == 0).is_some());
            }
            unsafe { (address as *mut u32).write(7) };
            assert!(accessible_region(address, 4, false).is_some());
            assert_eq!(unsafe { (address as *const u32).read() }, 7);
        });
        assert_eq!(query_count() - before, 1);
        scoped(|| assert!(accessible_region(address, 4, true).is_some()));
        assert_eq!(query_count() - before, 2);
        assert!(accessible_region(address, 4, true).is_some());
        assert!(accessible_region(address, 4, true).is_some());
        assert_eq!(query_count() - before, 4);
    }
    #[test]
    fn nested_operation_and_unwind_discard_all_permissions() {
        let allocation = Allocation::new();
        let address = allocation.address();
        let before = query_count();
        scoped(|| {
            assert!(accessible_region(address, 4, false).is_some());
            scoped(|| assert!(accessible_region(address, 4, false).is_some()));
            assert!(accessible_region(address, 4, false).is_some());
        });
        assert_eq!(query_count() - before, 3);
        assert!(std::panic::catch_unwind(|| scoped(|| panic!("fixture"))).is_err());
        assert_eq!(CACHE.with(|c| c.borrow().depth), 0);
    }
    #[test]
    fn readonly_guard_decommit_gap_and_overflow_still_fail_closed() {
        let allocation = Allocation::new();
        let address = allocation.address();
        scoped(|| assert!(accessible_region(address, 0x3000, true).is_some()));
        let mut old = PAGE_PROTECTION_FLAGS::default();
        unsafe { VirtualProtect(allocation.0, 0x1000, PAGE_READONLY, &mut old).unwrap() };
        scoped(|| {
            assert!(accessible_region(address, 4, false).is_some());
            assert!(accessible_region(address, 4, true).is_none());
        });
        unsafe {
            VirtualProtect(allocation.0, 0x1000, PAGE_READWRITE | PAGE_GUARD, &mut old).unwrap()
        };
        scoped(|| assert!(accessible_region(address, 4, false).is_none()));
        unsafe { VirtualProtect(allocation.0, 0x1000, PAGE_READWRITE, &mut old).unwrap() };
        unsafe { VirtualFree((address + 0x1000) as *mut _, 0x1000, MEM_DECOMMIT).unwrap() };
        scoped(|| {
            assert!(accessible_region(address, 4, true).is_some());
            assert!(accessible_region(address + 0x1000, 4, false).is_none());
            assert!(accessible_region(address + 0x2000, 4, true).is_some());
            assert!(accessible_region(address, 0x3000, true).is_none());
            assert!(accessible_region(usize::MAX - 1, 4, false).is_none());
            assert!(accessible_region(0, 4, false).is_none());
            assert!(accessible_region(address, 0, false).is_none());
        });
    }
    #[test]
    fn cache_is_thread_local() {
        let allocation = Allocation::new();
        let address = allocation.address();
        scoped(|| {
            assert!(accessible_region(address, 4, false).is_some());
            std::thread::spawn(move || {
                let before = query_count();
                assert!(accessible_region(address, 4, false).is_some());
                assert_eq!(query_count() - before, 1);
            })
            .join()
            .unwrap();
        });
    }
}
