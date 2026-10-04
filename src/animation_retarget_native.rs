//! WW2.7.1.0 hkb clip track scatter hook. No decompression, disk IO or blocking
//! asset work in the callback. Only verified local-unit biped skeleton identities
//! are registered. Binding maps and original sampled tracks are never mutated.
use crate::{
    animation_retarget::ClipPlan, animation_skeleton, equipment_retarget::Bone,
    equipment_retarget_pose as pose,
};
use ilhook::x64::{CallbackOption, HookFlags, Registers, hook_closure_retn};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
    },
};

#[path = "animation_retarget_motion.rs"]
mod motion;

const RVA: usize = 0x14A2F80;
const GUARD: &[u8] = &[
    0x48, 0x89, 0x54, 0x24, 0x10, 0x89, 0x4c, 0x24, 0x08, 0x41, 0x56, 0x48, 0x81, 0xec, 0x80, 0, 0,
    0,
];
static BASE: AtomicUsize = AtomicUsize::new(0);
struct Target {
    name: String,
    bones: Vec<Bone>,
    active: AtomicBool,
}
#[derive(Clone)]
struct Registered {
    header: [u8; 0x50],
    target: Arc<Target>,
}
static TARGETS: RwLock<Option<HashMap<usize, Registered>>> = RwLock::new(None);
fn target(metadata: usize) -> Option<Arc<Target>> {
    let registry = TARGETS.read().ok()?;
    let entry = registry.as_ref()?.get(&metadata)?;
    entry
        .target
        .active
        .load(Ordering::Acquire)
        .then(|| entry.target.clone())
}
static GENERATION: AtomicU64 = AtomicU64::new(0);
enum Source {
    Pending,
    Failed,
    Ready(Arc<Vec<Bone>>),
}
type Sources = HashMap<String, Source>;
static SOURCES: Mutex<Option<Sources>> = Mutex::new(None);
static LOADER: OnceLock<SyncSender<String>> = OnceLock::new();
// Read-only diagnostic consumers can inspect these counters without enabling
// logging or sampling the render thread. No file output or runtime allocation.
// foreign, unsupported, waiting, source_failed, plan_failed, sample_failed,
// identity_changed, applied.
#[unsafe(no_mangle)]
pub static ERCS_ANIMATION_DIAGNOSTICS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
pub(crate) static APPLIED: AtomicU64 = AtomicU64::new(0);
pub(crate) static WAITING: AtomicU64 = AtomicU64::new(0);
pub(crate) static REJECTED: AtomicU64 = AtomicU64::new(0);

fn read(at: usize, out: &mut [u8]) -> bool {
    if !crate::memory_query::accessible_span(at, out.len(), false) {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(at as *const u8, out.as_mut_ptr(), out.len());
    }
    true
}
fn ptr(at: usize) -> Option<usize> {
    pose::pointer(&read, at)
}

pub(crate) fn install(base: usize) -> bool {
    if BASE.load(Ordering::Acquire) != 0 {
        return true;
    }
    let mut guard = vec![0; GUARD.len()];
    if !read(base + RVA, &mut guard) || guard != GUARD {
        return false;
    }
    if !motion::guard(base) {
        return false;
    }
    let Some(directory) = crate::log::sibling_path("skeletons") else {
        return false;
    };
    let Some(game) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_owned()))
    else {
        return false;
    };
    let (sender, receiver) = mpsc::sync_channel::<String>(32);
    if std::thread::Builder::new()
        .name("ERCS source skeletons".into())
        .spawn(move || {
            while let Ok(name) = receiver.recv() {
                let bones = animation_skeleton::load(&name, &directory, &game)
                    .map_or(Source::Failed, |b| Source::Ready(Arc::new(b)));
                if let Ok(mut sources) = SOURCES.lock() {
                    sources.get_or_insert_with(HashMap::new).insert(name, bones);
                }
            }
        })
        .is_err()
    {
        return false;
    }
    let _ = LOADER.set(sender);
    let Ok(hook) =
        (unsafe { hook_closure_retn(base + RVA, hook, CallbackOption::None, HookFlags::empty()) })
    else {
        return false;
    };
    let Ok(motion_hook) = (unsafe { motion::install(base) }) else {
        return false;
    };
    let _ = Box::leak(Box::new(motion_hook));
    let _ = Box::leak(Box::new(hook));
    BASE.store(base, Ordering::Release);
    true
}

// Called once in the existing pre-physics unit enumeration. Include units
// with scale 1.0; animation registration is independent of matching scale rules.
pub(crate) fn refresh(units: &[crate::unit_runtime::Identity]) {
    if BASE.load(Ordering::Acquire) == 0 {
        return;
    }
    crate::memory_query::scoped(|| {
        // Parse new skeletons outside the published registry lock. Hot hooks
        // must never fall through to foreign native indices during registration.
        let mut registry = match TARGETS.read() {
            Ok(r) => r.clone().unwrap_or_default(),
            Err(_) => return,
        };
        let mut changed = false;
        let mut seen = std::collections::HashSet::new();
        for unit in units {
            let Some(metadata) = ptr(unit.pose + 0x48).filter(|&p| p != 0) else {
                continue;
            };
            let name = format!("c{:04}", unit.character_id);
            let Some(header) = pose::bytes::<0x50>(&read, metadata) else {
                continue;
            };
            if let Some(old) = registry.get(&metadata) {
                if old.target.name == name && old.header == header {
                    seen.insert(metadata);
                    continue;
                }
                old.target.active.store(false, Ordering::Release);
            }
            changed = true;
            registry.remove(&metadata);
            if registry.len() >= 512 {
                continue;
            }
            let Some((_, bones)) = pose::skeleton(&read, metadata) else {
                continue;
            };
            let supported = crate::animation_retarget::biped(&bones);
            registry.insert(
                metadata,
                Registered {
                    header,
                    target: Arc::new(Target {
                        name,
                        bones: crate::equipment_retarget::animation_reference(bones),
                        active: AtomicBool::new(supported),
                    }),
                },
            );
            seen.insert(metadata);
        }
        registry.retain(|metadata, entry| {
            let keep = seen.contains(metadata);
            if !keep {
                changed = true;
                entry.target.active.store(false, Ordering::Release);
            }
            keep
        });
        if changed && let Ok(mut published) = TARGETS.write() {
            *published = Some(registry);
        }
    });
}
pub(crate) fn suspend() {
    if let Ok(mut registry) = TARGETS.write()
        && let Some(entries) = registry.take()
    {
        for entry in entries.into_values() {
            entry.target.active.store(false, Ordering::Release);
        }
    }
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

struct Binding {
    target: Arc<Target>,
    header: [u8; 0x60],
    name: String,
    plan: Option<ClipPlan>,
    rejected: bool,
}
#[derive(Default)]
struct Cache {
    generation: u64,
    bindings: HashMap<(usize, usize), Binding>,
    scratch: Vec<u8>,
}
thread_local! { static CACHE: RefCell<Cache> = RefCell::new(Cache::default()); }

// Thirteen arguments, including narrow values in their own eight-byte stack
// slots. Kept explicit so ABI regression tests execute the actual trampoline.
type Native = unsafe extern "C" fn(
    i32,
    usize,
    usize,
    usize,
    usize,
    i16,
    usize,
    usize,
    u8,
    usize,
    usize,
    usize,
    u8,
);
#[derive(Clone, Copy)]
struct Args {
    count: i32,
    samples: usize,
    output: usize,
    indices: usize,
    partitions: usize,
    partition_count: i16,
    mapper: usize,
    reference: usize,
    additive: u8,
    skeleton: usize,
    mirrored: usize,
    mask: usize,
    mirror: u8,
}
impl Args {
    unsafe fn call(self, native: Native) {
        unsafe {
            native(
                self.count,
                self.samples,
                self.output,
                self.indices,
                self.partitions,
                self.partition_count,
                self.mapper,
                self.reference,
                self.additive,
                self.skeleton,
                self.mirrored,
                self.mask,
                self.mirror,
            );
        }
    }
    fn neutral(mut self) -> Self {
        self.count = 0;
        self.partition_count = 0;
        self.partitions = 0;
        self.mapper = 0;
        self
    }
}
fn hook(registers: *mut Registers, original: usize) -> usize {
    let r = unsafe { &*registers };
    // This is the original live call stack, guaranteed by the function ABI.
    let stack =
        |offset| unsafe { std::ptr::read_unaligned((r.rsp as usize + offset) as *const usize) };
    let args = Args {
        count: r.rcx as i32,
        samples: r.rdx as usize,
        output: r.r8 as usize,
        indices: r.r9 as usize,
        partitions: stack(0x28),
        partition_count: stack(0x30) as i16,
        mapper: stack(0x38),
        reference: stack(0x40),
        additive: stack(0x48) as u8,
        skeleton: stack(0x50),
        mirrored: stack(0x58),
        mask: stack(0x60),
        mirror: stack(0x68) as u8,
    };
    let native: Native = unsafe { std::mem::transmute(original) };
    let mut called = false;
    if args.skeleton != 0 && args.mapper == 0 && target(args.skeleton).is_some() {
        crate::memory_query::scoped(|| {
            CACHE.with(|cache| {
                if let Ok(mut cache) = cache.try_borrow_mut()
                    && let Some(corrected) = prepare(args, &mut cache)
                {
                    unsafe {
                        corrected.call(native);
                    }
                    called = true;
                }
            });
        });
    }
    if !called {
        unsafe {
            args.call(native);
        }
    }
    0
}

fn prepare(args: Args, cache: &mut Cache) -> Option<Args> {
    let target = target(args.skeleton)?;
    let generation = GENERATION.load(Ordering::Acquire);
    if cache.generation != generation {
        cache.bindings.clear();
        cache.generation = generation;
    }
    let binding = args.indices.checked_sub(0x28)?;
    let key = (args.skeleton, binding);
    let header: [u8; 0x60] = pose::bytes(&read, binding)?;
    let word = |at| usize::from_le_bytes(header[at..at + 8].try_into().unwrap());
    if word(0) != BASE.load(Ordering::Acquire) + 0x2D4D2B0 {
        return None;
    }
    if cache
        .bindings
        .get(&key)
        .is_none_or(|b| b.header != header || !Arc::ptr_eq(&b.target, &target))
    {
        if cache.bindings.len() >= 256 {
            cache
                .bindings
                .retain(|_, b| b.target.active.load(Ordering::Acquire));
        }
        if cache.bindings.len() >= 256 {
            cache.bindings.clear();
        }
        let name = pose::text(&read, word(0x18) & !1, false)?;
        let foreign = animation_skeleton::source_name(&name) && name != target.name;
        cache.bindings.insert(
            key,
            Binding {
                target: target.clone(),
                header,
                name,
                plan: None,
                rejected: !foreign,
            },
        );
    }
    let entry = cache.bindings.get_mut(&key)?;
    if !animation_skeleton::source_name(&entry.name) || entry.name == target.name {
        return None;
    }
    ERCS_ANIMATION_DIAGNOSTICS[0].fetch_add(1, Ordering::Relaxed);
    // Refuse additive, mirrored, partitioned and incomplete LOD clips rather
    // than let foreign indices write a visibly unrelated target bone.
    let native_tracks = i32::from_le_bytes(pose::bytes(&read, word(0x20) + 0x20)?);
    let map_count = i32::from_le_bytes(header[0x30..0x34].try_into().ok()?);
    if args.additive != 0
        || header[0x58] != 0
        || args.mirrored != 0
        || args.partition_count != 0
        || args.count <= 0
        || args.count != native_tracks
        || args.count as usize > pose::LIMIT
        || map_count != args.count && map_count != 0
    {
        ERCS_ANIMATION_DIAGNOSTICS[1].fetch_add(1, Ordering::Relaxed);
        REJECTED.fetch_add(1, Ordering::Relaxed);
        return Some(args.neutral());
    }
    if entry.rejected {
        return Some(args.neutral());
    }
    if entry.plan.is_none() {
        let Ok(mut sources) = SOURCES.try_lock() else {
            ERCS_ANIMATION_DIAGNOSTICS[2].fetch_add(1, Ordering::Relaxed);
            return Some(args.neutral());
        };
        let sources = sources.get_or_insert_with(HashMap::new);
        match sources.get(&entry.name) {
            None => {
                if sources.len() < 32 && LOADER.get()?.try_send(entry.name.clone()).is_ok() {
                    sources.insert(entry.name.clone(), Source::Pending);
                }
                ERCS_ANIMATION_DIAGNOSTICS[2].fetch_add(1, Ordering::Relaxed);
                WAITING.fetch_add(1, Ordering::Relaxed);
                return Some(args.neutral());
            }
            Some(Source::Pending) => {
                ERCS_ANIMATION_DIAGNOSTICS[2].fetch_add(1, Ordering::Relaxed);
                WAITING.fetch_add(1, Ordering::Relaxed);
                return Some(args.neutral());
            }
            Some(Source::Failed) => {
                ERCS_ANIMATION_DIAGNOSTICS[3].fetch_add(1, Ordering::Relaxed);
                entry.rejected = true;
                REJECTED.fetch_add(1, Ordering::Relaxed);
                return Some(args.neutral());
            }
            Some(Source::Ready(source)) => {
                let tracks = if map_count == 0 {
                    (0..args.count).map(|i| i as i16).collect()
                } else {
                    let mut raw = vec![0; args.count as usize * 2];
                    if !read(word(0x28), &mut raw) {
                        return Some(args.neutral());
                    }
                    raw.chunks_exact(2)
                        .map(|s| i16::from_le_bytes(s.try_into().unwrap()))
                        .collect()
                };
                entry.plan = ClipPlan::new(source.as_ref().clone(), target.bones.clone(), tracks);
                entry.rejected = entry.plan.is_none();
                if entry.rejected {
                    ERCS_ANIMATION_DIAGNOSTICS[4].fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
    let Some(plan) = entry.plan.as_mut() else {
        return Some(args.neutral());
    };
    cache.scratch.resize(args.count as usize * 48, 0);
    if !read(args.samples, &mut cache.scratch) || plan.prepare(&cache.scratch).is_none() {
        ERCS_ANIMATION_DIAGNOSTICS[5].fetch_add(1, Ordering::Relaxed);
        return Some(args.neutral());
    }
    if !target.active.load(Ordering::Acquire) || GENERATION.load(Ordering::Acquire) != generation {
        ERCS_ANIMATION_DIAGNOSTICS[6].fetch_add(1, Ordering::Relaxed);
        return Some(args.neutral());
    }
    // hkArray header lives through the native call in the TLS Binding. The
    // actual header is owned by the wrapper below, not a temporary stack value.
    cache_array(plan, args)
}

#[repr(C)]
struct Array {
    data: usize,
    count: i32,
    capacity: i32,
}
thread_local! { static ARRAY: RefCell<Array> = const { RefCell::new(Array { data:0, count:0, capacity:0 }) }; }
fn cache_array(plan: &mut ClipPlan, mut args: Args) -> Option<Args> {
    ARRAY.with(|a| {
        let mut a = a.try_borrow_mut().ok()?;
        a.data = plan.indices.as_ptr() as usize;
        a.count = plan.indices.len() as i32;
        a.capacity = a.count | i32::MIN;
        args.indices = &*a as *const Array as usize;
        args.samples = plan.sampled.as_ptr() as usize;
        args.count = plan.indices.len() as i32;
        args.partitions = 0;
        args.partition_count = 0;
        ERCS_ANIMATION_DIAGNOSTICS[7].fetch_add(1, Ordering::Relaxed);
        APPLIED.fetch_add(1, Ordering::Relaxed);
        Some(args)
    })
}

#[cfg(test)]
#[path = "animation_retarget_native_tests.rs"]
mod tests;
