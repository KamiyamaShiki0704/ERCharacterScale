//! Automatic local-player equipment adapter for WW2.7.1.0. Cold binding reads
//! reference skeletons; callbacks use private pose storage and copied render
//! outputs. No player animation, gameplay collider or asset data is written.
use crate::equipment_retarget::{Bone, Frame, MotionProfile, Plan};
use crate::equipment_retarget_pose::{self as pose, PoseScratch, SkeletonIdentity};
use crate::unit_runtime::Identity;
use glam::DMat4;
use ilhook::x64::{CallbackOption, HookFlags, Registers, hook_closure_retn};
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex, OnceLock, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

const MAPPER_VTABLE: usize = 0x2B6FAC8;
const RANGE_RVA: usize = 0xB47D70;
const MATRIX_RANGE_RVA: usize = 0xB48590;
const RANGE_GUARD: &[u8] = &[
    0x48, 0x8B, 0xC4, 0x55, 0x53, 0x56, 0x57, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57,
];
const INPUT_VTABLE: usize = 0x2B70360;
const CORE_VTABLE: usize = 0x329E3D0;
// The synchronous cloth consumer resolves model-dirty Qs rows itself.
const LAZY_MODEL_CONSUMER_RVA: usize = 0x26B1944;
const LAZY_MODEL_CONSUMER_GUARD: &[u8] = &[
    0x49, 0x8B, 0x47, 0x28, 0xF6, 0x04, 0x88, 0x02, 0x75, 0x0E, 0x48, 0x8D, 0x04, 0x49, 0x48, 0xC1,
    0xE0, 0x04, 0x49, 0x03, 0x47, 0x18, 0xEB, 0x08, 0x49, 0x8B, 0xCF, 0xE8, 0xAC, 0x30, 0xFA, 0xFE,
];
// Post native root-motion multiplier, before ChrDataModule consumption. The
// caller rebuilt ctrl+140 from the current animation at 3CC76D..3CC787.
const MOTION_RVA: usize = 0x3CC86B;
const MOTION_GUARD: &[u8] = &[
    0x48, 0x8B, 0x43, 0x10, 0x48, 0x8B, 0xD7, 0x48, 0x8B, 0x88, 0x90, 0x01, 0x00, 0x00,
];
// ExFollowCam target computation, before collision/camera integration. XMM3
// contains the authored follow height; both target positions consume it.
const CAMERA_RVA: usize = 0x3B85C8;
const CAMERA_GUARD: &[u8] = &[
    0x0F, 0xC6, 0xDB, 0x00, 0xF3, 0x0F, 0x10, 0xA3, 0x98, 0x01, 0x00, 0x00, 0x0F, 0xC6, 0xE4, 0x00,
];
#[path = "equipment_retarget_attachments.rs"]
mod attachments;
#[path = "equipment_retarget_dummies.rs"]
mod dummies;
#[path = "equipment_retarget_hook.rs"]
mod mid_hook;
#[path = "equipment_retarget_weapon.rs"]
mod weapon;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    pub enabled: bool,
    #[serde(alias = "models")]
    pub force_models: Vec<String>,
    pub exclude_models: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            force_models: Vec::new(),
            exclude_models: Vec::new(),
        }
    }
}

impl Settings {
    /// None excludes; false selects automatic detection; true forces binding.
    fn policy(&self, name: &str) -> Option<bool> {
        if !self.enabled
            || self
                .exclude_models
                .iter()
                .any(|n| n.eq_ignore_ascii_case(name))
        {
            return None;
        }
        Some(
            self.force_models
                .iter()
                .any(|n| n.eq_ignore_ascii_case(name)),
        )
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.force_models.len() + self.exclude_models.len() > 256 {
            return Err("retarget model overrides exceed 256 entries".into());
        }
        let mut seen = HashSet::new();
        for name in self.force_models.iter().chain(&self.exclude_models) {
            if name.is_empty()
                || name.len() > 128
                || name.trim() != name
                || !name.is_ascii()
                || !seen.insert(name.to_ascii_uppercase())
            {
                return Err(
                    "retarget model overrides contain an invalid, duplicate or conflicting name"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

static CONFIG: OnceLock<Settings> = OnceLock::new();
static BASE: AtomicUsize = AtomicUsize::new(0);
static READY: AtomicBool = AtomicBool::new(false);
static REGISTRY: std::sync::LazyLock<RwLock<Vec<Arc<Session>>>> =
    std::sync::LazyLock::new(|| RwLock::new(Vec::new()));
static REJECTED: Mutex<Vec<(Key, Identity, u64)>> = Mutex::new(Vec::new());
static NATIVE: Mutex<Vec<NativeBinding>> = Mutex::new(Vec::new());

struct NativeBinding {
    key: Key,
    identity: Identity,
    source: SkeletonIdentity,
    count: usize,
}

impl NativeBinding {
    fn current(&self, identity: Identity, base: usize) -> bool {
        self.identity == identity
            && self.key.current(base)
            && self.source.current(&read)
            && pose::bytes::<4>(&read, self.key.resource + 0x1C)
                == Some((self.count as i32).to_le_bytes())
    }
}

enum Binding {
    Retarget(Box<Session>),
    Native(Box<NativeBinding>),
}

pub(crate) fn initialize(settings: Settings) {
    let _ = CONFIG.set(settings);
}

fn enabled() -> bool {
    CONFIG.get().is_some_and(|s| s.enabled)
}

fn read(at: usize, out: &mut [u8]) -> bool {
    if out.is_empty() {
        return true;
    }
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
    if !enabled() {
        return true;
    }
    if READY.load(Ordering::Acquire) {
        return BASE.load(Ordering::Acquire) == base;
    }
    for (rva, slot) in [(RANGE_RVA, 0x40), (MATRIX_RANGE_RVA, 0x38)] {
        let mut guard = vec![0u8; RANGE_GUARD.len()];
        if !read(base + rva, &mut guard)
            || guard != RANGE_GUARD
            || ptr(base + MAPPER_VTABLE + slot) != Some(base + rva)
        {
            return false;
        }
    }
    for (rva, expected) in [
        (LAZY_MODEL_CONSUMER_RVA, LAZY_MODEL_CONSUMER_GUARD),
        (MOTION_RVA, MOTION_GUARD),
        (CAMERA_RVA, CAMERA_GUARD),
        (attachments::RVA, attachments::GUARD),
        (dummies::RVAS[0], dummies::GUARD),
        (dummies::RVAS[1], dummies::GUARD),
        (dummies::AFFINE_RVAS[0], dummies::AFFINE_GUARDS[0]),
        (dummies::AFFINE_RVAS[1], dummies::AFFINE_GUARDS[1]),
    ] {
        let mut actual = vec![0; expected.len()];
        if !read(base + rva, &mut actual) || actual != expected {
            return false;
        }
    }
    BASE.store(base, Ordering::Release);
    let hook = unsafe {
        hook_closure_retn(
            base + RANGE_RVA,
            render_hook,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(hook) = hook else {
        return false;
    };
    let matrix_hook = unsafe {
        hook_closure_retn(
            base + MATRIX_RANGE_RVA,
            matrix_render_hook,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(matrix_hook) = matrix_hook else {
        return false;
    };
    let motion_hook = unsafe { mid_hook::install(base + MOTION_RVA, motion_hook) };
    let Ok(motion_hook) = motion_hook else {
        return false;
    };
    let camera_hook = unsafe { mid_hook::install(base + CAMERA_RVA, camera_hook) };
    let Ok(camera_hook) = camera_hook else {
        return false;
    };
    let attachment_hook = unsafe {
        hook_closure_retn(
            base + attachments::RVA,
            attachments::hook,
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(attachment_hook) = attachment_hook else {
        return false;
    };
    let mut dummy_hooks = Vec::new();
    for rva in dummies::RVAS {
        let Ok(hook) = (unsafe {
            hook_closure_retn(
                base + rva,
                dummies::hook,
                CallbackOption::None,
                HookFlags::empty(),
            )
        }) else {
            return false;
        };
        dummy_hooks.push(hook);
    }
    for rva in dummies::AFFINE_RVAS {
        let Ok(hook) = (unsafe {
            hook_closure_retn(
                base + rva,
                dummies::affine_hook,
                CallbackOption::None,
                HookFlags::empty(),
            )
        }) else {
            return false;
        };
        dummy_hooks.push(hook);
    }
    let _ = Box::leak(Box::new(dummy_hooks));
    let _ = Box::leak(Box::new(attachment_hook));
    let _ = Box::leak(Box::new(motion_hook));
    let _ = Box::leak(Box::new(camera_hook));
    let _ = Box::leak(Box::new(hook));
    let _ = Box::leak(Box::new(matrix_hook));
    READY.store(true, Ordering::Release);
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    player: usize,
    assembly: usize,
    slot: usize,
    equipment: usize,
    item: usize,
    resource: usize,
    source: usize,
    source_meta: usize,
    owner: usize,
    input: usize,
    input_meta: usize,
    inner: usize,
    core: usize,
    mappers: Vec<usize>,
    // Pointer edges, not copied object data: replacement invalidates binding
    // even when the outer equipment object and mapper addresses are reused.
    route: Vec<(usize, usize)>,
}

impl Key {
    fn current(&self, base: usize) -> bool {
        ptr(self.player + 0x648) == Some(self.assembly)
            && ptr(self.assembly) == Some(base + crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA)
            && ptr(self.slot) == Some(self.equipment)
            && ptr(self.equipment) == Some(base + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA)
            && ptr(self.equipment + 0x10) == Some(self.item)
            && ptr(self.item + 0x68) == Some(self.resource)
            && ptr(self.player + 0x398) == Some(self.source)
            && ptr(self.source) == Some(base + INPUT_VTABLE)
            && ptr(self.source + 0x48) == Some(self.source_meta)
            && self.route.iter().all(|&(at, value)| ptr(at) == Some(value))
            && self
                .mappers
                .iter()
                .all(|m| ptr(*m) == Some(base + MAPPER_VTABLE))
            && ptr(self.equipment + 0x130) == Some(self.owner)
            && (self.owner == 0
                || (ptr(self.equipment + 0x130) == Some(self.owner)
                    && ptr(self.owner) == Some(base + 0x2B92A60)
                    && ptr(self.owner + 0x120) == Some(self.input)
                    && ptr(self.owner + 0x40) == Some(self.inner)
                    && ptr(self.inner) == Some(base + 0x329A2F8)
                    && ptr(self.inner + 0x30) == Some(self.core)
                    // core+18 belongs to a distinct native object; it is not
                    // an owner back-pointer. The forward chain above binds
                    // this exact core to the selected equipment's inner.
                    && ptr(self.core) == Some(base + CORE_VTABLE)
                    && ptr(self.input) == Some(base + INPUT_VTABLE)
                    && ptr(self.input + 0x48) == Some(self.input_meta)))
    }
}

struct Session {
    active: AtomicBool,
    key: Key,
    identity: Identity,
    source_identity: SkeletonIdentity,
    source_bones: Vec<Bone>,
    cloth_identity: Option<SkeletonIdentity>,
    cloth_bones: Vec<Bone>,
    mesh_bones: Vec<Bone>,
    mesh_plan: Plan,
    motion: Option<MotionProfile>,
    cloth_plan: Option<Plan>,
    mesh_to_cloth: Vec<Option<usize>>,
    cloth_to_mesh: Vec<Option<usize>>,
    work: Mutex<Work>,
}

struct Work {
    grip: weapon::GripCache,
    generation: u64,
    scale: f64,
    prepared: Option<u64>,
    solver_completed: Option<u64>,
    source: PoseScratch,
    solved: PoseScratch,
    mesh: Frame,
    cloth: Option<Frame>,
    render: Vec<DMat4>,
    correction: Vec<DMat4>,
    affine: Vec<[f32; 12]>,
    matrix: Vec<[f32; 16]>,
    overridden: Vec<bool>,
    spare: Option<Box<SolverStorage>>,
}

impl Session {
    fn current(&self) -> bool {
        self.active.load(Ordering::Acquire)
            && self.identity.current()
            && self.key.current(BASE.load(Ordering::Acquire))
            && self.source_identity.current(&read)
            && pose::bytes::<4>(&read, self.key.resource + 0x1C)
                == Some((self.mesh_bones.len() as i32).to_le_bytes())
            && self
                .cloth_identity
                .as_ref()
                .is_none_or(|s| s.current(&read))
    }

    fn prepare(&self, work: &mut Work) -> Option<()> {
        if work.prepared == Some(work.generation) {
            return Some(());
        }
        work.prepared = None;
        work.source.capture(
            &read,
            self.key.source + 0x48,
            &self.source_identity,
            &self.source_bones,
            work.scale,
        )?;
        self.mesh_plan
            .prepare(&work.source.local, work.generation, &mut work.mesh)
            .ok()?;
        if self.motion.is_some() {
            let style = work
                .grip
                .style(&read, self.key.player, BASE.load(Ordering::Acquire));
            let ground_normal = (|| -> Option<glam::DVec3> {
                let modules = ptr(self.key.player + 0x190)?;
                let behavior = ptr(modules + 0x28)?;
                let physics = ptr(modules + 0x68)?;
                let contact = pose::bytes::<2>(&read, physics + 0x1D0)?;
                let state = i32::from_le_bytes(pose::bytes(&read, behavior + 0x1680)?);
                if contact != [0, 1] || state == -1 || ptr(physics + 8) != Some(self.key.player) {
                    return None;
                }
                pose::ground_normal(&read, physics)
            })();
            self.mesh_plan
                .constrain(&mut work.mesh, style, ground_normal)
                .ok()?;
        }
        if let (Some(plan), Some(frame)) = (&self.cloth_plan, &mut work.cloth) {
            plan.prepare(&work.source.local, work.generation, frame)
                .ok()?;
            plan.align_model(work.mesh.outputs()?.model, &self.cloth_to_mesh, frame)
                .ok()?;
        }
        work.prepared = Some(work.generation);
        Some(())
    }
}

fn provider(node: usize) -> Option<usize> {
    ptr(ptr(node + 8)?)
}

fn find_mappers(
    start: usize,
    base: usize,
    result: &mut Vec<usize>,
    visited: &mut HashSet<usize>,
    depth: usize,
    route: &mut Vec<(usize, usize)>,
) -> Option<()> {
    if start == 0 || depth > 8 || !visited.insert(start) {
        return Some(());
    }
    let kind = ptr(start)?.checked_sub(base)?;
    route.push((start, base + kind));
    match kind {
        MAPPER_VTABLE => {
            if !result.contains(&start) {
                result.push(start);
            }
        }
        0x2B708D0 | 0x2B6EB88 => {
            let node = start + if kind == 0x2B708D0 { 0x68 } else { 0x48 };
            let holder = ptr(node + 8)?;
            let next = ptr(holder)?;
            route.extend([(node + 8, holder), (holder, next)]);
            find_mappers(next, base, result, visited, depth + 1, route)?;
        }
        _ => (),
    }
    Some(())
}

fn model_name(item: usize) -> Option<String> {
    let length = ptr(item + 0x6D8)?;
    let capacity = ptr(item + 0x6E0)?;
    if length == 0 || length > 128 || capacity < length {
        return None;
    }
    let address = if capacity < 8 {
        item + 0x6C8
    } else {
        ptr(item + 0x6C8)?
    };
    let mut raw = vec![0u8; length * 2];
    if !read(address, &mut raw) {
        return None;
    }
    String::from_utf16(
        &raw.chunks_exact(2)
            .map(|b| u16::from_le_bytes(b.try_into().unwrap()))
            .collect::<Vec<_>>(),
    )
    .ok()
}

fn mesh_skeleton(key: &Key) -> Option<Vec<Bone>> {
    let count = i32::from_le_bytes(pose::bytes(&read, key.resource + 0x1C)?);
    if count <= 0 || count as usize > pose::LIMIT {
        return None;
    }
    let count = count as usize;
    let skeleton = ptr(key.mappers[0] + 0x90)?;
    let bones = ptr(skeleton + 8)?;
    let reference = ptr(key.resource + 0x2F8)?;
    let mut raw_bones = vec![0u8; count * 64];
    let mut raw_bind = vec![0u8; count * 48];
    if !read(bones, &mut raw_bones) || !read(reference, &mut raw_bind) {
        return None;
    }
    let mut globals = Vec::with_capacity(count);
    for raw in raw_bind.chunks_exact(48) {
        let inverse = pose::affine(raw)?;
        if inverse.determinant().abs() < 1e-12 {
            return None;
        }
        globals.push(inverse.inverse());
    }
    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let bone = &raw_bones[i * 64..(i + 1) * 64];
        let parent = i16::from_le_bytes(bone[0x2C..0x2E].try_into().unwrap());
        if parent < -1 || parent as usize >= count && parent != -1 {
            return None;
        }
        let parent = (parent >= 0).then_some(parent as usize);
        let name_at = usize::from_le_bytes(bone[0x20..0x28].try_into().unwrap());
        result.push(Bone {
            name: pose::text(&read, name_at, true)?,
            parent,
            reference: pose::local(
                parent.map_or(globals[i], |p| globals[p].inverse() * globals[i]),
            )?,
        });
    }
    Some(result)
}

fn bind(key: Key, identity: Identity, generation: u64, scale: f64, force: bool) -> Option<Binding> {
    let (source_identity, source_bones) = pose::skeleton(&read, key.source_meta)?;
    // A valid empty FLVER has no skeleton to retarget. Cache native behavior
    // instead of retrying a permanently absent mesh skeleton every 120 frames.
    // Count and route changes still revoke the entry through NativeBinding.
    if pose::bytes::<4>(&read, key.resource + 0x1C) == Some(0i32.to_le_bytes()) {
        let native = NativeBinding {
            key,
            identity,
            source: source_identity,
            count: 0,
        };
        return native
            .current(identity, BASE.load(Ordering::Acquire))
            .then_some(Binding::Native(Box::new(native)));
    }
    let source_bones = crate::equipment_retarget::animation_reference(source_bones);
    let mesh_bones =
        crate::equipment_retarget::equipment_mesh_reference(&source_bones, &mesh_skeleton(&key)?)
            .ok()?;
    if !force {
        let difference =
            crate::equipment_retarget::proportion_difference(&source_bones, &mesh_bones).ok()?;
        if difference.changed == 0 {
            let native = NativeBinding {
                key,
                identity,
                source: source_identity,
                count: mesh_bones.len(),
            };
            return native
                .current(identity, BASE.load(Ordering::Acquire))
                .then_some(Binding::Native(Box::new(native)));
        }
    }
    let mut mesh_plan = Plan::new(source_bones.clone(), mesh_bones.clone(), &[]).ok()?;
    let motion = MotionProfile::new(&source_bones, &mesh_bones);
    if motion.is_some() {
        mesh_plan.calibrate_limb_directions();
    }
    let (cloth_identity, cloth_bones, cloth_plan) = if key.owner != 0 {
        let (id, bones) = pose::skeleton(&read, key.input_meta)?;
        let bones =
            crate::equipment_retarget::equipment_physics_reference(&mesh_bones, &bones).ok()?;
        let plan = Plan::new(source_bones.clone(), bones.clone(), &[]).ok()?;
        (Some(id), bones, Some(plan))
    } else {
        (None, Vec::new(), None)
    };
    let cloth_names: HashMap<_, _> = cloth_bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let mesh_to_cloth = mesh_bones
        .iter()
        .map(|b| cloth_names.get(b.name.as_str()).copied())
        .collect();
    let n = mesh_bones.len();
    let mesh_names: HashMap<_, _> = mesh_bones
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let cloth_to_mesh = cloth_bones
        .iter()
        .map(|b| mesh_names.get(b.name.as_str()).copied())
        .collect();
    let work = Work {
        grip: weapon::GripCache::default(),
        generation,
        scale,
        prepared: None,
        solver_completed: None,
        source: PoseScratch::default(),
        solved: PoseScratch::default(),
        mesh: mesh_plan.new_frame(),
        cloth: cloth_plan.as_ref().map(Plan::new_frame),
        render: vec![DMat4::IDENTITY; n],
        correction: vec![DMat4::IDENTITY; n],
        affine: vec![[0.0; 12]; n],
        matrix: vec![[0.0; 16]; n],
        overridden: vec![false; n],
        spare: None,
    };
    let session = Session {
        active: AtomicBool::new(true),
        key,
        identity,
        source_identity,
        source_bones,
        cloth_identity,
        cloth_bones,
        mesh_bones,
        mesh_plan,
        motion,
        cloth_plan,
        mesh_to_cloth,
        cloth_to_mesh,
        work: Mutex::new(work),
    };
    session
        .current()
        .then_some(Binding::Retarget(Box::new(session)))
}

/// Called for the verified local player at the existing pre-physics update.
/// Detect proportions only at binding; discovery never scans arbitrary memory.
pub(crate) fn refresh(player: usize, scale: f32, generation: u64) {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    crate::memory_query::scoped(|| {
        let Some(identity) = Identity::capture(player) else {
            return;
        };
        let Some(assembly) = ptr(player + 0x648) else {
            return;
        };
        let base = BASE.load(Ordering::Acquire);
        if ptr(assembly) != Some(base + crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA) {
            return;
        }
        let Some(source) = ptr(player + 0x398) else {
            return;
        };
        let Some(source_meta) = ptr(source + 0x48) else {
            return;
        };
        let Some(settings) = CONFIG.get() else {
            return;
        };
        let Ok(old) = REGISTRY.read() else {
            return;
        };
        let previous = old.clone();
        drop(old);
        let Ok(mut rejected) = REJECTED.lock() else {
            return;
        };
        let Ok(mut native) = NATIVE.lock() else {
            return;
        };
        native.retain(|n| n.current(identity, base));
        rejected.retain(|(key, owner, last)| {
            *owner == identity && generation.saturating_sub(*last) < 120 && key.current(base)
        });
        let mut next = Vec::new();
        for slot_index in 0..crate::cloth_owner_scope::MODEL_SLOT_COUNT {
            let slot = assembly + 0x28 + slot_index * 8;
            if native.iter().any(|n| n.key.slot == slot) {
                continue;
            }
            if let Some(session) = previous
                .iter()
                .find(|s| s.key.slot == slot && s.identity == identity && s.current())
            {
                if let Ok(mut work) = session.work.lock() {
                    if work.scale != f64::from(scale) {
                        work.prepared = None;
                        work.solver_completed = None;
                    }
                    work.generation = generation;
                    work.scale = f64::from(scale);
                }
                next.push(session.clone());
                continue;
            }
            let capture = || -> Option<(Key, bool)> {
                let slot = assembly + 0x28 + slot_index * 8;
                let equipment = ptr(slot)?;
                if ptr(equipment) != Some(base + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA) {
                    return None;
                }
                let item = ptr(equipment + 0x10)?;
                let name = model_name(item)?;
                let force = settings.policy(&name)?;
                let resource = ptr(item + 0x68)?;
                let exporter = ptr(item + 0x658)?;
                if ptr(exporter) != Some(base + 0x2B701A0)
                    || ptr(exporter + 0x100) != Some(resource)
                {
                    return None;
                }
                let mut mappers = Vec::new();
                let mut route = vec![
                    (item + 0x658, exporter),
                    (exporter, base + 0x2B701A0),
                    (exporter + 0x100, resource),
                    (resource + 0x2F8, ptr(resource + 0x2F8)?),
                ];
                let mut visited = HashSet::new();
                for offset in [0x48, 0x68] {
                    let holder = ptr(exporter + offset + 8)?;
                    // A mesh-only exporter has no cloth mapper list. Record
                    // the empty edge so a later list invalidates this binding.
                    if holder == 0 {
                        route.push((exporter + offset + 8, 0));
                        continue;
                    }
                    route.extend([(exporter + offset + 8, holder), (holder, ptr(holder)?)]);
                    find_mappers(
                        provider(exporter + offset)?,
                        base,
                        &mut mappers,
                        &mut visited,
                        0,
                        &mut route,
                    )?;
                }
                if mappers.is_empty() {
                    return None;
                }
                for &mapper in &mappers {
                    let skeleton = ptr(mapper + 0x90)?;
                    route.extend([
                        (mapper + 0x90, skeleton),
                        (skeleton + 8, ptr(skeleton + 8)?),
                    ]);
                }
                let owner = ptr(equipment + 0x130)?;
                let (input, input_meta, inner, core) = if owner != 0 {
                    if ptr(owner) != Some(base + 0x2B92A60) {
                        return None;
                    }
                    let input = ptr(owner + 0x120)?;
                    if input == source
                        || input == identity.cloth_pose
                        || ptr(input) != Some(base + INPUT_VTABLE)
                    {
                        return None;
                    }
                    let inner = ptr(owner + 0x40)?;
                    if ptr(inner) != Some(base + 0x329A2F8) {
                        return None;
                    }
                    (input, ptr(input + 0x48)?, inner, ptr(inner + 0x30)?)
                } else {
                    (0, 0, 0, 0)
                };
                Some((
                    Key {
                        player,
                        assembly,
                        slot,
                        equipment,
                        item,
                        resource,
                        source,
                        source_meta,
                        owner,
                        input,
                        input_meta,
                        inner,
                        core,
                        mappers,
                        route,
                    },
                    force,
                ))
            };
            let Some((key, force)) = capture() else {
                continue;
            };
            if rejected.iter().any(|(bad, _, _)| *bad == key) {
                continue;
            }
            match bind(key.clone(), identity, generation, f64::from(scale), force) {
                Some(Binding::Retarget(session)) => {
                    crate::log::line(format_args!(
                        "[ERCS-RETARGET] bound equipment=0x{:X} mesh_bones={} physics_bones={}",
                        key.equipment,
                        session.mesh_bones.len(),
                        session.cloth_bones.len()
                    ));
                    next.push(Arc::from(session));
                }
                Some(Binding::Native(binding)) => native.push(*binding),
                None => {
                    crate::log::line(format_args!(
                        "[ERCS-RETARGET] deferred equipment=0x{:X} reference-or-identity-invalid retry_frames=120",
                        key.equipment
                    ));
                    rejected.push((key, identity, generation));
                }
            }
        }
        if let Ok(mut registry) = REGISTRY.write() {
            for old in registry
                .iter()
                .filter(|old| !next.iter().any(|new| Arc::ptr_eq(old, new)))
            {
                old.active.store(false, Ordering::Release);
            }
            attachments::refresh(&next, base);
            dummies::refresh(&next, base);
            *registry = next;
        }
    });
}

pub(crate) fn suspend() {
    if !READY.load(Ordering::Acquire) {
        return;
    }
    // Use the same lock order as refresh. Dropping registry entries revokes
    // future lookups; outstanding owned buffers still retain safe storage.
    if let Ok(mut rejected) = REJECTED.lock() {
        rejected.clear();
        if let Ok(mut native) = NATIVE.lock() {
            native.clear();
        }
        if let Ok(mut registry) = REGISTRY.write() {
            for session in registry.iter() {
                session.active.store(false, Ordering::Release);
            }
            registry.clear();
            attachments::clear();
            dummies::clear();
        }
    }
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Qs([f32; 12]);
#[repr(C, align(16))]
struct Header([usize; 8]);

pub(crate) struct SolverInput {
    session: Arc<Session>,
    generation: u64,
    storage: Option<Box<SolverStorage>>,
}

struct SolverStorage {
    header: Header,
    local: Vec<Qs>,
    model: Vec<Qs>,
    flags: Vec<u32>,
}

impl Drop for SolverInput {
    fn drop(&mut self) {
        // Nested calls take a separate buffer. No lock or mutable borrow is
        // held across native code, and capacity is reused only after it returns.
        if let Ok(mut work) = self.session.work.lock() {
            work.spare = self.storage.take();
        }
    }
}

impl SolverInput {
    pub fn completed(&self) {
        if self.session.current()
            && let Ok(mut work) = self.session.work.lock()
            && work.generation == self.generation
        {
            work.solver_completed = Some(self.generation);
        }
    }
    pub fn context(&mut self) -> usize {
        let storage = self.storage.as_mut().unwrap();
        storage.header.0[1] = storage.local.as_mut_ptr() as usize;
        storage.header.0[3] = storage.model.as_mut_ptr() as usize;
        storage.header.0[5] = storage.flags.as_mut_ptr() as usize;
        storage.header.0.as_mut_ptr() as usize
    }
}

fn selected(key: usize, solver: bool) -> Option<Arc<Session>> {
    if !READY.load(Ordering::Acquire) {
        return None;
    }
    let registry = REGISTRY.read().ok()?;
    let mut matches = registry.iter().filter(|s| {
        if solver {
            s.key.inner == key
        } else {
            s.key.mappers.contains(&key)
        }
    });
    let first = matches.next()?.clone();
    if matches.next().is_some() {
        return None;
    }
    Some(first)
}

fn player_motion(player: usize) -> Option<MotionProfile> {
    if !READY.load(Ordering::Acquire) {
        return None;
    }
    let registry = REGISTRY.read().ok()?;
    let mut result: Option<MotionProfile> = None;
    for session in registry.iter().filter(|s| s.key.player == player) {
        let Some(profile) = session.motion else {
            continue;
        };
        if !session.current() {
            continue;
        }
        if let Some(previous) = result
            && ((previous.leg_ratio - profile.leg_ratio).abs() > 1e-4
                || (previous.height_ratio - profile.height_ratio).abs() > 1e-4)
        {
            // Different complete bodies must not arbitrarily compete over
            // one player controller. Partial equipment never owns locomotion.
            return None;
        }
        result = Some(profile);
    }
    result
}

extern "win64" fn motion_hook(registers: *mut Registers) {
    let r = unsafe { &*registers };
    let _ = crate::memory_query::scoped(|| -> Option<()> {
        let controller = r.rbx as usize;
        let address = r.rdi as usize;
        if address != controller.checked_add(0x140)? {
            return None;
        }
        let player = ptr(controller + 0x10)?;
        if ptr(player + 0x58) != Some(controller) {
            return None;
        }
        let profile = player_motion(player)?;
        let raw = pose::bytes::<16>(&read, address)?;
        let value =
            std::array::from_fn(|i| f32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap()));
        let output = profile.displacement(value)?;
        if !crate::memory_query::accessible_span(address, 12, true) {
            return None;
        }
        // Only animation translation XYZ. Quaternion, homogeneous lane,
        // gravity, collision shape and stick input are not part of this data.
        unsafe {
            std::ptr::copy_nonoverlapping(output.as_ptr(), address as *mut f32, 3);
        }
        Some(())
    });
}

extern "win64" fn camera_hook(registers: *mut Registers) {
    let r = unsafe { &mut *registers };
    let _ = crate::memory_query::scoped(|| -> Option<()> {
        let base = BASE.load(Ordering::Acquire);
        let world = ptr(base + 0x3D69FF8)?;
        let manager = ptr(base + 0x3D6D988)?;
        adjust_camera(r, base, world, manager)
    });
}

fn adjust_camera(r: &mut Registers, base: usize, world: usize, manager: usize) -> Option<()> {
    let player = ptr(world + 0x1E508)?;
    // Sole native caller keeps its followed character in R15.
    if r.r15 as usize != player {
        return None;
    }
    let cam = ptr(world + 0x1ECE0)?;
    let follow = ptr(cam + 0x60)?;
    if r.rbx as usize != follow || ptr(follow) != Some(base + 0x2A2ACA0) {
        return None;
    }
    let override_id = i32::from_le_bytes(pose::bytes(&read, manager + 0x50)?);
    if override_id >= 0 {
        return None;
    }
    let profile = player_motion(player)?;
    let height = f32::from_bits(r.xmm3 as u32);
    let height = profile.camera_height(height)?;
    // Change the consumed register, never persistently edit camera params.
    r.xmm3 = (r.xmm3 & !u128::from(u32::MAX)) | u128::from(height.to_bits());
    Some(())
}

/// Owned input remains alive through the original synchronous native call.
/// The native solver retains its authored constraints and free-particle state.
pub(crate) fn solver_input(inner: usize, context: usize, transform: usize) -> Option<SolverInput> {
    let session = selected(inner, true)?;
    crate::memory_query::scoped(|| {
        if context != session.key.input + 0x48
            || transform != session.key.owner + 0x60
            || !matches!(pose::bytes::<1>(&read, inner + 0x60), Some([v]) if v != 0)
            || !session.current()
        {
            return None;
        }
        let raw: [u8; 64] = pose::bytes(&read, context)?;
        let mut header = Header(std::array::from_fn(|i| {
            usize::from_le_bytes(raw[i * 8..i * 8 + 8].try_into().unwrap())
        }));
        let mut work = session.work.lock().ok()?;
        work.solver_completed = None;
        session.prepare(&mut work)?;
        let mut storage = work.spare.take().unwrap_or_else(|| {
            Box::new(SolverStorage {
                header: Header([0; 8]),
                local: Vec::new(),
                model: Vec::new(),
                flags: Vec::new(),
            })
        });
        let output = work.cloth.as_ref()?.outputs()?;
        storage.local.clear();
        storage.model.clear();
        // The source is in model-local units. Existing uniform scaling applies
        // the same native source convention used by the ordinary cloth path.
        for transform in output.local {
            let mut transform = *transform;
            transform.translation *= work.scale;
            storage.local.push(Qs(pose::qs_output(transform)?));
        }
        let n = output.model.len();
        storage.model.resize(n, Qs([0.0; 12]));
        storage.flags.resize(n, 0);
        storage.flags.fill(0);
        for &i in session.cloth_plan.as_ref()?.target_order() {
            let parent_dirty = session.cloth_bones[i]
                .parent
                .is_some_and(|p| storage.flags[p] & 2 != 0);
            let (model, flag) =
                pose::solver_model_cache(output.model[i], parent_dirty, work.scale)?;
            storage.model[i] = Qs(model);
            storage.flags[i] = flag;
        }
        // hkArray stores size + capacityAndFlags. Storage belongs to Rust;
        // mark it non-owning instead of advertising size n with capacity zero.
        let array_size_capacity = n | ((n | 0x8000_0000) << 32);
        header.0[2] = array_size_capacity;
        header.0[4] = array_size_capacity;
        header.0[6] = array_size_capacity;
        // 26B1944 tests each model-dirty bit before reading +18; 26B195F
        // calls hkaPose::calculateBoneModelSpace (1654A10) for lazy entries.
        // Leave local valid and mark the aggregate model cache out of sync.
        let model_sync = if storage.flags.iter().all(|&f| f == 0) {
            0x100
        } else {
            0
        };
        header.0[7] = (header.0[7] & !0xffff) | 1 | model_sync;
        if !session.current() {
            return None;
        }
        storage.header = header;
        Some(SolverInput {
            session: session.clone(),
            generation: work.generation,
            storage: Some(storage),
        })
    })
}

fn render_hook(registers: *mut Registers, original: usize) -> usize {
    render_range(registers, original, false)
}

fn matrix_render_hook(registers: *mut Registers, original: usize) -> usize {
    render_range(registers, original, true)
}

fn render_range(registers: *mut Registers, original: usize, matrix4: bool) -> usize {
    let stride = if matrix4 { 64 } else { 48 };
    let r = unsafe { &*registers };
    let this = r.rcx as usize;
    let address = r.rdx as usize;
    let requested = r.r8 as u32;
    let start = r.r9 as u32;
    let native: unsafe extern "C" fn(usize, usize, u32, u32) -> usize =
        unsafe { std::mem::transmute(original) };
    let result = unsafe { native(this, address, requested, start) };
    if result == 0 {
        return result;
    }
    let Some(session) = selected(this, false) else {
        return result;
    };
    let _ = crate::memory_query::scoped(|| -> Option<()> {
        if !session.current() {
            return None;
        }
        let count = result.min(requested as usize);
        if count == 0 {
            return None;
        }
        let end = (start as usize).checked_add(count)?;
        // B47D70 indexes output by the absolute bone index (R13 + EDI * 48),
        // including nonzero start. It does not return a packed subrange.
        let address = address.checked_add((start as usize).checked_mul(stride)?)?;
        if end > session.mesh_bones.len()
            || !crate::memory_query::accessible_span(address, count.checked_mul(stride)?, true)
        {
            return None;
        }
        let mut work = session.work.lock().ok()?;
        // Cloth must consume the target pose before this frame is displayed.
        if session.key.owner != 0 && work.solver_completed != Some(work.generation) {
            return None;
        }
        session.prepare(&mut work)?;
        let Work { mesh, render, .. } = &mut *work;
        render.copy_from_slice(mesh.outputs()?.model);
        work.correction.fill(DMat4::IDENTITY);
        work.overridden.fill(false);
        if let Some(identity) = &session.cloth_identity {
            let mask = crate::cloth_render_scale::capture_mask(
                session.key.core,
                session.cloth_bones.len(),
                &read,
            )?;
            work.solved.capture_model(
                &read,
                session.key.input + 0x48,
                identity,
                &session.cloth_bones,
                1.0,
            )?;
            // Native writeback scale convention is asymmetric; reconcile only
            // rows explicitly written by the solver, never infer from magnitude.
            for (i, mapped) in session.mesh_to_cloth.iter().enumerate() {
                if let Some(source) = mapped.filter(|&index| mask.contains(index)) {
                    let mut m = work.solved.model[source];
                    m.x_axis *= work.scale;
                    m.y_axis *= work.scale;
                    m.z_axis *= work.scale;
                    m.w_axis.x /= work.scale;
                    m.w_axis.y /= work.scale;
                    m.w_axis.z /= work.scale;
                    work.render[i] = m;
                    work.overridden[i] = true;
                }
            }
        }
        // Mesh skeletons can be stored out of parent order. A bounded
        // topological walk carries a simulated parent's correction to children.
        for &i in session.mesh_plan.target_order() {
            let base = work.mesh.outputs()?.model[i];
            if work.overridden[i] {
                work.correction[i] = work.render[i] * base.inverse();
            } else if let Some(parent) = session.mesh_bones[i].parent {
                work.correction[i] = work.correction[parent];
                work.render[i] = work.correction[i] * base;
            }
            work.affine[i] = pose::affine_output(work.render[i])?;
            if matrix4 {
                work.matrix[i] = work.render[i].as_mat4().to_cols_array();
            }
        }
        if !session.current() || !render_output_is_private(&session.key, address, count * stride) {
            return None;
        }
        // Only the caller's fresh copied result is written, never a pose cache.
        unsafe {
            std::ptr::copy_nonoverlapping(
                if matrix4 {
                    work.matrix[start as usize..end].as_ptr().cast::<u8>()
                } else {
                    work.affine[start as usize..end].as_ptr().cast::<u8>()
                },
                address as *mut u8,
                count * stride,
            );
        }
        Some(())
    });
    result
}

fn render_output_is_private(key: &Key, address: usize, length: usize) -> bool {
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    let separate = |pointer: usize, bytes: usize| {
        pointer
            .checked_add(bytes)
            .is_some_and(|other_end| address >= other_end || pointer >= end)
    };
    for input in [key.source, key.input].into_iter().filter(|p| *p != 0) {
        if !separate(input, 0x88) {
            return false;
        }
        let Some(meta) = ptr(input + 0x48) else {
            return false;
        };
        if !separate(meta, 0x50) {
            return false;
        }
        for (offset, stride) in [(0x20, 2usize), (0x30, 16), (0x40, 48)] {
            let Some(pointer) = ptr(meta + offset) else {
                return false;
            };
            let Some(raw) = pose::bytes::<4>(&read, meta + offset + 8) else {
                return false;
            };
            let Ok(count) = usize::try_from(i32::from_le_bytes(raw)) else {
                return false;
            };
            if count > pose::LIMIT || !separate(pointer, count * stride) {
                return false;
            }
        }
        for offset in [8usize, 0x18, 0x28] {
            let Some(pointer) = ptr(input + 0x48 + offset) else {
                return false;
            };
            let Some(raw) = pose::bytes::<4>(&read, input + 0x48 + offset + 8) else {
                return false;
            };
            let Ok(count) = usize::try_from(i32::from_le_bytes(raw)) else {
                return false;
            };
            if count > pose::LIMIT {
                return false;
            }
            let Some(other_end) = count
                .checked_mul(if offset == 0x28 { 4 } else { 48 })
                .and_then(|n| pointer.checked_add(n))
            else {
                return false;
            };
            if address < other_end && pointer < end {
                return false;
            }
        }
    }
    let Some(raw) = pose::bytes::<4>(&read, key.resource + 0x1C) else {
        return false;
    };
    let Ok(count) = usize::try_from(i32::from_le_bytes(raw)) else {
        return false;
    };
    let Some(reference) = ptr(key.resource + 0x2F8) else {
        return false;
    };
    if count > pose::LIMIT || !separate(reference, count * 48) {
        return false;
    }
    for mapper in &key.mappers {
        let Some(bones) = ptr(mapper + 0x90).and_then(|s| ptr(s + 8)) else {
            return false;
        };
        if !separate(bones, count * 64) {
            return false;
        }
    }
    true
}

#[cfg(test)]
#[path = "equipment_retarget_native_tests.rs"]
mod tests;
