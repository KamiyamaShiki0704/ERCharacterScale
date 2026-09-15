mod units;
use std::{
    cell::{Cell, RefCell},
    mem::transmute,
    sync::{
        RwLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
};
pub(crate) use units::{
    detach_unit, refresh_unit_registry, register_unit, unregister_unit, with_collider_unit,
};

use ilhook::x64::{CallbackOption, HookFlags, Registers, hook_closure_jmp_back, hook_closure_retn};
use windows::{
    Win32::{Foundation::HMODULE, System::LibraryLoader::GetModuleHandleA},
    core::s,
};

use crate::cloth_owner_scope::{Route as ClothOwnerRoute, Scope as ClothOwnerScope};
use crate::cloth_render_scale;
use crate::log;

const ER_POSE_IMPORTER_UPDATE_RVA: usize = 0xB4B950;
const ER_ANIM_SKELETON_GET_AFFINE_RVA: usize = 0xB45540;
const ER_ANIM_SKELETON_GET_AFFINE_RANGE_RVA: usize = 0xB45610;
const ER_CLOTH_RENDER_CALL_RVA: usize = 0xB47F01;
const ER_CLOTH_RENDER_RETURN_RVA: usize = ER_CLOTH_RENDER_CALL_RVA + 3;
// All frame modes converge after construction, BEFORE world/bind composition.
// RBX = mesh-mesh operator, RDI = input buffer, R14 = fresh hkArray<Matrix4>.
const ER_CLOTH_MESH_FRAME_COMMIT_RVA: usize = 0x15E61F7;
const ER_CLOTH_MESH_PN_FLOAT_RVA: usize = 0x15C7480;
const ER_CLOTH_MESH_PN_ALIGNED_RVA: usize = 0x15C7630;
const ER_CLOTH_MESH_PN_VTABLE_RVA: usize = 0x2D8B050;
const ER_CLOTH_SKIN_PN_RVA: usize = 0x15A9940;
const ER_CLOTH_SKIN_PN_VTABLE_RVA: usize = 0x2D86308;
const SKIN_PN_ENTRY: &[u8] = &[
    0x4C, 0x8B, 0x52, 0x10, 0x48, 0x63, 0x41, 0x68, 0x4D, 0x8B, 0x4A, 0x20, 0x49, 0x8B, 0x14, 0xC1,
];
// WW2.7.1.0: core+90 decomposition, input-table/map lookup, Qs scale product,
// and destination Matrix4 store. These are guards, NOT additional hooks.
const SKIN_SOURCE_SEAMS: &[(usize, &[u8])] = &[
    (
        0x26B15BF,
        &[
            0x48, 0x8D, 0x91, 0x90, 0, 0, 0, 0x48, 0x8D, 0x4D, 0, 0xE8, 0x81, 0x07, 0xFD, 0xFE,
        ],
    ),
    (
        0x26B15F0,
        &[
            0x49, 0x8B, 0x5C, 0x24, 0x40, 0x48, 0x03, 0xD9, 0x48, 0x89, 0x5D, 0xE8, 0x48, 0x8B,
            0x3B, 0x48, 0x89, 0x7D, 0x98, 0x48, 0x8B, 0x43, 0x08,
        ],
    ),
    (
        0x26B1926,
        &[
            0x41, 0x3B, 0x5E, 0x10, 0x73, 0x0A, 0x49, 0x8B, 0x46, 0x18, 0x0F, 0xBF, 0x14, 0x30,
            0xEB, 0x03, 0x83, 0xCA, 0xFF, 0x85, 0xD2, 0x0F, 0x88, 0x67, 0x01, 0, 0,
        ],
    ),
    (
        0x26B1A4B,
        &[
            0x0F, 0x28, 0x45, 0x20, 0x41, 0x0F, 0x59, 0xC1, 0x0F, 0x29, 0x45, 0x50,
        ],
    ),
    (
        0x26B1A98,
        &[
            0x48, 0x8B, 0x57, 0x18, 0x49, 0x03, 0xD4, 0x48, 0x8D, 0x4D, 0x30, 0xE8, 0xC8, 0, 0xFD,
            0xFE,
        ],
    ),
];

const ER_CLOTH_MESH_P_VTABLE_RVA: usize = 0x2D83590;
const CLOTH_MESH_FRAME_DISPATCH_PATTERN: &[u8] = &[
    0x8B, 0x41, 0x50, 0x48, 0x8B, 0xD9, 0x4D, 0x8B, 0xC1, 0x85, 0xC0, 0x74, 0x13, 0x83, 0xF8, 0x02,
    0x74, 0x07, 0xE8, 0x87, 0xF5, 0xFF, 0xFF, 0xEB, 0x0C, 0xE8, 0xE0, 0xFA, 0xFF, 0xFF, 0xEB, 0x05,
    0xE8, 0xE9, 0xF7, 0xFF, 0xFF,
];

// All three ER range paths converge here before advancing to the next matrix.
// At this exact point RBX is the current bone index, RDI is its output, R14 is
// the fallback provider and EAX still contains the provider's per-item result.
const ER_ANIM_SKELETON_GET_AFFINE_RANGE_ITEM_COMMIT_RVA: usize = 0xB45733;
const ER_ANIM_SKELETON_GET_RVA: usize = 0xB45770;
const ER_ANIM_SKELETON_GET_RANGE_RVA: usize = 0xB45860;
const ER_CLOTH_POSE_SELECTOR_RVA: usize = 0x3FF218;
const ER_CLOTH_INPUT_SETTER_RVA: usize = 0xC4CBF0;
const ER_CLOTH_SECONDARY_REFERENCE_SUBMIT_RVA: usize = 0x267BA00;
const ER_CLOTH_INNER_COMMIT_RVA: usize = 0x267BF90;
const ER_POSE_RESOLVE_TRANSFORM_RVA: usize = 0x1654A10;
const ER_POSE_IMPORTER_VTABLE_RVA: usize = 0x2B70360;
const ER_ANIM_SKELETON_VTABLE_RVA: usize = 0x2B6F1A0;
const ER_CLOTH_MODEL_VTABLE_RVA: usize = 0x2B92A60;
const ER_CLOTH_INNER_VTABLE_RVA: usize = 0x329A2F8;

const ER_PE_TIMESTAMP: u32 = 0x6A96B418;
const ER_SIZE_OF_IMAGE: u32 = 0x5E0DA00;
const ER_ENTRY_POINT_RVA: u32 = 0x24FE160;

const CHR_INS_POSE_IMPORTER_OFFSET: usize = 0x398;
const CHR_INS_ALTERNATE_POSE_IMPORTER_OFFSET: usize = 0x3A0;
const CHR_INS_CLOTH_POSE_OVERRIDE_OFFSET: usize = 0x3A8;
const CHR_INS_ANIM_SKELETON_MODIFIER_OFFSET: usize = 0x3B0;
const CHR_INS_ALTERNATE_POSE_STATE_OFFSET: usize = 0x568;
const POSE_INNER_OFFSET: usize = 0x48;
const POSE_METADATA_OFFSET: usize = 0x00;
const POSE_OUTPUT_OFFSET: usize = 0x18;
const POSE_MATERIALIZED_OFFSET: usize = 0x38;
const POSE_METADATA_COUNT_OFFSET: usize = 0x38;
const POSE_TRANSFORM_STRIDE: usize = 0x30;
const POSE_TRANSLATION_OFFSET: usize = 0x00;
const POSE_LOCAL_SCALE_OFFSET: usize = 0x20;
const AFFINE_MATRIX_STRIDE: usize = 0x30;
const MATRIX_STRIDE: usize = 0x40;
const MAX_OUTPUTS_PER_CALL: usize = cloth_render_scale::MAX_TRANSFORMS;
const MATRIX_CANDIDATE_SLOTS: usize = 16;
pub const CLOTH_INSTANCE_SLOTS: usize = 64;
pub const CLOTH_CHILD_SLOTS: usize = 64;
const MAX_CLOTH_GROUPS: usize = 32;
const MAX_CLOTH_CHILDREN_PER_GROUP: usize = 256;
const MAX_CLOTH_PARTICLES_PER_CHILD: usize = 16_384;
const MAX_CLOTH_TRANSFORM_ENTRIES_PER_CHILD: usize = 512;
const MAX_CLOTH_SOLVER_INPUT_ENTRIES: usize = 32;
const MAX_CLOTH_SOLVER_INPUT_TRANSFORMS: usize = 16_384;
const MAX_CLOTH_SOLVER_SOURCE_TRANSFORMS: usize = cloth_render_scale::MAX_TRANSFORMS;
const CLOTH_SOLVER_ENTRY_STRIDE: usize = 0x38;
const CLOTH_SOLVER_SOURCE_TRANSFORM_STRIDE: usize = 0x30;
pub const CLOTH_CONSTRAINT_SET_SLOTS: usize = 64;
const MAX_CLOTH_CONSTRAINT_SETS: usize = 256;
const MAX_CLOTH_CONSTRAINT_ELEMENTS: usize = 65_536;
const CLOTH_MAIN_TRANSFORM_OFFSET: usize = 0x60;
const CLOTH_SECONDARY_TRANSFORM_OFFSET: usize = 0xE0;
const CLOTH_SECONDARY_DIRTY_OFFSET: usize = 0x4B;
const CLOTH_CORE_REFERENCE_TRANSFORM_OFFSET: usize = 0x50;
const CLOTH_CORE_MAIN_TRANSFORM_OFFSET: usize = 0x90;
const CLOTH_CORE_SECONDARY_TRANSFORM_OFFSET: usize = 0xD0;
const NO_PENDING_CLOTH_SCALE_BITS: u32 = 0;
const IN_PROGRESS_CLOTH_SCALE_BITS: u32 = 0x7FC0_0000;
const MATRIX_CANDIDATE_KIND_SINGLE: u32 = 1;
const MATRIX_CANDIDATE_KIND_RANGE: u32 = 2;
// NR scales affine outputs only when the bone-map entry is negative and the
// output therefore came from the skeleton fallback rather than the already
// scaled pose cache. Build 1.8 exercised this branch with the wrong ER cloth
// pose source; Build 1.9 selected the correct source but disabled the branch.
// Keep this exact fallback path separate from the broad 0x40 matrix hooks.
// Build 2.24 proved that the exact ER negative-map/provider-success path was
// active, but it did not improve cloth placement.  The item-commit site is a
// very hot loop, so leave the implementation available for diagnostics while
// keeping it out of the runtime path.
const ENABLE_AFFINE_FALLBACK_HOOKS: bool = false;
// Build 1.8 proved that the broad 0x40 global matrix-output hooks can write
// tens of thousands of matrices without reaching the missing cloth fallback
// branch. Keep them available for regression comparison but off the hot path.
const ENABLE_LEGACY_MATRIX_OUTPUT_HOOKS: bool = false;
// Build 2.15/2.16 received the human result that cloth placement followed the
// scaled player while this synchronous source bracket was enabled. Build 2.22
// later proved only that the proposed lazy-source extension had zero targets;
// disabling the already-active direct-source branch in Build 2.23 regressed
// cloth that had previously followed the player. Build 2.29 regressed that
// placement by letting a guessed local-scale class reject this whole bracket.
// Keep translation independent from every local-scale decision and retain
// guarded lazy handling.
const ENABLE_EXTRA_CLOTH_SOLVER_SOURCE_TRANSLATION: bool = true;
const _: () = assert!(ENABLE_EXTRA_CLOTH_SOLVER_SOURCE_TRANSLATION);
// Build 2.10 proved that directly translating child particles, bounds, and
// transform entries is overwritten by the next native solver generation. The
// code remains available for regression comparison, but Build 2.13 drives
// the native one-shot secondary dirty branch without restoring hot-path writes.
const ENABLE_CLOTH_ATTACHMENT_POSITION_SYNC: bool = false;
const _: () = assert!(!ENABLE_CLOTH_ATTACHMENT_POSITION_SYNC);
// Build 2.13 proved that the native transition reaches all seven observed
// children immediately. Retaining the two full before/after particle scans in
// the wrapper would add avoidable transition and load-time work, so Build 2.14
// replaces it with a compact task-side solver-input aggregate.
const ENABLE_CLOTH_IMMEDIATE_TRANSITION_PROBE: bool = false;

const ER_HCL_STANDARD_LINK_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D89D58;
const ER_HCL_STRETCH_LINK_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D89480;
const ER_HCL_LOCAL_RANGE_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D7EEE0;
const ER_HCL_TRANSITION_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D8A1F8;
const ER_HCL_BEND_STIFFNESS_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D88BD8;
const ER_HCL_COMPRESSIBLE_LINK_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D7E360;
const ER_HCL_BONE_PLANES_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D82E78;
const ER_HCL_ANTI_PINCH_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D7E0D0;
const ER_HCL_BEND_LINK_CONSTRAINT_SET_VTABLE_RVA: usize = 0x2D88788;
const ER_HCL_STRETCH_LINK_CONSTRAINT_SET_MX_VTABLE_RVA: usize = 0x2D7E188;
const ER_HCL_STANDARD_LINK_CONSTRAINT_SET_MX_VTABLE_RVA: usize = 0x2D7EB18;
const ER_HCL_BEND_STIFFNESS_CONSTRAINT_SET_MX_VTABLE_RVA: usize = 0x2D85D50;
const ER_HCL_COMPRESSIBLE_LINK_CONSTRAINT_SET_MX_VTABLE_RVA: usize = 0x2D824E8;
const ER_HCL_BEND_LINK_CONSTRAINT_SET_MX_VTABLE_RVA: usize = 0x2D7D668;
const ER_HCL_VOLUME_CONSTRAINT_MX_VTABLE_RVA: usize = 0x2D88C18;

const POSE_IMPORTER_PATTERN: &[u8] = &[0x48, 0x83, 0xC1, 0x48, 0xE9, 0x67, 0x98, 0xB0, 0x00];
const CLOTH_POSE_SELECTOR_PATTERN: &[u8] = &[
    0x48, 0x8D, 0x9F, 0xA8, 0x03, 0x00, 0x00, // lea rbx,[rdi+0x3A8]
    0x48, 0x83, 0x3B, 0x00, // cmp qword ptr [rbx],0
    0x75, 0x18, // jne preferred
    0xBB, 0x98, 0x03, 0x00, 0x00, // mov ebx,0x398
    0xBA, 0xA0, 0x03, 0x00, 0x00, // mov edx,0x3A0
    0x48, 0x83, 0xBF, 0x68, 0x05, 0x00, 0x00, 0x00, // cmp qword ptr [rdi+0x568],0
    0x0F, 0x45, 0xDA, // cmovne ebx,edx
    0x48, 0x03, 0xDF, // add rbx,rdi
    0x48, 0x8B, 0x1B, // mov rbx,[rbx]
];
const CLOTH_INPUT_SETTER_PATTERN: &[u8] = &[
    0x48, 0x89, 0x5C, 0x24, 0x08, // mov [rsp+08],rbx
    0x48, 0x89, 0x74, 0x24, 0x10, // mov [rsp+10],rsi
    0x57, // push rdi
    0x48, 0x83, 0xEC, 0x20, // sub rsp,20
    0x48, 0x8B, 0x99, 0x20, 0x01, 0x00, 0x00, // mov rbx,[rcx+120]
    0x48, 0x8B, 0xFA, // mov rdi,rdx
    0x48, 0x8B, 0xF1, // mov rsi,rcx
    0x48, 0x3B, 0xDA, // cmp rbx,rdx
];
const CLOTH_SECONDARY_REFERENCE_SUBMIT_PATTERN: &[u8] = &[
    0x48, 0x8B, 0x41, 0x30, // mov rax,[rcx+30]
    0x48, 0x85, 0xC0, // test rax,rax
    0x74, 0x2F, // jz +2F
    0x0F, 0x28, 0x02, // movaps xmm0,[rdx]
    0x0F, 0x29, 0x80, 0xD0, 0x00, 0x00, 0x00, // movaps [rax+D0],xmm0
];
const CLOTH_INNER_COMMIT_PATTERN: &[u8] = &[
    0x48, 0x89, 0x54, 0x24, 0x10, // mov [rsp+10],rdx
    0x57, // push rdi
    0x48, 0x83, 0xEC, 0x30, // sub rsp,30
    0x48, 0xC7, 0x44, 0x24, 0x20, 0xFE, 0xFF, 0xFF, 0xFF, // mov [rsp+20],-2
    0x48, 0x89, 0x5C, 0x24, 0x40, // mov [rsp+40],rbx
    0x48, 0x8B, 0xFA, // mov rdi,rdx
    0x48, 0x8B, 0xD9, // mov rbx,rcx
    0x48, 0x8B, 0x49, 0x30, // mov rcx,[rcx+30]
];
const POSE_RESOLVE_TRANSFORM_PATTERN: &[u8] = &[
    0x40, 0x53, // push rbx
    0x48, 0x83, 0xEC, 0x10, // sub rsp,10
    0x4C, 0x8B, 0x49, 0x28, // mov r9,[rcx+28]
    0x4C, 0x8B, 0xD1, // mov r10,rcx
    0x4C, 0x63, 0xDA, // movsxd r11,edx
];
const ANIM_SKELETON_GET_AFFINE_PATTERN: &[u8] = &[
    0x48, 0x89, 0x5C, 0x24, 0x08, 0x48, 0x89, 0x6C, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x57,
    0x48, 0x83, 0xEC, 0x50,
];
const ANIM_SKELETON_GET_AFFINE_RANGE_PATTERN: &[u8] = &[
    0x48, 0x89, 0x5C, 0x24, 0x18, 0x55, 0x56, 0x57, 0x41, 0x54, 0x41, 0x56, 0x48, 0x83, 0xEC, 0x50,
];
const ANIM_SKELETON_GET_AFFINE_RANGE_ITEM_COMMIT_PATTERN: &[u8] = &[
    0xFF, 0xC3, // inc ebx
    0x48, 0x83, 0xC7, 0x30, // add rdi,30
    0x41, 0x8D, 0x04, 0x1F, // lea eax,[r15+rbx]
    0x3B, 0xC6, // cmp eax,esi
    0x0F, 0x82, 0x5B, 0xFF, 0xFF, 0xFF, // jb loop
];
const ANIM_SKELETON_GET_PATTERN: &[u8] = &[
    0x48, 0x89, 0x5C, 0x24, 0x08, 0x48, 0x89, 0x6C, 0x24, 0x10, 0x48, 0x89, 0x74, 0x24, 0x18, 0x57,
];
const ANIM_SKELETON_GET_RANGE_PATTERN: &[u8] = &[
    0x48, 0x89, 0x5C, 0x24, 0x18, 0x55, 0x57, 0x41, 0x54, 0x41, 0x56, 0x41, 0x57, 0x48, 0x83, 0xEC,
    0x60,
];

static INSTALL_ATTEMPTED: AtomicBool = AtomicBool::new(false);
static HOOKS_READY: AtomicBool = AtomicBool::new(false);
static MODULE_BASE: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(crate) static MODULE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
#[path = "test_support/character_units.rs"]
pub(crate) mod test_characters;

type SolverSourceTransform = [f32; 12];

#[derive(Clone, Copy)]
struct SolverSourceTransformBackup {
    index: usize,
    translation: [f32; 4],
}

// Retained only as a negative control for the Build 2.30 duplicate multiplier.
#[cfg(test)]
#[derive(Clone, Copy)]
struct SolverSourceLocalScaleBackup {
    index: usize,
    original: [f32; 4],
    scaled: [f32; 4],
}

#[derive(Default)]
struct SolverSourceScratch {
    indices: Vec<usize>,
    lazy_indices: Vec<usize>,
    backups: Vec<SolverSourceTransformBackup>,
    context: SolverPoseContext,
    local: Vec<AlignedSolverTransform>,
    model: Vec<AlignedSolverTransform>,
    flags: Vec<u32>,
}

#[repr(C, align(16))]
#[derive(Default)]
struct SolverPoseContext([usize; 8]);

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct AlignedSolverTransform(SolverSourceTransform);

const _: () = assert!(size_of::<AlignedSolverTransform>() == CLOTH_SOLVER_SOURCE_TRANSFORM_STRIDE);

/// ER 267BF20 -> 26B38D0 -> 26B14E0 consumes this hkaPose synchronously.
/// 16549A0 can materialize *ancestors* and change cache flags as well as the
/// selected model transform. XYZ-only restoration cannot isolate that effect
/// from the render consumer. Never resolve/scale the canonical pose here.
/// The private context must not move until the native call has returned.
fn solver_consumer_context(
    update_context: usize,
    scratch: &mut SolverSourceScratch,
) -> Option<usize> {
    if !is_memory_accessible(update_context, size_of::<SolverPoseContext>(), false) {
        return None;
    }
    let header = unsafe { (update_context as *const [usize; 8]).read_unaligned() };
    let metadata = header[0];
    let count =
        usize::try_from(read_i32(metadata.checked_add(POSE_METADATA_COUNT_OFFSET)?)?).ok()?;
    if count == 0 || count > MAX_CLOTH_SOLVER_SOURCE_TRANSFORMS {
        return None;
    }
    // The low DWORD is hkArray's signed size, the high DWORD is capacity/flags.
    // Only the skeleton's bounded count is consumed on this audited path.
    if [header[2], header[4], header[6]]
        .iter()
        .any(|&word| (word as u32 as i32) < count as i32)
    {
        return None;
    }
    let parents = read_usize(metadata.checked_add(0x20)?)?;
    if !is_memory_accessible(parents, count.checked_mul(size_of::<i16>())?, false) {
        return None;
    }
    for index in 0..count {
        let parent = unsafe { (parents as *const i16).add(index).read_unaligned() };
        // Native lazy resolution assumes parents precede children. Reject a
        // malformed/cyclic table before it can loop or access outside scratch.
        if parent < -1 || (parent >= 0 && parent as usize >= index) {
            return None;
        }
    }
    let transform_bytes = count.checked_mul(CLOTH_SOLVER_SOURCE_TRANSFORM_STRIDE)?;
    if !is_memory_accessible(header[1], transform_bytes, false)
        || !is_memory_accessible(header[3], transform_bytes, false)
        || !is_memory_accessible(header[5], count.checked_mul(size_of::<u32>())?, false)
    {
        return None;
    }
    scratch
        .local
        .try_reserve(count.saturating_sub(scratch.local.len()))
        .ok()?;
    scratch
        .model
        .try_reserve(count.saturating_sub(scratch.model.len()))
        .ok()?;
    scratch
        .flags
        .try_reserve(count.saturating_sub(scratch.flags.len()))
        .ok()?;
    scratch
        .local
        .resize(count, AlignedSolverTransform::default());
    scratch
        .model
        .resize(count, AlignedSolverTransform::default());
    scratch.flags.resize(count, 0);
    // Copy bytes: native source alignment and stale unused transform values do
    // not impose Rust alignment/finite-value requirements. Destination SIMD
    // alignment is guaranteed by AlignedSolverTransform, not allocator luck.
    unsafe {
        std::ptr::copy_nonoverlapping(
            header[1] as *const u8,
            scratch.local.as_mut_ptr().cast(),
            transform_bytes,
        );
        std::ptr::copy_nonoverlapping(
            header[3] as *const u8,
            scratch.model.as_mut_ptr().cast(),
            transform_bytes,
        );
        std::ptr::copy_nonoverlapping(
            header[5] as *const u8,
            scratch.flags.as_mut_ptr().cast(),
            count * size_of::<u32>(),
        );
    }
    if unsafe { (update_context as *const [usize; 8]).read_unaligned() } != header
        || read_i32(metadata.checked_add(POSE_METADATA_COUNT_OFFSET)?) != Some(count as i32)
        || read_usize(metadata.checked_add(0x20)?) != Some(parents)
    {
        return None;
    }
    scratch.context.0 = header;
    scratch.context.0[1] = scratch.local.as_mut_ptr() as usize;
    scratch.context.0[3] = scratch.model.as_mut_ptr() as usize;
    scratch.context.0[5] = scratch.flags.as_mut_ptr() as usize;
    Some(scratch.context.0.as_mut_ptr() as usize)
}

thread_local! {
    static SOLVER_SOURCE_SCRATCH: RefCell<SolverSourceScratch> = RefCell::new(SolverSourceScratch::default());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetBinding {
    pub ready: bool,
    pub pose_importer: usize,
    pub cloth_pose_importer: usize,
    pub cloth_pose_source: ClothPoseSource,
    pub anim_skeleton_modifier: usize,
    pub reason: &'static str,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClothPoseSource {
    #[default]
    Unavailable,
    Primary398,
    Alternate3a0,
    Override3a8,
}

impl ClothPoseSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Primary398 => "chr+0x398",
            Self::Alternate3a0 => "chr+0x3A0",
            Self::Override3a8 => "chr+0x3A8",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HookCounters {
    pub render_calls: u64,
    pub render_rows: u64,
    pub render_rejected: u64,
    pub render_max_us: u64,
    pub render_max_queries: u64,
    pub pose_target_calls: u64,
    pub pose_transforms_written: u64,
    pub cloth_pose_target_calls: u64,
    pub cloth_pose_transforms_written: u64,
    pub pose_gate_00: u64,
    pub pose_gate_01: u64,
    pub pose_gate_10: u64,
    pub pose_gate_11: u64,
    pub pose_gate_other: u64,
    pub pose_applied_scale_bits: u32,
    pub cloth_pose_applied_scale_bits: u32,
    pub affine_single_target_calls: u64,
    pub affine_single_matrices_written: u64,
    pub affine_range_target_calls: u64,
    pub affine_range_matrices_written: u64,
    pub affine_range_provider_successes: u64,
    pub affine_range_provider_identities: u64,
    pub affine_range_provider_failures: u64,
    pub single_target_calls: u64,
    pub single_matrices_written: u64,
    pub range_target_calls: u64,
    pub range_matrices_written: u64,
    pub rejected_outputs: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MatrixCandidate {
    pub slot: usize,
    pub kind: u32,
    pub caller_rva: usize,
    pub first_this: usize,
    pub last_this: usize,
    pub output: usize,
    pub arg8: u32,
    pub arg9: u32,
    pub qword_48: usize,
    pub qword_68: usize,
    pub qword_88: usize,
    pub hits: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothMatrixSummary {
    pub readable: bool,
    pub basis_x: f32,
    pub basis_y: f32,
    pub basis_z: f32,
    pub translation_x: f32,
    pub translation_y: f32,
    pub translation_z: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothInstanceSnapshot {
    pub valid: bool,
    pub slot: usize,
    pub owner: usize,
    pub input: usize,
    pub setter_hits: u64,
    pub equipment_owned: bool,
    pub source_calls: u64,
    pub reference_commits: u64,
    pub applied_scale_bits: u32,
    pub pending_scale_bits: u32,
    pub inner: usize,
    pub inner_vtable: usize,
    pub outer_flags: u32,
    pub outer_flag_4a: u8,
    pub inner_flag_49: u8,
    pub inner_flag_61: u8,
    pub inner_flag_62: u8,
    pub core: usize,
    pub downstream_state: usize,
    pub core_flag_4c: u8,
    pub core_flag_4d: u8,
    pub core_group_count: Option<usize>,
    pub downstream_entry_count: Option<usize>,
    pub core_matrix_90: ClothMatrixSummary,
    pub core_matrix_d0: ClothMatrixSummary,
    pub core_matrix_50: ClothMatrixSummary,
    pub matrix_60: ClothMatrixSummary,
    pub matrix_e0: ClothMatrixSummary,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothSolverInputSnapshot {
    pub readable: bool,
    pub entry_count: usize,
    pub transform_count: usize,
    pub scale_min_x: f32,
    pub scale_min_y: f32,
    pub scale_min_z: f32,
    pub scale_max_x: f32,
    pub scale_max_y: f32,
    pub scale_max_z: f32,
    pub translation_bounds: ClothPositionBounds,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClothInstanceProbeCounters {
    pub setter_calls: u64,
    pub slot_inserts: u64,
    pub slot_replacements: u64,
    pub occupied_slots: usize,
    pub scale_transitions_queued: u64,
    pub scale_transitions_deferred: u64,
    pub scale_transitions_rejected: u64,
    pub scale_reference_commits: u64,
    pub scale_wrapper_deferred: u64,
    pub scale_wrapper_rejected: u64,
    pub secondary_reference_calls: u64,
    pub secondary_reference_adjusted: u64,
    pub secondary_reference_passthrough: u64,
    pub secondary_reference_rejected: u64,
    pub secondary_probe_calls: u64,
    pub secondary_source_owner_e0_matches: u64,
    pub secondary_source_owner_e0_mismatches: u64,
    pub secondary_pre_core_requested: u64,
    pub secondary_pre_core_unit: u64,
    pub secondary_pre_core_other: u64,
    pub secondary_post_core_requested: u64,
    pub secondary_post_core_unit: u64,
    pub secondary_post_core_other: u64,
    pub secondary_post_copy_matches: u64,
    pub secondary_post_copy_mismatches: u64,
    pub secondary_core_changed: u64,
    pub secondary_dirty_marked: u64,
    pub secondary_dirty_rejected: u64,
    pub solver_source_bracket_calls: u64,
    pub solver_source_bracket_transforms: u64,
    pub solver_source_bracket_restores: u64,
    pub solver_source_bracket_rejected: u64,
    pub solver_source_direct_transforms: u64,
    pub solver_source_lazy_candidates: u64,
    pub solver_source_lazy_resolved: u64,
    pub solver_source_lazy_rejected: u64,
    pub solver_source_local_scale_calls: u64,
    pub solver_source_local_scale_transforms: u64,
    pub solver_source_local_scale_restores: u64,
    pub solver_source_local_scale_rejected: u64,
    pub solver_source_local_scale_passthrough: u64,
    pub solver_private_context_calls: u64,
    pub solver_private_context_returns: u64,
    pub immediate_probe_transitions: u64,
    pub immediate_children_observed: u64,
    pub immediate_particle_matches: u64,
    pub immediate_transform_matches: u64,
    pub immediate_both_matches: u64,
    pub immediate_unreadable: u64,
    pub transform_resync_calls: u64,
    pub transform_resync_children_observed: u64,
    pub transform_resync_children_changed: u64,
    pub transform_resync_entries_observed: u64,
    pub transform_resync_entries_changed: u64,
    pub transform_resync_entries_already_scaled: u64,
    pub transform_resync_entries_rejected: u64,
    pub transform_resync_topology_rejected: u64,
    pub attachment_position_buffers_shifted: u64,
    pub attachment_particles_shifted: u64,
    pub attachment_aabbs_shifted: u64,
    pub attachment_position_rejected: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ClothTransformResyncCounts {
    children_observed: u64,
    children_changed: u64,
    entries_observed: u64,
    entries_changed: u64,
    entries_already_scaled: u64,
    entries_rejected: u64,
    topology_rejected: u64,
    position_buffers_shifted: u64,
    particles_shifted: u64,
    aabbs_shifted: u64,
    position_rejected: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ImmediateChildTransitionCounts {
    children_observed: u64,
    particle_matches: u64,
    transform_matches: u64,
    both_matches: u64,
    unreadable: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothPositionBounds {
    pub readable: bool,
    pub count: usize,
    pub min_x: f32,
    pub min_y: f32,
    pub min_z: f32,
    pub max_x: f32,
    pub max_y: f32,
    pub max_z: f32,
}

impl ClothPositionBounds {
    pub fn span_x(self) -> f32 {
        self.max_x - self.min_x
    }

    pub fn span_y(self) -> f32 {
        self.max_y - self.min_y
    }

    pub fn span_z(self) -> f32 {
        self.max_z - self.min_z
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothTransformEntrySummary {
    pub readable: bool,
    pub count: usize,
    pub basis_min_x: f32,
    pub basis_min_y: f32,
    pub basis_min_z: f32,
    pub basis_max_x: f32,
    pub basis_max_y: f32,
    pub basis_max_z: f32,
    pub translation_bounds: ClothPositionBounds,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothChildSnapshot {
    pub valid: bool,
    pub group_index: usize,
    pub child_index: usize,
    pub group_holder: usize,
    pub group_root: usize,
    pub child: usize,
    pub child_vtable: usize,
    pub sim_data: usize,
    pub particle_count: usize,
    pub current_positions: ClothPositionBounds,
    pub previous_positions: ClothPositionBounds,
    pub transform_entries: ClothTransformEntrySummary,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothScalarBounds {
    pub readable: bool,
    pub count: usize,
    pub minimum: f32,
    pub maximum: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClothConstraintKind {
    #[default]
    Unknown,
    StandardLink,
    StretchLink,
    LocalRange,
    Transition,
    BendStiffness,
    CompressibleLink,
    BonePlanes,
    AntiPinch,
    BendLink,
    StretchLinkMx,
    StandardLinkMx,
    BendStiffnessMx,
    CompressibleLinkMx,
    BendLinkMx,
    VolumeMx,
}

impl ClothConstraintKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::StandardLink => "standard-link",
            Self::StretchLink => "stretch-link",
            Self::LocalRange => "local-range",
            Self::Transition => "transition",
            Self::BendStiffness => "bend-stiffness",
            Self::CompressibleLink => "compressible-link",
            Self::BonePlanes => "bone-planes",
            Self::AntiPinch => "anti-pinch",
            Self::BendLink => "bend-link",
            Self::StretchLinkMx => "stretch-link-mx",
            Self::StandardLinkMx => "standard-link-mx",
            Self::BendStiffnessMx => "bend-stiffness-mx",
            Self::CompressibleLinkMx => "compressible-link-mx",
            Self::BendLinkMx => "bend-link-mx",
            Self::VolumeMx => "volume-mx",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothConstraintSetSnapshot {
    pub valid: bool,
    pub anti_pinch_array: bool,
    pub array_index: usize,
    pub set: usize,
    pub vtable: usize,
    pub kind: ClothConstraintKind,
    pub constraint_id: u32,
    pub constraint_type: u32,
    pub elements_readable: bool,
    pub element_count: usize,
    pub primary_dimensions: ClothScalarBounds,
    pub secondary_dimensions: ClothScalarBounds,
    pub tertiary_dimensions: ClothScalarBounds,
    pub set_dimension_readable: bool,
    pub set_dimension: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothConstraintProbeSummary {
    pub topology_readable: bool,
    pub static_count: usize,
    pub anti_pinch_count: usize,
    pub captured_count: usize,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothLocalSimulationProfile {
    pub readable: bool,
    pub particle_radii: ClothScalarBounds,
    pub max_particle_radius_readable: bool,
    pub max_particle_radius: f32,
    pub first_pose_positions: ClothPositionBounds,
    pub sim_pose_count_readable: bool,
    pub sim_pose_count: usize,
    pub static_constraint_count_readable: bool,
    pub static_constraint_count: usize,
    pub anti_pinch_constraint_count_readable: bool,
    pub anti_pinch_constraint_count: usize,
    pub per_instance_collidable_count_readable: bool,
    pub per_instance_collidable_count: usize,
    pub particles_aabb: ClothPositionBounds,
    pub collision_particles_aabb: ClothPositionBounds,
    pub landscape_collision_particles_aabb: ClothPositionBounds,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClothChildProbeSummary {
    pub topology_readable: bool,
    pub group_count: usize,
    pub child_count: usize,
    pub captured_children: usize,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct TargetClothSimulationSet {
    pub readable: bool,
    pub count: usize,
    pub truncated: bool,
    pub sim_data: [usize; CLOTH_CHILD_SLOTS],
}

impl Default for TargetClothSimulationSet {
    fn default() -> Self {
        Self {
            readable: false,
            count: 0,
            truncated: false,
            sim_data: [0; CLOTH_CHILD_SLOTS],
        }
    }
}

#[derive(Clone, Copy)]
struct BoneIndexMapping {
    count: u32,
    entries: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AffineRangeFallbackAction {
    MappedPose,
    Identity,
    ProviderFailed,
    Scale,
    Reject,
}

#[repr(align(16))]
struct AlignedClothMatrix([f32; 16]);

pub fn install() -> bool {
    if INSTALL_ATTEMPTED.swap(true, Ordering::AcqRel) {
        return HOOKS_READY.load(Ordering::Acquire);
    }

    let Ok(module) = (unsafe { GetModuleHandleA(s!("eldenring.exe")) }) else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: GetModuleHandleA failed"
        ));
        return false;
    };
    let base = module_base(module);
    MODULE_BASE.store(base, Ordering::Release);

    if let Err(reason) = validate_runtime(base) {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: runtime validation failed: {reason}"
        ));
        return false;
    }

    let pose_hook = unsafe {
        hook_closure_retn(
            base + ER_POSE_IMPORTER_UPDATE_RVA,
            units::pose,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(pose_hook) = pose_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: pose importer hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(pose_hook));

    let cloth_setter_hook = unsafe {
        hook_closure_retn(
            base + ER_CLOTH_INPUT_SETTER_RVA,
            units::setter,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(cloth_setter_hook) = cloth_setter_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: cloth input setter hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(cloth_setter_hook));

    let cloth_secondary_reference_hook = unsafe {
        hook_closure_retn(
            base + ER_CLOTH_SECONDARY_REFERENCE_SUBMIT_RVA,
            units::secondary,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(cloth_secondary_reference_hook) = cloth_secondary_reference_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: cloth secondary reference submit hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(cloth_secondary_reference_hook));

    let cloth_commit_hook = unsafe {
        hook_closure_retn(
            base + ER_CLOTH_INNER_COMMIT_RVA,
            units::commit,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(cloth_commit_hook) = cloth_commit_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: cloth inner commit hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(cloth_commit_hook));

    let render_hook = unsafe {
        hook_closure_retn(
            base + ER_ANIM_SKELETON_GET_AFFINE_RANGE_RVA,
            units::render,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(render_hook) = render_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: cloth render range hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(render_hook));

    let mesh_frame_hook = unsafe {
        hook_closure_jmp_back(
            base + ER_CLOTH_MESH_FRAME_COMMIT_RVA,
            units::mesh_frame,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(mesh_frame_hook) = mesh_frame_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: cloth mesh frame hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(mesh_frame_hook));
    let mesh_pn_hook = unsafe {
        hook_closure_retn(
            base + ER_CLOTH_MESH_PN_FLOAT_RVA,
            units::mesh_pn,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(mesh_pn_hook) = mesh_pn_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: cloth mesh PN hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(mesh_pn_hook));
    let mesh_pn_aligned_hook = unsafe {
        hook_closure_retn(
            base + ER_CLOTH_MESH_PN_ALIGNED_RVA,
            units::mesh_pn,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(mesh_pn_aligned_hook) = mesh_pn_aligned_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: aligned cloth mesh PN hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(mesh_pn_aligned_hook));

    let skin_normal_hook = unsafe {
        hook_closure_retn(
            base + ER_CLOTH_SKIN_PN_RVA,
            units::skin,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(skin_normal_hook) = skin_normal_hook else {
        log::line(format_args!(
            "[player-scale-no-bone] ER body-scale port disabled: SkinPN normal hook failed"
        ));
        return false;
    };
    let _ = Box::leak(Box::new(skin_normal_hook));

    if ENABLE_AFFINE_FALLBACK_HOOKS {
        let affine_single_hook = unsafe {
            hook_closure_retn(
                base + ER_ANIM_SKELETON_GET_AFFINE_RVA,
                units::affine,
                CallbackOption::None,
                HookFlags::empty(),
            )
        };
        let Ok(affine_single_hook) = affine_single_hook else {
            log::line(format_args!(
                "[player-scale-no-bone] ER body-scale port disabled: affine single matrix hook failed"
            ));
            return false;
        };
        let _ = Box::leak(Box::new(affine_single_hook));

        let affine_range_item_hook = unsafe {
            hook_closure_jmp_back(
                base + ER_ANIM_SKELETON_GET_AFFINE_RANGE_ITEM_COMMIT_RVA,
                units::affine_item,
                CallbackOption::None,
                HookFlags::empty(),
            )
        };
        let Ok(affine_range_item_hook) = affine_range_item_hook else {
            log::line(format_args!(
                "[player-scale-no-bone] ER body-scale port disabled: affine matrix range item hook failed"
            ));
            return false;
        };
        let _ = Box::leak(Box::new(affine_range_item_hook));
    }

    if ENABLE_LEGACY_MATRIX_OUTPUT_HOOKS {
        let single_hook = unsafe {
            hook_closure_retn(
                base + ER_ANIM_SKELETON_GET_RVA,
                units::matrix,
                CallbackOption::None,
                HookFlags::empty(),
            )
        };
        let Ok(single_hook) = single_hook else {
            log::line(format_args!(
                "[player-scale-no-bone] ER body-scale port disabled: single matrix hook failed"
            ));
            return false;
        };
        let _ = Box::leak(Box::new(single_hook));

        let range_hook = unsafe {
            hook_closure_retn(
                base + ER_ANIM_SKELETON_GET_RANGE_RVA,
                units::range,
                CallbackOption::None,
                HookFlags::empty(),
            )
        };
        let Ok(range_hook) = range_hook else {
            log::line(format_args!(
                "[player-scale-no-bone] ER body-scale port disabled: matrix range hook failed"
            ));
            return false;
        };
        let _ = Box::leak(Box::new(range_hook));
    }

    // BEGIN253-COLLIDER-ROTATION-INSTALL
    if !crate::cloth_collider_rotation_hook::install(base) {
        log::line(format_args!(
            "[ERPS-COLLIDER-ROTATION] disabled: incompatible entry or hook failure"
        ));
        return false;
    }
    // END253-COLLIDER-ROTATION-INSTALL
    HOOKS_READY.store(true, Ordering::Release);
    if crate::ENABLE_SYNC_DIAGNOSTIC {
        crate::cloth_diagnostic::install(base);
    }
    log::line(format_args!(
        "[ERPS-CLOTH-MESH-FRAME] ready commit=+0x{ER_CLOTH_MESH_FRAME_COMMIT_RVA:X} pn=+0x{ER_CLOTH_MESH_PN_FLOAT_RVA:X} pn_aligned=+0x{ER_CLOTH_MESH_PN_ALIGNED_RVA:X} scope=current-owned-sim-buffer mode=unit-normal-or-P-area output=fresh-pre-bind unit-depth=multiply-requested area-depth=divide-requested normal-output=dimensionless authored=unchanged"
    ));
    log::line(format_args!(
        "[ERPS-CLOTH-RENDER] ready getter=+0x{ER_ANIM_SKELETON_GET_AFFINE_RANGE_RVA:X} caller=+0x{ER_CLOTH_RENDER_RETURN_RVA:X} scope=current-owned-independent-input selection=native-writeback-mask output=fresh-affine48 T=divide-requested basis=multiply-requested canonical=unchanged"
    ));
    log::line(format_args!(
        "[player-scale-no-bone] ER body-scale port hooks ready: pose=eldenring.exe+0x{ER_POSE_IMPORTER_UPDATE_RVA:X} pose_resolve_transform=+0x{ER_POSE_RESOLVE_TRANSFORM_RVA:X} cloth_selector=+0x{ER_CLOTH_POSE_SELECTOR_RVA:X} cloth_input_setter=+0x{ER_CLOTH_INPUT_SETTER_RVA:X} cloth_secondary_reference_submit=+0x{ER_CLOTH_SECONDARY_REFERENCE_SUBMIT_RVA:X} cloth_secondary_reference_probe=immediate-copy-verification cloth_secondary_reference_dirty=transition-commit cloth_inner_commit=+0x{ER_CLOTH_INNER_COMMIT_RVA:X} cloth_solver_source_translation=private-context-translation-only cloth_solver_source_storage=private-copy-local-model-flags cloth_instance_probe=readonly cloth_core_probe=readonly cloth_child_probe=disabled-proven-boundary cloth_solver_output_probe=compact-task-side cloth_local_simulation_probe=disabled-proven-dimensions cloth_local_dimension_scale=baseline-budgeted-event-cadence cloth_local_collision_scale=common-shapes-map-topology-observed cloth_scale_commit=persistent-native-retry cloth_attachment_position_sync=secondary-reference-submit cloth_transform_resync=disabled-overwritten-child-boundary cloth_transition_probe=lightweight-counters affine_fallback_hooks={ENABLE_AFFINE_FALLBACK_HOOKS} affine_range_item_commit=disabled-runtime-falsified affine_range_item_commit_rva=+0x{ER_ANIM_SKELETON_GET_AFFINE_RANGE_ITEM_COMMIT_RVA:X} affine_range_semantics=disabled-exact-path-no-visual-effect legacy_matrix_hooks={ENABLE_LEGACY_MATRIX_OUTPUT_HOOKS}"
    ));
    true
}

#[cfg(test)]
thread_local! { static BIND_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn binding_call_count() -> u64 {
    BIND_CALLS.with(std::cell::Cell::get)
}

pub fn bind_local_player(chr_ins_addr: usize, scale: f32) -> TargetBinding {
    #[cfg(test)]
    BIND_CALLS.with(|calls| calls.set(calls.get() + 1));
    crate::memory_query::scoped(|| bind_local_player_inner(chr_ins_addr, scale))
}

pub(crate) fn image_base() -> usize {
    MODULE_BASE.load(Ordering::Acquire)
}

/// Pre-physics, only after the runtime has revalidated the full unit identity.
pub(crate) fn restore_unit_pose() {
    let state = current_unit_state();
    if state.pose_write_lock.swap(true, Ordering::AcqRel) {
        return;
    }
    let primary = state.target_pose_importer.load(Ordering::Acquire);
    let cloth = state.target_cloth_pose_importer.load(Ordering::Acquire);
    for (importer, applied) in [
        (primary, &state.pose_applied_scale_bits),
        (cloth, &state.cloth_pose_applied_scale_bits),
    ] {
        if importer == 0
            || (importer == primary && std::ptr::eq(applied, &state.cloth_pose_applied_scale_bits))
            || !pose_importer_uses_update_hook(importer, image_base())
        {
            continue;
        }
        let inner = importer + POSE_INNER_OFFSET;
        let materialized = read_u8(inner + POSE_MATERIALIZED_OFFSET);
        let previous = f32::from_bits(applied.load(Ordering::Acquire));
        if let PoseScaleAction::Scale(ratio) =
            plan_pose_scale(previous, 1.0, materialized, materialized)
            && scale_pose_output(inner, ratio).is_some()
        {
            applied.store(1.0f32.to_bits(), Ordering::Release);
        }
    }
    state.pose_write_lock.store(false, Ordering::Release);
}

fn bind_local_player_inner(chr_ins_addr: usize, scale: f32) -> TargetBinding {
    let unit_state = current_unit_state();
    if !HOOKS_READY.load(Ordering::Acquire) {
        clear_target();
        return TargetBinding {
            ready: false,
            pose_importer: 0,
            cloth_pose_importer: 0,
            cloth_pose_source: ClothPoseSource::Unavailable,
            anim_skeleton_modifier: 0,
            reason: "hooks-not-ready",
        };
    }
    if chr_ins_addr == 0 || !valid_scale(scale) {
        clear_target();
        return TargetBinding {
            ready: false,
            pose_importer: 0,
            cloth_pose_importer: 0,
            cloth_pose_source: ClothPoseSource::Unavailable,
            anim_skeleton_modifier: 0,
            reason: "invalid-player-or-scale",
        };
    }

    let pose_importer = read_usize(chr_ins_addr + CHR_INS_POSE_IMPORTER_OFFSET).unwrap_or(0);
    let alternate_pose_importer =
        read_usize(chr_ins_addr + CHR_INS_ALTERNATE_POSE_IMPORTER_OFFSET).unwrap_or(0);
    let cloth_pose_override =
        read_usize(chr_ins_addr + CHR_INS_CLOTH_POSE_OVERRIDE_OFFSET).unwrap_or(0);
    let alternate_pose_state =
        read_usize(chr_ins_addr + CHR_INS_ALTERNATE_POSE_STATE_OFFSET).unwrap_or(0);
    let (cloth_pose_importer, cloth_pose_source) = choose_cloth_pose_importer(
        cloth_pose_override,
        pose_importer,
        alternate_pose_importer,
        alternate_pose_state,
    );
    let anim_skeleton_modifier =
        read_usize(chr_ins_addr + CHR_INS_ANIM_SKELETON_MODIFIER_OFFSET).unwrap_or(0);
    let base = MODULE_BASE.load(Ordering::Acquire);

    if read_usize(chr_ins_addr) == Some(base + crate::cloth_owner_scope::ENEMY_VTABLE_RVA) {
        let scope = ClothOwnerScope::capture(chr_ins_addr, cloth_pose_importer, base, read_usize);
        let model = read_usize(chr_ins_addr + 0x50).unwrap_or(0);
        let owner = model.checked_add(0x130).and_then(read_usize);
        if scope.player != chr_ins_addr
            || owner.is_none()
            || owner.is_some_and(|owner| {
                owner != 0 && !scope.routes.iter().any(|route| route.owner == owner)
            })
        {
            clear_target();
            return TargetBinding {
                ready: false,
                pose_importer,
                cloth_pose_importer,
                cloth_pose_source,
                anim_skeleton_modifier,
                reason: "enemy-model-cloth-ownership-unsupported",
            };
        }
    }
    if !pose_importer_uses_update_hook(pose_importer, base) {
        clear_target();
        return TargetBinding {
            ready: false,
            pose_importer,
            cloth_pose_importer,
            cloth_pose_source,
            anim_skeleton_modifier,
            reason: "pose-importer-not-ready",
        };
    }
    if !pose_importer_uses_update_hook(cloth_pose_importer, base) {
        clear_target();
        return TargetBinding {
            ready: false,
            pose_importer,
            cloth_pose_importer,
            cloth_pose_source,
            anim_skeleton_modifier,
            reason: "cloth-pose-importer-not-ready",
        };
    }
    if !object_has_vtable(anim_skeleton_modifier, base + ER_ANIM_SKELETON_VTABLE_RVA) {
        clear_target();
        return TargetBinding {
            ready: false,
            pose_importer,
            cloth_pose_importer,
            cloth_pose_source,
            anim_skeleton_modifier,
            reason: "anim-skeleton-modifier-not-ready",
        };
    }

    let primary_changed = unit_state.target_pose_importer.load(Ordering::Acquire) != pose_importer;
    let cloth_changed = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire)
        != cloth_pose_importer;
    let anim_skeleton_changed =
        unit_state.target_anim_skeleton.load(Ordering::Acquire) != anim_skeleton_modifier;
    if primary_changed || cloth_changed || anim_skeleton_changed {
        publish_neutral();
        if primary_changed {
            unit_state
                .pose_applied_scale_bits
                .store(1.0f32.to_bits(), Ordering::Release);
        }
        if cloth_changed {
            unit_state
                .cloth_pose_applied_scale_bits
                .store(1.0f32.to_bits(), Ordering::Release);
            reset_all_cloth_scale_states();
        }
        unit_state
            .target_pose_importer
            .store(pose_importer, Ordering::Release);
        unit_state
            .target_cloth_pose_importer
            .store(cloth_pose_importer, Ordering::Release);
        unit_state
            .target_anim_skeleton
            .store(anim_skeleton_modifier, Ordering::Release);
    }
    unit_state
        .target_scale_bits
        .store(scale.to_bits(), Ordering::Release);

    TargetBinding {
        ready: true,
        pose_importer,
        cloth_pose_importer,
        cloth_pose_source,
        anim_skeleton_modifier,
        reason: "ready",
    }
}

pub fn publish_target_scale(scale: f32) -> bool {
    let unit_state = current_unit_state();
    if !HOOKS_READY.load(Ordering::Acquire)
        || unit_state.target_pose_importer.load(Ordering::Acquire) == 0
        || unit_state
            .target_cloth_pose_importer
            .load(Ordering::Acquire)
            == 0
        || !valid_scale(scale)
    {
        return false;
    }
    unit_state
        .target_scale_bits
        .store(scale.to_bits(), Ordering::Release);
    true
}

pub fn clear_target() {
    publish_neutral();
    clear_object_targets();
}

pub fn counters() -> HookCounters {
    let unit_state = current_unit_state();
    HookCounters {
        render_calls: unit_state.render_calls.load(Ordering::Relaxed),
        render_rows: unit_state.render_rows.load(Ordering::Relaxed),
        render_rejected: unit_state.render_rejected.load(Ordering::Relaxed),
        render_max_us: unit_state.render_max_us.load(Ordering::Relaxed),
        render_max_queries: unit_state.render_max_queries.load(Ordering::Relaxed),
        pose_target_calls: unit_state.pose_target_calls.load(Ordering::Relaxed),
        pose_transforms_written: unit_state.pose_transforms_written.load(Ordering::Relaxed),
        cloth_pose_target_calls: unit_state.cloth_pose_target_calls.load(Ordering::Relaxed),
        cloth_pose_transforms_written: unit_state
            .cloth_pose_transforms_written
            .load(Ordering::Relaxed),
        pose_gate_00: unit_state.pose_gate_00.load(Ordering::Relaxed),
        pose_gate_01: unit_state.pose_gate_01.load(Ordering::Relaxed),
        pose_gate_10: unit_state.pose_gate_10.load(Ordering::Relaxed),
        pose_gate_11: unit_state.pose_gate_11.load(Ordering::Relaxed),
        pose_gate_other: unit_state.pose_gate_other.load(Ordering::Relaxed),
        pose_applied_scale_bits: unit_state.pose_applied_scale_bits.load(Ordering::Acquire),
        cloth_pose_applied_scale_bits: unit_state
            .cloth_pose_applied_scale_bits
            .load(Ordering::Acquire),
        affine_single_target_calls: unit_state
            .affine_single_target_calls
            .load(Ordering::Relaxed),
        affine_single_matrices_written: unit_state
            .affine_single_matrices_written
            .load(Ordering::Relaxed),
        affine_range_target_calls: unit_state.affine_range_target_calls.load(Ordering::Relaxed),
        affine_range_matrices_written: unit_state
            .affine_range_matrices_written
            .load(Ordering::Relaxed),
        affine_range_provider_successes: unit_state
            .affine_range_provider_successes
            .load(Ordering::Relaxed),
        affine_range_provider_identities: unit_state
            .affine_range_provider_identities
            .load(Ordering::Relaxed),
        affine_range_provider_failures: unit_state
            .affine_range_provider_failures
            .load(Ordering::Relaxed),
        single_target_calls: unit_state.single_target_calls.load(Ordering::Relaxed),
        single_matrices_written: unit_state.single_matrices_written.load(Ordering::Relaxed),
        range_target_calls: unit_state.range_target_calls.load(Ordering::Relaxed),
        range_matrices_written: unit_state.range_matrices_written.load(Ordering::Relaxed),
        rejected_outputs: unit_state.rejected_outputs.load(Ordering::Relaxed),
    }
}

pub fn matrix_candidates() -> [MatrixCandidate; MATRIX_CANDIDATE_SLOTS] {
    let unit_state = current_unit_state();
    let mut candidates = [MatrixCandidate::default(); MATRIX_CANDIDATE_SLOTS];
    for (slot, candidate) in candidates.iter_mut().enumerate() {
        let hits = unit_state.matrix_candidate_hits[slot].load(Ordering::Acquire);
        if hits == 0 {
            continue;
        }
        *candidate = MatrixCandidate {
            slot,
            kind: unit_state.matrix_candidate_kinds[slot].load(Ordering::Relaxed),
            caller_rva: unit_state.matrix_candidate_caller_rvas[slot].load(Ordering::Relaxed),
            first_this: unit_state.matrix_candidate_first_this[slot].load(Ordering::Relaxed),
            last_this: unit_state.matrix_candidate_last_this[slot].load(Ordering::Relaxed),
            output: unit_state.matrix_candidate_outputs[slot].load(Ordering::Relaxed),
            arg8: unit_state.matrix_candidate_arg8[slot].load(Ordering::Relaxed),
            arg9: unit_state.matrix_candidate_arg9[slot].load(Ordering::Relaxed),
            qword_48: unit_state.matrix_candidate_qword_48[slot].load(Ordering::Relaxed),
            qword_68: unit_state.matrix_candidate_qword_68[slot].load(Ordering::Relaxed),
            qword_88: unit_state.matrix_candidate_qword_88[slot].load(Ordering::Relaxed),
            hits,
        };
    }
    candidates
}

pub fn matrix_candidate_kind_name(kind: u32) -> &'static str {
    match kind {
        MATRIX_CANDIDATE_KIND_SINGLE => "single",
        MATRIX_CANDIDATE_KIND_RANGE => "range",
        _ => "unknown",
    }
}

fn publish_neutral() {
    let unit_state = current_unit_state();
    unit_state
        .target_scale_bits
        .store(1.0f32.to_bits(), Ordering::Release);
}

fn clear_object_targets() {
    let unit_state = current_unit_state();
    if let Ok(mut scope) = unit_state.target_cloth_scope.try_write() {
        *scope = ClothOwnerScope::default();
    }
    unit_state.target_pose_importer.store(0, Ordering::Release);
    unit_state
        .target_cloth_pose_importer
        .store(0, Ordering::Release);
    unit_state.target_anim_skeleton.store(0, Ordering::Release);
    unit_state
        .pose_applied_scale_bits
        .store(1.0f32.to_bits(), Ordering::Release);
    unit_state
        .cloth_pose_applied_scale_bits
        .store(1.0f32.to_bits(), Ordering::Release);
    reset_all_cloth_scale_states();
}

fn reset_all_cloth_scale_states() {
    let unit_state = current_unit_state();
    unit_state
        .cloth_pending_slot_mask
        .store(0, Ordering::Release);
    for slot in 0..CLOTH_INSTANCE_SLOTS {
        unit_state.cloth_instance_cores[slot].store(0, Ordering::Release);
        unit_state.cloth_instance_applied_scale_bits[slot]
            .store(1.0f32.to_bits(), Ordering::Release);
        unit_state.cloth_instance_pending_scale_bits[slot]
            .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
    }
}

fn validate_runtime(base: usize) -> Result<(), &'static str> {
    if !validate_pe_identity(base) {
        return Err("PE identity mismatch");
    }
    if !validate_skin_normal_runtime(base) {
        return Err("SkinPN execution layout mismatch");
    }
    if !bytes_equal(
        base + ER_CLOTH_MESH_PN_FLOAT_RVA,
        &[
            0x48, 0x89, 0x6C, 0x24, 0x18, 0x56, 0x57, 0x41, 0x56, 0x48, 0x83, 0xEC, 0x40,
        ],
    ) || !bytes_equal(
        base + ER_CLOTH_MESH_PN_ALIGNED_RVA,
        &[
            0x48, 0x89, 0x6C, 0x24, 0x18, 0x56, 0x57, 0x41, 0x56, 0x48, 0x83, 0xEC, 0x40,
        ],
    ) || !bytes_equal(base + 0x15C756F, &[0xE8, 0x4C, 0xD0, 0xFD, 0xFF])
        || !bytes_equal(base + 0x15C770B, &[0xE8, 0x90, 0xEA, 0x01, 0x00])
        || !bytes_equal(base + 0x15C771F, &[0xE8, 0x7C, 0xDC, 0xFD, 0xFF])
        || !bytes_equal(base + 0x15C8093, &[0xE8, 0x98, 0xF5, 0xFF, 0xFF])
        || read_usize(base + ER_CLOTH_MESH_PN_VTABLE_RVA + 0x20) != Some(base + 0x15C7FD0)
        || read_usize(base + ER_CLOTH_MESH_P_VTABLE_RVA + 0x20) != Some(base + 0x1589A20)
    {
        return Err("cloth mesh PN execution layout mismatch");
    }
    if !bytes_equal(
        base + 0x15E5834,
        &[
            0x0F, 0x28, 0xCA, 0x0F, 0xC6, 0xCA, 0xC9, 0x0F, 0xC6, 0xC4, 0xC9, 0x0F, 0x59, 0xC2,
            0x0F, 0x59, 0xCC, 0x0F, 0x5C, 0xC8, 0x0F, 0xC6, 0xC9, 0xC9, 0x41, 0x0F, 0x29, 0x48,
            0x20,
        ],
    ) || !bytes_equal(base + 0x15E61D2, CLOTH_MESH_FRAME_DISPATCH_PATTERN)
        || !bytes_equal(
            base + ER_CLOTH_MESH_FRAME_COMMIT_RVA,
            &[0x8B, 0x0D, 0x9F, 0xE3, 0x1F, 0x03],
        )
        || !bytes_equal(
            base + 0x15E5A26,
            &[
                0x8B, 0x5F, 0x60, 0x45, 0x33, 0xDB, 0x4C, 0x8B, 0x56, 0x30, 0x48, 0x8B, 0x6E, 0x18,
            ],
        )
        || !bytes_equal(
            base + 0x15E5AEA,
            &[
                0x0F, 0x52, 0xC2, 0x0F, 0x28, 0xCA, 0x0F, 0xC2, 0xCF, 0x02, 0x0F, 0x55, 0xC8, 0x0F,
                0x59, 0xCB,
            ],
        )
    {
        return Err("cloth mesh frame construction/commit mismatch");
    }
    if read_usize(base + ER_HCL_VOLUME_CONSTRAINT_MX_VTABLE_RVA) != Some(base + 0x1546430)
        || read_usize(base + ER_HCL_VOLUME_CONSTRAINT_MX_VTABLE_RVA + 0x30)
            != Some(base + 0x15E54C0)
    {
        return Err("volume constraint layout mismatch");
    }
    if !bytes_equal(base + ER_POSE_IMPORTER_UPDATE_RVA, POSE_IMPORTER_PATTERN) {
        return Err("pose importer entry mismatch");
    }
    if !bytes_equal(
        base + ER_CLOTH_POSE_SELECTOR_RVA,
        CLOTH_POSE_SELECTOR_PATTERN,
    ) {
        return Err("cloth pose selector mismatch");
    }
    if !bytes_equal(base + ER_CLOTH_INPUT_SETTER_RVA, CLOTH_INPUT_SETTER_PATTERN) {
        return Err("cloth input setter mismatch");
    }
    // Ownership seams are read-only, but gate their offsets as strictly as
    // hook entries. Never scan a guessed equipment table on another build.
    if !bytes_equal(
        base + 0x9EAA4F,
        &[
            0x48, 0x8D, 0x59, 0x28, 0x48, 0x89, 0x51, 0x10, 0x48, 0x8D, 0xBB, 0xD8, 0x00, 0x00,
            0x00,
        ],
    ) || !bytes_equal(base + 0x9EA1AF, &[0x48, 0x89, 0x51, 0x08])
        || !bytes_equal(
            base + 0x9F1448,
            &[
                0x48, 0x8B, 0x8B, 0x30, 0x01, 0x00, 0x00, 0x48, 0x8B, 0xD7, 0xE8, 0x99, 0xB7, 0x25,
                0x00,
            ],
        )
        || read_usize(base + crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA) != Some(base + 0x9EAA00)
        || read_usize(base + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA)
            != Some(base + 0x9F3B60)
    {
        return Err("equipment cloth ownership layout mismatch");
    }
    if !bytes_equal(
        base + ER_CLOTH_SECONDARY_REFERENCE_SUBMIT_RVA,
        CLOTH_SECONDARY_REFERENCE_SUBMIT_PATTERN,
    ) {
        return Err("cloth secondary reference submit mismatch");
    }
    if !bytes_equal(base + ER_CLOTH_INNER_COMMIT_RVA, CLOTH_INNER_COMMIT_PATTERN) {
        return Err("cloth inner commit mismatch");
    }
    if !bytes_equal(
        base + ER_POSE_RESOLVE_TRANSFORM_RVA,
        POSE_RESOLVE_TRANSFORM_PATTERN,
    ) {
        return Err("pose lazy transform resolver mismatch");
    }
    if !bytes_equal(
        base + ER_ANIM_SKELETON_GET_AFFINE_RVA,
        ANIM_SKELETON_GET_AFFINE_PATTERN,
    ) {
        return Err("affine single matrix entry mismatch");
    }
    if !bytes_equal(
        base + ER_ANIM_SKELETON_GET_AFFINE_RANGE_RVA,
        ANIM_SKELETON_GET_AFFINE_RANGE_PATTERN,
    ) {
        return Err("affine matrix range entry mismatch");
    }
    if !bytes_equal(
        base + ER_ANIM_SKELETON_GET_AFFINE_RANGE_ITEM_COMMIT_RVA,
        ANIM_SKELETON_GET_AFFINE_RANGE_ITEM_COMMIT_PATTERN,
    ) {
        return Err("affine matrix range item commit mismatch");
    }
    if !bytes_equal(base + ER_ANIM_SKELETON_GET_RVA, ANIM_SKELETON_GET_PATTERN) {
        return Err("single matrix entry mismatch");
    }
    if !bytes_equal(
        base + ER_ANIM_SKELETON_GET_RANGE_RVA,
        ANIM_SKELETON_GET_RANGE_PATTERN,
    ) {
        return Err("matrix range entry mismatch");
    }

    let pose_vtable = base + ER_POSE_IMPORTER_VTABLE_RVA;
    if read_usize(pose_vtable + 3 * size_of::<usize>()) != Some(base + ER_POSE_IMPORTER_UPDATE_RVA)
    {
        return Err("pose importer vtable mismatch");
    }
    let anim_vtable = base + ER_ANIM_SKELETON_VTABLE_RVA;
    if !bytes_equal(base + ER_CLOTH_RENDER_CALL_RVA, &[0xFF, 0x50, 0x40])
        || read_usize(anim_vtable + 0x40) != Some(base + ER_ANIM_SKELETON_GET_AFFINE_RANGE_RVA)
    {
        return Err("cloth render range call/vtable mismatch");
    }
    if read_usize(anim_vtable + 6 * size_of::<usize>())
        != Some(base + ER_ANIM_SKELETON_GET_AFFINE_RVA)
        || read_usize(anim_vtable + 5 * size_of::<usize>()) != Some(base + ER_ANIM_SKELETON_GET_RVA)
        || read_usize(anim_vtable + 7 * size_of::<usize>())
            != Some(base + ER_ANIM_SKELETON_GET_RANGE_RVA)
    {
        return Err("anim skeleton vtable mismatch");
    }

    Ok(())
}

fn validate_pe_identity(base: usize) -> bool {
    if read_u16(base) != Some(0x5A4D) {
        return false;
    }
    let Some(nt_offset) = read_u32(base + 0x3C).map(|value| value as usize) else {
        return false;
    };
    if !(0x40..=0x1000).contains(&nt_offset) {
        return false;
    }
    let nt = base + nt_offset;
    if read_u32(nt) != Some(0x0000_4550)
        || read_u16(nt + 4) != Some(0x8664)
        || read_u32(nt + 8) != Some(ER_PE_TIMESTAMP)
    {
        return false;
    }
    let optional = nt + 0x18;
    read_u16(optional) == Some(0x020B)
        && read_u32(optional + 0x10) == Some(ER_ENTRY_POINT_RVA)
        && read_u32(optional + 0x38) == Some(ER_SIZE_OF_IMAGE)
}

#[cfg(test)]
#[test]
#[ignore = "Requires ERPS_COMPAT_EXE; owned read-only PE validation, never executes the image"]
fn real_271_pe_runtime_guard_and_negative_controls() {
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    let path = std::env::var("ERPS_COMPAT_EXE").expect("ERPS_COMPAT_EXE");
    let file = std::fs::read(path).unwrap();
    let u16at = |b: &[u8], at: usize| u16::from_le_bytes(b[at..at + 2].try_into().unwrap());
    let u32at = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let u64at = |b: &[u8], at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
    let nt = u32at(&file, 0x3C) as usize;
    let opt = nt + 0x18;
    let size = u32at(&file, opt + 0x38) as usize;
    assert_eq!(size, ER_SIZE_OF_IMAGE as usize);
    let mut image = vec![0u8; size]; // RW heap only; never executable.
    let headers = u32at(&file, opt + 0x3C) as usize;
    image[..headers].copy_from_slice(&file[..headers]);
    let section_table = opt + u16at(&file, nt + 20) as usize;
    for s in 0..u16at(&file, nt + 6) as usize {
        let at = section_table + s * 40;
        let dest = u32at(&file, at + 12) as usize;
        let len = u32at(&file, at + 16) as usize;
        let src = u32at(&file, at + 20) as usize;
        image[dest..dest + len].copy_from_slice(&file[src..src + len]);
    }
    let base = image.as_ptr() as usize;
    let delta = (base as u64).wrapping_sub(u64at(&file, opt + 24));
    let mut block = u32at(&file, opt + 112 + 5 * 8) as usize;
    let end = block + u32at(&file, opt + 112 + 5 * 8 + 4) as usize;
    while block < end {
        let page = u32at(&image, block) as usize;
        let length = u32at(&image, block + 4) as usize;
        assert!(length >= 8 && block + length <= end);
        for at in (block + 8..block + length).step_by(2) {
            let item = u16at(&image, at);
            if item >> 12 == 10 {
                let address = page + (item & 0xFFF) as usize;
                let value = u64at(&image, address).wrapping_add(delta);
                image[address..address + 8].copy_from_slice(&value.to_le_bytes());
            } else {
                assert_eq!(item >> 12, 0);
            }
        }
        block += length;
    }
    assert_eq!(validate_runtime(base), Ok(()));
    assert!(crate::unit_runtime::entry_state_supported(base));
    for at in [0x3F0706, 0x3F0719, 0x3F8C59] {
        image[at] ^= 1;
        assert!(!crate::unit_runtime::entry_state_supported(base));
        image[at] ^= 1;
    }
    assert!(crate::unit_runtime::EnemyApi::validate(base).is_some());
    for at in [
        0x3F1C90, 0x3F1CB2, 0x3F1CDF, 0x51B5D0, 0x51B5E9, 0x51B580, 0x51B590, 0x51B5A0, 0x51B5B0,
        0x9F1C60, 0x9F1448,
    ] {
        image[at] ^= 1;
        assert!(
            crate::unit_runtime::EnemyApi::validate(base).is_none(),
            "enemy native witness {at:X}"
        );
        image[at] ^= 1;
    }
    for at in [
        nt + 8,
        ER_POSE_IMPORTER_UPDATE_RVA,
        ER_CLOTH_SKIN_PN_RVA,
        ER_CLOTH_MESH_FRAME_COMMIT_RVA,
        ER_ANIM_SKELETON_VTABLE_RVA + 0x40,
    ] {
        image[at] ^= 1;
        assert!(validate_runtime(base).is_err(), "failed negative at {at:X}");
        image[at] ^= 1;
        assert_eq!(validate_runtime(base), Ok(()));
    }
    println!(
        "REAL-271-VALIDATOR PASS: owned PE,5 rejected corruptions,no executable memory or game writes"
    );
}

fn bytes_equal(addr: usize, expected: &[u8]) -> bool {
    if !is_memory_accessible(addr, expected.len(), false) {
        return false;
    }
    expected
        .iter()
        .enumerate()
        .all(|(index, expected)| unsafe { (addr as *const u8).add(index).read() == *expected })
}

fn object_has_vtable(object: usize, expected_vtable: usize) -> bool {
    object != 0 && read_usize(object) == Some(expected_vtable)
}

fn pose_importer_uses_update_hook(object: usize, module_base: usize) -> bool {
    let Some(vtable) = read_usize(object) else {
        return false;
    };
    read_usize(vtable + 3 * size_of::<usize>()) == Some(module_base + ER_POSE_IMPORTER_UPDATE_RVA)
}

fn choose_cloth_pose_importer(
    preferred: usize,
    primary: usize,
    alternate: usize,
    alternate_state: usize,
) -> (usize, ClothPoseSource) {
    if preferred != 0 {
        (preferred, ClothPoseSource::Override3a8)
    } else if alternate_state != 0 {
        (alternate, ClothPoseSource::Alternate3a0)
    } else {
        (primary, ClothPoseSource::Primary398)
    }
}

fn cloth_input_setter_hook(registers: *mut Registers, original: usize) -> usize {
    let registers = unsafe { &*registers };
    let owner = registers.rcx as usize;
    let input = registers.rdx as usize;
    let original: unsafe extern "C" fn(usize, usize) -> usize = unsafe { transmute(original) };
    let result = unsafe { original(owner, input) };
    record_cloth_instance_binding(owner, input);
    result
}

fn cloth_secondary_reference_submit_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let registers = unsafe { &*registers };
    let inner = registers.rcx as usize;
    let source_address = registers.rdx as usize;
    let original: unsafe extern "C" fn(usize, usize) -> usize = unsafe { transmute(original) };

    let Some((slot, owner)) = target_cloth_slot_and_owner_for_inner(inner) else {
        return unsafe { original(inner, source_address) };
    };
    unit_state
        .cloth_secondary_reference_calls
        .fetch_add(1, Ordering::Relaxed);

    // The native +0x4B dirty branch runs on the callback after a transition
    // commit. The global SpEffect request can already have changed again, so
    // the secondary reference must use this exact instance's committed scale.
    let requested_bits = unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire);
    let Some(requested_scale) = committed_cloth_scale(requested_bits) else {
        unit_state
            .cloth_secondary_reference_rejected
            .fetch_add(1, Ordering::Relaxed);
        return unsafe { original(inner, source_address) };
    };
    let Some(source) = read_matrix(source_address) else {
        unit_state
            .cloth_secondary_reference_rejected
            .fetch_add(1, Ordering::Relaxed);
        return unsafe { original(inner, source_address) };
    };
    let Some(root) = read_matrix(owner.saturating_add(CLOTH_MAIN_TRANSFORM_OFFSET)) else {
        unit_state
            .cloth_secondary_reference_rejected
            .fetch_add(1, Ordering::Relaxed);
        return unsafe { original(inner, source_address) };
    };
    let Some(planned) = secondary_reference_matrix_for_scale(
        source,
        requested_scale,
        [root[12], root[13], root[14]],
    ) else {
        unit_state
            .cloth_secondary_reference_rejected
            .fetch_add(1, Ordering::Relaxed);
        return unsafe { original(inner, source_address) };
    };

    let passthrough = planned == source;
    if !passthrough
        && (unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire)
            != requested_bits
            || target_cloth_slot_and_owner_for_inner(inner) != Some((slot, owner)))
    {
        unit_state
            .cloth_secondary_reference_rejected
            .fetch_add(1, Ordering::Relaxed);
        return unsafe { original(inner, source_address) };
    }

    // ER uses MOVAPS for the four source rows, so a replacement matrix passed
    // to the original copy routine must retain 16-byte alignment for the full
    // duration of the synchronous call.
    let adjusted = AlignedClothMatrix(planned);
    let effective_source_address = if passthrough {
        unit_state
            .cloth_secondary_reference_passthrough
            .fetch_add(1, Ordering::Relaxed);
        source_address
    } else {
        unit_state
            .cloth_secondary_reference_adjusted
            .fetch_add(1, Ordering::Relaxed);
        adjusted.0.as_ptr() as usize
    };

    // Only non-unit target calls enter this immediate probe. It executes at
    // most once per native secondary-reference submission and does not walk
    // particles, collision shapes, or downstream child arrays.
    let probe_active = (requested_scale - 1.0).abs() > 0.01;
    let core_before = read_usize(inner.saturating_add(0x30)).unwrap_or(0);
    if probe_active {
        unit_state
            .cloth_secondary_probe_calls
            .fetch_add(1, Ordering::Relaxed);
        if source_address == owner.saturating_add(CLOTH_SECONDARY_TRANSFORM_OFFSET) {
            unit_state
                .cloth_secondary_source_owner_e0_matches
                .fetch_add(1, Ordering::Relaxed);
        } else {
            unit_state
                .cloth_secondary_source_owner_e0_mismatches
                .fetch_add(1, Ordering::Relaxed);
        }
        let core_before_matrix =
            read_matrix(core_before.saturating_add(CLOTH_CORE_SECONDARY_TRANSFORM_OFFSET));
        record_cloth_matrix_scale_class(
            classify_cloth_matrix_scale(core_before_matrix.as_ref(), requested_scale),
            &unit_state.cloth_secondary_pre_core_requested,
            &unit_state.cloth_secondary_pre_core_unit,
            &unit_state.cloth_secondary_pre_core_other,
        );
    }

    let result = unsafe { original(inner, effective_source_address) };

    if probe_active {
        let core_after = read_usize(inner.saturating_add(0x30)).unwrap_or(0);
        if core_after != core_before {
            unit_state
                .cloth_secondary_core_changed
                .fetch_add(1, Ordering::Relaxed);
        }
        let core_after_matrix =
            read_matrix(core_after.saturating_add(CLOTH_CORE_SECONDARY_TRANSFORM_OFFSET));
        record_cloth_matrix_scale_class(
            classify_cloth_matrix_scale(core_after_matrix.as_ref(), requested_scale),
            &unit_state.cloth_secondary_post_core_requested,
            &unit_state.cloth_secondary_post_core_unit,
            &unit_state.cloth_secondary_post_core_other,
        );
        if core_after_matrix == Some(planned) {
            unit_state
                .cloth_secondary_post_copy_matches
                .fetch_add(1, Ordering::Relaxed);
        } else {
            unit_state
                .cloth_secondary_post_copy_mismatches
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    result
}

fn target_cloth_slot_and_owner_for_inner(inner: usize) -> Option<(usize, usize)> {
    let unit_state = current_unit_state();
    let base = MODULE_BASE.load(Ordering::Acquire);
    let target_input = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire);
    if base == 0 || inner == 0 || target_input == 0 {
        return None;
    }
    let expected_outer_vtable = base.saturating_add(ER_CLOTH_MODEL_VTABLE_RVA);
    let expected_inner_vtable = base.saturating_add(ER_CLOTH_INNER_VTABLE_RVA);
    if !object_has_vtable(inner, expected_inner_vtable)
        || read_usize(inner.saturating_add(0x30)).unwrap_or(0) == 0
    {
        return None;
    }

    for slot in 0..CLOTH_INSTANCE_SLOTS {
        let owner = unit_state.cloth_instance_owners[slot].load(Ordering::Acquire);
        if owner != 0
            && (unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire) == target_input
                || unit_state.cloth_instance_was_equipment[slot].load(Ordering::Acquire))
            && read_usize(owner.saturating_add(0x40)) == Some(inner)
            && target_cloth_input(slot, target_input).is_some()
            && object_has_vtable(owner, expected_outer_vtable)
        {
            return Some((slot, owner));
        }
    }
    None
}

// Shared selection seam for source, transition, collision and diagnostic paths.
fn target_cloth_input(slot: usize, target_input: usize) -> Option<usize> {
    let unit_state = current_unit_state();
    let owner = unit_state.cloth_instance_owners[slot].load(Ordering::Acquire);
    let input = unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire);
    if owner == 0
        || input == 0
        || target_input == 0
        || unit_state
            .target_cloth_pose_importer
            .load(Ordering::Acquire)
            != target_input
    {
        return None;
    }
    let scope = unit_state.target_cloth_scope.try_read().ok()?;
    if let Some(route) = scope.find(owner, input) {
        // A cached address/vtable alone does not prove current ownership.
        // Recheck the direct slot, back-reference, input and core at use time.
        return (scope.anchor == target_input
            && scope.route_is_current(route, MODULE_BASE.load(Ordering::Acquire), read_usize))
        .then_some(input);
    }
    // Preserve the pre-existing exact-selected-input path (including owners
    // not in an equipment slot). Independent inputs require a proven route.
    (!unit_state.cloth_instance_was_equipment[slot].load(Ordering::Acquire)
        && input == target_input
        && read_usize(owner.saturating_add(0x120)) == Some(input))
    .then_some(input)
}

/// Pre-physics only. Bounded model/owner pointers; no particles, matrices or
/// allocations. Hooks take a nonblocking read guard and never scan equipment.
pub fn refresh_owned_cloth_inputs(player: usize, anchor: usize) {
    crate::memory_query::scoped(|| refresh_owned_cloth_inputs_inner(player, anchor));
}

fn refresh_owned_cloth_inputs_inner(player: usize, anchor: usize) {
    let unit_state = current_unit_state();
    if anchor == 0
        || unit_state
            .target_cloth_pose_importer
            .load(Ordering::Acquire)
            != anchor
    {
        return;
    }
    let next = ClothOwnerScope::capture(
        player,
        anchor,
        MODULE_BASE.load(Ordering::Acquire),
        read_usize,
    );
    let Ok(mut current) = unit_state.target_cloth_scope.try_write() else {
        return;
    };
    let changed = *current != next;
    let previous = *current;
    *current = next;
    drop(current);
    if changed {
        // Do not leave revoked pending slots preventing AABB work forever.
        for route in previous.routes.iter().filter(|r| r.owner != 0) {
            if next.find(route.owner, route.input).is_none() {
                for (slot, owner) in unit_state.cloth_instance_owners.iter().enumerate() {
                    if owner.load(Ordering::Acquire) == route.owner {
                        cancel_pending_cloth_transition(slot);
                    }
                }
            }
        }
        unit_state
            .cloth_topology_generation
            .fetch_add(1, Ordering::Release);
    }
    // Discovery must not depend on the setter having run after hook install,
    // nor on the global diagnostic ring retaining an equipment owner forever.
    for route in next.routes.iter().filter(|r| r.owner != 0) {
        let recorded = (0..CLOTH_INSTANCE_SLOTS).any(|slot| {
            unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) == route.owner
                && unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire) == route.input
        });
        if !recorded {
            observe_cloth_instance_binding(route.owner, route.input, false);
        }
        for slot in 0..CLOTH_INSTANCE_SLOTS {
            if unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) == route.owner {
                unit_state.cloth_instance_was_equipment[slot].store(true, Ordering::Release);
            }
        }
    }
}

fn target_solver_source_slot(
    inner: usize,
    current_transform: usize,
    update_context: usize,
) -> Option<usize> {
    let unit_state = current_unit_state();
    let (slot, owner) = target_cloth_slot_and_owner_for_inner(inner)?;
    let input = unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire);
    (current_transform == owner.saturating_add(CLOTH_MAIN_TRANSFORM_OFFSET)
        && input != 0
        && read_usize(owner.saturating_add(0x120)) == Some(input)
        && update_context == input.saturating_add(POSE_INNER_OFFSET))
    .then_some(slot)
}

fn collect_solver_source_indices(
    core: usize,
    update_context: usize,
    indices: &mut Vec<usize>,
    lazy_indices: &mut Vec<usize>,
) -> Option<(usize, usize)> {
    indices.clear();
    lazy_indices.clear();
    let metadata = read_usize(update_context.checked_add(POSE_METADATA_OFFSET)?)?;
    let source_count =
        usize::try_from(read_i32(metadata.checked_add(POSE_METADATA_COUNT_OFFSET)?)?).ok()?;
    if source_count == 0 || source_count > MAX_CLOTH_SOLVER_SOURCE_TRANSFORMS {
        return None;
    }
    let transforms = read_usize(update_context.checked_add(POSE_OUTPUT_OFFSET)?)?;
    let flags = read_usize(update_context.checked_add(0x28)?)?;
    let transform_bytes = source_count.checked_mul(CLOTH_SOLVER_SOURCE_TRANSFORM_STRIDE)?;
    let flag_bytes = source_count.checked_mul(size_of::<u32>())?;
    if transforms == 0
        || flags == 0
        || !is_memory_accessible(transforms, transform_bytes, true)
        || !is_memory_accessible(flags, flag_bytes, false)
    {
        return None;
    }

    let entry_count = usize::try_from(read_i32(core.checked_add(0x48)?)?).ok()?;
    if entry_count == 0 || entry_count > MAX_CLOTH_SOLVER_INPUT_ENTRIES {
        return None;
    }
    let entries = read_usize(core.checked_add(0x40)?)?;
    let entry_bytes = entry_count.checked_mul(CLOTH_SOLVER_ENTRY_STRIDE)?;
    if entries == 0 || !is_memory_accessible(entries, entry_bytes, false) {
        return None;
    }

    for entry_index in 0..entry_count {
        let entry = entries.checked_add(entry_index.checked_mul(CLOTH_SOLVER_ENTRY_STRIDE)?)?;
        let output_owner = unsafe { (entry as *const usize).read_unaligned() };
        let mapping = unsafe { (entry.checked_add(0x08)? as *const usize).read_unaligned() };
        if output_owner == 0 || mapping == 0 {
            return None;
        }
        let output_count = usize::try_from(read_i32(output_owner.checked_add(0x20)?)?).ok()?;
        let mapping_count = usize::try_from(read_u32(mapping.checked_add(0x10)?)?).ok()?;
        // These are skeleton-wide maps, not per-child simulation arrays.
        // Match the render writeback bound so large equipment cannot receive
        // render correction while silently bypassing source preparation.
        if output_count > cloth_render_scale::MAX_TRANSFORMS
            || mapping_count > cloth_render_scale::MAX_TRANSFORMS
        {
            return None;
        }
        let mapped_count = output_count.min(mapping_count);
        if mapped_count == 0 {
            continue;
        }
        let mapped_indices = read_usize(mapping.checked_add(0x18)?)?;
        let mapped_bytes = mapped_count.checked_mul(size_of::<i16>())?;
        if mapped_indices == 0 || !is_memory_accessible(mapped_indices, mapped_bytes, false) {
            return None;
        }

        for output_index in 0..mapped_count {
            let mapped_address = mapped_indices.checked_add(output_index.checked_mul(2)?)?;
            let source_index = unsafe { (mapped_address as *const i16).read_unaligned() };
            if source_index < 0 {
                continue;
            }
            let source_index = source_index as usize;
            if source_index >= source_count {
                return None;
            }
            let flag = unsafe {
                (flags.checked_add(source_index.checked_mul(size_of::<u32>())?)? as *const u32)
                    .read_unaligned()
            };
            if !indices.contains(&source_index) {
                indices.push(source_index);
            }
            // Bit 1 means ER will lazily materialize this source through
            // FUN_1416549A0 instead of consuming update_context+0x18 directly.
            // Retain it as a selected source, but resolve that exact cache
            // entry before the reversible scale bracket writes it.
            if flag & 2 != 0 && !lazy_indices.contains(&source_index) {
                lazy_indices.push(source_index);
            }
        }
    }

    (!indices.is_empty()).then_some((transforms, source_count))
}

fn materialize_lazy_solver_sources(
    update_context: usize,
    transforms: usize,
    source_count: usize,
    selected_indices: &[usize],
    lazy_indices: &[usize],
    mut resolve: impl FnMut(usize, i32) -> usize,
) -> Option<usize> {
    if update_context == 0 || transforms == 0 || source_count == 0 {
        return None;
    }
    for &index in lazy_indices {
        if index >= source_count || !selected_indices.contains(&index) {
            return None;
        }
        let index_i32 = i32::try_from(index).ok()?;
        let expected =
            transforms.checked_add(index.checked_mul(CLOTH_SOLVER_SOURCE_TRANSFORM_STRIDE)?)?;
        let resolved = resolve(update_context, index_i32);
        if resolved != expected
            || !is_memory_accessible(resolved, size_of::<SolverSourceTransform>(), true)
        {
            return None;
        }
    }
    Some(lazy_indices.len())
}

fn materialize_live_lazy_solver_sources(
    update_context: usize,
    transforms: usize,
    source_count: usize,
    selected_indices: &[usize],
    lazy_indices: &[usize],
) -> Option<usize> {
    let base = MODULE_BASE.load(Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let resolve: unsafe extern "C" fn(usize, i32) -> usize =
        unsafe { transmute(base.checked_add(ER_POSE_RESOLVE_TRANSFORM_RVA)?) };
    materialize_lazy_solver_sources(
        update_context,
        transforms,
        source_count,
        selected_indices,
        lazy_indices,
        |context, index| unsafe { resolve(context, index) },
    )
}

fn scale_solver_source_transforms(
    transforms: &mut [SolverSourceTransform],
    indices: &[usize],
    scale: f32,
    backups: &mut Vec<SolverSourceTransformBackup>,
) -> Option<usize> {
    backups.clear();
    if !valid_active_scale(scale) {
        return None;
    }

    for &index in indices {
        if index >= transforms.len() {
            backups.clear();
            return None;
        }
        if backups.iter().any(|saved| saved.index == index) {
            continue;
        }
        let translation = [
            transforms[index][0],
            transforms[index][1],
            transforms[index][2],
            transforms[index][3],
        ];
        let Some(scaled_translation) = scaled_translation(translation, scale) else {
            backups.clear();
            return None;
        };
        if !scaled_translation[..3]
            .iter()
            .all(|value| value.is_finite())
        {
            backups.clear();
            return None;
        }
        backups.push(SolverSourceTransformBackup { index, translation });
    }

    for saved in backups.iter() {
        let translation = scaled_translation(saved.translation, scale)?;
        transforms[saved.index][..4].copy_from_slice(&translation);
    }
    Some(backups.len())
}

fn restore_solver_source_transforms(
    transforms: &mut [SolverSourceTransform],
    backups: &[SolverSourceTransformBackup],
) {
    for saved in backups {
        if let Some(transform) = transforms.get_mut(saved.index) {
            transform[..4].copy_from_slice(&saved.translation);
        }
    }
}

#[cfg(test)]
fn should_scale_independent_solver_source(
    equipment_owned: bool,
    captured_input: usize,
    target_input: usize,
) -> bool {
    equipment_owned && captured_input != 0 && target_input != 0 && captured_input != target_input
}

#[cfg(test)]
fn scale_solver_source_local_scales(
    transforms: &mut [SolverSourceTransform],
    indices: &[usize],
    scale: f32,
    backups: &mut Vec<SolverSourceLocalScaleBackup>,
) -> Option<usize> {
    backups.clear();
    if !valid_active_scale(scale) {
        return None;
    }

    // Build 2.30 negative control only. A malformed
    // local scale may disable this completion path, but it must never cancel
    // the independently validated source-translation placement correction.
    for &index in indices {
        if index >= transforms.len() {
            backups.clear();
            return None;
        }
        if backups.iter().any(|saved| saved.index == index) {
            continue;
        }
        let original = [
            transforms[index][8],
            transforms[index][9],
            transforms[index][10],
            transforms[index][11],
        ];
        let Some(scaled) = scaled_translation(original, scale) else {
            backups.clear();
            return None;
        };
        backups.push(SolverSourceLocalScaleBackup {
            index,
            original,
            scaled,
        });
    }

    for saved in backups.iter() {
        transforms[saved.index][8..12].copy_from_slice(&saved.scaled);
    }
    Some(backups.len())
}

#[cfg(test)]
fn restore_solver_source_local_scales(
    transforms: &mut [SolverSourceTransform],
    backups: &[SolverSourceLocalScaleBackup],
) {
    for saved in backups {
        if let Some(transform) = transforms.get_mut(saved.index) {
            transform[8..12].copy_from_slice(&saved.original);
        }
    }
}

fn cloth_inner_commit_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let registers = unsafe { &*registers };
    let inner = registers.rcx as usize;
    let event = registers.rdx as usize;
    let current_transform = registers.r8 as usize;
    let update_context = registers.r9 as usize;
    let original_force_main = fifth_stack_argument_u8(registers.rsp as usize);
    let original: unsafe extern "C" fn(usize, usize, usize, usize, u8) -> usize =
        unsafe { transmute(original) };

    let mut committed_transition = None;
    let mut force_main = original_force_main;
    let mut pending_mask = unit_state.cloth_pending_slot_mask.load(Ordering::Acquire);
    if pending_mask != 0 {
        let base = MODULE_BASE.load(Ordering::Acquire);
        let expected_outer_vtable = base.saturating_add(ER_CLOTH_MODEL_VTABLE_RVA);
        let expected_inner_vtable = base.saturating_add(ER_CLOTH_INNER_VTABLE_RVA);
        let target_input = unit_state
            .target_cloth_pose_importer
            .load(Ordering::Acquire);

        while pending_mask != 0 {
            let slot = pending_mask.trailing_zeros() as usize;
            pending_mask &= pending_mask - 1;
            let pending_bits =
                unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire);
            if pending_bits == NO_PENDING_CLOTH_SCALE_BITS
                || pending_bits == IN_PROGRESS_CLOTH_SCALE_BITS
            {
                continue;
            }

            let owner = unit_state.cloth_instance_owners[slot].load(Ordering::Acquire);
            let captured_input = unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire);
            let core = unit_state.cloth_instance_cores[slot].load(Ordering::Acquire);
            if owner == 0
                || captured_input == 0
                || target_cloth_input(slot, target_input) != Some(captured_input)
                || read_usize(owner.saturating_add(0x40)) != Some(inner)
                || read_usize(owner.saturating_add(0x120)) != Some(captured_input)
                || current_transform != owner.saturating_add(CLOTH_MAIN_TRANSFORM_OFFSET)
                || read_usize(inner.saturating_add(0x30)) != Some(core)
            {
                continue;
            }

            if !object_has_vtable(owner, expected_outer_vtable)
                || !object_has_vtable(inner, expected_inner_vtable)
                || !matches!(read_u8(inner.saturating_add(0x60)), Some(value) if value != 0)
                || core == 0
            {
                unit_state
                    .cloth_scale_wrapper_rejected
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }

            let requested_scale = f32::from_bits(pending_bits);
            let previous_scale = f32::from_bits(
                unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire),
            );
            let Some(current_matrix) = read_matrix(current_transform) else {
                unit_state
                    .cloth_scale_wrapper_deferred
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let Some(reference_matrix) = reference_matrix_for_scale_transition(
                current_matrix,
                previous_scale,
                requested_scale,
            ) else {
                unit_state
                    .cloth_scale_wrapper_deferred
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let reference_address = core.saturating_add(CLOTH_CORE_REFERENCE_TRANSFORM_OFFSET);
            if !is_memory_accessible(reference_address, size_of::<[f32; 16]>(), true) {
                unit_state
                    .cloth_scale_wrapper_rejected
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }

            if unit_state.cloth_instance_pending_scale_bits[slot]
                .compare_exchange(
                    pending_bits,
                    IN_PROGRESS_CLOTH_SCALE_BITS,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
            {
                continue;
            }

            if unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) != owner
                || unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire) != captured_input
                || unit_state
                    .target_cloth_pose_importer
                    .load(Ordering::Acquire)
                    != target_input
                || target_cloth_input(slot, target_input) != Some(captured_input)
                || read_usize(owner.saturating_add(0x40)) != Some(inner)
                || read_usize(owner.saturating_add(0x120)) != Some(captured_input)
                || read_usize(inner.saturating_add(0x30)) != Some(core)
            {
                unit_state.cloth_instance_pending_scale_bits[slot]
                    .store(pending_bits, Ordering::Release);
                unit_state
                    .cloth_scale_wrapper_rejected
                    .fetch_add(1, Ordering::Relaxed);
                continue;
            }

            write_matrix(reference_address, reference_matrix);
            force_main = 1;
            committed_transition = Some((
                slot,
                pending_bits,
                previous_scale,
                requested_scale,
                core,
                [current_matrix[12], current_matrix[13], current_matrix[14]],
                owner,
            ));
            break;
        }
    }

    // This bounded child scan runs only for an actual scale transition. Keeping
    // the before sample across the native call lets the next log distinguish a
    // native partial update from a later solver rebuild without restoring the
    // previous per-frame diagnostic overhead.
    let immediate_probe = if ENABLE_CLOTH_IMMEDIATE_TRANSITION_PROBE {
        committed_transition.map(|(_, _, previous_scale, requested_scale, core, root, _)| {
            (
                previous_scale,
                requested_scale,
                core,
                root,
                boxed_cloth_child_snapshots(core),
            )
        })
    } else {
        None
    };

    // Preserve the human-positive source translation in a PRIVATE consumer
    // pose. Lazy resolution must not leave parents/cache flags in the render
    // pose. Role-aware 2.30 captures also show a duplicate scale on 71 active
    // inputs: native core90 already contributes the requested scale. Preserve
    // all source Qs scale values, without selecting/normalizing by magnitude.
    let target_input = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire);
    let requested_scale = current_scale();
    let target_slot = target_solver_source_slot(inner, current_transform, update_context);
    let solver_source_eligible = ENABLE_EXTRA_CLOTH_SOLVER_SOURCE_TRANSLATION
        && target_slot.is_some()
        && valid_active_scale(requested_scale);
    let mut solver_scratch =
        SOLVER_SOURCE_SCRATCH.with(|scratch| std::mem::take(&mut *scratch.borrow_mut()));
    let solver_source_span = if solver_source_eligible {
        let core = read_usize(inner.saturating_add(0x30)).unwrap_or(0);
        solver_consumer_context(update_context, &mut solver_scratch).and_then(|consumer_context| {
            collect_solver_source_indices(
                core,
                consumer_context,
                &mut solver_scratch.indices,
                &mut solver_scratch.lazy_indices,
            )
            .and_then(|(transforms, count)| {
                let lazy_count = solver_scratch.lazy_indices.len();
                unit_state
                    .cloth_solver_source_lazy_candidates
                    .fetch_add(lazy_count as u64, Ordering::Relaxed);
                let resolved = materialize_live_lazy_solver_sources(
                    consumer_context,
                    transforms,
                    count,
                    &solver_scratch.indices,
                    &solver_scratch.lazy_indices,
                );
                let Some(resolved) = resolved else {
                    unit_state
                        .cloth_solver_source_lazy_rejected
                        .fetch_add(1, Ordering::Relaxed);
                    return None;
                };
                let direct = solver_scratch.indices.len().saturating_sub(lazy_count);
                let source = unsafe {
                    std::slice::from_raw_parts_mut(transforms as *mut SolverSourceTransform, count)
                };
                scale_solver_source_transforms(
                    source,
                    &solver_scratch.indices,
                    requested_scale,
                    &mut solver_scratch.backups,
                )
                .map(|writes| {
                    unit_state
                        .cloth_solver_source_local_scale_passthrough
                        .fetch_add(1, Ordering::Relaxed);
                    if let Some(slot) = target_slot {
                        unit_state.cloth_instance_source_calls[slot]
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    unit_state
                        .cloth_solver_source_bracket_calls
                        .fetch_add(1, Ordering::Relaxed);
                    unit_state
                        .cloth_solver_source_bracket_transforms
                        .fetch_add(writes as u64, Ordering::Relaxed);
                    unit_state
                        .cloth_solver_source_direct_transforms
                        .fetch_add(direct as u64, Ordering::Relaxed);
                    unit_state
                        .cloth_solver_source_lazy_resolved
                        .fetch_add(resolved as u64, Ordering::Relaxed);
                    (consumer_context, transforms, count, writes)
                })
            })
        })
    } else {
        None
    };
    if solver_source_eligible && solver_source_span.is_none() {
        unit_state
            .cloth_solver_source_bracket_rejected
            .fetch_add(1, Ordering::Relaxed);
    }

    // A failed private preparation discards only scratch. The native fallback
    // receives the untouched canonical context; never retry live mutations.
    let native_context = solver_source_span.map_or(update_context, |span| span.0);
    if solver_source_span.is_some() {
        unit_state
            .cloth_solver_private_context_calls
            .fetch_add(1, Ordering::Relaxed);
    }
    let result = unsafe { original(inner, event, current_transform, native_context, force_main) };

    if let Some((_, transforms, count, writes)) = solver_source_span {
        unit_state
            .cloth_solver_private_context_returns
            .fetch_add(1, Ordering::Relaxed);
        let source = unsafe {
            std::slice::from_raw_parts_mut(transforms as *mut SolverSourceTransform, count)
        };
        // Historical restore counters now describe only private scratch.
        restore_solver_source_transforms(source, &solver_scratch.backups);
        unit_state
            .cloth_solver_source_bracket_restores
            .fetch_add(writes as u64, Ordering::Relaxed);
    }
    SOLVER_SOURCE_SCRATCH.with(|scratch| {
        *scratch.borrow_mut() = solver_scratch;
    });

    if let Some((previous_scale, requested_scale, core, root, before)) = immediate_probe {
        if read_usize(inner.saturating_add(0x30)) == Some(core) {
            let after = boxed_cloth_child_snapshots(core);
            record_immediate_child_transition(&before, &after, requested_scale / previous_scale);
            if ENABLE_CLOTH_ATTACHMENT_POSITION_SYNC && valid_active_scale(requested_scale) {
                let counts = sync_immediate_cloth_attachment_positions(
                    &before,
                    &after,
                    requested_scale,
                    requested_scale / previous_scale,
                    root,
                );
                record_cloth_position_sync_counts(counts);
            }
        } else {
            unit_state
                .cloth_immediate_probe_transitions
                .fetch_add(1, Ordering::Relaxed);
            unit_state
                .cloth_immediate_unreadable
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    if let Some((slot, requested_bits, _, _, core, _, owner)) = committed_transition {
        // Native code may replace a core/owner while handling the transition.
        // Never publish the old operation's applied state into a reused slot.
        if unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) != owner
            || unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) != core
            || target_cloth_input(slot, target_input).is_none()
        {
            unit_state
                .cloth_scale_wrapper_rejected
                .fetch_add(1, Ordering::Relaxed);
            if unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) == owner
                && unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) == core
            {
                unit_state.cloth_instance_pending_scale_bits[slot]
                    .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
                unit_state
                    .cloth_pending_slot_mask
                    .fetch_and(!(1u64 << slot), Ordering::AcqRel);
            }
            return result;
        }
        unit_state.cloth_instance_applied_scale_bits[slot].store(requested_bits, Ordering::Release);
        unit_state.cloth_instance_pending_scale_bits[slot]
            .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
        unit_state
            .cloth_pending_slot_mask
            .fetch_and(!(1u64 << slot), Ordering::AcqRel);
        unit_state
            .cloth_scale_reference_commits
            .fetch_add(1, Ordering::Relaxed);
        unit_state.cloth_instance_commits[slot].fetch_add(1, Ordering::Relaxed);
        // Applied scale changes which native child generation and collision
        // caches are authoritative even when the owner/core pointers stay the
        // same. Publish that event so task-side local/AABB work runs once on
        // the newly committed generation instead of polling inactive wrappers.
        unit_state
            .cloth_topology_generation
            .fetch_add(1, Ordering::Release);

        // C4D510 consumes outer+0x4B on its next callback by calling the
        // native secondary submit, then clears the byte itself. This keeps
        // the update at the game's authoritative one-shot boundary instead
        // of repairing core/child state every frame.
        let still_bound = unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) == owner
            && target_cloth_input(slot, target_input).is_some()
            && read_usize(owner.saturating_add(0x40)) == Some(inner)
            && read_usize(inner.saturating_add(0x30)) == Some(core);
        if still_bound && mark_secondary_reference_dirty(owner) {
            unit_state
                .cloth_secondary_dirty_marked
                .fetch_add(1, Ordering::Relaxed);
        } else {
            unit_state
                .cloth_secondary_dirty_rejected
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    // Build 2.8 handles local simulated size. Build 2.10 first corrects every
    // committed transition from its exact before/after child centers above.
    // If the ordinary solver later rebuilds a child from unit attachment
    // transforms, this path repairs that generation without rescaling the
    // already-sized particle span.
    if ENABLE_CLOTH_ATTACHMENT_POSITION_SYNC {
        sync_target_cloth_attachment_positions(inner, current_transform);
    }
    result
}

fn sync_target_cloth_attachment_positions(inner: usize, current_transform: usize) {
    let unit_state = current_unit_state();
    let base = MODULE_BASE.load(Ordering::Acquire);
    let target_input = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire);
    let requested_bits = unit_state.target_scale_bits.load(Ordering::Acquire);
    let requested_scale = f32::from_bits(requested_bits);
    if target_input == 0 || !valid_active_scale(requested_scale) {
        return;
    }

    let expected_outer_vtable = base.saturating_add(ER_CLOTH_MODEL_VTABLE_RVA);
    let expected_inner_vtable = base.saturating_add(ER_CLOTH_INNER_VTABLE_RVA);
    for slot in 0..CLOTH_INSTANCE_SLOTS {
        let owner = unit_state.cloth_instance_owners[slot].load(Ordering::Acquire);
        if owner == 0
            || unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire) != target_input
            || unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire)
                != requested_bits
            || read_usize(owner.saturating_add(0x40)) != Some(inner)
            || read_usize(owner.saturating_add(0x120)) != Some(target_input)
            || current_transform != owner.saturating_add(CLOTH_MAIN_TRANSFORM_OFFSET)
            || !object_has_vtable(owner, expected_outer_vtable)
            || !object_has_vtable(inner, expected_inner_vtable)
        {
            continue;
        }

        let core = unit_state.cloth_instance_cores[slot].load(Ordering::Acquire);
        if core == 0 || read_usize(inner.saturating_add(0x30)) != Some(core) {
            return;
        }
        let Some(root_matrix) = read_matrix(current_transform) else {
            unit_state
                .cloth_transform_resync_topology_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        };
        if !cloth_matrix_array_basis_matches_scale(&root_matrix, requested_scale) {
            unit_state
                .cloth_transform_resync_topology_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        }

        let counts = sync_cloth_attachment_positions(
            core,
            requested_scale,
            [root_matrix[12], root_matrix[13], root_matrix[14]],
        );
        record_cloth_position_sync_counts(counts);
        return;
    }
}

fn record_cloth_position_sync_counts(counts: ClothTransformResyncCounts) {
    let unit_state = current_unit_state();
    unit_state
        .cloth_transform_resync_calls
        .fetch_add(1, Ordering::Relaxed);
    unit_state
        .cloth_transform_resync_children_observed
        .fetch_add(counts.children_observed, Ordering::Relaxed);
    unit_state
        .cloth_transform_resync_children_changed
        .fetch_add(counts.children_changed, Ordering::Relaxed);
    unit_state
        .cloth_transform_resync_entries_observed
        .fetch_add(counts.entries_observed, Ordering::Relaxed);
    unit_state
        .cloth_transform_resync_entries_changed
        .fetch_add(counts.entries_changed, Ordering::Relaxed);
    unit_state
        .cloth_transform_resync_entries_already_scaled
        .fetch_add(counts.entries_already_scaled, Ordering::Relaxed);
    unit_state
        .cloth_transform_resync_entries_rejected
        .fetch_add(counts.entries_rejected, Ordering::Relaxed);
    unit_state
        .cloth_transform_resync_topology_rejected
        .fetch_add(counts.topology_rejected, Ordering::Relaxed);
    unit_state
        .cloth_attachment_position_buffers_shifted
        .fetch_add(counts.position_buffers_shifted, Ordering::Relaxed);
    unit_state
        .cloth_attachment_particles_shifted
        .fetch_add(counts.particles_shifted, Ordering::Relaxed);
    unit_state
        .cloth_attachment_aabbs_shifted
        .fetch_add(counts.aabbs_shifted, Ordering::Relaxed);
    unit_state
        .cloth_attachment_position_rejected
        .fetch_add(counts.position_rejected, Ordering::Relaxed);
}

fn record_cloth_instance_binding(owner: usize, input: usize) {
    observe_cloth_instance_binding(owner, input, true);
}

fn observe_cloth_instance_binding(owner: usize, input: usize, from_setter: bool) {
    let unit_state = current_unit_state();
    if owner == 0 {
        return;
    }
    let hits = u64::from(from_setter);
    unit_state
        .cloth_setter_calls
        .fetch_add(hits, Ordering::Relaxed);

    for slot in 0..CLOTH_INSTANCE_SLOTS {
        if unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) == owner {
            if unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire) != input {
                cancel_pending_cloth_transition(slot);
                unit_state
                    .cloth_topology_generation
                    .fetch_add(1, Ordering::Release);
            }
            unit_state.cloth_instance_inputs[slot].store(input, Ordering::Release);
            unit_state.cloth_instance_setter_hits[slot].fetch_add(hits, Ordering::Relaxed);
            return;
        }
    }

    for slot in 0..CLOTH_INSTANCE_SLOTS {
        if unit_state.cloth_instance_owners[slot]
            .compare_exchange(0, owner, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            unit_state.cloth_instance_inputs[slot].store(input, Ordering::Release);
            unit_state.cloth_instance_setter_hits[slot].store(hits, Ordering::Release);
            unit_state.cloth_instance_was_equipment[slot].store(false, Ordering::Release);
            unit_state.cloth_instance_source_calls[slot].store(0, Ordering::Release);
            unit_state.cloth_instance_commits[slot].store(0, Ordering::Release);
            reset_cloth_slot_scale_state(slot, 0);
            unit_state
                .cloth_slot_inserts
                .fetch_add(1, Ordering::Relaxed);
            unit_state
                .cloth_topology_generation
                .fetch_add(1, Ordering::Release);
            return;
        }
    }

    let start = unit_state
        .cloth_replacement_cursor
        .fetch_add(1, Ordering::Relaxed)
        % CLOTH_INSTANCE_SLOTS;
    // A stream of NPC setters cannot evict the <=27 current equipment owners.
    let Ok(scope) = unit_state.target_cloth_scope.try_read() else {
        return;
    };
    let Some(slot) = (0..CLOTH_INSTANCE_SLOTS)
        .map(|n| (start + n) % CLOTH_INSTANCE_SLOTS)
        .find(|&slot| {
            scope
                .find(
                    unit_state.cloth_instance_owners[slot].load(Ordering::Acquire),
                    unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire),
                )
                .is_none()
        })
    else {
        return;
    };
    drop(scope);
    // The task-side reader validates the live owner vtable and owner+0x120
    // before publishing a snapshot, so a concurrent replacement can only
    // suppress one sample; it cannot make an unrelated object pass the filter.
    reset_cloth_slot_scale_state(slot, 0);
    unit_state.cloth_instance_inputs[slot].store(input, Ordering::Release);
    unit_state.cloth_instance_setter_hits[slot].store(hits, Ordering::Release);
    unit_state.cloth_instance_was_equipment[slot].store(false, Ordering::Release);
    unit_state.cloth_instance_source_calls[slot].store(0, Ordering::Release);
    unit_state.cloth_instance_commits[slot].store(0, Ordering::Release);
    unit_state.cloth_instance_owners[slot].store(owner, Ordering::Release);
    unit_state
        .cloth_slot_replacements
        .fetch_add(1, Ordering::Relaxed);
    unit_state
        .cloth_topology_generation
        .fetch_add(1, Ordering::Release);
}

pub fn cloth_topology_generation() -> u64 {
    let unit_state = current_unit_state();
    unit_state.cloth_topology_generation.load(Ordering::Acquire)
}

fn cancel_pending_cloth_transition(slot: usize) {
    let unit_state = current_unit_state();
    let pending_bits = unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire);
    if pending_bits == NO_PENDING_CLOTH_SCALE_BITS || pending_bits == IN_PROGRESS_CLOTH_SCALE_BITS {
        return;
    }
    if unit_state.cloth_instance_pending_scale_bits[slot]
        .compare_exchange(
            pending_bits,
            NO_PENDING_CLOTH_SCALE_BITS,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
    {
        unit_state
            .cloth_pending_slot_mask
            .fetch_and(!(1u64 << slot), Ordering::AcqRel);
    }
}

fn reset_cloth_slot_scale_state(slot: usize, core: usize) {
    let unit_state = current_unit_state();
    unit_state
        .cloth_pending_slot_mask
        .fetch_and(!(1u64 << slot), Ordering::AcqRel);
    unit_state.cloth_instance_pending_scale_bits[slot]
        .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
    unit_state.cloth_instance_applied_scale_bits[slot].store(1.0f32.to_bits(), Ordering::Release);
    unit_state.cloth_instance_cores[slot].store(core, Ordering::Release);
}

fn should_stage_cloth_transition(
    applied_bits: u32,
    pending_bits: u32,
    requested_bits: u32,
) -> bool {
    applied_bits != requested_bits && pending_bits == NO_PENDING_CLOTH_SCALE_BITS
}

pub fn queue_cloth_scale_transitions(target_input: usize, requested_scale: f32) -> usize {
    crate::memory_query::scoped(|| {
        queue_cloth_scale_transitions_inner(target_input, requested_scale)
    })
}

fn queue_cloth_scale_transitions_inner(target_input: usize, requested_scale: f32) -> usize {
    let unit_state = current_unit_state();
    if target_input == 0 || !valid_scale(requested_scale) {
        return 0;
    }

    let requested_bits = requested_scale.to_bits();
    let base = MODULE_BASE.load(Ordering::Acquire);
    let expected_vtable = base.saturating_add(ER_CLOTH_MODEL_VTABLE_RVA);
    let expected_inner_vtable = base.saturating_add(ER_CLOTH_INNER_VTABLE_RVA);
    let mut queued = 0usize;
    for slot in 0..CLOTH_INSTANCE_SLOTS {
        let owner = unit_state.cloth_instance_owners[slot].load(Ordering::Acquire);
        let captured_input = unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire);
        if owner == 0
            || target_cloth_input(slot, target_input) != Some(captured_input)
            || !object_has_vtable(owner, expected_vtable)
            || read_usize(owner.saturating_add(0x120)) != Some(captured_input)
        {
            cancel_pending_cloth_transition(slot);
            continue;
        }

        let inner = read_usize(owner.saturating_add(0x40)).unwrap_or(0);
        let inner_enabled =
            matches!(read_u8(inner.saturating_add(0x60)), Some(value) if value != 0);
        if !object_has_vtable(inner, expected_inner_vtable) || !inner_enabled {
            unit_state
                .cloth_scale_transitions_deferred
                .fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let core = read_usize(inner.saturating_add(0x30)).unwrap_or(0);
        if core == 0 {
            unit_state
                .cloth_scale_transitions_deferred
                .fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) != core {
            reset_cloth_slot_scale_state(slot, core);
            unit_state
                .cloth_topology_generation
                .fetch_add(1, Ordering::Release);
        }

        let pending_bits =
            unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire);
        if pending_bits != NO_PENDING_CLOTH_SCALE_BITS
            && pending_bits != IN_PROGRESS_CLOTH_SCALE_BITS
            && pending_bits != requested_bits
        {
            cancel_pending_cloth_transition(slot);
        }

        let applied_bits =
            unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire);
        let pending_bits =
            unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire);
        if !should_stage_cloth_transition(applied_bits, pending_bits, requested_bits) {
            continue;
        }

        if unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) != owner
            || target_cloth_input(slot, target_input) != Some(captured_input)
            || !object_has_vtable(owner, expected_vtable)
            || read_usize(owner.saturating_add(0x120)) != Some(captured_input)
            || read_usize(owner.saturating_add(0x40)) != Some(inner)
            || read_usize(inner.saturating_add(0x30)) != Some(core)
        {
            unit_state
                .cloth_scale_transitions_rejected
                .fetch_add(1, Ordering::Relaxed);
            continue;
        }

        if unit_state.cloth_instance_pending_scale_bits[slot]
            .compare_exchange(
                NO_PENDING_CLOTH_SCALE_BITS,
                requested_bits,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            continue;
        }
        unit_state
            .cloth_pending_slot_mask
            .fetch_or(1u64 << slot, Ordering::Release);
        unit_state
            .cloth_scale_transitions_queued
            .fetch_add(1, Ordering::Relaxed);
        queued += 1;
    }
    queued
}

pub fn has_pending_cloth_scale_transitions() -> bool {
    let unit_state = current_unit_state();
    unit_state.cloth_pending_slot_mask.load(Ordering::Acquire) != 0
}

pub fn cloth_instance_probe_counters() -> ClothInstanceProbeCounters {
    let unit_state = current_unit_state();
    ClothInstanceProbeCounters {
        setter_calls: unit_state.cloth_setter_calls.load(Ordering::Acquire),
        slot_inserts: unit_state.cloth_slot_inserts.load(Ordering::Acquire),
        slot_replacements: unit_state.cloth_slot_replacements.load(Ordering::Acquire),
        occupied_slots: unit_state
            .cloth_instance_owners
            .iter()
            .filter(|owner| owner.load(Ordering::Acquire) != 0)
            .count(),
        scale_transitions_queued: unit_state
            .cloth_scale_transitions_queued
            .load(Ordering::Acquire),
        scale_transitions_deferred: unit_state
            .cloth_scale_transitions_deferred
            .load(Ordering::Acquire),
        scale_transitions_rejected: unit_state
            .cloth_scale_transitions_rejected
            .load(Ordering::Acquire),
        scale_reference_commits: unit_state
            .cloth_scale_reference_commits
            .load(Ordering::Acquire),
        scale_wrapper_deferred: unit_state
            .cloth_scale_wrapper_deferred
            .load(Ordering::Acquire),
        scale_wrapper_rejected: unit_state
            .cloth_scale_wrapper_rejected
            .load(Ordering::Acquire),
        secondary_reference_calls: unit_state
            .cloth_secondary_reference_calls
            .load(Ordering::Acquire),
        secondary_reference_adjusted: unit_state
            .cloth_secondary_reference_adjusted
            .load(Ordering::Acquire),
        secondary_reference_passthrough: unit_state
            .cloth_secondary_reference_passthrough
            .load(Ordering::Acquire),
        secondary_reference_rejected: unit_state
            .cloth_secondary_reference_rejected
            .load(Ordering::Acquire),
        secondary_probe_calls: unit_state
            .cloth_secondary_probe_calls
            .load(Ordering::Acquire),
        secondary_source_owner_e0_matches: unit_state
            .cloth_secondary_source_owner_e0_matches
            .load(Ordering::Acquire),
        secondary_source_owner_e0_mismatches: unit_state
            .cloth_secondary_source_owner_e0_mismatches
            .load(Ordering::Acquire),
        secondary_pre_core_requested: unit_state
            .cloth_secondary_pre_core_requested
            .load(Ordering::Acquire),
        secondary_pre_core_unit: unit_state
            .cloth_secondary_pre_core_unit
            .load(Ordering::Acquire),
        secondary_pre_core_other: unit_state
            .cloth_secondary_pre_core_other
            .load(Ordering::Acquire),
        secondary_post_core_requested: unit_state
            .cloth_secondary_post_core_requested
            .load(Ordering::Acquire),
        secondary_post_core_unit: unit_state
            .cloth_secondary_post_core_unit
            .load(Ordering::Acquire),
        secondary_post_core_other: unit_state
            .cloth_secondary_post_core_other
            .load(Ordering::Acquire),
        secondary_post_copy_matches: unit_state
            .cloth_secondary_post_copy_matches
            .load(Ordering::Acquire),
        secondary_post_copy_mismatches: unit_state
            .cloth_secondary_post_copy_mismatches
            .load(Ordering::Acquire),
        secondary_core_changed: unit_state
            .cloth_secondary_core_changed
            .load(Ordering::Acquire),
        secondary_dirty_marked: unit_state
            .cloth_secondary_dirty_marked
            .load(Ordering::Acquire),
        secondary_dirty_rejected: unit_state
            .cloth_secondary_dirty_rejected
            .load(Ordering::Acquire),
        solver_source_bracket_calls: unit_state
            .cloth_solver_source_bracket_calls
            .load(Ordering::Acquire),
        solver_source_bracket_transforms: unit_state
            .cloth_solver_source_bracket_transforms
            .load(Ordering::Acquire),
        solver_source_bracket_restores: unit_state
            .cloth_solver_source_bracket_restores
            .load(Ordering::Acquire),
        solver_source_bracket_rejected: unit_state
            .cloth_solver_source_bracket_rejected
            .load(Ordering::Acquire),
        solver_source_direct_transforms: unit_state
            .cloth_solver_source_direct_transforms
            .load(Ordering::Acquire),
        solver_source_lazy_candidates: unit_state
            .cloth_solver_source_lazy_candidates
            .load(Ordering::Acquire),
        solver_source_lazy_resolved: unit_state
            .cloth_solver_source_lazy_resolved
            .load(Ordering::Acquire),
        solver_source_lazy_rejected: unit_state
            .cloth_solver_source_lazy_rejected
            .load(Ordering::Acquire),
        solver_source_local_scale_calls: unit_state
            .cloth_solver_source_local_scale_calls
            .load(Ordering::Acquire),
        solver_source_local_scale_transforms: unit_state
            .cloth_solver_source_local_scale_transforms
            .load(Ordering::Acquire),
        solver_source_local_scale_restores: unit_state
            .cloth_solver_source_local_scale_restores
            .load(Ordering::Acquire),
        solver_source_local_scale_rejected: unit_state
            .cloth_solver_source_local_scale_rejected
            .load(Ordering::Acquire),
        solver_source_local_scale_passthrough: unit_state
            .cloth_solver_source_local_scale_passthrough
            .load(Ordering::Acquire),
        solver_private_context_calls: unit_state
            .cloth_solver_private_context_calls
            .load(Ordering::Acquire),
        solver_private_context_returns: unit_state
            .cloth_solver_private_context_returns
            .load(Ordering::Acquire),
        immediate_probe_transitions: unit_state
            .cloth_immediate_probe_transitions
            .load(Ordering::Acquire),
        immediate_children_observed: unit_state
            .cloth_immediate_children_observed
            .load(Ordering::Acquire),
        immediate_particle_matches: unit_state
            .cloth_immediate_particle_matches
            .load(Ordering::Acquire),
        immediate_transform_matches: unit_state
            .cloth_immediate_transform_matches
            .load(Ordering::Acquire),
        immediate_both_matches: unit_state
            .cloth_immediate_both_matches
            .load(Ordering::Acquire),
        immediate_unreadable: unit_state
            .cloth_immediate_unreadable
            .load(Ordering::Acquire),
        transform_resync_calls: unit_state
            .cloth_transform_resync_calls
            .load(Ordering::Acquire),
        transform_resync_children_observed: unit_state
            .cloth_transform_resync_children_observed
            .load(Ordering::Acquire),
        transform_resync_children_changed: unit_state
            .cloth_transform_resync_children_changed
            .load(Ordering::Acquire),
        transform_resync_entries_observed: unit_state
            .cloth_transform_resync_entries_observed
            .load(Ordering::Acquire),
        transform_resync_entries_changed: unit_state
            .cloth_transform_resync_entries_changed
            .load(Ordering::Acquire),
        transform_resync_entries_already_scaled: unit_state
            .cloth_transform_resync_entries_already_scaled
            .load(Ordering::Acquire),
        transform_resync_entries_rejected: unit_state
            .cloth_transform_resync_entries_rejected
            .load(Ordering::Acquire),
        transform_resync_topology_rejected: unit_state
            .cloth_transform_resync_topology_rejected
            .load(Ordering::Acquire),
        attachment_position_buffers_shifted: unit_state
            .cloth_attachment_position_buffers_shifted
            .load(Ordering::Acquire),
        attachment_particles_shifted: unit_state
            .cloth_attachment_particles_shifted
            .load(Ordering::Acquire),
        attachment_aabbs_shifted: unit_state
            .cloth_attachment_aabbs_shifted
            .load(Ordering::Acquire),
        attachment_position_rejected: unit_state
            .cloth_attachment_position_rejected
            .load(Ordering::Acquire),
    }
}

pub fn cloth_instance_snapshots(
    target_input: usize,
) -> [ClothInstanceSnapshot; CLOTH_INSTANCE_SLOTS] {
    let unit_state = current_unit_state();
    let mut snapshots = [ClothInstanceSnapshot::default(); CLOTH_INSTANCE_SLOTS];
    if target_input == 0 {
        return snapshots;
    }

    let base = MODULE_BASE.load(Ordering::Acquire);
    let expected_vtable = base.saturating_add(ER_CLOTH_MODEL_VTABLE_RVA);
    for (slot, snapshot) in snapshots.iter_mut().enumerate() {
        let owner = unit_state.cloth_instance_owners[slot].load(Ordering::Acquire);
        let captured_input = unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire);
        if owner == 0
            || target_cloth_input(slot, target_input) != Some(captured_input)
            || !object_has_vtable(owner, expected_vtable)
            || read_usize(owner.saturating_add(0x120)) != Some(captured_input)
        {
            continue;
        }

        let inner = read_usize(owner.saturating_add(0x40)).unwrap_or(0);
        let inner_vtable = read_usize(inner).unwrap_or(0);
        // ER FUN_14267BF20 copies CSClothModelIns+0x60 into core+0x90,
        // then FUN_1426B38D0 decomposes that matrix into a QsTransform and
        // propagates it to the cloth children. The wrapper owns the core at
        // +0x30 and the downstream ring state at +0x38. This probe remains
        // read-only and only accepts bounded vector spans proven by those
        // functions' layouts.
        let core = read_usize(inner.saturating_add(0x30)).unwrap_or(0);
        let downstream_state = read_usize(inner.saturating_add(0x38)).unwrap_or(0);
        let outer_flags = read_u32(owner.saturating_add(0x48)).unwrap_or(0);
        *snapshot = ClothInstanceSnapshot {
            valid: true,
            slot,
            owner,
            input: captured_input,
            setter_hits: unit_state.cloth_instance_setter_hits[slot].load(Ordering::Acquire),
            equipment_owned: unit_state.cloth_instance_was_equipment[slot].load(Ordering::Acquire),
            source_calls: unit_state.cloth_instance_source_calls[slot].load(Ordering::Acquire),
            reference_commits: unit_state.cloth_instance_commits[slot].load(Ordering::Acquire),
            applied_scale_bits: unit_state.cloth_instance_applied_scale_bits[slot]
                .load(Ordering::Acquire),
            pending_scale_bits: unit_state.cloth_instance_pending_scale_bits[slot]
                .load(Ordering::Acquire),
            inner,
            inner_vtable,
            outer_flags,
            outer_flag_4a: read_u8(owner.saturating_add(0x4A)).unwrap_or(0),
            inner_flag_49: read_u8(inner.saturating_add(0x49)).unwrap_or(0),
            inner_flag_61: read_u8(inner.saturating_add(0x61)).unwrap_or(0),
            inner_flag_62: read_u8(inner.saturating_add(0x62)).unwrap_or(0),
            core,
            downstream_state,
            core_flag_4c: read_u8(core.saturating_add(0x4C)).unwrap_or(0),
            core_flag_4d: read_u8(core.saturating_add(0x4D)).unwrap_or(0),
            core_group_count: bounded_vector_count(core, 0x28, 0x30, 0x8, 256),
            downstream_entry_count: bounded_vector_count(downstream_state, 0x20, 0x28, 0x30, 256),
            core_matrix_90: summarize_cloth_matrix(
                core.saturating_add(CLOTH_CORE_MAIN_TRANSFORM_OFFSET),
            ),
            core_matrix_d0: summarize_cloth_matrix(core.saturating_add(0xD0)),
            core_matrix_50: summarize_cloth_matrix(
                core.saturating_add(CLOTH_CORE_REFERENCE_TRANSFORM_OFFSET),
            ),
            matrix_60: summarize_cloth_matrix(owner.saturating_add(CLOTH_MAIN_TRANSFORM_OFFSET)),
            matrix_e0: summarize_cloth_matrix(owner.saturating_add(0xE0)),
        };
    }
    snapshots
}

/// Summarize the QsTransform array rebuilt by ER RVA `0x26B14E0` before the
/// dirty relative transform is propagated to hcl children.
///
/// The native function walks `core+0x40` as `0x38`-byte entries. Each entry's
/// first pointer owns a `0x40`-stride QsTransform array at `+0x18`, counted by
/// the signed value at `+0x20`. This probe validates each complete span once,
/// then produces one compact aggregate without retaining game pointers.
pub fn cloth_solver_input_snapshot(core: usize) -> ClothSolverInputSnapshot {
    let Some(entry_count) =
        bounded_i32_count(core.saturating_add(0x48), MAX_CLOTH_SOLVER_INPUT_ENTRIES)
    else {
        return ClothSolverInputSnapshot::default();
    };
    if entry_count == 0 {
        return ClothSolverInputSnapshot {
            readable: true,
            ..ClothSolverInputSnapshot::default()
        };
    }

    let entries = read_usize(core.saturating_add(0x40)).unwrap_or(0);
    let Some(entry_bytes) = entry_count.checked_mul(0x38) else {
        return ClothSolverInputSnapshot::default();
    };
    if entries == 0 || !is_memory_accessible(entries, entry_bytes, false) {
        return ClothSolverInputSnapshot::default();
    }

    let mut scale_minimum = [f32::INFINITY; 3];
    let mut scale_maximum = [f32::NEG_INFINITY; 3];
    let mut translation_minimum = [f32::INFINITY; 3];
    let mut translation_maximum = [f32::NEG_INFINITY; 3];
    let mut transform_count = 0usize;

    for entry_index in 0..entry_count {
        let entry = entries.saturating_add(entry_index * 0x38);
        let owner = unsafe { (entry as *const usize).read_unaligned() };
        if owner == 0 || !is_memory_accessible(owner, 0x24, false) {
            return ClothSolverInputSnapshot::default();
        }
        let Some(owner_transform_count) = bounded_i32_count(
            owner.saturating_add(0x20),
            MAX_CLOTH_SOLVER_INPUT_TRANSFORMS,
        ) else {
            return ClothSolverInputSnapshot::default();
        };
        if solver_input_transform_total(transform_count, owner_transform_count).is_none() {
            return ClothSolverInputSnapshot::default();
        }
        if owner_transform_count == 0 {
            continue;
        }
        let transforms = read_usize(owner.saturating_add(0x18)).unwrap_or(0);
        let Some(transform_bytes) = owner_transform_count.checked_mul(0x40) else {
            return ClothSolverInputSnapshot::default();
        };
        if transforms == 0 || !is_memory_accessible(transforms, transform_bytes, false) {
            return ClothSolverInputSnapshot::default();
        }

        for transform_index in 0..owner_transform_count {
            let transform = transforms.saturating_add(transform_index * 0x40);
            let translation = unsafe {
                [
                    (transform as *const f32).read_unaligned(),
                    (transform.saturating_add(0x04) as *const f32).read_unaligned(),
                    (transform.saturating_add(0x08) as *const f32).read_unaligned(),
                ]
            };
            let scale = unsafe {
                [
                    (transform.saturating_add(0x20) as *const f32)
                        .read_unaligned()
                        .abs(),
                    (transform.saturating_add(0x24) as *const f32)
                        .read_unaligned()
                        .abs(),
                    (transform.saturating_add(0x28) as *const f32)
                        .read_unaligned()
                        .abs(),
                ]
            };
            if !translation
                .iter()
                .chain(scale.iter())
                .all(|value| value.is_finite())
            {
                return ClothSolverInputSnapshot::default();
            }
            for component in 0..3 {
                translation_minimum[component] =
                    translation_minimum[component].min(translation[component]);
                translation_maximum[component] =
                    translation_maximum[component].max(translation[component]);
                scale_minimum[component] = scale_minimum[component].min(scale[component]);
                scale_maximum[component] = scale_maximum[component].max(scale[component]);
            }
            transform_count = transform_count.saturating_add(1);
        }
    }

    let translation_bounds = if transform_count == 0 {
        ClothPositionBounds {
            readable: true,
            ..ClothPositionBounds::default()
        }
    } else {
        ClothPositionBounds {
            readable: true,
            count: transform_count,
            min_x: translation_minimum[0],
            min_y: translation_minimum[1],
            min_z: translation_minimum[2],
            max_x: translation_maximum[0],
            max_y: translation_maximum[1],
            max_z: translation_maximum[2],
        }
    };
    let (scale_minimum, scale_maximum) = if transform_count == 0 {
        ([0.0; 3], [0.0; 3])
    } else {
        (scale_minimum, scale_maximum)
    };

    ClothSolverInputSnapshot {
        readable: true,
        entry_count,
        transform_count,
        scale_min_x: scale_minimum[0],
        scale_min_y: scale_minimum[1],
        scale_min_z: scale_minimum[2],
        scale_max_x: scale_maximum[0],
        scale_max_y: scale_maximum[1],
        scale_max_z: scale_maximum[2],
        translation_bounds,
    }
}

fn solver_input_transform_total(current: usize, additional: usize) -> Option<usize> {
    current
        .checked_add(additional)
        .filter(|total| *total <= MAX_CLOTH_SOLVER_INPUT_TRANSFORMS)
}

/// Return the deduplicated hclSimClothData roots for the selected player,
/// including directly owned equipment with independent pose inputs.
///
/// Unlike `cloth_child_snapshots`, this task-side path deliberately does not
/// summarize particle buffers, AABBs, or transform entries. It is safe to run
/// every pre-physics frame to notice equipment changes without recreating the
/// heavy profiling work that caused the earlier scaling hitch.
pub fn target_cloth_simulation_set(target_input: usize) -> TargetClothSimulationSet {
    let unit_state = current_unit_state();
    let mut result = TargetClothSimulationSet::default();
    if target_input == 0 {
        return result;
    }

    let base = MODULE_BASE.load(Ordering::Acquire);
    let expected_owner_vtable = base.saturating_add(ER_CLOTH_MODEL_VTABLE_RVA);
    let expected_inner_vtable = base.saturating_add(ER_CLOTH_INNER_VTABLE_RVA);

    for (slot, owner) in unit_state.cloth_instance_owners.iter().enumerate() {
        let owner = owner.load(Ordering::Acquire);
        if owner == 0
            || target_cloth_input(slot, target_input).is_none()
            || !object_has_vtable(owner, expected_owner_vtable)
        {
            continue;
        }

        let inner = read_usize(owner.saturating_add(0x40)).unwrap_or(0);
        if !object_has_vtable(inner, expected_inner_vtable) {
            return result;
        }
        let core = read_usize(inner.saturating_add(0x30)).unwrap_or(0);
        let Some((groups_begin, group_count)) =
            bounded_vector_span(core, 0x28, 0x30, size_of::<usize>(), MAX_CLOTH_GROUPS)
        else {
            return result;
        };

        for group_index in 0..group_count {
            let Some(group_holder) =
                read_usize(groups_begin.saturating_add(group_index * size_of::<usize>()))
            else {
                return result;
            };
            let Some(group_root) = read_usize(group_holder) else {
                return result;
            };
            let Some(child_count) = bounded_i32_count(
                group_root.saturating_add(0x48),
                MAX_CLOTH_CHILDREN_PER_GROUP,
            ) else {
                return result;
            };
            let children = read_usize(group_root.saturating_add(0x40)).unwrap_or(0);
            let Some(children_bytes) = child_count.checked_mul(size_of::<usize>()) else {
                return result;
            };
            if child_count > 0
                && (children == 0 || !is_memory_accessible(children, children_bytes, false))
            {
                return result;
            }

            for child_index in 0..child_count {
                let Some(child) =
                    read_usize(children.saturating_add(child_index * size_of::<usize>()))
                else {
                    return result;
                };
                if child == 0 || !is_memory_accessible(child, 0x20, false) {
                    return result;
                }
                let sim_data = read_usize(child.saturating_add(0x18)).unwrap_or(0);
                if sim_data == 0 || result.sim_data[..result.count].contains(&sim_data) {
                    continue;
                }
                if result.count == CLOTH_CHILD_SLOTS {
                    result.readable = true;
                    result.truncated = true;
                    return result;
                }
                result.sim_data[result.count] = sim_data;
                result.count += 1;
            }
        }
    }

    result.readable = true;
    result
}

pub fn cloth_child_snapshots(
    core: usize,
) -> (
    ClothChildProbeSummary,
    [ClothChildSnapshot; CLOTH_CHILD_SLOTS],
) {
    let mut summary = ClothChildProbeSummary::default();
    let mut snapshots = [ClothChildSnapshot::default(); CLOTH_CHILD_SLOTS];
    let Some((groups_begin, group_count)) =
        bounded_vector_span(core, 0x28, 0x30, size_of::<usize>(), MAX_CLOTH_GROUPS)
    else {
        return (summary, snapshots);
    };
    summary.group_count = group_count;

    let mut captured_children = 0usize;
    let mut child_count = 0usize;
    for group_index in 0..group_count {
        let Some(group_offset) = group_index.checked_mul(size_of::<usize>()) else {
            return (summary, snapshots);
        };
        let Some(group_holder) = read_usize(groups_begin.saturating_add(group_offset)) else {
            return (summary, snapshots);
        };
        let Some(group_root) = read_usize(group_holder) else {
            return (summary, snapshots);
        };
        let Some(group_child_count) = bounded_i32_count(
            group_root.saturating_add(0x48),
            MAX_CLOTH_CHILDREN_PER_GROUP,
        ) else {
            return (summary, snapshots);
        };
        let group_children = read_usize(group_root.saturating_add(0x40)).unwrap_or(0);
        let Some(group_children_bytes) = group_child_count.checked_mul(size_of::<usize>()) else {
            return (summary, snapshots);
        };
        if group_child_count > 0
            && (group_children == 0
                || !is_memory_accessible(group_children, group_children_bytes, false))
        {
            return (summary, snapshots);
        }
        child_count = child_count.saturating_add(group_child_count);

        for child_index in 0..group_child_count {
            if captured_children == CLOTH_CHILD_SLOTS {
                summary.topology_readable = true;
                summary.child_count = child_count;
                summary.captured_children = captured_children;
                summary.truncated = true;
                return (summary, snapshots);
            }
            let child_offset = child_index * size_of::<usize>();
            let child = unsafe {
                (group_children.saturating_add(child_offset) as *const usize).read_unaligned()
            };
            if child == 0 || !is_memory_accessible(child, 0x178, false) {
                return (summary, snapshots);
            }

            let sim_data = read_usize(child.saturating_add(0x18)).unwrap_or(0);
            let particle_count = read_u32(sim_data.saturating_add(0x48))
                .and_then(|count| usize::try_from(count).ok())
                .unwrap_or(MAX_CLOTH_PARTICLES_PER_CHILD.saturating_add(1));
            let current_positions = if particle_count <= MAX_CLOTH_PARTICLES_PER_CHILD {
                summarize_position_buffer(
                    read_usize(child.saturating_add(0x20)).unwrap_or(0),
                    particle_count,
                )
            } else {
                ClothPositionBounds::default()
            };
            let previous_positions = if particle_count <= MAX_CLOTH_PARTICLES_PER_CHILD {
                summarize_position_buffer(
                    read_usize(child.saturating_add(0x30)).unwrap_or(0),
                    particle_count,
                )
            } else {
                ClothPositionBounds::default()
            };

            snapshots[captured_children] = ClothChildSnapshot {
                valid: true,
                group_index,
                child_index,
                group_holder,
                group_root,
                child,
                child_vtable: read_usize(child).unwrap_or(0),
                sim_data,
                particle_count,
                current_positions,
                previous_positions,
                transform_entries: summarize_cloth_transform_entries(child),
            };
            captured_children += 1;
        }
    }

    summary.topology_readable = true;
    summary.child_count = child_count;
    summary.captured_children = captured_children;
    (summary, snapshots)
}

pub fn cloth_local_simulation_profile(
    child: usize,
    sim_data: usize,
) -> ClothLocalSimulationProfile {
    if child == 0
        || sim_data == 0
        || !is_memory_accessible(child, 0xD0, false)
        || !is_memory_accessible(sim_data, 0xE4, false)
    {
        return ClothLocalSimulationProfile::default();
    }

    let particle_radii =
        bounded_hk_array_span(sim_data, 0x40, 0x48, 0x10, MAX_CLOTH_PARTICLES_PER_CHILD)
            .map_or_else(ClothScalarBounds::default, |(begin, count)| {
                summarize_strided_scalar(begin, count, 0x10, 0x08)
            });

    let (sim_pose_count_readable, sim_pose_count, first_pose_positions) =
        match bounded_hk_array_span(sim_data, 0x78, 0x80, size_of::<usize>(), 256) {
            Some((poses, count)) => {
                let positions = if count == 0 {
                    ClothPositionBounds {
                        readable: true,
                        ..ClothPositionBounds::default()
                    }
                } else {
                    let pose = read_usize(poses).unwrap_or(0);
                    match bounded_hk_array_span(
                        pose,
                        0x20,
                        0x28,
                        size_of::<[f32; 4]>(),
                        MAX_CLOTH_PARTICLES_PER_CHILD,
                    ) {
                        Some((begin, position_count)) => {
                            summarize_position_buffer(begin, position_count)
                        }
                        None => ClothPositionBounds::default(),
                    }
                };
                (true, count, positions)
            }
            None => (false, 0, ClothPositionBounds::default()),
        };

    let static_constraint_count =
        bounded_hk_array_count(sim_data, 0x88, 0x90, MAX_CLOTH_CONSTRAINT_SETS);
    let anti_pinch_constraint_count =
        bounded_hk_array_count(sim_data, 0x98, 0xA0, MAX_CLOTH_CONSTRAINT_SETS);
    let per_instance_collidable_count =
        bounded_hk_array_count(sim_data, 0xD0, 0xD8, MAX_CLOTH_CONSTRAINT_SETS);
    let max_particle_radius =
        read_f32(sim_data.saturating_add(0xE0)).filter(|value| value.is_finite() && *value >= 0.0);

    ClothLocalSimulationProfile {
        readable: particle_radii.readable
            && sim_pose_count_readable
            && (sim_pose_count == 0 || first_pose_positions.readable)
            && static_constraint_count.is_some()
            && anti_pinch_constraint_count.is_some()
            && per_instance_collidable_count.is_some()
            && max_particle_radius.is_some(),
        particle_radii,
        max_particle_radius_readable: max_particle_radius.is_some(),
        max_particle_radius: max_particle_radius.unwrap_or_default(),
        first_pose_positions,
        sim_pose_count_readable,
        sim_pose_count,
        static_constraint_count_readable: static_constraint_count.is_some(),
        static_constraint_count: static_constraint_count.unwrap_or_default(),
        anti_pinch_constraint_count_readable: anti_pinch_constraint_count.is_some(),
        anti_pinch_constraint_count: anti_pinch_constraint_count.unwrap_or_default(),
        per_instance_collidable_count_readable: per_instance_collidable_count.is_some(),
        per_instance_collidable_count: per_instance_collidable_count.unwrap_or_default(),
        // Reflected hclSimClothInstance members in Havok 2018.2:
        // m_particlesAabb +0x60, m_collisionParticlesAabb +0x80, and
        // m_landscapeCollisionParticlesAabb +0xB0.
        particles_aabb: summarize_cloth_aabb(child.saturating_add(0x60)),
        collision_particles_aabb: summarize_cloth_aabb(child.saturating_add(0x80)),
        landscape_collision_particles_aabb: summarize_cloth_aabb(child.saturating_add(0xB0)),
    }
}

pub fn cloth_constraint_set_snapshots(
    sim_data: usize,
) -> (
    ClothConstraintProbeSummary,
    [ClothConstraintSetSnapshot; CLOTH_CONSTRAINT_SET_SLOTS],
) {
    let mut summary = ClothConstraintProbeSummary::default();
    let mut snapshots = [ClothConstraintSetSnapshot::default(); CLOTH_CONSTRAINT_SET_SLOTS];
    let Some((static_sets, static_count)) = bounded_hk_array_span(
        sim_data,
        0x88,
        0x90,
        size_of::<usize>(),
        MAX_CLOTH_CONSTRAINT_SETS,
    ) else {
        return (summary, snapshots);
    };
    let Some((anti_pinch_sets, anti_pinch_count)) = bounded_hk_array_span(
        sim_data,
        0x98,
        0xA0,
        size_of::<usize>(),
        MAX_CLOTH_CONSTRAINT_SETS,
    ) else {
        return (summary, snapshots);
    };
    summary.static_count = static_count;
    summary.anti_pinch_count = anti_pinch_count;

    let mut captured_count = 0usize;
    for (anti_pinch_array, sets, count) in [
        (false, static_sets, static_count),
        (true, anti_pinch_sets, anti_pinch_count),
    ] {
        for array_index in 0..count {
            if captured_count == CLOTH_CONSTRAINT_SET_SLOTS {
                summary.topology_readable = true;
                summary.captured_count = captured_count;
                summary.truncated = true;
                return (summary, snapshots);
            }
            let Some(set) = read_usize(sets.saturating_add(array_index * size_of::<usize>()))
            else {
                return (summary, snapshots);
            };
            if set == 0 || !is_memory_accessible(set, 0x28, false) {
                return (summary, snapshots);
            }
            snapshots[captured_count] =
                summarize_constraint_set(set, anti_pinch_array, array_index);
            captured_count += 1;
        }
    }

    summary.topology_readable = true;
    summary.captured_count = captured_count;
    (summary, snapshots)
}

fn summarize_constraint_set(
    set: usize,
    anti_pinch_array: bool,
    array_index: usize,
) -> ClothConstraintSetSnapshot {
    let vtable = read_usize(set).unwrap_or(0);
    let kind = constraint_kind_from_vtable_rva(module_rva(vtable));
    let mut snapshot = ClothConstraintSetSnapshot {
        valid: true,
        anti_pinch_array,
        array_index,
        set,
        vtable,
        kind,
        constraint_id: read_u32(set.saturating_add(0x20)).unwrap_or(u32::MAX),
        constraint_type: read_u32(set.saturating_add(0x24)).unwrap_or(u32::MAX),
        ..ClothConstraintSetSnapshot::default()
    };

    let layout = match kind {
        ClothConstraintKind::StandardLink | ClothConstraintKind::StretchLink => {
            Some((0x0C, Some(0x04), None, None))
        }
        ClothConstraintKind::LocalRange => Some((0x10, Some(0x04), Some(0x08), Some(0x0C))),
        ClothConstraintKind::Transition => Some((0x10, Some(0x0C), None, None)),
        ClothConstraintKind::BendStiffness => {
            snapshot.set_dimension_readable = read_f32(set.saturating_add(0x38))
                .filter(|value| value.is_finite())
                .map(|value| snapshot.set_dimension = value)
                .is_some();
            Some((0x20, Some(0x14), None, None))
        }
        _ => None,
    };
    let Some((stride, primary_offset, secondary_offset, tertiary_offset)) = layout else {
        return snapshot;
    };
    let Some((elements, element_count)) =
        bounded_hk_array_span(set, 0x28, 0x30, stride, MAX_CLOTH_CONSTRAINT_ELEMENTS)
    else {
        return snapshot;
    };
    snapshot.elements_readable = true;
    snapshot.element_count = element_count;
    if let Some(offset) = primary_offset {
        snapshot.primary_dimensions =
            summarize_strided_scalar(elements, element_count, stride, offset);
    }
    if let Some(offset) = secondary_offset {
        snapshot.secondary_dimensions =
            summarize_strided_scalar(elements, element_count, stride, offset);
    }
    if let Some(offset) = tertiary_offset {
        snapshot.tertiary_dimensions =
            summarize_strided_scalar(elements, element_count, stride, offset);
    }
    snapshot
}

pub fn constraint_kind_from_vtable_rva(vtable_rva: usize) -> ClothConstraintKind {
    match vtable_rva {
        ER_HCL_STANDARD_LINK_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::StandardLink,
        ER_HCL_STRETCH_LINK_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::StretchLink,
        ER_HCL_LOCAL_RANGE_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::LocalRange,
        ER_HCL_TRANSITION_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::Transition,
        ER_HCL_BEND_STIFFNESS_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::BendStiffness,
        ER_HCL_COMPRESSIBLE_LINK_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::CompressibleLink,
        ER_HCL_BONE_PLANES_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::BonePlanes,
        ER_HCL_ANTI_PINCH_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::AntiPinch,
        ER_HCL_BEND_LINK_CONSTRAINT_SET_VTABLE_RVA => ClothConstraintKind::BendLink,
        ER_HCL_STRETCH_LINK_CONSTRAINT_SET_MX_VTABLE_RVA => ClothConstraintKind::StretchLinkMx,
        ER_HCL_STANDARD_LINK_CONSTRAINT_SET_MX_VTABLE_RVA => ClothConstraintKind::StandardLinkMx,
        ER_HCL_BEND_STIFFNESS_CONSTRAINT_SET_MX_VTABLE_RVA => ClothConstraintKind::BendStiffnessMx,
        ER_HCL_COMPRESSIBLE_LINK_CONSTRAINT_SET_MX_VTABLE_RVA => {
            ClothConstraintKind::CompressibleLinkMx
        }
        ER_HCL_BEND_LINK_CONSTRAINT_SET_MX_VTABLE_RVA => ClothConstraintKind::BendLinkMx,
        ER_HCL_VOLUME_CONSTRAINT_MX_VTABLE_RVA => ClothConstraintKind::VolumeMx,
        _ => ClothConstraintKind::Unknown,
    }
}

fn record_immediate_child_transition(
    before: &(
        ClothChildProbeSummary,
        [ClothChildSnapshot; CLOTH_CHILD_SLOTS],
    ),
    after: &(
        ClothChildProbeSummary,
        [ClothChildSnapshot; CLOTH_CHILD_SLOTS],
    ),
    expected_ratio: f32,
) {
    let unit_state = current_unit_state();
    let counts = immediate_child_transition_counts(before, after, expected_ratio);
    unit_state
        .cloth_immediate_probe_transitions
        .fetch_add(1, Ordering::Relaxed);
    unit_state
        .cloth_immediate_children_observed
        .fetch_add(counts.children_observed, Ordering::Relaxed);
    unit_state
        .cloth_immediate_particle_matches
        .fetch_add(counts.particle_matches, Ordering::Relaxed);
    unit_state
        .cloth_immediate_transform_matches
        .fetch_add(counts.transform_matches, Ordering::Relaxed);
    unit_state
        .cloth_immediate_both_matches
        .fetch_add(counts.both_matches, Ordering::Relaxed);
    unit_state
        .cloth_immediate_unreadable
        .fetch_add(counts.unreadable, Ordering::Relaxed);
}

#[inline(never)]
fn boxed_cloth_child_snapshots(
    core: usize,
) -> Box<(
    ClothChildProbeSummary,
    [ClothChildSnapshot; CLOTH_CHILD_SLOTS],
)> {
    // Keep the large bounded arrays out of the high-frequency wrapper hook's
    // ordinary stack frame. This helper is reached only for a committed scale
    // transition, and the returned Box leaves only a pointer in the hook.
    Box::new(cloth_child_snapshots(core))
}

fn immediate_child_transition_counts(
    before: &(
        ClothChildProbeSummary,
        [ClothChildSnapshot; CLOTH_CHILD_SLOTS],
    ),
    after: &(
        ClothChildProbeSummary,
        [ClothChildSnapshot; CLOTH_CHILD_SLOTS],
    ),
    expected_ratio: f32,
) -> ImmediateChildTransitionCounts {
    let mut counts = ImmediateChildTransitionCounts::default();
    if !valid_scale(expected_ratio)
        || !before.0.topology_readable
        || !after.0.topology_readable
        || before.0.truncated
        || after.0.truncated
    {
        counts.unreadable = 1;
        return counts;
    }

    for before_child in before.1.iter().filter(|child| child.valid) {
        let Some(after_child) = after
            .1
            .iter()
            .find(|child| child.valid && child.child == before_child.child)
        else {
            counts.unreadable += 1;
            continue;
        };
        counts.children_observed += 1;

        let particle_ratio = position_extent_ratio(
            before_child.current_positions,
            after_child.current_positions,
        );
        let transform_ratio = transform_basis_ratio(
            before_child.transform_entries,
            after_child.transform_entries,
        );
        if particle_ratio.is_none() || transform_ratio.is_none() {
            counts.unreadable += 1;
        }

        let particle_matches =
            particle_ratio.is_some_and(|ratio| transition_ratio_matches(ratio, expected_ratio));
        let transform_matches =
            transform_ratio.is_some_and(|ratio| transition_ratio_matches(ratio, expected_ratio));
        counts.particle_matches += u64::from(particle_matches);
        counts.transform_matches += u64::from(transform_matches);
        counts.both_matches += u64::from(particle_matches && transform_matches);
    }
    counts
}

fn position_extent_ratio(before: ClothPositionBounds, after: ClothPositionBounds) -> Option<f32> {
    if !before.readable || !after.readable || before.count == 0 || before.count != after.count {
        return None;
    }
    let before_extent = vector_length3(before.span_x(), before.span_y(), before.span_z());
    let after_extent = vector_length3(after.span_x(), after.span_y(), after.span_z());
    (before_extent > 1.0e-4 && after_extent.is_finite()).then_some(after_extent / before_extent)
}

fn transform_basis_ratio(
    before: ClothTransformEntrySummary,
    after: ClothTransformEntrySummary,
) -> Option<f32> {
    if !before.readable || !after.readable || before.count == 0 || before.count != after.count {
        return None;
    }
    let before_basis = [
        before.basis_min_x,
        before.basis_min_y,
        before.basis_min_z,
        before.basis_max_x,
        before.basis_max_y,
        before.basis_max_z,
    ];
    let after_basis = [
        after.basis_min_x,
        after.basis_min_y,
        after.basis_min_z,
        after.basis_max_x,
        after.basis_max_y,
        after.basis_max_z,
    ];
    if !before_basis
        .iter()
        .chain(after_basis.iter())
        .all(|value| value.is_finite())
    {
        return None;
    }
    let before_mean = before_basis.iter().sum::<f32>() / before_basis.len() as f32;
    let after_mean = after_basis.iter().sum::<f32>() / after_basis.len() as f32;
    (before_mean > 1.0e-4).then_some(after_mean / before_mean)
}

fn transition_ratio_matches(actual: f32, expected: f32) -> bool {
    let tolerance = (expected * 0.10).max(0.05);
    actual.is_finite() && (actual - expected).abs() <= tolerance
}

fn position_bounds_center(bounds: ClothPositionBounds) -> Option<[f32; 3]> {
    if !bounds.readable || bounds.count == 0 {
        return None;
    }
    let center = [
        (bounds.min_x + bounds.max_x) * 0.5,
        (bounds.min_y + bounds.max_y) * 0.5,
        (bounds.min_z + bounds.max_z) * 0.5,
    ];
    center
        .iter()
        .all(|value| value.is_finite())
        .then_some(center)
}

fn sync_immediate_cloth_attachment_positions(
    before: &(
        ClothChildProbeSummary,
        [ClothChildSnapshot; CLOTH_CHILD_SLOTS],
    ),
    after: &(
        ClothChildProbeSummary,
        [ClothChildSnapshot; CLOTH_CHILD_SLOTS],
    ),
    requested_scale: f32,
    expected_ratio: f32,
    root_translation: [f32; 3],
) -> ClothTransformResyncCounts {
    let mut counts = ClothTransformResyncCounts::default();
    if !valid_active_scale(requested_scale)
        || !valid_scale(expected_ratio)
        || !root_translation.iter().all(|value| value.is_finite())
        || !before.0.topology_readable
        || !after.0.topology_readable
        || before.0.truncated
        || after.0.truncated
    {
        counts.topology_rejected = 1;
        return counts;
    }

    for before_child in before.1.iter().filter(|child| child.valid) {
        let Some(after_child) = after
            .1
            .iter()
            .find(|child| child.valid && child.child == before_child.child)
        else {
            counts.position_rejected += 1;
            continue;
        };
        counts.children_observed += 1;
        let Some(before_center) =
            position_bounds_center(before_child.transform_entries.translation_bounds)
        else {
            counts.position_rejected += 1;
            continue;
        };
        let Some(after_center) =
            position_bounds_center(after_child.transform_entries.translation_bounds)
        else {
            counts.position_rejected += 1;
            continue;
        };
        let Some(delta) = transition_attachment_center_delta(
            before_center,
            after_center,
            root_translation,
            expected_ratio,
        ) else {
            counts.position_rejected += 1;
            continue;
        };
        if vector_length3(delta[0], delta[1], delta[2]) <= 1.0e-4 {
            counts.entries_already_scaled += after_child.transform_entries.count as u64;
            continue;
        }
        if sync_transition_child_attachment_position(
            after_child.child,
            requested_scale,
            delta,
            &mut counts,
        ) {
            counts.children_changed += 1;
        }
    }
    counts
}

fn sync_transition_child_attachment_position(
    child: usize,
    requested_scale: f32,
    delta: [f32; 3],
    counts: &mut ClothTransformResyncCounts,
) -> bool {
    let Some(entry_count) = bounded_i32_count(
        child.saturating_add(0x170),
        MAX_CLOTH_TRANSFORM_ENTRIES_PER_CHILD,
    ) else {
        counts.entries_rejected += 1;
        counts.position_rejected += 1;
        return false;
    };
    if entry_count == 0 {
        return false;
    }
    let entries = read_usize(child.saturating_add(0x168)).unwrap_or(0);
    let Some(entries_bytes) = entry_count.checked_mul(size_of::<usize>()) else {
        counts.entries_rejected += entry_count as u64;
        counts.position_rejected += 1;
        return false;
    };
    if entries == 0 || !is_memory_accessible(entries, entries_bytes, false) {
        counts.entries_rejected += entry_count as u64;
        counts.position_rejected += 1;
        return false;
    }

    // This allocation occurs only on a committed scale transition. Keeping
    // complete outputs until every matrix and particle buffer is validated
    // prevents a half-written child if one entry is malformed.
    let mut matrix_outputs: Vec<(usize, [f32; 16])> = Vec::with_capacity(entry_count);
    for entry_index in 0..entry_count {
        counts.entries_observed += 1;
        let entry = unsafe {
            (entries.saturating_add(entry_index * size_of::<usize>()) as *const usize)
                .read_unaligned()
        };
        let matrix_address = entry.saturating_add(0x20);
        if matrix_outputs
            .iter()
            .any(|(existing, _)| *existing == matrix_address)
        {
            continue;
        }
        let Some(matrix) = read_matrix(matrix_address) else {
            counts.entries_rejected += 1;
            counts.position_rejected += 1;
            return false;
        };
        if !is_memory_accessible(matrix_address, size_of::<[f32; 16]>(), true)
            || !cloth_matrix_array_basis_matches_scale(&matrix, requested_scale)
        {
            counts.entries_rejected += 1;
            counts.position_rejected += 1;
            return false;
        }
        let Some(translated) = translated_cloth_matrix(matrix, delta) else {
            counts.entries_rejected += 1;
            counts.position_rejected += 1;
            return false;
        };
        matrix_outputs.push((matrix_address, translated));
    }

    let Some((current_positions, previous_positions, particle_count)) =
        child_position_buffers(child)
    else {
        counts.position_rejected += 1;
        return false;
    };
    if !position_buffer_is_writable(current_positions, particle_count)
        || (previous_positions != current_positions
            && !position_buffer_is_writable(previous_positions, particle_count))
    {
        counts.position_rejected += 1;
        return false;
    }

    translate_position_buffer(current_positions, particle_count, delta);
    counts.position_buffers_shifted += 1;
    counts.particles_shifted += particle_count as u64;
    if previous_positions != current_positions {
        translate_position_buffer(previous_positions, particle_count, delta);
        counts.position_buffers_shifted += 1;
        counts.particles_shifted += particle_count as u64;
    }
    counts.aabbs_shifted += translate_available_cloth_aabbs(
        [child.saturating_add(0x60), child.saturating_add(0x80)],
        delta,
    );
    for (matrix_address, translated) in matrix_outputs {
        write_matrix(matrix_address, translated);
        counts.entries_changed += 1;
    }
    true
}

fn sync_cloth_attachment_positions(
    core: usize,
    requested_scale: f32,
    root_translation: [f32; 3],
) -> ClothTransformResyncCounts {
    let mut counts = ClothTransformResyncCounts::default();
    if !valid_active_scale(requested_scale)
        || !root_translation.iter().all(|value| value.is_finite())
    {
        counts.topology_rejected = 1;
        return counts;
    }

    let Some((groups_begin, group_count)) =
        bounded_vector_span(core, 0x28, 0x30, size_of::<usize>(), MAX_CLOTH_GROUPS)
    else {
        counts.topology_rejected = 1;
        return counts;
    };

    for group_index in 0..group_count {
        let group_holder = read_usize(groups_begin + group_index * size_of::<usize>());
        let Some(group_root) = group_holder.and_then(read_usize) else {
            counts.topology_rejected += 1;
            return counts;
        };
        let Some(child_count) = bounded_i32_count(
            group_root.saturating_add(0x48),
            MAX_CLOTH_CHILDREN_PER_GROUP,
        ) else {
            counts.topology_rejected += 1;
            return counts;
        };
        let children = read_usize(group_root.saturating_add(0x40)).unwrap_or(0);
        let Some(children_bytes) = child_count.checked_mul(size_of::<usize>()) else {
            counts.topology_rejected += 1;
            return counts;
        };
        if child_count > 0
            && (children == 0 || !is_memory_accessible(children, children_bytes, false))
        {
            counts.topology_rejected += 1;
            return counts;
        }

        for child_index in 0..child_count {
            let child = unsafe {
                (children.saturating_add(child_index * size_of::<usize>()) as *const usize)
                    .read_unaligned()
            };
            if child == 0 || !is_memory_accessible(child, 0x178, false) {
                counts.topology_rejected += 1;
                return counts;
            }
            counts.children_observed += 1;
            if sync_child_attachment_position(child, requested_scale, root_translation, &mut counts)
            {
                counts.children_changed += 1;
            }
        }
    }
    counts
}

fn sync_child_attachment_position(
    child: usize,
    requested_scale: f32,
    root_translation: [f32; 3],
    counts: &mut ClothTransformResyncCounts,
) -> bool {
    let Some(entry_count) = bounded_i32_count(
        child.saturating_add(0x170),
        MAX_CLOTH_TRANSFORM_ENTRIES_PER_CHILD,
    ) else {
        counts.entries_rejected += 1;
        return false;
    };
    if entry_count == 0 {
        return false;
    }

    let entries = read_usize(child.saturating_add(0x168)).unwrap_or(0);
    let Some(entries_bytes) = entry_count.checked_mul(size_of::<usize>()) else {
        counts.entries_rejected += entry_count as u64;
        return false;
    };
    if entries == 0 || !is_memory_accessible(entries, entries_bytes, false) {
        counts.entries_rejected += entry_count as u64;
        return false;
    }

    let mut matrix_addresses = [0usize; MAX_CLOTH_TRANSFORM_ENTRIES_PER_CHILD];
    let mut matrix_count = 0usize;
    let mut unit_count = 0usize;
    let mut requested_count = 0usize;
    let mut unit_translation_sum = [0.0f64; 3];
    for entry_index in 0..entry_count {
        counts.entries_observed += 1;
        let entry = unsafe {
            (entries.saturating_add(entry_index * size_of::<usize>()) as *const usize)
                .read_unaligned()
        };
        let matrix_address = entry.saturating_add(0x20);
        if matrix_addresses[..matrix_count].contains(&matrix_address) {
            continue;
        }
        let Some(matrix) = read_matrix(matrix_address) else {
            counts.entries_rejected += 1;
            counts.position_rejected += 1;
            return false;
        };
        if !is_memory_accessible(matrix_address, size_of::<[f32; 16]>(), true) {
            counts.entries_rejected += 1;
            counts.position_rejected += 1;
            return false;
        }
        matrix_addresses[matrix_count] = matrix_address;
        matrix_count += 1;
        if cloth_matrix_array_basis_matches_scale(&matrix, requested_scale) {
            requested_count += 1;
        } else if cloth_matrix_array_basis_matches_scale(&matrix, 1.0) {
            unit_count += 1;
            for component in 0..3 {
                unit_translation_sum[component] += f64::from(matrix[12 + component]);
            }
        } else {
            counts.entries_rejected += 1;
            counts.position_rejected += 1;
            return false;
        }
    }

    if requested_count == matrix_count {
        counts.entries_already_scaled += matrix_count as u64;
        return false;
    }
    if unit_count != matrix_count {
        counts.entries_already_scaled += requested_count as u64;
        counts.entries_rejected += (matrix_count - requested_count) as u64;
        counts.position_rejected += 1;
        return false;
    }

    let attachment_center = unit_translation_sum.map(|sum| (sum / matrix_count as f64) as f32);
    let Some(delta) = attachment_center_delta(attachment_center, root_translation, requested_scale)
    else {
        counts.position_rejected += 1;
        return false;
    };
    let Some((current_positions, previous_positions, particle_count)) =
        child_position_buffers(child)
    else {
        counts.position_rejected += 1;
        return false;
    };
    if !position_buffer_is_writable(current_positions, particle_count)
        || (previous_positions != current_positions
            && !position_buffer_is_writable(previous_positions, particle_count))
    {
        counts.position_rejected += 1;
        return false;
    }

    translate_position_buffer(current_positions, particle_count, delta);
    counts.position_buffers_shifted += 1;
    counts.particles_shifted += particle_count as u64;
    if previous_positions != current_positions {
        translate_position_buffer(previous_positions, particle_count, delta);
        counts.position_buffers_shifted += 1;
        counts.particles_shifted += particle_count as u64;
    }
    counts.aabbs_shifted += translate_available_cloth_aabbs(
        [child.saturating_add(0x60), child.saturating_add(0x80)],
        delta,
    );

    for &matrix_address in &matrix_addresses[..matrix_count] {
        let matrix = unsafe { (matrix_address as *const [f32; 16]).read_unaligned() };
        let Some(scaled) =
            absolute_uniform_scale_matrix_about_pivot(matrix, requested_scale, root_translation)
        else {
            // This can only happen if another thread changed a matrix between
            // the validation and write passes. Leave that entry untouched and
            // expose the race through the rejection counters instead of
            // panicking inside the injected callback.
            counts.entries_rejected += 1;
            counts.position_rejected += 1;
            continue;
        };
        write_matrix(matrix_address, scaled);
        counts.entries_changed += 1;
    }
    true
}

fn attachment_center_delta(center: [f32; 3], root: [f32; 3], scale: f32) -> Option<[f32; 3]> {
    if !valid_active_scale(scale)
        || !center
            .iter()
            .chain(root.iter())
            .all(|value| value.is_finite())
    {
        return None;
    }
    let delta = std::array::from_fn(|index| (center[index] - root[index]) * (scale - 1.0));
    delta.iter().all(|value| value.is_finite()).then_some(delta)
}

fn transition_attachment_center_delta(
    before_center: [f32; 3],
    after_center: [f32; 3],
    root: [f32; 3],
    ratio: f32,
) -> Option<[f32; 3]> {
    if !valid_scale(ratio)
        || !before_center
            .iter()
            .chain(after_center.iter())
            .chain(root.iter())
            .all(|value| value.is_finite())
    {
        return None;
    }
    let desired: [f32; 3] =
        std::array::from_fn(|index| root[index] + ratio * (before_center[index] - root[index]));
    let delta: [f32; 3] = std::array::from_fn(|index| desired[index] - after_center[index]);
    delta.iter().all(|value| value.is_finite()).then_some(delta)
}

fn translated_cloth_matrix(mut matrix: [f32; 16], delta: [f32; 3]) -> Option<[f32; 16]> {
    if !matrix.iter().all(|value| value.is_finite()) || !delta.iter().all(|value| value.is_finite())
    {
        return None;
    }
    for component in 0..3 {
        matrix[12 + component] += delta[component];
    }
    matrix
        .iter()
        .all(|value| value.is_finite())
        .then_some(matrix)
}

fn child_position_buffers(child: usize) -> Option<(usize, usize, usize)> {
    let sim_data = read_usize(child.checked_add(0x18)?)?;
    let particle_count = usize::try_from(read_u32(sim_data.checked_add(0x48)?)?).ok()?;
    if particle_count > MAX_CLOTH_PARTICLES_PER_CHILD {
        return None;
    }
    Some((
        read_usize(child.checked_add(0x20)?)?,
        read_usize(child.checked_add(0x30)?)?,
        particle_count,
    ))
}

fn position_buffer_is_writable(address: usize, count: usize) -> bool {
    if count == 0 {
        return true;
    }
    let Some(bytes) = count.checked_mul(size_of::<[f32; 4]>()) else {
        return false;
    };
    if address == 0 || !is_memory_accessible(address, bytes, true) {
        return false;
    }
    (0..count).all(|index| {
        let position = unsafe {
            (address.saturating_add(index * size_of::<[f32; 4]>()) as *const [f32; 4])
                .read_unaligned()
        };
        position[..3].iter().all(|value| value.is_finite())
    })
}

fn translate_position_buffer(address: usize, count: usize, delta: [f32; 3]) {
    for index in 0..count {
        let position_address = address.saturating_add(index * size_of::<[f32; 4]>());
        let mut position = unsafe { (position_address as *const [f32; 4]).read_unaligned() };
        for component in 0..3 {
            position[component] += delta[component];
        }
        write_vec4(position_address, position);
    }
}

fn cloth_aabb_is_writable(address: usize) -> bool {
    if !is_memory_accessible(address, 2 * size_of::<[f32; 4]>(), true) {
        return false;
    }
    let minimum = unsafe { (address as *const [f32; 4]).read_unaligned() };
    let maximum = unsafe {
        (address.saturating_add(size_of::<[f32; 4]>()) as *const [f32; 4]).read_unaligned()
    };
    minimum[..3]
        .iter()
        .chain(maximum[..3].iter())
        .all(|value| value.is_finite())
        && (0..3).all(|component| maximum[component] >= minimum[component])
}

fn translate_cloth_aabb(address: usize, delta: [f32; 3]) {
    for endpoint in 0..2 {
        let endpoint_address = address.saturating_add(endpoint * size_of::<[f32; 4]>());
        let mut value = unsafe { (endpoint_address as *const [f32; 4]).read_unaligned() };
        for component in 0..3 {
            value[component] += delta[component];
        }
        write_vec4(endpoint_address, value);
    }
}

fn translate_available_cloth_aabbs(addresses: [usize; 2], delta: [f32; 3]) -> u64 {
    let mut shifted = 0;
    for address in addresses {
        if cloth_aabb_is_writable(address) {
            translate_cloth_aabb(address, delta);
            shifted += 1;
        }
    }
    shifted
}

pub(crate) fn write_cloth_instance_aabbs(
    child: usize,
    expected_vtable: usize,
    expected_sim_data: usize,
    targets: [(usize, Option<ClothPositionBounds>); 3],
) -> Option<usize> {
    if child == 0
        || expected_vtable == 0
        || expected_sim_data == 0
        || read_usize(child) != Some(expected_vtable)
        || read_usize(child.checked_add(0x18)?) != Some(expected_sim_data)
    {
        return None;
    }

    let mut writes = 0usize;
    for &(offset, target) in &targets {
        if !matches!(offset, 0x60 | 0x80 | 0xB0) {
            return None;
        }
        let Some(target) = target else {
            continue;
        };
        if !cloth_position_bounds_are_valid(target)
            || !cloth_aabb_is_writable(child.checked_add(offset)?)
        {
            return None;
        }
        writes += 1;
    }

    for (offset, target) in targets {
        let Some(target) = target else {
            continue;
        };
        let address = child.checked_add(offset)?;
        let mut minimum = read_vec4(address)?;
        let mut maximum = read_vec4(address.checked_add(size_of::<[f32; 4]>())?)?;
        minimum[..3].copy_from_slice(&[target.min_x, target.min_y, target.min_z]);
        maximum[..3].copy_from_slice(&[target.max_x, target.max_y, target.max_z]);
        write_vec4(address, minimum);
        write_vec4(address.checked_add(size_of::<[f32; 4]>())?, maximum);
    }
    Some(writes)
}

fn cloth_position_bounds_are_valid(bounds: ClothPositionBounds) -> bool {
    bounds.readable
        && [
            bounds.min_x,
            bounds.min_y,
            bounds.min_z,
            bounds.max_x,
            bounds.max_y,
            bounds.max_z,
        ]
        .into_iter()
        .all(f32::is_finite)
        && bounds.max_x >= bounds.min_x
        && bounds.max_y >= bounds.min_y
        && bounds.max_z >= bounds.min_z
}

fn absolute_uniform_scale_matrix_about_pivot(
    mut matrix: [f32; 16],
    scale: f32,
    pivot: [f32; 3],
) -> Option<[f32; 16]> {
    if !valid_active_scale(scale)
        || !matrix.iter().all(|value| value.is_finite())
        || !pivot.iter().all(|value| value.is_finite())
    {
        return None;
    }
    for index in [0usize, 1, 2, 4, 5, 6, 8, 9, 10] {
        matrix[index] *= scale;
    }
    for (index, pivot_component) in [12usize, 13, 14].into_iter().zip(pivot) {
        matrix[index] = pivot_component + (matrix[index] - pivot_component) * scale;
    }
    matrix
        .iter()
        .all(|value| value.is_finite())
        .then_some(matrix)
}

fn secondary_reference_matrix_for_scale(
    mut source: [f32; 16],
    requested_scale: f32,
    root: [f32; 3],
) -> Option<[f32; 16]> {
    if !valid_scale(requested_scale)
        || !source.iter().all(|value| value.is_finite())
        || !root.iter().all(|value| value.is_finite())
    {
        return None;
    }

    let basis = [
        vector_length3(source[0], source[1], source[2]),
        vector_length3(source[4], source[5], source[6]),
        vector_length3(source[8], source[9], source[10]),
    ];
    let source_scale = basis.iter().sum::<f32>() / basis.len() as f32;
    if !valid_scale(source_scale) {
        return None;
    }
    let uniform_tolerance = (source_scale * 0.05).max(0.01);
    if basis
        .iter()
        .any(|component| (*component - source_scale).abs() > uniform_tolerance)
    {
        return None;
    }

    if cloth_matrix_array_basis_matches_scale(&source, requested_scale) {
        return Some(source);
    }

    let ratio = requested_scale / source_scale;
    if !valid_scale(ratio) {
        return None;
    }
    for index in [0usize, 1, 2, 4, 5, 6, 8, 9, 10] {
        source[index] *= ratio;
    }
    for (index, root_component) in [12usize, 13, 14].into_iter().zip(root) {
        source[index] = root_component + (source[index] - root_component) * ratio;
    }

    (source.iter().all(|value| value.is_finite())
        && cloth_matrix_array_basis_matches_scale(&source, requested_scale))
    .then_some(source)
}

fn summarize_position_buffer(address: usize, count: usize) -> ClothPositionBounds {
    if count == 0 {
        return ClothPositionBounds {
            readable: true,
            ..ClothPositionBounds::default()
        };
    }
    let Some(bytes) = count.checked_mul(size_of::<[f32; 4]>()) else {
        return ClothPositionBounds::default();
    };
    if address == 0 || !is_memory_accessible(address, bytes, false) {
        return ClothPositionBounds::default();
    }

    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for index in 0..count {
        let position = unsafe {
            (address.saturating_add(index * size_of::<[f32; 4]>()) as *const [f32; 4])
                .read_unaligned()
        };
        if !position[..3].iter().all(|value| value.is_finite()) {
            return ClothPositionBounds::default();
        }
        for component in 0..3 {
            minimum[component] = minimum[component].min(position[component]);
            maximum[component] = maximum[component].max(position[component]);
        }
    }

    ClothPositionBounds {
        readable: true,
        count,
        min_x: minimum[0],
        min_y: minimum[1],
        min_z: minimum[2],
        max_x: maximum[0],
        max_y: maximum[1],
        max_z: maximum[2],
    }
}

fn summarize_cloth_aabb(address: usize) -> ClothPositionBounds {
    let Some(minimum) = read_vec4(address) else {
        return ClothPositionBounds::default();
    };
    let Some(maximum) = read_vec4(address.saturating_add(size_of::<[f32; 4]>())) else {
        return ClothPositionBounds::default();
    };
    if !minimum[..3]
        .iter()
        .chain(maximum[..3].iter())
        .all(|value| value.is_finite())
        || (0..3).any(|component| maximum[component] < minimum[component])
    {
        return ClothPositionBounds::default();
    }
    ClothPositionBounds {
        readable: true,
        count: 2,
        min_x: minimum[0],
        min_y: minimum[1],
        min_z: minimum[2],
        max_x: maximum[0],
        max_y: maximum[1],
        max_z: maximum[2],
    }
}

fn summarize_strided_scalar(
    address: usize,
    count: usize,
    stride: usize,
    field_offset: usize,
) -> ClothScalarBounds {
    if stride < field_offset.saturating_add(size_of::<f32>()) {
        return ClothScalarBounds::default();
    }
    if count == 0 {
        return ClothScalarBounds {
            readable: true,
            ..ClothScalarBounds::default()
        };
    }
    let Some(bytes) = count.checked_mul(stride) else {
        return ClothScalarBounds::default();
    };
    if address == 0 || !is_memory_accessible(address, bytes, false) {
        return ClothScalarBounds::default();
    }

    let mut minimum = f32::INFINITY;
    let mut maximum = f32::NEG_INFINITY;
    for index in 0..count {
        let value = unsafe {
            (address
                .saturating_add(index * stride)
                .saturating_add(field_offset) as *const f32)
                .read_unaligned()
        };
        if !value.is_finite() {
            return ClothScalarBounds::default();
        }
        minimum = minimum.min(value);
        maximum = maximum.max(value);
    }
    ClothScalarBounds {
        readable: true,
        count,
        minimum,
        maximum,
    }
}

fn summarize_cloth_transform_entries(child: usize) -> ClothTransformEntrySummary {
    let Some(count) = bounded_i32_count(
        child.saturating_add(0x170),
        MAX_CLOTH_TRANSFORM_ENTRIES_PER_CHILD,
    ) else {
        return ClothTransformEntrySummary::default();
    };
    if count == 0 {
        return ClothTransformEntrySummary {
            readable: true,
            translation_bounds: ClothPositionBounds {
                readable: true,
                ..ClothPositionBounds::default()
            },
            ..ClothTransformEntrySummary::default()
        };
    }

    let entries = read_usize(child.saturating_add(0x168)).unwrap_or(0);
    let Some(bytes) = count.checked_mul(size_of::<usize>()) else {
        return ClothTransformEntrySummary::default();
    };
    if entries == 0 || !is_memory_accessible(entries, bytes, false) {
        return ClothTransformEntrySummary::default();
    }

    let mut basis_minimum = [f32::INFINITY; 3];
    let mut basis_maximum = [f32::NEG_INFINITY; 3];
    let mut translation_minimum = [f32::INFINITY; 3];
    let mut translation_maximum = [f32::NEG_INFINITY; 3];
    for index in 0..count {
        let entry = unsafe {
            (entries.saturating_add(index * size_of::<usize>()) as *const usize).read_unaligned()
        };
        let Some(matrix) = read_matrix(entry.saturating_add(0x20)) else {
            return ClothTransformEntrySummary::default();
        };
        if !matrix.iter().all(|value| value.is_finite()) {
            return ClothTransformEntrySummary::default();
        }
        let basis = [
            vector_length3(matrix[0], matrix[1], matrix[2]),
            vector_length3(matrix[4], matrix[5], matrix[6]),
            vector_length3(matrix[8], matrix[9], matrix[10]),
        ];
        let translation = [matrix[12], matrix[13], matrix[14]];
        for component in 0..3 {
            basis_minimum[component] = basis_minimum[component].min(basis[component]);
            basis_maximum[component] = basis_maximum[component].max(basis[component]);
            translation_minimum[component] =
                translation_minimum[component].min(translation[component]);
            translation_maximum[component] =
                translation_maximum[component].max(translation[component]);
        }
    }

    ClothTransformEntrySummary {
        readable: true,
        count,
        basis_min_x: basis_minimum[0],
        basis_min_y: basis_minimum[1],
        basis_min_z: basis_minimum[2],
        basis_max_x: basis_maximum[0],
        basis_max_y: basis_maximum[1],
        basis_max_z: basis_maximum[2],
        translation_bounds: ClothPositionBounds {
            readable: true,
            count,
            min_x: translation_minimum[0],
            min_y: translation_minimum[1],
            min_z: translation_minimum[2],
            max_x: translation_maximum[0],
            max_y: translation_maximum[1],
            max_z: translation_maximum[2],
        },
    }
}

fn bounded_i32_count(address: usize, maximum: usize) -> Option<usize> {
    let count = read_i32(address)?;
    if count < 0 {
        return None;
    }
    let count = usize::try_from(count).ok()?;
    (count <= maximum).then_some(count)
}

fn bounded_hk_array_count(
    object: usize,
    pointer_offset: usize,
    size_offset: usize,
    maximum: usize,
) -> Option<usize> {
    if object == 0 {
        return None;
    }
    let count = bounded_i32_count(object.saturating_add(size_offset), maximum)?;
    let begin = read_usize(object.saturating_add(pointer_offset))?;
    (count == 0 || begin != 0).then_some(count)
}

fn bounded_hk_array_span(
    object: usize,
    pointer_offset: usize,
    size_offset: usize,
    stride: usize,
    maximum: usize,
) -> Option<(usize, usize)> {
    if stride == 0 {
        return None;
    }
    let count = bounded_hk_array_count(object, pointer_offset, size_offset, maximum)?;
    let begin = read_usize(object.saturating_add(pointer_offset))?;
    let bytes = count.checked_mul(stride)?;
    if count > 0 && !is_memory_accessible(begin, bytes, false) {
        return None;
    }
    Some((begin, count))
}

fn bounded_vector_span(
    object: usize,
    begin_offset: usize,
    end_offset: usize,
    stride: usize,
    maximum: usize,
) -> Option<(usize, usize)> {
    let count = bounded_vector_count(object, begin_offset, end_offset, stride, maximum)?;
    let begin = read_usize(object.saturating_add(begin_offset))?;
    let bytes = count.checked_mul(stride)?;
    if count > 0 && (begin == 0 || !is_memory_accessible(begin, bytes, false)) {
        return None;
    }
    Some((begin, count))
}

fn bounded_vector_count(
    object: usize,
    begin_offset: usize,
    end_offset: usize,
    stride: usize,
    maximum: usize,
) -> Option<usize> {
    if object == 0 || stride == 0 {
        return None;
    }
    let begin = read_usize(object.saturating_add(begin_offset))?;
    let end = read_usize(object.saturating_add(end_offset))?;
    if begin == 0 || end < begin {
        return None;
    }
    let byte_count = end - begin;
    if byte_count % stride != 0 {
        return None;
    }
    let count = byte_count / stride;
    (count <= maximum).then_some(count)
}

pub fn module_rva(address: usize) -> usize {
    let base = MODULE_BASE.load(Ordering::Acquire);
    if address >= base && address < base.saturating_add(ER_SIZE_OF_IMAGE as usize) {
        address - base
    } else {
        0
    }
}

fn summarize_cloth_matrix(address: usize) -> ClothMatrixSummary {
    let Some(matrix) = read_matrix(address) else {
        return ClothMatrixSummary::default();
    };
    if !matrix.iter().all(|value| value.is_finite()) {
        return ClothMatrixSummary::default();
    }

    ClothMatrixSummary {
        readable: true,
        basis_x: vector_length3(matrix[0], matrix[1], matrix[2]),
        basis_y: vector_length3(matrix[4], matrix[5], matrix[6]),
        basis_z: vector_length3(matrix[8], matrix[9], matrix[10]),
        translation_x: matrix[12],
        translation_y: matrix[13],
        translation_z: matrix[14],
    }
}

fn cloth_matrix_array_basis_matches_scale(matrix: &[f32; 16], requested_scale: f32) -> bool {
    if !valid_scale(requested_scale) || !matrix.iter().all(|value| value.is_finite()) {
        return false;
    }
    let tolerance = (requested_scale * 0.05).max(0.01);
    [
        vector_length3(matrix[0], matrix[1], matrix[2]),
        vector_length3(matrix[4], matrix[5], matrix[6]),
        vector_length3(matrix[8], matrix[9], matrix[10]),
    ]
    .iter()
    .all(|basis| (*basis - requested_scale).abs() <= tolerance)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClothMatrixScaleClass {
    Requested,
    Unit,
    Other,
}

fn classify_cloth_matrix_scale(
    matrix: Option<&[f32; 16]>,
    requested_scale: f32,
) -> ClothMatrixScaleClass {
    let Some(matrix) = matrix else {
        return ClothMatrixScaleClass::Other;
    };
    if cloth_matrix_array_basis_matches_scale(matrix, requested_scale) {
        ClothMatrixScaleClass::Requested
    } else if cloth_matrix_array_basis_matches_scale(matrix, 1.0) {
        ClothMatrixScaleClass::Unit
    } else {
        ClothMatrixScaleClass::Other
    }
}

fn record_cloth_matrix_scale_class(
    class: ClothMatrixScaleClass,
    requested: &AtomicU64,
    unit: &AtomicU64,
    other: &AtomicU64,
) {
    match class {
        ClothMatrixScaleClass::Requested => requested.fetch_add(1, Ordering::Relaxed),
        ClothMatrixScaleClass::Unit => unit.fetch_add(1, Ordering::Relaxed),
        ClothMatrixScaleClass::Other => other.fetch_add(1, Ordering::Relaxed),
    };
}

fn reference_matrix_for_scale_transition(
    mut current: [f32; 16],
    previous_scale: f32,
    requested_scale: f32,
) -> Option<[f32; 16]> {
    if !valid_scale(previous_scale)
        || !valid_scale(requested_scale)
        || (previous_scale - requested_scale).abs() <= f32::EPSILON
        || !current.iter().all(|value| value.is_finite())
    {
        return None;
    }

    let tolerance = (requested_scale * 0.05).max(0.01);
    let current_basis = [
        vector_length3(current[0], current[1], current[2]),
        vector_length3(current[4], current[5], current[6]),
        vector_length3(current[8], current[9], current[10]),
    ];
    if current_basis
        .iter()
        .any(|basis| (*basis - requested_scale).abs() > tolerance)
    {
        return None;
    }

    let ratio = previous_scale / requested_scale;
    if !ratio.is_finite() || ratio <= 0.0 {
        return None;
    }
    for index in [0usize, 1, 2, 4, 5, 6, 8, 9, 10] {
        current[index] *= ratio;
        if !current[index].is_finite() {
            return None;
        }
    }

    // The native relative transform multiplies every child by
    // requested/previous. Its translation is current-reference, so keeping the
    // reference at the same world position scales children around world origin.
    // Moving the reference translation by the forward ratio produces
    // p' = ratio * p + (1 - ratio) * root, i.e. scale around the player root.
    let forward_ratio = requested_scale / previous_scale;
    for index in [12usize, 13, 14] {
        current[index] *= forward_ratio;
        if !current[index].is_finite() {
            return None;
        }
    }

    let reference_tolerance = (previous_scale * 0.05).max(0.01);
    let reference_basis = [
        vector_length3(current[0], current[1], current[2]),
        vector_length3(current[4], current[5], current[6]),
        vector_length3(current[8], current[9], current[10]),
    ];
    reference_basis
        .iter()
        .all(|basis| (*basis - previous_scale).abs() <= reference_tolerance)
        .then_some(current)
}

fn vector_length3(x: f32, y: f32, z: f32) -> f32 {
    x.mul_add(x, y.mul_add(y, z * z)).sqrt()
}

fn pose_importer_update_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let registers = unsafe { &*registers };
    let this = registers.rcx as usize;
    let primary_before = unit_state.target_pose_importer.load(Ordering::Acquire);
    let cloth_before = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire);
    // A shared pointer is handled by the primary slot exactly once. A distinct
    // cloth pointer gets independent ratio state so transitions can be restored
    // without double-scaling either cache.
    let cloth_only_target = this == cloth_before && cloth_before != primary_before;
    let is_target = this == primary_before || cloth_only_target;
    let scale = current_scale();
    let inner = this.saturating_add(POSE_INNER_OFFSET);
    let materialized_before = if is_target && valid_scale(scale) {
        read_u8(inner + POSE_MATERIALIZED_OFFSET)
    } else {
        None
    };

    let original: unsafe extern "C" fn(usize) -> usize = unsafe { transmute(original) };
    let result = unsafe { original(this) };

    let current_target = if cloth_only_target {
        unit_state
            .target_cloth_pose_importer
            .load(Ordering::Acquire)
    } else {
        unit_state.target_pose_importer.load(Ordering::Acquire)
    };
    if !is_target || this != current_target || !valid_scale(scale) || !unit_state.identity_current()
    {
        return result;
    }
    unit_state.pose_target_calls.fetch_add(1, Ordering::Relaxed);
    if cloth_only_target {
        unit_state
            .cloth_pose_target_calls
            .fetch_add(1, Ordering::Relaxed);
    }

    let materialized_after = read_u8(inner + POSE_MATERIALIZED_OFFSET);
    record_pose_gate(materialized_before, materialized_after);

    if unit_state.pose_write_lock.swap(true, Ordering::AcqRel) {
        return result;
    }
    let current_target = if cloth_only_target {
        unit_state
            .target_cloth_pose_importer
            .load(Ordering::Acquire)
    } else {
        unit_state.target_pose_importer.load(Ordering::Acquire)
    };
    if this != current_target {
        unit_state.pose_write_lock.store(false, Ordering::Release);
        return result;
    }

    let applied_scale_bits = if cloth_only_target {
        &unit_state.cloth_pose_applied_scale_bits
    } else {
        &unit_state.pose_applied_scale_bits
    };
    let previous_applied_scale = f32::from_bits(applied_scale_bits.load(Ordering::Acquire));
    match plan_pose_scale(
        previous_applied_scale,
        scale,
        materialized_before,
        materialized_after,
    ) {
        PoseScaleAction::Noop => {
            applied_scale_bits.store(scale.to_bits(), Ordering::Release);
        }
        PoseScaleAction::Scale(ratio) => match scale_pose_output(inner, ratio) {
            Some(count) => {
                applied_scale_bits.store(scale.to_bits(), Ordering::Release);
                unit_state
                    .pose_transforms_written
                    .fetch_add(count as u64, Ordering::Relaxed);
                if cloth_only_target {
                    unit_state
                        .cloth_pose_transforms_written
                        .fetch_add(count as u64, Ordering::Relaxed);
                }
            }
            None => {
                unit_state.rejected_outputs.fetch_add(1, Ordering::Relaxed);
            }
        },
        PoseScaleAction::Reject => {
            unit_state.rejected_outputs.fetch_add(1, Ordering::Relaxed);
        }
    }
    unit_state.pose_write_lock.store(false, Ordering::Release);
    result
}

fn record_pose_gate(before: Option<u8>, after: Option<u8>) {
    let unit_state = current_unit_state();
    let counter = match (before, after) {
        (Some(0), Some(0)) => &unit_state.pose_gate_00,
        (Some(0), Some(1)) => &unit_state.pose_gate_01,
        (Some(1), Some(0)) => &unit_state.pose_gate_10,
        (Some(1), Some(1)) => &unit_state.pose_gate_11,
        _ => &unit_state.pose_gate_other,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderScope {
    route: ClothOwnerRoute,
    player: usize,
    assembly: usize,
    anchor: usize,
    generation: u64,
    slot: usize,
    metadata: usize,
    source_count: usize,
    pose_arrays: [usize; 6],
    map_object: usize,
    map_entries: usize,
    map_count: usize,
}

fn render_scope(this: usize, scale: f32) -> Option<RenderScope> {
    let unit_state = current_unit_state();
    if !HOOKS_READY.load(Ordering::Acquire)
        || current_scale().to_bits() != scale.to_bits()
        || !valid_active_scale(scale)
    {
        return None;
    }
    let base = MODULE_BASE.load(Ordering::Acquire);
    if !object_has_vtable(this, base + ER_ANIM_SKELETON_VTABLE_RVA) {
        return None;
    }
    // B50CF0(node) resolves *node.holder; never call game code during selection.
    let holder = read_usize(this.checked_add(0x70)?)?;
    let input = read_usize(holder)?;
    let anchor = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire);
    if input == 0
        || anchor == 0
        || input == anchor
        || input == unit_state.target_pose_importer.load(Ordering::Acquire)
    {
        return None;
    }
    // Only search our bounded cached identities; no scene/equipment discovery.
    let slot = (0..CLOTH_INSTANCE_SLOTS).find(|&slot| {
        unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire) == input
            && unit_state.cloth_instance_was_equipment[slot].load(Ordering::Acquire)
    })?;
    let owner = unit_state.cloth_instance_owners[slot].load(Ordering::Acquire);
    if unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire) != scale.to_bits()
        || unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire)
            != NO_PENDING_CLOTH_SCALE_BITS
    {
        return None;
    }
    let scope = unit_state.target_cloth_scope.try_read().ok()?;
    let route = scope.find(owner, input)?;
    if scope.anchor != anchor
        || !scope.route_is_current(route, base, read_usize)
        || scope.routes.iter().filter(|r| r.input == input).count() != 1
        || unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) != route.core
    {
        return None;
    }
    let context = input.checked_add(0x48)?;
    if !is_memory_accessible(context, 0x38, false) {
        return None;
    }
    let header = unsafe { (context as *const [usize; 7]).read_unaligned() };
    let metadata = header[0];
    let source_count = usize::try_from(read_i32(metadata.checked_add(0x38)?)?).ok()?;
    if source_count == 0
        || source_count > cloth_render_scale::MAX_TRANSFORMS
        || [header[2], header[4], header[6]]
            .iter()
            .any(|&n| (n as u32 as i32) < source_count as i32)
        || [header[1], header[3], header[5]].contains(&0)
    {
        return None;
    }
    let mapping = read_bone_index_mapping(this)?;
    if !mapping.entries.is_multiple_of(align_of::<i16>()) {
        return None;
    }
    Some(RenderScope {
        route,
        player: scope.player,
        assembly: scope.assembly,
        anchor,
        generation: unit_state.cloth_topology_generation.load(Ordering::Acquire),
        slot,
        metadata,
        source_count,
        pose_arrays: header[1..].try_into().ok()?,
        map_object: read_usize(this + 0x88)?,
        map_entries: mapping.entries,
        map_count: mapping.count as usize,
    })
}

fn copy_render_bytes(address: usize, output: &mut [u8]) -> bool {
    if output.is_empty() {
        return true;
    }
    if !is_memory_accessible(address, output.len(), false) {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(address as *const u8, output.as_mut_ptr(), output.len())
    };
    true
}

fn spans_overlap(a: usize, a_len: usize, b: usize, b_len: usize) -> bool {
    match (a.checked_add(a_len), b.checked_add(b_len)) {
        (Some(a_end), Some(b_end)) => a < b_end && b < a_end,
        _ => true,
    }
}

fn reconcile_render_output(
    this: usize,
    output: usize,
    count: usize,
    start: usize,
    scale: f32,
    before: RenderScope,
) -> Option<usize> {
    if count > cloth_render_scale::MAX_TRANSFORMS
        || start.checked_add(count)? > before.map_count
        || !output.is_multiple_of(align_of::<[f32; 12]>())
        || render_scope(this, scale)? != before
    {
        return None;
    }
    if count == 0 {
        return Some(0);
    }
    let bytes = count.checked_mul(AFFINE_MATRIX_STRIDE)?;
    // Even a malformed caller must not turn this copied-output fix into a
    // canonical local/model/flags mutation.
    for (pointer, stride) in [
        (before.pose_arrays[0], 48),
        (before.pose_arrays[2], 48),
        (before.pose_arrays[4], 4),
    ] {
        if spans_overlap(
            output,
            bytes,
            pointer,
            before.source_count.checked_mul(stride)?,
        ) {
            return None;
        }
    }
    if !is_memory_accessible(output, bytes, true) {
        return None;
    }
    let mask = cloth_render_scale::capture_mask(
        before.route.core,
        before.source_count,
        &copy_render_bytes,
    )?;
    let mut mapping = [0i16; cloth_render_scale::MAX_TRANSFORMS];
    // Mapping storage was validated as one span by render_scope. Copy it once,
    // not one VirtualQuery per row. Header identity is rechecked before writes.
    unsafe {
        std::ptr::copy_nonoverlapping(
            before.map_entries as *const i16,
            mapping.as_mut_ptr(),
            before.map_count,
        )
    };
    if render_scope(this, scale)? != before {
        return None;
    }
    let values = unsafe { std::slice::from_raw_parts_mut(output as *mut [f32; 12], count) };
    cloth_render_scale::reconcile_range(values, &mapping[..before.map_count], start, &mask, scale)
}

fn cloth_render_range_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let registers = unsafe { &*registers };
    let this = registers.rcx as usize;
    let output = registers.rdx as usize;
    let requested = registers.r8 as u32;
    let start = registers.r9 as u32;
    let original: unsafe extern "C" fn(usize, usize, u32, u32) -> usize =
        unsafe { transmute(original) };
    let scale = current_scale();
    // All other callers and neutral scale take the cheap original path.
    if !valid_active_scale(scale)
        || caller_from_stack(registers.rsp as usize)
            != MODULE_BASE.load(Ordering::Acquire) + ER_CLOTH_RENDER_RETURN_RVA
    {
        return unsafe { original(this, output, requested, start) };
    }
    let timer = std::time::Instant::now();
    let queries_before = crate::memory_query::query_count();
    let before = crate::memory_query::scoped(|| render_scope(this, scale));
    let pre_queries = crate::memory_query::query_count() - queries_before;
    let pre_time = timer.elapsed();
    let result = unsafe { original(this, output, requested, start) };
    let Some(before) = before else {
        return result;
    };
    let timer = std::time::Instant::now();
    let post_queries_before = crate::memory_query::query_count();
    unit_state.render_calls.fetch_add(1, Ordering::Relaxed);
    let changed = if result <= requested as usize {
        crate::memory_query::scoped(|| {
            reconcile_render_output(this, output, result, start as usize, scale, before)
        })
    } else {
        None
    };
    match changed {
        Some(rows) => {
            unit_state
                .render_rows
                .fetch_add(rows as u64, Ordering::Relaxed);
        }
        None => {
            unit_state.render_rejected.fetch_add(1, Ordering::Relaxed);
        }
    }
    unit_state.render_max_us.fetch_max(
        (pre_time + timer.elapsed()).as_micros() as u64,
        Ordering::Relaxed,
    );
    unit_state.render_max_queries.fetch_max(
        pre_queries + crate::memory_query::query_count() - post_queries_before,
        Ordering::Relaxed,
    );
    result
}

pub fn cloth_mesh_frame_counters() -> [u64; 9] {
    let unit_state = current_unit_state();
    [
        unit_state.cloth_mesh_frame_seen.load(Ordering::Relaxed),
        unit_state.cloth_mesh_frame_calls.load(Ordering::Relaxed),
        unit_state.cloth_mesh_frames_written.load(Ordering::Relaxed),
        unit_state.cloth_mesh_frame_max_us.load(Ordering::Relaxed),
        unit_state
            .cloth_mesh_frame_max_queries
            .load(Ordering::Relaxed),
        unit_state.cloth_mesh_normal_rows.load(Ordering::Relaxed),
        unit_state.cloth_mesh_pn_max_us.load(Ordering::Relaxed),
        unit_state.cloth_mesh_area_calls.load(Ordering::Relaxed),
        unit_state.cloth_mesh_area_frames.load(Ordering::Relaxed),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MeshNormalSpan {
    data: usize,
    count: usize,
    stride: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SkinNormalCall {
    root: usize,
    output: usize,
    transform_set: usize,
    span: MeshNormalSpan,
    slot: usize,
    owner: usize,
    core: usize,
    anchor: usize,
    generation: u64,
    scale_bits: u32,
}

fn validate_skin_normal_runtime(base: usize) -> bool {
    bytes_equal(base + ER_CLOTH_SKIN_PN_RVA, SKIN_PN_ENTRY)
        && read_usize(base + ER_CLOTH_SKIN_PN_VTABLE_RVA + 0x20)
            == Some(base + ER_CLOTH_SKIN_PN_RVA)
        && bytes_equal(base + 0x15A99F3, &[0xE8, 0x58, 0xA3, 0xFF, 0xFF])
        && bytes_equal(base + 0x15A3E61, &[0xE8, 0x3A, 0x15, 0x00, 0x00])
        && bytes_equal(base + 0x15D8226, &[0x41, 0x0F, 0xB7, 0x00])
        && SKIN_SOURCE_SEAMS
            .iter()
            .all(|(rva, code)| bytes_equal(base + rva, code))
}

pub fn cloth_skin_normal_counters() -> [u64; 3] {
    let unit_state = current_unit_state();
    [
        unit_state.cloth_skin_normal_calls.load(Ordering::Relaxed),
        unit_state.cloth_skin_normal_rows.load(Ordering::Relaxed),
        unit_state.cloth_skin_normal_max_us.load(Ordering::Relaxed),
    ]
}

/// Stack-only observations from reads the existing guard already performs.
/// Diagnostic output never participates in a write authorization decision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SkinNormalTrace {
    pub gate: &'static str,
    pub index: usize,
    pub observed: [usize; 4],
    pub expected: usize,
    pub matrix_bits: Option<[u32; 16]>,
}
impl SkinNormalTrace {
    fn step(&mut self, gate: &'static str, index: usize, observed: [usize; 4], expected: usize) {
        *self = Self {
            gate,
            index,
            observed,
            expected,
            matrix_bits: None,
        };
    }
    fn matrix(&mut self, matrix: &[f32; 16]) {
        self.matrix_bits = Some(matrix.map(f32::to_bits));
    }
}

#[cfg(test)]
include!("test_support/skin_normal_call_241.rs");
#[cfg(test)]
include!("test_support/packed_normal_span_241.rs");

#[cfg(test)]
fn skin_normal_call(op: usize, context: usize) -> Option<SkinNormalCall> {
    let result = skin_normal_call_traced(op, context, &mut SkinNormalTrace::default());
    // Same owned fixture, same stable reads. Any decision drift is a regression.
    assert_eq!(result, skin_normal_call_241(op, context));
    result
}

fn skin_normal_call_traced(
    op: usize,
    context: usize,
    trace: &mut SkinNormalTrace,
) -> Option<SkinNormalCall> {
    let unit_state = current_unit_state();
    let base = MODULE_BASE.load(Ordering::Acquire);
    let scale = current_scale();
    trace.step("hooks_ready", 0, [0; 4], 1);
    if !HOOKS_READY.load(Ordering::Acquire) {
        return None;
    }
    trace.step("active_scale", 0, [scale.to_bits() as usize, 0, 0, 0], 0);
    if !valid_active_scale(scale) {
        return None;
    }
    trace.step("operator_vtable", 0, [op, 0, 0, 0], 0);
    let vtable = read_usize(op)?;
    trace.observed[1] = vtable;
    let expected_vtable = base.checked_add(ER_CLOTH_SKIN_PN_VTABLE_RVA)?;
    trace.expected = expected_vtable;
    if vtable != expected_vtable {
        return None;
    }
    trace.step("context_root", 0, [context, 0, 0, 0], 0);
    let root = read_usize(context.checked_add(0x10)?)?;
    let generation = cloth_topology_generation();
    trace.step("scope_lock", 0, [root, 0, 0, 0], 0);
    let scope = *unit_state.target_cloth_scope.try_read().ok()?;
    trace.step("scope_anchor", 0, [scope.anchor, root, 0, 0], 0);
    if scope.anchor == 0 {
        return None;
    }
    let anchor = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire);
    trace.expected = anchor;
    if scope.anchor != anchor {
        return None;
    }
    let mut selected = None;
    // Four counters: no instance slot, uncommitted scale, pending scale, stale
    // owner route. They describe skipped routes; they never admit a route.
    let mut route_skips = [0usize; 4];
    for route in scope.routes.iter().copied().filter(|r| r.owner != 0) {
        let Some(slot) = (0..CLOTH_INSTANCE_SLOTS).find(|&s| {
            unit_state.cloth_instance_owners[s].load(Ordering::Acquire) == route.owner
                && unit_state.cloth_instance_inputs[s].load(Ordering::Acquire) == route.input
                && unit_state.cloth_instance_cores[s].load(Ordering::Acquire) == route.core
        }) else {
            route_skips[0] += 1;
            continue;
        };
        if unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire)
            != scale.to_bits()
        {
            route_skips[1] += 1;
            continue;
        }
        if unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire)
            != NO_PENDING_CLOTH_SCALE_BITS
        {
            route_skips[2] += 1;
            continue;
        }
        if !scope.route_is_current(route, base, read_usize) {
            route_skips[3] += 1;
            continue;
        }
        trace.step(
            "root_group_span",
            slot,
            [route.owner, route.core, root, 0],
            0,
        );
        let (groups, count) = bounded_vector_span(route.core, 0x28, 0x30, 8, MAX_CLOTH_GROUPS)?;
        for i in 0..count {
            trace.step(
                "root_group_pointer",
                i,
                [groups, count, root, route.owner],
                root,
            );
            if read_usize(read_usize(groups.checked_add(i * 8)?)?)? == root {
                if selected.is_some() {
                    trace.step(
                        "duplicate_root",
                        i,
                        [root, route.owner, route.core, slot],
                        1,
                    );
                    return None;
                }
                selected = Some((slot, route));
            }
        }
    }
    trace.step("route_selection", 0, route_skips, root);
    let (slot, route) = selected?;
    trace.step("output_buffers", 0, [root, 0, 0, 0], 128);
    let buffers = read_usize(root.checked_add(0x20)?)?;
    let count = bounded_i32_count(root.checked_add(0x28)?, 128)?;
    trace.step("output_selector", 0, [buffers, count, 0, 0], 127);
    let first = bounded_i32_count(op.checked_add(0x68)?, 127)?;
    trace.observed[2] = first;
    if first >= count {
        return None;
    }
    let selector = read_usize(buffers.checked_add(first * 8)?)?;
    trace.step("selected_buffer", first, [selector, count, 0, 0], 127);
    let index = bounded_i32_count(selector.checked_add(0x110)?, 127)?;
    trace.observed[2] = index;
    if index >= count {
        return None;
    }
    let output = read_usize(buffers.checked_add(index * 8)?)?;
    let span = packed_normal_span_traced(op, output, 0x70, trace)?;
    trace.step(
        "output_vertices",
        index,
        [output, 0, 0, 0],
        crate::cloth_mesh_scale::MAX_FRAMES,
    );
    let vertices = bounded_i32_count(
        output.checked_add(0x20)?,
        crate::cloth_mesh_scale::MAX_FRAMES,
    )?;
    trace.step("output_vertex_end", index, [vertices, 0, 0, 0], vertices);
    let end = usize::from(read_u16(op.checked_add(0x102)?)?);
    trace.observed[1] = end;
    if end >= vertices {
        return None;
    }
    for offset in [0x28usize, 0x50] {
        trace.step("output_aligned_flag", offset, [output, 0, 0, 0], 1);
        let flag = read_u8(output.checked_add(offset)?)?;
        trace.observed[1] = flag as usize;
        if flag & 1 != 1 {
            return None;
        }
    }
    for offset in [0x90usize, 0xD0] {
        trace.step(
            "output_basis",
            offset,
            [output, 0, 0, 0],
            1f32.to_bits() as usize,
        );
        let matrix = read_matrix(output.checked_add(offset)?)?;
        trace.matrix(&matrix);
        if !cloth_matrix_array_basis_matches_scale(&matrix, 1.) {
            return None;
        }
    }
    // Only the packed PN branch demonstrated by 15A9966 -> 15A3CE0.
    trace.step("packed_local_pn", 0, [op, 0, 0, 0], 512);
    let packed = read_usize(op.checked_add(0x108)?)?;
    let packed_count = bounded_i32_count(op.checked_add(0x110)?, 512)?;
    trace.observed = [packed, packed_count, 0, 0];
    if packed_count == 0 {
        return None;
    }
    let controls = bounded_i32_count(op.checked_add(0xF8)?, 512)?;
    trace.expected = controls;
    if packed_count != controls {
        return None;
    }
    let packed_bytes = packed_count.checked_mul(256)?;
    trace.step(
        "packed_local_pn_readable",
        0,
        [packed, packed_count, 0, 0],
        packed_bytes,
    );
    if !is_memory_accessible(packed, packed_bytes, false) {
        return None;
    }
    trace.step("transform_set_selector", 0, [root, op, 0, 0], 128);
    let sets = read_usize(root.checked_add(0x30)?)?;
    let set_count = bounded_i32_count(root.checked_add(0x38)?, 128)?;
    let set_index = bounded_i32_count(op.checked_add(0x6C)?, 127)?;
    trace.observed = [sets, set_count, set_index, 0];
    if set_index >= set_count {
        return None;
    }
    let transform_set = read_usize(sets.checked_add(set_index * 8)?)?;
    trace.step("source_palette", set_index, [transform_set, 0, 0, 0], 2048);
    let matrices = read_usize(transform_set.checked_add(0x18)?)?;
    let matrix_count = bounded_i32_count(transform_set.checked_add(0x20)?, 2048)?;
    trace.step("source_subset", 0, [matrices, matrix_count, 0, 0], 256);
    let subset = read_usize(op.checked_add(0x58)?)?;
    let subset_count = bounded_i32_count(op.checked_add(0x60)?, 256)?;
    let binds = read_usize(op.checked_add(0x48)?)?;
    trace.observed = [subset, subset_count, binds, matrix_count];
    if subset_count == 0 || bounded_i32_count(op.checked_add(0x50)?, 256)? != subset_count {
        return None;
    }
    for i in 0..subset_count {
        trace.step(
            "source_bone_index",
            i,
            [subset, matrix_count, subset_count, 0],
            matrix_count,
        );
        let bone = usize::from(read_u16(subset.checked_add(i * 2)?)?);
        trace.observed[3] = bone;
        if bone >= matrix_count {
            return None;
        }
        trace.step(
            "source_bone_basis",
            i,
            [bone, matrices, matrix_count, subset_count],
            scale.to_bits() as usize,
        );
        let matrix = read_matrix(matrices.checked_add(bone * 64)?)?;
        trace.matrix(&matrix);
        if !cloth_matrix_array_basis_matches_scale(&matrix, scale) {
            if !skin_basis_is_single_axis_contraction(&matrix, scale) {
                return None;
            }
            trace.step(
                "source_scale_provenance",
                i,
                [route.core, root, transform_set, bone],
                scale.to_bits() as usize,
            );
            if !skin_source_has_owned_contraction(
                route.core,
                root,
                transform_set,
                bone,
                &matrix,
                scale,
            ) {
                return None;
            }
        }
        trace.step(
            "bind_basis",
            i,
            [bone, binds, subset_count, 0],
            1f32.to_bits() as usize,
        );
        let matrix = read_matrix(binds.checked_add(i * 64)?)?;
        trace.matrix(&matrix);
        if !cloth_matrix_array_basis_matches_scale(&matrix, 1.) {
            return None;
        }
    }
    trace.step(
        "final_scale_generation",
        0,
        [scale.to_bits() as usize, generation as usize, 0, 0],
        0,
    );
    let final_scale = current_scale().to_bits();
    trace.observed[2] = final_scale as usize;
    if final_scale != scale.to_bits() {
        return None;
    }
    let final_generation = cloth_topology_generation();
    trace.observed[3] = final_generation as usize;
    if generation != final_generation {
        return None;
    }
    trace.step(
        "accepted",
        slot,
        [root, output, transform_set, span.count],
        scale.to_bits() as usize,
    );
    Some(SkinNormalCall {
        root,
        output,
        transform_set,
        span,
        slot,
        owner: route.owner,
        core: route.core,
        anchor: scope.anchor,
        generation,
        scale_bits: scale.to_bits(),
    })
}

/// A native Qs transform can retain an authored/animated single-axis contraction.
/// This is NOT a general larger tolerance for deciding the external body factor.
/// All fallback writes additionally require the exact owned native input route.
fn positive_orthogonal_basis(matrix: &[f32; 16], scale: f32) -> Option<[f32; 3]> {
    if !valid_scale(scale)
        || !matrix.iter().all(|x| x.is_finite())
        || [matrix[3], matrix[7], matrix[11], matrix[15] - 1.]
            .iter()
            .any(|x| x.abs() > 1e-4)
    {
        return None;
    }
    let rows: [[f32; 3]; 3] =
        std::array::from_fn(|i| std::array::from_fn(|j| matrix[i * 4 + j] / scale));
    let lengths: [f32; 3] = rows.map(|r| (r.iter().map(|x| x * x).sum::<f32>()).sqrt());
    if lengths.iter().any(|x| !x.is_finite() || *x < 1e-4) {
        return None;
    }
    for i in 0..3 {
        for j in i + 1..3 {
            let dot = (0..3).map(|k| rows[i][k] * rows[j][k]).sum::<f32>();
            if dot.abs() > 0.002 * lengths[i] * lengths[j] {
                return None;
            }
        }
    }
    let [a, b, c] = rows;
    let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
        + a[2] * (b[0] * c[1] - b[1] * c[0]);
    (det.is_finite() && det > 0.).then_some(lengths)
}

fn skin_source_has_owned_contraction(
    core: usize,
    root: usize,
    transform_set: usize,
    bone: usize,
    matrix: &[f32; 16],
    scale: f32,
) -> bool {
    fn check(
        core: usize,
        root: usize,
        transform_set: usize,
        bone: usize,
        matrix: &[f32; 16],
        scale: f32,
    ) -> Option<()> {
        if !skin_basis_is_single_axis_contraction(matrix, scale) {
            return None;
        }
        // 26B1550 decomposes core+90, multiplies its scale with each mapped
        // source Qs scale, then stores a Matrix4 into this exact transform set.
        // Never derive the external factor from the contracted axis itself.
        let global = read_matrix(core.checked_add(CLOTH_CORE_MAIN_TRANSFORM_OFFSET)?)?;
        if positive_orthogonal_basis(&global, scale)?
            .iter()
            .any(|x| (*x - 1.).abs() > 0.002)
        {
            return None;
        }
        let entries = read_usize(core.checked_add(0x40)?)?;
        let count = bounded_i32_count(core.checked_add(0x48)?, MAX_CLOTH_SOLVER_INPUT_ENTRIES)?;
        if count == 0
            || !is_memory_accessible(
                entries,
                count.checked_mul(CLOTH_SOLVER_ENTRY_STRIDE)?,
                false,
            )
        {
            return None;
        }
        let mut matched = None;
        for i in 0..count {
            let entry = entries.checked_add(i * CLOTH_SOLVER_ENTRY_STRIDE)?;
            if read_usize(entry)? == transform_set {
                if matched.is_some() {
                    return None;
                }
                matched = Some(entry);
            }
        }
        let entry = matched?;
        match read_u8(entry.checked_add(0x30)?)? {
            0 => {
                let (groups, n) = bounded_vector_span(core, 0x28, 0x30, 8, MAX_CLOTH_GROUPS)?;
                let index = bounded_i32_count(entry.checked_add(0x10)?, MAX_CLOTH_GROUPS)?;
                if index >= n || read_usize(read_usize(groups.checked_add(index * 8)?)?)? != root {
                    return None;
                }
            }
            1 => {}
            _ => return None,
        }
        let map = read_usize(entry.checked_add(8)?)?;
        let map_count =
            bounded_i32_count(map.checked_add(0x10)?, MAX_CLOTH_SOLVER_SOURCE_TRANSFORMS)?;
        if bone >= map_count {
            return None;
        }
        let indices = read_usize(map.checked_add(0x18)?)?;
        let mapped = read_u16(indices.checked_add(bone.checked_mul(2)?)?)? as i16;
        if mapped < 0 || mapped as usize >= MAX_CLOTH_SOLVER_SOURCE_TRANSFORMS {
            return None;
        }
        Some(())
    }
    check(core, root, transform_set, bone, matrix, scale).is_some()
}

fn skin_basis_is_single_axis_contraction(matrix: &[f32; 16], scale: f32) -> bool {
    positive_orthogonal_basis(matrix, scale).is_some_and(|lengths| {
        lengths.iter().filter(|x| (**x - 1.).abs() <= 0.002).count() == 2
            // Bound the newly supported shape to a well-conditioned <=2:1
            // contraction. Severe deformation remains outside this candidate.
            && lengths.iter().filter(|x| **x >= 0.5 && **x < 0.95).count() == 1
    })
}

fn cloth_skin_normal_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let timer = std::time::Instant::now();
    let r = unsafe { &*registers };
    let (op, context) = (r.rcx as usize, r.rdx as usize);
    let mut pre_trace = SkinNormalTrace::default();
    let marker =
        crate::memory_query::scoped(|| skin_normal_call_traced(op, context, &mut pre_trace));
    let pre_time = timer.elapsed();
    let native: unsafe extern "C" fn(usize, usize) -> usize = unsafe { transmute(original) };
    let result = unsafe { native(op, context) };
    let post_timer = std::time::Instant::now();
    let mut diagnostic_rows = None;
    let mut post_trace = None;
    if let Some(marker) = marker {
        let trace = post_trace.insert(SkinNormalTrace::default());
        diagnostic_rows = crate::memory_query::scoped(|| {
            let after = skin_normal_call_traced(op, context, trace);
            if after != Some(marker) {
                if after.is_some() {
                    trace.step(
                        "post_marker_changed",
                        0,
                        [marker.root, marker.output, marker.generation as usize, 0],
                        0,
                    );
                }
                return None;
            }
            let span = marker.span;
            let raw = unsafe {
                std::slice::from_raw_parts_mut(span.data as *mut u8, span.count * span.stride)
            };
            trace.step(
                "normal_write_guard",
                0,
                [span.data, span.count, span.stride, 0],
                marker.scale_bits as usize,
            );
            let written = crate::cloth_mesh_scale::restore_normal_length(
                raw,
                span.count,
                span.stride,
                f32::from_bits(marker.scale_bits),
            )?;
            trace.step(
                "corrected",
                marker.slot,
                [span.data, span.count, span.stride, written],
                marker.scale_bits as usize,
            );
            unit_state
                .cloth_skin_normal_calls
                .fetch_add(1, Ordering::Relaxed);
            unit_state
                .cloth_skin_normal_rows
                .fetch_add(written as u64, Ordering::Relaxed);
            Some(written)
        });
        unit_state.cloth_skin_normal_max_us.fetch_max(
            (pre_time + post_timer.elapsed()).as_micros() as u64,
            Ordering::Relaxed,
        );
    }
    if crate::ENABLE_SYNC_DIAGNOSTIC {
        crate::cloth_diagnostic::observe_skin(
            op,
            context,
            marker.is_some(),
            diagnostic_rows,
            pre_trace,
            post_trace,
        );
    }
    result
}

#[cfg(test)]
#[test]
fn skin_normal_hook_fresh_output_scope_and_lifecycle_regression() {
    let unit_state = current_unit_state();
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    clear_target();
    let image = 0x140000000usize;
    let mut memory = vec![0u128; 0x8000 / 16];
    let heap = memory.as_mut_ptr() as usize;
    let put =
        |at: usize, value: usize| unsafe { ((heap + at) as *mut usize).write_unaligned(value) };
    let byte = |at: usize, value: u8| unsafe { ((heap + at) as *mut u8).write(value) };
    for (at, off) in [
        (0x748, 0x800),
        (0x150, 0xA00),
        (0x808, 0xA00),
        (0x828, 0xA00),
        (0xB30, 0xC00),
        (0xD20, 0x1200),
        (0xC40, 0xE00),
        (0xE30, 0x1000),
        (0x1028, 0x2400),
        (0x1030, 0x2408),
        (0x2400, 0x2420),
        (0x2420, 0x2500),
        (0x2310, 0x2500),
        (0x2520, 0x2600),
        (0x2600, 0x2700),
        (0x2530, 0x2B00),
        (0x2B00, 0x2900),
        (0x2918, 0x2A00),
        (0x2718, 0x3000),
        (0x2740, 0x3100),
        (0x3248, 0x3400),
        (0x3258, 0x3500),
        (0x32E0, 0x3600),
        (0x32F0, 0x3700),
        (0x3308, 0x3800),
    ] {
        put(at, heap + off);
    }
    for (at, rva) in [
        (0x800, crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA),
        (0xA00, crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA),
        (0xC00, ER_CLOTH_MODEL_VTABLE_RVA),
        (0xE00, ER_CLOTH_INNER_VTABLE_RVA),
        (0x1200, ER_POSE_IMPORTER_VTABLE_RVA),
        (0x3200, ER_CLOTH_SKIN_PN_VTABLE_RVA),
    ] {
        put(at, image + rva);
    }
    for at in [
        0x2528, 0x2538, 0x2920, 0x3250, 0x3260, 0x32E8, 0x32F8, 0x3310,
    ] {
        put(at, 1);
    }
    put(0x2720, 3);
    put(0x3300, 2 << 16);
    byte(0x3700, 3);
    for i in 0..16 {
        unsafe { ((heap + 0x3600 + i * 2) as *mut u16).write_unaligned(i.min(2) as u16) };
    }
    for at in [0x2724, 0x274C] {
        byte(at, 16);
    }
    for at in [0x2728, 0x2750] {
        byte(at, 1);
    }
    let unit = [
        1f32, 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
    ];
    for at in [0x2790, 0x27D0, 0x3400] {
        unsafe { ((heap + at) as *mut [f32; 16]).write_unaligned(unit) };
    }
    MODULE_BASE.store(image, Ordering::Release);
    HOOKS_READY.store(true, Ordering::Release);
    unit_state
        .target_cloth_pose_importer
        .store(heap + 0x2280, Ordering::Release);
    unit_state
        .target_pose_importer
        .store(heap + 0x2280, Ordering::Release);
    refresh_owned_cloth_inputs(heap + 0x100, heap + 0x2280);
    let slot = (0..CLOTH_INSTANCE_SLOTS)
        .find(|&s| unit_state.cloth_instance_inputs[s].load(Ordering::Acquire) == heap + 0x1200)
        .unwrap();
    unit_state.cloth_instance_cores[slot].store(heap + 0x1000, Ordering::Release);
    unit_state.cloth_instance_pending_scale_bits[slot]
        .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
    unsafe extern "C" fn native(op: usize, _ctx: usize) -> usize {
        let unit_state = current_unit_state();
        let heap = op - 0x3200;
        unsafe {
            let calls = (heap + 0x4000) as *mut u64;
            calls.write(calls.read() + 1);
            let scale = ((heap + 0x2A00) as *const f32).read();
            for i in 0..3 {
                ((heap + 0x3100 + i * 16) as *mut [u32; 4]).write_unaligned([
                    (0.3 * scale).to_bits(),
                    (0.4 * scale).to_bits(),
                    (0.8 * scale).to_bits(),
                    0x7FC00011,
                ]);
                ((heap + 0x3000 + i * 16) as *mut [u32; 4]).write_unaligned([1, 2, 3, 0x7FC00019]);
            }
            if ((heap + 0x4008) as *const u8).read() == 1 {
                unit_state
                    .cloth_topology_generation
                    .fetch_add(1, Ordering::AcqRel);
            }
            if ((heap + 0x4008) as *const u8).read() == 2 {
                ((heap + 0x2740) as *mut usize).write_unaligned(heap + 0x3180);
            }
        }
        0x1234
    }
    let mut r: Registers = unsafe { std::mem::zeroed() };
    r.rcx = (heap + 0x3200) as u64;
    r.rdx = (heap + 0x2300) as u64;
    let invoke = |r: &mut Registers| cloth_skin_normal_hook(r, native as *const () as usize);
    for scale in [0.5f32, 0.5, 3., 1., 0.5] {
        unit_state
            .target_scale_bits
            .store(scale.to_bits(), Ordering::Release);
        unit_state.cloth_instance_applied_scale_bits[slot]
            .store(scale.to_bits(), Ordering::Release);
        let mut m = unit;
        for j in [0, 5, 10] {
            m[j] = scale;
        }
        unsafe { ((heap + 0x2A00) as *mut [f32; 16]).write_unaligned(m) };
        if scale != 1. {
            assert!(
                crate::memory_query::scoped(|| skin_normal_call(heap + 0x3200, heap + 0x2300))
                    .is_some()
            );
        }
        assert_eq!(invoke(&mut r), 0x1234);
        let raw = unsafe { ((heap + 0x3100) as *const [u32; 4]).read_unaligned() };
        assert_eq!(raw[3], 0x7FC00011);
        for (j, expected) in [0.3, 0.4, 0.8].into_iter().enumerate() {
            assert!(
                (f32::from_bits(raw[j]) - expected).abs() < 1e-6,
                "fresh SkinPN reference normal retains body scale {scale}"
            );
        }
        assert_eq!(
            unsafe { ((heap + 0x3000) as *const [u32; 4]).read_unaligned() },
            [1, 2, 3, 0x7FC00019]
        );
    }
    // Diagnostic decision regression: compare with frozen2.41, then require
    // a precise first gate. No native call or fixture modification is hidden
    // in this helper. Test both the outer and nested full-write guard oracles.
    let check_trace = |expected: &str| {
        let mut trace = SkinNormalTrace::default();
        let queries = crate::memory_query::query_count();
        let actual = crate::memory_query::scoped(|| {
            skin_normal_call_traced(heap + 0x3200, heap + 0x2300, &mut trace)
        });
        let actual_queries = crate::memory_query::query_count() - queries;
        let queries = crate::memory_query::query_count();
        let baseline =
            crate::memory_query::scoped(|| skin_normal_call_241(heap + 0x3200, heap + 0x2300));
        let baseline_queries = crate::memory_query::query_count() - queries;
        assert_eq!(actual, baseline, "decision drift at {expected}");
        assert_eq!(
            actual_queries, baseline_queries,
            "extra protection query at {expected}"
        );
        assert_eq!(trace.gate, expected);
        trace
    };
    check_trace("accepted");
    for (at, replacement, expected) in [
        (0x828, 0, "route_selection"),
        (
            0x3200,
            image + ER_CLOTH_MESH_PN_VTABLE_RVA,
            "operator_vtable",
        ),
        (0x3300, 1usize << 32, "pn_deformer"),
        (0x32F8, 0, "pn_controls_memory"),
        (0x3258, 0, "source_bone_index"),
        (0x2A00, 1f32.to_bits() as usize, "source_bone_basis"),
        (0x3400, 0.5f32.to_bits() as usize, "bind_basis"),
        (0x2790, 0.5f32.to_bits() as usize, "output_basis"),
        (0x3278, 1, "pn_unsupported_blend"),
    ] {
        let old = unsafe { ((heap + at) as *const usize).read_unaligned() };
        put(at, replacement);
        check_trace(expected);
        put(at, old);
    }
    let old_vertex = read_u16(heap + 0x3600).unwrap();
    unsafe { ((heap + 0x3600) as *mut u16).write_unaligned(9) };
    assert_eq!(check_trace("pn_vertex_range").expected, 9);
    unsafe { ((heap + 0x3600) as *mut u16).write_unaligned(1) };
    assert_eq!(check_trace("pn_unwritten_vertex").index, 0);
    unsafe { ((heap + 0x3600) as *mut u16).write_unaligned(old_vertex) };
    let old_control = read_u8(heap + 0x3700).unwrap();
    byte(0x3700, 7);
    assert_eq!(check_trace("pn_control_byte").observed[2], 7);
    byte(0x3700, old_control);
    // First failing source bone is retained, not the last valid one; nonfinite
    // lanes remain raw integer bits in diagnostic data.
    for at in [0x3250, 0x3260, 0x2920] {
        put(at, 2);
    }
    unsafe {
        ((heap + 0x3502) as *mut u16).write_unaligned(1);
        ((heap + 0x3440) as *mut [f32; 16]).write_unaligned(unit);
        ((heap + 0x2A40) as *mut [f32; 16]).write_unaligned(unit);
    }
    let trace = check_trace("source_bone_basis");
    assert_eq!(trace.index, 1);
    assert_eq!(trace.observed[0], 1);
    assert_eq!(trace.expected, 0.5f32.to_bits() as usize);
    assert_eq!(trace.matrix_bits, Some(unit.map(f32::to_bits)));
    unsafe { ((heap + 0x2A40) as *mut u32).write_unaligned(0x7FC00011) };
    assert_eq!(
        check_trace("source_bone_basis").matrix_bits.unwrap()[0],
        0x7FC00011
    );
    for at in [0x3250, 0x3260, 0x2920] {
        put(at, 1);
    }
    check_trace("accepted");
    // The trace is stack-only and must not change any fixture bytes.
    let before = memory.clone();
    check_trace("accepted");
    assert_eq!(memory, before);
    let mut trace = SkinNormalTrace::default();
    for use_new in [false, true, false, true] {
        let begin = std::time::Instant::now();
        for _ in 0..2000 {
            let result = crate::memory_query::scoped(|| {
                if use_new {
                    skin_normal_call_traced(heap + 0x3200, heap + 0x2300, &mut trace)
                } else {
                    skin_normal_call_241(heap + 0x3200, heap + 0x2300)
                }
            });
            std::hint::black_box(result).unwrap();
        }
        println!(
            "[ERPS-TEST-SKIN-GUARD-PERF] traced={use_new} mean_us={:.3}",
            begin.elapsed().as_secs_f64() * 1e6 / 2000.
        );
    }
    for (at, replacement) in [
        (0x828, 0),
        (0x3200, image + ER_CLOTH_MESH_PN_VTABLE_RVA),
        (0x3300, 1usize << 32),
        (0x32F8, 0),
        (0x3258, 0),
    ] {
        let old = unsafe { ((heap + at) as *const usize).read_unaligned() };
        put(at, replacement);
        assert_eq!(invoke(&mut r), 0x1234);
        assert_eq!(read_f32(heap + 0x3100), Some(0.15));
        put(at, old);
    }
    // Already-unit source / pending transition must not be divided again.
    unsafe { ((heap + 0x2A00) as *mut [f32; 16]).write_unaligned(unit) };
    assert_eq!(invoke(&mut r), 0x1234);
    assert_eq!(read_f32(heap + 0x3100), Some(0.3));
    let mut half = unit;
    for j in [0, 5, 10] {
        half[j] = 0.5;
    }
    unsafe { ((heap + 0x2A00) as *mut [f32; 16]).write_unaligned(half) };
    unit_state.cloth_instance_pending_scale_bits[slot].store(0.5f32.to_bits(), Ordering::Release);
    assert_eq!(invoke(&mut r), 0x1234);
    assert_eq!(read_f32(heap + 0x3100), Some(0.15));
    unit_state.cloth_instance_pending_scale_bits[slot]
        .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
    byte(0x4008, 2);
    assert_eq!(invoke(&mut r), 0x1234);
    assert_eq!(read_f32(heap + 0x3100), Some(0.15));
    put(0x2740, heap + 0x3100);
    byte(0x4008, 1);
    assert_eq!(invoke(&mut r), 0x1234);
    assert_eq!(read_f32(heap + 0x3100), Some(0.15));
    byte(0x4008, 0);
    let start = std::time::Instant::now();
    for _ in 0..2000 {
        assert_eq!(invoke(&mut r), 0x1234);
    }
    println!(
        "[ERPS-TEST-SKIN-PERF] mean_us={:.3}",
        start.elapsed().as_secs_f64() * 1e6 / 2000.
    );
    assert_eq!(unsafe { ((heap + 0x4000) as *const u64).read() }, 2014);
    // Exercise the installed trampoline with all 16 actual stolen bytes.
    // Never hot-patch a compiler-optimized Rust helper.
    {
        use windows::Win32::{
            Foundation::HANDLE,
            System::{
                Diagnostics::Debug::FlushInstructionCache,
                Memory::{
                    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_PROTECTION_FLAGS,
                    PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
                },
            },
        };
        let mut code = SKIN_PN_ENTRY.to_vec();
        code.extend_from_slice(&[0x48, 0xB8]);
        code.extend_from_slice(&(native as *const () as usize).to_le_bytes());
        code.extend_from_slice(&[0xFF, 0xE0]);
        let allocation =
            unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        assert!(!allocation.is_null());
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), allocation.cast::<u8>(), code.len())
        };
        let mut old = PAGE_PROTECTION_FLAGS::default();
        unsafe {
            VirtualProtect(allocation, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
            FlushInstructionCache(HANDLE(-1isize as *mut _), Some(allocation), 4096).unwrap();
        }
        let hook = unsafe {
            hook_closure_retn(
                allocation as usize,
                cloth_skin_normal_hook,
                CallbackOption::None,
                HookFlags::empty(),
            )
        }
        .unwrap();
        let call: unsafe extern "C" fn(usize, usize) -> usize = unsafe { transmute(allocation) };
        for i in 2015..=2017 {
            assert_eq!(unsafe { call(heap + 0x3200, heap + 0x2300) }, 0x1234);
            assert_eq!(read_f32(heap + 0x3100), Some(0.3));
            assert_eq!(unsafe { ((heap + 0x4000) as *const u64).read() }, i);
        }
        drop(hook);
        unsafe { VirtualFree(allocation, 0, MEM_RELEASE).unwrap() };
    }
    // 2.47's actual rejected bone106, minimized to one mapped source bone.
    // Full owner/output/native-return boundary, not a relaxed-matrix unit test.
    let captured = [
        3187786997u32,
        3174435322,
        1055208685,
        0,
        3183017603,
        1056670166,
        1019086462,
        0,
        3203532744,
        3181662160,
        3188797332,
        0,
        1085549919,
        1075012917,
        1088478478,
        1065353216,
    ]
    .map(f32::from_bits);
    put(0x1040, heap + 0x4100);
    put(0x1048, 1);
    put(0x4100, heap + 0x2900);
    put(0x4108, heap + 0x4200);
    put(0x4210, 1);
    put(0x4218, heap + 0x4300);
    // Group0, bone mapping0; a unique input entry rebuilt by the owned core.
    unsafe extern "C" fn contracted_native(op: usize, _ctx: usize) -> usize {
        let unit_state = current_unit_state();
        let heap = op - 0x3200;
        let m = unsafe { ((heap + 0x2A00) as *const [f32; 16]).read_unaligned() };
        for i in 0..3 {
            let n = [0.3f32, 0.4, 0.8];
            let out = std::array::from_fn::<_, 3, _>(|j| {
                (0..3).map(|k| n[k] * m[k * 4 + j]).sum::<f32>()
            });
            unsafe {
                ((heap + 0x3100 + i * 16) as *mut [u32; 4]).write_unaligned([
                    out[0].to_bits(),
                    out[1].to_bits(),
                    out[2].to_bits(),
                    0x7FC00011,
                ])
            };
        }
        match unsafe { ((heap + 0x4008) as *const u8).read() } {
            1 => {
                unit_state
                    .cloth_topology_generation
                    .fetch_add(1, Ordering::AcqRel);
            }
            2 => unsafe { ((heap + 0x2740) as *mut usize).write_unaligned(heap + 0x3180) },
            3 => unsafe { ((heap + 0x4100) as *mut usize).write_unaligned(0) },
            _ => {}
        }
        0x2345
    }
    let n = [0.3f32, 0.4, 0.8];
    let expected = std::array::from_fn::<_, 3, _>(|j| {
        (0..3)
            .map(|k| n[k] * captured[k * 4 + j] / 0.5)
            .sum::<f32>()
    });
    for scale in [0.5f32, 3., 0.5] {
        unit_state
            .target_scale_bits
            .store(scale.to_bits(), Ordering::Release);
        unit_state.cloth_instance_applied_scale_bits[slot]
            .store(scale.to_bits(), Ordering::Release);
        let mut source = captured;
        let mut core = unit;
        for j in [0, 5, 10] {
            core[j] = scale;
        }
        for k in [0, 4, 8] {
            for j in 0..3 {
                source[k + j] *= scale / 0.5;
            }
        }
        unsafe {
            ((heap + 0x2A00) as *mut [f32; 16]).write_unaligned(source);
            ((heap + 0x1090) as *mut [f32; 16]).write_unaligned(core);
        }
        assert!(
            crate::memory_query::scoped(|| skin_normal_call_241(heap + 0x3200, heap + 0x2300))
                .is_none()
        );
        let before = memory.clone();
        assert_eq!(
            cloth_skin_normal_hook(&mut r, contracted_native as *const () as usize),
            0x2345
        );
        let actual = unsafe { ((heap + 0x3100) as *const [u32; 4]).read_unaligned() };
        for j in 0..3 {
            assert!(
                (f32::from_bits(actual[j]) - expected[j]).abs() < 1e-6,
                "captured single-axis contraction bypasses fresh normal correction at scale{scale}"
            );
        }
        assert_eq!(actual[3], 0x7FC00011);
        // No positions, palettes, controls, maps or other instance state changed.
        let mut after = memory.clone();
        after[0x3100 / 16..0x3130 / 16].copy_from_slice(&before[0x3100 / 16..0x3130 / 16]);
        assert_eq!(after, before);
    }
    let rejected = || {
        assert_eq!(
            cloth_skin_normal_hook(&mut r.clone(), contracted_native as *const () as usize),
            0x2345
        );
        for (j, value) in expected.iter().enumerate() {
            assert!(
                (read_f32(heap + 0x3100 + j * 4).unwrap() - value * 0.5).abs() < 1e-6,
                "unproven contracted source received a normal write"
            );
        }
    };
    for (at, replacement) in [
        (0x828, 0),
        (0x1048, 0),
        (0x1040, 0),
        (0x4100, heap + 0x4500),
        (0x4108, 0),
        (0x4110, 1),
        (0x4130, 2),
        (0x4210, 0),
        (0x4218, 0),
        (0x4300, 0xFFFF),
        (0x4300, 0x4000),
        (0x1090, 1f32.to_bits() as usize),
    ] {
        let old = read_usize(heap + at).unwrap();
        put(at, replacement);
        rejected();
        put(at, old);
    }
    // Duplicate native entries cannot establish unique producer provenance.
    put(0x4138, heap + 0x2900);
    put(0x1048, 2);
    rejected();
    put(0x1048, 1);
    // A bad later bone prevents all writes even after the first fallback passed.
    for at in [0x3250, 0x3260, 0x2920] {
        put(at, 2);
    }
    unsafe {
        ((heap + 0x3502) as *mut u16).write_unaligned(1);
        ((heap + 0x3440) as *mut [f32; 16]).write_unaligned(unit);
        ((heap + 0x2A40) as *mut [f32; 16]).write_unaligned(unit);
    }
    rejected();
    for at in [0x3250, 0x3260, 0x2920] {
        put(at, 1);
    }
    unit_state.cloth_instance_pending_scale_bits[slot].store(0.5f32.to_bits(), Ordering::Release);
    rejected();
    unit_state.cloth_instance_pending_scale_bits[slot]
        .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
    for flag in [1, 2, 3] {
        byte(0x4008, flag);
        rejected();
        byte(0x4008, 0);
        put(0x2740, heap + 0x3100);
        put(0x4100, heap + 0x2900);
    }
    let begin = std::time::Instant::now();
    for _ in 0..2000 {
        assert_eq!(
            cloth_skin_normal_hook(&mut r, contracted_native as *const () as usize),
            0x2345
        );
    }
    println!(
        "[ERPS-TEST-CONTRACTED-SKIN-PERF] mean_us={:.3}",
        begin.elapsed().as_secs_f64() * 1e6 / 2000.
    );
    clear_target();
    HOOKS_READY.store(false, Ordering::Release);
    MODULE_BASE.store(0, Ordering::Release);
}

#[cfg(test)]
#[test]
fn contracted_skin_basis_rejects_mixed_scale_shear_reflection_and_nonfinite() {
    for scale in [0.5f32, 3.] {
        for axis in 0..3 {
            let mut m = [0f32; 16];
            m[15] = 1.;
            for j in [0, 5, 10] {
                m[j] = scale;
            }
            m[axis * 5] *= 0.934;
            assert!(skin_basis_is_single_axis_contraction(&m, scale));
            for index in 0..16 {
                let mut bad = m;
                bad[index] = f32::NAN;
                assert!(!skin_basis_is_single_axis_contraction(&bad, scale));
            }
            for v in [0., -0.934, 1.1, 1. / scale] {
                let mut bad = m;
                bad[axis * 5] = scale * v;
                assert!(!skin_basis_is_single_axis_contraction(&bad, scale));
            }
            let mut bad = m;
            bad[((axis + 1) % 3) * 5] *= 0.8;
            assert!(!skin_basis_is_single_axis_contraction(&bad, scale));
            let mut bad = m;
            bad[1] = scale * 0.1;
            assert!(!skin_basis_is_single_axis_contraction(&bad, scale));
            let mut bad = m;
            bad[3] = 0.5;
            assert!(!skin_basis_is_single_axis_contraction(&bad, scale));
        }
    }
}

#[cfg(test)]
#[test]
fn skin_normal_runtime_guard_rejects_changed_entry_vtable_and_calls() {
    let mut image = vec![0u8; ER_CLOTH_SKIN_PN_VTABLE_RVA + 0x40];
    let base = image.as_mut_ptr() as usize;
    let entries = [
        (ER_CLOTH_SKIN_PN_RVA, SKIN_PN_ENTRY),
        (0x15A99F3, &[0xE8, 0x58, 0xA3, 0xFF, 0xFF][..]),
        (0x15A3E61, &[0xE8, 0x3A, 0x15, 0, 0][..]),
        (0x15D8226, &[0x41, 0x0F, 0xB7, 0][..]),
    ]
    .into_iter()
    .chain(SKIN_SOURCE_SEAMS.iter().copied())
    .collect::<Vec<_>>();
    for &(at, raw) in &entries {
        image[at..at + raw.len()].copy_from_slice(raw);
    }
    let slot = ER_CLOTH_SKIN_PN_VTABLE_RVA + 0x20;
    image[slot..slot + 8].copy_from_slice(&(base + ER_CLOTH_SKIN_PN_RVA).to_le_bytes());
    assert!(validate_skin_normal_runtime(base));
    for (at, raw) in entries {
        for i in 0..raw.len() {
            image[at + i] ^= 1;
            assert!(!validate_skin_normal_runtime(base));
            image[at + i] ^= 1;
        }
    }
    image[slot] ^= 1;
    assert!(!validate_skin_normal_runtime(base));
}

#[derive(Clone, Copy)]
struct MeshPNCall {
    op: usize,
    input: usize,
    output: usize,
    scale: f32,
    span: Option<MeshNormalSpan>,
    mark: Option<(ClothOwnerRoute, usize, usize, u64)>,
}
thread_local! { static MESH_PN_CALL: Cell<Option<MeshPNCall>> = const { Cell::new(None) }; }
struct MeshPNGuard(Option<MeshPNCall>);
impl Drop for MeshPNGuard {
    fn drop(&mut self) {
        MESH_PN_CALL.set(self.0);
    }
}

fn mesh_normal_span(op: usize, output: usize) -> Option<MeshNormalSpan> {
    packed_normal_span(op, output, 0x78)
}

fn packed_normal_span(op: usize, output: usize, offset: usize) -> Option<MeshNormalSpan> {
    let result = packed_normal_span_traced(op, output, offset, &mut SkinNormalTrace::default());
    #[cfg(test)]
    assert_eq!(result, packed_normal_span_241(op, output, offset));
    result
}

fn packed_normal_span_traced(
    op: usize,
    output: usize,
    offset: usize,
    trace: &mut SkinNormalTrace,
) -> Option<MeshNormalSpan> {
    trace.step("pn_deformer", offset, [op, output, 0, 0], 0);
    let deformer = op.checked_add(offset)?;
    let partial = read_u8(deformer.checked_add(0x94)?)?;
    trace.observed[2] = partial as usize;
    if partial != 0 {
        return None;
    }
    trace.step(
        "pn_range",
        0,
        [deformer, 0, 0, 0],
        crate::cloth_mesh_scale::MAX_FRAMES,
    );
    let start = usize::from(read_u16(deformer.checked_add(0x90)?)?);
    let end = usize::from(read_u16(deformer.checked_add(0x92)?)?);
    trace.observed[1] = start;
    trace.observed[2] = end;
    let count = end.checked_sub(start)?.checked_add(1)?;
    if count > crate::cloth_mesh_scale::MAX_FRAMES {
        return None;
    }
    trace.step("pn_layout", 0, [output, 0, 0, 0], 0);
    let stride = usize::from(read_u8(output.checked_add(0x4C)?)?);
    let position_stride = usize::from(read_u8(output.checked_add(0x24)?)?);
    let position_flags = read_u8(output.checked_add(0x28)?)? & 1;
    let normal_flags = read_u8(output.checked_add(0x50)?)? & 1;
    trace.observed = [
        stride,
        position_stride,
        position_flags as usize,
        normal_flags as usize,
    ];
    // Bit 0 selects aligned float4 stores in 15A5330, not a compressed normal.
    // Only the two packed PN wrappers installed above may mark this pair.
    if ![12, 16].contains(&stride)
        || ![12, 16].contains(&position_stride)
        || position_flags != normal_flags
        || (normal_flags == 1 && (stride != 16 || position_stride != 16))
    {
        return None;
    }
    trace.step("pn_output_memory", 0, [output, count, stride, 0], 0);
    let data = read_usize(output.checked_add(0x40)?)?.checked_add(start * stride)?;
    let positions = read_usize(output.checked_add(0x18)?)?;
    trace.observed = [data, positions, count, stride];
    if data == 0
        || positions == 0
        || (normal_flags == 1 && (data % 16 != 0 || positions % 16 != 0))
        || !is_memory_accessible(data, count * stride, true)
    {
        return None;
    }
    // Do not divide un-written normals in a sparse/partial deformer. These
    // four layouts are verified against the native packed PN kernel; higher
    // blend formats and unknown control bytes fail closed for the whole pair.
    let mut seen = [0u64; 128];
    let mut blocks = [0usize; 4];
    for index in 0..8 {
        trace.step("pn_blend_header", index, [deformer, 0, 0, 0], 512);
        let at = deformer.checked_add(index * 16)?;
        let n = bounded_i32_count(at + 8, 512)?;
        trace.observed[1] = n;
        if index < 4 {
            if n != 0 {
                trace.gate = "pn_unsupported_blend";
                return None;
            }
            continue;
        }
        blocks[index - 4] = n;
        let begin = read_usize(at)?;
        let size = [224usize, 128 + 48, 128, 64][index - 4];
        trace.step("pn_blend_memory", index, [begin, n, size, 0], n * size);
        if n > 0 && !is_memory_accessible(begin, n * size, false) {
            return None;
        }
        for b in 0..n {
            for v in 0..16 {
                trace.step("pn_vertex_range", index, [b, v, start, end], 0);
                let vertex = usize::from(read_u16(begin + b * size + v * 2)?);
                trace.expected = vertex;
                if vertex < start || vertex > end {
                    return None;
                }
                let at = vertex - start;
                seen[at / 64] |= 1u64 << (at % 64);
            }
        }
    }
    trace.step("pn_controls_memory", 0, [deformer, 0, 0, 0], 512);
    let control_count = bounded_i32_count(deformer.checked_add(0x88)?, 512)?;
    let controls = read_usize(deformer.checked_add(0x80)?)?;
    trace.observed = [controls, control_count, 0, 0];
    let mut actual = [0usize; 4];
    if control_count == 0 || !is_memory_accessible(controls, control_count, false) {
        return None;
    }
    for i in 0..control_count {
        trace.step("pn_control_byte", i, [controls, control_count, 0, 0], 3);
        let c = usize::from(read_u8(controls + i)?);
        trace.observed[2] = c;
        if c >= 4 {
            return None;
        }
        actual[c] += 1;
    }
    trace.step("pn_control_counts", 0, actual, 0);
    if actual != blocks {
        if let Some(i) = (0..4).find(|&i| actual[i] != blocks[i]) {
            trace.index = i;
            trace.expected = blocks[i];
        }
        return None;
    }
    trace.step("pn_unwritten_vertex", 0, [start, end, count, 0], count);
    if let Some(i) = (0..count).find(|&i| seen[i / 64] & (1u64 << (i % 64)) == 0) {
        trace.index = i + start;
        return None;
    }
    trace.step(
        "pn_position_overlap",
        0,
        [data, count * stride, positions, 0],
        0,
    );
    let position_end = positions.checked_add((end + 1) * position_stride)?;
    trace.observed[3] = position_end;
    if !(data + count * stride <= positions || position_end <= data) {
        return None;
    }
    Some(MeshNormalSpan {
        data,
        count,
        stride,
    })
}

fn cloth_mesh_pn_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let timer = std::time::Instant::now();
    let r = unsafe { &*registers };
    let (op, local, input, output) = (r.rcx as usize, r.rdx as usize, r.r8 as usize, r.r9 as usize);
    let native: unsafe extern "C" fn(usize, usize, usize, usize) -> usize =
        unsafe { transmute(original) };
    let scale = current_scale();
    let call = crate::memory_query::scoped(|| {
        if !HOOKS_READY.load(Ordering::Acquire)
            || !valid_active_scale(scale)
            || read_usize(op)
                != Some(MODULE_BASE.load(Ordering::Acquire) + ER_CLOTH_MESH_PN_VTABLE_RVA)
        {
            return None;
        }
        Some(MeshPNCall {
            op,
            input,
            output,
            scale,
            span: None,
            mark: None,
        })
    });
    let guard = MeshPNGuard(MESH_PN_CALL.replace(call));
    let pre_time = timer.elapsed();
    let result = unsafe { native(op, local, input, output) };
    let post_timer = std::time::Instant::now();
    let finished = MESH_PN_CALL.get();
    drop(guard); // Never retain a marker into the next native invocation.
    if let Some(call) = finished {
        let _ = crate::memory_query::scoped(|| {
            let (route, slot, anchor, generation) = call.mark?;
            let span = call.span?;
            let scope = unit_state.target_cloth_scope.try_read().ok()?;
            if current_scale().to_bits() != call.scale.to_bits()
                || scope.anchor != anchor
                || unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) != route.core
                || unit_state
                    .target_cloth_pose_importer
                    .load(Ordering::Acquire)
                    != anchor
                || unit_state.cloth_topology_generation.load(Ordering::Acquire) != generation
                || unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire)
                    != call.scale.to_bits()
                || unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire)
                    != NO_PENDING_CLOTH_SCALE_BITS
                || !scope.route_is_current(route, MODULE_BASE.load(Ordering::Acquire), read_usize)
                || mesh_normal_span(op, output) != Some(span)
            {
                return None;
            }
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(span.data as *mut u8, span.count * span.stride)
            };
            let n = crate::cloth_mesh_scale::restore_normal_length(
                bytes,
                span.count,
                span.stride,
                call.scale,
            )?;
            unit_state
                .cloth_mesh_normal_rows
                .fetch_add(n as u64, Ordering::Relaxed);
            // BEGIN249-MESH-OUTPUT-OBSERVER
            if crate::ENABLE_SYNC_DIAGNOSTIC {
                crate::cloth_diagnostic::observe_mesh_output(op, output, span.data);
            }
            // END249-MESH-OUTPUT-OBSERVER
            Some(n)
        });
    }
    if call.is_some() {
        unit_state.cloth_mesh_pn_max_us.fetch_max(
            (pre_time + post_timer.elapsed()).as_micros() as u64,
            Ordering::Relaxed,
        );
    }
    result
}

fn cloth_mesh_frame_hook(registers: *mut Registers) {
    let unit_state = current_unit_state();
    if !HOOKS_READY.load(Ordering::Acquire) || !valid_active_scale(current_scale()) {
        return;
    }
    let registers = unsafe { &*registers };
    let start = std::time::Instant::now();
    let queries = crate::memory_query::query_count();
    unit_state
        .cloth_mesh_frame_seen
        .fetch_add(1, Ordering::Relaxed);
    let _ = crate::memory_query::scoped(|| {
        correct_cloth_mesh_frames(
            registers.rbx as usize,
            registers.rdi as usize,
            registers.r14 as usize,
            registers.rbp as usize,
        )
    });
    unit_state
        .cloth_mesh_frame_max_us
        .fetch_max(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    unit_state.cloth_mesh_frame_max_queries.fetch_max(
        crate::memory_query::query_count() - queries,
        Ordering::Relaxed,
    );
}

fn correct_cloth_mesh_frames(
    op: usize,
    buffer: usize,
    array: usize,
    output_inverse: usize,
) -> Option<usize> {
    let unit_state = current_unit_state();
    let scale = current_scale();
    let mode = read_u32(op.checked_add(0x50)?)?;
    if !valid_active_scale(scale) || mode > 1 {
        return None;
    }
    let op_type = read_usize(op)?;
    let image = MODULE_BASE.load(Ordering::Acquire);
    let pn = if op_type == image + ER_CLOTH_MESH_PN_VTABLE_RVA {
        if mode != 0 {
            return None;
        }
        let call = MESH_PN_CALL.get()?;
        if call.op != op
            || call.input != buffer
            || call.output.checked_add(0xD0)? != output_inverse
            || call.scale.to_bits() != scale.to_bits()
            || call.mark.is_some()
        {
            return None;
        }
        Some(call)
    } else if op_type == image + ER_CLOTH_MESH_P_VTABLE_RVA {
        None
    } else {
        return None;
    };
    let count = bounded_i32_count(array.checked_add(8)?, crate::cloth_mesh_scale::MAX_FRAMES)?;
    let subset = bounded_i32_count(op.checked_add(0x60)?, crate::cloth_mesh_scale::MAX_FRAMES)?;
    let triangle_count = bounded_i32_count(buffer.checked_add(0x38)?, 65536)?;
    if count == 0
        || count != if subset == 0 { triangle_count } else { subset }
        || read_u32(array.checked_add(12)?)? as usize & 0x3FFF_FFFF < count
        || bounded_i32_count(op.checked_add(0x70)?, crate::cloth_mesh_scale::MAX_FRAMES)? != count
        || read_u8(buffer.checked_add(0x24)?)? != 16
    {
        return None;
    }
    let frames = read_usize(array)?;
    let positions = read_usize(buffer.checked_add(0x18)?)?;
    let binds = read_usize(op.checked_add(0x68)?)?;
    let world = read_matrix(buffer.checked_add(0x90)?)?;
    // Only world-length, native simulated position buffers are proved here.
    // No blanket fix for unscaled skin inputs or arbitrary display buffers.
    if !cloth_matrix_array_basis_matches_scale(&world, 1.0)
        || frames % 4 != 0
        || !is_memory_accessible(frames, count * 64, true)
    {
        return None;
    }
    let base = MODULE_BASE.load(Ordering::Acquire);
    let anchor = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire);
    let generation = unit_state.cloth_topology_generation.load(Ordering::Acquire);
    let scope = unit_state.target_cloth_scope.try_read().ok()?;
    if anchor == 0 || scope.anchor != anchor {
        return None;
    }
    let mut selected = None;
    let mut visited = 0usize;
    for route in scope.routes.iter().copied().filter(|r| r.owner != 0) {
        let Some(slot) = (0..CLOTH_INSTANCE_SLOTS).find(|&s| {
            unit_state.cloth_instance_owners[s].load(Ordering::Acquire) == route.owner
                && unit_state.cloth_instance_inputs[s].load(Ordering::Acquire) == route.input
        }) else {
            continue;
        };
        if unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) != route.core
            || unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire)
                != scale.to_bits()
            || unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire)
                != NO_PENDING_CLOTH_SCALE_BITS
            || !scope.route_is_current(route, base, read_usize)
        {
            continue;
        }
        let (groups, group_count) =
            bounded_vector_span(route.core, 0x28, 0x30, 8, MAX_CLOTH_GROUPS)?;
        for g in 0..group_count {
            visited += 1;
            if visited > 256 {
                return None;
            }
            let holder = read_usize(groups + g * 8)?;
            let root = read_usize(holder)?;
            let buffers = read_usize(root.checked_add(0x20)?)?;
            let buffers_count = bounded_i32_count(root.checked_add(0x28)?, 128)?;
            let contains = (0..buffers_count)
                .any(|i| read_usize(buffers.saturating_add(i * 8)) == Some(buffer));
            if !contains {
                continue;
            }
            if let Some(call) = pn
                && !(0..buffers_count)
                    .any(|i| read_usize(buffers.saturating_add(i * 8)) == Some(call.output))
            {
                return None;
            }
            let children = read_usize(root.checked_add(0x40)?)?;
            let child_count =
                bounded_i32_count(root.checked_add(0x48)?, MAX_CLOTH_CHILDREN_PER_GROUP)?;
            for c in 0..child_count {
                visited += 1;
                if visited > 256 {
                    return None;
                }
                let child = read_usize(children.checked_add(c * 8)?)?;
                if read_usize(child.checked_add(0x20)?) != Some(positions)
                    || read_usize(child.checked_add(0x268)?) != Some(root)
                {
                    continue;
                }
                let sim = read_usize(child.checked_add(0x18)?)?;
                let particles =
                    bounded_i32_count(sim.checked_add(0x48)?, MAX_CLOTH_PARTICLES_PER_CHILD)?;
                if particles == 0 || selected.is_some() {
                    return None;
                }
                selected = Some((route, slot, particles));
            }
        }
    }
    let (route, slot, particles) = selected?;
    let disjoint = |address: usize, length: usize| -> Option<bool> {
        Some(frames.checked_add(count * 64)? <= address || address.checked_add(length)? <= frames)
    };
    if positions == 0
        || binds == 0
        || !disjoint(positions, particles * 16)?
        || !disjoint(binds, count * 64)?
        || !disjoint(array, 16)?
        || !disjoint(op, 0x138)?
        || !disjoint(buffer, 0x118)?
        || !disjoint(read_usize(buffer.checked_add(0x30)?)?, triangle_count * 6)?
        || current_scale().to_bits() != scale.to_bits()
        || unit_state
            .target_cloth_pose_importer
            .load(Ordering::Acquire)
            != anchor
        || unit_state.cloth_topology_generation.load(Ordering::Acquire) != generation
        || unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) != route.core
        || unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire)
            != scale.to_bits()
        || unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire)
            != NO_PENDING_CLOTH_SCALE_BITS
        || !scope.route_is_current(route, base, read_usize)
    {
        return None;
    }
    // Native allocated/fills these frames immediately above this hook. No
    // baseline, asset mutation, particle write or persistent temporary state.
    // Parse normal write coverage only AFTER proving local ownership.
    let pn_span = if let Some(call) = pn {
        let span = mesh_normal_span(op, call.output)?;
        if !disjoint(span.data, span.count * span.stride)? {
            return None;
        }
        Some(span)
    } else {
        None
    };
    let matrices = unsafe { std::slice::from_raw_parts_mut(frames as *mut [f32; 16], count) };
    let written = if mode == 1 {
        crate::cloth_mesh_scale::scale_area_depth(matrices, scale)?
    } else {
        crate::cloth_mesh_scale::scale_normal_depth(matrices, scale)?
    };
    if let Some(mut call) = pn {
        call.span = pn_span;
        call.mark = Some((route, slot, anchor, generation));
        MESH_PN_CALL.set(Some(call));
        // BEGIN249-MESH-FRAME-OBSERVER
        if crate::ENABLE_SYNC_DIAGNOSTIC {
            crate::cloth_diagnostic::observe_mesh_frames(
                crate::cloth_diagnostic::MeshBoundarySource {
                    op,
                    input_buffer: buffer,
                    output_buffer: call.output,
                    positions,
                    particles,
                    frames,
                    frame_count: count,
                    binds,
                    model: route.model,
                    owner: route.owner,
                    input: route.input,
                    generation,
                    scale_bits: scale.to_bits(),
                },
            );
        }
        // END249-MESH-FRAME-OBSERVER
    }
    unit_state
        .cloth_mesh_frame_calls
        .fetch_add(1, Ordering::Relaxed);
    unit_state
        .cloth_mesh_frames_written
        .fetch_add(written as u64, Ordering::Relaxed);
    if mode == 1 {
        unit_state
            .cloth_mesh_area_calls
            .fetch_add(1, Ordering::Relaxed);
        unit_state
            .cloth_mesh_area_frames
            .fetch_add(written as u64, Ordering::Relaxed);
    }
    Some(written)
}

fn anim_skeleton_get_affine_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let registers = unsafe { &*registers };
    let this = registers.rcx as usize;
    let output = registers.rdx as usize;
    let index = registers.r8 as u32;
    let target_before = unit_state.target_anim_skeleton.load(Ordering::Acquire);
    let scale = current_scale();
    let uses_fallback = if this == target_before && valid_active_scale(scale) {
        affine_index_uses_fallback(this, index)
    } else {
        None
    };
    let original: unsafe extern "C" fn(usize, usize, u32) -> usize = unsafe { transmute(original) };
    let result = unsafe { original(this, output, index) };

    if result == 0
        || this != target_before
        || this != unit_state.target_anim_skeleton.load(Ordering::Acquire)
        || !valid_active_scale(scale)
    {
        return result;
    }
    unit_state
        .affine_single_target_calls
        .fetch_add(1, Ordering::Relaxed);
    match uses_fallback {
        Some(false) => {}
        Some(true) if scale_affine_output(output, 1, scale) => {
            unit_state
                .affine_single_matrices_written
                .fetch_add(1, Ordering::Relaxed);
        }
        Some(true) | None => {
            unit_state.rejected_outputs.fetch_add(1, Ordering::Relaxed);
        }
    }
    result
}

fn anim_skeleton_get_affine_range_item_commit_hook(registers: *mut Registers) {
    let unit_state = current_unit_state();
    let registers = unsafe { &*registers };
    let this = registers.rbp as usize;
    let target_before = unit_state.target_anim_skeleton.load(Ordering::Acquire);
    let scale = current_scale();
    if this != target_before
        || this != unit_state.target_anim_skeleton.load(Ordering::Acquire)
        || !valid_active_scale(scale)
    {
        return;
    }

    let index = registers.rbx as u32;
    let action = affine_range_fallback_action(
        affine_index_uses_fallback(this, index),
        registers.r14 != 0,
        registers.rax as u32,
    );
    match action {
        AffineRangeFallbackAction::MappedPose => {}
        AffineRangeFallbackAction::Identity => {
            unit_state
                .affine_range_target_calls
                .fetch_add(1, Ordering::Relaxed);
            unit_state
                .affine_range_provider_identities
                .fetch_add(1, Ordering::Relaxed);
        }
        AffineRangeFallbackAction::ProviderFailed => {
            unit_state
                .affine_range_target_calls
                .fetch_add(1, Ordering::Relaxed);
            unit_state
                .affine_range_provider_failures
                .fetch_add(1, Ordering::Relaxed);
        }
        AffineRangeFallbackAction::Scale => {
            unit_state
                .affine_range_target_calls
                .fetch_add(1, Ordering::Relaxed);
            unit_state
                .affine_range_provider_successes
                .fetch_add(1, Ordering::Relaxed);
            if scale_affine_output(registers.rdi as usize, 1, scale) {
                unit_state
                    .affine_range_matrices_written
                    .fetch_add(1, Ordering::Relaxed);
            } else {
                unit_state.rejected_outputs.fetch_add(1, Ordering::Relaxed);
            }
        }
        AffineRangeFallbackAction::Reject => {
            unit_state.rejected_outputs.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn anim_skeleton_get_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let registers = unsafe { &*registers };
    let this = registers.rcx as usize;
    let output = registers.rdx as usize;
    let target_before = unit_state.target_anim_skeleton.load(Ordering::Acquire);
    let scale = current_scale();
    let caller = caller_from_stack(registers.rsp as usize);
    let arg8 = registers.r8 as u32;
    let original: unsafe extern "C" fn(usize, usize, u32) -> usize = unsafe { transmute(original) };
    let result = unsafe { original(this, output, arg8) };

    if result != 0 && this != target_before && valid_active_scale(scale) {
        record_matrix_candidate(MATRIX_CANDIDATE_KIND_SINGLE, caller, this, output, arg8, 0);
    }

    if result == 0
        || this != target_before
        || this != unit_state.target_anim_skeleton.load(Ordering::Acquire)
        || !valid_active_scale(scale)
    {
        return result;
    }
    unit_state
        .single_target_calls
        .fetch_add(1, Ordering::Relaxed);
    if scale_matrix_output(output, 1, scale) {
        unit_state
            .single_matrices_written
            .fetch_add(1, Ordering::Relaxed);
    } else {
        unit_state.rejected_outputs.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn anim_skeleton_get_range_hook(registers: *mut Registers, original: usize) -> usize {
    let unit_state = current_unit_state();
    let registers = unsafe { &*registers };
    let this = registers.rcx as usize;
    let output = registers.rdx as usize;
    let requested = registers.r8 as u32;
    let start = registers.r9 as u32;
    let target_before = unit_state.target_anim_skeleton.load(Ordering::Acquire);
    let scale = current_scale();
    let caller = caller_from_stack(registers.rsp as usize);
    let original: unsafe extern "C" fn(usize, usize, u32, u32) -> usize =
        unsafe { transmute(original) };
    let result = unsafe { original(this, output, requested, start) };

    if result != 0 && this != target_before && valid_active_scale(scale) {
        record_matrix_candidate(
            MATRIX_CANDIDATE_KIND_RANGE,
            caller,
            this,
            output,
            requested,
            start,
        );
    }

    if result == 0
        || this != target_before
        || this != unit_state.target_anim_skeleton.load(Ordering::Acquire)
        || !valid_active_scale(scale)
    {
        return result;
    }
    unit_state
        .range_target_calls
        .fetch_add(1, Ordering::Relaxed);
    let count = result.min(requested as usize);
    if count <= MAX_OUTPUTS_PER_CALL && scale_matrix_output(output, count, scale) {
        unit_state
            .range_matrices_written
            .fetch_add(count as u64, Ordering::Relaxed);
    } else {
        unit_state.rejected_outputs.fetch_add(1, Ordering::Relaxed);
    }
    result
}

fn caller_from_stack(stack_pointer: usize) -> usize {
    if stack_pointer == 0 {
        return 0;
    }
    // The hook executes at a normal x64 function entry, so RSP points at the
    // caller's return address. Avoid VirtualQuery here because this diagnostic
    // runs on a hot matrix path.
    unsafe { (stack_pointer as *const usize).read_unaligned() }
}

fn fifth_stack_argument_u8(stack_pointer: usize) -> u8 {
    if stack_pointer == 0 {
        return 0;
    }
    // At a Win64 function entry, RSP points to the return address, followed by
    // four eight-byte home slots. The fifth argument begins at RSP+0x28.
    unsafe { (stack_pointer.saturating_add(0x28) as *const u8).read_unaligned() }
}

fn module_relative_rva(module_base: usize, address: usize) -> Option<usize> {
    let module_end = module_base.checked_add(ER_SIZE_OF_IMAGE as usize)?;
    if address >= module_base && address < module_end {
        Some(address - module_base)
    } else {
        None
    }
}

fn matrix_candidate_key(kind: u32, caller_rva: usize) -> u64 {
    let mut key = (caller_rva as u64).rotate_left(17) ^ u64::from(kind);
    if key == 0 {
        key = 1;
    }
    key
}

fn record_matrix_candidate(
    kind: u32,
    caller: usize,
    this: usize,
    output: usize,
    arg8: u32,
    arg9: u32,
) {
    let unit_state = current_unit_state();
    let module_base = MODULE_BASE.load(Ordering::Acquire);
    let Some(caller_rva) = module_relative_rva(module_base, caller) else {
        return;
    };
    let key = matrix_candidate_key(kind, caller_rva);

    for slot in 0..MATRIX_CANDIDATE_SLOTS {
        let existing_key = unit_state.matrix_candidate_keys[slot].load(Ordering::Acquire);
        if existing_key == key {
            if unit_state.matrix_candidate_kinds[slot].load(Ordering::Relaxed) == kind
                && unit_state.matrix_candidate_caller_rvas[slot].load(Ordering::Relaxed)
                    == caller_rva
            {
                unit_state.matrix_candidate_last_this[slot].store(this, Ordering::Relaxed);
                unit_state.matrix_candidate_outputs[slot].store(output, Ordering::Relaxed);
                unit_state.matrix_candidate_arg8[slot].store(arg8, Ordering::Relaxed);
                unit_state.matrix_candidate_arg9[slot].store(arg9, Ordering::Relaxed);
                unit_state.matrix_candidate_hits[slot].fetch_add(1, Ordering::Release);
                return;
            }
            continue;
        }
        if existing_key != 0
            || unit_state.matrix_candidate_keys[slot]
                .compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            continue;
        }

        unit_state.matrix_candidate_kinds[slot].store(kind, Ordering::Relaxed);
        unit_state.matrix_candidate_caller_rvas[slot].store(caller_rva, Ordering::Relaxed);
        unit_state.matrix_candidate_first_this[slot].store(this, Ordering::Relaxed);
        unit_state.matrix_candidate_last_this[slot].store(this, Ordering::Relaxed);
        unit_state.matrix_candidate_outputs[slot].store(output, Ordering::Relaxed);
        unit_state.matrix_candidate_arg8[slot].store(arg8, Ordering::Relaxed);
        unit_state.matrix_candidate_arg9[slot].store(arg9, Ordering::Relaxed);
        unit_state.matrix_candidate_qword_48[slot].store(
            read_usize(this.saturating_add(0x48)).unwrap_or(0),
            Ordering::Relaxed,
        );
        unit_state.matrix_candidate_qword_68[slot].store(
            read_usize(this.saturating_add(0x68)).unwrap_or(0),
            Ordering::Relaxed,
        );
        unit_state.matrix_candidate_qword_88[slot].store(
            read_usize(this.saturating_add(0x88)).unwrap_or(0),
            Ordering::Relaxed,
        );
        unit_state.matrix_candidate_hits[slot].fetch_add(1, Ordering::Release);
        return;
    }
}

fn scale_pose_output(inner: usize, ratio: f32) -> Option<usize> {
    let metadata = read_usize(inner + POSE_METADATA_OFFSET)?;
    let count = read_i32(metadata + POSE_METADATA_COUNT_OFFSET)?;
    if count <= 0 || count as usize > MAX_OUTPUTS_PER_CALL {
        return None;
    }
    let count = count as usize;
    let output = read_usize(inner + POSE_OUTPUT_OFFSET)?;
    let bytes = count.checked_mul(POSE_TRANSFORM_STRIDE)?;
    if output == 0 || !is_memory_accessible(output, bytes, true) {
        return None;
    }

    for index in 0..count {
        let addr = output + index * POSE_TRANSFORM_STRIDE;
        let translation = read_vec4(addr + POSE_TRANSLATION_OFFSET)?;
        let local_scale = read_vec4(addr + POSE_LOCAL_SCALE_OFFSET)?;
        scaled_pose_vectors(translation, local_scale, ratio)?;
    }
    for index in 0..count {
        let addr = output + index * POSE_TRANSFORM_STRIDE;
        let translation = read_vec4(addr + POSE_TRANSLATION_OFFSET)?;
        let local_scale = read_vec4(addr + POSE_LOCAL_SCALE_OFFSET)?;
        let (translation, local_scale) = scaled_pose_vectors(translation, local_scale, ratio)?;
        write_vec4(addr + POSE_TRANSLATION_OFFSET, translation);
        write_vec4(addr + POSE_LOCAL_SCALE_OFFSET, local_scale);
    }
    Some(count)
}

fn scale_matrix_output(output: usize, count: usize, scale: f32) -> bool {
    if count == 0 || count > MAX_OUTPUTS_PER_CALL || !valid_active_scale(scale) {
        return false;
    }
    let Some(bytes) = count.checked_mul(MATRIX_STRIDE) else {
        return false;
    };
    if output == 0 || !is_memory_accessible(output, bytes, true) {
        return false;
    }

    for index in 0..count {
        let Some(matrix) = read_matrix(output + index * MATRIX_STRIDE) else {
            return false;
        };
        if scaled_row_matrix(matrix, scale).is_none() {
            return false;
        }
    }
    for index in 0..count {
        let addr = output + index * MATRIX_STRIDE;
        let Some(matrix) = read_matrix(addr) else {
            return false;
        };
        let Some(matrix) = scaled_row_matrix(matrix, scale) else {
            return false;
        };
        write_matrix(addr, matrix);
    }
    true
}

fn scale_affine_output(output: usize, count: usize, scale: f32) -> bool {
    if count == 0 || count > MAX_OUTPUTS_PER_CALL || !valid_active_scale(scale) {
        return false;
    }
    let Some(bytes) = count.checked_mul(AFFINE_MATRIX_STRIDE) else {
        return false;
    };
    if output == 0 || !is_memory_accessible(output, bytes, true) {
        return false;
    }

    for index in 0..count {
        let Some(matrix) = read_affine(output + index * AFFINE_MATRIX_STRIDE) else {
            return false;
        };
        if scaled_affine_matrix(matrix, scale).is_none() {
            return false;
        }
    }
    for index in 0..count {
        let addr = output + index * AFFINE_MATRIX_STRIDE;
        let Some(matrix) = read_affine(addr) else {
            return false;
        };
        let Some(matrix) = scaled_affine_matrix(matrix, scale) else {
            return false;
        };
        write_affine(addr, matrix);
    }
    true
}

fn affine_index_uses_fallback(this: usize, index: u32) -> Option<bool> {
    mapping_index_uses_fallback(read_bone_index_mapping(this)?, index)
}

fn affine_range_fallback_action(
    uses_fallback: Option<bool>,
    provider_present: bool,
    provider_result: u32,
) -> AffineRangeFallbackAction {
    match (uses_fallback, provider_present, provider_result != 0) {
        (Some(false), _, _) => AffineRangeFallbackAction::MappedPose,
        (Some(true), false, _) => AffineRangeFallbackAction::Identity,
        (Some(true), true, false) => AffineRangeFallbackAction::ProviderFailed,
        (Some(true), true, true) => AffineRangeFallbackAction::Scale,
        (None, _, _) => AffineRangeFallbackAction::Reject,
    }
}

fn read_bone_index_mapping(this: usize) -> Option<BoneIndexMapping> {
    let mapping = read_usize(this.checked_add(0x88)?)?;
    let count = read_u32(mapping.checked_add(0x04)?)?;
    if count == 0 || count as usize > MAX_OUTPUTS_PER_CALL {
        return None;
    }
    let entries = read_usize(mapping.checked_add(0x08)?)?;
    let entries_bytes = (count as usize).checked_mul(size_of::<i16>())?;
    if !is_memory_accessible(entries, entries_bytes, false) {
        return None;
    }
    Some(BoneIndexMapping { count, entries })
}

fn mapping_index_uses_fallback(mapping: BoneIndexMapping, index: u32) -> Option<bool> {
    if index >= mapping.count {
        return None;
    }
    let entry_offset = usize::try_from(index).ok()?.checked_mul(size_of::<i16>())?;
    let entry = mapping.entries.checked_add(entry_offset)?;
    let mapped_index = unsafe { (entry as *const i16).read_unaligned() };
    Some(mapping_value_uses_fallback(mapped_index))
}

fn mapping_value_uses_fallback(mapped_index: i16) -> bool {
    mapped_index < 0
}

fn scaled_translation(mut value: [f32; 4], scale: f32) -> Option<[f32; 4]> {
    if !valid_pose_scale_ratio(scale) || !value[..3].iter().all(|value| value.is_finite()) {
        return None;
    }
    for component in &mut value[..3] {
        *component = crate::scale_math::product(*component, scale)?;
    }
    Some(value)
}

fn scaled_pose_vectors(
    translation: [f32; 4],
    local_scale: [f32; 4],
    ratio: f32,
) -> Option<([f32; 4], [f32; 4])> {
    Some((
        scaled_translation(translation, ratio)?,
        scaled_translation(local_scale, ratio)?,
    ))
}

fn scaled_row_matrix(mut value: [f32; 16], scale: f32) -> Option<[f32; 16]> {
    if !valid_active_scale(scale) || !value.iter().all(|value| value.is_finite()) {
        return None;
    }
    for row in 0..4 {
        for column in 0..3 {
            let component = &mut value[row * 4 + column];
            *component = crate::scale_math::product(*component, scale)?;
        }
    }
    Some(value)
}

fn scaled_affine_matrix(mut value: [f32; 12], scale: f32) -> Option<[f32; 12]> {
    if !valid_active_scale(scale) || !value.iter().all(|value| value.is_finite()) {
        return None;
    }
    for component in &mut value {
        *component = crate::scale_math::product(*component, scale)?;
    }
    Some(value)
}

// BEGIN253-COLLIDER-ROTATION-SCOPE
/// Current player ownership for the exact native Simulate collider invocation.
/// Caller return-site/RSI are checked separately; no capture result authorizes it.
pub(crate) fn collider_rotation_scope(child: usize, collider: usize) -> Option<f32> {
    let unit_state = current_unit_state();
    let base = MODULE_BASE.load(Ordering::Acquire);
    let scale = current_scale();
    let generation = cloth_topology_generation();
    if !HOOKS_READY.load(Ordering::Acquire)
        || base == 0
        || !valid_active_scale(scale)
        || read_usize(child)? != base.checked_add(0x2D872A0)?
        || read_usize(collider)? != base.checked_add(0x2D8B758)?
    {
        return None;
    }
    let sim = read_usize(child.checked_add(0x18)?)?;
    if read_usize(sim)? != base.checked_add(0x2D8B6F8)? {
        return None;
    }
    let root = read_usize(child.checked_add(0x268)?)?;
    let colliders = read_usize(child.checked_add(0x168)?)?;
    // Use the same bound as cloth resource preflight. A smaller limit here
    // silently skips rigid velocity inputs for otherwise supported units.
    let count = bounded_i32_count(
        child.checked_add(0x170)?,
        crate::cloth_local_scale::MAX_COLLIDABLES,
    )?;
    if count == 0
        || (0..count)
            .filter(|i| colliders.checked_add(i * 8).and_then(read_usize) == Some(collider))
            .count()
            != 1
    {
        return None;
    }
    let children = read_usize(root.checked_add(0x40)?)?;
    let count = bounded_i32_count(root.checked_add(0x48)?, 32)?;
    if (0..count)
        .filter(|i| children.checked_add(i * 8).and_then(read_usize) == Some(child))
        .count()
        != 1
    {
        return None;
    }
    let scope = *unit_state.target_cloth_scope.try_read().ok()?;
    let anchor = unit_state
        .target_cloth_pose_importer
        .load(Ordering::Acquire);
    if scope.anchor == 0 || scope.anchor != anchor {
        return None;
    }
    let mut found = false;
    for route in scope.routes.iter().copied().filter(|r| r.owner != 0) {
        let Some(slot) = (0..CLOTH_INSTANCE_SLOTS).find(|&i| {
            unit_state.cloth_instance_owners[i].load(Ordering::Acquire) == route.owner
                && unit_state.cloth_instance_inputs[i].load(Ordering::Acquire) == route.input
                && unit_state.cloth_instance_cores[i].load(Ordering::Acquire) == route.core
                && unit_state.cloth_instance_applied_scale_bits[i].load(Ordering::Acquire)
                    == scale.to_bits()
                && unit_state.cloth_instance_pending_scale_bits[i].load(Ordering::Acquire)
                    == NO_PENDING_CLOTH_SCALE_BITS
        }) else {
            continue;
        };
        if !scope.route_is_current(route, base, read_usize) {
            continue;
        }
        let (groups, count) = bounded_vector_span(route.core, 0x28, 0x30, 8, MAX_CLOTH_GROUPS)?;
        for i in 0..count {
            if read_usize(read_usize(groups.checked_add(i * 8)?)?)? == root {
                if found {
                    return None;
                }
                found = true;
            }
        }
        if unit_state.cloth_instance_owners[slot].load(Ordering::Acquire) != route.owner
            || unit_state.cloth_instance_inputs[slot].load(Ordering::Acquire) != route.input
            || unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) != route.core
            || unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire)
                != scale.to_bits()
            || unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire)
                != NO_PENDING_CLOTH_SCALE_BITS
            || !scope.route_is_current(route, base, read_usize)
        {
            return None;
        }
    }
    (found
        && generation == cloth_topology_generation()
        && scale.to_bits() == current_scale().to_bits()
        && anchor
            == unit_state
                .target_cloth_pose_importer
                .load(Ordering::Acquire))
    .then_some(scale)
}
// END253-COLLIDER-ROTATION-SCOPE

/// Read-only, exact equipment/active-state selection for the one-shot recorder.
/// No address from an old log or spatial-proximity match is accepted.
pub(crate) fn diagnostic_targets() -> Option<[crate::cloth_diagnostic::Identity; 4]> {
    let unit_state = current_unit_state();
    use crate::cloth_diagnostic::{COUNTS, Identity};
    let base = MODULE_BASE.load(Ordering::Acquire);
    let scale = current_scale();
    if base == 0 || !valid_active_scale(scale) {
        return None;
    }
    let generation = cloth_topology_generation();
    let scope = *unit_state.target_cloth_scope.try_read().ok()?;
    if scope.anchor == 0
        || scope.anchor
            != unit_state
                .target_cloth_pose_importer
                .load(Ordering::Acquire)
    {
        return None;
    }
    let mut result = [Identity::default(); 4];
    let mut found_model = false;
    for route in scope.routes.iter().copied().filter(|r| r.owner != 0) {
        if !scope.route_is_current(route, base, read_usize) {
            continue;
        }
        let item = read_usize(route.model.checked_add(0x10)?)?;
        if read_usize(item.checked_add(0x6D8)?) != Some(9) {
            continue;
        }
        let capacity = read_usize(item.checked_add(0x6E0)?)?;
        if !(9..=256).contains(&capacity) {
            continue;
        }
        let name = read_usize(item.checked_add(0x6C8)?)?;
        if !b"BD_M_9004"
            .iter()
            .enumerate()
            .all(|(i, &c)| read_u16(name.saturating_add(i * 2)) == Some(u16::from(c)))
        {
            continue;
        }
        if found_model {
            return None;
        }
        found_model = true;
        let slot = (0..CLOTH_INSTANCE_SLOTS).find(|&s| {
            unit_state.cloth_instance_owners[s].load(Ordering::Acquire) == route.owner
                && unit_state.cloth_instance_inputs[s].load(Ordering::Acquire) == route.input
        })?;
        if unit_state.cloth_instance_cores[slot].load(Ordering::Acquire) != route.core
            || unit_state.cloth_instance_applied_scale_bits[slot].load(Ordering::Acquire)
                != scale.to_bits()
            || unit_state.cloth_instance_pending_scale_bits[slot].load(Ordering::Acquire)
                != NO_PENDING_CLOTH_SCALE_BITS
        {
            return None;
        }
        let (groups, group_count) = bounded_vector_span(route.core, 0x28, 0x30, 8, 16)?;
        for g in 0..group_count {
            let root = read_usize(read_usize(groups + g * 8)?)?;
            let asset = read_usize(root.checked_add(0x18)?)?;
            let states = read_usize(asset.checked_add(0x60)?)?;
            let state_count = bounded_i32_count(asset.checked_add(0x68)?, 128)?;
            let state_index = bounded_i32_count(root.checked_add(0x114)?, 127)?;
            if state_index >= state_count {
                return None;
            }
            let state = read_usize(states.checked_add(state_index * 8)?)?;
            let active = read_usize(state.checked_add(0x50)?)?;
            let active_count = bounded_i32_count(state.checked_add(0x58)?, 4)?;
            let children = read_usize(root.checked_add(0x40)?)?;
            let children_count = bounded_i32_count(root.checked_add(0x48)?, 32)?;
            for i in 0..active_count {
                let index = bounded_i32_count(active.checked_add(i * 4)?, 31)?;
                if index >= children_count {
                    return None;
                }
                let child = read_usize(children.checked_add(index * 8)?)?;
                if read_usize(child) != Some(base + 0x2D872A0)
                    || read_usize(child.checked_add(0x268)?) != Some(root)
                {
                    return None;
                }
                let sim = read_usize(child.checked_add(0x18)?)?;
                if read_usize(sim) != Some(base + 0x2D8B6F8) {
                    return None;
                }
                let count = bounded_i32_count(sim.checked_add(0x48)?, 512)?;
                let bucket = COUNTS.iter().position(|&n| n == count)?;
                if result[bucket].child != 0 {
                    return None;
                }
                result[bucket] = Identity {
                    base,
                    child,
                    sim,
                    root,
                    model: route.model,
                    owner: route.owner,
                    input: route.input,
                    generation,
                    scale_bits: scale.to_bits(),
                    count,
                };
            }
        }
    }
    if !found_model
        || result.iter().any(|i| i.child == 0)
        || generation != cloth_topology_generation()
        || current_scale().to_bits() != scale.to_bits()
    {
        return None;
    }
    Some(result)
}

#[cfg(test)]
#[test]
fn diagnostic_selector_requires_exact_current_bd9004_active_children() {
    let unit_state = current_unit_state();
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    let mut data = vec![0usize; 0x10000 / 8];
    let heap = data.as_ptr() as usize;
    let image = 0x140000000usize;
    let put = |data: &mut Vec<usize>, at: usize, value: usize| data[at / 8] = value;
    for (at, offset) in [
        (0x748, 0x800),
        (0x150, 0xA00),
        (0x808, 0xA00),
        (0x828, 0xA00),
        (0xA10, 0x2000),
        (0xB30, 0xC00),
        (0xC40, 0xE00),
        (0xD20, 0x1200),
        (0xE30, 0x1000),
        (0x26C8, 0x2800),
        (0x1028, 0x3000),
        (0x1030, 0x3008),
        (0x3000, 0x3020),
        (0x3020, 0x3200),
        (0x3218, 0x3400),
        (0x3460, 0x3600),
        (0x3600, 0x3800),
        (0x3850, 0x3900),
        (0x3240, 0x3A00),
    ] {
        put(&mut data, at, heap + offset);
    }
    for (at, rva) in [
        (0x800, 0x2B35980),
        (0xA00, 0x2B36718),
        (0xC00, 0x2B92A60),
        (0xE00, 0x329A2F8),
        (0x1200, 0x2B70360),
    ] {
        put(&mut data, at, image + rva);
    }
    for (at, n) in [
        (0x26D8, 9),
        (0x26E0, 15),
        (0x3468, 1),
        (0x3858, 4),
        (0x3248, 4),
    ] {
        put(&mut data, at, n);
    }
    for (n, c) in "BD_M_9004".encode_utf16().enumerate() {
        unsafe {
            ((heap + 0x2800 + n * 2) as *mut u16).write(c);
        }
    }
    for (n, count) in crate::cloth_diagnostic::COUNTS.into_iter().enumerate() {
        let child = 0x4000 + n * 0x400;
        let sim = 0x6000 + n * 0x400;
        put(&mut data, 0x3A00 + n * 8, heap + child);
        unsafe {
            ((heap + 0x3900 + n * 4) as *mut u32).write(n as u32);
        }
        put(&mut data, child, image + 0x2D872A0);
        put(&mut data, child + 0x18, heap + sim);
        put(&mut data, child + 0x268, heap + 0x3200);
        put(&mut data, sim, image + 0x2D8B6F8);
        put(&mut data, sim + 0x48, count);
    }
    MODULE_BASE.store(image, Ordering::Release);
    HOOKS_READY.store(true, Ordering::Release);
    unit_state
        .target_cloth_pose_importer
        .store(heap + 0x1600, Ordering::Release);
    unit_state
        .target_scale_bits
        .store(0.5f32.to_bits(), Ordering::Release);
    refresh_owned_cloth_inputs(heap + 0x100, heap + 0x1600);
    let slot = (0..CLOTH_INSTANCE_SLOTS)
        .find(|&s| unit_state.cloth_instance_owners[s].load(Ordering::Acquire) == heap + 0xC00)
        .unwrap();
    unit_state.cloth_instance_cores[slot].store(heap + 0x1000, Ordering::Release);
    unit_state.cloth_instance_applied_scale_bits[slot].store(0.5f32.to_bits(), Ordering::Release);
    unit_state.cloth_instance_pending_scale_bits[slot].store(0, Ordering::Release);
    // BEGIN253-COLLIDER-ROTATION-TEST
    put(&mut data, 0x4168, heap + 0x8000);
    put(&mut data, 0x4170, 1);
    put(&mut data, 0x8000, heap + 0x8100);
    put(&mut data, 0x8100, image + 0x2D8B758);
    let rotation_scope =
        || crate::memory_query::scoped(|| collider_rotation_scope(heap + 0x4000, heap + 0x8100));
    let unchanged = data.clone();
    assert_eq!(rotation_scope(), Some(0.5));
    crate::cloth_collider_rotation_hook::exercise_owned_route(
        image,
        heap + 0x4000,
        heap + 0x6000,
        heap + 0x8100,
    );
    assert_eq!(data, unchanged);
    // c3185 has 112 colliders in its second simulation. The actual hook must
    // still send rigid matrices to native velocity, including its last entry.
    let mut many_colliders: Vec<usize> = (0..256).map(|i| heap + 0x9000 + i * 8).collect();
    many_colliders[0] = heap + 0x8100;
    put(&mut data, 0x4168, many_colliders.as_ptr() as usize);
    put(&mut data, 0x4170, many_colliders.len());
    unit_state
        .target_scale_bits
        .store(0.85f32.to_bits(), Ordering::Release);
    unit_state.cloth_instance_applied_scale_bits[slot].store(0.85f32.to_bits(), Ordering::Release);
    for count in [64, 65, 112, 256] {
        put(&mut data, 0x4170, count);
        println!("COLLIDER-COUNT {count}");
        crate::cloth_collider_rotation_hook::exercise_owned_route_at_scale(
            image,
            heap + 0x4000,
            heap + 0x6000,
            heap + 0x8100,
            0.85,
        );
        let start = std::time::Instant::now();
        for _ in 0..256 {
            assert_eq!(rotation_scope(), Some(0.85));
        }
        println!(
            "COLLIDER-SCOPE-COST count={count} calls=256 elapsed_us={}",
            start.elapsed().as_micros()
        );
    }
    many_colliders[0] = heap + 0x9000;
    many_colliders[255] = heap + 0x8100;
    assert_eq!(
        rotation_scope(),
        Some(0.85),
        "last collider remains eligible"
    );
    many_colliders[0] = heap + 0x8100;
    assert_eq!(rotation_scope(), None, "duplicate collider still rejected");
    std::hint::black_box(&many_colliders);
    unit_state
        .target_scale_bits
        .store(0.5f32.to_bits(), Ordering::Release);
    unit_state.cloth_instance_applied_scale_bits[slot].store(0.5f32.to_bits(), Ordering::Release);
    put(&mut data, 0x4168, heap + 0x8000);
    put(&mut data, 0x4170, 1);
    for (at, bad) in [
        (0x8000, heap + 0x8200),
        (0x4170, 257),
        (0x4170, 0),
        (0x4168, usize::MAX - 3),
        (0x4268, heap + 0x3400),
        (0x3A00, heap + 0x4400),
        (0x3020, heap + 0x3400),
        (0x828, 0),
        (0xD20, heap + 0x1800),
        (0x1030, 0),
    ] {
        let old = data[at / 8];
        put(&mut data, at, bad);
        assert_eq!(rotation_scope(), None, "offset {at:x}");
        put(&mut data, at, old);
    }
    unit_state.cloth_instance_pending_scale_bits[slot].store(3f32.to_bits(), Ordering::Release);
    assert_eq!(rotation_scope(), None);
    unit_state.cloth_instance_pending_scale_bits[slot].store(0, Ordering::Release);
    unit_state
        .target_scale_bits
        .store(1f32.to_bits(), Ordering::Release);
    assert_eq!(rotation_scope(), None);
    unit_state
        .target_scale_bits
        .store(0.5f32.to_bits(), Ordering::Release);
    assert_eq!(rotation_scope(), Some(0.5));
    // END253-COLLIDER-ROTATION-TEST
    let original = data.clone();
    assert_eq!(
        crate::memory_query::scoped(diagnostic_targets)
            .unwrap()
            .map(|i| i.count),
        crate::cloth_diagnostic::COUNTS
    );
    assert_eq!(data, original);
    data[0x2800 / 8] ^= 1;
    assert!(diagnostic_targets().is_none());
    data[0x2800 / 8] ^= 1;
    put(&mut data, 0x3248, 3);
    assert!(diagnostic_targets().is_none());
    put(&mut data, 0x3248, 4);
    put(&mut data, 0x828, 0);
    assert!(diagnostic_targets().is_none());
    put(&mut data, 0x828, heap + 0xA00);
    unit_state.cloth_instance_pending_scale_bits[slot].store(3f32.to_bits(), Ordering::Release);
    assert!(diagnostic_targets().is_none());
    unit_state.cloth_instance_pending_scale_bits[slot].store(0, Ordering::Release);
    unit_state
        .target_scale_bits
        .store(1f32.to_bits(), Ordering::Release);
    assert!(diagnostic_targets().is_none());
    clear_target();
    MODULE_BASE.store(0, Ordering::Release);
    HOOKS_READY.store(false, Ordering::Release);
    for s in 0..CLOTH_INSTANCE_SLOTS {
        unit_state.cloth_instance_owners[s].store(0, Ordering::Release);
        unit_state.cloth_instance_inputs[s].store(0, Ordering::Release);
    }
    *unit_state.target_cloth_scope.write().unwrap() = ClothOwnerScope::default();
}

fn current_scale() -> f32 {
    let unit_state = current_unit_state();
    f32::from_bits(unit_state.target_scale_bits.load(Ordering::Acquire))
}

fn committed_cloth_scale(bits: u32) -> Option<f32> {
    if bits == NO_PENDING_CLOTH_SCALE_BITS || bits == IN_PROGRESS_CLOTH_SCALE_BITS {
        return None;
    }
    let scale = f32::from_bits(bits);
    valid_scale(scale).then_some(scale)
}

fn mark_secondary_reference_dirty(owner: usize) -> bool {
    let Some(address) = owner.checked_add(CLOTH_SECONDARY_DIRTY_OFFSET) else {
        return false;
    };
    if !is_memory_accessible(address, size_of::<u8>(), true) {
        return false;
    }
    unsafe { (address as *mut u8).write_unaligned(1) };
    true
}

fn valid_scale(scale: f32) -> bool {
    crate::scale_math::valid(scale)
}

fn valid_active_scale(scale: f32) -> bool {
    valid_scale(scale) && (scale - 1.0).abs() > f32::EPSILON
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PoseScaleAction {
    Noop,
    Scale(f32),
    Reject,
}

fn plan_pose_scale(
    previous_applied_scale: f32,
    requested_scale: f32,
    materialized_before: Option<u8>,
    materialized_after: Option<u8>,
) -> PoseScaleAction {
    if !valid_scale(previous_applied_scale)
        || !valid_scale(requested_scale)
        || materialized_after != Some(1)
    {
        return PoseScaleAction::Reject;
    }

    let source_scale = match materialized_before {
        Some(0) => 1.0,
        Some(1) => previous_applied_scale,
        _ => return PoseScaleAction::Reject,
    };
    let ratio = requested_scale / source_scale;
    if !ratio.is_finite() || ratio <= 0.0 {
        return PoseScaleAction::Reject;
    }
    if (ratio - 1.0).abs() <= f32::EPSILON {
        PoseScaleAction::Noop
    } else if valid_pose_scale_ratio(ratio) {
        PoseScaleAction::Scale(ratio)
    } else {
        PoseScaleAction::Reject
    }
}

fn valid_pose_scale_ratio(ratio: f32) -> bool {
    crate::scale_math::valid(ratio) && (ratio - 1.0).abs() > f32::EPSILON
}

fn module_base(module: HMODULE) -> usize {
    module.0 as usize
}

fn read_u8(addr: usize) -> Option<u8> {
    is_memory_accessible(addr, size_of::<u8>(), false)
        .then(|| unsafe { (addr as *const u8).read_unaligned() })
}

fn read_u16(addr: usize) -> Option<u16> {
    is_memory_accessible(addr, size_of::<u16>(), false)
        .then(|| unsafe { (addr as *const u16).read_unaligned() })
}

fn read_u32(addr: usize) -> Option<u32> {
    is_memory_accessible(addr, size_of::<u32>(), false)
        .then(|| unsafe { (addr as *const u32).read_unaligned() })
}

fn read_f32(addr: usize) -> Option<f32> {
    is_memory_accessible(addr, size_of::<f32>(), false)
        .then(|| unsafe { (addr as *const f32).read_unaligned() })
}

fn read_i32(addr: usize) -> Option<i32> {
    is_memory_accessible(addr, size_of::<i32>(), false)
        .then(|| unsafe { (addr as *const i32).read_unaligned() })
}

fn read_usize(addr: usize) -> Option<usize> {
    is_memory_accessible(addr, size_of::<usize>(), false)
        .then(|| unsafe { (addr as *const usize).read_unaligned() })
}

fn read_vec4(addr: usize) -> Option<[f32; 4]> {
    is_memory_accessible(addr, size_of::<[f32; 4]>(), false)
        .then(|| unsafe { (addr as *const [f32; 4]).read_unaligned() })
}

fn write_vec4(addr: usize, value: [f32; 4]) {
    unsafe { (addr as *mut [f32; 4]).write_unaligned(value) };
}

fn read_matrix(addr: usize) -> Option<[f32; 16]> {
    is_memory_accessible(addr, size_of::<[f32; 16]>(), false)
        .then(|| unsafe { (addr as *const [f32; 16]).read_unaligned() })
}

fn read_affine(addr: usize) -> Option<[f32; 12]> {
    is_memory_accessible(addr, size_of::<[f32; 12]>(), false)
        .then(|| unsafe { (addr as *const [f32; 12]).read_unaligned() })
}

fn write_affine(addr: usize, value: [f32; 12]) {
    unsafe { (addr as *mut [f32; 12]).write_unaligned(value) };
}

fn write_matrix(addr: usize, value: [f32; 16]) {
    unsafe { (addr as *mut [f32; 16]).write_unaligned(value) };
}

fn is_memory_accessible(addr: usize, len: usize, write: bool) -> bool {
    crate::memory_query::accessible_span(addr, len, write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_equipment_inputs_reach_source_transition_and_collision_paths() {
        let unit_state = current_unit_state();
        let _module_guard = MODULE_TEST_LOCK.lock().unwrap();
        // Three independent inputs mirror the frozen HD / BD / HR live chain.
        // A duplicate equipment slot must not double-apply the same owner.
        let base = 0x140000000;
        let mut player = vec![0usize; 0x650 / 8];
        let mut assembly = vec![0usize; 0x370 / 8];
        let mut models = [0; 3].map(|_| vec![0usize; 0x150 / 8]);
        let mut owners = [0; 3].map(|_| vec![0usize; 0x140 / 8]);
        let mut inputs = [0; 3].map(|_| vec![0usize; 0xA0 / 8]);
        let mut inners = [0; 3].map(|_| vec![0usize; 0x70 / 8]);
        let mut cores = [0; 3].map(|_| vec![0usize; 0x120 / 8]);
        let mut holders = [[0usize; 1]; 3];
        let mut groups = [[0usize; 1]; 3];
        let mut roots = [0; 3].map(|_| vec![0usize; 0x50 / 8]);
        let mut children = [[0usize; 1]; 3];
        let mut sims = [[0usize; 1]; 3];
        let mut child_objects = [[0usize; 4]; 3];
        let anchor = inputs[0].as_ptr() as usize;
        let player_address = player.as_ptr() as usize;
        player[0x648 / 8] = assembly.as_ptr() as usize;
        player[0x50 / 8] = 0x123456;
        assembly[0] = base + crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA;
        assembly[1] = player[0x50 / 8];
        for i in 0..3 {
            assembly[(0x38 + i * 8) / 8] = models[i].as_ptr() as usize;
            models[i][0] = base + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA;
            models[i][0x130 / 8] = owners[i].as_ptr() as usize;
            owners[i][0] = base + ER_CLOTH_MODEL_VTABLE_RVA;
            owners[i][0x120 / 8] = inputs[i].as_ptr() as usize;
            owners[i][0x40 / 8] = inners[i].as_ptr() as usize;
            inputs[i][0] = base + ER_POSE_IMPORTER_VTABLE_RVA;
            inners[i][0] = base + ER_CLOTH_INNER_VTABLE_RVA;
            inners[i][0x30 / 8] = cores[i].as_ptr() as usize;
            inners[i][0x60 / 8] = 1;
            child_objects[i][3] = sims[i].as_mut_ptr() as usize;
            children[i][0] = child_objects[i].as_ptr() as usize;
            roots[i][0x40 / 8] = children[i].as_ptr() as usize;
            roots[i][0x48 / 8] = 1;
            holders[i][0] = roots[i].as_ptr() as usize;
            groups[i][0] = holders[i].as_ptr() as usize;
            cores[i][0x28 / 8] = groups[i].as_ptr() as usize;
            cores[i][0x30 / 8] = groups[i].as_ptr() as usize + 8;
        }
        // Exercise the last statically proven slot as well as shared ownership.
        assembly[0xF8 / 8] = models[2].as_ptr() as usize;
        assembly[0x48 / 8] = 0;
        let direct_before = crate::memory_query::query_count();
        let scope = ClothOwnerScope::capture(player_address, anchor, base, read_usize);
        let direct_queries = crate::memory_query::query_count() - direct_before;
        let scoped_before = crate::memory_query::query_count();
        let batched = crate::memory_query::scoped(|| {
            ClothOwnerScope::capture(player_address, anchor, base, read_usize)
        });
        let batched_queries = crate::memory_query::query_count() - scoped_before;
        assert_eq!(
            batched, scope,
            "query batching must not cache ownership values"
        );
        println!("equipment capture queries direct={direct_queries} batched={batched_queries}");
        assert!(batched_queries * 2 < direct_queries);
        assert_eq!(
            scope.routes.iter().filter(|route| route.owner != 0).count(),
            3
        );
        assert_eq!(
            scope
                .find(owners[2].as_ptr() as usize, inputs[2].as_ptr() as usize)
                .unwrap()
                .slot_address,
            assembly.as_ptr() as usize + 0xF8
        );
        // Unknown layouts/back-references are denied, even while readable.
        assembly[0] ^= 8;
        assert_eq!(
            ClothOwnerScope::capture(player_address, anchor, base, read_usize),
            ClothOwnerScope::default()
        );
        assembly[0] ^= 8;
        let back_reference = assembly[1];
        assembly[1] = 0;
        assert_eq!(
            ClothOwnerScope::capture(player_address, anchor, base, read_usize),
            ClothOwnerScope::default()
        );
        assembly[1] = back_reference;
        assembly[0x48 / 8] = models[2].as_ptr() as usize;
        MODULE_BASE.store(base, Ordering::Release);
        unit_state
            .target_cloth_pose_importer
            .store(anchor, Ordering::Release);
        *unit_state.target_cloth_scope.write().unwrap() = scope;
        for i in 0..3 {
            record_cloth_instance_binding(owners[i].as_ptr() as usize, inputs[i].as_ptr() as usize);
        }
        refresh_owned_cloth_inputs(player_address, anchor);
        let selected: Vec<_> = (0..3)
            .map(|i| target_cloth_slot_and_owner_for_inner(inners[i].as_ptr() as usize))
            .collect();
        assert!(
            selected.iter().all(Option::is_some),
            "equipment source paths excluded: {selected:?}"
        );
        assert_eq!(queue_cloth_scale_transitions(anchor, 0.5), 3);
        assert_eq!(queue_cloth_scale_transitions(anchor, 0.5), 0);
        let snapshots = cloth_instance_snapshots(anchor);
        assert_eq!(snapshots.iter().filter(|s| s.valid).count(), 3);
        for i in 0..3 {
            let slot = selected[i].unwrap().0;
            assert_eq!(
                target_cloth_input(slot, anchor),
                Some(inputs[i].as_ptr() as usize)
            );
            assert_eq!(snapshots[slot].input, inputs[i].as_ptr() as usize);
            assert!(snapshots[slot].equipment_owned);
            assert_eq!(
                target_solver_source_slot(
                    inners[i].as_ptr() as usize,
                    owners[i].as_ptr() as usize + 0x60,
                    inputs[i].as_ptr() as usize + 0x48
                ),
                Some(slot)
            );
            if i != 0 {
                assert_eq!(
                    target_solver_source_slot(
                        inners[i].as_ptr() as usize,
                        owners[i].as_ptr() as usize + 0x60,
                        anchor + 0x48
                    ),
                    None
                );
            }
        }
        let simulations = target_cloth_simulation_set(anchor);
        assert!(simulations.readable && !simulations.truncated);
        assert_eq!(simulations.count, 3);
        // Shared sim resources are captured once across independent owners.
        child_objects[2][3] = child_objects[0][3];
        std::hint::black_box(&child_objects);
        assert_eq!(target_cloth_simulation_set(anchor).count, 2);
        // Shared importer pointers still have distinct per-owner transitions;
        // source writes are scoped to the synchronous native invocation.
        owners[2][0x120 / 8] = inputs[1].as_ptr() as usize;
        refresh_owned_cloth_inputs(player_address, anchor);
        assert_eq!(
            target_cloth_input(selected[2].unwrap().0, anchor),
            Some(inputs[1].as_ptr() as usize)
        );
        // A replaced core invalidates the old route before any task refresh.
        let old_core = inners[2][0x30 / 8];
        inners[2][0x30 / 8] = cores[0].as_ptr() as usize;
        assert!(target_cloth_slot_and_owner_for_inner(inners[2].as_ptr() as usize).is_none());
        inners[2][0x30 / 8] = old_core;
        // A noisy global setter registry cannot evict the player's routes.
        for npc in 0..100 {
            record_cloth_instance_binding(0x10000 + npc * 0x1000, 0x30000 + npc * 0x1000);
        }
        for inner in &inners {
            assert!(target_cloth_slot_and_owner_for_inner(inner.as_ptr() as usize).is_some());
        }
        // Revocation is immediate even before a refresh: old object is readable,
        // with the right vtable, but no longer belongs to the player's slot.
        assembly[0x40 / 8] = 0;
        assert!(target_cloth_slot_and_owner_for_inner(inners[1].as_ptr() as usize).is_none());
        // Matching roots or vtables cannot authorize another player's input.
        let stale_scope = ClothOwnerScope::capture(player_address, anchor, base, read_usize);
        assert!(
            stale_scope
                .find(owners[1].as_ptr() as usize, inputs[1].as_ptr() as usize)
                .is_none()
        );
        refresh_owned_cloth_inputs(player_address, anchor);
        assert_eq!(
            unit_state.cloth_instance_pending_scale_bits[selected[1].unwrap().0]
                .load(Ordering::Acquire),
            0
        );
        // Removing a selected-input equipment owner cannot resurrect it through
        // the legacy exact-input path on the following refresh either.
        assembly[0x38 / 8] = 0;
        refresh_owned_cloth_inputs(player_address, anchor);
        assert!(target_cloth_slot_and_owner_for_inner(inners[0].as_ptr() as usize).is_none());
        clear_target();
        MODULE_BASE.store(0, Ordering::Release);
        for slot in 0..CLOTH_INSTANCE_SLOTS {
            unit_state.cloth_instance_owners[slot].store(0, Ordering::Release);
            unit_state.cloth_instance_inputs[slot].store(0, Ordering::Release);
        }
        *unit_state.target_cloth_scope.write().unwrap() = ClothOwnerScope::default();
    }

    #[test]
    #[ignore = "requires private replay data; set ER_CHARACTER_SCALE_FIXTURES"]
    fn bd9004_mesh_reference_hook_corrects_owned_fresh_frames() {
        let unit_state = current_unit_state();
        let _lock = MODULE_TEST_LOCK.lock().unwrap();
        clear_target();
        let image = 0x140000000;
        let mut memory = vec![0usize; 0x4000 / 8];
        let heap = memory.as_mut_ptr() as usize;
        fn put(m: &mut [usize], at: usize, value: usize) {
            m[at / 8] = value;
        }
        put(&mut memory, 0x3200, image + ER_CLOTH_MESH_P_VTABLE_RVA);
        for (at, offset) in [
            (0x748, 0x800),
            (0x150, 0xA00),
            (0x808, 0xA00),
            (0x828, 0xA00),
            (0xB30, 0xC00),
            (0xD20, 0x1200),
            (0xC40, 0xE00),
            (0xE30, 0x1000),
            (0x1028, 0x2400),
            (0x1030, 0x2408),
            (0x2400, 0x2420),
            (0x2420, 0x2500),
            (0x2520, 0x2600),
            (0x2600, 0x2700),
            (0x2540, 0x2900),
            (0x2900, 0x2A00),
            (0x2A18, 0x2E00),
            (0x2A20, 0x3000),
            (0x2C68, 0x2500),
            (0x2718, 0x3000),
            (0x2730, 0x3100),
            (0x3258, 0x3300),
            (0x3268, 0x3400),
            (0x3500, 0x3600),
        ] {
            put(&mut memory, at, heap + offset);
        }
        for (at, rva) in [
            (0x800, crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA),
            (0xA00, crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA),
            (0xC00, ER_CLOTH_MODEL_VTABLE_RVA),
            (0xE00, ER_CLOTH_INNER_VTABLE_RVA),
            (0x1200, ER_POSE_IMPORTER_VTABLE_RVA),
        ] {
            put(&mut memory, at, image + rva);
        }
        for at in [0x2528, 0x2548] {
            put(&mut memory, at, 1);
        }
        for at in [0x2E48, 0x2738, 0x3260, 0x3270] {
            put(&mut memory, at, 3);
        }
        put(&mut memory, 0x3508, 3 | (3 << 32));
        unsafe {
            ((heap + 0x2724) as *mut u8).write(16);
        }
        let unit = [
            1f32, 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
        ];
        unsafe {
            ((heap + 0x2790) as *mut [f32; 16]).write_unaligned(unit);
        }
        MODULE_BASE.store(image, Ordering::Release);
        HOOKS_READY.store(true, Ordering::Release);
        unit_state
            .target_cloth_pose_importer
            .store(heap + 0x2280, Ordering::Release);
        unit_state
            .target_pose_importer
            .store(heap + 0x2280, Ordering::Release);
        unit_state
            .target_scale_bits
            .store(0.5f32.to_bits(), Ordering::Release);
        refresh_owned_cloth_inputs(heap + 0x100, heap + 0x2280);
        let slot = (0..CLOTH_INSTANCE_SLOTS)
            .find(|&s| unit_state.cloth_instance_inputs[s].load(Ordering::Acquire) == heap + 0x1200)
            .unwrap();
        unit_state.cloth_instance_cores[slot].store(heap + 0x1000, Ordering::Release);
        unit_state.cloth_instance_pending_scale_bits[slot].store(0, Ordering::Release);
        let rows: Vec<Vec<f32>> = crate::test_fixtures::text("bd9004-mesh-reference-witness.txt")
            .lines()
            .filter(|s| !s.starts_with('#'))
            .map(|s| {
                s.split_whitespace()
                    .map(|x| f32::from_bits(u32::from_str_radix(x, 16).unwrap()))
                    .collect()
            })
            .collect();
        assert_eq!(rows.len(), 4);
        let mut registers: Registers = unsafe { std::mem::zeroed() };
        registers.rbx = (heap + 0x3200) as u64;
        registers.rdi = (heap + 0x2700) as u64;
        registers.r14 = (heap + 0x3500) as u64;
        fn transform(m: &[f32], p: &[f32]) -> [f32; 3] {
            std::array::from_fn(|j| m[j] * p[0] + m[4 + j] * p[1] + m[8 + j] * p[2] + m[12 + j])
        }
        let project = |frames: &[[f32; 16]]| -> [f32; 3] {
            let mut p = [0.; 3];
            for (i, row) in rows[1..].iter().enumerate() {
                assert_eq!(row.len(), 65);
                let q = transform(&frames[i], &transform(&row[1..17], &rows[0]));
                for j in 0..3 {
                    p[j] += q[j] * row[0];
                }
            }
            p
        };
        let baseline: Vec<[f32; 16]> = rows[1..]
            .iter()
            .map(|r| r[33..49].try_into().unwrap())
            .collect();
        let expected = project(&baseline);
        for (scale, start) in [(0.5f32, 17), (1., 33), (3., 49), (0.5, 17), (0.5, 17)] {
            unit_state
                .target_scale_bits
                .store(scale.to_bits(), Ordering::Release);
            unit_state.cloth_instance_applied_scale_bits[slot]
                .store(scale.to_bits(), Ordering::Release);
            let fresh: Vec<[f32; 16]> = rows[1..]
                .iter()
                .map(|r| r[start..start + 16].try_into().unwrap())
                .collect();
            for (i, m) in fresh.iter().enumerate() {
                unsafe {
                    ((heap + 0x3600 + i * 64) as *mut [f32; 16]).write_unaligned(*m);
                }
            }
            cloth_mesh_frame_hook(&mut registers);
            let actual: Vec<[f32; 16]> = (0..3)
                .map(|i| unsafe { ((heap + 0x3600 + i * 64) as *const [f32; 16]).read_unaligned() })
                .collect();
            let point = project(&actual);
            let error = (0..3)
                .map(|j| (point[j] - expected[j] * scale).powi(2))
                .sum::<f32>()
                .sqrt();
            assert!(
                error < 1e-4,
                "owned BD9004 mesh reference remains at wrong normal depth: scale={scale}, error={error}"
            );
            for (before, after) in fresh.iter().zip(&actual) {
                for i in (0..16).filter(|i| !(8..11).contains(i)) {
                    assert_eq!(before[i].to_bits(), after[i].to_bits());
                }
            }
            if std::env::var_os("ERPS_TEST_DUMP_MESH_FRAMES").is_some() {
                for (i, frame) in actual.iter().enumerate() {
                    let hex = frame
                        .iter()
                        .flat_map(|x| x.to_le_bytes())
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>();
                    println!("[ERPS-TEST-MESH-FRAME] scale={scale} index={i} raw={hex}");
                }
            }
        }
        // Every failed ownership/layout/scale predicate must leave the whole
        // native batch untouched, including when the target changes outfit.
        let native_half: Vec<[f32; 16]> = rows[1..]
            .iter()
            .map(|r| r[17..33].try_into().unwrap())
            .collect();
        let refill = || {
            for (i, m) in native_half.iter().enumerate() {
                unsafe {
                    ((heap + 0x3600 + i * 64) as *mut [f32; 16]).write_unaligned(*m);
                }
            }
        };
        for (at, value) in [
            (0x828, 0),
            (0x808, 0),
            (0xD20, 0),
            (0xE30, 0),
            (0x2600, 0),
            (0x2A20, heap + 0x3010),
            (0x2C68, 0),
            (0x3250, 1),
            (0x3250, 2),
            (0x3260, 8193),
            (0x3270, 2),
            (0x3508, 2 | (3 << 32)),
            (0x3508, 3 | (2 << 32)),
            (0x3500, heap + 0x3400),
            (0x3500, heap + 0x3000),
        ] {
            refill();
            let previous = memory[at / 8];
            put(&mut memory, at, value);
            let before = memory.clone();
            cloth_mesh_frame_hook(&mut registers);
            assert_eq!(
                memory, before,
                "rejected descriptor wrote memory: offset={at:X}"
            );
            put(&mut memory, at, previous);
        }
        for pending in [IN_PROGRESS_CLOTH_SCALE_BITS, 3f32.to_bits()] {
            refill();
            unit_state.cloth_instance_pending_scale_bits[slot].store(pending, Ordering::Release);
            let before = memory.clone();
            cloth_mesh_frame_hook(&mut registers);
            assert_eq!(memory, before);
        }
        unit_state.cloth_instance_pending_scale_bits[slot].store(0, Ordering::Release);
        refill();
        let before_queries = crate::memory_query::query_count();
        cloth_mesh_frame_hook(&mut registers);
        let queries = crate::memory_query::query_count() - before_queries;
        assert!(
            queries <= 8,
            "fresh operation query budget regressed: {queries}"
        );
        let timer = std::time::Instant::now();
        for _ in 0..2000 {
            refill();
            cloth_mesh_frame_hook(std::hint::black_box(&mut registers));
        }
        println!(
            "[ERPS-TEST-MESH-PERF] frames=3 iterations=2000 mean_us={:.3} queries={queries}",
            timer.elapsed().as_micros() as f64 / 2000.
        );
        // BD9004 cloak/cape/inner skirt use mode1: the original native cross
        // column scales quadratically, unlike the mode0 unit column above.
        let area_rows: Vec<Vec<f32>> = crate::test_fixtures::text("bd9004-area-frame-witness.txt")
            .lines()
            .filter(|s| !s.starts_with('#'))
            .map(|s| {
                s.split_whitespace()
                    .map(|x| f32::from_bits(u32::from_str_radix(x, 16).unwrap()))
                    .collect()
            })
            .collect();
        let area_project = |frames: &[[f32; 16]]| -> [f32; 3] {
            let mut point = [0.; 3];
            for (i, row) in area_rows[1..].iter().enumerate() {
                let q = transform(&frames[i], &transform(&row[1..17], &area_rows[0]));
                for j in 0..3 {
                    point[j] += q[j] * row[0];
                }
            }
            point
        };
        let baseline_area: Vec<[f32; 16]> = area_rows[1..]
            .iter()
            .map(|r| r[33..49].try_into().unwrap())
            .collect();
        let expected_area = area_project(&baseline_area);
        put(&mut memory, 0x3250, 1);
        for (scale, start) in [(0.5f32, 17), (1., 33), (3., 49), (0.5, 17)] {
            unit_state
                .target_scale_bits
                .store(scale.to_bits(), Ordering::Release);
            unit_state.cloth_instance_applied_scale_bits[slot]
                .store(scale.to_bits(), Ordering::Release);
            let fresh: Vec<[f32; 16]> = area_rows[1..]
                .iter()
                .map(|r| r[start..start + 16].try_into().unwrap())
                .collect();
            for (i, frame) in fresh.iter().enumerate() {
                unsafe {
                    ((heap + 0x3600 + i * 64) as *mut [f32; 16]).write_unaligned(*frame);
                }
            }
            cloth_mesh_frame_hook(&mut registers);
            let actual: Vec<[f32; 16]> = (0..3)
                .map(|i| unsafe { ((heap + 0x3600 + i * 64) as *const [f32; 16]).read_unaligned() })
                .collect();
            let point = area_project(&actual);
            let error = (0..3)
                .map(|j| (point[j] - expected_area[j] * scale).powi(2))
                .sum::<f32>()
                .sqrt();
            assert!(
                error < 1e-4,
                "BD9004 area-mode display depth is not proportional: scale={scale}, error={error}"
            );
            for (before, after) in fresh.iter().zip(&actual) {
                for i in (0..16).filter(|i| !(8..11).contains(i)) {
                    assert_eq!(before[i].to_bits(), after[i].to_bits());
                }
            }
        }
        put(&mut memory, 0x3250, 0);
        // The real PN route must bracket the native call, not shrink reference
        // normals together with the positions. Three padded output vertices.
        put(&mut memory, 0x3200, image + ER_CLOTH_MESH_PN_VTABLE_RVA);
        for (at, offset) in [
            (0x32C8, 0x3800),
            (0x32F8, 0x3900),
            (0x2608, 0x3A00),
            (0x3A18, 0x3B80),
            (0x3A40, 0x3C00),
        ] {
            put(&mut memory, at, heap + offset);
        }
        for at in [0x32D0, 0x3300, 0x3900] {
            put(&mut memory, at, 1);
        }
        put(&mut memory, 0x3308, 2 << 16);
        put(&mut memory, 0x2528, 2);
        unsafe {
            ((heap + 0x3A24) as *mut u8).write(16);
            ((heap + 0x3A4C) as *mut u8).write(16);
            for i in 0..16 {
                ((heap + 0x3800 + i * 2) as *mut u16).write_unaligned(i.min(2) as u16);
            }
            for i in 0..3 {
                ((heap + 0x3C0C + i * 16) as *mut u32).write_unaligned(0x7FC00011);
            }
        }
        unsafe extern "C" fn pn_native(
            op: usize,
            _local: usize,
            input: usize,
            output: usize,
        ) -> usize {
            let heap = op - 0x3200;
            let mut r: Registers = unsafe { std::mem::zeroed() };
            r.rbx = op as u64;
            r.rdi = input as u64;
            r.r14 = (heap + 0x3500) as u64;
            r.rbp = (output + 0xD0) as u64;
            cloth_mesh_frame_hook(&mut r);
            // PN kernel transforms normals linearly with this palette too.
            for i in 0..3 {
                for j in 0..3 {
                    unsafe {
                        let x = ((heap + 0x3620 + i * 64 + j * 4) as *const f32).read_unaligned();
                        ((heap + 0x3C00 + i * 16 + j * 4) as *mut f32).write_unaligned(x);
                    }
                }
            }
            0x1234
        }
        let mut pn_registers: Registers = unsafe { std::mem::zeroed() };
        pn_registers.rcx = (heap + 0x3200) as u64;
        pn_registers.r8 = (heap + 0x2700) as u64;
        pn_registers.r9 = (heap + 0x3A00) as u64;
        for (flags, scale, start) in [
            (0u8, 0.5f32, 17),
            (0, 1., 33),
            (0, 3., 49),
            (1, 0.5, 17),
            (1, 1., 33),
            (1, 3., 49),
            (1, 0.5, 17),
        ] {
            // Real BD9004 object58 uses aligned float4 slots, flags=1 for
            // both positions and normals. The old synthetic test only used 0.
            unsafe {
                ((heap + 0x3A28) as *mut u8).write(flags);
                ((heap + 0x3A50) as *mut u8).write(flags);
            }
            unit_state
                .target_scale_bits
                .store(scale.to_bits(), Ordering::Release);
            unit_state.cloth_instance_applied_scale_bits[slot]
                .store(scale.to_bits(), Ordering::Release);
            let fresh: Vec<[f32; 16]> = rows[1..]
                .iter()
                .map(|r| r[start..start + 16].try_into().unwrap())
                .collect();
            for (i, m) in fresh.iter().enumerate() {
                unsafe {
                    ((heap + 0x3600 + i * 64) as *mut [f32; 16]).write_unaligned(*m);
                }
            }
            assert_eq!(
                cloth_mesh_pn_hook(&mut pn_registers, pn_native as *const () as usize),
                0x1234
            );
            let actual: Vec<[f32; 16]> = (0..3)
                .map(|i| unsafe { ((heap + 0x3600 + i * 64) as *const [f32; 16]).read_unaligned() })
                .collect();
            let point = project(&actual);
            let error = (0..3)
                .map(|j| (point[j] - expected[j] * scale).powi(2))
                .sum::<f32>()
                .sqrt();
            assert!(
                error < 1e-4,
                "PN reference correction was bypassed: flags={flags}, scale={scale}, error={error}"
            );
            for (i, frame) in fresh.iter().enumerate() {
                for j in 0..3 {
                    let n = unsafe {
                        ((heap + 0x3C00 + i * 16 + j * 4) as *const f32).read_unaligned()
                    };
                    assert!(
                        (n - frame[8 + j]).abs() < 1e-6,
                        "PN normal length changed with body scale"
                    );
                }
                assert_eq!(
                    unsafe { ((heap + 0x3C0C + i * 16) as *const u32).read_unaligned() },
                    0x7FC00011
                );
            }
            assert!(MESH_PN_CALL.get().is_none(), "invocation state leaked");
        }
        put(&mut memory, 0x3250, 1);
        refill();
        let before_normals = unit_state.cloth_mesh_normal_rows.load(Ordering::Relaxed);
        cloth_mesh_pn_hook(&mut pn_registers, pn_native as *const () as usize);
        for (i, frame) in native_half.iter().enumerate() {
            assert_eq!(
                unsafe { ((heap + 0x3600 + i * 64) as *const [f32; 16]).read_unaligned() },
                *frame
            );
        }
        assert_eq!(
            unit_state.cloth_mesh_normal_rows.load(Ordering::Relaxed),
            before_normals,
            "unverified PN area-mode must not be modified"
        );
        put(&mut memory, 0x3250, 0);
        for at in [0x3308, 0x32D0, 0x3300, 0x3900, 0x3800] {
            let old = memory[at / 8];
            put(&mut memory, at, usize::MAX);
            assert!(
                crate::memory_query::scoped(|| mesh_normal_span(heap + 0x3200, heap + 0x3A00))
                    .is_none()
            );
            put(&mut memory, at, old);
        }
        let pn_timer = std::time::Instant::now();
        for _ in 0..2000 {
            refill();
            cloth_mesh_pn_hook(&mut pn_registers, pn_native as *const () as usize);
        }
        println!(
            "[ERPS-TEST-MESH-PN-PERF] normals=3 iterations=2000 mean_us={:.3}",
            pn_timer.elapsed().as_micros() as f64 / 2000.
        );
        clear_target();
        *unit_state.target_cloth_scope.write().unwrap() = ClothOwnerScope::default();
        HOOKS_READY.store(false, Ordering::Release);
        MODULE_BASE.store(0, Ordering::Release);
    }

    #[test]
    #[ignore = "requires private replay data; set ER_CHARACTER_SCALE_FIXTURES"]
    fn bd9004_aligned_pn_real_coverage_and_rejections() {
        let mut storage = vec![0u128; 0x5000 / 16];
        let heap = storage.as_mut_ptr() as usize;
        let rows: Vec<Vec<usize>> = crate::test_fixtures::text("bd9004-pn-reference-coverage.txt")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .map(|line| {
                line.split_whitespace()
                    .map(|x| x.parse().unwrap())
                    .collect()
            })
            .collect();
        assert_eq!(rows[0], [0, 288, 0]);
        assert_eq!(rows[1], vec![1; 19]);
        assert_eq!(rows.len(), 21);
        let (op, output, blocks, controls, positions, normals) = (
            heap,
            heap + 0x200,
            heap + 0x400,
            heap + 0x1200,
            heap + 0x2000,
            heap + 0x3400,
        );
        let put64 = |at: usize, value: usize| unsafe { (at as *mut usize).write_unaligned(value) };
        let put8 = |at: usize, value: u8| unsafe { (at as *mut u8).write(value) };
        put64(op + 0xC8, blocks);
        put64(op + 0xD0, 19);
        put64(op + 0xF8, controls);
        put64(op + 0x100, 19);
        put64(op + 0x108, 288 << 16);
        put64(output + 0x18, positions);
        put64(output + 0x40, normals);
        for at in [output + 0x24, output + 0x4C] {
            put8(at, 16);
        }
        for at in [output + 0x28, output + 0x50] {
            put8(at, 1);
        }
        for (b, vertices) in rows[2..].iter().enumerate() {
            assert_eq!(vertices.len(), 16);
            put8(controls + b, rows[1][b] as u8);
            for (i, vertex) in vertices.iter().enumerate() {
                unsafe {
                    ((blocks + b * 176 + i * 2) as *mut u16).write_unaligned(*vertex as u16);
                }
            }
        }
        let parse = || crate::memory_query::scoped(|| mesh_normal_span(op, output));
        let expected = Some(MeshNormalSpan {
            data: normals,
            count: 289,
            stride: 16,
        });
        assert_eq!(parse(), expected);
        let before_queries = crate::memory_query::query_count();
        assert_eq!(parse(), expected);
        let queries = crate::memory_query::query_count() - before_queries;
        assert!(queries <= 4, "metadata permission query budget: {queries}");
        let timer = std::time::Instant::now();
        for _ in 0..1000 {
            assert_eq!(parse(), expected);
        }
        println!(
            "[ERPS-TEST-PN-COVERAGE] blocks=19 vertices=289 mean_us={:.3} queries={queries}",
            timer.elapsed().as_micros() as f64 / 1000.
        );
        for (at, value) in [
            (output + 0x24, 12),
            (output + 0x4C, 12),
            (output + 0x28, 0),
            (output + 0x50, 0),
            (op + 0x10C, 1),
            (controls, 4),
        ] {
            let old = unsafe { (at as *const u8).read() };
            put8(at, value);
            assert!(parse().is_none(), "invalid metadata accepted: {at:X}");
            put8(at, old);
        }
        for (at, value) in [
            (output + 0x18, normals),
            (output + 0x18, positions + 4),
            (output + 0x40, normals + 4),
        ] {
            let old = unsafe { (at as *const usize).read_unaligned() };
            put64(at, value);
            assert!(parse().is_none());
            put64(at, old);
        }
        // Native pads the last SIMD block with the final vertex, not an
        // un-written hole. Reject a descriptor that omits that last vertex.
        for i in 0..16 {
            unsafe {
                ((blocks + 18 * 176 + i * 2) as *mut u16).write_unaligned(287);
            }
        }
        assert!(parse().is_none());
    }

    #[test]
    fn render_hook_owns_fresh_ranges_and_rejects_lifecycle_changes() {
        let unit_state = current_unit_state();
        let _lock = MODULE_TEST_LOCK.lock().unwrap();
        clear_target();
        let image = 0x140000000;
        let mut memory = vec![0usize; 0x2300 / 8];
        let heap = memory.as_mut_ptr() as usize;
        fn put(m: &mut [usize], at: usize, value: usize) {
            m[at / 8] = value;
        }
        for (at, offset) in [
            (0x748, 0x800),
            (0x150, 0xA00),
            (0x808, 0xA00),
            (0x828, 0xA00),
            (0xB30, 0xC00),
            (0xD20, 0x1200),
            (0xC40, 0xE00),
            (0xE30, 0x1000),
            (0x1248, 0x1400),
            (0x1250, 0x1900),
            (0x1260, 0x1A00),
            (0x1270, 0x1B00),
            (0x1570, 0x1600),
            (0x1600, 0x1200),
            (0x1588, 0x1700),
            (0x1708, 0x1800),
            (0x1018, 0x1C00),
            (0x1040, 0x1D00),
            (0x1D00, 0x1E00),
            (0x1D08, 0x1F00),
            (0x1D18, 0x2100),
            (0x1F18, 0x2000),
        ] {
            put(&mut memory, at, heap + offset);
        }
        for (at, rva) in [
            (0x800, crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA),
            (0xA00, crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA),
            (0xC00, ER_CLOTH_MODEL_VTABLE_RVA),
            (0xE00, ER_CLOTH_INNER_VTABLE_RVA),
            (0x1200, ER_POSE_IMPORTER_VTABLE_RVA),
            (0x1500, ER_ANIM_SKELETON_VTABLE_RVA),
        ] {
            put(&mut memory, at, image + rva);
        }
        for at in [0x1258, 0x1268, 0x1278, 0x1438, 0x1D28, 0x1E20, 0x1F10] {
            put(&mut memory, at, 3);
        }
        for at in [0x1048, 0x1D20, 0x1C68] {
            put(&mut memory, at, 1);
        }
        put(&mut memory, 0x1700, 4 << 32);
        put(
            &mut memory,
            0x1800,
            u64::from_le_bytes([255, 255, 0, 0, 1, 0, 2, 0]) as usize,
        );
        put(
            &mut memory,
            0x2000,
            u64::from_le_bytes([0, 0, 1, 0, 2, 0, 0, 0]) as usize,
        );
        put(&mut memory, 0x2100, 0b110);
        MODULE_BASE.store(image, Ordering::Release);
        HOOKS_READY.store(true, Ordering::Release);
        unit_state
            .target_cloth_pose_importer
            .store(heap + 0x2280, Ordering::Release);
        unit_state
            .target_pose_importer
            .store(heap + 0x2280, Ordering::Release);
        unit_state
            .target_scale_bits
            .store(0.5f32.to_bits(), Ordering::Release);
        refresh_owned_cloth_inputs(heap + 0x100, heap + 0x2280);
        let slot = (0..CLOTH_INSTANCE_SLOTS)
            .find(|&i| unit_state.cloth_instance_inputs[i].load(Ordering::Acquire) == heap + 0x1200)
            .unwrap();
        unit_state.cloth_instance_cores[slot].store(heap + 0x1000, Ordering::Release);
        unit_state.cloth_instance_applied_scale_bits[slot]
            .store(0.5f32.to_bits(), Ordering::Release);
        unit_state.cloth_instance_pending_scale_bits[slot].store(0, Ordering::Release);
        let this = heap + 0x1500;
        let before = render_scope(this, 0.5).expect("fixture must reach the real ownership gate");
        const RAW: [f32; 12] = [2., 0., 0., 0.4, 0., 2., 0., 0.8, 0., 0., 2., 0.2];
        unsafe extern "C" fn native(_this: usize, output: usize, count: u32, start: u32) -> usize {
            let n = count.min(4u32.saturating_sub(start));
            for i in 0..n {
                unsafe { (output as *mut [f32; 12]).add(i as usize).write(RAW) };
            }
            n as usize
        }
        let stack = [image + ER_CLOTH_RENDER_RETURN_RVA];
        let mut output = [[0.; 12]; 4];
        let mut registers: Registers = unsafe { std::mem::zeroed() };
        registers.rcx = this as u64;
        registers.rdx = output.as_mut_ptr() as u64;
        registers.rsp = stack.as_ptr() as u64;
        registers.r8 = 4;
        let saved_memory = memory.clone();
        for _ in 0..3 {
            assert_eq!(
                cloth_render_range_hook(&mut registers, native as *const () as usize),
                4
            );
            assert_eq!(output[0], RAW); // negative fallback
            assert_eq!(output[1], RAW); // ordinary source, not selected by native mask
            assert_eq!(output[2][0], 1.);
            assert_eq!(output[2][7], 1.6);
            assert_eq!(output[3], output[2]);
            assert_eq!(memory, saved_memory); // canonical state untouched
        }
        registers.r9 = 2;
        registers.r8 = 1;
        assert_eq!(
            cloth_render_range_hook(&mut registers, native as *const () as usize),
            1
        );
        assert_eq!(output[0][7], 1.6);
        registers.r9 = 0;
        registers.r8 = 4;
        for scale in [1.0f32, 3., 0.5] {
            unit_state
                .target_scale_bits
                .store(scale.to_bits(), Ordering::Release);
            unit_state.cloth_instance_applied_scale_bits[slot]
                .store(scale.to_bits(), Ordering::Release);
            cloth_render_range_hook(&mut registers, native as *const () as usize);
            assert_eq!(output[2][0], RAW[0] * scale);
            assert_eq!(output[2][7], RAW[7] / scale);
            assert_eq!(output[1], RAW);
        }
        // Changed call site, pending transition, primary input, and revoked
        // equipment all pass through even if the memory/vtable still exists.
        let other_stack = [image + ER_CLOTH_RENDER_RETURN_RVA + 1];
        registers.rsp = other_stack.as_ptr() as u64;
        cloth_render_range_hook(&mut registers, native as *const () as usize);
        assert_eq!(output, [RAW; 4]);
        registers.rsp = stack.as_ptr() as u64;
        unit_state.cloth_instance_pending_scale_bits[slot].store(3f32.to_bits(), Ordering::Release);
        assert!(render_scope(this, 0.5).is_none());
        unit_state.cloth_instance_pending_scale_bits[slot].store(0, Ordering::Release);
        unit_state
            .target_pose_importer
            .store(heap + 0x1200, Ordering::Release);
        assert!(render_scope(this, 0.5).is_none());
        unit_state
            .target_pose_importer
            .store(heap + 0x2280, Ordering::Release);
        assert_eq!(
            reconcile_render_output(this, heap + 0x1A00, 3, 0, 0.5, before),
            None
        );
        put(&mut memory, 0x828, 0); // equipment replaced between original and post-hook
        assert_eq!(
            reconcile_render_output(this, output.as_mut_ptr() as usize, 4, 0, 0.5, before),
            None
        );
        put(&mut memory, 0x828, heap + 0xA00);
        unit_state
            .cloth_topology_generation
            .fetch_add(1, Ordering::AcqRel);
        assert_eq!(
            reconcile_render_output(this, output.as_mut_ptr() as usize, 4, 0, 0.5, before),
            None
        );
        // Runtime-adapter microbenchmark uses real VirtualQuery checks; it is
        // not a game-frame or GPU performance acceptance measurement.
        let clock = std::time::Instant::now();
        let queries_before = crate::memory_query::query_count();
        for _ in 0..2000 {
            std::hint::black_box(cloth_render_range_hook(
                &mut registers,
                native as *const () as usize,
            ));
        }
        println!(
            "render adapter synthetic 4-row mean_us={:.2} queries_per_call={:.2}",
            clock.elapsed().as_secs_f64() * 1e6 / 2000.,
            (crate::memory_query::query_count() - queries_before) as f64 / 2000.
        );
        let query_delta = crate::memory_query::query_count() - queries_before;
        clear_target();
        HOOKS_READY.store(false, Ordering::Release);
        MODULE_BASE.store(0, Ordering::Release);
        for i in 0..CLOTH_INSTANCE_SLOTS {
            unit_state.cloth_instance_owners[i].store(0, Ordering::Release);
            unit_state.cloth_instance_inputs[i].store(0, Ordering::Release);
            unit_state.cloth_instance_was_equipment[i].store(false, Ordering::Release);
        }
        assert!(
            query_delta <= 12 * 2000,
            "render range repeats OS protection queries: {query_delta} across 2000 calls"
        );
    }

    #[test]
    fn scales_pose_translation_xyz_and_preserves_w() {
        assert_eq!(
            scaled_translation([2.0, -4.0, 8.0, 1.0], 0.5),
            Some([1.0, -2.0, 4.0, 1.0])
        );
    }

    #[test]
    fn matches_nr_pose_helper_translation_and_local_scale_writes() {
        assert_eq!(
            scaled_pose_vectors([2.0, -4.0, 8.0, 1.0], [1.0, 1.5, 2.0, 0.0], 0.5,),
            Some(([1.0, -2.0, 4.0, 1.0], [0.5, 0.75, 1.0, 0.0]))
        );
    }

    #[test]
    fn pose_vector_ratios_restore_translation_and_local_scale() {
        let baseline = ([2.0, 4.0, 6.0, 1.0], [1.0, 1.0, 1.0, 0.0]);
        let small = scaled_pose_vectors(baseline.0, baseline.1, 0.5).unwrap();
        let large = scaled_pose_vectors(small.0, small.1, 6.0).unwrap();
        let restored = scaled_pose_vectors(large.0, large.1, 1.0 / 3.0).unwrap();

        assert_eq!(small, ([1.0, 2.0, 3.0, 1.0], [0.5, 0.5, 0.5, 0.0]));
        assert_eq!(large, ([6.0, 12.0, 18.0, 1.0], [3.0, 3.0, 3.0, 0.0]));
        assert_eq!(restored, baseline);
    }

    #[test]
    fn rejects_neutral_or_non_finite_pose_scale() {
        assert_eq!(scaled_translation([1.0, 2.0, 3.0, 1.0], 1.0), None);
        assert_eq!(scaled_translation([f32::NAN, 2.0, 3.0, 1.0], 0.5), None);
    }

    #[test]
    fn post_multiplies_row_vector_matrix_by_uniform_scale() {
        let matrix: [f32; 16] = [
            1.0, 0.0, 0.0, 0.0, // basis X
            0.0, 1.0, 0.0, 0.0, // basis Y
            0.0, 0.0, 1.0, 0.0, // basis Z
            10.0, 20.0, 30.0, 1.0, // translation
        ];
        assert_eq!(
            scaled_row_matrix(matrix, 0.5),
            Some([
                0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 5.0, 10.0, 15.0, 1.0,
            ])
        );
    }

    #[test]
    fn summarizes_cloth_matrix_basis_without_changing_translation() {
        let matrix: [f32; 16] = [
            3.0, 4.0, 0.0, 0.0, // basis X length 5
            0.0, 0.0, 2.0, 0.0, // basis Y length 2
            0.0, 6.0, 8.0, 0.0, // basis Z length 10
            10.0, 20.0, 30.0, 1.0, // translation
        ];
        let summary = summarize_cloth_matrix(matrix.as_ptr() as usize);

        assert!(summary.readable);
        assert_eq!(
            [summary.basis_x, summary.basis_y, summary.basis_z],
            [5.0, 2.0, 10.0]
        );
        assert_eq!(
            [
                summary.translation_x,
                summary.translation_y,
                summary.translation_z,
            ],
            [10.0, 20.0, 30.0]
        );
    }

    #[test]
    fn stages_cloth_transition_before_the_native_main_transform_catches_up() {
        assert!(should_stage_cloth_transition(
            1.0f32.to_bits(),
            NO_PENDING_CLOTH_SCALE_BITS,
            0.5f32.to_bits(),
        ));
        assert!(!should_stage_cloth_transition(
            0.5f32.to_bits(),
            NO_PENDING_CLOTH_SCALE_BITS,
            0.5f32.to_bits(),
        ));
        assert!(!should_stage_cloth_transition(
            1.0f32.to_bits(),
            0.5f32.to_bits(),
            0.5f32.to_bits(),
        ));
    }

    #[test]
    fn selectively_recognizes_unit_and_requested_cloth_entry_bases() {
        let unit = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 12.0, 24.0, 36.0, 1.0,
        ];
        let half =
            absolute_uniform_scale_matrix_about_pivot(unit, 0.5, [10.0, 20.0, 30.0]).unwrap();

        assert!(cloth_matrix_array_basis_matches_scale(&unit, 1.0));
        assert!(!cloth_matrix_array_basis_matches_scale(&unit, 0.5));
        assert!(cloth_matrix_array_basis_matches_scale(&half, 0.5));
        assert!(!cloth_matrix_array_basis_matches_scale(&half, 1.0));
    }

    #[test]
    fn classifies_secondary_copy_matrix_against_requested_and_unit_scale() {
        let unit = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 12.0, 24.0, 36.0, 1.0,
        ];
        let half =
            absolute_uniform_scale_matrix_about_pivot(unit, 0.5, [10.0, 20.0, 30.0]).unwrap();
        let malformed = [f32::NAN; 16];

        assert_eq!(
            classify_cloth_matrix_scale(Some(&half), 0.5),
            ClothMatrixScaleClass::Requested
        );
        assert_eq!(
            classify_cloth_matrix_scale(Some(&unit), 0.5),
            ClothMatrixScaleClass::Unit
        );
        assert_eq!(
            classify_cloth_matrix_scale(Some(&malformed), 0.5),
            ClothMatrixScaleClass::Other
        );
        assert_eq!(
            classify_cloth_matrix_scale(None, 0.5),
            ClothMatrixScaleClass::Other
        );
    }

    #[test]
    fn absolutely_scales_cloth_entry_around_player_root_without_compounding() {
        let unit = [
            0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 12.0, 24.0, 36.0, 1.0,
        ];
        let scaled =
            absolute_uniform_scale_matrix_about_pivot(unit, 0.5, [10.0, 20.0, 30.0]).unwrap();

        assert_eq!(
            scaled,
            [
                0.0, 0.5, 0.0, 0.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 11.0, 22.0, 33.0, 1.0,
            ]
        );
        assert!(cloth_matrix_array_basis_matches_scale(&scaled, 0.5));
        assert!(!cloth_matrix_array_basis_matches_scale(&scaled, 1.0));
    }

    #[test]
    fn secondary_reference_submit_scales_unit_matrix_about_player_root() {
        let unit = [
            0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 12.0, 24.0, 36.0, 1.0,
        ];

        let scaled = secondary_reference_matrix_for_scale(unit, 0.5, [10.0, 20.0, 30.0]).unwrap();

        assert_eq!(
            scaled,
            [
                0.0, 0.5, 0.0, 0.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 11.0, 22.0, 33.0, 1.0,
            ]
        );
    }

    #[test]
    fn secondary_reference_submit_restores_from_scaled_source_without_compounding() {
        let half = [
            0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 11.0, 22.0, 33.0, 1.0,
        ];

        let restored = secondary_reference_matrix_for_scale(half, 1.0, [10.0, 20.0, 30.0]).unwrap();

        assert_eq!(
            restored,
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 12.0, 24.0, 36.0, 1.0,
            ]
        );
        assert_eq!(
            secondary_reference_matrix_for_scale(restored, 1.0, [10.0, 20.0, 30.0]),
            Some(restored)
        );
    }

    #[test]
    fn secondary_reference_submit_rejects_nonuniform_or_nonfinite_source() {
        let nonuniform = [
            1.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 12.0, 24.0, 36.0, 1.0,
        ];
        assert_eq!(
            secondary_reference_matrix_for_scale(nonuniform, 0.5, [10.0, 20.0, 30.0]),
            None
        );

        let mut nonfinite = nonuniform;
        nonfinite[0] = f32::NAN;
        assert_eq!(
            secondary_reference_matrix_for_scale(nonfinite, 0.5, [10.0, 20.0, 30.0]),
            None
        );
    }

    #[test]
    fn secondary_reference_uses_the_instances_committed_scale() {
        assert_eq!(committed_cloth_scale(0.5f32.to_bits()), Some(0.5));
        assert_eq!(committed_cloth_scale(1.0f32.to_bits()), Some(1.0));
        assert_eq!(committed_cloth_scale(NO_PENDING_CLOTH_SCALE_BITS), None);
        assert_eq!(committed_cloth_scale(IN_PROGRESS_CLOTH_SCALE_BITS), None);
    }

    #[test]
    fn summarizes_the_solver_rebuild_qs_transform_input_layout() {
        let mut transforms = Box::new([[0.0f32; 16]; 2]);
        transforms[0][0..3].copy_from_slice(&[1.0, 2.0, 3.0]);
        transforms[0][8..11].copy_from_slice(&[0.5, 0.5, 0.5]);
        transforms[1][0..3].copy_from_slice(&[4.0, 5.0, 6.0]);
        transforms[1][8..11].copy_from_slice(&[0.5, 0.5, 0.5]);

        let mut owner = Box::new([0u8; 0x28]);
        unsafe {
            (owner.as_mut_ptr().add(0x18) as *mut usize)
                .write_unaligned(transforms.as_ptr() as usize);
            (owner.as_mut_ptr().add(0x20) as *mut i32).write_unaligned(2);
        }
        let mut entries = Box::new([0u8; 0x38]);
        unsafe {
            (entries.as_mut_ptr() as *mut usize).write_unaligned(owner.as_ptr() as usize);
        }
        let mut core = Box::new([0u8; 0x50]);
        unsafe {
            (core.as_mut_ptr().add(0x40) as *mut usize).write_unaligned(entries.as_ptr() as usize);
            (core.as_mut_ptr().add(0x48) as *mut i32).write_unaligned(1);
        }

        let snapshot = cloth_solver_input_snapshot(core.as_ptr() as usize);
        assert!(snapshot.readable);
        assert_eq!(snapshot.entry_count, 1);
        assert_eq!(snapshot.transform_count, 2);
        assert_eq!(
            [
                snapshot.scale_min_x,
                snapshot.scale_min_y,
                snapshot.scale_min_z,
                snapshot.scale_max_x,
                snapshot.scale_max_y,
                snapshot.scale_max_z,
            ],
            [0.5; 6],
        );
        assert_eq!(snapshot.translation_bounds.min_x, 1.0);
        assert_eq!(snapshot.translation_bounds.max_z, 6.0);
    }

    #[test]
    fn solver_translation_bracket_never_touches_local_scale_and_restores_exact_bits() {
        let mut transforms = [
            [1.25, -2.5, 4.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0],
            [
                8.0, 16.0, -32.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0, 0.0,
            ],
        ];
        let original = transforms;
        let mut backups = Vec::new();

        assert_eq!(
            scale_solver_source_transforms(&mut transforms, &[1], 0.5, &mut backups),
            Some(1)
        );
        assert_eq!(transforms[0], original[0]);
        assert_eq!(&transforms[1][..4], &[4.0, 8.0, -16.0, 1.0]);
        assert_eq!(&transforms[1][4..8], &original[1][4..8]);
        assert_eq!(&transforms[1][8..], &original[1][8..]);

        restore_solver_source_transforms(&mut transforms, &backups);
        for (actual, expected) in transforms.iter().zip(original.iter()) {
            assert_eq!(
                actual.map(f32::to_bits),
                expected.map(f32::to_bits),
                "temporary solver input mutation must be bit-exactly reversible"
            );
        }
    }

    #[test]
    fn solver_context_bracket_deduplicates_indices_and_rejects_bad_input() {
        let mut transforms = [
            [
                2.0,
                2.0,
                2.0,
                2.0,
                0.0,
                0.0,
                0.0,
                1.0,
                1.0 / 3.0,
                1.0 / 3.0,
                1.0 / 3.0,
                0.0,
            ],
            [
                2.0,
                2.0,
                2.0,
                2.0,
                0.0,
                0.0,
                0.0,
                1.0,
                1.0 / 3.0,
                1.0 / 3.0,
                1.0 / 3.0,
                0.0,
            ],
        ];
        let mut backups = Vec::new();

        assert_eq!(
            scale_solver_source_transforms(&mut transforms, &[0, 0, 1], 3.0, &mut backups),
            Some(2)
        );
        assert_eq!(backups.len(), 2);
        restore_solver_source_transforms(&mut transforms, &backups);

        transforms[1][0] = f32::NAN;
        assert_eq!(
            scale_solver_source_transforms(&mut transforms, &[1], 0.5, &mut backups),
            None
        );
        assert!(backups.is_empty());
        assert_eq!(
            scale_solver_source_transforms(&mut transforms, &[2], 0.5, &mut backups),
            None
        );
    }

    #[test]
    fn solver_translation_bracket_ignores_unknown_nonuniform_and_nan_local_scale() {
        let mut transforms = [
            [2.0, 4.0, 6.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 2.0, 2.0, 0.0],
            [
                3.0,
                6.0,
                9.0,
                1.0,
                0.0,
                0.0,
                0.0,
                1.0,
                1.5,
                f32::NAN,
                0.75,
                f32::INFINITY,
            ],
        ];
        let original = transforms;
        let mut backups = Vec::new();
        let mut local_scale_backups = Vec::new();

        assert_eq!(
            scale_solver_source_transforms(&mut transforms, &[0, 1], 0.5, &mut backups),
            Some(2)
        );
        assert_eq!(&transforms[0][..4], &[1.0, 2.0, 3.0, 1.0]);
        assert_eq!(&transforms[1][..4], &[1.5, 3.0, 4.5, 1.0]);
        assert_eq!(
            transforms[0][8..]
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            original[0][8..]
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            transforms[1][8..]
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>(),
            original[1][8..]
                .iter()
                .map(|value| value.to_bits())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            scale_solver_source_local_scales(
                &mut transforms,
                &[0, 1],
                0.5,
                &mut local_scale_backups,
            ),
            None
        );
        assert!(local_scale_backups.is_empty());
        assert_eq!(&transforms[0][..4], &[1.0, 2.0, 3.0, 1.0]);
        assert_eq!(&transforms[1][..4], &[1.5, 3.0, 4.5, 1.0]);

        restore_solver_source_transforms(&mut transforms, &backups);
        for (actual, expected) in transforms.iter().zip(original.iter()) {
            assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
        }
    }

    #[test]
    fn independent_solver_local_scale_bracket_scales_every_xyz_and_restores_exact_bits() {
        let mut transforms = [
            [2.0, 4.0, 6.0, 1.0, 0.0, 0.0, 0.0, 1.0, 2.0, 1.0, 0.75, 0.0],
            [3.0, 6.0, 9.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.5, 0.5, 3.0, 7.0],
        ];
        let original = transforms;
        let mut backups = Vec::new();

        assert_eq!(
            scale_solver_source_local_scales(&mut transforms, &[0, 0, 1], 0.5, &mut backups),
            Some(2)
        );
        assert_eq!(&transforms[0][..8], &original[0][..8]);
        assert_eq!(&transforms[1][..8], &original[1][..8]);
        assert_eq!(&transforms[0][8..], &[1.0, 0.5, 0.375, 0.0]);
        assert_eq!(&transforms[1][8..], &[0.75, 0.25, 1.5, 7.0]);

        restore_solver_source_local_scales(&mut transforms, &backups);
        for (actual, expected) in transforms.iter().zip(original.iter()) {
            assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
        }
    }

    #[test]
    fn solver_local_scale_policy_requires_owned_independent_input_identity() {
        assert!(should_scale_independent_solver_source(true, 0x2000, 0x1000));
        assert!(!should_scale_independent_solver_source(
            false, 0x2000, 0x1000
        ));
        assert!(!should_scale_independent_solver_source(
            true, 0x1000, 0x1000
        ));
        assert!(!should_scale_independent_solver_source(true, 0, 0x1000));
        assert!(!should_scale_independent_solver_source(true, 0x2000, 0));
    }

    #[test]
    fn solver_source_selection_keeps_direct_and_lazy_resolved_mappings() {
        check_solver_source_selection(2, &[vec![0, 1]], true);
    }

    #[test]
    fn solver_source_selection_covers_large_equipment_maps() {
        // Equipment can expose the entire skeleton in every cloth entry,
        // including unmapped rows. BD_M_5290 exposes 850 rows per entry.
        for count in [
            512,
            513,
            850,
            4097,
            crate::cloth_render_scale::MAX_TRANSFORMS,
        ] {
            let mut mapping = vec![-1; count];
            mapping[0] = 0;
            mapping[count - 1] = (count - 1) as i16;
            check_solver_source_selection(count, &[mapping.clone(), mapping], true);
        }
        let invalid_count = crate::cloth_render_scale::MAX_TRANSFORMS + 1;
        check_solver_source_selection(invalid_count, &[vec![0; invalid_count]], false);
    }

    #[test]
    fn solver_source_selection_replays_optional_equipment_capture() {
        let Ok(path) = std::env::var("ER_SCALE_SOURCE_MAPPING_FIXTURE") else {
            return;
        };
        let fixture = std::fs::read_to_string(path).unwrap();
        let mut lines = fixture.lines();
        let count: usize = lines.next().unwrap().parse().unwrap();
        let mappings: Vec<Vec<i16>> = lines
            .map(|line| {
                let mut values = line.split_whitespace();
                let output_count: usize = values.next().unwrap().parse().unwrap();
                let mapping: Vec<i16> = values.map(|value| value.parse().unwrap()).collect();
                assert_eq!(mapping.len(), output_count);
                mapping
            })
            .collect();
        check_solver_source_selection(count, &mappings, true);
    }

    fn check_solver_source_selection(count: usize, maps: &[Vec<i16>], accepted: bool) {
        let mut transforms =
            vec![[0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0]; count];
        let mut flags = vec![0u32; count];
        flags[count - 1] = 2;
        let mut metadata = Box::new([0u8; 0x40]);
        unsafe {
            (metadata.as_mut_ptr().add(POSE_METADATA_COUNT_OFFSET) as *mut i32)
                .write_unaligned(count as i32);
        }
        let mut update_context = Box::new([0u8; 0x30]);
        unsafe {
            (update_context.as_mut_ptr().add(POSE_METADATA_OFFSET) as *mut usize)
                .write_unaligned(metadata.as_ptr() as usize);
            (update_context.as_mut_ptr().add(POSE_OUTPUT_OFFSET) as *mut usize)
                .write_unaligned(transforms.as_mut_ptr() as usize);
            (update_context.as_mut_ptr().add(0x28) as *mut usize)
                .write_unaligned(flags.as_mut_ptr() as usize);
        }

        let mut destinations = vec![[0u8; 0x28]; maps.len()];
        let mut mappings = vec![[0u8; 0x20]; maps.len()];
        let mut entries = vec![[0u8; CLOTH_SOLVER_ENTRY_STRIDE]; maps.len()];
        for (index, mapped_indices) in maps.iter().enumerate() {
            let destination = &mut destinations[index];
            let mapping = &mut mappings[index];
            let entry = &mut entries[index];
            unsafe {
                (destination.as_mut_ptr().add(0x20) as *mut i32)
                    .write_unaligned(mapped_indices.len() as i32);
                (mapping.as_mut_ptr().add(0x10) as *mut u32)
                    .write_unaligned(mapped_indices.len() as u32);
                (mapping.as_mut_ptr().add(0x18) as *mut usize)
                    .write_unaligned(mapped_indices.as_ptr() as usize);
                (entry.as_mut_ptr() as *mut usize).write_unaligned(destination.as_ptr() as usize);
                (entry.as_mut_ptr().add(0x08) as *mut usize)
                    .write_unaligned(mapping.as_ptr() as usize);
            }
        }
        let mut core = Box::new([0u8; 0x50]);
        unsafe {
            (core.as_mut_ptr().add(0x40) as *mut usize).write_unaligned(entries.as_ptr() as usize);
            (core.as_mut_ptr().add(0x48) as *mut i32).write_unaligned(maps.len() as i32);
        }

        let mut selected = Vec::new();
        let mut lazy = Vec::new();
        let result = collect_solver_source_indices(
            core.as_ptr() as usize,
            update_context.as_ptr() as usize,
            &mut selected,
            &mut lazy,
        );
        if !accepted {
            assert_eq!(result, None);
            return;
        }
        assert_eq!(
            result,
            Some((transforms.as_ptr() as usize, count)),
            "source count {count}"
        );
        let mut expected = Vec::new();
        for &index in maps.iter().flatten().filter(|&&index| index >= 0) {
            if !expected.contains(&(index as usize)) {
                expected.push(index as usize);
            }
        }
        let expected_lazy: Vec<_> = expected
            .iter()
            .copied()
            .filter(|&index| flags[index] & 2 != 0)
            .collect();
        assert_eq!(selected, expected);
        assert_eq!(lazy, expected_lazy);
        let original = transforms.clone();
        let mut backups = Vec::new();
        for scale in [1.12, 0.85, 2.0] {
            assert_eq!(
                scale_solver_source_transforms(&mut transforms, &selected, scale, &mut backups),
                Some(selected.len())
            );
            for index in 0..count {
                assert_eq!(
                    transforms[index][1],
                    if selected.contains(&index) {
                        2.0 * scale
                    } else {
                        2.0
                    }
                );
                assert_eq!(transforms[index][3..], original[index][3..]);
            }
            restore_solver_source_transforms(&mut transforms, &backups);
            assert_eq!(transforms, original);
        }
    }

    #[test]
    fn lazy_solver_source_materialization_accepts_only_the_selected_exact_output() {
        let mut transforms = Box::new([[0.0f32; 12]; 2]);
        let transforms_address = transforms.as_mut_ptr() as usize;
        let update_context = 0x1234usize;

        assert_eq!(
            materialize_lazy_solver_sources(
                update_context,
                transforms_address,
                transforms.len(),
                &[0, 1],
                &[1],
                |context, index| {
                    assert_eq!(context, update_context);
                    assert_eq!(index, 1);
                    transforms_address + CLOTH_SOLVER_SOURCE_TRANSFORM_STRIDE
                },
            ),
            Some(1)
        );
        assert_eq!(
            materialize_lazy_solver_sources(
                update_context,
                transforms_address,
                transforms.len(),
                &[0, 1],
                &[1],
                |_, _| transforms_address,
            ),
            None
        );
        assert_eq!(
            materialize_lazy_solver_sources(
                update_context,
                transforms_address,
                transforms.len(),
                &[0],
                &[1],
                |_, _| transforms_address + CLOTH_SOLVER_SOURCE_TRANSFORM_STRIDE,
            ),
            None
        );
    }

    #[test]
    fn solver_consumer_materialization_and_scale_leave_render_pose_cache_unchanged() {
        // Reproduce both consumers: cloth lazily resolves child 1 (which also
        // resolves parent 0), then rendering consumes the canonical pose after
        // the temporary source-translation bracket has completed.
        let mut local = [[0.0f32; 12]; 2];
        for pose in &mut local {
            pose[1] = 1.0;
            pose[7] = 1.0;
            pose[8..11].fill(1.0);
        }
        let mut output = [[9.0f32; 12]; 2];
        let mut flags = [2u32, 2];
        let parents = [-1i16, 0];
        let mut metadata = [0usize; 8];
        metadata[4] = parents.as_ptr() as usize;
        metadata[7] = 2;
        let mut context = [0usize; 8];
        context[0] = metadata.as_ptr() as usize;
        context[1] = local.as_mut_ptr() as usize;
        context[2] = 2;
        context[3] = output.as_mut_ptr() as usize;
        context[4] = 2;
        context[5] = flags.as_mut_ptr() as usize;
        context[6] = 2;
        let baseline = (local, output, flags, context);
        let mut scratch = SolverSourceScratch::default();
        let consumer = solver_consumer_context(context.as_ptr() as usize, &mut scratch).unwrap();
        let model_buffer = read_usize(consumer + 0x18).unwrap();
        assert_eq!(
            materialize_lazy_solver_sources(consumer, model_buffer, 2, &[1], &[1], |ctx, index| {
                assert_eq!(index, 1);
                let local_address = read_usize(ctx + 8).unwrap();
                let output_address = read_usize(ctx + 0x18).unwrap();
                let flag_address = read_usize(ctx + 0x28).unwrap();
                // Identity-rotation two-node case of ER 16549A0; it writes the
                // parent and flags too, not only the selected child's XYZ.
                unsafe {
                    let src = std::slice::from_raw_parts(
                        local_address as *const SolverSourceTransform,
                        2,
                    );
                    let dst = std::slice::from_raw_parts_mut(
                        output_address as *mut SolverSourceTransform,
                        2,
                    );
                    dst.copy_from_slice(src);
                    dst[1][1] += dst[0][1];
                    *(flag_address as *mut u32) = 0;
                    *((flag_address as *mut u32).add(1)) = 0;
                }
                output_address + 48
            }),
            Some(1)
        );
        let source = unsafe {
            std::slice::from_raw_parts_mut(model_buffer as *mut SolverSourceTransform, 2)
        };
        assert_eq!(
            scale_solver_source_transforms(source, &[1], 0.5, &mut scratch.backups),
            Some(1)
        );
        assert_eq!(
            source[1][1], 1.0,
            "cloth must retain the half-size placement input"
        );
        assert_eq!(
            source[1][8], 1.0,
            "native .5 core must not receive another .5 source scale"
        );
        restore_solver_source_transforms(source, &scratch.backups);
        assert_eq!(local, baseline.0);
        assert_eq!(
            output, baseline.1,
            "cloth preparation leaked into the rendering pose cache"
        );
        assert_eq!(
            flags, baseline.2,
            "XYZ restoration did not restore native lazy-cache flags"
        );
        assert_eq!(context, baseline.3);
    }

    #[test]
    fn solver_consumer_rejects_malformed_topology_without_touching_canonical() {
        let local = [[1.0f32; 12]; 2];
        let output = [[2.0f32; 12]; 2];
        let flags = [2u32; 2];
        let mut parents = [-1i16, 0];
        let mut metadata = [0usize; 8];
        metadata[4] = parents.as_mut_ptr() as usize;
        metadata[7] = 2;
        let header = [
            metadata.as_ptr() as usize,
            local.as_ptr() as usize,
            2,
            output.as_ptr() as usize,
            2,
            flags.as_ptr() as usize,
            2,
            0,
        ];
        let mut scratch = SolverSourceScratch::default();
        assert!(solver_consumer_context(0, &mut scratch).is_none());
        for (field, value) in [
            (1, 0),
            (3, 0),
            (5, 0),
            (2, 1),
            (4, 1),
            (6, 1),
            (2, u32::MAX as usize),
        ] {
            let mut invalid = header;
            invalid[field] = value;
            assert!(solver_consumer_context(invalid.as_ptr() as usize, &mut scratch).is_none());
        }
        for parent in [-2, 1, 2] {
            parents[1] = parent;
            assert!(solver_consumer_context(header.as_ptr() as usize, &mut scratch).is_none());
        }
        parents[1] = 0;
        for count in [0, MAX_CLOTH_SOLVER_SOURCE_TRANSFORMS + 1] {
            metadata[7] = count;
            assert!(solver_consumer_context(header.as_ptr() as usize, &mut scratch).is_none());
        }
        metadata[7] = 2;
        assert!(solver_consumer_context(header.as_ptr() as usize, &mut scratch).is_some());
        assert_eq!(parents, [-1, 0]);
        assert_eq!(metadata[7], 2);
        assert_eq!(local, [[1.0; 12]; 2]);
        assert_eq!(output, [[2.0; 12]; 2]);
        assert_eq!(flags, [2; 2]);
    }

    #[test]
    fn solver_consumer_scratch_reuse_refreshes_all_bits_and_nested_calls_are_isolated() {
        let mut local = [[0.0f32; 12]; 1];
        local[0][3] = f32::from_bits(0x7fc0_1234);
        let mut output = local;
        output[0][0] = 7.0;
        let flags = [2u32];
        let parents = [-1i16];
        let mut metadata = [0usize; 8];
        metadata[4] = parents.as_ptr() as usize;
        metadata[7] = 1;
        let header = [
            metadata.as_ptr() as usize,
            local.as_ptr() as usize,
            1,
            output.as_ptr() as usize,
            1,
            flags.as_ptr() as usize,
            1,
            0x101,
        ];
        let mut outer = SolverSourceScratch::default();
        let mut nested = SolverSourceScratch::default();
        let ctx = solver_consumer_context(header.as_ptr() as usize, &mut outer).unwrap();
        let nested_ctx = solver_consumer_context(header.as_ptr() as usize, &mut nested).unwrap();
        assert_ne!(ctx, header.as_ptr() as usize);
        assert_ne!(ctx, nested_ctx);
        assert_eq!(ctx % 16, 0);
        assert_eq!(outer.local.as_ptr() as usize % 16, 0);
        assert_eq!(outer.model.as_ptr() as usize % 16, 0);
        assert_eq!(outer.local[0].0[3].to_bits(), 0x7fc0_1234);
        assert_eq!(outer.context.0[7], header[7]);
        assert_ne!(outer.model.as_ptr(), nested.model.as_ptr());
        nested.model[0].0[0] = 50.0;
        nested.flags[0] = 0;
        assert_eq!(outer.model[0].0[0], 7.0);
        assert_eq!(outer.flags[0], 2);
        let buffers = (
            outer.local.as_ptr(),
            outer.model.as_ptr(),
            outer.flags.as_ptr(),
        );
        outer.model[0].0[0] = 99.0;
        outer.flags[0] = 0;
        outer.context.0[7] = 0;
        output[0][0] = 13.0;
        assert_eq!(
            solver_consumer_context(header.as_ptr() as usize, &mut outer),
            Some(ctx)
        );
        assert_eq!(
            buffers,
            (
                outer.local.as_ptr(),
                outer.model.as_ptr(),
                outer.flags.as_ptr()
            )
        );
        assert_eq!(outer.model[0].0[0], 13.0);
        assert_eq!(output[0][0], 13.0);
        assert_eq!(outer.flags[0], 2);
        assert_eq!(outer.context.0[7], 0x101);
        assert_eq!(outer.model[0].0[3].to_bits(), 0x7fc0_1234);
    }

    #[test]
    fn solver_source_projection_has_one_scale_factor_not_build_2_30_two() {
        let mut original = [[0.0f32; 12]; 1];
        original[0][1] = 2.0;
        original[0][7] = 1.0;
        original[0][8..12].copy_from_slice(&[1.0, 2.0, 0.75, -0.0]);
        for scale in [0.5, 0.75, 1.5, 3.0] {
            let mut private = original;
            let mut backups = Vec::new();
            assert_eq!(
                scale_solver_source_transforms(&mut private, &[0], scale, &mut backups),
                Some(1)
            );
            assert_eq!(private[0][1], 2.0 * scale);
            // Native 26B14E0 uses core90-scale * Qs-scale. The source must
            // preserve authored relative scale, even nonuniform values.
            for lane in 8..11 {
                assert_eq!(scale * private[0][lane], scale * original[0][lane]);
            }
            assert_eq!(private[0][11].to_bits(), original[0][11].to_bits());
            let mut old_control = private;
            let mut old_backups = Vec::new();
            scale_solver_source_local_scales(&mut old_control, &[0], scale, &mut old_backups)
                .unwrap();
            assert_ne!(scale * old_control[0][8], scale * private[0][8]);
        }
    }

    #[test]
    fn caps_the_solver_input_probe_across_all_entries() {
        assert_eq!(
            solver_input_transform_total(MAX_CLOTH_SOLVER_INPUT_TRANSFORMS - 1, 1),
            Some(MAX_CLOTH_SOLVER_INPUT_TRANSFORMS)
        );
        assert_eq!(
            solver_input_transform_total(MAX_CLOTH_SOLVER_INPUT_TRANSFORMS, 1),
            None
        );
    }

    #[test]
    fn transition_marks_the_native_secondary_reference_dirty_byte() {
        let mut owner = [0u8; 0x80];
        let owner_address = owner.as_mut_ptr() as usize;

        assert!(mark_secondary_reference_dirty(owner_address));
        assert_eq!(owner[0x4B], 1);
        assert_eq!(owner[0x4A], 0);
        assert!(!mark_secondary_reference_dirty(0));
    }

    #[test]
    fn attachment_center_delta_maps_unit_offset_to_requested_root_offset() {
        assert_eq!(
            attachment_center_delta([12.0, 24.0, 36.0], [10.0, 20.0, 30.0], 0.5),
            Some([-1.0, -2.0, -3.0])
        );
        assert_eq!(
            attachment_center_delta([11.0, 22.0, 33.0], [10.0, 20.0, 30.0], 3.0),
            Some([2.0, 4.0, 6.0])
        );
    }

    #[test]
    fn transition_attachment_delta_uses_before_center_even_when_native_already_moved_it() {
        assert_eq!(
            transition_attachment_center_delta(
                [12.0, 24.0, 36.0],
                [11.5, 23.0, 34.5],
                [10.0, 20.0, 30.0],
                0.5,
            ),
            Some([-0.5, -1.0, -1.5])
        );
    }

    #[test]
    fn translating_requested_matrix_preserves_basis_and_homogeneous_lane() {
        let half = [
            0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 11.5, 23.0, 34.5, 1.0,
        ];

        let translated = translated_cloth_matrix(half, [-0.5, -1.0, -1.5]).unwrap();

        assert_eq!(
            translated,
            [
                0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 11.0, 22.0, 33.0, 1.0,
            ]
        );
    }

    #[test]
    fn unavailable_previous_aabb_does_not_block_current_aabb_translation() {
        let mut current: [[f32; 4]; 2] = [[10.0, 20.0, 30.0, 0.0], [12.0, 24.0, 36.0, 1.0]];
        let mut invalid_previous: [[f32; 4]; 2] = [[f32::NAN, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

        let shifted = translate_available_cloth_aabbs(
            [
                current.as_mut_ptr() as usize,
                invalid_previous.as_mut_ptr() as usize,
            ],
            [-1.0, -2.0, -3.0],
        );

        assert_eq!(shifted, 1);
        assert_eq!(current, [[9.0, 18.0, 27.0, 0.0], [11.0, 22.0, 33.0, 1.0]]);
        assert!(invalid_previous[0][0].is_nan());
    }

    #[test]
    fn transition_sync_moves_requested_generation_with_only_current_aabb_available() {
        let mut child = [0u8; 0x178];
        let mut sim_data = [0u8; 0x50];
        let mut current_positions = [[12.0f32, 24.0, 36.0, 0.25], [13.0, 26.0, 39.0, 0.75]];
        let mut previous_positions = current_positions;
        let mut transform_entry = [0u8; 0x60];
        let mut transform_entries = [transform_entry.as_mut_ptr() as usize];
        let half_matrix = [
            0.5f32, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0, 11.5, 23.0, 34.5, 1.0,
        ];
        let current_aabb = [[10.0f32, 20.0, 30.0, 0.0], [12.0, 24.0, 36.0, 1.0]];
        let invalid_previous_aabb = [[f32::NAN, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        let child_address = child.as_mut_ptr() as usize;

        unsafe {
            (sim_data.as_mut_ptr().add(0x48) as *mut u32).write_unaligned(2);
            (child.as_mut_ptr().add(0x18) as *mut usize)
                .write_unaligned(sim_data.as_mut_ptr() as usize);
            (child.as_mut_ptr().add(0x20) as *mut usize)
                .write_unaligned(current_positions.as_mut_ptr() as usize);
            (child.as_mut_ptr().add(0x30) as *mut usize)
                .write_unaligned(previous_positions.as_mut_ptr() as usize);
            (child.as_mut_ptr().add(0x60) as *mut [[f32; 4]; 2]).write_unaligned(current_aabb);
            (child.as_mut_ptr().add(0x80) as *mut [[f32; 4]; 2])
                .write_unaligned(invalid_previous_aabb);
            (transform_entry.as_mut_ptr().add(0x20) as *mut [f32; 16]).write_unaligned(half_matrix);
            (child.as_mut_ptr().add(0x168) as *mut usize)
                .write_unaligned(transform_entries.as_mut_ptr() as usize);
            (child.as_mut_ptr().add(0x170) as *mut i32).write_unaligned(1);
        }

        let mut counts = ClothTransformResyncCounts::default();
        assert!(sync_transition_child_attachment_position(
            child_address,
            0.5,
            [-0.5, -1.0, -1.5],
            &mut counts,
        ));

        assert_eq!(counts.children_observed, 0);
        assert_eq!(counts.entries_changed, 1);
        assert_eq!(counts.position_buffers_shifted, 2);
        assert_eq!(counts.particles_shifted, 4);
        assert_eq!(counts.aabbs_shifted, 1);
        assert_eq!(counts.position_rejected, 0);
        assert_eq!(
            current_positions,
            [[11.5, 23.0, 34.5, 0.25], [12.5, 25.0, 37.5, 0.75]]
        );
        assert_eq!(previous_positions, current_positions);
        let written_matrix =
            unsafe { (transform_entry.as_ptr().add(0x20) as *const [f32; 16]).read_unaligned() };
        assert_eq!(
            [written_matrix[0], written_matrix[5], written_matrix[10]],
            [0.5; 3]
        );
        assert_eq!(
            [written_matrix[12], written_matrix[13], written_matrix[14]],
            [11.0, 22.0, 33.0]
        );
        let written_current_aabb =
            unsafe { (child.as_ptr().add(0x60) as *const [[f32; 4]; 2]).read_unaligned() };
        assert_eq!(
            written_current_aabb,
            [[9.5, 19.0, 28.5, 0.0], [11.5, 23.0, 34.5, 1.0]]
        );
        let written_previous_aabb =
            unsafe { (child.as_ptr().add(0x80) as *const [[f32; 4]; 2]).read_unaligned() };
        assert!(written_previous_aabb[0][0].is_nan());
    }

    #[test]
    fn attachment_translation_preserves_scaled_particle_span_and_w() {
        let mut positions: [[f32; 4]; 2] = [[12.0, 24.0, 36.0, 0.25], [13.0, 26.0, 39.0, 0.75]];
        let address = positions.as_mut_ptr() as usize;

        assert!(position_buffer_is_writable(address, positions.len()));
        translate_position_buffer(address, positions.len(), [-1.0, -2.0, -3.0]);

        assert_eq!(
            positions,
            [[11.0, 22.0, 33.0, 0.25], [12.0, 24.0, 36.0, 0.75]]
        );
        assert_eq!(
            [
                positions[1][0] - positions[0][0],
                positions[1][1] - positions[0][1],
                positions[1][2] - positions[0][2],
            ],
            [1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn attachment_translation_moves_both_aabb_endpoints_without_resizing() {
        let mut aabb: [[f32; 4]; 2] = [[10.0, 20.0, 30.0, 0.0], [12.0, 24.0, 36.0, 1.0]];
        let address = aabb.as_mut_ptr() as usize;

        assert!(cloth_aabb_is_writable(address));
        translate_cloth_aabb(address, [-1.0, -2.0, -3.0]);

        assert_eq!(aabb, [[9.0, 18.0, 27.0, 0.0], [11.0, 22.0, 33.0, 1.0]]);
    }

    #[test]
    fn builds_previous_scale_reference_with_player_root_pivot() {
        let current = [
            0.0, 0.5, 0.0, 0.0, // rotated basis X
            -0.5, 0.0, 0.0, 0.0, // rotated basis Y
            0.0, 0.0, 0.5, 0.0, // basis Z
            10.0, 20.0, 30.0, 1.0, // world translation
        ];
        let reference = reference_matrix_for_scale_transition(current, 1.0, 0.5).unwrap();

        assert_eq!(
            reference,
            [
                0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 5.0, 10.0, 15.0, 1.0,
            ]
        );
    }

    #[test]
    fn builds_ratio_reference_for_direct_scale_transitions_and_restoration() {
        let current_growth = [
            3.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 0.0, 3.0, 0.0, 4.0, 5.0, 6.0, 1.0,
        ];
        let from_half = reference_matrix_for_scale_transition(current_growth, 0.5, 3.0).unwrap();
        assert_eq!([from_half[0], from_half[5], from_half[10]], [0.5; 3]);
        assert_eq!(
            [from_half[12], from_half[13], from_half[14]],
            [24.0, 30.0, 36.0]
        );

        let current_unit = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 4.0, 5.0, 6.0, 1.0,
        ];
        let from_growth = reference_matrix_for_scale_transition(current_unit, 3.0, 1.0).unwrap();
        assert_eq!([from_growth[0], from_growth[5], from_growth[10]], [3.0; 3]);
        for (actual, expected) in [from_growth[12], from_growth[13], from_growth[14]]
            .into_iter()
            .zip([4.0 / 3.0, 5.0 / 3.0, 2.0])
        {
            assert!((actual - expected).abs() < 1.0e-6);
        }
    }

    #[test]
    fn counts_immediate_child_scale_matches_per_channel() {
        fn bounds(span: f32) -> ClothPositionBounds {
            ClothPositionBounds {
                readable: true,
                count: 4,
                max_x: span,
                max_y: span * 2.0,
                max_z: span * 3.0,
                ..ClothPositionBounds::default()
            }
        }
        fn transforms(scale: f32) -> ClothTransformEntrySummary {
            ClothTransformEntrySummary {
                readable: true,
                count: 2,
                basis_min_x: scale,
                basis_min_y: scale,
                basis_min_z: scale,
                basis_max_x: scale,
                basis_max_y: scale,
                basis_max_z: scale,
                ..ClothTransformEntrySummary::default()
            }
        }

        let mut before_children = [ClothChildSnapshot::default(); CLOTH_CHILD_SLOTS];
        let mut after_children = [ClothChildSnapshot::default(); CLOTH_CHILD_SLOTS];
        before_children[0] = ClothChildSnapshot {
            valid: true,
            child: 0x1000,
            current_positions: bounds(2.0),
            transform_entries: transforms(1.0),
            ..ClothChildSnapshot::default()
        };
        after_children[0] = ClothChildSnapshot {
            valid: true,
            child: 0x1000,
            current_positions: bounds(1.0),
            transform_entries: transforms(0.5),
            ..ClothChildSnapshot::default()
        };
        let summary = ClothChildProbeSummary {
            topology_readable: true,
            child_count: 1,
            captured_children: 1,
            ..ClothChildProbeSummary::default()
        };

        let counts = immediate_child_transition_counts(
            &(summary, before_children),
            &(summary, after_children),
            0.5,
        );

        assert_eq!(
            counts,
            ImmediateChildTransitionCounts {
                children_observed: 1,
                particle_matches: 1,
                transform_matches: 1,
                both_matches: 1,
                unreadable: 0,
            }
        );
    }

    #[test]
    fn rejects_reference_when_current_matrix_does_not_match_requested_scale() {
        let unit = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        assert_eq!(reference_matrix_for_scale_transition(unit, 1.0, 0.5), None);
        assert_eq!(reference_matrix_for_scale_transition(unit, 1.0, 1.0), None);
    }

    #[test]
    fn reads_win64_fifth_argument_from_entry_stack() {
        let mut stack = [0usize; 6];
        stack[5] = 0xAB;
        assert_eq!(fifth_stack_argument_u8(stack.as_ptr() as usize), 0xAB);
    }

    #[test]
    fn summarizes_cloth_particle_bounds_with_one_region_check() {
        let positions = [
            [2.0f32, -3.0, 7.0, 1.0],
            [-4.0, 5.0, 1.0, 1.0],
            [8.0, 2.0, -6.0, 1.0],
        ];
        let bounds = summarize_position_buffer(positions.as_ptr() as usize, positions.len());

        assert!(bounds.readable);
        assert_eq!(bounds.count, 3);
        assert_eq!(
            [bounds.min_x, bounds.min_y, bounds.min_z],
            [-4.0, -3.0, -6.0]
        );
        assert_eq!([bounds.max_x, bounds.max_y, bounds.max_z], [8.0, 5.0, 7.0]);
        assert_eq!(
            [bounds.span_x(), bounds.span_y(), bounds.span_z()],
            [12.0, 8.0, 13.0]
        );
    }

    #[test]
    fn rejects_non_finite_cloth_particle_bounds() {
        let positions = [[0.0f32, f32::NAN, 0.0, 1.0]];
        assert!(!summarize_position_buffer(positions.as_ptr() as usize, 1).readable);
    }

    #[test]
    fn summarizes_havok_particle_radius_field_without_treating_mass_as_size() {
        let particles = [[2.0f32, 0.5, 0.125, 0.2], [4.0f32, 0.25, 0.375, 0.4]];
        let radii = summarize_strided_scalar(
            particles.as_ptr() as usize,
            particles.len(),
            size_of::<[f32; 4]>(),
            0x08,
        );

        assert!(radii.readable);
        assert_eq!(radii.count, 2);
        assert_eq!([radii.minimum, radii.maximum], [0.125, 0.375]);
    }

    #[test]
    fn profiles_reflected_hcl_sim_cloth_instance_aabb_members() {
        let mut child = [0u8; 0xD0];
        let mut sim_data = [0u8; 0xE4];
        let particles = [[1.0f32, 2.0, 3.0, 0.0], [2.0, 4.0, 6.0, 1.0]];
        let collision = [[0.5f32, 1.0, 1.5, 0.0], [2.5, 5.0, 7.5, 1.0]];
        let landscape = [[-1.0f32, -2.0, -3.0, 0.0], [3.0, 6.0, 9.0, 1.0]];
        unsafe {
            (sim_data.as_mut_ptr().add(0xE0) as *mut f32).write_unaligned(0.0);
            (child.as_mut_ptr().add(0x60) as *mut [[f32; 4]; 2]).write_unaligned(particles);
            (child.as_mut_ptr().add(0x80) as *mut [[f32; 4]; 2]).write_unaligned(collision);
            (child.as_mut_ptr().add(0xB0) as *mut [[f32; 4]; 2]).write_unaligned(landscape);
        }

        let profile =
            cloth_local_simulation_profile(child.as_ptr() as usize, sim_data.as_ptr() as usize);

        assert!(profile.readable);
        assert_eq!(
            [
                profile.particles_aabb.span_x(),
                profile.particles_aabb.span_y(),
                profile.particles_aabb.span_z(),
            ],
            [1.0, 2.0, 3.0]
        );
        assert_eq!(
            [
                profile.collision_particles_aabb.span_x(),
                profile.collision_particles_aabb.span_y(),
                profile.collision_particles_aabb.span_z(),
            ],
            [2.0, 4.0, 6.0]
        );
        assert_eq!(
            [
                profile.landscape_collision_particles_aabb.span_x(),
                profile.landscape_collision_particles_aabb.span_y(),
                profile.landscape_collision_particles_aabb.span_z(),
            ],
            [4.0, 8.0, 12.0]
        );
    }

    #[test]
    fn reads_havok_pointer_size_array_instead_of_begin_end_vector() {
        let values = [10u32, 20, 30];
        let mut object = [0u8; 0x20];
        unsafe {
            (object.as_mut_ptr().add(0x08) as *mut usize).write_unaligned(values.as_ptr() as usize);
            (object.as_mut_ptr().add(0x10) as *mut i32).write_unaligned(values.len() as i32);
        }

        assert_eq!(
            bounded_hk_array_span(object.as_ptr() as usize, 0x08, 0x10, size_of::<u32>(), 8,),
            Some((values.as_ptr() as usize, values.len()))
        );
    }

    #[test]
    fn maps_profiled_er_constraint_vtables_to_dimensional_layouts() {
        assert_eq!(
            constraint_kind_from_vtable_rva(ER_HCL_STANDARD_LINK_CONSTRAINT_SET_VTABLE_RVA),
            ClothConstraintKind::StandardLink
        );
        assert_eq!(
            constraint_kind_from_vtable_rva(ER_HCL_LOCAL_RANGE_CONSTRAINT_SET_VTABLE_RVA),
            ClothConstraintKind::LocalRange
        );
        assert_eq!(
            constraint_kind_from_vtable_rva(ER_HCL_TRANSITION_CONSTRAINT_SET_VTABLE_RVA),
            ClothConstraintKind::Transition
        );
        assert_eq!(
            constraint_kind_from_vtable_rva(0xDEADBEEF),
            ClothConstraintKind::Unknown
        );
    }

    #[test]
    fn scales_every_component_of_transposed_affine_matrix() {
        let affine = [
            1.0, 0.0, 0.0, 10.0, // first basis column and translation X
            0.0, 1.0, 0.0, 20.0, // second basis column and translation Y
            0.0, 0.0, 1.0, 30.0, // third basis column and translation Z
        ];
        assert_eq!(
            scaled_affine_matrix(affine, 0.5),
            Some([0.5, 0.0, 0.0, 5.0, 0.0, 0.5, 0.0, 10.0, 0.0, 0.0, 0.5, 15.0])
        );
    }

    #[test]
    fn rejects_non_finite_affine_matrix() {
        let mut affine = [1.0; 12];
        affine[7] = f32::NAN;
        assert_eq!(scaled_affine_matrix(affine, 0.5), None);
    }

    #[test]
    fn only_negative_bone_mappings_use_the_nr_scale_fallback() {
        assert!(mapping_value_uses_fallback(-1));
        assert!(mapping_value_uses_fallback(i16::MIN));
        assert!(!mapping_value_uses_fallback(0));
        assert!(!mapping_value_uses_fallback(i16::MAX));
    }

    #[test]
    fn range_fallback_scales_only_a_successful_provider_result() {
        assert_eq!(
            affine_range_fallback_action(Some(true), true, 1),
            AffineRangeFallbackAction::Scale
        );
        assert_eq!(
            affine_range_fallback_action(Some(true), true, 0),
            AffineRangeFallbackAction::ProviderFailed
        );
        assert_eq!(
            affine_range_fallback_action(Some(true), false, 1),
            AffineRangeFallbackAction::Identity
        );
    }

    #[test]
    fn range_fallback_never_scales_mapped_or_unreadable_entries() {
        assert_eq!(
            affine_range_fallback_action(Some(false), true, 1),
            AffineRangeFallbackAction::MappedPose
        );
        assert_eq!(
            affine_range_fallback_action(None, true, 1),
            AffineRangeFallbackAction::Reject
        );
    }

    #[test]
    fn validates_positive_finite_scale() {
        assert!(valid_scale(0.5));
        assert!(valid_scale(3.0));
        assert!(valid_scale(0.49));
        assert!(valid_scale(3.01));
        assert!(!valid_scale(0.0));
        assert!(!valid_scale(-1.0));
        assert!(!valid_scale(f32::NAN));
    }

    #[test]
    fn cloth_pose_selector_prefers_chr_3a8_when_present() {
        assert_eq!(
            choose_cloth_pose_importer(0x3A8, 0x398, 0x3A0, 0),
            (0x3A8, ClothPoseSource::Override3a8)
        );
        assert_eq!(
            choose_cloth_pose_importer(0x3A8, 0x398, 0x3A0, 1),
            (0x3A8, ClothPoseSource::Override3a8)
        );
    }

    #[test]
    fn cloth_pose_selector_matches_er_fallback_branch() {
        assert_eq!(
            choose_cloth_pose_importer(0, 0x398, 0x3A0, 0),
            (0x398, ClothPoseSource::Primary398)
        );
        assert_eq!(
            choose_cloth_pose_importer(0, 0x398, 0x3A0, 1),
            (0x3A0, ClothPoseSource::Alternate3a0)
        );
    }

    #[test]
    fn scales_an_already_materialized_pose_on_first_binding() {
        assert_eq!(
            plan_pose_scale(1.0, 0.5, Some(1), Some(1)),
            PoseScaleAction::Scale(0.5)
        );
    }

    #[test]
    fn uses_a_ratio_for_cached_pose_scale_transitions() {
        assert_eq!(
            plan_pose_scale(0.5, 3.0, Some(1), Some(1)),
            PoseScaleAction::Scale(6.0)
        );
        assert_eq!(
            plan_pose_scale(3.0, 1.0, Some(1), Some(1)),
            PoseScaleAction::Scale(1.0 / 3.0)
        );
        assert_eq!(
            plan_pose_scale(0.5, 0.5, Some(1), Some(1)),
            PoseScaleAction::Noop
        );
    }

    #[test]
    fn treats_a_freshly_materialized_pose_as_unscaled() {
        assert_eq!(
            plan_pose_scale(3.0, 0.5, Some(0), Some(1)),
            PoseScaleAction::Scale(0.5)
        );
        assert_eq!(
            plan_pose_scale(3.0, 1.0, Some(0), Some(1)),
            PoseScaleAction::Noop
        );
    }

    #[test]
    fn accepts_only_addresses_inside_the_er_image() {
        let base = 0x0001_4000_0000_usize;
        assert_eq!(module_relative_rva(base, base), Some(0));
        assert_eq!(
            module_relative_rva(base, base + ER_SIZE_OF_IMAGE as usize - 1),
            Some(ER_SIZE_OF_IMAGE as usize - 1)
        );
        assert_eq!(module_relative_rva(base, base - 1), None);
        assert_eq!(
            module_relative_rva(base, base + ER_SIZE_OF_IMAGE as usize),
            None
        );
    }

    #[test]
    fn matrix_candidate_keys_separate_call_kind_and_site() {
        let single = matrix_candidate_key(MATRIX_CANDIDATE_KIND_SINGLE, 0x1234);
        assert_ne!(single, 0);
        assert_eq!(
            single,
            matrix_candidate_key(MATRIX_CANDIDATE_KIND_SINGLE, 0x1234)
        );
        assert_ne!(
            single,
            matrix_candidate_key(MATRIX_CANDIDATE_KIND_RANGE, 0x1234)
        );
        assert_ne!(
            single,
            matrix_candidate_key(MATRIX_CANDIDATE_KIND_SINGLE, 0x1235)
        );
    }
}

/// Per-character state; native hooks retain an Arc for their full invocation.
pub(crate) struct UnitState {
    identity: RwLock<Option<crate::unit_runtime::Identity>>,
    cloth_skin_normal_calls: AtomicU64,
    cloth_skin_normal_rows: AtomicU64,
    cloth_skin_normal_max_us: AtomicU64,
    cloth_mesh_frame_calls: AtomicU64,
    cloth_mesh_frames_written: AtomicU64,
    cloth_mesh_frame_seen: AtomicU64,
    cloth_mesh_frame_max_us: AtomicU64,
    cloth_mesh_frame_max_queries: AtomicU64,
    cloth_mesh_normal_rows: AtomicU64,
    cloth_mesh_pn_max_us: AtomicU64,
    cloth_mesh_area_calls: AtomicU64,
    cloth_mesh_area_frames: AtomicU64,
    target_pose_importer: AtomicUsize,
    target_cloth_pose_importer: AtomicUsize,
    target_anim_skeleton: AtomicUsize,
    target_cloth_scope: RwLock<ClothOwnerScope>,
    target_scale_bits: AtomicU32,
    pose_applied_scale_bits: AtomicU32,
    cloth_pose_applied_scale_bits: AtomicU32,
    pose_write_lock: AtomicBool,
    pose_target_calls: AtomicU64,
    pose_transforms_written: AtomicU64,
    cloth_pose_target_calls: AtomicU64,
    cloth_pose_transforms_written: AtomicU64,
    pose_gate_00: AtomicU64,
    pose_gate_01: AtomicU64,
    pose_gate_10: AtomicU64,
    pose_gate_11: AtomicU64,
    pose_gate_other: AtomicU64,
    affine_single_target_calls: AtomicU64,
    affine_single_matrices_written: AtomicU64,
    affine_range_target_calls: AtomicU64,
    affine_range_matrices_written: AtomicU64,
    affine_range_provider_successes: AtomicU64,
    affine_range_provider_identities: AtomicU64,
    affine_range_provider_failures: AtomicU64,
    single_target_calls: AtomicU64,
    single_matrices_written: AtomicU64,
    range_target_calls: AtomicU64,
    range_matrices_written: AtomicU64,
    rejected_outputs: AtomicU64,
    render_calls: AtomicU64,
    render_rows: AtomicU64,
    render_rejected: AtomicU64,
    render_max_us: AtomicU64,
    render_max_queries: AtomicU64,
    cloth_setter_calls: AtomicU64,
    cloth_slot_inserts: AtomicU64,
    cloth_slot_replacements: AtomicU64,
    cloth_topology_generation: AtomicU64,
    cloth_replacement_cursor: AtomicUsize,
    cloth_instance_owners: [AtomicUsize; CLOTH_INSTANCE_SLOTS],
    cloth_instance_inputs: [AtomicUsize; CLOTH_INSTANCE_SLOTS],
    cloth_instance_was_equipment: [AtomicBool; CLOTH_INSTANCE_SLOTS],
    cloth_instance_source_calls: [AtomicU64; CLOTH_INSTANCE_SLOTS],
    cloth_instance_commits: [AtomicU64; CLOTH_INSTANCE_SLOTS],
    cloth_instance_setter_hits: [AtomicU64; CLOTH_INSTANCE_SLOTS],
    cloth_instance_cores: [AtomicUsize; CLOTH_INSTANCE_SLOTS],
    cloth_instance_applied_scale_bits: [AtomicU32; CLOTH_INSTANCE_SLOTS],
    cloth_instance_pending_scale_bits: [AtomicU32; CLOTH_INSTANCE_SLOTS],
    cloth_pending_slot_mask: AtomicU64,
    cloth_scale_transitions_queued: AtomicU64,
    cloth_scale_transitions_deferred: AtomicU64,
    cloth_scale_transitions_rejected: AtomicU64,
    cloth_scale_reference_commits: AtomicU64,
    cloth_scale_wrapper_deferred: AtomicU64,
    cloth_scale_wrapper_rejected: AtomicU64,
    cloth_secondary_reference_calls: AtomicU64,
    cloth_secondary_reference_adjusted: AtomicU64,
    cloth_secondary_reference_passthrough: AtomicU64,
    cloth_secondary_reference_rejected: AtomicU64,
    cloth_secondary_probe_calls: AtomicU64,
    cloth_secondary_source_owner_e0_matches: AtomicU64,
    cloth_secondary_source_owner_e0_mismatches: AtomicU64,
    cloth_secondary_pre_core_requested: AtomicU64,
    cloth_secondary_pre_core_unit: AtomicU64,
    cloth_secondary_pre_core_other: AtomicU64,
    cloth_secondary_post_core_requested: AtomicU64,
    cloth_secondary_post_core_unit: AtomicU64,
    cloth_secondary_post_core_other: AtomicU64,
    cloth_secondary_post_copy_matches: AtomicU64,
    cloth_secondary_post_copy_mismatches: AtomicU64,
    cloth_solver_source_bracket_calls: AtomicU64,
    cloth_solver_source_bracket_transforms: AtomicU64,
    cloth_solver_source_bracket_restores: AtomicU64,
    cloth_solver_source_bracket_rejected: AtomicU64,
    cloth_solver_source_direct_transforms: AtomicU64,
    cloth_solver_source_lazy_candidates: AtomicU64,
    cloth_solver_source_lazy_resolved: AtomicU64,
    cloth_solver_source_lazy_rejected: AtomicU64,
    cloth_solver_source_local_scale_calls: AtomicU64,
    cloth_solver_source_local_scale_transforms: AtomicU64,
    cloth_solver_source_local_scale_restores: AtomicU64,
    cloth_solver_source_local_scale_rejected: AtomicU64,
    cloth_solver_source_local_scale_passthrough: AtomicU64,
    cloth_solver_private_context_calls: AtomicU64,
    cloth_solver_private_context_returns: AtomicU64,
    cloth_secondary_core_changed: AtomicU64,
    cloth_secondary_dirty_marked: AtomicU64,
    cloth_secondary_dirty_rejected: AtomicU64,
    cloth_immediate_probe_transitions: AtomicU64,
    cloth_immediate_children_observed: AtomicU64,
    cloth_immediate_particle_matches: AtomicU64,
    cloth_immediate_transform_matches: AtomicU64,
    cloth_immediate_both_matches: AtomicU64,
    cloth_immediate_unreadable: AtomicU64,
    cloth_transform_resync_calls: AtomicU64,
    cloth_transform_resync_children_observed: AtomicU64,
    cloth_transform_resync_children_changed: AtomicU64,
    cloth_transform_resync_entries_observed: AtomicU64,
    cloth_transform_resync_entries_changed: AtomicU64,
    cloth_transform_resync_entries_already_scaled: AtomicU64,
    cloth_transform_resync_entries_rejected: AtomicU64,
    cloth_transform_resync_topology_rejected: AtomicU64,
    cloth_attachment_position_buffers_shifted: AtomicU64,
    cloth_attachment_particles_shifted: AtomicU64,
    cloth_attachment_aabbs_shifted: AtomicU64,
    cloth_attachment_position_rejected: AtomicU64,
    matrix_candidate_keys: [AtomicU64; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_hits: [AtomicU64; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_kinds: [AtomicU32; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_caller_rvas: [AtomicUsize; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_first_this: [AtomicUsize; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_last_this: [AtomicUsize; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_outputs: [AtomicUsize; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_arg8: [AtomicU32; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_arg9: [AtomicU32; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_qword_48: [AtomicUsize; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_qword_68: [AtomicUsize; MATRIX_CANDIDATE_SLOTS],
    matrix_candidate_qword_88: [AtomicUsize; MATRIX_CANDIDATE_SLOTS],
}

impl UnitState {
    pub(crate) fn set_identity(&self, identity: crate::unit_runtime::Identity) {
        if let Ok(mut slot) = self.identity.write() {
            *slot = Some(identity);
        }
    }
    fn identity_current(&self) -> bool {
        self.identity
            .try_read()
            .is_ok_and(|identity| identity.is_none_or(|identity| identity.current()))
    }
}

impl Default for UnitState {
    fn default() -> Self {
        Self {
            identity: RwLock::new(None),
            cloth_skin_normal_calls: AtomicU64::new(0),
            cloth_skin_normal_rows: AtomicU64::new(0),
            cloth_skin_normal_max_us: AtomicU64::new(0),
            cloth_mesh_frame_calls: AtomicU64::new(0),
            cloth_mesh_frames_written: AtomicU64::new(0),
            cloth_mesh_frame_seen: AtomicU64::new(0),
            cloth_mesh_frame_max_us: AtomicU64::new(0),
            cloth_mesh_frame_max_queries: AtomicU64::new(0),
            cloth_mesh_normal_rows: AtomicU64::new(0),
            cloth_mesh_pn_max_us: AtomicU64::new(0),
            cloth_mesh_area_calls: AtomicU64::new(0),
            cloth_mesh_area_frames: AtomicU64::new(0),
            target_pose_importer: AtomicUsize::new(0),
            target_cloth_pose_importer: AtomicUsize::new(0),
            target_anim_skeleton: AtomicUsize::new(0),
            target_cloth_scope: RwLock::new(ClothOwnerScope {
                player: 0,
                player_model: 0,
                assembly: 0,
                anchor: 0,
                routes: [ClothOwnerRoute {
                    slot_address: 0,
                    model: 0,
                    owner: 0,
                    input: 0,
                    inner: 0,
                    core: 0,
                }; 27],
            }),
            target_scale_bits: AtomicU32::new(1.0f32.to_bits()),
            pose_applied_scale_bits: AtomicU32::new(1.0f32.to_bits()),
            cloth_pose_applied_scale_bits: AtomicU32::new(1.0f32.to_bits()),
            pose_write_lock: AtomicBool::new(false),
            pose_target_calls: AtomicU64::new(0),
            pose_transforms_written: AtomicU64::new(0),
            cloth_pose_target_calls: AtomicU64::new(0),
            cloth_pose_transforms_written: AtomicU64::new(0),
            pose_gate_00: AtomicU64::new(0),
            pose_gate_01: AtomicU64::new(0),
            pose_gate_10: AtomicU64::new(0),
            pose_gate_11: AtomicU64::new(0),
            pose_gate_other: AtomicU64::new(0),
            affine_single_target_calls: AtomicU64::new(0),
            affine_single_matrices_written: AtomicU64::new(0),
            affine_range_target_calls: AtomicU64::new(0),
            affine_range_matrices_written: AtomicU64::new(0),
            affine_range_provider_successes: AtomicU64::new(0),
            affine_range_provider_identities: AtomicU64::new(0),
            affine_range_provider_failures: AtomicU64::new(0),
            single_target_calls: AtomicU64::new(0),
            single_matrices_written: AtomicU64::new(0),
            range_target_calls: AtomicU64::new(0),
            range_matrices_written: AtomicU64::new(0),
            rejected_outputs: AtomicU64::new(0),
            render_calls: AtomicU64::new(0),
            render_rows: AtomicU64::new(0),
            render_rejected: AtomicU64::new(0),
            render_max_us: AtomicU64::new(0),
            render_max_queries: AtomicU64::new(0),
            cloth_setter_calls: AtomicU64::new(0),
            cloth_slot_inserts: AtomicU64::new(0),
            cloth_slot_replacements: AtomicU64::new(0),
            cloth_topology_generation: AtomicU64::new(0),
            cloth_replacement_cursor: AtomicUsize::new(0),
            cloth_instance_owners: [const { AtomicUsize::new(0) }; CLOTH_INSTANCE_SLOTS],
            cloth_instance_inputs: [const { AtomicUsize::new(0) }; CLOTH_INSTANCE_SLOTS],
            cloth_instance_was_equipment: [const { AtomicBool::new(false) }; CLOTH_INSTANCE_SLOTS],
            cloth_instance_source_calls: [const { AtomicU64::new(0) }; CLOTH_INSTANCE_SLOTS],
            cloth_instance_commits: [const { AtomicU64::new(0) }; CLOTH_INSTANCE_SLOTS],
            cloth_instance_setter_hits: [const { AtomicU64::new(0) }; CLOTH_INSTANCE_SLOTS],
            cloth_instance_cores: [const { AtomicUsize::new(0) }; CLOTH_INSTANCE_SLOTS],
            cloth_instance_applied_scale_bits: [const { AtomicU32::new(1.0f32.to_bits()) };
                CLOTH_INSTANCE_SLOTS],
            cloth_instance_pending_scale_bits: [const { AtomicU32::new(NO_PENDING_CLOTH_SCALE_BITS) };
                CLOTH_INSTANCE_SLOTS],
            cloth_pending_slot_mask: AtomicU64::new(0),
            cloth_scale_transitions_queued: AtomicU64::new(0),
            cloth_scale_transitions_deferred: AtomicU64::new(0),
            cloth_scale_transitions_rejected: AtomicU64::new(0),
            cloth_scale_reference_commits: AtomicU64::new(0),
            cloth_scale_wrapper_deferred: AtomicU64::new(0),
            cloth_scale_wrapper_rejected: AtomicU64::new(0),
            cloth_secondary_reference_calls: AtomicU64::new(0),
            cloth_secondary_reference_adjusted: AtomicU64::new(0),
            cloth_secondary_reference_passthrough: AtomicU64::new(0),
            cloth_secondary_reference_rejected: AtomicU64::new(0),
            cloth_secondary_probe_calls: AtomicU64::new(0),
            cloth_secondary_source_owner_e0_matches: AtomicU64::new(0),
            cloth_secondary_source_owner_e0_mismatches: AtomicU64::new(0),
            cloth_secondary_pre_core_requested: AtomicU64::new(0),
            cloth_secondary_pre_core_unit: AtomicU64::new(0),
            cloth_secondary_pre_core_other: AtomicU64::new(0),
            cloth_secondary_post_core_requested: AtomicU64::new(0),
            cloth_secondary_post_core_unit: AtomicU64::new(0),
            cloth_secondary_post_core_other: AtomicU64::new(0),
            cloth_secondary_post_copy_matches: AtomicU64::new(0),
            cloth_secondary_post_copy_mismatches: AtomicU64::new(0),
            cloth_solver_source_bracket_calls: AtomicU64::new(0),
            cloth_solver_source_bracket_transforms: AtomicU64::new(0),
            cloth_solver_source_bracket_restores: AtomicU64::new(0),
            cloth_solver_source_bracket_rejected: AtomicU64::new(0),
            cloth_solver_source_direct_transforms: AtomicU64::new(0),
            cloth_solver_source_lazy_candidates: AtomicU64::new(0),
            cloth_solver_source_lazy_resolved: AtomicU64::new(0),
            cloth_solver_source_lazy_rejected: AtomicU64::new(0),
            cloth_solver_source_local_scale_calls: AtomicU64::new(0),
            cloth_solver_source_local_scale_transforms: AtomicU64::new(0),
            cloth_solver_source_local_scale_restores: AtomicU64::new(0),
            cloth_solver_source_local_scale_rejected: AtomicU64::new(0),
            cloth_solver_source_local_scale_passthrough: AtomicU64::new(0),
            cloth_solver_private_context_calls: AtomicU64::new(0),
            cloth_solver_private_context_returns: AtomicU64::new(0),
            cloth_secondary_core_changed: AtomicU64::new(0),
            cloth_secondary_dirty_marked: AtomicU64::new(0),
            cloth_secondary_dirty_rejected: AtomicU64::new(0),
            cloth_immediate_probe_transitions: AtomicU64::new(0),
            cloth_immediate_children_observed: AtomicU64::new(0),
            cloth_immediate_particle_matches: AtomicU64::new(0),
            cloth_immediate_transform_matches: AtomicU64::new(0),
            cloth_immediate_both_matches: AtomicU64::new(0),
            cloth_immediate_unreadable: AtomicU64::new(0),
            cloth_transform_resync_calls: AtomicU64::new(0),
            cloth_transform_resync_children_observed: AtomicU64::new(0),
            cloth_transform_resync_children_changed: AtomicU64::new(0),
            cloth_transform_resync_entries_observed: AtomicU64::new(0),
            cloth_transform_resync_entries_changed: AtomicU64::new(0),
            cloth_transform_resync_entries_already_scaled: AtomicU64::new(0),
            cloth_transform_resync_entries_rejected: AtomicU64::new(0),
            cloth_transform_resync_topology_rejected: AtomicU64::new(0),
            cloth_attachment_position_buffers_shifted: AtomicU64::new(0),
            cloth_attachment_particles_shifted: AtomicU64::new(0),
            cloth_attachment_aabbs_shifted: AtomicU64::new(0),
            cloth_attachment_position_rejected: AtomicU64::new(0),
            matrix_candidate_keys: [const { AtomicU64::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_hits: [const { AtomicU64::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_kinds: [const { AtomicU32::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_caller_rvas: [const { AtomicUsize::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_first_this: [const { AtomicUsize::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_last_this: [const { AtomicUsize::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_outputs: [const { AtomicUsize::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_arg8: [const { AtomicU32::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_arg9: [const { AtomicU32::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_qword_48: [const { AtomicUsize::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_qword_68: [const { AtomicUsize::new(0) }; MATRIX_CANDIDATE_SLOTS],
            matrix_candidate_qword_88: [const { AtomicUsize::new(0) }; MATRIX_CANDIDATE_SLOTS],
        }
    }
}

thread_local! {
    static CURRENT_UNIT: RefCell<Option<std::sync::Arc<UnitState>>> = const { RefCell::new(None) };
}
static DEFAULT_UNIT: std::sync::OnceLock<std::sync::Arc<UnitState>> = std::sync::OnceLock::new();

pub(crate) fn default_unit_state() -> std::sync::Arc<UnitState> {
    DEFAULT_UNIT
        .get_or_init(|| std::sync::Arc::new(UnitState::default()))
        .clone()
}

fn current_unit_state() -> std::sync::Arc<UnitState> {
    CURRENT_UNIT
        .with(|slot| slot.borrow().clone())
        .unwrap_or_else(default_unit_state)
}

struct UnitGuard(Option<std::sync::Arc<UnitState>>);
impl Drop for UnitGuard {
    fn drop(&mut self) {
        CURRENT_UNIT.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

pub(crate) fn with_unit_state<T>(state: &std::sync::Arc<UnitState>, run: impl FnOnce() -> T) -> T {
    let _guard = UnitGuard(CURRENT_UNIT.with(|slot| slot.replace(Some(state.clone()))));
    run()
}
