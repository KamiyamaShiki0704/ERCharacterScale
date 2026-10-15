#![allow(dead_code)]
#![allow(clippy::absurd_extreme_comparisons)]
#![allow(clippy::manual_find)]
#![allow(clippy::redundant_closure)]
#![allow(clippy::too_many_arguments)]

use std::{
    fmt::Write,
    slice,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use ilhook::x64::{CallbackOption, HookFlags, Registers, hook_closure_jmp_back};
use windows::{
    Win32::{
        Foundation::HMODULE,
        System::{
            LibraryLoader::GetModuleHandleA,
            Memory::{
                MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ,
                PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_READONLY, PAGE_READWRITE,
                PAGE_WRITECOPY, VirtualQuery,
            },
        },
    },
    core::s,
};

use crate::log;

const NR_CHARACTER_SCALE_COPY_RVA: usize = 0x66CF66;
const NR_CLOTH_MODEL_INS_CTOR_RVA: usize = 0xD84300;
const NR_PHYS_SYS_OWNER_INIT_RVA: usize = 0xD8B0B0;
const NR_HAVOK_CAPACITY_UPDATE_RVA: usize = 0xD9E420;
const NR_PHYS_WORLD_DAB9E0_RVA: usize = 0xDAB9E0;
const NR_PHYS_GLOBAL_SLOT_RVA: usize = 0x3C1D628;
const NR_CSHAVOK_BUFFER_CAPACITY_TYPE_RVA: usize = 0x2C9E880;
const NR_CSPHYSWORLD_TYPE_RVA: usize = 0x2C9EF68;

const OBSERVED_LIMIT: usize = 4;
const OBSERVED_SCALE_OWNER_LIMIT: usize = 16;
const DETAIL_LIMIT: usize = 180;
const ACCESSOR_LOG_LIMIT: usize = 32;
const HAVOK_CAPACITY_UPDATE_LOG_LIMIT: usize = 8;
const PHYS_WORLD_METHOD_LOG_LIMIT: usize = 16;
const HAVOK_CAPACITY_RAW_SCAN_RECORDS: usize = 24;
const VTABLE_SUMMARY_ENTRY_COUNT: usize = 24;
const VTABLE_SUMMARY_LOG_LIMIT: usize = 32;
const FIELD_SUMMARY_LOG_LIMIT: usize = 80;
const FIXED_CHILD_DEEP_LOG_LIMIT: usize = 72;
const LIVE_CHILD_POOL_LOG_LIMIT: usize = 120;
const LIVE_CHILD_POOL_MAX_ENTRIES: usize = 24;
const LIVE_CHILD_POOL_HIT_SAMPLE_LIMIT: usize = 2;
const HKX_RESOURCE_SCAN_PASSES: usize = 3;
const HKX_RESOURCE_SCAN_INITIAL_DELAY_SECONDS: u64 = 10;
const HKX_RESOURCE_SCAN_INTERVAL_SECONDS: u64 = 12;
const HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS: usize = 1536 * 1024 * 1024;
const HKX_RESOURCE_SCAN_HIT_LIMIT_PER_PASS: usize = 8;
const HKX_RESOURCE_SCAN_TYPE_HIT_LIMIT_PER_PASS: usize = 0;
const HKX_TARGET_RESOURCE_HIT_LIMIT_PER_PASS: usize = 0;
const HKX_TARGET_REFERENCE_LIMIT_PER_PASS: usize = 48;
const HKX_TARGET_STRING_ADDR_LIMIT: usize = 48;
const HKX_OWNER_STRING_NEIGHBOR_LIMIT: usize = 24;
const HKX_POINTER_ARRAY_MIN_TARGETS: usize = 4;
const HKX_POINTER_ARRAY_CANDIDATE_LIMIT_PER_PASS: usize = 8;
const HKX_POINTER_ARRAY_OWNER_TARGET_LIMIT: usize = 48;
const HKX_POINTER_ARRAY_OWNER_REF_LIMIT_PER_PASS: usize = 24;
const HKX_RESOURCE_PATH_DETAIL_LIMIT: usize = 24;
const HKX_RESOURCE_SCAN_REGION_SLEEP_BYTES: usize = 64 * 1024 * 1024;
const DUMP_PASSES: usize = 4;
const DUMP_INTERVAL_SECONDS: u64 = 2;
const CORRELATION_QWORD_SCAN_SIZE: usize = 0x900;
const CORRELATION_FLOAT_SCAN_SIZE: usize = 0x300;
const SCALE_OWNER_NEIGHBOR_SCAN_SIZE: usize = 0x240;
const SCALE_OWNER_NEIGHBOR_DETAIL_LIMIT: usize = 160;
const SCALE_OWNER_NEIGHBOR_OFFSETS: &[usize] = &[
    0x3C8, 0x5A0, 0x5A8, 0x5B0, 0x5D0, 0x5F0, 0x608, 0x610, 0x668, 0x670, 0x678, 0x680, 0x6B0,
];
const HKNP_WORLD_CORE_SIZE: usize = 0xB70;
const WORLD_CORE_POINTER_EXTRA_LIMIT: usize = 0;
const WORLD_CORE_ARRAY_SCAN_ENTRIES: usize = 96;
const WORLD_CORE_ARRAY_OUTPUT_LIMIT: usize = 16;
const WORLD_CORE_RAW_ROW_LIMIT: usize = 2;
const WORLD_CORE_STRIDE_CANDIDATES: [usize; 6] = [0x10, 0x20, 0x30, 0x40, 0x80, 0xA0];
const WORLD_CORE_POINTER_FOCUS_OFFSETS: [usize; 5] = [0x0, 0x488, 0x4C8, 0x4D0, 0x4D8];
const WORLD_CORE_CANDIDATE_CHILD_OFFSETS: [usize; 3] = [0x4D8, 0x4D0, 0x4C8];
const WORLD_CORE_CHILD_SLOT_LOG_LIMIT: usize = 12;
const WORLD_CORE_CHILD_SLOTS_488: &[usize] = &[0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40];
const WORLD_CORE_CHILD_SLOTS_4B8: &[usize] = &[0x0, 0x40, 0x50];
const WORLD_CORE_CHILD_SLOTS_4C0: &[usize] = &[0x0, 0x8, 0x10, 0x50, 0x60, 0x70, 0x78, 0x80, 0x88];
const WORLD_CORE_CHILD_SLOTS_CLUSTER: &[usize] = &[
    0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x68, 0x70, 0x78,
    0x80, 0x88,
];
const WORLD_CORE_4D8_NODE_OFFSETS: &[usize] = &[0x20, 0x28, 0x30, 0x38, 0x40, 0x48];

#[derive(Clone, Copy)]
struct ScaleOwnerObservation {
    owner: usize,
    source: usize,
    scale: f32,
    pre_6a4: f32,
    pre_6b8: f32,
}

static NR_HOOKS_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static NR_DUMP_THREAD_STARTED: AtomicUsize = AtomicUsize::new(0);
static NR_LATE_CORRELATION_THREAD_STARTED: AtomicUsize = AtomicUsize::new(0);
static NR_HKX_RESOURCE_SCAN_THREAD_STARTED: AtomicUsize = AtomicUsize::new(0);
static NR_MODULE_BASE: AtomicUsize = AtomicUsize::new(0);
static DETAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
static CACHED_PHYS_SYS_INS: AtomicUsize = AtomicUsize::new(0);
static ACCESSOR_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
static HAVOK_CAPACITY_UPDATE_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
static PHYS_WORLD_METHOD_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
static HKX_RESOURCE_PATH_DETAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
static HKX_OWNER_STRING_NEIGHBOR_COUNT: AtomicUsize = AtomicUsize::new(0);
static SCALE_OWNER_NEIGHBOR_DETAIL_COUNT: AtomicUsize = AtomicUsize::new(0);
static VTABLE_SUMMARY_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
static FIELD_SUMMARY_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
static FIXED_CHILD_DEEP_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
static LIVE_CHILD_POOL_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
static OBSERVED_SCALE_OWNERS: OnceLock<Mutex<Vec<ScaleOwnerObservation>>> = OnceLock::new();
static OBSERVED_CLOTH_MODELS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
static OBSERVED_PHYS_OWNERS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();

pub fn install_havok_runtime_root_probe() {
    if NR_HOOKS_INSTALLED.swap(1, Ordering::AcqRel) != 0 {
        return;
    }

    let Ok(module) = (unsafe { GetModuleHandleA(s!("nightreign.exe")) }) else {
        log::line(format_args!(
            "[player-scale-no-bone] NR phys-accessor probe: GetModuleHandleA failed"
        ));
        return;
    };

    let base = module_base(module);
    NR_MODULE_BASE.store(base, Ordering::Release);
    log_static_anchors();
    start_delayed_dump_thread();

    let mut installed = 0usize;
    installed += install_hook(
        base,
        "CharacterScale_copy_66cf66",
        NR_CHARACTER_SCALE_COPY_RVA,
        character_scale_copy_hook,
    ) as usize;
    installed += install_hook(
        base,
        "CSClothModelIns_ctor_d84300",
        NR_CLOTH_MODEL_INS_CTOR_RVA,
        cloth_model_ins_ctor_hook,
    ) as usize;
    installed += install_hook(
        base,
        "CSPhysSysIns_owner_init_d8b0b0",
        NR_PHYS_SYS_OWNER_INIT_RVA,
        phys_sys_owner_init_hook,
    ) as usize;
    installed += install_hook(
        base,
        "CSPhysWorld_method_dab9e0",
        NR_PHYS_WORLD_DAB9E0_RVA,
        phys_world_dab9e0_hook,
    ) as usize;

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-world dab9e0 arg probe: installed {installed}/4 hooks"
    ));
}

fn log_static_anchors() {
    log::line(format_args!(
        "[player-scale-no-bone] NR phys-real-entry static: owner+0x3C8 remains classified as FD4/model-location scaling"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR phys-real-entry static: hook CSClothModelIns ctor at 0xD84300; immediate +0x88 is early/in-construction"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR phys-real-entry static: hook real function entry 0xD8B0B0; static 0xD8B142 stores new 0x2C9DDD0 object at owner+0x30"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR phys-real-entry static: internal offset hooks 0xDA6480/0xDA6530/0xDA65F0 remain disabled after load-crash report"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR phys-world-core static: CSPhysWorld+0x8 shared with CSHavokBufferCapacity+0x8; q0 0x2FCF9F8 is near hknp proxy/deferred-update/collision task strings"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR scale-phys static: world_core+0x4D8 is broadphase/NavMesh-adjacent; deep world-core expansion disabled"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR scale-phys static: CharacterScale copy hook at 0x66CF66 reads source+0x10C and stores owner+0x6B8/+0x6A4"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR scale-owner-neighbor static: late passes log owner+0x668/+0x670/+0x608/+0x610 and other neighbors without adding new hooks"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR havok-capacity static: 0xD9E420 update-window hook disabled; branch classified as capacity/model-task metadata"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] NR phys-world static: hook CSPhysWorld real entry 0xDAB9E0; log q0-matched CSPhysWorld rcx with optional phys_owner match"
    ));
}

fn install_hook(
    base: usize,
    label: &'static str,
    rva: usize,
    callback: fn(*mut Registers),
) -> bool {
    let addr = base + rva;
    log::line(format_args!(
        "[player-scale-no-bone] NR phys-real-entry probe: installing {label} at nightreign.exe+0x{rva:X} addr=0x{addr:X}"
    ));

    match unsafe {
        hook_closure_jmp_back(
            addr,
            move |registers| callback(registers),
            CallbackOption::None,
            HookFlags::empty(),
        )
    } {
        Ok(hook) => {
            let _ = Box::leak(Box::new(hook));
            log::line(format_args!(
                "[player-scale-no-bone] NR phys-real-entry probe: {label} hook installed"
            ));
            true
        }
        Err(err) => {
            log::line(format_args!(
                "[player-scale-no-bone] NR phys-real-entry probe: {label} hook failed: {err:?}"
            ));
            false
        }
    }
}

fn character_scale_copy_hook(registers: *mut Registers) {
    let registers = unsafe { &*registers };
    record_scale_owner(registers.rbx as usize, registers.rax as usize);
}

fn cloth_model_ins_ctor_hook(registers: *mut Registers) {
    let registers = unsafe { &*registers };
    record_cloth_model(registers.rcx as usize, registers);
}

fn phys_sys_owner_init_hook(registers: *mut Registers) {
    let registers = unsafe { &*registers };
    record_phys_owner(registers.rcx as usize, registers);
}

fn havok_capacity_update_hook(registers: *mut Registers) {
    let registers = unsafe { &*registers };
    record_havok_capacity_update(registers.rcx as usize, registers);
}

fn phys_world_dab9e0_hook(registers: *mut Registers) {
    let registers = unsafe { &*registers };
    record_phys_world_method_hit("DAB9E0", registers.rcx as usize, registers);
}

fn record_scale_owner(owner: usize, source: usize) {
    if owner == 0 || source == 0 {
        return;
    }

    let scale = read_f32(source + 0x10C);
    if !scale_like(scale) {
        return;
    }

    let pre_6a4 = read_f32(owner + 0x6A4);
    let pre_6b8 = read_f32(owner + 0x6B8);

    {
        let observed = OBSERVED_SCALE_OWNERS.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(mut objects) = observed.lock() else {
            return;
        };

        if let Some(existing) = objects.iter_mut().find(|entry| entry.owner == owner) {
            existing.source = source;
            existing.scale = scale;
            existing.pre_6a4 = pre_6a4;
            existing.pre_6b8 = pre_6b8;
            return;
        }

        if objects.len() >= OBSERVED_SCALE_OWNER_LIMIT {
            return;
        }

        objects.push(ScaleOwnerObservation {
            owner,
            source,
            scale,
            pre_6a4,
            pre_6b8,
        });
    }

    start_late_correlation_thread();

    log::line(format_args!(
        "[player-scale-no-bone] NR scale-copy observed owner=0x{owner:X} source=0x{source:X} sourceScale10C={scale:.4} ownerPre6A4={pre_6a4:.4} ownerPre6B8={pre_6b8:.4} sourceQ0=0x{:X}({}) ownerQ0=0x{:X}({})",
        read_usize(source),
        format_module_rva(read_usize(source)),
        read_usize(owner),
        format_module_rva(read_usize(owner)),
    ));
}

fn record_cloth_model(addr: usize, registers: &Registers) {
    if addr == 0 {
        return;
    }

    {
        let observed = OBSERVED_CLOTH_MODELS.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(mut objects) = observed.lock() else {
            return;
        };

        if objects.contains(&addr) {
            return;
        }

        if objects.len() >= OBSERVED_LIMIT {
            return;
        }

        objects.push(addr);
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-real-entry observed CSClothModelIns this=0x{addr:X} rax=0x{:X} rbx=0x{:X} rcx=0x{:X} rdx=0x{:X} r8=0x{:X} r9=0x{:X}",
        registers.rax as usize,
        registers.rbx as usize,
        registers.rcx as usize,
        registers.rdx as usize,
        registers.r8 as usize,
        registers.r9 as usize,
    ));

    let phys_sys = read_usize(addr + 0x88);
    if phys_sys != 0 {
        CACHED_PHYS_SYS_INS.store(phys_sys, Ordering::Release);
        log::line(format_args!(
            "[player-scale-no-bone] NR phys-real-entry early cached CSClothModelIns+0x88=0x{phys_sys:X}"
        ));

        if is_readable_memory(phys_sys, 0x80) {
            log_qwords(
                0,
                "safe likely CSPhysSysIns head",
                addr,
                0x88,
                phys_sys,
                &[
                    0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60,
                    0x68, 0x70, 0x78,
                ],
            );
        }
    }
}

fn record_phys_owner(addr: usize, registers: &Registers) {
    if addr == 0 {
        return;
    }

    {
        let observed = OBSERVED_PHYS_OWNERS.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(mut objects) = observed.lock() else {
            return;
        };

        if objects.contains(&addr) {
            return;
        }

        if objects.len() >= OBSERVED_LIMIT {
            return;
        }

        objects.push(addr);
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-real-entry observed phys owner init this=0x{addr:X} rax=0x{:X} rbx=0x{:X} rcx=0x{:X} rdx=0x{:X} r8=0x{:X} r9=0x{:X}",
        registers.rax as usize,
        registers.rbx as usize,
        registers.rcx as usize,
        registers.rdx as usize,
        registers.r8 as usize,
        registers.r9 as usize,
    ));
}

fn record_havok_capacity_update(capacity: usize, registers: &Registers) {
    if capacity == 0 || !is_readable_memory(capacity, 0x70) {
        return;
    }

    let base = NR_MODULE_BASE.load(Ordering::Acquire);
    let q0 = read_usize(capacity);
    if base != 0 && q0 != base + NR_CSHAVOK_BUFFER_CAPACITY_TYPE_RVA {
        return;
    }

    let log_count = HAVOK_CAPACITY_UPDATE_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if log_count > HAVOK_CAPACITY_UPDATE_LOG_LIMIT {
        return;
    }

    let phys_owner = find_phys_owner_for_havok_capacity(capacity).unwrap_or(0);
    log::line(format_args!(
        "[player-scale-no-bone] NR havok-capacity update hit #{log_count}: this=0x{capacity:X} physOwner=0x{phys_owner:X} rax=0x{:X} rbx=0x{:X} rcx=0x{:X} rdx=0x{:X} r8=0x{:X} r9=0x{:X} q0=0x{q0:X}({}) q8=0x{:X} q10=0x{:X} q18=0x{:X} q28=0x{:X} q38=0x{:X} q48=0x{:X} q58=0x{:X} q68=0x{:X}",
        registers.rax as usize,
        registers.rbx as usize,
        registers.rcx as usize,
        registers.rdx as usize,
        registers.r8 as usize,
        registers.r9 as usize,
        format_module_rva(q0),
        read_usize(capacity + 0x8),
        read_usize(capacity + 0x10),
        read_usize(capacity + 0x18),
        read_usize(capacity + 0x28),
        read_usize(capacity + 0x38),
        read_usize(capacity + 0x48),
        read_usize(capacity + 0x58),
        read_usize(capacity + 0x68),
    ));

    log_havok_capacity_update_window(log_count, capacity, phys_owner);
}

fn find_phys_owner_for_havok_capacity(capacity: usize) -> Option<usize> {
    let observed = OBSERVED_PHYS_OWNERS.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(objects) = observed.lock() else {
        return None;
    };

    objects
        .iter()
        .copied()
        .find(|owner| read_usize(*owner + 0x90) == capacity)
}

fn find_phys_owner_for_phys_world(world: usize) -> Option<usize> {
    let observed = OBSERVED_PHYS_OWNERS.get_or_init(|| Mutex::new(Vec::new()));
    let Ok(objects) = observed.lock() else {
        return None;
    };

    objects
        .iter()
        .copied()
        .find(|owner| read_usize(*owner + 0x98) == world)
}

fn record_phys_world_method_hit(label: &str, world: usize, registers: &Registers) {
    if world == 0 || !is_readable_memory(world, 0x20) {
        return;
    }

    let base = NR_MODULE_BASE.load(Ordering::Acquire);
    let q0 = read_usize(world);
    if base != 0 && q0 != base + NR_CSPHYSWORLD_TYPE_RVA {
        return;
    }

    let phys_owner = find_phys_owner_for_phys_world(world).unwrap_or(0);

    let log_count = PHYS_WORLD_METHOD_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if log_count > PHYS_WORLD_METHOD_LOG_LIMIT {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-world method hit #{log_count} {label}: world=0x{world:X} physOwner=0x{phys_owner:X} rax=0x{:X} rbx=0x{:X} rcx=0x{:X} rdx=0x{:X} r8=0x{:X} r9=0x{:X} q0=0x{q0:X}({}) q8=0x{:X} q10=0x{:X} q18=0x{:X}",
        registers.rax as usize,
        registers.rbx as usize,
        registers.rcx as usize,
        registers.rdx as usize,
        registers.r8 as usize,
        registers.r9 as usize,
        format_module_rva(q0),
        read_usize(world + 0x8),
        read_usize(world + 0x10),
        read_usize(world + 0x18),
    ));

    log_phys_world_method_arg(log_count, label, world, "rax", registers.rax as usize);
    log_phys_world_method_arg(log_count, label, world, "rbx", registers.rbx as usize);
    log_phys_world_method_arg(log_count, label, world, "rdx", registers.rdx as usize);
    log_phys_world_method_arg(log_count, label, world, "r8", registers.r8 as usize);
    log_phys_world_method_arg(log_count, label, world, "r9", registers.r9 as usize);
}

fn log_phys_world_method_arg(hit: usize, label: &str, world: usize, name: &str, ptr: usize) {
    if ptr == 0 || ptr == world || !is_readable_memory(ptr, 0x48) {
        return;
    }

    let q0 = read_usize(ptr);
    let q8 = read_usize(ptr + 0x8);
    let q10 = read_usize(ptr + 0x10);
    let q18 = read_usize(ptr + 0x18);
    let q20 = read_usize(ptr + 0x20);
    let q28 = read_usize(ptr + 0x28);
    let q30 = read_usize(ptr + 0x30);
    let q38 = read_usize(ptr + 0x38);
    let q40 = read_usize(ptr + 0x40);

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-world method arg #{hit} {label}.{name}: world=0x{world:X} ptr=0x{ptr:X} q0=0x{q0:X}({}) q8=0x{q8:X}({}) q10=0x{q10:X} q18=0x{q18:X} q20=0x{q20:X} q28=0x{q28:X} q30=0x{q30:X} q38=0x{q38:X} q40=0x{q40:X}",
        format_module_rva(q0),
        format_module_rva(q8),
    ));
}

fn log_havok_capacity_update_window(hit: usize, capacity: usize, phys_owner: usize) {
    if !is_readable_memory(capacity, 0x70) {
        return;
    }

    let pass = 9000 + hit;
    log_qwords(
        pass,
        "CSHavokBufferCapacity update-window head",
        phys_owner,
        0x90,
        capacity,
        &[
            0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x68, 0x80,
            0x88,
        ],
    );

    for offset in [0x8, 0x18, 0x28, 0x38, 0x48, 0x58, 0x68, 0x80, 0x88] {
        let child = read_usize(capacity + offset);
        log_child_head(
            pass,
            "CSHavokBufferCapacity update-window",
            capacity,
            offset,
            child,
        );
    }

    for offset in [0x18, 0x28, 0x38, 0x80] {
        let child = read_usize(capacity + offset);
        log_capacity_raw_stride_summary(hit, capacity, offset, child);
    }

    let active = read_usize(capacity + 0x38);
    if active != 0 && is_readable_memory(active, 0xE0) {
        log_qwords(
            pass,
            "CSHavokBufferCapacity+0x38 active child",
            capacity,
            0x38,
            active,
            &[
                0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x70,
                0x80, 0x88, 0x90, 0x98, 0xA0, 0xB0, 0xC0, 0xD0, 0xD8,
            ],
        );
        log_float_rows(
            pass,
            "CSHavokBufferCapacity+0x38 active child",
            active,
            &[0x0, 0x10, 0x20, 0x30, 0x40, 0x60, 0x80, 0xA0, 0xC0],
        );
        log_havok_landmarks(pass, "CSHavokBufferCapacity+0x38 active child", active);
    }

    let window = read_usize(capacity + 0x48);
    if window == 0 {
        return;
    }

    if !is_readable_memory(window, 0xE0) {
        log::line(format_args!(
            "[player-scale-no-bone] NR havok-capacity update-window #{hit}: +0x48 ptr=0x{window:X} unreadable"
        ));
        return;
    }

    log_qwords(
        pass,
        "CSHavokBufferCapacity+0x48 update child",
        capacity,
        0x48,
        window,
        &[
            0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x70, 0x80,
            0x88, 0x90, 0x98, 0xA0, 0xB0, 0xC0, 0xD0, 0xD8,
        ],
    );
    log_float_rows(
        pass,
        "CSHavokBufferCapacity+0x48 update child",
        window,
        &[0x20, 0x40, 0x60, 0x70, 0x80, 0xA0, 0xC0],
    );
    log_havok_landmarks(pass, "CSHavokBufferCapacity+0x48 update child", window);
}

fn log_capacity_raw_stride_summary(hit: usize, capacity: usize, offset: usize, buffer: usize) {
    if buffer == 0 || !is_readable_memory(buffer, 0x40) {
        return;
    }

    for stride in [0x10usize, 0x20, 0x40, 0x80] {
        let mut nonzero = 0usize;
        let mut pointer_like = 0usize;
        let mut module_like = 0usize;
        let mut finite_vec4 = 0usize;
        let mut sentinel_heavy = 0usize;

        for index in 0..HAVOK_CAPACITY_RAW_SCAN_RECORDS {
            let record = buffer + index * stride;
            if !is_readable_memory(record, stride.min(0x40)) {
                continue;
            }

            let q0 = read_usize(record);
            let q8 = if stride >= 0x10 {
                read_usize(record + 0x8)
            } else {
                0
            };
            let q10 = if stride >= 0x20 {
                read_usize(record + 0x10)
            } else {
                0
            };
            let q18 = if stride >= 0x20 {
                read_usize(record + 0x18)
            } else {
                0
            };

            if q0 != 0 || q8 != 0 || q10 != 0 || q18 != 0 {
                nonzero += 1;
            }
            if [q0, q8, q10, q18]
                .iter()
                .any(|value| *value != 0 && is_readable_memory(*value, 0x10))
            {
                pointer_like += 1;
            }
            if [q0, q8, q10, q18]
                .iter()
                .any(|value| format_module_rva(*value).starts_with("nightreign.exe+"))
            {
                module_like += 1;
            }
            if vec4_finite_plausible(read_vec4(record), 100000.0) {
                finite_vec4 += 1;
            }
            if [q0, q8, q10, q18]
                .iter()
                .filter(|value| **value == usize::MAX)
                .count()
                >= 2
            {
                sentinel_heavy += 1;
            }
        }

        log::line(format_args!(
            "[player-scale-no-bone] NR havok-capacity raw-summary hit #{hit}: capacity=0x{capacity:X}+0x{offset:X} buffer=0x{buffer:X} stride=0x{stride:X} scanned={} nonzero={nonzero} ptrLike={pointer_like} moduleLike={module_like} finiteVec4={finite_vec4} sentinelHeavy={sentinel_heavy}",
            HAVOK_CAPACITY_RAW_SCAN_RECORDS
        ));
    }
}

fn log_phys_sys_accessor(label: &str, registers: &Registers) {
    let rcx = registers.rcx as usize;
    let cached = CACHED_PHYS_SYS_INS.load(Ordering::Acquire);
    if rcx == 0 || cached == 0 || rcx != cached {
        return;
    }

    let log_count = ACCESSOR_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if log_count > ACCESSOR_LOG_LIMIT {
        return;
    }

    let out = registers.rdx as usize;
    let index = registers.r8 as i32;
    let table = read_usize(rcx + 0x70);
    let table_array = read_usize(table + 0x28);
    let table_count = read_u32(table + 0x30);
    let selected_id = if index >= 0 && (index as u32) < table_count && table_array != 0 {
        read_u32(table_array + (index as usize * 4))
    } else {
        0
    };

    let base = NR_MODULE_BASE.load(Ordering::Acquire);
    let global_slot = if base == 0 {
        0
    } else {
        read_usize(base + NR_PHYS_GLOBAL_SLOT_RVA)
    };
    let global_vtable = read_usize(global_slot);
    let global_10 = read_usize(global_slot + 0x10);
    let global_98 = read_usize(global_slot + 0x98);
    let manager_vtable = read_usize(global_98);
    let manager_28 = read_usize(global_98 + 0x28);
    let manager_30 = read_usize(global_98 + 0x30);

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-accessor hit #{log_count} {label} rcx=0x{rcx:X} out_rdx=0x{out:X} r8_index={index} table70=0x{table:X} table_array28=0x{table_array:X} table_count30={table_count} selected_id=0x{selected_id:X} global_slot=0x{global_slot:X} global_vtable=0x{global_vtable:X}({}) global10=0x{global_10:X} global98=0x{global_98:X} manager_vtable=0x{manager_vtable:X}({}) manager28=0x{manager_28:X} manager30=0x{manager_30:X}",
        format_module_rva(global_vtable),
        format_module_rva(manager_vtable),
    ));

    if is_readable_memory(table, 0x40) {
        log_qwords(
            0,
            "accessor table70",
            rcx,
            0x70,
            table,
            &[0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38],
        );
    }
    if is_readable_memory(global_98, 0x80) {
        log_qwords(
            0,
            "accessor global98 manager",
            global_slot,
            0x98,
            global_98,
            &[
                0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x70,
            ],
        );
    }
}

fn start_delayed_dump_thread() {
    if NR_DUMP_THREAD_STARTED.swap(1, Ordering::AcqRel) != 0 {
        return;
    }

    thread::spawn(|| {
        for pass in 1..=DUMP_PASSES {
            thread::sleep(Duration::from_secs(DUMP_INTERVAL_SECONDS));
            dump_observed_runtime_summary(pass);
        }
    });
}

fn start_hkx_resource_scan_thread() {
    if NR_HKX_RESOURCE_SCAN_THREAD_STARTED.swap(1, Ordering::AcqRel) != 0 {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-target scan: scheduled c.hkx pointer-array owner probe passes={HKX_RESOURCE_SCAN_PASSES} initial_delay={HKX_RESOURCE_SCAN_INITIAL_DELAY_SECONDS}s interval={HKX_RESOURCE_SCAN_INTERVAL_SECONDS}s max_bytes_per_pass={HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS} useful_c_hkx_limit={HKX_RESOURCE_SCAN_HIT_LIMIT_PER_PASS} array_limit={HKX_POINTER_ARRAY_CANDIDATE_LIMIT_PER_PASS} owner_ref_limit={HKX_POINTER_ARRAY_OWNER_REF_LIMIT_PER_PASS}"
    ));

    thread::spawn(|| {
        thread::sleep(Duration::from_secs(HKX_RESOURCE_SCAN_INITIAL_DELAY_SECONDS));
        for pass in 1..=HKX_RESOURCE_SCAN_PASSES {
            scan_hkx_resource_strings(pass);
            if pass != HKX_RESOURCE_SCAN_PASSES {
                thread::sleep(Duration::from_secs(HKX_RESOURCE_SCAN_INTERVAL_SECONDS));
            }
        }
    });
}

fn scan_hkx_resource_strings(pass: usize) {
    let ascii_hkx_patterns: &[(&str, &[u8])] = &[("c.hkx", b"c.hkx")];
    let utf16_hkx_patterns = [("c.hkx", utf16_bytes("c.hkx"))];
    let ascii_target_patterns: &[(&str, &[u8])] = &[];
    let utf16_target_patterns: [(&str, Vec<u8>); 0] = [];

    let mut addr = 0x10000usize;
    let max_addr = 0x0000_7FFF_FFFF_FFFFusize;
    let mut regions = 0usize;
    let mut scanned_regions = 0usize;
    let mut scanned_bytes = 0usize;
    let mut hkx_hits = 0usize;
    let mut target_hits = 0usize;
    let mut capped = false;
    let mut bytes_since_sleep = 0usize;
    let mut c_hkx_string_addrs = Vec::new();

    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-target scan pass #{pass}: begin"
    ));

    while addr < max_addr
        && scanned_bytes < HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS
        && (hkx_hits < HKX_RESOURCE_SCAN_HIT_LIMIT_PER_PASS
            || target_hits < HKX_TARGET_RESOURCE_HIT_LIMIT_PER_PASS)
    {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let result = unsafe {
            VirtualQuery(
                Some(addr as *const _),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if result == 0 {
            break;
        }

        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        let next = base.saturating_add(size);
        regions += 1;

        if is_scan_candidate_region(&mbi) {
            let remaining = HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS.saturating_sub(scanned_bytes);
            let scan_len = size.min(remaining);
            if scan_len > 0 {
                scanned_regions += 1;
                scanned_bytes = scanned_bytes.saturating_add(scan_len);
                bytes_since_sleep = bytes_since_sleep.saturating_add(scan_len);

                let bytes = unsafe { slice::from_raw_parts(base as *const u8, scan_len) };
                let (region_hkx_hits, region_target_hits) = scan_hkx_resource_region(
                    pass,
                    base,
                    bytes,
                    &mbi,
                    ascii_hkx_patterns,
                    &utf16_hkx_patterns,
                    ascii_target_patterns,
                    &utf16_target_patterns,
                    HKX_RESOURCE_SCAN_HIT_LIMIT_PER_PASS.saturating_sub(hkx_hits),
                    HKX_TARGET_RESOURCE_HIT_LIMIT_PER_PASS.saturating_sub(target_hits),
                    &mut c_hkx_string_addrs,
                );
                hkx_hits += region_hkx_hits;
                target_hits += region_target_hits;

                if bytes_since_sleep >= HKX_RESOURCE_SCAN_REGION_SLEEP_BYTES {
                    bytes_since_sleep = 0;
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }

        if next <= addr {
            addr = addr.saturating_add(0x1000);
        } else {
            addr = next;
        }
    }

    if scanned_bytes >= HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS
        || (HKX_RESOURCE_SCAN_HIT_LIMIT_PER_PASS > 0
            && hkx_hits >= HKX_RESOURCE_SCAN_HIT_LIMIT_PER_PASS)
        || (HKX_TARGET_RESOURCE_HIT_LIMIT_PER_PASS > 0
            && target_hits >= HKX_TARGET_RESOURCE_HIT_LIMIT_PER_PASS)
    {
        capped = true;
    }

    c_hkx_string_addrs.sort_unstable();
    c_hkx_string_addrs.dedup();
    if c_hkx_string_addrs.len() > HKX_TARGET_STRING_ADDR_LIMIT {
        c_hkx_string_addrs.truncate(HKX_TARGET_STRING_ADDR_LIMIT);
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-target scan pass #{pass}: end regions={regions} scanned_regions={scanned_regions} scanned_bytes={scanned_bytes} hkx_hits={hkx_hits} target_hits={target_hits} c_hkx_string_addrs={} capped={capped}",
        c_hkx_string_addrs.len()
    ));

    scan_c_hkx_pointer_array_owners(pass, &c_hkx_string_addrs);
}

fn scan_hkx_resource_region(
    pass: usize,
    base: usize,
    bytes: &[u8],
    mbi: &MEMORY_BASIC_INFORMATION,
    ascii_hkx_patterns: &[(&str, &[u8])],
    utf16_hkx_patterns: &[(&str, Vec<u8>)],
    ascii_target_patterns: &[(&str, &[u8])],
    utf16_target_patterns: &[(&str, Vec<u8>)],
    remaining_hkx_hits: usize,
    remaining_target_hits: usize,
    target_string_addrs: &mut Vec<usize>,
) -> (usize, usize) {
    let mut hkx_hits = 0usize;
    let mut target_hits = 0usize;

    for (label, pattern) in ascii_hkx_patterns {
        hkx_hits += scan_pattern_hits(
            pass,
            base,
            bytes,
            mbi,
            label,
            pattern,
            true,
            "hkx",
            true,
            remaining_hkx_hits.saturating_sub(hkx_hits),
            target_string_addrs,
        );
        if hkx_hits >= remaining_hkx_hits {
            break;
        }
    }

    for (label, pattern) in utf16_hkx_patterns {
        if hkx_hits >= remaining_hkx_hits {
            break;
        }
        hkx_hits += scan_pattern_hits(
            pass,
            base,
            bytes,
            mbi,
            label,
            pattern,
            true,
            "hkx",
            true,
            remaining_hkx_hits.saturating_sub(hkx_hits),
            target_string_addrs,
        );
    }

    for (label, pattern) in ascii_target_patterns {
        target_hits += scan_pattern_hits(
            pass,
            base,
            bytes,
            mbi,
            label,
            pattern,
            false,
            "resource",
            false,
            remaining_target_hits.saturating_sub(target_hits),
            target_string_addrs,
        );
        if target_hits >= remaining_target_hits {
            break;
        }
    }

    for (label, pattern) in utf16_target_patterns {
        if target_hits >= remaining_target_hits {
            break;
        }
        target_hits += scan_pattern_hits(
            pass,
            base,
            bytes,
            mbi,
            label,
            pattern,
            true,
            "resource",
            false,
            remaining_target_hits.saturating_sub(target_hits),
            target_string_addrs,
        );
    }

    (hkx_hits, target_hits)
}

fn scan_pattern_hits(
    pass: usize,
    base: usize,
    bytes: &[u8],
    mbi: &MEMORY_BASIC_INFORMATION,
    label: &str,
    pattern: &[u8],
    utf16: bool,
    category: &str,
    record_target_addr: bool,
    limit: usize,
    target_string_addrs: &mut Vec<usize>,
) -> usize {
    if pattern.is_empty() || bytes.len() < pattern.len() || limit == 0 {
        return 0;
    }

    let mut hits = 0usize;
    let mut pos = 0usize;
    while pos + pattern.len() <= bytes.len() && hits < limit {
        let haystack = &bytes[pos..];
        let Some(relative) = find_subslice(haystack, pattern) else {
            break;
        };
        let index = pos + relative;
        let addr = base + index;
        let context = if utf16 {
            sanitize_utf16_context(bytes, index)
        } else {
            sanitize_ascii_context(bytes, index)
        };
        let encoding = if utf16 { "utf16" } else { "ascii" };
        let record_reference_target =
            record_target_addr && should_record_hkx_reference_target(category, &context);
        if category == "hkx" && !record_reference_target {
            pos = index.saturating_add(pattern.len());
            continue;
        }
        let string_start_index = if record_reference_target {
            if utf16 {
                infer_utf16_string_start(bytes, index)
            } else {
                infer_ascii_string_start(bytes, index)
            }
        } else {
            index
        };
        let string_start = base + string_start_index;
        if record_reference_target && target_string_addrs.len() < HKX_TARGET_STRING_ADDR_LIMIT {
            target_string_addrs.push(addr);
            if string_start != addr && target_string_addrs.len() < HKX_TARGET_STRING_ADDR_LIMIT {
                target_string_addrs.push(string_start);
            }
            log_hkx_string_neighborhood(pass, label, addr, string_start, mbi, &context);
        }
        log::line(format_args!(
            "[player-scale-no-bone] NR hkx-target hit pass #{pass}: category={category} marker={label} encoding={encoding} addr=0x{addr:X} string_start=0x{string_start:X} region=0x{:X} size=0x{:X} protect=0x{:X} type=0x{:X} context='{context}'",
            mbi.BaseAddress as usize, mbi.RegionSize, mbi.Protect.0, mbi.Type.0,
        ));
        hits += 1;
        pos = index.saturating_add(pattern.len());
    }

    hits
}

fn should_record_hkx_reference_target(category: &str, context: &str) -> bool {
    if category != "hkx" {
        return false;
    }

    let lower = context.to_ascii_lowercase();
    lower.contains("5050") && lower.contains("parts") && lower.contains("_c.hkx")
}

fn log_hkx_string_neighborhood(
    pass: usize,
    label: &str,
    addr: usize,
    string_start: usize,
    mbi: &MEMORY_BASIC_INFORMATION,
    context: &str,
) {
    let detail = HKX_OWNER_STRING_NEIGHBOR_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail > HKX_OWNER_STRING_NEIGHBOR_LIMIT {
        return;
    }

    let aligned = string_start & !0x7;
    let start = aligned.saturating_sub(0x40);
    let mut line = format!(
        "[player-scale-no-bone] NR hkx-owner string pass #{pass} detail #{detail}: marker={label} addr=0x{addr:X} string_start=0x{string_start:X} region=0x{:X} size=0x{:X} protect=0x{:X} type=0x{:X}",
        mbi.BaseAddress as usize, mbi.RegionSize, mbi.Protect.0, mbi.Type.0,
    );
    for offset in (0..=0x80).step_by(8) {
        let q = read_usize(start + offset);
        let _ = write!(
            line,
            " q{:+X}=0x{q:X}({})",
            offset as isize - 0x40,
            format_module_rva(q)
        );
    }
    log::line(format_args!("{line}"));
    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-owner string-context pass #{pass} detail #{detail}: marker={label} addr=0x{addr:X} string_start=0x{string_start:X} context='{context}'"
    ));
}

fn scan_c_hkx_pointer_array_owners(pass: usize, target_addrs: &[usize]) {
    if target_addrs.is_empty() {
        log::line(format_args!(
            "[player-scale-no-bone] NR hkx-array scan pass #{pass}: skipped no c.hkx string addresses"
        ));
        return;
    }

    let mut addr = 0x10000usize;
    let max_addr = 0x0000_7FFF_FFFF_FFFFusize;
    let mut regions = 0usize;
    let mut scanned_regions = 0usize;
    let mut scanned_bytes = 0usize;
    let mut arrays = 0usize;
    let mut owner_targets = Vec::new();
    let mut bytes_since_sleep = 0usize;

    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-array scan pass #{pass}: begin targets={} min_targets={HKX_POINTER_ARRAY_MIN_TARGETS}",
        target_addrs.len()
    ));

    while addr < max_addr
        && scanned_bytes < HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS
        && arrays < HKX_POINTER_ARRAY_CANDIDATE_LIMIT_PER_PASS
    {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let result = unsafe {
            VirtualQuery(
                Some(addr as *const _),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if result == 0 {
            break;
        }

        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        let next = base.saturating_add(size);
        regions += 1;

        if is_scan_candidate_region(&mbi) {
            let remaining = HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS.saturating_sub(scanned_bytes);
            let scan_len = size.min(remaining);
            if scan_len >= std::mem::size_of::<usize>() {
                scanned_regions += 1;
                scanned_bytes = scanned_bytes.saturating_add(scan_len);
                bytes_since_sleep = bytes_since_sleep.saturating_add(scan_len);

                let bytes = unsafe { slice::from_raw_parts(base as *const u8, scan_len) };
                arrays += scan_c_hkx_pointer_array_region(
                    pass,
                    base,
                    bytes,
                    &mbi,
                    target_addrs,
                    HKX_POINTER_ARRAY_CANDIDATE_LIMIT_PER_PASS.saturating_sub(arrays),
                    &mut owner_targets,
                );

                if bytes_since_sleep >= HKX_RESOURCE_SCAN_REGION_SLEEP_BYTES {
                    bytes_since_sleep = 0;
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }

        if next <= addr {
            addr = addr.saturating_add(0x1000);
        } else {
            addr = next;
        }
    }

    owner_targets.sort_unstable();
    owner_targets.dedup();
    if owner_targets.len() > HKX_POINTER_ARRAY_OWNER_TARGET_LIMIT {
        owner_targets.truncate(HKX_POINTER_ARRAY_OWNER_TARGET_LIMIT);
    }

    let capped = scanned_bytes >= HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS
        || arrays >= HKX_POINTER_ARRAY_CANDIDATE_LIMIT_PER_PASS;
    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-array scan pass #{pass}: end regions={regions} scanned_regions={scanned_regions} scanned_bytes={scanned_bytes} arrays={arrays} owner_targets={} capped={capped}",
        owner_targets.len()
    ));

    scan_c_hkx_pointer_array_owner_references(pass, &owner_targets);
}

fn scan_c_hkx_pointer_array_region(
    pass: usize,
    base: usize,
    bytes: &[u8],
    mbi: &MEMORY_BASIC_INFORMATION,
    target_addrs: &[usize],
    limit: usize,
    owner_targets: &mut Vec<usize>,
) -> usize {
    if limit == 0 || bytes.len() < std::mem::size_of::<usize>() {
        return 0;
    }

    let end = bytes.len().saturating_sub(std::mem::size_of::<usize>());
    let mut arrays = 0usize;
    let mut index = 0usize;
    while index <= end && arrays < limit {
        let q = read_usize_from_bytes(bytes, index);
        if target_addrs.binary_search(&q).is_err() {
            index = index.saturating_add(std::mem::size_of::<usize>());
            continue;
        }

        let run_start = index;
        let mut run_end = index;
        let mut targets = 0usize;
        while run_end <= end {
            let value = read_usize_from_bytes(bytes, run_end);
            if target_addrs.binary_search(&value).is_err() {
                break;
            }
            targets += 1;
            run_end = run_end.saturating_add(std::mem::size_of::<usize>());
        }

        if targets >= HKX_POINTER_ARRAY_MIN_TARGETS {
            let run_addr = base + run_start;
            let run_bytes = targets.saturating_mul(std::mem::size_of::<usize>());
            log_hkx_pointer_array_candidate(pass, run_addr, targets, run_bytes, mbi);
            collect_hkx_pointer_array_owner_targets(run_addr, owner_targets);
            arrays += 1;
            index = run_end;
        } else {
            index = run_start.saturating_add(std::mem::size_of::<usize>());
        }
    }

    arrays
}

fn collect_hkx_pointer_array_owner_targets(run_addr: usize, owner_targets: &mut Vec<usize>) {
    for target in [
        run_addr.saturating_sub(0x40),
        run_addr.saturating_sub(0x30),
        run_addr.saturating_sub(0x20),
        run_addr.saturating_sub(0x10),
        run_addr,
    ] {
        if target != 0 && owner_targets.len() < HKX_POINTER_ARRAY_OWNER_TARGET_LIMIT {
            owner_targets.push(target);
        }
    }
}

fn log_hkx_pointer_array_candidate(
    pass: usize,
    run_addr: usize,
    targets: usize,
    run_bytes: usize,
    mbi: &MEMORY_BASIC_INFORMATION,
) {
    let start = run_addr.saturating_sub(0x40);
    let mut line = format!(
        "[player-scale-no-bone] NR hkx-array candidate pass #{pass}: run=0x{run_addr:X} targets={targets} run_bytes=0x{run_bytes:X} region=0x{:X} size=0x{:X} protect=0x{:X} type=0x{:X}",
        mbi.BaseAddress as usize, mbi.RegionSize, mbi.Protect.0, mbi.Type.0,
    );
    for offset in (0..=0xC0).step_by(8) {
        let q = read_usize(start + offset);
        let _ = write!(
            line,
            " q{:+X}=0x{q:X}({})",
            offset as isize - 0x40,
            format_module_rva(q)
        );
    }
    log::line(format_args!("{line}"));
}

fn scan_c_hkx_pointer_array_owner_references(pass: usize, owner_targets: &[usize]) {
    if owner_targets.is_empty() {
        log::line(format_args!(
            "[player-scale-no-bone] NR hkx-array owner refs pass #{pass}: skipped no owner targets"
        ));
        return;
    }

    let mut addr = 0x10000usize;
    let max_addr = 0x0000_7FFF_FFFF_FFFFusize;
    let mut regions = 0usize;
    let mut scanned_regions = 0usize;
    let mut scanned_bytes = 0usize;
    let mut refs = 0usize;
    let mut bytes_since_sleep = 0usize;

    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-array owner refs pass #{pass}: begin targets={}",
        owner_targets.len()
    ));

    while addr < max_addr
        && scanned_bytes < HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS
        && refs < HKX_POINTER_ARRAY_OWNER_REF_LIMIT_PER_PASS
    {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let result = unsafe {
            VirtualQuery(
                Some(addr as *const _),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if result == 0 {
            break;
        }

        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        let next = base.saturating_add(size);
        regions += 1;

        if is_scan_candidate_region(&mbi) {
            let remaining = HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS.saturating_sub(scanned_bytes);
            let scan_len = size.min(remaining);
            if scan_len >= std::mem::size_of::<usize>() {
                scanned_regions += 1;
                scanned_bytes = scanned_bytes.saturating_add(scan_len);
                bytes_since_sleep = bytes_since_sleep.saturating_add(scan_len);

                let bytes = unsafe { slice::from_raw_parts(base as *const u8, scan_len) };
                let end = bytes.len().saturating_sub(std::mem::size_of::<usize>());
                for index in (0..=end).step_by(std::mem::size_of::<usize>()) {
                    let q = read_usize_from_bytes(bytes, index);
                    if owner_targets.binary_search(&q).is_ok() {
                        let ref_addr = base + index;
                        log_hkx_pointer_array_owner_reference(pass, ref_addr, q, &mbi);
                        refs += 1;
                        if refs >= HKX_POINTER_ARRAY_OWNER_REF_LIMIT_PER_PASS {
                            break;
                        }
                    }
                }

                if bytes_since_sleep >= HKX_RESOURCE_SCAN_REGION_SLEEP_BYTES {
                    bytes_since_sleep = 0;
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }

        if next <= addr {
            addr = addr.saturating_add(0x1000);
        } else {
            addr = next;
        }
    }

    let capped = scanned_bytes >= HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS
        || refs >= HKX_POINTER_ARRAY_OWNER_REF_LIMIT_PER_PASS;
    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-array owner refs pass #{pass}: end regions={regions} scanned_regions={scanned_regions} scanned_bytes={scanned_bytes} refs={refs} capped={capped}"
    ));
}

fn log_hkx_pointer_array_owner_reference(
    pass: usize,
    ref_addr: usize,
    target_addr: usize,
    mbi: &MEMORY_BASIC_INFORMATION,
) {
    let start = (ref_addr & !0x7).saturating_sub(0x40);
    let mut line = format!(
        "[player-scale-no-bone] NR hkx-array owner ref pass #{pass}: ref=0x{ref_addr:X} target=0x{target_addr:X} region=0x{:X} size=0x{:X} protect=0x{:X} type=0x{:X}",
        mbi.BaseAddress as usize, mbi.RegionSize, mbi.Protect.0, mbi.Type.0,
    );
    for offset in (0..=0x80).step_by(8) {
        let q = read_usize(start + offset);
        let _ = write!(
            line,
            " q{:+X}=0x{q:X}({})",
            offset as isize - 0x40,
            format_module_rva(q)
        );
    }
    log::line(format_args!("{line}"));
}

fn scan_c_hkx_string_references(pass: usize, target_addrs: &[usize]) {
    if target_addrs.is_empty() {
        log::line(format_args!(
            "[player-scale-no-bone] NR hkx-owner refs pass #{pass}: skipped no c.hkx string addresses"
        ));
        return;
    }

    let mut addr = 0x10000usize;
    let max_addr = 0x0000_7FFF_FFFF_FFFFusize;
    let mut regions = 0usize;
    let mut scanned_regions = 0usize;
    let mut scanned_bytes = 0usize;
    let mut refs = 0usize;
    let mut bytes_since_sleep = 0usize;

    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-owner refs pass #{pass}: begin targets={}",
        target_addrs.len()
    ));

    while addr < max_addr
        && scanned_bytes < HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS
        && refs < HKX_TARGET_REFERENCE_LIMIT_PER_PASS
    {
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        let result = unsafe {
            VirtualQuery(
                Some(addr as *const _),
                &mut mbi,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if result == 0 {
            break;
        }

        let base = mbi.BaseAddress as usize;
        let size = mbi.RegionSize;
        let next = base.saturating_add(size);
        regions += 1;

        if is_scan_candidate_region(&mbi) {
            let remaining = HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS.saturating_sub(scanned_bytes);
            let scan_len = size.min(remaining);
            if scan_len >= std::mem::size_of::<usize>() {
                scanned_regions += 1;
                scanned_bytes = scanned_bytes.saturating_add(scan_len);
                bytes_since_sleep = bytes_since_sleep.saturating_add(scan_len);

                let bytes = unsafe { slice::from_raw_parts(base as *const u8, scan_len) };
                let end = bytes.len().saturating_sub(std::mem::size_of::<usize>());
                for index in (0..=end).step_by(std::mem::size_of::<usize>()) {
                    let q = read_usize_from_bytes(bytes, index);
                    if target_addrs.binary_search(&q).is_ok() {
                        let ref_addr = base + index;
                        log_hkx_owner_reference_neighborhood(pass, ref_addr, q, &mbi);
                        refs += 1;
                        if refs >= HKX_TARGET_REFERENCE_LIMIT_PER_PASS {
                            break;
                        }
                    }
                }

                if bytes_since_sleep >= HKX_RESOURCE_SCAN_REGION_SLEEP_BYTES {
                    bytes_since_sleep = 0;
                    thread::sleep(Duration::from_millis(2));
                }
            }
        }

        if next <= addr {
            addr = addr.saturating_add(0x1000);
        } else {
            addr = next;
        }
    }

    let capped = scanned_bytes >= HKX_RESOURCE_SCAN_MAX_BYTES_PER_PASS
        || refs >= HKX_TARGET_REFERENCE_LIMIT_PER_PASS;
    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-owner refs pass #{pass}: end regions={regions} scanned_regions={scanned_regions} scanned_bytes={scanned_bytes} refs={refs} capped={capped}"
    ));
}

fn log_hkx_owner_reference_neighborhood(
    pass: usize,
    ref_addr: usize,
    target_addr: usize,
    mbi: &MEMORY_BASIC_INFORMATION,
) {
    let aligned = ref_addr & !0x7;
    let start = aligned.saturating_sub(0x40);
    let mut line = format!(
        "[player-scale-no-bone] NR hkx-owner ref pass #{pass}: ref=0x{ref_addr:X} target=0x{target_addr:X} region=0x{:X} size=0x{:X} protect=0x{:X} type=0x{:X}",
        mbi.BaseAddress as usize, mbi.RegionSize, mbi.Protect.0, mbi.Type.0,
    );
    for offset in (0..=0x80).step_by(8) {
        let q = read_usize(start + offset);
        let _ = write!(
            line,
            " q{:+X}=0x{q:X}({})",
            offset as isize - 0x40,
            format_module_rva(q)
        );
    }
    log::line(format_args!("{line}"));
}

fn read_usize_from_bytes(bytes: &[u8], index: usize) -> usize {
    let mut raw = [0u8; std::mem::size_of::<usize>()];
    raw.copy_from_slice(&bytes[index..index + std::mem::size_of::<usize>()]);
    usize::from_le_bytes(raw)
}

fn infer_ascii_string_start(bytes: &[u8], index: usize) -> usize {
    let lower_bound = index.saturating_sub(256);
    let mut start = index;
    while start > lower_bound {
        let b = bytes[start - 1];
        if !(b.is_ascii_graphic() || b == b' ' || b == b'\\' || b == b'/') {
            break;
        }
        start -= 1;
    }
    start
}

fn infer_utf16_string_start(bytes: &[u8], index: usize) -> usize {
    let lower_bound = index.saturating_sub(512);
    let mut start = index;
    while start >= lower_bound + 2 {
        let low = bytes[start - 2];
        let high = bytes[start - 1];
        if high != 0 || !(low.is_ascii_graphic() || low == b' ' || low == b'\\' || low == b'/') {
            break;
        }
        start -= 2;
    }
    start
}

fn is_concrete_resource_context(label: &str, context: &str) -> bool {
    label.contains("bnd")
        || label.contains(".hkx")
        || context.contains("_mod\\parts")
        || context.contains("_mod\\chr")
        || context.contains("\\parts\\")
        || context.contains("\\chr\\")
        || context.contains(".partsbnd")
        || context.contains(".chrbnd")
        || context.contains(".anibnd")
}

fn log_resource_path_neighborhood(pass: usize, label: &str, addr: usize, context: &str) {
    let detail = HKX_RESOURCE_PATH_DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail > HKX_RESOURCE_PATH_DETAIL_LIMIT {
        return;
    }

    let aligned = addr & !0x7;
    let start = aligned.saturating_sub(0x40);
    let mut line = format!(
        "[player-scale-no-bone] NR hkx-resource path-neighbor pass #{pass} detail #{detail}: marker={label} addr=0x{addr:X} aligned=0x{aligned:X}"
    );
    for offset in (0..=0x80).step_by(8) {
        let q = read_usize(start + offset);
        let _ = write!(
            line,
            " q{:+X}=0x{q:X}({})",
            offset as isize - 0x40,
            format_module_rva(q)
        );
    }
    log::line(format_args!("{line}"));

    log::line(format_args!(
        "[player-scale-no-bone] NR hkx-resource path-context pass #{pass} detail #{detail}: marker={label} addr=0x{addr:X} context='{context}'"
    ));
}

fn is_scan_candidate_region(mbi: &MEMORY_BASIC_INFORMATION) -> bool {
    if mbi.State != MEM_COMMIT || mbi.Type == MEM_IMAGE {
        return false;
    }

    let protect = mbi.Protect;
    protect == PAGE_READONLY
        || protect == PAGE_READWRITE
        || protect == PAGE_WRITECOPY
        || protect == PAGE_EXECUTE_READ
        || protect == PAGE_EXECUTE_READWRITE
        || protect == PAGE_EXECUTE_WRITECOPY
        || protect == PAGE_EXECUTE
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn sanitize_ascii_context(bytes: &[u8], index: usize) -> String {
    let start = index.saturating_sub(48);
    let end = (index + 96).min(bytes.len());
    bytes[start..end]
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            }
        })
        .collect()
}

fn sanitize_utf16_context(bytes: &[u8], index: usize) -> String {
    let start = index.saturating_sub(64) & !1;
    let end = ((index + 128).min(bytes.len())) & !1;
    let mut text = String::new();
    for pair in bytes[start..end].chunks_exact(2) {
        let value = u16::from_le_bytes([pair[0], pair[1]]);
        let ch = char::from_u32(value as u32).unwrap_or('.');
        if ch.is_ascii_graphic() || ch == ' ' {
            text.push(ch);
        } else if ch == '\0' {
            text.push('.');
        } else {
            text.push('?');
        }
    }
    text
}

fn utf16_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn start_late_correlation_thread() {
    if NR_LATE_CORRELATION_THREAD_STARTED.swap(1, Ordering::AcqRel) != 0 {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR scale-phys late correlation worker scheduled after first scale-copy"
    ));

    thread::spawn(|| {
        for (index, seconds) in [1_u64, 3, 5].iter().copied().enumerate() {
            thread::sleep(Duration::from_secs(seconds));
            let pass = 100 + index;
            dump_scale_phys_correlations(pass);
        }
    });
}

fn dump_observed_runtime_summary(pass: usize) {
    dump_observed_cloth_model_summary(pass);
    dump_observed_phys_owner_summary(pass);
    dump_scale_phys_correlations(pass);
}

fn dump_observed_cloth_model_summary(pass: usize) {
    let objects = {
        let observed = OBSERVED_CLOTH_MODELS.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(objects) = observed.lock() else {
            return;
        };
        objects.clone()
    };

    if objects.is_empty() {
        if pass == 1 || pass == DUMP_PASSES {
            log::line(format_args!(
                "[player-scale-no-bone] NR phys-real-entry delayed summary pass #{pass}: no CSClothModelIns roots observed yet"
            ));
        }
        return;
    }

    for owner in objects {
        let ptr = read_usize(owner + 0x88);
        let q0 = read_usize(ptr);
        log::line(format_args!(
            "[player-scale-no-bone] NR phys-real-entry delayed summary pass #{pass}: CSClothModelIns=0x{owner:X} +0x88=0x{ptr:X} q0=0x{q0:X}({})",
            format_module_rva(q0)
        ));
    }
}

fn dump_observed_phys_owner_summary(pass: usize) {
    let objects = {
        let observed = OBSERVED_PHYS_OWNERS.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(objects) = observed.lock() else {
            return;
        };
        objects.clone()
    };

    if objects.is_empty() {
        if pass == 1 || pass == DUMP_PASSES {
            log::line(format_args!(
                "[player-scale-no-bone] NR phys-real-entry delayed summary pass #{pass}: no phys owner init roots observed yet"
            ));
        }
        return;
    }

    for owner in objects {
        let phys = read_usize(owner + 0x30);
        let q0 = read_usize(phys);
        let owner_20 = read_usize(owner + 0x20);
        let owner_28 = read_usize(owner + 0x28);
        let owner_90 = read_usize(owner + 0x90);
        let owner_98 = read_usize(owner + 0x98);
        let owner_a8 = read_usize(owner + 0xA8);
        log::line(format_args!(
            "[player-scale-no-bone] NR phys-owner-neighbor delayed summary pass #{pass}: phys_owner=0x{owner:X} +0x30=0x{phys:X} q0=0x{q0:X}({}) owner20=0x{owner_20:X} owner28=0x{owner_28:X} owner90=0x{owner_90:X} owner98=0x{owner_98:X} ownerA8=0x{owner_a8:X}",
            format_module_rva(q0),
        ));

        log_owner_neighbor_head(pass, owner, 0x20, owner_20);
        log_owner_neighbor_head(pass, owner, 0x28, owner_28);
        log_owner_neighbor_head(pass, owner, 0x30, phys);
        log_owner_neighbor_head(pass, owner, 0x90, owner_90);
        log_owner_neighbor_head(pass, owner, 0x98, owner_98);
        log_owner_neighbor_head(pass, owner, 0xA8, owner_a8);
        log_phys_world_children(pass, owner_98);
        log_havok_buffer_children(pass, owner_90);
        log_vtable_summary(pass, "CSPhysSysIns", phys);
        log_vtable_summary(pass, "CSHavokBufferCapacity", owner_90);
        log_vtable_summary(pass, "CSPhysWorld", owner_98);
        log_vtable_summary(
            pass,
            "CSPhysWorld+0x8 world-core",
            read_usize(owner_98 + 0x8),
        );
        log_fixed_field_pointer_summary(pass, "CSPhysSysIns", phys, 0x180);
        log_fixed_field_pointer_summary(pass, "CSHavokBufferCapacity", owner_90, 0x90);
        log_fixed_field_pointer_summary(pass, "CSPhysWorld", owner_98, 0x90);

        if phys != 0 && is_readable_memory(phys, 0x80) {
            log_qwords(
                pass,
                "real-entry likely CSPhysSysIns+0x30 head",
                owner,
                0x30,
                phys,
                &[
                    0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60,
                    0x68, 0x70, 0x78,
                ],
            );
        }
    }
}

fn log_vtable_summary(pass: usize, label: &str, object: usize) {
    if object == 0 || !is_readable_memory(object, 0x8) {
        return;
    }

    let vtable = read_usize(object);
    if !is_module_pointer(vtable) || !is_readable_memory(vtable, VTABLE_SUMMARY_ENTRY_COUNT * 8) {
        return;
    }

    let detail = VTABLE_SUMMARY_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail > VTABLE_SUMMARY_LOG_LIMIT {
        return;
    }

    let mut line = format!(
        "[player-scale-no-bone] NR vtable-summary pass #{pass} detail #{detail}: {label} object=0x{object:X} vtable=0x{vtable:X}({})",
        format_module_rva(vtable),
    );
    for index in 0..VTABLE_SUMMARY_ENTRY_COUNT {
        let entry = read_usize(vtable + index * 8);
        let _ = write!(
            line,
            " m{index:02}=0x{entry:X}({})",
            format_module_rva(entry)
        );
    }
    log::line(format_args!("{line}"));
}

fn log_fixed_field_pointer_summary(pass: usize, label: &str, object: usize, scan_len: usize) {
    if object == 0 || !is_readable_memory(object, scan_len.min(0x20)) {
        return;
    }

    for offset in (0..scan_len).step_by(8) {
        let ptr = read_usize(object + offset);
        if ptr == 0 || ptr == object || !is_readable_memory(ptr, 0x40) {
            continue;
        }

        let detail = FIELD_SUMMARY_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if detail > FIELD_SUMMARY_LOG_LIMIT {
            return;
        }

        let q0 = read_usize(ptr);
        let q8 = read_usize(ptr + 0x8);
        let q10 = read_usize(ptr + 0x10);
        let pointer_count = raw_region_pointer_count(ptr, 0x80);
        let module_count = raw_region_module_pointer_count(ptr, 0x80);
        let finite_vec_count = raw_region_finite_vec_count(ptr, 0x80);
        let small_id_count = raw_region_small_id_count(ptr, 0x80);

        log::line(format_args!(
            "[player-scale-no-bone] NR field-summary pass #{pass} detail #{detail}: {label}=0x{object:X}+0x{offset:X} ptr=0x{ptr:X} q0=0x{q0:X}({}) q8=0x{q8:X} q10=0x{q10:X} ptrCount80={pointer_count} moduleCount80={module_count} finiteVec80={finite_vec_count} smallId80={small_id_count}",
            format_module_rva(q0),
        ));
    }
}

fn log_fixed_physics_child_deep_summary(
    pass: usize,
    phys_sys: usize,
    havok_capacity: usize,
    phys_world: usize,
) {
    for &offset in &[
        0x18usize, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x130, 0x140, 0xF8, 0x158,
    ] {
        let child = read_usize(phys_sys + offset);
        log_fixed_child_deep(pass, "CSPhysSysIns", phys_sys, offset, child);
    }

    for &offset in &[0x18usize, 0x38, 0x68] {
        let child = read_usize(phys_world + offset);
        log_fixed_child_deep(pass, "CSPhysWorld", phys_world, offset, child);
    }

    for &offset in &[0x80usize, 0x88] {
        let child = read_usize(havok_capacity + offset);
        log_fixed_child_deep(pass, "CSHavokBufferCapacity", havok_capacity, offset, child);
    }
}

fn log_fixed_child_deep(pass: usize, owner_label: &str, owner: usize, offset: usize, child: usize) {
    if child == 0 || !is_readable_memory(child, 0x80) {
        return;
    }

    let detail = FIXED_CHILD_DEEP_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail > FIXED_CHILD_DEEP_LOG_LIMIT {
        return;
    }

    let pointer_count = raw_region_pointer_count(child, 0x100);
    let module_count = raw_region_module_pointer_count(child, 0x100);
    let finite_vec_count = raw_region_finite_vec_count(child, 0x100);
    let small_id_count = raw_region_small_id_count(child, 0x100);
    let nan_count = raw_region_nan_count(child, 0x100);

    let mut line = format!(
        "[player-scale-no-bone] NR fixed-child pass #{pass} detail #{detail}: {owner_label}=0x{owner:X}+0x{offset:X} child=0x{child:X} ptrCount100={pointer_count} moduleCount100={module_count} finiteVec100={finite_vec_count} smallId100={small_id_count} nan100={nan_count}"
    );
    for qoff in (0..=0x80).step_by(8) {
        let q = read_usize(child + qoff);
        let _ = write!(line, " q{qoff:X}=0x{q:X}({})", format_module_rva(q));
    }
    log::line(format_args!("{line}"));

    log_fixed_child_float_preview(pass, detail, owner_label, owner, offset, child);
    log_fixed_child_ascii_preview(pass, detail, owner_label, owner, offset, child);
    log_fixed_child_entry_preview(pass, detail, owner_label, owner, offset, child);
}

fn log_fixed_child_float_preview(
    pass: usize,
    detail: usize,
    owner_label: &str,
    owner: usize,
    offset: usize,
    child: usize,
) {
    let mut line = format!(
        "[player-scale-no-bone] NR fixed-child-f32 pass #{pass} detail #{detail}: {owner_label}=0x{owner:X}+0x{offset:X} child=0x{child:X}"
    );
    for foff in [0x0usize, 0x10, 0x20, 0x30, 0x40, 0x60, 0x80] {
        if !is_readable_memory(child + foff, 0x10) {
            continue;
        }
        let row = read_vec4(child + foff);
        let _ = write!(
            line,
            " f{foff:X}=({:.4},{:.4},{:.4},{:.4})",
            row[0], row[1], row[2], row[3]
        );
    }
    log::line(format_args!("{line}"));
}

fn log_fixed_child_ascii_preview(
    pass: usize,
    detail: usize,
    owner_label: &str,
    owner: usize,
    offset: usize,
    child: usize,
) {
    if !is_readable_memory(child, 0x80) {
        return;
    }

    let bytes = unsafe { slice::from_raw_parts(child as *const u8, 0x80) };
    if !bytes.iter().any(|byte| byte.is_ascii_alphabetic()) {
        return;
    }

    let text: String = bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            }
        })
        .collect();

    log::line(format_args!(
        "[player-scale-no-bone] NR fixed-child-ascii pass #{pass} detail #{detail}: {owner_label}=0x{owner:X}+0x{offset:X} child=0x{child:X} text='{text}'"
    ));
}

fn log_fixed_child_entry_preview(
    pass: usize,
    detail: usize,
    owner_label: &str,
    owner: usize,
    offset: usize,
    child: usize,
) {
    let mut emitted = 0usize;
    for entry_offset in (0..=0x40).step_by(8) {
        let entry = read_usize(child + entry_offset);
        if entry == 0 || entry == child || entry == owner || !is_readable_memory(entry, 0x40) {
            continue;
        }

        let q0 = read_usize(entry);
        let q8 = read_usize(entry + 0x8);
        let q10 = read_usize(entry + 0x10);
        let ptr_count = raw_region_pointer_count(entry, 0x80);
        let module_count = raw_region_module_pointer_count(entry, 0x80);
        let finite_vec_count = raw_region_finite_vec_count(entry, 0x80);
        log::line(format_args!(
            "[player-scale-no-bone] NR fixed-child-entry pass #{pass} detail #{detail}.{emitted}: {owner_label}=0x{owner:X}+0x{offset:X} child=0x{child:X}+0x{entry_offset:X} entry=0x{entry:X} q0=0x{q0:X}({}) q8=0x{q8:X} q10=0x{q10:X} ptrCount80={ptr_count} moduleCount80={module_count} finiteVec80={finite_vec_count}",
            format_module_rva(q0),
        ));

        emitted += 1;
        if emitted >= 4 {
            break;
        }
    }
}

fn log_live_child_buffer_pool_sampler(pass: usize, havok_capacity: usize, phys_world: usize) {
    if phys_world != 0 && is_readable_memory(phys_world, 0x80) {
        let world_child = read_usize(phys_world + 0x18);
        log_live_child_target(
            pass,
            "CSPhysWorld+0x18",
            phys_world,
            0x18,
            world_child,
            &[0x18, 0x38],
        );
    }

    if havok_capacity != 0 && is_readable_memory(havok_capacity, 0x90) {
        let capacity_child = read_usize(havok_capacity + 0x80);
        log_live_child_target(
            pass,
            "CSHavokBufferCapacity+0x80",
            havok_capacity,
            0x80,
            capacity_child,
            &[0x28, 0x30, 0x38, 0x40],
        );
    }
}

fn log_live_child_target(
    pass: usize,
    label: &str,
    owner: usize,
    owner_offset: usize,
    target: usize,
    nested_offsets: &[usize],
) {
    if target == 0 || !is_readable_memory(target, 0x50) {
        return;
    }

    log_live_child_head(pass, label, owner, owner_offset, target);
    scan_live_child_candidate_bases(pass, label, target);

    for &nested_offset in nested_offsets {
        let nested = read_usize(target + nested_offset);
        if nested == 0 || !is_readable_memory(nested, 0x50) {
            continue;
        }

        let nested_label = format!("{label}->+0x{nested_offset:X}");
        log_live_child_head(pass, &nested_label, target, nested_offset, nested);
        scan_live_child_candidate_bases(pass, &nested_label, nested);
    }
}

fn log_live_child_head(pass: usize, label: &str, owner: usize, owner_offset: usize, target: usize) {
    let detail = LIVE_CHILD_POOL_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail > LIVE_CHILD_POOL_LOG_LIMIT {
        return;
    }

    let q0 = read_usize(target);
    let pointer_count = raw_region_pointer_count(target, 0x100);
    let module_count = raw_region_module_pointer_count(target, 0x100);
    let finite_vec_count = raw_region_finite_vec_count(target, 0x100);
    let small_id_count = raw_region_small_id_count(target, 0x100);
    let mut line = format!(
        "[player-scale-no-bone] NR live-child-head pass #{pass} detail #{detail}: {label} owner=0x{owner:X}+0x{owner_offset:X} target=0x{target:X} q0=0x{q0:X}({}) ptrCount100={pointer_count} moduleCount100={module_count} finiteVec100={finite_vec_count} smallId100={small_id_count}",
        format_module_rva(q0),
    );

    for qoff in (0..=0x50).step_by(8) {
        let q = read_usize(target + qoff);
        let _ = write!(line, " q{qoff:X}=0x{q:X}({})", format_module_rva(q));
    }
    log::line(format_args!("{line}"));
}

fn scan_live_child_candidate_bases(pass: usize, label: &str, target: usize) {
    scan_live_child_pool_base(pass, label, target, usize::MAX, target);

    for field_offset in [
        0x0usize, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x60, 0x68, 0x70,
        0x78, 0x80,
    ] {
        if !is_readable_memory(target + field_offset, 0x8) {
            continue;
        }

        let base = read_usize(target + field_offset);
        if base == 0 || base == target || is_module_pointer(base) || !is_readable_memory(base, 0x80)
        {
            continue;
        }

        scan_live_child_pool_base(pass, label, target, field_offset, base);
    }
}

fn scan_live_child_pool_base(
    pass: usize,
    label: &str,
    target: usize,
    source_offset: usize,
    base: usize,
) {
    if base == 0 || is_module_pointer(base) || !is_readable_memory(base, 0x80) {
        return;
    }

    let source = if source_offset == usize::MAX {
        "self".to_string()
    } else {
        format!("q0x{source_offset:X}")
    };

    for stride in [0x20usize, 0x30, 0x40, 0x50, 0x60, 0x80, 0xA0] {
        if !is_readable_memory(base, stride) {
            continue;
        }

        let mut scanned = 0usize;
        let mut body_hits = 0usize;
        let mut motion_hits = 0usize;
        let mut best_body_score = 0usize;
        let mut best_motion_score = 0usize;

        for index in 0..LIVE_CHILD_POOL_MAX_ENTRIES {
            let entry = base.saturating_add(index.saturating_mul(stride));
            if !is_readable_memory(entry, stride.min(0x80)) {
                continue;
            }
            scanned += 1;

            let body_score = hknp_body_live_score(entry);
            if body_score >= 5 {
                best_body_score = best_body_score.max(body_score);
                if body_hits < LIVE_CHILD_POOL_HIT_SAMPLE_LIMIT {
                    let sample_label =
                        format!("{label} {source} stride=0x{stride:X} index={index}");
                    log_hknp_body_sample(pass, &sample_label, entry);
                }
                body_hits += 1;
            }

            let motion_score = hknp_motion_live_score(entry);
            if motion_score >= 4 {
                best_motion_score = best_motion_score.max(motion_score);
                if motion_hits < LIVE_CHILD_POOL_HIT_SAMPLE_LIMIT {
                    let sample_label =
                        format!("{label} {source} stride=0x{stride:X} index={index}");
                    log_hknp_motion_sample(pass, &sample_label, entry);
                }
                motion_hits += 1;
            }
        }

        if body_hits == 0 && motion_hits == 0 {
            continue;
        }

        let detail = LIVE_CHILD_POOL_LOG_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if detail > LIVE_CHILD_POOL_LOG_LIMIT {
            return;
        }

        log::line(format_args!(
            "[player-scale-no-bone] NR live-child-pool scan pass #{pass} detail #{detail}: {label} target=0x{target:X} source={source} base=0x{base:X} stride=0x{stride:X} scanned={scanned} bodyHits={body_hits} bestBodyScore={best_body_score} motionHits={motion_hits} bestMotionScore={best_motion_score}"
        ));
    }
}

fn log_owner_neighbor_head(pass: usize, owner: usize, offset: usize, ptr: usize) {
    if ptr == 0 {
        return;
    }

    if !is_readable_memory(ptr, 0x40) {
        log::line(format_args!(
            "[player-scale-no-bone] NR phys-owner-neighbor pass #{pass}: owner+0x{offset:X}=0x{ptr:X} unreadable"
        ));
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-owner-neighbor pass #{pass}: owner=0x{owner:X}+0x{offset:X} ptr=0x{ptr:X} q0=0x{:X}({}) q8=0x{:X} q10=0x{:X} q18=0x{:X} q20=0x{:X} q28=0x{:X} q30=0x{:X}",
        read_usize(ptr),
        format_module_rva(read_usize(ptr)),
        read_usize(ptr + 0x8),
        read_usize(ptr + 0x10),
        read_usize(ptr + 0x18),
        read_usize(ptr + 0x20),
        read_usize(ptr + 0x28),
        read_usize(ptr + 0x30),
    ));
}

fn log_phys_world_children(pass: usize, phys_world: usize) {
    if phys_world == 0 || !is_readable_memory(phys_world, 0x70) {
        return;
    }

    let world_core = read_usize(phys_world + 0x8);
    log_child_head(pass, "CSPhysWorld+0x8", phys_world, 0x8, world_core);
    log_child_head(
        pass,
        "CSPhysWorld+0x10",
        phys_world,
        0x10,
        read_usize(phys_world + 0x10),
    );
    log_child_head(
        pass,
        "CSPhysWorld+0x68",
        phys_world,
        0x68,
        read_usize(phys_world + 0x68),
    );
}

fn dump_scale_phys_correlations(pass: usize) {
    let scale_owners = {
        let observed = OBSERVED_SCALE_OWNERS.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(objects) = observed.lock() else {
            return;
        };
        objects.clone()
    };
    let phys_owners = {
        let observed = OBSERVED_PHYS_OWNERS.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(objects) = observed.lock() else {
            return;
        };
        objects.clone()
    };

    if scale_owners.is_empty() {
        if pass == 1 || pass == DUMP_PASSES {
            log::line(format_args!(
                "[player-scale-no-bone] NR scale-phys correlation pass #{pass}: no scale owners observed yet"
            ));
        }
        return;
    }

    if phys_owners.is_empty() {
        if pass == 1 || pass == DUMP_PASSES {
            log::line(format_args!(
                "[player-scale-no-bone] NR scale-phys correlation pass #{pass}: scaleOwners={} but no phys owners observed yet",
                scale_owners.len()
            ));
        }
        return;
    }

    for scale_owner in scale_owners {
        let active_6a4 = read_f32(scale_owner.owner + 0x6A4);
        let active_6b8 = read_f32(scale_owner.owner + 0x6B8);
        log::line(format_args!(
            "[player-scale-no-bone] NR scale-owner summary pass #{pass}: owner=0x{:X} source=0x{:X} sourceScale={:.4} active6A4={active_6a4:.4} active6B8={active_6b8:.4}",
            scale_owner.owner, scale_owner.source, scale_owner.scale,
        ));

        for phys_owner in &phys_owners {
            log_scale_phys_pair(pass, scale_owner, *phys_owner, active_6a4);
        }
    }
}

fn log_scale_phys_pair(
    pass: usize,
    scale_owner: ScaleOwnerObservation,
    phys_owner: usize,
    active_scale: f32,
) {
    let phys_ins = read_usize(phys_owner + 0x30);
    let havok_capacity = read_usize(phys_owner + 0x90);
    let phys_world = read_usize(phys_owner + 0x98);

    let scale_to_phys_owner =
        scan_qword_for_pointer(scale_owner.owner, phys_owner, CORRELATION_QWORD_SCAN_SIZE);
    let scale_to_phys_ins =
        scan_qword_for_pointer(scale_owner.owner, phys_ins, CORRELATION_QWORD_SCAN_SIZE);
    let scale_to_phys_world =
        scan_qword_for_pointer(scale_owner.owner, phys_world, CORRELATION_QWORD_SCAN_SIZE);
    let phys_owner_to_scale =
        scan_qword_for_pointer(phys_owner, scale_owner.owner, CORRELATION_QWORD_SCAN_SIZE);
    let phys_ins_to_scale =
        scan_qword_for_pointer(phys_ins, scale_owner.owner, CORRELATION_QWORD_SCAN_SIZE);

    let phys_owner_scale =
        scan_f32_for_value(phys_owner, active_scale, CORRELATION_FLOAT_SCAN_SIZE);
    let phys_ins_scale = scan_f32_for_value(phys_ins, active_scale, CORRELATION_FLOAT_SCAN_SIZE);
    let havok_capacity_scale =
        scan_f32_for_value(havok_capacity, active_scale, CORRELATION_FLOAT_SCAN_SIZE);
    let phys_world_scale =
        scan_f32_for_value(phys_world, active_scale, CORRELATION_FLOAT_SCAN_SIZE);

    log::line(format_args!(
        "[player-scale-no-bone] NR scale-phys correlation pass #{pass}: scaleOwner=0x{:X} scale={active_scale:.4} physOwner=0x{phys_owner:X} physIns=0x{phys_ins:X} havokCapacity=0x{havok_capacity:X} physWorld=0x{phys_world:X} scaleOwner->physOwner={} scaleOwner->physIns={} scaleOwner->physWorld={} physOwner->scaleOwner={} physIns->scaleOwner={} physOwnerScale={} physInsScale={} havokCapacityScale={} physWorldScale={}",
        scale_owner.owner,
        format_optional_offset(scale_to_phys_owner),
        format_optional_offset(scale_to_phys_ins),
        format_optional_offset(scale_to_phys_world),
        format_optional_offset(phys_owner_to_scale),
        format_optional_offset(phys_ins_to_scale),
        format_optional_offset(phys_owner_scale),
        format_optional_offset(phys_ins_scale),
        format_optional_offset(havok_capacity_scale),
        format_optional_offset(phys_world_scale),
    ));

    if pass >= 100 && (active_scale - 1.0).abs() > 0.001 {
        log_scale_owner_neighbor_summary(
            pass,
            scale_owner.owner,
            active_scale,
            phys_owner,
            phys_ins,
            havok_capacity,
            phys_world,
        );
    }
}

fn log_scale_owner_neighbor_summary(
    pass: usize,
    owner: usize,
    active_scale: f32,
    phys_owner: usize,
    phys_ins: usize,
    havok_capacity: usize,
    phys_world: usize,
) {
    if owner == 0 || !is_readable_memory(owner, 0x700) {
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR scale-owner-neighbor pass #{pass}: owner=0x{owner:X} scale={active_scale:.4} ownerQ0=0x{:X}({}) f6A4={:.4} f6B8={:.4} f6E4={:.4}",
        read_usize(owner),
        format_module_rva(read_usize(owner)),
        read_f32(owner + 0x6A4),
        read_f32(owner + 0x6B8),
        read_f32(owner + 0x6E4),
    ));

    for &offset in SCALE_OWNER_NEIGHBOR_OFFSETS {
        let ptr = read_usize(owner + offset);
        if ptr == 0 {
            continue;
        }

        let detail = SCALE_OWNER_NEIGHBOR_DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if detail > SCALE_OWNER_NEIGHBOR_DETAIL_LIMIT {
            return;
        }

        if !is_readable_memory(ptr, 0x50) {
            log::line(format_args!(
                "[player-scale-no-bone] NR scale-owner-neighbor pass #{pass} detail #{detail}: owner+0x{offset:X}=0x{ptr:X} unreadable"
            ));
            continue;
        }

        let ptr_to_phys_owner =
            scan_qword_for_pointer(ptr, phys_owner, SCALE_OWNER_NEIGHBOR_SCAN_SIZE);
        let ptr_to_phys_ins = scan_qword_for_pointer(ptr, phys_ins, SCALE_OWNER_NEIGHBOR_SCAN_SIZE);
        let ptr_to_havok_capacity =
            scan_qword_for_pointer(ptr, havok_capacity, SCALE_OWNER_NEIGHBOR_SCAN_SIZE);
        let ptr_to_phys_world =
            scan_qword_for_pointer(ptr, phys_world, SCALE_OWNER_NEIGHBOR_SCAN_SIZE);
        let scale_match = scan_f32_for_value(ptr, active_scale, SCALE_OWNER_NEIGHBOR_SCAN_SIZE);

        let q0 = read_usize(ptr);
        log::line(format_args!(
            "[player-scale-no-bone] NR scale-owner-neighbor pass #{pass} detail #{detail}: owner=0x{owner:X}+0x{offset:X} ptr=0x{ptr:X} q0=0x{q0:X}({}) q8=0x{:X} q10=0x{:X} q18=0x{:X} q20=0x{:X} q28=0x{:X} q30=0x{:X} q38=0x{:X} q40=0x{:X} ->physOwner={} ->physIns={} ->havokCapacity={} ->physWorld={} scaleMatch={}",
            format_module_rva(q0),
            read_usize(ptr + 0x8),
            read_usize(ptr + 0x10),
            read_usize(ptr + 0x18),
            read_usize(ptr + 0x20),
            read_usize(ptr + 0x28),
            read_usize(ptr + 0x30),
            read_usize(ptr + 0x38),
            read_usize(ptr + 0x40),
            format_optional_offset(ptr_to_phys_owner),
            format_optional_offset(ptr_to_phys_ins),
            format_optional_offset(ptr_to_havok_capacity),
            format_optional_offset(ptr_to_phys_world),
            format_optional_offset(scale_match),
        ));
    }
}

fn log_hknp_world_core_summary(pass: usize, world_core: usize) {
    if world_core == 0 || !is_readable_memory(world_core, HKNP_WORLD_CORE_SIZE) {
        return;
    }

    log_qwords(
        pass,
        "CSPhysWorld+0x8 hknp world-core head",
        world_core,
        0,
        world_core,
        &[
            0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x68, 0x70,
            0x78, 0x80, 0xA0, 0xC0, 0xE0, 0x100, 0x140, 0x180, 0x1C0, 0x200, 0x280, 0x300, 0x380,
            0x400, 0x480, 0x500, 0x580, 0x600, 0x680, 0x700, 0x800, 0x900, 0xA00, 0xB00,
        ],
    );
    log_float_rows(
        pass,
        "CSPhysWorld+0x8 hknp world-core rows",
        world_core,
        &[0x20, 0x30, 0x70, 0x100, 0x180, 0x280, 0x380],
    );
    log_world_core_pointer_map(pass, world_core);
    log_world_core_candidate_children(pass, world_core);
    log_world_core_4d8_deep(pass, world_core);
}

fn log_world_core_array_scans(pass: usize, world_core: usize) {
    let pool_28 = read_usize(world_core + 0x28);
    let count_28 = read_u32(world_core + 0x30) as usize;
    let pool_180 = read_usize(world_core + 0x180);
    let count_180 = read_u32(world_core + 0x188) as usize;

    log_world_core_raw_pool_layout(
        pass,
        "world_core+0x28 raw pool",
        world_core,
        0x28,
        pool_28,
        count_28,
    );
    log_world_core_raw_pool_layout(
        pass,
        "world_core+0x180 raw pool",
        world_core,
        0x180,
        pool_180,
        count_180,
    );
}

fn log_world_core_raw_pool_layout(
    pass: usize,
    label: &str,
    world_core: usize,
    offset: usize,
    pool: usize,
    count_hint: usize,
) {
    if pool == 0 {
        log::line(format_args!(
            "[player-scale-no-bone] NR raw-pool pass #{pass} {label} core=0x{world_core:X}+0x{offset:X} pool=null count_hint=0x{count_hint:X}"
        ));
        return;
    }

    if !is_readable_memory(pool, 0x20) {
        log::line(format_args!(
            "[player-scale-no-bone] NR raw-pool pass #{pass} {label} core=0x{world_core:X}+0x{offset:X} pool=0x{pool:X} unreadable count_hint=0x{count_hint:X}"
        ));
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR raw-pool pass #{pass} {label} core=0x{world_core:X}+0x{offset:X} pool=0x{pool:X} count_hint=0x{count_hint:X} q0=0x{:X} q8=0x{:X} q10=0x{:X} q18=0x{:X} q20=0x{:X} q28=0x{:X} q30=0x{:X} q38=0x{:X}",
        read_usize(pool),
        read_usize(pool + 0x8),
        read_usize(pool + 0x10),
        read_usize(pool + 0x18),
        read_usize(pool + 0x20),
        read_usize(pool + 0x28),
        read_usize(pool + 0x30),
        read_usize(pool + 0x38),
    ));

    log_world_core_raw_rows(pass, label, world_core, offset, pool, count_hint);

    for stride in WORLD_CORE_STRIDE_CANDIDATES {
        log_world_core_stride_summary(pass, label, world_core, offset, pool, count_hint, stride);
    }
}

fn log_world_core_raw_rows(
    pass: usize,
    label: &str,
    world_core: usize,
    offset: usize,
    pool: usize,
    count_hint: usize,
) {
    let row_count = if count_hint == 0 {
        WORLD_CORE_RAW_ROW_LIMIT
    } else {
        count_hint.min(WORLD_CORE_RAW_ROW_LIMIT)
    };

    for row in 0..row_count {
        let entry = pool.saturating_add(row.saturating_mul(0x20));
        if !is_readable_memory(entry, 0x20) {
            continue;
        }

        let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if detail_count > DETAIL_LIMIT {
            return;
        }

        let f0 = read_vec4(entry);
        let f10 = read_vec4(entry + 0x10);
        log::line(format_args!(
            "[player-scale-no-bone] NR raw-row pass #{pass} detail #{detail_count} {label} core=0x{world_core:X}+0x{offset:X} pool=0x{pool:X} row20={row} entry=0x{entry:X} q0=0x{:X} q8=0x{:X} q10=0x{:X} q18=0x{:X} f0=({:.4},{:.4},{:.4},{:.4}) f10=({:.4},{:.4},{:.4},{:.4})",
            read_usize(entry),
            read_usize(entry + 0x8),
            read_usize(entry + 0x10),
            read_usize(entry + 0x18),
            f0[0],
            f0[1],
            f0[2],
            f0[3],
            f10[0],
            f10[1],
            f10[2],
            f10[3],
        ));
    }
}

fn log_world_core_stride_summary(
    pass: usize,
    label: &str,
    world_core: usize,
    offset: usize,
    pool: usize,
    count_hint: usize,
    stride: usize,
) {
    if stride == 0 {
        return;
    }

    let scan_count = if count_hint == 0 {
        WORLD_CORE_ARRAY_SCAN_ENTRIES
    } else {
        count_hint.min(WORLD_CORE_ARRAY_SCAN_ENTRIES)
    };
    let sample_len = stride.clamp(0x10, 0xA0);

    let mut scanned = 0usize;
    let mut non_empty = 0usize;
    let mut sentinel_heavy = 0usize;
    let mut readable_ptr = 0usize;
    let mut module_ptr = 0usize;
    let mut small_id = 0usize;
    let mut finite_vec = 0usize;
    let mut nan_rows = 0usize;
    let mut first_ptr = 0usize;
    let mut first_module_ptr = 0usize;

    for index in 0..scan_count {
        let entry = pool.saturating_add(index.saturating_mul(stride));
        if !is_readable_memory(entry, sample_len) {
            continue;
        }

        scanned += 1;
        if raw_record_non_empty(entry, sample_len) {
            non_empty += 1;
        }
        if raw_record_sentinel_heavy(entry, sample_len) {
            sentinel_heavy += 1;
        }
        if let Some(ptr) = raw_record_first_readable_pointer(entry, sample_len) {
            readable_ptr += 1;
            if first_ptr == 0 {
                first_ptr = ptr;
            }
        }
        if let Some(ptr) = raw_record_first_module_pointer(entry, sample_len) {
            module_ptr += 1;
            if first_module_ptr == 0 {
                first_module_ptr = ptr;
            }
        }
        if raw_record_has_small_id(entry, sample_len) {
            small_id += 1;
        }
        if raw_record_has_finite_vec(entry, sample_len) {
            finite_vec += 1;
        }
        if raw_record_has_nan(entry, sample_len) {
            nan_rows += 1;
        }
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR raw-pool stride-summary pass #{pass} {label} core=0x{world_core:X}+0x{offset:X} pool=0x{pool:X} count_hint=0x{count_hint:X} stride=0x{stride:X} scanned={scanned} nonEmpty={non_empty} sentinelHeavy={sentinel_heavy} readablePtr={readable_ptr} modulePtr={module_ptr} smallId={small_id} finiteVec={finite_vec} nanRows={nan_rows} firstPtr=0x{first_ptr:X}({}) firstModulePtr=0x{first_module_ptr:X}({})",
        format_module_rva(first_ptr),
        format_module_rva(first_module_ptr),
    ));
}

fn raw_record_non_empty(entry: usize, len: usize) -> bool {
    for offset in (0..len).step_by(8) {
        let q = read_usize(entry + offset);
        if q != 0 && !raw_qword_is_sentinel(q) {
            return true;
        }
    }
    false
}

fn raw_record_sentinel_heavy(entry: usize, len: usize) -> bool {
    let mut qwords = 0usize;
    let mut sentinels = 0usize;
    for offset in (0..len).step_by(8) {
        qwords += 1;
        let q = read_usize(entry + offset);
        if raw_qword_is_sentinel(q) || raw_qword_has_sentinel_lane(q) {
            sentinels += 1;
        }
    }
    qwords != 0 && sentinels * 2 >= qwords
}

fn raw_record_first_readable_pointer(entry: usize, len: usize) -> Option<usize> {
    for offset in (0..len).step_by(8) {
        let q = read_usize(entry + offset);
        if q != entry && is_readable_memory(q, 0x10) {
            return Some(q);
        }
    }
    None
}

fn raw_record_first_module_pointer(entry: usize, len: usize) -> Option<usize> {
    for offset in (0..len).step_by(8) {
        let q = read_usize(entry + offset);
        if is_module_pointer(q) {
            return Some(q);
        }
    }
    None
}

fn raw_record_has_small_id(entry: usize, len: usize) -> bool {
    for offset in (0..len).step_by(4) {
        let value = read_u32(entry + offset);
        if value != 0 && value != 0xFFFF_FFFF && value != 0x00FF_FFFF && value <= 0x10000 {
            return true;
        }
    }
    false
}

fn raw_record_has_finite_vec(entry: usize, len: usize) -> bool {
    if len < 0x10 {
        return false;
    }
    for offset in (0..=len - 0x10).step_by(0x10) {
        let row = read_vec4(entry + offset);
        if vec4_finite_plausible(row, 100000.0) && !vec4_near_zero(row) {
            return true;
        }
    }
    false
}

fn raw_record_has_nan(entry: usize, len: usize) -> bool {
    for offset in (0..len).step_by(4) {
        if read_f32(entry + offset).is_nan() {
            return true;
        }
    }
    false
}

fn raw_qword_is_sentinel(value: usize) -> bool {
    value == usize::MAX
        || value == 0x7FFFFF7FFF7FFFFF
        || value == 0x8000000000000000
        || value == 0xFFFFFFFF00000000
        || value == 0x00000000FFFFFFFF
}

fn raw_qword_has_sentinel_lane(value: usize) -> bool {
    let lo = value as u32;
    let hi = (value >> 32) as u32;
    matches!(lo, 0xFFFF_FFFF | 0x00FF_FFFF | 0x7FFFFF7F | 0x80000000)
        || matches!(hi, 0xFFFF_FFFF | 0x00FF_FFFF | 0x7FFFFF7F | 0x80000000)
}

fn is_module_pointer(value: usize) -> bool {
    let base = NR_MODULE_BASE.load(Ordering::Acquire);
    base != 0 && value >= base && value - base < 0x8000000
}

fn log_world_core_body_pool_scan(
    pass: usize,
    label: &str,
    world_core: usize,
    offset: usize,
    pool: usize,
    count: usize,
    stride: usize,
    max_entries: usize,
) {
    if pool == 0 || stride == 0 || !is_readable_memory(pool, stride) {
        return;
    }

    let scan_count = count.min(max_entries);
    let mut hits = 0usize;
    let mut scanned = 0usize;
    for index in 0..scan_count {
        let entry = pool.saturating_add(index.saturating_mul(stride));
        if !is_readable_memory(entry, 0xA0) {
            continue;
        }
        scanned += 1;
        let score = hknp_body_live_score(entry);
        if score < 3 {
            continue;
        }
        if hits < WORLD_CORE_ARRAY_OUTPUT_LIMIT {
            log_hknp_body_pool_entry(pass, label, world_core, offset, pool, index, entry, score);
        }
        hits += 1;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR hknp array-scan pass #{pass} {label} core=0x{world_core:X}+0x{offset:X} pool=0x{pool:X} count_hint=0x{count:X} stride=0x{stride:X} scanned={scanned} hits={hits}"
    ));
}

fn log_world_core_motion_pool_scan(
    pass: usize,
    label: &str,
    world_core: usize,
    offset: usize,
    pool: usize,
    count: usize,
    stride: usize,
    max_entries: usize,
) {
    if pool == 0 || stride == 0 || !is_readable_memory(pool, stride) {
        return;
    }

    let scan_count = count.min(max_entries);
    let mut hits = 0usize;
    let mut scanned = 0usize;
    for index in 0..scan_count {
        let entry = pool.saturating_add(index.saturating_mul(stride));
        if !is_readable_memory(entry, 0x80) {
            continue;
        }
        scanned += 1;
        let score = hknp_motion_live_score(entry);
        if score < 3 {
            continue;
        }
        if hits < WORLD_CORE_ARRAY_OUTPUT_LIMIT {
            log_hknp_motion_pool_entry(pass, label, world_core, offset, pool, index, entry, score);
        }
        hits += 1;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR hknp array-scan pass #{pass} {label} core=0x{world_core:X}+0x{offset:X} pool=0x{pool:X} count_hint=0x{count:X} stride=0x{stride:X} scanned={scanned} hits={hits}"
    ));
}

fn log_hknp_body_pool_entry(
    pass: usize,
    label: &str,
    world_core: usize,
    offset: usize,
    pool: usize,
    index: usize,
    entry: usize,
    score: usize,
) {
    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let t0 = read_vec4(entry);
    let t1 = read_vec4(entry + 0x10);
    let t2 = read_vec4(entry + 0x20);
    let t3 = read_vec4(entry + 0x30);
    let motion_id = read_u32(entry + 0x40);
    let flags = read_u32(entry + 0x44);
    let shape = read_usize(entry + 0x60);
    let id = read_u32(entry + 0x70);
    let next_body = read_u32(entry + 0x78);
    let user_data = read_usize(entry + 0x90);
    let radius = read_f32(entry + 0x9C);

    log::line(format_args!(
        "[player-scale-no-bone] NR hknp body-pool pass #{pass} detail #{detail_count} {label} core=0x{world_core:X}+0x{offset:X} pool=0x{pool:X} index={index} entry=0x{entry:X} liveScore={score} t0=({:.3},{:.3},{:.3},{:.3}) t1=({:.3},{:.3},{:.3},{:.3}) t2=({:.3},{:.3},{:.3},{:.3}) t3=({:.3},{:.3},{:.3},{:.3}) motionId40=0x{motion_id:X} flags44=0x{flags:X} shape60=0x{shape:X}({}) id70=0x{id:X} nextBody78=0x{next_body:X} userData90=0x{user_data:X} radius9C={radius:.4}",
        t0[0],
        t0[1],
        t0[2],
        t0[3],
        t1[0],
        t1[1],
        t1[2],
        t1[3],
        t2[0],
        t2[1],
        t2[2],
        t2[3],
        t3[0],
        t3[1],
        t3[2],
        t3[3],
        format_module_rva(shape),
    ));
}

fn log_hknp_motion_pool_entry(
    pass: usize,
    label: &str,
    world_core: usize,
    offset: usize,
    pool: usize,
    index: usize,
    entry: usize,
    score: usize,
) {
    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let center = read_vec4(entry);
    let orientation = read_vec4(entry + 0x10);
    let inverse_inertia = read_vec4(entry + 0x20);
    let first_body = read_u32(entry + 0x28);
    let velocity = read_vec4(entry + 0x40);
    let angular = read_vec4(entry + 0x50);

    log::line(format_args!(
        "[player-scale-no-bone] NR hknp motion-pool pass #{pass} detail #{detail_count} {label} core=0x{world_core:X}+0x{offset:X} pool=0x{pool:X} index={index} entry=0x{entry:X} liveScore={score} center=({:.3},{:.3},{:.3},{:.3}) orientation=({:.3},{:.3},{:.3},{:.3}) invInertia20=({:.3},{:.3},{:.3},{:.3}) firstBody28=0x{first_body:X} linVel40=({:.3},{:.3},{:.3},{:.3}) angVel50=({:.3},{:.3},{:.3},{:.3})",
        center[0],
        center[1],
        center[2],
        center[3],
        orientation[0],
        orientation[1],
        orientation[2],
        orientation[3],
        inverse_inertia[0],
        inverse_inertia[1],
        inverse_inertia[2],
        inverse_inertia[3],
        velocity[0],
        velocity[1],
        velocity[2],
        velocity[3],
        angular[0],
        angular[1],
        angular[2],
        angular[3],
    ));
}

fn hknp_body_live_score(entry: usize) -> usize {
    if entry_is_empty_or_sentinel(entry, 0xA0) {
        return 0;
    }

    let mut score = 0usize;
    let transform_rows = [
        read_vec4(entry),
        read_vec4(entry + 0x10),
        read_vec4(entry + 0x20),
        read_vec4(entry + 0x30),
    ];
    if transform_rows
        .iter()
        .all(|row| vec4_finite_plausible(*row, 100000.0))
    {
        score += 1;
    }
    if transform_rows.iter().any(|row| !vec4_near_zero(*row)) {
        score += 1;
    }

    let motion_id = read_u32(entry + 0x40);
    if plausible_pool_id(motion_id) && motion_id != 0 {
        score += 1;
    }

    let shape = read_usize(entry + 0x60);
    if non_sentinel_usize(shape) {
        score += 1;
        if is_readable_memory(shape, 0x20) {
            score += 2;
        }
    }

    let id = read_u32(entry + 0x70);
    if plausible_pool_id(id) && id != 0 && id != 0x00FF_FFFF {
        score += 1;
    }

    let user_data = read_usize(entry + 0x90);
    if non_sentinel_usize(user_data) {
        score += 1;
    }

    if read_f32(entry + 0x9C).is_finite() {
        score += 1;
    }

    score
}

fn hknp_motion_live_score(entry: usize) -> usize {
    if entry_is_empty_or_sentinel(entry, 0x80) {
        return 0;
    }

    let mut score = 0usize;
    let center = read_vec4(entry);
    let orientation = read_vec4(entry + 0x10);
    let inverse_inertia = read_vec4(entry + 0x20);
    let first_body = read_u32(entry + 0x28);
    let velocity = read_vec4(entry + 0x40);
    let angular = read_vec4(entry + 0x50);

    if vec4_finite_plausible(center, 100000.0) && !vec4_near_zero(center) {
        score += 1;
    }
    if quaternion_plausible(orientation) {
        score += 2;
    }
    if vec4_finite_plausible(inverse_inertia, 100000.0) && !vec4_near_zero(inverse_inertia) {
        score += 1;
    }
    if plausible_pool_id(first_body) && first_body != 0 && first_body != 0x00FF_FFFF {
        score += 1;
    }
    if vec4_finite_plausible(velocity, 100000.0) && !vec4_near_zero(velocity) {
        score += 1;
    }
    if vec4_finite_plausible(angular, 100000.0) && !vec4_near_zero(angular) {
        score += 1;
    }

    score
}

fn entry_is_empty_or_sentinel(entry: usize, len: usize) -> bool {
    let qword_count = len / 8;
    let mut non_empty = 0usize;
    for index in 0..qword_count {
        let q = read_usize(entry + index * 8);
        if q != 0 && q != usize::MAX && q != 0x7FFFFF7FFF7FFFFF {
            non_empty += 1;
        }
    }
    non_empty == 0
}

fn plausible_pool_id(value: u32) -> bool {
    value == 0xFFFF_FFFF || value <= 0x00FF_FFFF
}

fn non_sentinel_usize(value: usize) -> bool {
    value != 0 && value != usize::MAX && value != 0x7FFFFF7FFF7FFFFF
}

fn log_world_core_pointer_map(pass: usize, world_core: usize) {
    let mut readable_fields = 0usize;
    let mut module_fields = 0usize;
    let mut target_pointer_rich = 0usize;

    for offset in (0..HKNP_WORLD_CORE_SIZE.saturating_sub(0x10)).step_by(8) {
        let ptr = read_usize(world_core + offset);
        if ptr == 0 {
            continue;
        }

        let readable = is_readable_memory(ptr, 0x40);
        let module_ptr = is_module_pointer(ptr);
        if !readable && !module_ptr {
            continue;
        }

        readable_fields += readable as usize;
        module_fields += module_ptr as usize;

        if readable && raw_region_pointer_count(ptr, 0x80) >= 2 {
            target_pointer_rich += 1;
        }
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR world-core pointer-focus pass #{pass}: core=0x{world_core:X} scanned_size=0x{HKNP_WORLD_CORE_SIZE:X} readableFields={readable_fields} moduleFields={module_fields} pointerRichTargets={target_pointer_rich}"
    ));

    let mut logged = 0usize;
    for offset in WORLD_CORE_POINTER_FOCUS_OFFSETS {
        if offset >= HKNP_WORLD_CORE_SIZE {
            continue;
        }

        let ptr = read_usize(world_core + offset);
        let readable = is_readable_memory(ptr, 0x40);
        let module_ptr = is_module_pointer(ptr);
        logged += 1;
        log_world_core_pointer_map_entry(
            pass, world_core, offset, ptr, logged, readable, module_ptr,
        );
    }

    let mut extra_logged = 0usize;
    for offset in (0..HKNP_WORLD_CORE_SIZE.saturating_sub(0x10)).step_by(8) {
        if extra_logged >= WORLD_CORE_POINTER_EXTRA_LIMIT {
            break;
        }
        if is_world_core_focus_offset(offset) || is_early_world_core_chain_offset(offset) {
            continue;
        }

        let ptr = read_usize(world_core + offset);
        if ptr == 0 {
            continue;
        }

        let readable = is_readable_memory(ptr, 0x40);
        let module_ptr = is_module_pointer(ptr);
        if !readable && !module_ptr {
            continue;
        }

        let pointer_count = if readable {
            raw_region_pointer_count(ptr, 0x80)
        } else {
            0
        };
        let module_count = if readable {
            raw_region_module_pointer_count(ptr, 0x80)
        } else {
            0
        };
        let finite_vec_count = if readable {
            raw_region_finite_vec_count(ptr, 0x80)
        } else {
            0
        };

        if pointer_count < 4 && module_count == 0 && finite_vec_count == 0 {
            continue;
        }

        extra_logged += 1;
        logged += 1;
        log_world_core_pointer_map_entry(
            pass, world_core, offset, ptr, logged, readable, module_ptr,
        );
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR world-core pointer-focus pass #{pass}: core=0x{world_core:X} focusLogged={} extraLogged={extra_logged} totalLogged={logged}",
        WORLD_CORE_POINTER_FOCUS_OFFSETS.len()
    ));
}

fn is_world_core_focus_offset(offset: usize) -> bool {
    WORLD_CORE_POINTER_FOCUS_OFFSETS.contains(&offset)
}

fn is_early_world_core_chain_offset(offset: usize) -> bool {
    (0x1A0..=0x3D0).contains(&offset)
}

fn log_world_core_pointer_map_entry(
    pass: usize,
    world_core: usize,
    offset: usize,
    ptr: usize,
    index: usize,
    readable: bool,
    module_ptr: bool,
) {
    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let target_q0 = if readable { read_usize(ptr) } else { 0 };
    let inline_size = read_u32(world_core + offset + 0x8);
    let inline_cap = read_u32(world_core + offset + 0xC);

    let q8 = if readable { read_usize(ptr + 0x8) } else { 0 };
    let q10 = if readable { read_usize(ptr + 0x10) } else { 0 };
    let q18 = if readable { read_usize(ptr + 0x18) } else { 0 };
    let q20 = if readable { read_usize(ptr + 0x20) } else { 0 };
    let q28 = if readable { read_usize(ptr + 0x28) } else { 0 };
    let q30 = if readable { read_usize(ptr + 0x30) } else { 0 };
    let pointer_count = if readable {
        raw_region_pointer_count(ptr, 0x80)
    } else {
        0
    };
    let module_count = if readable {
        raw_region_module_pointer_count(ptr, 0x80)
    } else {
        0
    };
    let finite_vec_count = if readable {
        raw_region_finite_vec_count(ptr, 0x80)
    } else {
        0
    };
    let small_id_count = if readable {
        raw_region_small_id_count(ptr, 0x80)
    } else {
        0
    };
    let nan_count = if readable {
        raw_region_nan_count(ptr, 0x80)
    } else {
        0
    };

    log::line(format_args!(
        "[player-scale-no-bone] NR world-core pointer-map pass #{pass} detail #{detail_count} entry #{index} core=0x{world_core:X}+0x{offset:X} ptr=0x{ptr:X} ptrKind={} targetQ0=0x{target_q0:X}({}) inlineNextU32=0x{inline_size:X}/0x{inline_cap:X} targetQ8=0x{q8:X} targetQ10=0x{q10:X} targetQ18=0x{q18:X} targetQ20=0x{q20:X} targetQ28=0x{q28:X} targetQ30=0x{q30:X} targetPtrCount80={pointer_count} targetModulePtrCount80={module_count} finiteVec80={finite_vec_count} smallId80={small_id_count} nan32Count80={nan_count}",
        pointer_kind(readable, module_ptr),
        format_module_rva(target_q0),
    ));
}

fn log_world_core_candidate_children(pass: usize, world_core: usize) {
    for offset in WORLD_CORE_CANDIDATE_CHILD_OFFSETS {
        let target = read_usize(world_core + offset);
        log_world_core_candidate_target(pass, world_core, offset, target);
    }
}

fn log_world_core_candidate_target(pass: usize, world_core: usize, offset: usize, target: usize) {
    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let readable = is_readable_memory(target, 0x100);
    let module_ptr = is_module_pointer(target);
    if target == 0 || (!readable && !module_ptr) {
        log::line(format_args!(
            "[player-scale-no-bone] NR world-core child-target pass #{pass} detail #{detail_count} core=0x{world_core:X}+0x{offset:X} target=0x{target:X} ptrKind={} unreadable-or-null",
            pointer_kind(readable, module_ptr)
        ));
        return;
    }

    let q0 = if readable { read_usize(target) } else { 0 };
    let q8 = if readable {
        read_usize(target + 0x8)
    } else {
        0
    };
    let q10 = if readable {
        read_usize(target + 0x10)
    } else {
        0
    };
    let q18 = if readable {
        read_usize(target + 0x18)
    } else {
        0
    };
    let q20 = if readable {
        read_usize(target + 0x20)
    } else {
        0
    };
    let q28 = if readable {
        read_usize(target + 0x28)
    } else {
        0
    };
    let q30 = if readable {
        read_usize(target + 0x30)
    } else {
        0
    };
    let q38 = if readable {
        read_usize(target + 0x38)
    } else {
        0
    };
    let q40 = if readable {
        read_usize(target + 0x40)
    } else {
        0
    };
    let q48 = if readable {
        read_usize(target + 0x48)
    } else {
        0
    };
    let q50 = if readable {
        read_usize(target + 0x50)
    } else {
        0
    };
    let q58 = if readable {
        read_usize(target + 0x58)
    } else {
        0
    };
    let q60 = if readable {
        read_usize(target + 0x60)
    } else {
        0
    };
    let q68 = if readable {
        read_usize(target + 0x68)
    } else {
        0
    };
    let q70 = if readable {
        read_usize(target + 0x70)
    } else {
        0
    };
    let q78 = if readable {
        read_usize(target + 0x78)
    } else {
        0
    };
    let pointer_count = if readable {
        raw_region_pointer_count(target, 0x100)
    } else {
        0
    };
    let module_count = if readable {
        raw_region_module_pointer_count(target, 0x100)
    } else {
        0
    };
    let finite_vec_count = if readable {
        raw_region_finite_vec_count(target, 0x100)
    } else {
        0
    };
    let small_id_count = if readable {
        raw_region_small_id_count(target, 0x100)
    } else {
        0
    };
    let nan_count = if readable {
        raw_region_nan_count(target, 0x100)
    } else {
        0
    };

    log::line(format_args!(
        "[player-scale-no-bone] NR world-core child-target pass #{pass} detail #{detail_count} core=0x{world_core:X}+0x{offset:X} target=0x{target:X} ptrKind={} q0=0x{q0:X}({}) q8=0x{q8:X} q10=0x{q10:X} q18=0x{q18:X} q20=0x{q20:X} q28=0x{q28:X} q30=0x{q30:X} q38=0x{q38:X} q40=0x{q40:X} q48=0x{q48:X} q50=0x{q50:X} q58=0x{q58:X} q60=0x{q60:X} q68=0x{q68:X} q70=0x{q70:X} q78=0x{q78:X} ptrCount100={pointer_count} modulePtrCount100={module_count} finiteVec100={finite_vec_count} smallId100={small_id_count} nan32Count100={nan_count}",
        pointer_kind(readable, module_ptr),
        format_module_rva(q0),
    ));

    if readable {
        log_world_core_candidate_child_slots(pass, world_core, offset, target);
    }
}

fn log_world_core_candidate_child_slots(
    pass: usize,
    world_core: usize,
    owner_offset: usize,
    target: usize,
) {
    let mut logged = 0usize;

    for &child_offset in world_core_child_slot_offsets(owner_offset) {
        if logged >= WORLD_CORE_CHILD_SLOT_LOG_LIMIT {
            break;
        }

        let child = read_usize(target + child_offset);
        if child == 0 || child == target {
            continue;
        }

        let readable = is_readable_memory(child, 0x80);
        let module_ptr = is_module_pointer(child);
        if !readable && !module_ptr {
            continue;
        }

        let child_q0 = if readable { read_usize(child) } else { 0 };
        let pointer_count = if readable {
            raw_region_pointer_count(child, 0x80)
        } else {
            0
        };
        let module_count = if readable {
            raw_region_module_pointer_count(child, 0x80)
        } else {
            0
        };
        let finite_vec_count = if readable {
            raw_region_finite_vec_count(child, 0x80)
        } else {
            0
        };
        let small_id_count = if readable {
            raw_region_small_id_count(child, 0x80)
        } else {
            0
        };
        let nan_count = if readable {
            raw_region_nan_count(child, 0x80)
        } else {
            0
        };

        logged += 1;
        let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if detail_count > DETAIL_LIMIT {
            return;
        }

        log::line(format_args!(
            "[player-scale-no-bone] NR world-core child-slot pass #{pass} detail #{detail_count} core=0x{world_core:X}+0x{owner_offset:X} target=0x{target:X}+0x{child_offset:X} child=0x{child:X} childKind={} childQ0=0x{child_q0:X}({}) childQ8=0x{:X} childQ10=0x{:X} childQ18=0x{:X} childQ20=0x{:X} ptrCount80={pointer_count} modulePtrCount80={module_count} finiteVec80={finite_vec_count} smallId80={small_id_count} nan32Count80={nan_count}",
            pointer_kind(readable, module_ptr),
            format_module_rva(child_q0),
            if readable { read_usize(child + 0x8) } else { 0 },
            if readable {
                read_usize(child + 0x10)
            } else {
                0
            },
            if readable {
                read_usize(child + 0x18)
            } else {
                0
            },
            if readable {
                read_usize(child + 0x20)
            } else {
                0
            },
        ));
    }
}

fn log_world_core_4d8_deep(pass: usize, world_core: usize) {
    let target = read_usize(world_core + 0x4D8);
    if target == 0 || !is_readable_memory(target, 0x80) {
        return;
    }

    let child_8 = read_usize(target + 0x8);
    log_world_core_4d8_child_head(pass, world_core, target, 0x8, child_8);
    log_world_core_4d8_float_rows(pass, world_core, target, 0x8, child_8);

    let list_child = read_usize(target + 0x10);
    log_world_core_4d8_child_head(pass, world_core, target, 0x10, list_child);
    log_world_core_4d8_pointer_list(pass, world_core, target, 0x10, list_child);

    for &node_offset in WORLD_CORE_4D8_NODE_OFFSETS {
        let node = read_usize(target + node_offset);
        log_world_core_4d8_node(pass, world_core, target, node_offset, node);
    }
}

fn log_world_core_4d8_child_head(
    pass: usize,
    world_core: usize,
    target: usize,
    child_offset: usize,
    child: usize,
) {
    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    if child == 0 || !is_readable_memory(child, 0x80) {
        log::line(format_args!(
            "[player-scale-no-bone] NR world-core 4D8-deep pass #{pass} detail #{detail_count} core=0x{world_core:X} target=0x{target:X}+0x{child_offset:X} child=0x{child:X} unreadable-or-null"
        ));
        return;
    }

    let q0 = read_usize(child);
    let q8 = read_usize(child + 0x8);
    let q10 = read_usize(child + 0x10);
    let q18 = read_usize(child + 0x18);
    let q20 = read_usize(child + 0x20);
    let q28 = read_usize(child + 0x28);
    let q30 = read_usize(child + 0x30);
    let q38 = read_usize(child + 0x38);
    let q40 = read_usize(child + 0x40);
    let q48 = read_usize(child + 0x48);
    let q50 = read_usize(child + 0x50);
    let q58 = read_usize(child + 0x58);
    let q60 = read_usize(child + 0x60);
    let q68 = read_usize(child + 0x68);
    let q70 = read_usize(child + 0x70);
    let q78 = read_usize(child + 0x78);
    let pointer_count = raw_region_pointer_count(child, 0x80);
    let module_count = raw_region_module_pointer_count(child, 0x80);
    let finite_vec_count = raw_region_finite_vec_count(child, 0x80);
    let small_id_count = raw_region_small_id_count(child, 0x80);
    let nan_count = raw_region_nan_count(child, 0x80);

    log::line(format_args!(
        "[player-scale-no-bone] NR world-core 4D8-deep pass #{pass} detail #{detail_count} core=0x{world_core:X} target=0x{target:X}+0x{child_offset:X} child=0x{child:X} q0=0x{q0:X}({}) q8=0x{q8:X} q10=0x{q10:X} q18=0x{q18:X} q20=0x{q20:X}({}) q28=0x{q28:X}({}) q30=0x{q30:X} q38=0x{q38:X} q40=0x{q40:X} q48=0x{q48:X} q50=0x{q50:X} q58=0x{q58:X} q60=0x{q60:X} q68=0x{q68:X} q70=0x{q70:X} q78=0x{q78:X} ptrCount80={pointer_count} modulePtrCount80={module_count} finiteVec80={finite_vec_count} smallId80={small_id_count} nan32Count80={nan_count}",
        format_module_rva(q0),
        format_module_rva(q20),
        format_module_rva(q28),
    ));
}

fn log_world_core_4d8_float_rows(
    pass: usize,
    world_core: usize,
    target: usize,
    child_offset: usize,
    child: usize,
) {
    if child == 0 || !is_readable_memory(child, 0x80) {
        return;
    }

    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let f0 = read_vec4(child);
    let f10 = read_vec4(child + 0x10);
    let f20 = read_vec4(child + 0x20);
    let f30 = read_vec4(child + 0x30);
    let f40 = read_vec4(child + 0x40);
    let f50 = read_vec4(child + 0x50);

    log::line(format_args!(
        "[player-scale-no-bone] NR world-core 4D8-f32 pass #{pass} detail #{detail_count} core=0x{world_core:X} target=0x{target:X}+0x{child_offset:X} child=0x{child:X} f0=({:.4},{:.4},{:.4},{:.4}) f10=({:.4},{:.4},{:.4},{:.4}) f20=({:.4},{:.4},{:.4},{:.4}) f30=({:.4},{:.4},{:.4},{:.4}) f40=({:.4},{:.4},{:.4},{:.4}) f50=({:.4},{:.4},{:.4},{:.4})",
        f0[0],
        f0[1],
        f0[2],
        f0[3],
        f10[0],
        f10[1],
        f10[2],
        f10[3],
        f20[0],
        f20[1],
        f20[2],
        f20[3],
        f30[0],
        f30[1],
        f30[2],
        f30[3],
        f40[0],
        f40[1],
        f40[2],
        f40[3],
        f50[0],
        f50[1],
        f50[2],
        f50[3],
    ));
}

fn log_world_core_4d8_pointer_list(
    pass: usize,
    world_core: usize,
    target: usize,
    child_offset: usize,
    list: usize,
) {
    if list == 0 || !is_readable_memory(list, 0x80) {
        return;
    }

    for entry_offset in (0..0x40).step_by(8) {
        let entry = read_usize(list + entry_offset);
        if entry == 0 || entry == list || !is_readable_memory(entry, 0x40) {
            continue;
        }

        let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if detail_count > DETAIL_LIMIT {
            return;
        }

        let q0 = read_usize(entry);
        let q8 = read_usize(entry + 0x8);
        let q10 = read_usize(entry + 0x10);
        let q18 = read_usize(entry + 0x18);
        let q20 = read_usize(entry + 0x20);
        let q28 = read_usize(entry + 0x28);
        let q30 = read_usize(entry + 0x30);
        let pointer_count = raw_region_pointer_count(entry, 0x40);
        let finite_vec_count = raw_region_finite_vec_count(entry, 0x40);

        log::line(format_args!(
            "[player-scale-no-bone] NR world-core 4D8-list pass #{pass} detail #{detail_count} core=0x{world_core:X} target=0x{target:X}+0x{child_offset:X} list=0x{list:X}+0x{entry_offset:X} entry=0x{entry:X} q0=0x{q0:X}({}) q8=0x{q8:X} q10=0x{q10:X} q18=0x{q18:X} q20=0x{q20:X}({}) q28=0x{q28:X}({}) q30=0x{q30:X} ptrCount40={pointer_count} finiteVec40={finite_vec_count}",
            format_module_rva(q0),
            format_module_rva(q20),
            format_module_rva(q28),
        ));
    }
}

fn log_world_core_4d8_node(
    pass: usize,
    world_core: usize,
    target: usize,
    node_offset: usize,
    node: usize,
) {
    log_world_core_4d8_child_head(pass, world_core, target, node_offset, node);
    log_world_core_4d8_float_rows(pass, world_core, target, node_offset, node);

    if node == 0 || !is_readable_memory(node, 0x40) {
        return;
    }

    for &nested_offset in &[0x0, 0x8, 0x18, 0x20, 0x28] {
        let nested = read_usize(node + nested_offset);
        if nested == 0 || nested == node || !is_readable_memory(nested, 0x30) {
            continue;
        }

        let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if detail_count > DETAIL_LIMIT {
            return;
        }

        let q0 = read_usize(nested);
        let q8 = read_usize(nested + 0x8);
        let q10 = read_usize(nested + 0x10);
        let q18 = read_usize(nested + 0x18);
        let q20 = read_usize(nested + 0x20);

        log::line(format_args!(
            "[player-scale-no-bone] NR world-core 4D8-node-child pass #{pass} detail #{detail_count} core=0x{world_core:X} target=0x{target:X}+0x{node_offset:X} node=0x{node:X}+0x{nested_offset:X} nested=0x{nested:X} q0=0x{q0:X}({}) q8=0x{q8:X} q10=0x{q10:X} q18=0x{q18:X} q20=0x{q20:X}({})",
            format_module_rva(q0),
            format_module_rva(q20),
        ));
    }
}

fn world_core_child_slot_offsets(owner_offset: usize) -> &'static [usize] {
    match owner_offset {
        0x488 => WORLD_CORE_CHILD_SLOTS_488,
        0x4B8 => WORLD_CORE_CHILD_SLOTS_4B8,
        0x4C0 => WORLD_CORE_CHILD_SLOTS_4C0,
        0x4C8 | 0x4D0 | 0x4D8 => WORLD_CORE_CHILD_SLOTS_CLUSTER,
        _ => WORLD_CORE_CHILD_SLOTS_CLUSTER,
    }
}

fn pointer_kind(readable: bool, module_ptr: bool) -> &'static str {
    match (readable, module_ptr) {
        (true, true) => "readable,module",
        (true, false) => "readable",
        (false, true) => "module",
        (false, false) => "unknown",
    }
}

fn raw_region_pointer_count(ptr: usize, len: usize) -> usize {
    let mut count = 0usize;
    for offset in (0..len).step_by(8) {
        let q = read_usize(ptr + offset);
        if q != ptr && is_readable_memory(q, 0x10) {
            count += 1;
        }
    }
    count
}

fn raw_region_module_pointer_count(ptr: usize, len: usize) -> usize {
    let mut count = 0usize;
    for offset in (0..len).step_by(8) {
        if is_module_pointer(read_usize(ptr + offset)) {
            count += 1;
        }
    }
    count
}

fn raw_region_finite_vec_count(ptr: usize, len: usize) -> usize {
    if len < 0x10 {
        return 0;
    }
    let mut count = 0usize;
    for offset in (0..=len - 0x10).step_by(0x10) {
        let row = read_vec4(ptr + offset);
        if vec4_finite_plausible(row, 100000.0) && !vec4_near_zero(row) {
            count += 1;
        }
    }
    count
}

fn raw_region_small_id_count(ptr: usize, len: usize) -> usize {
    let mut count = 0usize;
    for offset in (0..len).step_by(4) {
        let value = read_u32(ptr + offset);
        if value != 0 && value != 0xFFFF_FFFF && value != 0x00FF_FFFF && value <= 0x10000 {
            count += 1;
        }
    }
    count
}

fn raw_region_nan_count(ptr: usize, len: usize) -> usize {
    let mut count = 0usize;
    for offset in (0..len).step_by(4) {
        if read_f32(ptr + offset).is_nan() {
            count += 1;
        }
    }
    count
}

fn log_hknp_body_sample(pass: usize, label: &str, ptr: usize) {
    if !is_readable_memory(ptr, 0xA0) {
        return;
    }

    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let t0 = read_vec4(ptr);
    let t1 = read_vec4(ptr + 0x10);
    let t2 = read_vec4(ptr + 0x20);
    let t3 = read_vec4(ptr + 0x30);
    let motion_id = read_u32(ptr + 0x40);
    let flags = read_u32(ptr + 0x44);
    let shape = read_usize(ptr + 0x60);
    let id = read_u32(ptr + 0x70);
    let next_body = read_u32(ptr + 0x78);
    let user_data = read_usize(ptr + 0x90);
    let radius_bits = read_u32(ptr + 0x9C);
    let radius = read_f32(ptr + 0x9C);

    log::line(format_args!(
        "[player-scale-no-bone] NR hknp sample pass #{pass} detail #{detail_count} {label} ptr=0x{ptr:X} t0=({:.3},{:.3},{:.3},{:.3}) t1=({:.3},{:.3},{:.3},{:.3}) t2=({:.3},{:.3},{:.3},{:.3}) t3=({:.3},{:.3},{:.3},{:.3}) motionId40=0x{motion_id:X} flags44=0x{flags:X} shape60=0x{shape:X}({}) id70=0x{id:X} nextBody78=0x{next_body:X} userData90=0x{user_data:X} radius9C={radius:.4}/0x{radius_bits:X}",
        t0[0],
        t0[1],
        t0[2],
        t0[3],
        t1[0],
        t1[1],
        t1[2],
        t1[3],
        t2[0],
        t2[1],
        t2[2],
        t2[3],
        t3[0],
        t3[1],
        t3[2],
        t3[3],
        format_module_rva(shape),
    ));
}

fn log_hknp_motion_sample(pass: usize, label: &str, ptr: usize) {
    if !is_readable_memory(ptr, 0x80) {
        return;
    }

    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let center = read_vec4(ptr);
    let orientation = read_vec4(ptr + 0x10);
    let inverse_inertia = read_vec4(ptr + 0x20);
    let first_body = read_u32(ptr + 0x28);
    let velocity = read_vec4(ptr + 0x40);
    let angular = read_vec4(ptr + 0x50);

    log::line(format_args!(
        "[player-scale-no-bone] NR hknp sample pass #{pass} detail #{detail_count} {label} ptr=0x{ptr:X} center=({:.3},{:.3},{:.3},{:.3}) orientation=({:.3},{:.3},{:.3},{:.3}) invInertia20=({:.3},{:.3},{:.3},{:.3}) firstBody28=0x{first_body:X} linVel40=({:.3},{:.3},{:.3},{:.3}) angVel50=({:.3},{:.3},{:.3},{:.3})",
        center[0],
        center[1],
        center[2],
        center[3],
        orientation[0],
        orientation[1],
        orientation[2],
        orientation[3],
        inverse_inertia[0],
        inverse_inertia[1],
        inverse_inertia[2],
        inverse_inertia[3],
        velocity[0],
        velocity[1],
        velocity[2],
        velocity[3],
        angular[0],
        angular[1],
        angular[2],
        angular[3],
    ));
}

fn hknp_body_sample_score(ptr: usize) -> usize {
    if !is_readable_memory(ptr, 0xA0) {
        return 0;
    }

    let mut score = 0usize;
    for row in [0x0, 0x10, 0x20, 0x30] {
        if vec4_finite_plausible(read_vec4(ptr + row), 100000.0) {
            score += 1;
        }
    }
    let shape = read_usize(ptr + 0x60);
    if shape == 0 || is_readable_memory(shape, 0x20) {
        score += 1;
    }
    let id = read_u32(ptr + 0x70);
    if id == 0xFFFF_FFFF || id < 0x100000 {
        score += 1;
    }
    if read_f32(ptr + 0x9C).is_finite() {
        score += 1;
    }
    score
}

fn hknp_motion_sample_score(ptr: usize) -> usize {
    if !is_readable_memory(ptr, 0x80) {
        return 0;
    }

    let mut score = 0usize;
    if vec4_finite_plausible(read_vec4(ptr), 100000.0) {
        score += 1;
    }
    if vec4_finite_plausible(read_vec4(ptr + 0x10), 10.0) {
        score += 1;
    }
    if vec4_finite_plausible(read_vec4(ptr + 0x20), 100000.0) {
        score += 1;
    }
    let first_body = read_u32(ptr + 0x28);
    if first_body == 0xFFFF_FFFF || first_body < 0x100000 {
        score += 1;
    }
    if vec4_finite_plausible(read_vec4(ptr + 0x40), 100000.0) {
        score += 1;
    }
    if vec4_finite_plausible(read_vec4(ptr + 0x50), 100000.0) {
        score += 1;
    }
    score
}

fn read_vec4(addr: usize) -> [f32; 4] {
    [
        read_f32(addr),
        read_f32(addr + 0x4),
        read_f32(addr + 0x8),
        read_f32(addr + 0xC),
    ]
}

fn vec4_finite_plausible(values: [f32; 4], limit: f32) -> bool {
    values
        .iter()
        .all(|value| value.is_finite() && value.abs() <= limit)
}

fn vec4_near_zero(values: [f32; 4]) -> bool {
    values.iter().all(|value| value.abs() <= 0.0001)
}

fn quaternion_plausible(values: [f32; 4]) -> bool {
    if !vec4_finite_plausible(values, 2.0) {
        return false;
    }

    let len_sq = values.iter().map(|value| value * value).sum::<f32>();
    (0.5..=1.5).contains(&len_sq)
}

fn log_havok_buffer_children(pass: usize, buffer_capacity: usize) {
    if buffer_capacity == 0 || !is_readable_memory(buffer_capacity, 0x70) {
        return;
    }

    for offset in [0x8, 0x18, 0x28, 0x48, 0x58, 0x68] {
        log_child_head(
            pass,
            "CSHavokBufferCapacity",
            buffer_capacity,
            offset,
            read_usize(buffer_capacity + offset),
        );
    }
}

fn log_child_head(pass: usize, label: &str, parent: usize, offset: usize, ptr: usize) {
    if ptr == 0 {
        return;
    }

    if !is_readable_memory(ptr, 0x50) {
        log::line(format_args!(
            "[player-scale-no-bone] NR phys-world-child pass #{pass}: {label} parent=0x{parent:X}+0x{offset:X} ptr=0x{ptr:X} unreadable"
        ));
        return;
    }

    let q0 = read_usize(ptr);
    log::line(format_args!(
        "[player-scale-no-bone] NR phys-world-child pass #{pass}: {label} parent=0x{parent:X}+0x{offset:X} ptr=0x{ptr:X} q0=0x{q0:X}({}) q8=0x{:X} q10=0x{:X} q18=0x{:X} q20=0x{:X} q28=0x{:X} q30=0x{:X} q38=0x{:X} q40=0x{:X}",
        format_module_rva(q0),
        read_usize(ptr + 0x8),
        read_usize(ptr + 0x10),
        read_usize(ptr + 0x18),
        read_usize(ptr + 0x20),
        read_usize(ptr + 0x28),
        read_usize(ptr + 0x30),
        read_usize(ptr + 0x38),
        read_usize(ptr + 0x40),
    ));
}

fn dump_observed_cloth_models(pass: usize) {
    let objects = {
        let observed = OBSERVED_CLOTH_MODELS.get_or_init(|| Mutex::new(Vec::new()));
        let Ok(objects) = observed.lock() else {
            return;
        };
        objects.clone()
    };

    if objects.is_empty() {
        if pass == 1 || pass == 5 || pass == DUMP_PASSES {
            log::line(format_args!(
                "[player-scale-no-bone] NR phys-accessor delayed dump pass #{pass}: no CSClothModelIns roots observed yet"
            ));
        }
        return;
    }

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-accessor delayed dump pass #{pass}: cloth_models={}",
        objects.len()
    ));

    for addr in objects {
        dump_cloth_model(pass, addr);
    }
}

fn dump_cloth_model(pass: usize, addr: usize) {
    if !is_readable_memory(addr, 0x210) {
        log::line(format_args!(
            "[player-scale-no-bone] NR phys-accessor pass #{pass} CSClothModelIns this=0x{addr:X} unreadable"
        ));
        return;
    }

    let root_offsets = [
        0x0, 0x8, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x70, 0x78, 0x80, 0x88, 0x90,
        0x98, 0xA0, 0xD0, 0xE0, 0x100, 0x160, 0x188, 0x1D8, 0x200,
    ];
    log_qwords(pass, "root CSClothModelIns", addr, 0, addr, &root_offsets);

    let phys_sys = read_usize(addr + 0x88);
    dump_phys_sys_ins(pass, addr, phys_sys);

    for offset in [0x90, 0x98, 0xA0] {
        let ptr = read_usize(addr + offset);
        dump_debug_draw_summary(pass, addr, offset, ptr);
    }

    let metadata_ptr = read_usize(addr + 0x188);
    dump_ascii_landmark(pass, addr, 0x188, metadata_ptr);
}

fn dump_phys_sys_ins(pass: usize, owner: usize, ptr: usize) {
    if ptr == 0 || !is_readable_memory(ptr, 0x180) {
        return;
    }

    let first_offsets = [
        0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x68, 0x70,
        0x78,
    ];
    let second_offsets = [
        0x80, 0x88, 0x90, 0x98, 0xA0, 0xA8, 0xB0, 0xB8, 0xC0, 0xC8, 0xD0, 0xD8, 0xE0, 0xE8, 0xF0,
        0xF8,
    ];
    let third_offsets = [
        0x100, 0x108, 0x110, 0x118, 0x120, 0x128, 0x130, 0x138, 0x140, 0x148, 0x150, 0x158, 0x160,
        0x168, 0x170, 0x178,
    ];

    log_qwords(
        pass,
        "likely CSPhysSysIns[00..78]",
        owner,
        0x88,
        ptr,
        &first_offsets,
    );
    log_qwords(
        pass,
        "likely CSPhysSysIns[80..F8]",
        owner,
        0x88,
        ptr,
        &second_offsets,
    );
    log_qwords(
        pass,
        "likely CSPhysSysIns[100..178]",
        owner,
        0x88,
        ptr,
        &third_offsets,
    );
    log_float_rows(
        pass,
        "likely CSPhysSysIns",
        ptr,
        &[0x20, 0x40, 0x60, 0x80, 0xA0, 0xC0],
    );
    log_havok_landmarks(pass, "likely CSPhysSysIns", ptr);

    for offset in [
        0x18, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x70, 0x88, 0x98, 0xD0, 0xD8,
    ] {
        let child = read_usize(ptr + offset);
        dump_grandchild(pass, "likely CSPhysSysIns", ptr, offset, child);
    }
}

fn dump_debug_draw_summary(pass: usize, owner: usize, owner_offset: usize, ptr: usize) {
    if ptr == 0 || !is_readable_memory(ptr, 0x60) {
        return;
    }

    let offsets = [
        0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58,
    ];
    let label = format!("summary debug-draw? CSClothModelIns+0x{owner_offset:X}");
    log_qwords(pass, &label, owner, owner_offset, ptr, &offsets);
}

fn dump_grandchild(pass: usize, parent_label: &str, parent: usize, offset: usize, ptr: usize) {
    if ptr == 0 || !is_readable_memory(ptr, 0xA0) {
        return;
    }

    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let label = format!("grandchild {parent_label}+0x{offset:X}");
    let offsets = [
        0x0, 0x8, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x60, 0x70, 0x80,
        0x88, 0x98,
    ];
    log_qwords(pass, &label, parent, offset, ptr, &offsets);
    log_ascii_window(pass, &label, ptr, 0x60);
    log_ascii_window(pass, &label, ptr, 0x70);
    log_ascii_window(pass, &label, ptr, 0x80);
    log_float_rows(pass, &label, ptr, &[0x20, 0x40, 0x60, 0x80]);
    log_havok_landmarks(pass, &label, ptr);
}

fn log_float_rows(pass: usize, label: &str, ptr: usize, offsets: &[usize]) {
    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let mut line = format!(
        "[player-scale-no-bone] NR phys-accessor pass #{pass} f32 #{detail_count} {label} ptr=0x{ptr:X}"
    );
    for &offset in offsets {
        let a = read_f32(ptr + offset);
        let b = read_f32(ptr + offset + 0x4);
        let c = read_f32(ptr + offset + 0x8);
        let d = read_f32(ptr + offset + 0xC);
        let _ = write!(line, " f{offset:X}=({a:.4},{b:.4},{c:.4},{d:.4})");
    }
    log::line(format_args!("{line}"));
}

fn log_qwords(
    pass: usize,
    label: &str,
    owner: usize,
    owner_offset: usize,
    ptr: usize,
    offsets: &[usize],
) {
    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let mut line = format!(
        "[player-scale-no-bone] NR phys-accessor pass #{pass} detail #{detail_count} {label} owner=0x{owner:X}+0x{owner_offset:X} ptr=0x{ptr:X}"
    );
    for &offset in offsets {
        let q = read_usize(ptr + offset);
        let _ = write!(line, " q{offset:X}=0x{q:X}");
        if offset == 0 {
            let _ = write!(line, "({})", format_module_rva(q));
        }
    }
    log::line(format_args!("{line}"));
}

fn log_havok_landmarks(pass: usize, label: &str, ptr: usize) {
    let detail_count = DETAIL_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if detail_count > DETAIL_LIMIT {
        return;
    }

    let hcl_shape = read_usize(ptr + 0x88);
    let hcl_enabled = read_u8(ptr + 0x9F);
    let sim_collidables = read_usize(ptr + 0xD0);
    let sim_collidable_map = read_usize(ptr + 0xD8);
    let shape_instance_scale = [
        read_f32(ptr + 0x40),
        read_f32(ptr + 0x44),
        read_f32(ptr + 0x48),
        read_f32(ptr + 0x4C),
    ];
    let shape_instance_shape = read_usize(ptr + 0x50);
    let hknp_convex_radius = read_f32(ptr + 0x20);
    let hknp_a = [
        read_f32(ptr + 0x60),
        read_f32(ptr + 0x64),
        read_f32(ptr + 0x68),
        read_f32(ptr + 0x6C),
    ];
    let hknp_b = [
        read_f32(ptr + 0x70),
        read_f32(ptr + 0x74),
        read_f32(ptr + 0x78),
        read_f32(ptr + 0x7C),
    ];

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-accessor pass #{pass} landmark #{detail_count} {label} ptr=0x{ptr:X} hcl_shape@88=0x{hcl_shape:X}({}) hcl_enabled@9F=0x{hcl_enabled:02X} hclSim_collidables@D0=0x{sim_collidables:X} hclSim_map@D8=0x{sim_collidable_map:X} hknpShapeInstance_scale40=({:.4},{:.4},{:.4},{:.4}) shape50=0x{shape_instance_shape:X}({}) hknpCapsule_convex20={:.4} a60=({:.4},{:.4},{:.4},{:.4}) b70=({:.4},{:.4},{:.4},{:.4})",
        format_module_rva(hcl_shape),
        shape_instance_scale[0],
        shape_instance_scale[1],
        shape_instance_scale[2],
        shape_instance_scale[3],
        format_module_rva(shape_instance_shape),
        hknp_convex_radius,
        hknp_a[0],
        hknp_a[1],
        hknp_a[2],
        hknp_a[3],
        hknp_b[0],
        hknp_b[1],
        hknp_b[2],
        hknp_b[3],
    ));
}

fn dump_ascii_landmark(pass: usize, owner: usize, owner_offset: usize, ptr: usize) {
    if ptr == 0 || !is_readable_memory(ptr, 0x40) {
        return;
    }

    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = read_u8(ptr + index);
    }

    let text: String = bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            }
        })
        .collect();

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-accessor pass #{pass} ascii CSClothModelIns+0x{owner_offset:X} owner=0x{owner:X} ptr=0x{ptr:X} text='{text}'"
    ));
}

fn log_ascii_window(pass: usize, label: &str, ptr: usize, offset: usize) {
    if !is_readable_memory(ptr + offset, 0x20) {
        return;
    }

    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = read_u8(ptr + offset + index);
    }

    if !bytes.iter().any(|byte| byte.is_ascii_alphabetic()) {
        return;
    }

    let text: String = bytes
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() || *byte == b' ' {
                *byte as char
            } else {
                '.'
            }
        })
        .collect();

    log::line(format_args!(
        "[player-scale-no-bone] NR phys-accessor pass #{pass} ascii {label}+0x{offset:X} ptr=0x{ptr:X} text='{text}'"
    ));
}

fn scan_qword_for_pointer(base: usize, target: usize, max_len: usize) -> Option<usize> {
    if base == 0 || target == 0 {
        return None;
    }

    for offset in (0..max_len).step_by(8) {
        if read_usize(base + offset) == target {
            return Some(offset);
        }
    }
    None
}

fn scan_f32_for_value(base: usize, target: f32, max_len: usize) -> Option<usize> {
    if base == 0 || !scale_like(target) {
        return None;
    }

    for offset in (0..max_len).step_by(4) {
        let value = read_f32(base + offset);
        if float_close(value, target) {
            return Some(offset);
        }
    }
    None
}

fn format_optional_offset(offset: Option<usize>) -> String {
    match offset {
        Some(offset) => format!("+0x{offset:X}"),
        None => "none".to_string(),
    }
}

fn scale_like(value: f32) -> bool {
    value.is_finite() && (0.1..=5.0).contains(&value)
}

fn float_close(a: f32, b: f32) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 0.001
}

fn module_base(module: HMODULE) -> usize {
    module.0 as usize
}

fn format_module_rva(addr: usize) -> String {
    let base = NR_MODULE_BASE.load(Ordering::Acquire);
    if base != 0 && addr >= base {
        let rva = addr - base;
        if rva < 0x8000000 {
            return format!("nightreign.exe+0x{rva:X}");
        }
    }
    "external".to_string()
}

fn is_readable_memory(addr: usize, len: usize) -> bool {
    if addr == 0 || len == 0 {
        return false;
    }

    let mut mbi = MEMORY_BASIC_INFORMATION::default();
    let result = unsafe {
        VirtualQuery(
            Some(addr as *const _),
            &mut mbi,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if result == 0 || mbi.State != MEM_COMMIT {
        return false;
    }

    let protect = mbi.Protect;
    let readable = protect == PAGE_READONLY
        || protect == PAGE_READWRITE
        || protect == PAGE_WRITECOPY
        || protect == PAGE_EXECUTE_READ
        || protect == PAGE_EXECUTE_READWRITE
        || protect == PAGE_EXECUTE_WRITECOPY
        || protect == PAGE_EXECUTE;
    if !readable {
        return false;
    }

    let region_start = mbi.BaseAddress as usize;
    let region_end = region_start.saturating_add(mbi.RegionSize);
    addr >= region_start && addr.saturating_add(len) <= region_end
}

fn read_usize(addr: usize) -> usize {
    if !is_readable_memory(addr, std::mem::size_of::<usize>()) {
        return 0;
    }

    unsafe { (addr as *const usize).read_unaligned() }
}

fn read_f32(addr: usize) -> f32 {
    if !is_readable_memory(addr, std::mem::size_of::<f32>()) {
        return f32::NAN;
    }

    unsafe { (addr as *const f32).read_unaligned() }
}

fn read_u32(addr: usize) -> u32 {
    if !is_readable_memory(addr, std::mem::size_of::<u32>()) {
        return 0;
    }

    unsafe { (addr as *const u32).read_unaligned() }
}

fn read_u8(addr: usize) -> u8 {
    if !is_readable_memory(addr, std::mem::size_of::<u8>()) {
        return 0;
    }

    unsafe { (addr as *const u8).read_unaligned() }
}
