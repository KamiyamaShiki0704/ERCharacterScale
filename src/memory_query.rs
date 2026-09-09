//! Current-operation memory protection checks. Never caches object data or
//! ownership, never keeps region permissions across a native call/frame.
use std::cell::{Cell, RefCell};
use windows::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};

thread_local! {
    static QUERIES: Cell<u64> = const { Cell::new(0) };
    static CACHE: RefCell<Cache> = const { RefCell::new(Cache { depth: 0, entries: [None; 128], next: 0 }) };
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
}

impl Cache {
    fn clear(&mut self) {
        self.entries.fill(None);
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
    let protection = info.Protect.0 & 0xFF;
    let writable = matches!(protection, 0x04 | 0x08 | 0x40 | 0x80);
    let readable = matches!(protection, 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x40 | 0x80);
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
