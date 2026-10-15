//! Temporary [ERPS-SYNC253] one-shot recorder. Engine memory is read-only.
//! The native call is NOT inside a memory-query scope or recorder mutex. The
//! writer consumes owned copies only; no engine pointer is dereferenced there.
use ilhook::x64::{CallbackOption, HookFlags, Registers, hook_closure_jmp_back, hook_closure_retn};
use std::{
    cell::{Cell, RefCell},
    fs::OpenOptions,
    io::{self, BufWriter, Write},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
#[path = "cloth_diagnostic_step.rs"]
mod ordered;

pub(crate) const COUNTS: [usize; 4] = [347, 160, 448, 289];
const STAGES: usize = 10;
const PER_STAGE: usize = 3;
const RECORDS: usize = 4 * STAGES * PER_STAGE;
const BYTES: usize = 96 * 1024;
const BLOCKS: usize = 192;
// Ordered profile2 measures this target without discarding a complete step.
// Legacy samplers retain their own cutoff behavior and are disabled in profile2.
const COPY_BUDGET_US: u64 = 4_000;
const SAMPLES_PER_FRAME: u64 = 1;
const SKIN_BUDGET_US: u64 = 2_000;
const SKIN_START_MS: u64 = 1200;
const SKIN_OBSERVATIONS: u64 = 3;
const LINK_FRAME_BUDGET_US: u64 = 500;
const WINDOW: Duration = Duration::from_secs(5);
//2.46 established the active branch. Detailed stage8 replaces discovery;
// do not install the eight extra hooks or read their lists again in2.47.
const CAPTURE_DISPATCH_SETS: bool = false;
fn dispatch_capture_enabled(stage: usize) -> bool {
    CAPTURE_DISPATCH_SETS && stage == 7
}
const NAMES: [&str; STAGES] = [
    "collider_map",
    "standard_link",
    "compressible_link",
    "stretch_link",
    "local_range",
    "bend_stiffness",
    "transition",
    "collision_pass",
    "contact_capsule",
    "contact_tapered",
];
// Constraint virtual +0x30, Win64 (this, child, XMM2 strength, R9 flags).
const RVAS: [usize; STAGES] = [
    0x15DC260, 0x1584480, 0x15A1B20, 0x1581640, 0x159B040, 0x157FCE0, 0x1585B10, 0x1602880,
    0x15F0DE0, 0x16009B0,
];
const VTABLES: [usize; STAGES] = [
    0, 0x2D89D58, 0x2D7E360, 0x2D89480, 0x2D7EEE0, 0x2D88BD8, 0x2D8A1F8, 0, 0, 0,
];
const PROLOGUES: [&[u8]; STAGES] = [
    &[
        0x4C, 0x8B, 0xDC, 0x48, 0x81, 0xEC, 0x88, 0x02, 0, 0, 0x44, 0x8B, 0x81, 0x70, 1, 0, 0,
    ],
    &[
        0x40, 0x57, 0x48, 0x83, 0xEC, 0x50, 0x0F, 0x29, 0x74, 0x24, 0x40, 0x0F, 0x57, 0xC0,
    ],
    &[
        0x40, 0x57, 0x48, 0x83, 0xEC, 0x50, 0x0F, 0x29, 0x74, 0x24, 0x40, 0x0F, 0x57, 0xC0,
    ],
    &[
        0x40, 0x57, 0x48, 0x83, 0xEC, 0x50, 0x0F, 0x29, 0x74, 0x24, 0x40, 0x0F, 0x57, 0xC0,
    ],
    &[
        0x48, 0x8B, 0xC4, 0x53, 0x41, 0x56, 0x48, 0x83, 0xEC, 0x58, 0x0F, 0x29, 0x70, 0xD8,
    ],
    &[
        0x48, 0x8B, 0xC4, 0x53, 0x48, 0x83, 0xEC, 0x60, 0x0F, 0x29, 0x70, 0xE8, 0x0F, 0x57, 0xC0,
    ],
    &[
        0x48, 0x89, 0x5C, 0x24, 0x18, 0x56, 0x48, 0x83, 0xEC, 0x30, 0x48, 0x8B, 0x82, 0x68, 2, 0, 0,
    ],
    &[
        0x48, 0x8B, 0xC4, 0x44, 0x88, 0x40, 0x18, 0x88, 0x50, 0x10, 0x48, 0x89, 0x48, 0x08,
    ],
    &[
        0x48, 0x8B, 0xC4, 0x4C, 0x89, 0x48, 0x20, 0x4C, 0x89, 0x40, 0x18, 0x48, 0x89, 0x50, 0x10,
    ],
    &[
        0x48, 0x8B, 0xC4, 0x4C, 0x89, 0x48, 0x20, 0x4C, 0x89, 0x40, 0x18, 0x48, 0x89, 0x50, 0x10,
    ],
];
// Audited ABI/consumer seams. In particular +190 is the copied collidable
// array actually consumed, NOT the original pointer array at child+168.
const CONTACT_SEAMS: &[(usize, &[u8])] = &[
    (0x1602930, &[0x48, 0x8B, 0x92, 0x90, 0x01, 0, 0]),
    (0x1602979, &[0x80, 0xBD, 0xC0, 0x07, 0, 0, 0]),
    (0x16040A0, &[0x85, 0x32, 0x74, 0x30]),
    (
        0x1603669,
        &[
            0x48, 0x8D, 0x44, 0x24, 0x40, 0x49, 0x8B, 0xCF, 0x48, 0x89, 0x44, 0x24, 0x38, 0x4C,
            0x8D, 0x8D, 0xF8, 0x01, 0x00, 0x00, 0x48, 0x8D, 0x44, 0x24, 0x40, 0x48, 0x89, 0x44,
            0x24, 0x30, 0x4C, 0x8D, 0x85, 0x18, 0x02, 0x00, 0x00, 0x48, 0x8B, 0x85, 0xA0, 0x07,
            0x00, 0x00, 0x48, 0x8D, 0x55, 0x98, 0xF3, 0x0F, 0x11, 0x7C, 0x24, 0x28, 0x48, 0x89,
            0x44, 0x24, 0x20, 0xE8, 0x37, 0xD7, 0xFE, 0xFF,
        ],
    ),
    (0x1604516, &[0xE8, 0x95, 0xC4, 0xFF, 0xFF]),
    (0x1600A2D, &[0x49, 0x8B, 0x45, 0x30, 0x4D, 0x8B, 0x4D, 0x20]),
    (0x1600B37, &[0xE8, 0x94, 0x0C, 0xFD, 0xFF]),
];

const DISPATCH_SITES: &[(usize, &[u8])] = &[
    (
        0x1602DD7,
        &[
            0xE8, 0xA4, 0xE9, 0xFE, 0xFF, 0x8B, 0x0D, 0x6E, 0xBF, 0x1D, 0x03, 0xFF, 0x15, 0xA4,
            0xE7, 0x60,
        ],
    ),
    (
        0x160305B,
        &[
            0xE8, 0x00, 0xD3, 0xFE, 0xFF, 0x8B, 0x0D, 0xEA, 0xBC, 0x1D, 0x03, 0xFF, 0x15, 0x20,
            0xE5, 0x60,
        ],
    ),
    (
        0x16033D0,
        &[
            0xE8, 0xEB, 0xF0, 0xFE, 0xFF, 0x8B, 0x0D, 0x75, 0xB9, 0x1D, 0x03, 0xFF, 0x15, 0xAB,
            0xE1, 0x60,
        ],
    ),
    (
        0x16036A4,
        &[
            0xE8, 0x37, 0xD7, 0xFE, 0xFF, 0x8B, 0x0D, 0xA1, 0xB6, 0x1D, 0x03, 0xFF, 0x15, 0xD7,
            0xDE, 0x60,
        ],
    ),
    (
        0x1603C37,
        &[
            0xE8, 0x04, 0xD4, 0xFF, 0xFF, 0x8B, 0x0D, 0x0E, 0xB1, 0x1D, 0x03, 0xFF, 0x15, 0x44,
            0xD9, 0x60,
        ],
    ),
    (
        0x1603EBB,
        &[
            0xE8, 0x10, 0xC4, 0xFF, 0xFF, 0x8B, 0x0D, 0x8A, 0xAE, 0x1D, 0x03, 0xFF, 0x15, 0xC0,
            0xD6, 0x60,
        ],
    ),
    (
        0x1604292,
        &[
            0xE8, 0x59, 0xD8, 0xFF, 0xFF, 0x8B, 0x0D, 0xB3, 0xAA, 0x1D, 0x03, 0xFF, 0x15, 0xE9,
            0xD2, 0x60,
        ],
    ),
    (
        0x1604516,
        &[
            0xE8, 0x95, 0xC4, 0xFF, 0xFF, 0x8B, 0x0D, 0x2F, 0xA8, 0x1D, 0x03, 0xFF, 0x15, 0x65,
            0xD0, 0x60,
        ],
    ),
];

#[derive(Clone, Copy, Default)]
struct DispatchEvent {
    site: usize,
    collider: usize,
    count: usize,
    bits: [u64; 8],
}
struct CollisionDispatch {
    particles: usize,
    colliders: usize,
    collider_count: usize,
    events: [DispatchEvent; 64],
    len: usize,
    observer_us: u64,
    error: &'static str,
    failure: [usize; 3],
}
impl CollisionDispatch {
    fn new(particles: usize, colliders: usize, collider_count: usize) -> Self {
        Self {
            particles,
            colliders,
            collider_count,
            events: [DispatchEvent::default(); 64],
            len: 0,
            observer_us: 0,
            error: "none",
            failure: [0; 3],
        }
    }
    fn observe(&mut self, site: usize, collider: usize, list: usize) {
        if self.error != "none" {
            return;
        }
        self.failure = [DISPATCH_SITES.get(site).map_or(0, |s| s.0), collider, list];
        if self.len == self.events.len() {
            self.error = "event_cap";
            return;
        }
        if self.observer_us >= 500 {
            self.error = "observer_budget";
            return;
        }
        let timer = Instant::now();
        let event = crate::memory_query::scoped(|| {
            let delta = collider.checked_sub(self.colliders)?;
            if site >= DISPATCH_SITES.len()
                || !delta.is_multiple_of(0xA0)
                || delta / 0xA0 >= self.collider_count
            {
                return None;
            }
            let n = count(list.checked_add(8)?, self.particles)?;
            let address = ptr(list)?;
            if n == 0 || self.particles > 512 {
                return None;
            }
            crate::memory_query::accessible_region(address, n.checked_mul(2)?, false)?;
            let mut e = DispatchEvent {
                site,
                collider: delta / 0xA0,
                count: n,
                ..Default::default()
            };
            for j in 0..n {
                let index = unsafe { ((address + j * 2) as *const u16).read_unaligned() } as usize;
                if index >= self.particles || e.bits[index / 64] & (1u64 << (index % 64)) != 0 {
                    return None;
                }
                e.bits[index / 64] |= 1u64 << (index % 64);
            }
            Some(e)
        });
        self.observer_us += timer.elapsed().as_micros() as u64;
        if let Some(e) = event {
            self.events[self.len] = e;
            self.len += 1;
        } else {
            self.error = "dispatch_identity_or_indices";
        }
        if self.observer_us >= 500 {
            self.error = "observer_budget";
        }
        if self.error == "none" {
            self.failure = [0; 3];
        }
    }
}
thread_local! {static COLLISION_DISPATCH: RefCell<Option<CollisionDispatch>> = const {RefCell::new(None)};}
struct DispatchScope {
    previous: Option<CollisionDispatch>,
    active: bool,
}
impl DispatchScope {
    fn enter(trace: Option<CollisionDispatch>) -> Self {
        let mut previous = COLLISION_DISPATCH.with(|t| t.replace(trace));
        if let Some(p) = previous.as_mut() {
            p.error = "nested_pass";
        }
        Self {
            previous,
            active: true,
        }
    }
    fn finish(mut self) -> Option<CollisionDispatch> {
        self.active = false;
        COLLISION_DISPATCH.with(|t| t.replace(self.previous.take()))
    }
}
impl Drop for DispatchScope {
    fn drop(&mut self) {
        if self.active {
            COLLISION_DISPATCH.with(|t| t.replace(self.previous.take()));
        }
    }
}
fn observe_dispatch(site: usize, registers: *mut Registers) {
    COLLISION_DISPATCH.with(|t| {
        if let Ok(mut trace) = t.try_borrow_mut()
            && let Some(trace) = trace.as_mut()
        {
            let r = unsafe { &*registers };
            trace.observe(site, r.rcx as usize, r.rdx as usize);
        }
    });
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Identity {
    pub base: usize,
    pub child: usize,
    pub sim: usize,
    pub root: usize,
    pub model: usize,
    pub owner: usize,
    pub input: usize,
    pub generation: u64,
    pub scale_bits: u32,
    pub count: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Layout {
    current: usize,
    previous: usize,
    particles: usize,
    colliders: usize,
    collider_count: usize,
    consumer_colliders: usize,
    consumer_count: usize,
}
#[derive(Clone, Copy, Default)]
struct ContactArgs {
    indices: usize,
    radii: usize,
    friction: usize,
}
#[derive(Default)]
struct SkinCoverage {
    op: usize,
    root: usize,
    buffer: usize,
    vertices: usize,
    calls: u64,
    pre_rejected: u64,
    post_rejected: u64,
    corrected_calls: u64,
    corrected_rows: usize,
    metadata_rejected: u64,
    guard_samples: [Option<SkinGuardSample>; SKIN_OBSERVATIONS as usize],
    latest: Option<SkinLink>,
}
#[derive(Clone, Copy)]
struct SkinLink {
    op: usize,
    root: usize,
    buffer: usize,
    generation: u64,
    scale_bits: u32,
    captured_us: u64,
    sample: SkinGuardSample,
    probes: Option<[[u32; 3]; 3]>,
    duplicate: bool,
}
fn read_normal_points(
    pre: crate::body_scale_port::SkinNormalTrace,
    post: Option<crate::body_scale_port::SkinNormalTrace>,
) -> Option<[[u32; 3]; 3]> {
    let post = post.filter(|p| p.gate == "corrected")?;
    if pre.gate != "accepted" || pre.expected != post.expected {
        return None;
    }
    let [address, count, stride, written] = post.observed;
    if address == 0 || count == 0 || count > 1024 || count != written || ![12, 16].contains(&stride)
    {
        return None;
    }
    let mut result = [[0; 3]; 3];
    for (out, index) in result.iter_mut().zip([0, count / 2, count - 1]) {
        let at = address.checked_add(index.checked_mul(stride)?)?;
        crate::memory_query::accessible_region(at, 12, false)?;
        *out = unsafe { (at as *const [u32; 3]).read_unaligned() };
    }
    Some(result)
}
#[derive(Clone, Copy)]
struct SkinGuardSample {
    frame: u64,
    pre: crate::body_scale_port::SkinNormalTrace,
    post: Option<crate::body_scale_port::SkinNormalTrace>,
}
impl SkinCoverage {
    fn observe(&mut self, pre: bool, rows: Option<usize>, sample: SkinGuardSample) -> bool {
        if self.calls >= SKIN_OBSERVATIONS {
            return false;
        }
        self.guard_samples[self.calls as usize] = Some(sample);
        self.calls += 1;
        self.pre_rejected += u64::from(!pre);
        self.post_rejected += u64::from(pre && rows.is_none());
        if let Some(rows) = rows {
            self.corrected_calls += 1;
            self.corrected_rows += rows;
        }
        true
    }
}
struct Block {
    name: &'static str,
    index: usize,
    address: usize,
    start: usize,
    length: usize,
}
struct Record {
    id: usize,
    stage: usize,
    frame: u64,
    identity: Identity,
    object: usize,
    strength_bits: u32,
    flags: usize,
    contact: ContactArgs,
    layout: Layout,
    valid: bool,
    error: &'static str,
    before_us: u64,
    after_us: u64,
    identity_before_us: u64,
    identity_after_us: u64,
    before_queries: u64,
    after_queries: u64,
    before_copy_reads: u64,
    after_copy_reads: u64,
    native_enter_us: u64,
    native_exit_us: u64,
    data: Vec<u8>,
    blocks: Vec<Block>,
    skin_links: [Option<SkinLink>; 16],
    skin_read_start_us: u64,
    collision_dispatch: Option<CollisionDispatch>,
}
impl Record {
    fn new() -> Self {
        Self {
            id: 0,
            stage: 0,
            frame: 0,
            identity: Identity::default(),
            object: 0,
            strength_bits: 0,
            flags: 0,
            contact: ContactArgs::default(),
            layout: Layout::default(),
            valid: false,
            error: "not_captured",
            before_us: 0,
            after_us: 0,
            identity_before_us: 0,
            identity_after_us: 0,
            before_queries: 0,
            after_queries: 0,
            before_copy_reads: 0,
            after_copy_reads: 0,
            native_enter_us: 0,
            native_exit_us: 0,
            data: Vec::with_capacity(BYTES),
            blocks: Vec::with_capacity(BLOCKS),
            skin_links: [None; 16],
            skin_read_start_us: 0,
            collision_dispatch: None,
        }
    }
    fn copy(
        &mut self,
        name: &'static str,
        index: usize,
        address: usize,
        length: usize,
    ) -> Option<()> {
        if self.blocks.len() >= BLOCKS || length > BYTES.checked_sub(self.data.len())? {
            return None;
        }
        #[cfg(test)]
        COPY_TEST_DELAY_MS.with(|delay| {
            let ms = delay.replace(0);
            if ms != 0 {
                std::thread::sleep(Duration::from_millis(ms));
            }
        });
        let start = self.data.len();
        if length != 0 && COPY_READ_ACTIVE.with(Cell::get) {
            self.data.resize(start + length, 0);
            if read_current_process(address, &mut self.data[start..]).is_none() {
                self.data.truncate(start);
                return None;
            }
        } else if length != 0 {
            crate::memory_query::accessible_region(address, length, false)?;
            // A scoped/native-phase guarded read, not a lifetime guarantee from VirtualQuery.
            self.data.extend_from_slice(unsafe {
                std::slice::from_raw_parts(address as *const u8, length)
            });
        }
        self.blocks.push(Block {
            name,
            index,
            address,
            start,
            length,
        });
        Some(())
    }
}
struct Slot {
    state: AtomicU8,
    record: Mutex<Option<Record>>,
}
// A single preallocated snapshot at the already ownership-checked meshPN seam.
// This is part of the existing recorder, not another hook/recorder framework.
// It remains independently readable when the later primary pair overruns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MeshBoundarySource {
    pub op: usize,
    pub input_buffer: usize,
    pub output_buffer: usize,
    pub positions: usize,
    pub particles: usize,
    pub frames: usize,
    pub frame_count: usize,
    pub binds: usize,
    pub model: usize,
    pub owner: usize,
    pub input: usize,
    pub generation: u64,
    pub scale_bits: u32,
}
struct MeshBoundary {
    source: Option<MeshBoundarySource>,
    record: Record,
    complete: bool,
    paired: bool,
    consumer_id: Option<usize>,
    copy_us: u64,
    error: &'static str,
}
struct ScalarSkinReach {
    op: usize,
    root: usize,
    sample: SkinGuardSample,
}
impl MeshBoundary {
    fn new() -> Self {
        Self {
            source: None,
            record: Record::new(),
            complete: false,
            paired: false,
            consumer_id: None,
            copy_us: 0,
            error: "not_reached",
        }
    }
    fn start(&mut self, source: MeshBoundarySource, identity: Identity, frame: u64) -> Option<()> {
        if self.source.is_some()
            || source.particles != 448
            || source.frame_count != 570
            || identity.count != 289
            || source.scale_bits != identity.scale_bits
            || source.generation != identity.generation
            || source.model != identity.model
            || source.owner != identity.owner
            || source.input != identity.input
        {
            return None;
        }
        self.source = Some(source);
        self.record.identity = identity;
        self.record.frame = frame;
        self.error = "frame_copy_guard";
        self.record
            .copy("mesh_input_positions", 0, source.positions, 448 * 16)?;
        self.record
            .copy("mesh_corrected_frames", 0, source.frames, 570 * 64)?;
        self.record.copy("mesh_binds", 0, source.binds, 570 * 64)?;
        self.record
            .copy("mesh_input_header", 0, source.input_buffer, 0x118)?;
        self.record
            .copy("mesh_output_header_before", 0, source.output_buffer, 0x118)?;
        self.error = "awaiting_mesh_output";
        Some(())
    }
    fn finish(&mut self, op: usize, output: usize, normals: usize, frame: u64) -> Option<()> {
        let source = self.source?;
        if self.error != "awaiting_mesh_output"
            || source.op != op
            || source.output_buffer != output
            || frame != self.record.frame
        {
            return None;
        }
        self.error = "output_copy_guard";
        if count(output + 0x20, 289)? != 289
            || byte(output + 0x24)? != 16
            || byte(output + 0x4C)? != 16
            || ptr(output + 0x40)? != normals
        {
            return None;
        }
        self.record
            .copy("mesh_output_positions", 0, ptr(output + 0x18)?, 289 * 16)?;
        self.record
            .copy("mesh_output_normals", 0, normals, 289 * 16)?;
        self.record
            .copy("mesh_output_header_after", 0, output, 0x118)?;
        self.complete = true;
        self.error = "none";
        Some(())
    }
}
struct Session {
    ordered: ordered::Capture,
    slots: Vec<Slot>,
    phase: AtomicU8,
    reason: AtomicU8,
    next: AtomicUsize,
    in_flight: AtomicUsize,
    frame: AtomicU64,
    frame_budget: AtomicU64,
    counts: [AtomicUsize; 4 * STAGES],
    last_frame: [AtomicU64; 4 * STAGES],
    skin: Mutex<Vec<SkinCoverage>>,
    skin_dropped: AtomicUsize,
    skin_max_us: AtomicU64,
    skin_budget_exceeded: AtomicBool,
    skin_busy: AtomicBool,
    link_budget: AtomicU64,
    targets: OnceLock<[Identity; 4]>,
    started: OnceLock<Instant>,
    candidate: Mutex<Option<[Identity; 4]>>,
    mesh_boundary: Mutex<MeshBoundary>,
    scalar_skin_reach: Mutex<Vec<ScalarSkinReach>>,
}
static SESSION: OnceLock<Session> = OnceLock::new();
fn stage_allowed(stage: usize, elapsed: Duration) -> bool {
    if elapsed >= WINDOW {
        false
    } else if elapsed < Duration::from_millis(600) {
        (8..STAGES).contains(&stage)
    } else if elapsed < Duration::from_millis(1200) {
        stage == 7
    } else {
        stage < STAGES
    }
}
fn skin_needs_observation(coverage: &[SkinCoverage], op: usize, root: usize) -> bool {
    coverage
        .iter()
        .find(|c| c.op == op && c.root == root)
        .is_none_or(|c| c.calls < SKIN_OBSERVATIONS)
}
impl Session {
    fn new() -> Self {
        Self {
            ordered: ordered::Capture::new(),
            slots: (0..RECORDS)
                .map(|_| Slot {
                    state: AtomicU8::new(0),
                    record: Mutex::new(Some(Record::new())),
                })
                .collect(),
            phase: AtomicU8::new(0),
            reason: AtomicU8::new(0),
            next: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            frame: AtomicU64::new(0),
            frame_budget: AtomicU64::new(0),
            counts: [const { AtomicUsize::new(0) }; 4 * STAGES],
            last_frame: [const { AtomicU64::new(0) }; 4 * STAGES],
            skin: Mutex::new(Vec::with_capacity(16)),
            skin_dropped: AtomicUsize::new(0),
            skin_max_us: AtomicU64::new(0),
            skin_budget_exceeded: AtomicBool::new(false),
            skin_busy: AtomicBool::new(false),
            link_budget: AtomicU64::new(0),
            targets: OnceLock::new(),
            started: OnceLock::new(),
            candidate: Mutex::new(None),
            mesh_boundary: Mutex::new(MeshBoundary::new()),
            scalar_skin_reach: Mutex::new(Vec::with_capacity(16)),
        }
    }
    fn stop(&self, reason: u8) {
        let _ = self
            .reason
            .compare_exchange(0, reason, Ordering::AcqRel, Ordering::Acquire);
        self.phase.store(3, Ordering::Release);
    }
    fn finish_skin_observation(&self, elapsed: u64) {
        self.skin_max_us.fetch_max(elapsed, Ordering::Relaxed);
        if elapsed > SKIN_BUDGET_US {
            // This auxiliary channel may disable itself,not erase primary
            // collision evidence. Never rearm it in this one-shot session.
            self.skin_budget_exceeded.store(true, Ordering::Release);
        }
    }
    fn admit_skin(&self, elapsed: Duration) -> Option<SkinAdmission<'_>> {
        if self.phase.load(Ordering::Acquire) != 2
            || elapsed < Duration::from_millis(SKIN_START_MS)
            || elapsed >= WINDOW
            || self.skin_budget_exceeded.load(Ordering::Acquire)
        {
            return None;
        }
        if self
            .skin_busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            self.skin_dropped.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        let admission = SkinAdmission(self);
        // An in-flight observer/stop may have finished between the checks.
        if self.phase.load(Ordering::Acquire) != 2
            || self.skin_budget_exceeded.load(Ordering::Acquire)
        {
            return None;
        }
        Some(admission)
    }
    fn reserve(&self, bucket: usize, stage: usize, frame: u64) -> Option<usize> {
        let key = bucket * STAGES + stage;
        if self.counts[key].load(Ordering::Relaxed) >= PER_STAGE {
            return None;
        }
        let old = self.last_frame[key].load(Ordering::Acquire);
        if old >= frame
            || self.last_frame[key]
                .compare_exchange(old, frame, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return None;
        }
        // At most ONE pair per task frame across all native hooks/threads.
        self.frame_budget
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                if n >> 8 > frame {
                    return None;
                }
                let used = if n >> 8 == frame { n & 255 } else { 0 };
                (used < SAMPLES_PER_FRAME).then_some((frame << 8) | (used + 1))
            })
            .ok()?;
        self.counts[key]
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < PER_STAGE).then_some(n + 1)
            })
            .ok()?;
        let id = self.next.fetch_add(1, Ordering::AcqRel);
        (id < RECORDS).then_some(id)
    }
}
struct SkinAdmission<'a>(&'a Session);
impl Drop for SkinAdmission<'_> {
    fn drop(&mut self) {
        self.0.skin_busy.store(false, Ordering::Release);
    }
}

thread_local! {
    static COPY_READ_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static COPY_READ_CALLS: Cell<u64> = const { Cell::new(0) };
}
#[cfg(test)]
thread_local! { static COPY_TEST_DELAY_MS: Cell<u64> = const { Cell::new(0) }; }
struct CopyReadGuard(bool);
impl Drop for CopyReadGuard {
    fn drop(&mut self) {
        COPY_READ_ACTIVE.with(|v| v.set(self.0));
    }
}
fn copy_read_count() -> u64 {
    COPY_READ_CALLS.with(Cell::get)
}
fn copy_read_scope<T>(operation: impl FnOnce() -> T) -> T {
    let previous = COPY_READ_ACTIVE.with(|v| v.replace(true));
    let _guard = CopyReadGuard(previous);
    // Full owner validation still uses its original memory-query scope.
    // There is no permission/data cache in the exact-byte copy backend.
    crate::memory_query::scoped(operation)
}
fn read_current_process(address: usize, destination: &mut [u8]) -> Option<()> {
    use windows::Win32::{Foundation::HANDLE, System::Diagnostics::Debug::ReadProcessMemory};
    if destination.is_empty() {
        return Some(());
    }
    if address == 0 {
        return None;
    }
    address.checked_add(destination.len())?;
    let mut copied = 0;
    COPY_READ_CALLS.with(|v| v.set(v.get().saturating_add(1)));
    // Current-process pseudo-handle only. Windows checks the actual requested
    // range, copies into recorder-owned storage, and reports short/failed reads.
    // This does not establish object lifetime; native ownership/phase gates do.
    unsafe {
        ReadProcessMemory(
            HANDLE(-1isize as *mut _),
            address as *const _,
            destination.as_mut_ptr().cast(),
            destination.len(),
            Some(&mut copied),
        )
        .ok()?;
    }
    (copied == destination.len()).then_some(())
}
fn ptr(address: usize) -> Option<usize> {
    if COPY_READ_ACTIVE.with(Cell::get) {
        let mut raw = [0u8; 8];
        read_current_process(address, &mut raw)?;
        return Some(usize::from_le_bytes(raw));
    }
    crate::memory_query::accessible_region(address, 8, false)?;
    Some(unsafe { (address as *const usize).read_unaligned() })
}
fn count(address: usize, max: usize) -> Option<usize> {
    if COPY_READ_ACTIVE.with(Cell::get) {
        let mut raw = [0u8; 4];
        read_current_process(address, &mut raw)?;
        return usize::try_from(i32::from_le_bytes(raw))
            .ok()
            .filter(|&n| n <= max);
    }
    crate::memory_query::accessible_region(address, 4, false)?;
    let n = unsafe { (address as *const i32).read_unaligned() };
    let n = usize::try_from(n).ok()?;
    (n <= max).then_some(n)
}
fn byte(address: usize) -> Option<u8> {
    if COPY_READ_ACTIVE.with(Cell::get) {
        let mut raw = [0u8; 1];
        read_current_process(address, &mut raw)?;
        return Some(raw[0]);
    }
    crate::memory_query::accessible_region(address, 1, false)?;
    Some(unsafe { (address as *const u8).read() })
}
fn layout(i: Identity) -> Option<Layout> {
    if ptr(i.child)? != i.base + 0x2D872A0
        || ptr(i.child.checked_add(0x18)?)? != i.sim
        || ptr(i.sim)? != i.base + 0x2D8B6F8
        || ptr(i.child.checked_add(0x268)?)? != i.root
        || count(i.sim.checked_add(0x48)?, 512)? != i.count
    {
        return None;
    }
    Some(Layout {
        current: ptr(i.child + 0x20)?,
        previous: ptr(i.child + 0x30)?,
        particles: ptr(i.sim + 0x40)?,
        colliders: ptr(i.child + 0x168)?,
        collider_count: count(i.child + 0x170, 32)?,
        consumer_colliders: ptr(i.child + 0x190)?,
        consumer_count: count(i.child + 0x188, 32)?,
    })
}
fn constraint_member(i: Identity, stage: usize, object: usize) -> Option<()> {
    if ptr(object)? != i.base + VTABLES[stage] {
        return None;
    }
    for (p, n) in [(0x88, 0x90), (0x98, 0xA0)] {
        let array = ptr(i.sim + p)?;
        let length = count(i.sim + n, 64)?;
        if (0..length).any(|x| array.checked_add(x * 8).and_then(ptr) == Some(object)) {
            return Some(());
        }
    }
    None
}
fn reference(r: &mut Record) -> Option<()> {
    let offset = match r.stage {
        4 => 0x38,
        6 => 0x48,
        _ => return Some(()),
    };
    let buffers = ptr(r.identity.root + 0x20)?;
    let n = count(r.identity.root + 0x28, 128)?;
    let index = count(r.object.checked_add(offset)?, 127)?;
    if index >= n {
        return None;
    }
    let first = ptr(buffers.checked_add(index * 8)?)?;
    r.copy("reference_selector", 0, first, 0x118)?;
    let selected = count(first.checked_add(0x110)?, 127)?;
    if selected >= n {
        return None;
    }
    let buffer = ptr(buffers.checked_add(selected * 8)?)?;
    r.copy("reference_selected", selected, buffer, 0x118)?;
    let stride = usize::from(byte(buffer.checked_add(0x24)?)?);
    if ![12, 16].contains(&stride) {
        return None;
    }
    let vertices = count(buffer.checked_add(0x20)?, 1024)?;
    if vertices < r.identity.count {
        return None;
    }
    r.copy(
        "reference_positions",
        selected,
        ptr(buffer + 0x18)?,
        vertices * stride,
    )?;
    // LocalRange's exact dispatch: +44 AND a non-null normal stream.
    let uses_normals = r.stage == 4 && byte(r.object + 0x44)? != 0 && ptr(buffer + 0x40)? != 0;
    if uses_normals {
        let stride = usize::from(byte(buffer + 0x4C)?);
        if ![12, 16].contains(&stride) {
            return None;
        }
        r.copy(
            "reference_normals",
            selected,
            ptr(buffer + 0x40)?,
            vertices * stride,
        )?;
    }
    Some(())
}
fn collider_before(r: &mut Record) -> Option<()> {
    let i = r.identity;
    let set_index = count(i.sim + 0xA8, 127)?;
    let sets = ptr(i.root + 0x30)?;
    let set_count = count(i.root + 0x38, 128)?;
    if set_index >= set_count {
        return None;
    }
    let set = ptr(sets.checked_add(set_index * 8)?)?;
    let bones = ptr(set.checked_add(0x18)?)?;
    let bone_count = count(set.checked_add(0x20)?, 2048)?;
    let indices = ptr(i.sim + 0xB0)?;
    let offsets = ptr(i.sim + 0xC0)?;
    if count(i.sim + 0xB8, 32)? != r.layout.collider_count
        || count(i.sim + 0xC8, 32)? != r.layout.collider_count
    {
        return None;
    }
    r.copy("transform_set", set_index, set, 0x28)?;
    r.copy("collider_indices", 0, indices, r.layout.collider_count * 4)?;
    r.copy("collider_offsets", 0, offsets, r.layout.collider_count * 64)?;
    for c in 0..r.layout.collider_count {
        let index = count(indices.checked_add(c * 4)?, 2047)?;
        if index >= bone_count {
            return None;
        }
        r.copy("bone_matrix", c, bones.checked_add(index * 64)?, 64)?;
    }
    Some(())
}
fn colliders(r: &mut Record, after: bool) -> Option<()> {
    for c in 0..r.layout.collider_count {
        let object = ptr(r.layout.colliders.checked_add(c * 8)?)?;
        if ptr(object)? != r.identity.base + 0x2D8B758 {
            return None;
        }
        if after
            && !r
                .blocks
                .iter()
                .any(|b| b.name == "collider_before" && b.index == c && b.address == object)
        {
            return None;
        }
        r.copy(
            if after {
                "collider_after"
            } else {
                "collider_before"
            },
            c,
            object,
            0x90,
        )?;
        if !after {
            let shape = ptr(object.checked_add(0x88)?)?;
            let size = match ptr(shape)?.checked_sub(r.identity.base)? {
                0x2D7DBC8 => 0xB0,
                0x2D896D0 => 0x60,
                _ => 0x10,
            };
            r.copy("shape", c, shape, size)?;
        }
    }
    Some(())
}
fn consumer_colliders(r: &mut Record, after: bool) -> Option<()> {
    for c in 0..r.layout.consumer_count {
        let object = r.layout.consumer_colliders.checked_add(c * 0xA0)?;
        let shape = ptr(object.checked_add(0x88)?)?;
        let shape_size = match ptr(shape)?.checked_sub(r.identity.base)? {
            0x2D7DBC8 => 0xB0,
            0x2D896D0 => 0x60,
            _ => 0x20, // Explicit unknown type header, never guessed larger read.
        };
        if after
            && !r
                .blocks
                .iter()
                .any(|b| b.name == "consumer_shape" && b.index == c && b.address == shape)
        {
            return None;
        }
        r.copy(
            if after {
                "consumer_after"
            } else {
                "consumer_before"
            },
            c,
            object,
            0xA0,
        )?;
        if !after {
            r.copy("consumer_shape", c, shape, shape_size)?;
        }
    }
    Some(())
}
fn contact_inputs(r: &mut Record) -> Option<()> {
    let offset = r.object.checked_sub(r.layout.consumer_colliders)?;
    if offset % 0xA0 != 0 || offset / 0xA0 >= r.layout.consumer_count {
        return None;
    }
    let shape = ptr(r.object + 0x88)?;
    let expected = if r.stage == 8 { 0x2D896D0 } else { 0x2D7DBC8 };
    if ptr(shape)? != r.identity.base + expected {
        return None;
    }
    let n = count(r.contact.indices.checked_add(8)?, r.identity.count)?;
    if n == 0 {
        return None;
    }
    let indices = ptr(r.contact.indices)?;
    r.copy("contact_indices_header", 0, r.contact.indices, 16)?;
    r.copy("contact_indices", 0, indices, n * 2)?;
    for x in 0..n {
        crate::memory_query::accessible_region(indices + x * 2, 2, false)?;
        if usize::from(unsafe { ((indices + x * 2) as *const u16).read_unaligned() })
            >= r.identity.count
        {
            return None;
        }
    }
    for (name, head, header_name) in [
        ("contact_radii", r.contact.radii, "contact_radii_header"),
        (
            "contact_friction",
            r.contact.friction,
            "contact_friction_header",
        ),
    ] {
        if count(head.checked_add(8)?, r.identity.count)? != n {
            return None;
        }
        r.copy(header_name, 0, head, 16)?;
        r.copy(name, 0, ptr(head)?, n * 4)?;
    }
    Some(())
}
fn same_contact_headers(r: &Record) -> Option<()> {
    for b in r.blocks.iter().filter(|b| {
        matches!(
            b.name,
            "contact_indices_header" | "contact_radii_header" | "contact_friction_header"
        )
    }) {
        crate::memory_query::accessible_region(b.address, b.length, false)?;
        let now = unsafe { std::slice::from_raw_parts(b.address as *const u8, b.length) };
        if now != &r.data[b.start..b.start + b.length] {
            return None;
        }
    }
    Some(())
}
// One metadata snapshot at the first active289 constraint. These are NOT
// action-entry force inputs; a complete marker only means the bounded copy
// finished. Unknown action classes expose a16-byte header, no guessed layout.
fn capture_runtime_action_metadata(r: &mut Record) -> Option<()> {
    let child = r.identity.child;
    if ptr(child + 0x18)? != r.identity.sim || count(child + 0x28, 512)? != r.identity.count {
        return None;
    }
    r.copy("dynamics_child_header", 0, child, 0x270)?;
    let n = count(child + 0xF0, 4)?;
    let array = ptr(child + 0xE8)?;
    r.copy("runtime_action_slots", 0, array, n * 8)?;
    for index in 0..n {
        let action = ptr(array.checked_add(index * 8)?)?;
        if action == 0 {
            return None;
        }
        let vtable = ptr(action)?;
        r.copy("runtime_action_header", index, action, 16)?;
        if vtable == r.identity.base.checked_add(0x2D89CE0)? {
            r.copy("runtime_simple_wind", index, action, 0x50)?;
        }
    }
    if count(child + 0x48, 512)? == r.identity.count {
        r.copy(
            "simulation_normals_at_constraint",
            0,
            ptr(child + 0x40)?,
            r.identity.count * 16,
        )?;
    }
    let override_info = ptr(child + 0x240)?;
    if override_info != 0 {
        r.copy("simulation_info_override", 0, override_info, 0x20)?;
    }
    if ptr(child + 0x18)? != r.identity.sim
        || count(child + 0xF0, 4)? != n
        || ptr(child + 0xE8)? != array
    {
        return None;
    }
    r.copy("runtime_actions_complete", 0, 0, 0)
}
fn capture_before(r: &mut Record) -> Option<()> {
    r.layout = layout(r.identity)?;
    r.copy("current_before", 0, r.layout.current, r.identity.count * 16)?;
    r.copy(
        "previous_before",
        0,
        r.layout.previous,
        r.identity.count * 16,
    )?;
    r.copy(
        "particle_data",
        0,
        r.layout.particles,
        r.identity.count * 16,
    )?;
    r.copy("simulation_header", 0, r.identity.sim, 0x1C0)?;
    r.copy("consumer_layout", 0, r.identity.child + 0x188, 16)?;
    if r.stage == 0 {
        collider_before(r)?;
    } else if r.stage <= 6 {
        constraint_member(r.identity, r.stage, r.object)?;
        // Copy only each verified class prefix, not a blanket oversized header.
        r.copy(
            "constraint_header",
            0,
            r.object,
            [0, 0x38, 0x38, 0x38, 0x48, 0x40, 0x50][r.stage],
        )?;
        let stride = [0, 12, 16, 12, 16, 32, 16][r.stage];
        let n = count(r.object.checked_add(0x30)?, 1280)?;
        r.copy(
            "constraint_rows",
            0,
            ptr(r.object.checked_add(0x28)?)?,
            n * stride,
        )?;
        reference(r)?;
        transition_state(r, false)?;
        if r.id == 0 && r.stage == 4 && r.identity.count == 289 {
            // Partial optional metadata never discards the primary boundary;
            // the pair's existing total copy budget still accounts for it.
            let _ = capture_runtime_action_metadata(r);
        }
    } else {
        r.copy("collision_child_header", 0, r.identity.child, 0x270)?;
        let masks = count(r.identity.sim + 0xF0, r.identity.count)?;
        if masks != 0 && masks != r.identity.count {
            return None;
        }
        r.copy("collision_masks", 0, ptr(r.identity.sim + 0xE8)?, masks * 4)?;
        if r.stage >= 8 {
            contact_inputs(r)?;
        }
    }
    colliders(r, false)?;
    consumer_colliders(r, false)?;
    (layout(r.identity)? == r.layout).then_some(())
}
fn capture_after(r: &mut Record) -> Option<()> {
    if layout(r.identity)? != r.layout {
        return None;
    }
    if (1..=6).contains(&r.stage) {
        constraint_member(r.identity, r.stage, r.object)?;
    }
    r.copy("current_after", 0, r.layout.current, r.identity.count * 16)?;
    r.copy(
        "previous_after",
        0,
        r.layout.previous,
        r.identity.count * 16,
    )?;
    same_contact_headers(r)?;
    transition_state(r, true)?;
    colliders(r, true)?;
    consumer_colliders(r, true)?;
    (layout(r.identity)? == r.layout).then_some(())
}

fn transition_state(r: &mut Record, after: bool) -> Option<()> {
    if r.stage != 6 {
        return Some(());
    }
    let array = ptr(r.identity.child + 0x158)?;
    let n = count(r.identity.child + 0x160, 64)?;
    if !after {
        r.copy("transition_state_table", 0, array, n * 16)?;
    }
    let id = count(r.object + 0x20, i32::MAX as usize)?;
    for x in 0..n {
        let entry = array.checked_add(x * 16)?;
        if count(entry, i32::MAX as usize)? == id {
            let state = ptr(entry + 8)?;
            if after
                && !r
                    .blocks
                    .iter()
                    .any(|b| b.name == "transition_state_before" && b.address == state)
            {
                return None;
            }
            return r.copy(
                if after {
                    "transition_state_after"
                } else {
                    "transition_state_before"
                },
                x,
                state,
                0x30,
            );
        }
    }
    // Missing state is explicitly visible (empty table/no matching block); no guessed pointer.
    Some(())
}

pub(crate) fn tick(frame: u64) {
    let Some(s) = SESSION.get() else {
        return;
    };
    s.frame.store(frame, Ordering::Release);
    match s.phase.load(Ordering::Acquire) {
        1 if frame.is_multiple_of(30) => {
            let targets = crate::memory_query::scoped(crate::body_scale_port::diagnostic_targets);
            let Ok(mut previous) = s.candidate.try_lock() else {
                return;
            };
            if let Some(value) = targets
                && targets == *previous
            {
                let _ = s.targets.set(value);
                let _ = s.started.set(Instant::now());
                let _ = s
                    .phase
                    .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire);
            }
            *previous = targets;
        }
        2 => {
            if ordered::ENABLED {
                ordered::poll_end(s);
            } else if s.started.get().is_some_and(|t| t.elapsed() >= WINDOW) {
                s.stop(1);
            }
        }
        _ => {}
    }
}
fn identity_current(expected: Identity) -> bool {
    crate::body_scale_port::diagnostic_targets().is_some_and(|targets| targets.contains(&expected))
}
/// Called only inside the existing successful owned mesh correction scope.
/// No full identity traversal, native calls, allocation, or blocking lock here.
pub(crate) fn collider_velocity_begin(
    child: usize,
    witness: crate::cloth_collider_rotation_hook::Witness,
) -> Option<usize> {
    ordered::collider_velocity_begin(child, witness)
}
pub(crate) fn collider_velocity_end(child: usize, token: usize, output: Option<[u32; 8]>) {
    ordered::collider_velocity_end(child, token, output);
}

pub(crate) fn observe_mesh_frames(source: MeshBoundarySource) {
    let Some(s) = SESSION
        .get()
        .filter(|s| s.phase.load(Ordering::Acquire) == 2)
    else {
        return;
    };
    s.in_flight.fetch_add(1, Ordering::AcqRel);
    let _flight = Flight(&s.in_flight);
    if ordered::ENABLED {
        ordered::mesh_begin(s, source);
        return;
    }
    if s.phase.load(Ordering::Acquire) != 2 || s.started.get().is_none_or(|t| t.elapsed() >= WINDOW)
    {
        return;
    }
    let Some(identity) = s
        .targets
        .get()
        .and_then(|targets| {
            targets.iter().find(|i| {
                i.count == 289
                    && i.model == source.model
                    && i.owner == source.owner
                    && i.input == source.input
                    && i.generation == source.generation
                    && i.scale_bits == source.scale_bits
            })
        })
        .copied()
    else {
        return;
    };
    let Ok(mut mesh) = s.mesh_boundary.try_lock() else {
        return;
    };
    if mesh.source.is_some() {
        return;
    }
    let start = Instant::now();
    let _ = mesh.start(source, identity, s.frame.load(Ordering::Acquire));
    mesh.record.native_enter_us = s
        .started
        .get()
        .map_or(0, |t| t.elapsed().as_micros() as u64);
    mesh.copy_us += start.elapsed().as_micros() as u64;
    if mesh.copy_us > SKIN_BUDGET_US {
        mesh.error = "mesh_copy_budget";
        s.stop(2);
    }
}
pub(crate) fn observe_mesh_output(op: usize, output: usize, normals: usize) {
    let Some(s) = SESSION
        .get()
        .filter(|s| s.phase.load(Ordering::Acquire) == 2)
    else {
        return;
    };
    s.in_flight.fetch_add(1, Ordering::AcqRel);
    let _flight = Flight(&s.in_flight);
    if ordered::ENABLED {
        ordered::mesh_end(s, op, output, normals);
        return;
    }
    if s.phase.load(Ordering::Acquire) != 2 || s.started.get().is_none_or(|t| t.elapsed() >= WINDOW)
    {
        return;
    }
    let Ok(mut mesh) = s.mesh_boundary.try_lock() else {
        return;
    };
    if mesh.error != "awaiting_mesh_output" {
        return;
    }
    let start = Instant::now();
    let _ = mesh.finish(op, output, normals, s.frame.load(Ordering::Acquire));
    mesh.record.native_exit_us = s
        .started
        .get()
        .map_or(0, |t| t.elapsed().as_micros() as u64);
    mesh.copy_us += start.elapsed().as_micros() as u64;
    if mesh.copy_us > SKIN_BUDGET_US {
        mesh.error = "mesh_copy_budget";
        mesh.complete = false;
        s.stop(2);
    }
}
fn boundary_pair_allowed(
    s: &Session,
    stage: usize,
    bucket: usize,
    frame: u64,
    strength_bits: u32,
) -> bool {
    let Ok(mesh) = s.mesh_boundary.try_lock() else {
        return false;
    };
    if mesh.paired {
        return stage_allowed(stage, s.started.get().map_or(WINDOW, Instant::elapsed));
    }
    // Reserve the first primary pair for this producer's next consumer. The
    // old early contact pair could stop every channel before meshPN was seen.
    let strength = f32::from_bits(strength_bits);
    // Original type5/10 adaptive constraints run only in the last substep.
    // A zero-strength wrapper returns without reading its reference. Do not
    // spend this frame's only pair on that wrapper; forward it unchanged.
    mesh.complete
        && stage == 4
        && bucket == 3
        && mesh.record.frame == frame
        && strength.is_finite()
        && strength > 0.0
}
fn capture_pair_before(s: &Session, r: &mut Record, check_identity: impl FnOnce() -> bool) {
    let mesh_cost = boundary_pair_cost(s, r.id);
    let start = Instant::now();
    let queries = crate::memory_query::query_count();
    let result = crate::memory_query::scoped(|| {
        let identity_timer = Instant::now();
        let current = check_identity();
        r.identity_before_us = identity_timer.elapsed().as_micros() as u64;
        if !current {
            return None;
        }
        if r.stage == 4 {
            if let Ok(coverage) = s.skin.try_lock() {
                for (out, c) in r
                    .skin_links
                    .iter_mut()
                    .zip(coverage.iter().filter(|c| c.root == r.identity.root))
                {
                    *out = c.latest;
                }
            }
            r.skin_read_start_us = s.started.get()?.elapsed().as_micros() as u64;
        }
        // Check immediately after a slow OS/identity operation: do not spend
        // additional copies after the hard limit is already known exceeded.
        if start.elapsed().as_micros() as u64 + mesh_cost > COPY_BUDGET_US {
            return None;
        }
        capture_before(r)
    });
    r.before_us = start.elapsed().as_micros() as u64;
    r.before_queries = crate::memory_query::query_count().saturating_sub(queries);
    r.error = if result.is_some() {
        "awaiting_after"
    } else {
        "before_guard"
    };
    if r.before_us + mesh_cost > COPY_BUDGET_US {
        s.stop(2);
        r.error = "copy_budget";
    }
}
fn boundary_pair_cost(s: &Session, id: usize) -> u64 {
    s.mesh_boundary.try_lock().map_or(COPY_BUDGET_US, |m| {
        if m.consumer_id == Some(id) {
            m.copy_us
        } else {
            0
        }
    })
}
fn retain_scalar_skin_reach(
    s: &Session,
    op: usize,
    pre: crate::body_scale_port::SkinNormalTrace,
    post: Option<crate::body_scale_port::SkinNormalTrace>,
) {
    // This copies already validated values, never dereferences context/op/root.
    // A rejection before ownership was established is deliberately unattributed.
    if pre.gate != "accepted"
        || s.phase.load(Ordering::Acquire) != 2
        || s.started.get().is_none_or(|t| t.elapsed() >= WINDOW)
        || s.targets.get().is_none_or(|targets| {
            !targets.iter().any(|i| {
                i.root == pre.observed[0]
                    && i.scale_bits as usize == pre.expected
                    && i.generation == crate::body_scale_port::cloth_topology_generation()
            })
        })
    {
        return;
    }
    let Ok(mut reach) = s.scalar_skin_reach.try_lock() else {
        return;
    };
    if reach.len() == 16 || reach.iter().any(|r| r.op == op) {
        return;
    }
    reach.push(ScalarSkinReach {
        op,
        root: pre.observed[0],
        sample: SkinGuardSample {
            frame: s.frame.load(Ordering::Acquire),
            pre,
            post,
        },
    });
}
struct Flight(&'static AtomicUsize);
impl Drop for Flight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
/// Bounded per-operator reach, after the unchanged normal correction. No raw
/// geometry writes, allocations, blocking locks, or observations after stop.
fn update_skin_link(s: &Session, c: &mut SkinCoverage, sample: SkinGuardSample) {
    let Some(started) = s.started.get() else {
        return;
    };
    if started.elapsed() >= WINDOW {
        s.stop(1);
        return;
    }
    let timer = Instant::now();
    let token = s.link_budget.load(Ordering::Relaxed);
    let used = if token >> 20 == sample.frame {
        token & ((1 << 20) - 1)
    } else {
        0
    };
    let duplicate = c.latest.is_some_and(|l| l.sample.frame == sample.frame);
    // A second invocation invalidates the sampled geometry;do not pretend the
    // first write is the latest just because no second read was admitted.
    if used >= LINK_FRAME_BUDGET_US && !duplicate {
        return;
    }
    let generation = crate::body_scale_port::cloth_topology_generation();
    let probes = if duplicate {
        None
    } else {
        read_normal_points(sample.pre, sample.post)
    };
    c.latest = Some(SkinLink {
        op: c.op,
        root: c.root,
        buffer: if sample.pre.gate == "accepted" {
            sample.pre.observed[1]
        } else {
            c.buffer
        },
        generation,
        scale_bits: sample.post.map_or(sample.pre.expected, |p| p.expected) as u32,
        captured_us: started.elapsed().as_micros() as u64,
        sample,
        probes: if s.frame.load(Ordering::Acquire) == sample.frame
            && crate::body_scale_port::cloth_topology_generation() == generation
        {
            probes
        } else {
            None
        },
        duplicate,
    });
    let spent = used
        .saturating_add(timer.elapsed().as_micros() as u64)
        .min((1 << 20) - 1);
    s.link_budget
        .store((sample.frame << 20) | spent, Ordering::Relaxed);
}
pub(crate) fn observe_skin(
    op: usize,
    context: usize,
    pre: bool,
    rows: Option<usize>,
    pre_trace: crate::body_scale_port::SkinNormalTrace,
    post_trace: Option<crate::body_scale_port::SkinNormalTrace>,
) {
    if ordered::ENABLED {
        return;
    }
    let Some(s) = SESSION
        .get()
        .filter(|s| s.phase.load(Ordering::Acquire) == 2)
    else {
        return;
    };
    s.in_flight.fetch_add(1, Ordering::AcqRel);
    let _flight = Flight(&s.in_flight);
    if s.phase.load(Ordering::Acquire) != 2 {
        return;
    }
    retain_scalar_skin_reach(s, op, pre_trace, post_trace);
    let Some(_admission) = s.started.get().and_then(|t| s.admit_skin(t.elapsed())) else {
        return;
    };
    let timer = Instant::now();
    let _ = crate::memory_query::scoped(|| {
        let root = ptr(context.checked_add(0x10)?)?;
        let target = s.targets.get()?.iter().find(|i| i.root == root)?;
        // Already sampled operators bypass the expensive full target scan.
        // The key is only used to SKIP observation, never to authorize a read.
        {
            let mut coverage = match s.skin.try_lock() {
                Ok(value) => value,
                Err(_) => {
                    s.skin_dropped.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            };
            if !skin_needs_observation(&coverage, op, root) {
                if let Some(c) = coverage.iter_mut().find(|c| c.op == op && c.root == root) {
                    update_skin_link(
                        s,
                        c,
                        SkinGuardSample {
                            frame: s.frame.load(Ordering::Acquire),
                            pre: pre_trace,
                            post: post_trace,
                        },
                    );
                }
                return Some(());
            }
        }
        if ptr(op)? != target.base + 0x2D86308 || !identity_current(*target) {
            return None;
        }
        let mut coverage = match s.skin.try_lock() {
            Ok(value) => value,
            Err(_) => {
                s.skin_dropped.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        let index = match coverage.iter().position(|c| c.op == op && c.root == root) {
            Some(index) => index,
            None if coverage.len() < 16 => {
                coverage.push(SkinCoverage {
                    op,
                    root,
                    ..SkinCoverage::default()
                });
                coverage.len() - 1
            }
            None => {
                s.skin_dropped.fetch_add(1, Ordering::Relaxed);
                return None;
            }
        };
        let c = &mut coverage[index];
        if !c.observe(
            pre,
            rows,
            SkinGuardSample {
                frame: s.frame.load(Ordering::Acquire),
                pre: pre_trace,
                post: post_trace,
            },
        ) {
            return Some(());
        }
        let metadata = (|| {
            let buffers = ptr(root + 0x20)?;
            let n = count(root + 0x28, 128)?;
            let first = count(op + 0x68, 127)?;
            if first >= n {
                return None;
            }
            let selector = ptr(buffers + first * 8)?;
            let selected = count(selector + 0x110, 127)?;
            if selected >= n {
                return None;
            }
            let buffer = ptr(buffers + selected * 8)?;
            Some((buffer, count(buffer + 0x20, 1024)?))
        })();
        if let Some((buffer, vertices)) = metadata {
            c.buffer = buffer;
            c.vertices = vertices;
        } else {
            c.metadata_rejected += 1;
        }
        update_skin_link(
            s,
            c,
            SkinGuardSample {
                frame: s.frame.load(Ordering::Acquire),
                pre: pre_trace,
                post: post_trace,
            },
        );
        Some(())
    });
    let elapsed = timer.elapsed().as_micros() as u64;
    s.finish_skin_observation(elapsed);
}
fn begin(
    stage: usize,
    child: usize,
    object: usize,
    strength_bits: u32,
    flags: usize,
    contact: ContactArgs,
) -> Option<(Record, Flight)> {
    if ordered::ENABLED {
        return None;
    }
    let s = SESSION.get()?;
    if s.phase.load(Ordering::Acquire) != 2 {
        return None;
    }
    let bucket = s.targets.get()?.iter().position(|i| i.child == child)?;
    // Acquire before checking the phase, so a stop/footer cannot race a reservation.
    s.in_flight.fetch_add(1, Ordering::AcqRel);
    let flight = Flight(&s.in_flight);
    if s.phase.load(Ordering::Acquire) != 2 {
        return None;
    }
    let elapsed = s.started.get()?.elapsed();
    if elapsed >= WINDOW {
        s.stop(1);
        return None;
    }
    let frame = s.frame.load(Ordering::Acquire);
    if !boundary_pair_allowed(s, stage, bucket, frame, strength_bits) {
        return None;
    }
    let id = s.reserve(bucket, stage, frame)?;
    let slot = &s.slots[id];
    let mut r = slot.record.try_lock().ok()?.take()?;
    slot.state.store(1, Ordering::Release);
    r.id = id;
    r.stage = stage;
    r.frame = frame;
    r.identity = s.targets.get()?[bucket];
    r.object = object;
    r.strength_bits = strength_bits;
    r.flags = flags;
    r.contact = contact;
    if let Ok(mut mesh) = s.mesh_boundary.try_lock()
        && !mesh.paired
        && mesh.complete
        && stage == 4
        && bucket == 3
        && mesh.record.frame == frame
    {
        mesh.paired = true;
        mesh.consumer_id = Some(id);
    }
    let identity = r.identity;
    capture_pair_before(s, &mut r, || identity_current(identity));
    Some((r, flight))
}
fn finish(mut r: Record) {
    let s = SESSION.get().unwrap();
    if r.error == "awaiting_after" {
        let start = Instant::now();
        let queries = crate::memory_query::query_count();
        let result = crate::memory_query::scoped(|| {
            let identity_timer = Instant::now();
            let current = identity_current(r.identity);
            r.identity_after_us = identity_timer.elapsed().as_micros() as u64;
            if !current {
                return None;
            }
            capture_after(&mut r)
        });
        r.after_us = start.elapsed().as_micros() as u64;
        r.after_queries = crate::memory_query::query_count().saturating_sub(queries);
        r.valid = result.is_some();
        r.error = if r.valid { "none" } else { "after_guard" };
        if !r.valid {
            s.stop(3);
        }
        let dispatch_us = r.collision_dispatch.as_ref().map_or(0, |t| t.observer_us);
        if r.before_us + r.after_us + dispatch_us + boundary_pair_cost(s, r.id) > COPY_BUDGET_US {
            s.stop(2);
        }
    }
    let id = r.id;
    // No other thread may lock a state=1 slot. Writer only claims state=2.
    if let Ok(mut target) = s.slots[id].record.try_lock() {
        *target = Some(r);
        s.slots[id].state.store(2, Ordering::Release);
    } else {
        s.slots[id].state.store(4, Ordering::Release);
        s.stop(6);
    }
    if s.next.load(Ordering::Acquire) >= RECORDS {
        s.stop(4);
    }
}
type Apply = unsafe extern "C" fn(usize, usize, f32, usize);
type Map = unsafe extern "C" fn(usize);
type CollisionPass = unsafe extern "C" fn(usize, u8, u8, f32, u8) -> usize;
type ContactApply =
    unsafe extern "C" fn(usize, usize, usize, usize, usize, f32, usize, usize) -> usize;
fn call_original(stage: usize, registers: &Registers, original: usize) -> usize {
    unsafe {
        if stage == 7 {
            return std::mem::transmute::<usize, CollisionPass>(original)(
                registers.rcx as usize,
                registers.rdx as u8,
                registers.r8 as u8,
                f32::from_bits(registers.xmm3 as u32),
                registers.get_stack(5) as u8,
            );
        } else if stage >= 8 {
            return std::mem::transmute::<usize, ContactApply>(original)(
                registers.rcx as usize,
                registers.rdx as usize,
                registers.r8 as usize,
                registers.r9 as usize,
                registers.get_stack(5) as usize,
                f32::from_bits(registers.get_stack(6) as u32),
                registers.get_stack(7) as usize,
                registers.get_stack(8) as usize,
            );
        } else if stage == 0 {
            std::mem::transmute::<usize, Map>(original)(registers.rcx as usize);
        } else {
            std::mem::transmute::<usize, Apply>(original)(
                registers.rcx as usize,
                registers.rdx as usize,
                f32::from_bits(registers.xmm2 as u32),
                registers.r9 as usize,
            );
        }
    }
    0
}
fn dispatch(stage: usize, registers: *mut Registers, original: usize) -> usize {
    let registers = unsafe { &*registers };
    let ordered_event = ordered::constraint_begin(stage, registers);
    let child = if stage == 0 || stage == 7 {
        registers.rcx
    } else if stage >= 8 {
        // Audited Win64 stack arguments; forward even when capture is disabled.
        unsafe { registers.get_stack(5) }
    } else {
        registers.rdx
    } as usize;
    let record = begin(
        stage,
        child,
        registers.rcx as usize,
        if stage == 7 {
            registers.xmm3 as u32
        } else if stage >= 8 {
            unsafe { registers.get_stack(6) as u32 }
        } else {
            registers.xmm2 as u32
        },
        if stage == 7 {
            (registers.rdx as u8 as usize)
                | ((registers.r8 as u8 as usize) << 8)
                | ((unsafe { registers.get_stack(5) } as u8 as usize) << 16)
        } else {
            registers.r9 as usize
        },
        ContactArgs {
            indices: registers.rdx as usize,
            radii: registers.r8 as usize,
            friction: registers.r9 as usize,
        },
    );
    let mut record = record;
    if let Some((r, _)) = record.as_mut() {
        r.native_enter_us = SESSION
            .get()
            .unwrap()
            .started
            .get()
            .unwrap()
            .elapsed()
            .as_micros() as u64;
    }
    let dispatch_scope = dispatch_capture_enabled(stage).then(|| {
        DispatchScope::enter(record.as_ref().and_then(|(r, _)| {
            (r.error == "awaiting_after").then(|| {
                CollisionDispatch::new(
                    r.identity.count,
                    r.layout.consumer_colliders,
                    r.layout.consumer_count,
                )
            })
        }))
    });
    let result = call_original(stage, registers, original);
    ordered::constraint_end(ordered_event);
    if let Some(scope) = dispatch_scope {
        let trace = scope.finish();
        if let Some((r, _)) = record.as_mut() {
            r.collision_dispatch = trace;
        }
    }
    if let Some((mut r, flight)) = record {
        r.native_exit_us = SESSION
            .get()
            .unwrap()
            .started
            .get()
            .unwrap()
            .elapsed()
            .as_micros() as u64;
        finish(r);
        drop(flight);
    }
    result
}
fn compatible(base: usize) -> bool {
    (0..STAGES).all(|s| {
        let Some(address) = base.checked_add(RVAS[s]) else {
            return false;
        };
        if crate::memory_query::accessible_region(address, PROLOGUES[s].len(), false).is_none() {
            return false;
        }
        let bytes = unsafe { std::slice::from_raw_parts(address as *const u8, PROLOGUES[s].len()) };
        bytes == PROLOGUES[s] && (VTABLES[s] == 0 || ptr(base + VTABLES[s] + 0x30) == Some(address))
    }) && CONTACT_SEAMS
        .iter()
        .chain(DISPATCH_SITES.iter())
        .all(|&(rva, expected)| {
            let Some(address) = base.checked_add(rva) else {
                return false;
            };
            if crate::memory_query::accessible_region(address, expected.len(), false).is_none() {
                return false;
            }
            unsafe { std::slice::from_raw_parts(address as *const u8, expected.len()) == expected }
        })
}
pub(crate) fn install(base: usize) {
    if SESSION.get().is_some() {
        return;
    }
    if !crate::memory_query::scoped(|| compatible(base) && ordered::compatible(base)) {
        crate::log::line(format_args!(
            "[ERPS-SYNC253] disabled: native entry/vtable mismatch; scaling unchanged"
        ));
        return;
    }
    // Allocation and file open are initialization work, never native hot-path work.
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let name = format!(
        "ERCharacterScale_diag_2_54_rc5_{}_{stamp}.jsonl",
        std::process::id()
    );
    let Some(path) = crate::log::sibling_path(&name) else {
        return;
    };
    let sink = OpenOptions::new().write(true).create_new(true).open(&path);
    let Ok(file) = sink else {
        crate::log::line(format_args!(
            "[ERPS-SYNC253] disabled: cannot create diagnostic file"
        ));
        return;
    };
    let _ = SESSION.set(Session::new());
    let s = SESSION.get().unwrap();
    for (site, (rva, _)) in DISPATCH_SITES
        .iter()
        .enumerate()
        .filter(|_| CAPTURE_DISPATCH_SETS)
    {
        let hook = unsafe {
            hook_closure_jmp_back(
                base + rva,
                move |r| observe_dispatch(site, r),
                CallbackOption::None,
                HookFlags::empty(),
            )
        };
        let Ok(hook) = hook else {
            crate::log::line(format_args!(
                "[ERPS-SYNC253] disabled: dispatch hook {site} failed"
            ));
            return;
        };
        let _ = Box::leak(Box::new(hook));
    }
    for (stage, rva) in RVAS.iter().copied().enumerate() {
        let hook = unsafe {
            hook_closure_retn(
                base + rva,
                move |r, o| dispatch(stage, r, o),
                CallbackOption::None,
                HookFlags::empty(),
            )
        };
        let Ok(hook) = hook else {
            crate::log::line(format_args!(
                "[ERPS-SYNC253] disabled: hook {stage} failed; installed diagnostic hooks pass through"
            ));
            return;
        };
        let _ = Box::leak(Box::new(hook));
    }
    if !ordered::install(base) {
        crate::log::line(format_args!(
            "[ERPS-SYNC253] disabled: ordered hook failed; installed hooks pass through"
        ));
        return;
    }
    let mut sink = BufWriter::with_capacity(64 * 1024, file);
    if writeln!(sink, "{{\"type\":\"header\",\"schema\":\"er-cloth-sync-4\",\"ordered_step_profile_version\":2,\"budget_policy\":\"complete-step-measure-only\",\"copy_backend\":\"ReadProcessMemory/current-process\",\"primary_sampler_enabled\":false,\"auxiliary_skin_sampler_enabled\":false,\"start_ms\":1200,\"max_steps\":3,\"max_events_per_step\":20,\"mesh_copy_budget_us\":2000,\"copy_budget_us\":4000,\"build\":\"{}\",\"pid\":{},\"base\":{},\"model\":\"BD_M_9004\",\"max_record_bytes\":{},\"window_ms\":5000,\"native_calls_unchanged\":false,\"native_call_count_unchanged\":true,\"collider_velocity_rigid_inputs\":true}}", crate::BUILD_MODE, std::process::id(), base, BYTES).and_then(|_| sink.flush()).is_err() {
        crate::log::line(format_args!("[ERPS-SYNC253] disabled: header write failed")); return;
    }
    if std::thread::Builder::new()
        .name("er-cloth-diag-writer".into())
        .spawn(move || writer(sink))
        .is_err()
    {
        crate::log::line(format_args!("[ERPS-SYNC253] disabled: writer unavailable"));
        return;
    }
    s.phase.store(1, Ordering::Release);
    crate::log::line(format_args!(
        "[ERPS-SYNC253] armed path={} once=true ordered_steps=3 start_ms=1200 window_ms=5000 max_events=20 budget_policy=complete-step-measure-only copy_target_us=4000 mesh_copy_target_us=2000 primary_sampler=false auxiliary_skin_sampler=false copy_backend=ReadProcessMemory/current-process",
        path.display()
    ));
}
fn write_record(sink: &mut impl Write, r: &Record) -> io::Result<()> {
    write_record_named(sink, r, NAMES[r.stage], RVAS[r.stage])?;
    writeln!(sink)
}
fn write_record_named(sink: &mut impl Write, r: &Record, name: &str, rva: usize) -> io::Result<()> {
    let i = r.identity;
    write!(
        sink,
        "{{\"type\":\"sample\",\"id\":{},\"stage\":\"{}\",\"frame\":{},\"child\":{},\"sim\":{},\"root\":{},\"model\":{},\"owner\":{},\"input\":{},\"generation\":{},\"scale_bits\":{},\"count\":{},\"object\":{},\"strength_bits\":{},\"flags\":{},\"valid\":{},\"error\":\"{}\",\"before_us\":{},\"after_us\":{},\"native_enter_us\":{},\"native_exit_us\":{},\"blocks\":[",
        r.id,
        name,
        r.frame,
        i.child,
        i.sim,
        i.root,
        i.model,
        i.owner,
        i.input,
        i.generation,
        i.scale_bits,
        i.count,
        r.object,
        r.strength_bits,
        r.flags,
        r.valid,
        r.error,
        r.before_us,
        r.after_us,
        r.native_enter_us,
        r.native_exit_us
    )?;
    for (index, b) in r.blocks.iter().enumerate() {
        if index != 0 {
            sink.write_all(b",")?;
        }
        let mut hex = String::with_capacity(b.length * 2);
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for byte in &r.data[b.start..b.start + b.length] {
            hex.push(DIGITS[(byte >> 4) as usize] as char);
            hex.push(DIGITS[(byte & 15) as usize] as char);
        }
        write!(
            sink,
            "{{\"name\":\"{}\",\"index\":{},\"address\":{},\"length\":{},\"hex\":\"{}\"}}",
            b.name, b.index, b.address, b.length, hex
        )?;
    }
    write!(
        sink,
        "],\"native_rva\":{},\"capture_cost\":{{\"identity_before_us\":{},\"identity_after_us\":{},\"before_queries\":{},\"after_queries\":{},\"before_copy_reads\":{},\"after_copy_reads\":{}}},\"skin_read_start_us\":{},\"skin_links\":[",
        rva,
        r.identity_before_us,
        r.identity_after_us,
        r.before_queries,
        r.after_queries,
        r.before_copy_reads,
        r.after_copy_reads,
        r.skin_read_start_us
    )?;
    for (index, link) in r.skin_links.iter().flatten().enumerate() {
        if index != 0 {
            write!(sink, ",")?;
        }
        write!(
            sink,
            "{{\"op\":{},\"root\":{},\"buffer\":{},\"generation\":{},\"scale_bits\":{},\"captured_us\":{},\"frame\":{},\"duplicate\":{},\"probe_bits\":",
            link.op,
            link.root,
            link.buffer,
            link.generation,
            link.scale_bits,
            link.captured_us,
            link.sample.frame,
            link.duplicate
        )?;
        if let Some(probes) = link.probes {
            write!(sink, "{probes:?}")?;
        } else {
            write!(sink, "null")?;
        }
        write!(sink, ",\"pre\":")?;
        write_skin_trace(sink, &link.sample.pre)?;
        write!(sink, ",\"post\":")?;
        if let Some(post) = link.sample.post {
            write_skin_trace(sink, &post)?;
        } else {
            write!(sink, "null")?;
        }
        write!(sink, "}}")?;
    }
    write!(sink, "] ,\"collision_dispatch\":")?;
    if let Some(t) = &r.collision_dispatch {
        write!(
            sink,
            "{{\"error\":\"{}\",\"observer_us\":{},\"failure\":{:?},\"events\":[",
            t.error, t.observer_us, t.failure
        )?;
        for (j, e) in t.events[..t.len].iter().enumerate() {
            if j != 0 {
                write!(sink, ",")?;
            }
            write!(
                sink,
                "{{\"site_rva\":{},\"collider\":{},\"count\":{},\"particle_bits\":{:?}}}",
                DISPATCH_SITES[e.site].0, e.collider, e.count, e.bits
            )?;
        }
        write!(sink, "]}}")?;
    } else {
        write!(sink, "null")?;
    }
    write!(sink, "}}")
}
fn writer(mut sink: impl Write) {
    let s = SESSION.get().unwrap();
    let mut written = 0;
    let mut valid = 0;
    loop {
        for slot in &s.slots {
            if slot.state.load(Ordering::Acquire) != 2 {
                continue;
            }
            let Ok(mut locked) = slot.record.try_lock() else {
                continue;
            };
            let Some(r) = locked.take() else {
                continue;
            };
            drop(locked);
            if write_record(&mut sink, &r)
                .and_then(|_| sink.flush())
                .is_err()
            {
                s.stop(5);
                crate::log::line(format_args!(
                    "[ERPS-SYNC253] stopped: writer error; partial file is NOT complete evidence"
                ));
                return;
            }
            written += 1;
            valid += usize::from(r.valid);
            slot.state.store(3, Ordering::Release);
        }
        if ordered::ENABLED {
            ordered::poll_end(s);
        } else if s.phase.load(Ordering::Acquire) == 2
            && s.started.get().is_some_and(|t| t.elapsed() >= WINDOW)
        {
            s.stop(1);
        }
        if crate::SHUTDOWN.load(Ordering::Acquire) {
            s.stop(7);
        }
        if s.phase.load(Ordering::Acquire) == 3
            && s.in_flight.load(Ordering::Acquire) == 0
            && s.slots
                .iter()
                .all(|v| ![1, 2].contains(&v.state.load(Ordering::Acquire)))
        {
            let result = write_footer(&mut sink, s, written, valid).and_then(|_| sink.flush());
            crate::log::line(format_args!(
                "[ERPS-SYNC253] stopped written={written} valid={valid} reason={} footer_written={} (diagnostic only, not physics PASS)",
                s.reason.load(Ordering::Acquire),
                result.is_ok()
            ));
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn write_skin_trace(
    sink: &mut impl Write,
    trace: &crate::body_scale_port::SkinNormalTrace,
) -> io::Result<()> {
    write!(
        sink,
        "{{\"gate\":\"{}\",\"index\":{},\"observed\":{:?},\"expected\":{},\"matrix_bits\":",
        trace.gate, trace.index, trace.observed, trace.expected
    )?;
    if let Some(bits) = trace.matrix_bits {
        write!(sink, "{bits:?}")?;
    } else {
        sink.write_all(b"null")?;
    }
    sink.write_all(b"}")
}

fn write_footer(
    sink: &mut impl Write,
    s: &Session,
    written: usize,
    valid: usize,
) -> io::Result<()> {
    write!(
        sink,
        "{{\"type\":\"footer\",\"written\":{written},\"valid\":{valid},\"attempted\":{},\"reason\":{},\"human_cloth_acceptance\":false,\"skin_dropped\":{},\"skin_max_us\":{},\"skin_budget_exceeded\":{},\"skin_coverage\":[",
        s.next.load(Ordering::Acquire),
        s.reason.load(Ordering::Acquire),
        s.skin_dropped.load(Ordering::Acquire),
        s.skin_max_us.load(Ordering::Acquire),
        s.skin_budget_exceeded.load(Ordering::Acquire)
    )?;
    let coverage = s
        .skin
        .lock()
        .map_err(|_| io::Error::other("skin coverage mutex poisoned"))?;
    for (index, c) in coverage.iter().enumerate() {
        if index != 0 {
            sink.write_all(b",")?;
        }
        write!(
            sink,
            "{{\"op\":{},\"root\":{},\"buffer\":{},\"vertices\":{},\"calls\":{},\"pre_rejected\":{},\"post_rejected\":{},\"corrected_calls\":{},\"corrected_rows\":{},\"metadata_rejected\":{},\"guard_samples\":[",
            c.op,
            c.root,
            c.buffer,
            c.vertices,
            c.calls,
            c.pre_rejected,
            c.post_rejected,
            c.corrected_calls,
            c.corrected_rows,
            c.metadata_rejected
        )?;
        for (n, sample) in c.guard_samples.iter().take(c.calls as usize).enumerate() {
            if n != 0 {
                sink.write_all(b",")?;
            }
            let Some(sample) = sample else {
                sink.write_all(b"null")?;
                continue;
            };
            write!(sink, "{{\"frame\":{},\"pre\":", sample.frame)?;
            write_skin_trace(sink, &sample.pre)?;
            sink.write_all(b",\"post\":")?;
            if let Some(post) = sample.post {
                write_skin_trace(sink, &post)?;
            } else {
                sink.write_all(b"null")?;
            }
            sink.write_all(b"}")?;
        }
        sink.write_all(b"]}")?;
    }
    sink.write_all(b"],\"mesh_boundary\":")?;
    let mesh = s
        .mesh_boundary
        .lock()
        .map_err(|_| io::Error::other("mesh boundary mutex poisoned"))?;
    write_mesh_boundary(sink, &mesh)?;
    sink.write_all(b",\"scalar_skin_reach\":[")?;
    let reach = s
        .scalar_skin_reach
        .lock()
        .map_err(|_| io::Error::other("scalar skin mutex poisoned"))?;
    for (i, r) in reach.iter().enumerate() {
        if i != 0 {
            sink.write_all(b",")?;
        }
        write!(
            sink,
            "{{\"op\":{},\"root\":{},\"frame\":{},\"pre\":",
            r.op, r.root, r.sample.frame
        )?;
        write_skin_trace(sink, &r.sample.pre)?;
        sink.write_all(b",\"post\":")?;
        if let Some(post) = r.sample.post {
            write_skin_trace(sink, &post)?;
        } else {
            sink.write_all(b"null")?;
        }
        sink.write_all(b"}")?;
    }
    sink.write_all(b"]")?;
    ordered::write(sink, &s.ordered)?;
    writeln!(sink, "}}")
}

fn write_mesh_boundary(sink: &mut impl Write, mesh: &MeshBoundary) -> io::Result<()> {
    write!(
        sink,
        "{{\"version\":1,\"complete\":{},\"paired\":{},\"copy_us\":{},\"error\":\"{}\",\"consumer_id\":",
        mesh.complete, mesh.paired, mesh.copy_us, mesh.error
    )?;
    if let Some(id) = mesh.consumer_id {
        write!(sink, "{id}")?;
    } else {
        sink.write_all(b"null")?;
    }
    if let Some(source) = mesh.source {
        write!(
            sink,
            ",\"op\":{},\"input_buffer\":{},\"output_buffer\":{},\"frame_count\":{},\"snapshot\":",
            source.op, source.input_buffer, source.output_buffer, source.frame_count
        )?;
        // This is a producer boundary snapshot, not a primary pair. Do not
        // serialize the reused storage's default stage/native-RVA fields.
        let r = &mesh.record;
        let i = r.identity;
        write!(
            sink,
            "{{\"frame\":{},\"root\":{},\"model\":{},\"owner\":{},\"input\":{},\"generation\":{},\"scale_bits\":{},\"count\":{},\"native_enter_us\":{},\"native_exit_us\":{},\"blocks\":[",
            r.frame,
            i.root,
            i.model,
            i.owner,
            i.input,
            i.generation,
            i.scale_bits,
            i.count,
            r.native_enter_us,
            r.native_exit_us
        )?;
        for (index, b) in r.blocks.iter().enumerate() {
            if index != 0 {
                sink.write_all(b",")?;
            }
            write!(
                sink,
                "{{\"name\":\"{}\",\"index\":{},\"address\":{},\"length\":{},\"hex\":\"",
                b.name, b.index, b.address, b.length
            )?;
            for byte in &r.data[b.start..b.start + b.length] {
                write!(sink, "{byte:02x}")?;
            }
            sink.write_all(b"\"}")?;
        }
        sink.write_all(b"]}")?;
    }
    sink.write_all(b"}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_action_metadata_owns_known_payload_and_bounds_unknown_types() {
        let mut child = [0usize; 0x280 / 8];
        let mut wind = [0u32; 0x50 / 4];
        let unknown = [0xDEADusize, 0];
        let mut normals = [0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let base = 0x100000usize;
        wind[..2].copy_from_slice(&[(base + 0x2D89CE0) as u32, 0]);
        wind[0x20 / 4] = 1.0f32.to_bits();
        wind[0x3C / 4] = 2.0f32.to_bits();
        wind[0x40 / 4] = 3.0f32.to_bits();
        let actions = [wind.as_ptr() as usize, unknown.as_ptr() as usize];
        child[0x18 / 8] = 0x50000;
        child[0x28 / 8] = 2;
        child[0x40 / 8] = normals.as_ptr() as usize;
        child[0x48 / 8] = 2;
        child[0xE8 / 8] = actions.as_ptr() as usize;
        child[0xF0 / 8] = 2;
        let mut r = Record::new();
        r.identity = Identity {
            base,
            child: child.as_ptr() as usize,
            sim: 0x50000,
            count: 2,
            ..Identity::default()
        };
        r.stage = 4;
        r.strength_bits = 1.0f32.to_bits();
        r.valid = true;
        r.error = "none";
        crate::memory_query::scoped(|| capture_runtime_action_metadata(&mut r)).unwrap();
        assert!(
            r.blocks
                .iter()
                .any(|b| b.name == "runtime_actions_complete")
        );
        assert_eq!(
            r.blocks
                .iter()
                .filter(|b| b.name == "runtime_action_header")
                .count(),
            2
        );
        assert_eq!(
            r.blocks
                .iter()
                .filter(|b| b.name == "runtime_simple_wind")
                .count(),
            1
        );
        let owned = r.data.clone();
        wind.fill(0xEEEEEEEE);
        normals.fill(7.0);
        assert_eq!(r.data, owned);
        // Optional metadata remains bounded and explicitly incomplete on an
        // unsupported action-list count, instead of widening memory reads.
        child[0xF0 / 8] = 5;
        let mut bad = Record::new();
        bad.identity = r.identity;
        bad.identity.child = child.as_ptr() as usize;
        assert!(
            crate::memory_query::scoped(|| capture_runtime_action_metadata(&mut bad)).is_none()
        );
        assert!(
            !bad.blocks
                .iter()
                .any(|b| b.name == "runtime_actions_complete")
        );
        if let Ok(path) = std::env::var("ERPS_ACTION_METADATA_FIXTURE_PATH") {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap();
            write_record(&mut file, &r).unwrap();
        }
    }

    #[test]
    fn mesh_boundary_first_substep_does_not_consume_active_substep_slot() {
        let s = Session::new();
        s.phase.store(2, Ordering::Release);
        s.started.set(Instant::now()).unwrap();
        {
            let mut mesh = s.mesh_boundary.lock().unwrap();
            mesh.complete = true;
            mesh.record.frame = 42;
        }
        let queries = crate::memory_query::query_count();
        // Actual ER type5 two-substep order:0 then1 in the same frame.
        // Exercise both admission and the real per-frame slot reservation.
        let mut admitted = Vec::new();
        for (substep, strength) in [0.0f32, 1.0].into_iter().enumerate() {
            if boundary_pair_allowed(&s, 4, 3, 42, strength.to_bits())
                && let Some(id) = s.reserve(3, 4, 42)
            {
                admitted.push((substep, strength, id));
            }
        }
        assert_eq!(admitted, [(1, 1.0, 0)]);
        assert_eq!(crate::memory_query::query_count(), queries);
        assert_eq!(s.next.load(Ordering::Acquire), 1);
    }

    #[test]
    #[ignore = "requires private replay data; set ER_CHARACTER_SCALE_FIXTURES"]
    fn mesh_boundary_slow_identity_retains_owned_evidence_without_more_reads() {
        // Other native fixture tests advance the shared topology generation.
        // Hold their existing module lock while this owned identity is alive.
        let _lock = crate::body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
        let s = Session::new();
        s.phase.store(2, Ordering::Release);
        s.started.set(Instant::now()).unwrap();
        s.frame.store(42, Ordering::Release);
        let mut payload = crate::test_fixtures::bytes("mesh_boundary_249.bin");
        let address = payload.as_ptr() as usize;
        let mut input_header = [0u8; 0x118];
        let mut output_header = [0u8; 0x118];
        input_header[0x18..0x20].copy_from_slice(&address.to_le_bytes());
        input_header[0x20..0x24].copy_from_slice(&448u32.to_le_bytes());
        input_header[0x24] = 16;
        output_header[0x18..0x20].copy_from_slice(&(address + 80128).to_le_bytes());
        output_header[0x20..0x24].copy_from_slice(&289u32.to_le_bytes());
        output_header[0x24] = 16;
        output_header[0x40..0x48].copy_from_slice(&(address + 84752).to_le_bytes());
        output_header[0x4C] = 16;
        for header in [&mut input_header, &mut output_header] {
            for base in [0x90, 0xD0] {
                for off in [0, 20, 40, 60] {
                    header[base + off..base + off + 4].copy_from_slice(&1f32.to_le_bytes());
                }
            }
        }
        let source = MeshBoundarySource {
            op: 0x1110,
            input_buffer: input_header.as_ptr() as usize,
            output_buffer: output_header.as_ptr() as usize,
            positions: address,
            particles: 448,
            frames: address + 7168,
            frame_count: 570,
            binds: address + 43648,
            model: 1,
            owner: 2,
            input: 3,
            generation: crate::body_scale_port::cloth_topology_generation(),
            scale_bits: 0.5f32.to_bits(),
        };
        let identity = Identity {
            root: 4,
            model: 1,
            owner: 2,
            input: 3,
            generation: source.generation,
            scale_bits: 0.5f32.to_bits(),
            count: 289,
            ..Identity::default()
        };
        s.targets.set([identity; 4]).unwrap();
        assert!(!boundary_pair_allowed(&s, 8, 3, 42, 1.0f32.to_bits()));
        // RED control: reproduce the old contact-first policy with the actual
        // production before-copy path and an injected slow identity operation.
        let old = Session::new();
        old.phase.store(2, Ordering::Release);
        old.started.set(Instant::now()).unwrap();
        let mut lost = Record::new();
        capture_pair_before(&old, &mut lost, || {
            std::thread::sleep(Duration::from_millis(5));
            true
        });
        assert_eq!(lost.error, "copy_budget");
        assert!(!old.mesh_boundary.lock().unwrap().complete);
        assert!(old.scalar_skin_reach.lock().unwrap().is_empty());
        let pre = crate::body_scale_port::SkinNormalTrace {
            gate: "accepted",
            observed: [4, source.output_buffer, 0, 289],
            expected: 0.5f32.to_bits() as usize,
            ..Default::default()
        };
        let post = crate::body_scale_port::SkinNormalTrace {
            gate: "corrected",
            observed: [address + 84752, 289, 16, 289],
            expected: 0.5f32.to_bits() as usize,
            ..Default::default()
        };
        let queries = crate::memory_query::query_count();
        retain_scalar_skin_reach(&s, 0x1100, pre, Some(post));
        assert_eq!(crate::memory_query::query_count(), queries);
        {
            let mut mesh = s.mesh_boundary.lock().unwrap();
            crate::memory_query::scoped(|| mesh.start(source, identity, 42)).unwrap();
            assert!(!mesh.complete);
            crate::memory_query::scoped(|| {
                mesh.finish(source.op, source.output_buffer, address + 84752, 42)
            })
            .unwrap();
            assert!(mesh.complete);
        }
        assert!(boundary_pair_allowed(&s, 4, 3, 42, 1.0f32.to_bits()));
        assert!(!boundary_pair_allowed(&s, 4, 3, 43, 1.0f32.to_bits()));
        assert!(!boundary_pair_allowed(&s, 8, 3, 42, 1.0f32.to_bits()));
        let mut r = Record::new();
        {
            let mut mesh = s.mesh_boundary.lock().unwrap();
            mesh.paired = true;
            mesh.consumer_id = Some(r.id);
        }
        // GREEN: same slow identity is still a hard FAIL for the pair. Earlier
        // producer bytes and scalar correction reach remain valid and owned.
        capture_pair_before(&s, &mut r, || {
            std::thread::sleep(Duration::from_millis(5));
            true
        });
        assert_eq!(r.error, "copy_budget");
        assert!(r.identity_before_us > COPY_BUDGET_US);
        assert!(r.blocks.is_empty());
        assert_eq!(r.before_queries, 0);
        assert_eq!(s.reason.load(Ordering::Acquire), 2);
        let original = s.mesh_boundary.lock().unwrap().record.data.clone();
        payload.fill(0xEE);
        assert_eq!(s.mesh_boundary.lock().unwrap().record.data, original);
        retain_scalar_skin_reach(&s, 0x1200, pre, Some(post));
        assert_eq!(s.scalar_skin_reach.lock().unwrap().len(), 1);
        if let Ok(path) = std::env::var("ERPS_MESH_BOUNDARY_FIXTURE_PATH") {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap();
            write_footer(&mut file, &s, 0, 0).unwrap();
        }
    }
    #[test]
    fn mesh_boundary_rejects_wrong_owner_scale_and_partial_output() {
        let mut mesh = MeshBoundary::new();
        let source = MeshBoundarySource {
            op: 1,
            input_buffer: 0,
            output_buffer: 0,
            positions: 0,
            particles: 448,
            frames: 0,
            frame_count: 570,
            binds: 0,
            model: 1,
            owner: 2,
            input: 3,
            generation: 7,
            scale_bits: 0.5f32.to_bits(),
        };
        let identity = Identity {
            count: 289,
            model: 99,
            ..Identity::default()
        };
        assert!(mesh.start(source, identity, 1).is_none());
        assert!(mesh.source.is_none());
        assert!(mesh.finish(1, 0, 0, 1).is_none());
        assert!(!mesh.complete);
        assert!(mesh.record.data.is_empty());
    }
    #[test]
    fn dispatch_indices_read_only_caps_and_bad_inputs() {
        let indices = [0u16, 64, 159];
        let original = indices;
        let header = [indices.as_ptr() as usize, 3];
        let mut t = CollisionDispatch::new(160, 0x1000, 2);
        t.observe(2, 0x10A0, header.as_ptr() as usize);
        assert_eq!(t.error, "none");
        assert_eq!(t.len, 1);
        assert_eq!(t.events[0].collider, 1);
        assert_eq!(t.events[0].bits[..3], [1, 1, 1 << 31]);
        assert_eq!(indices, original);
        t.observer_us = 500;
        t.observe(2, 0x10A0, header.as_ptr() as usize);
        assert_eq!(t.error, "observer_budget");
        assert_eq!(t.len, 1);
        t.error = "none";
        t.observer_us = 0;
        t.len = 64;
        t.observe(2, 0x10A0, header.as_ptr() as usize);
        assert_eq!(t.error, "event_cap");
        for (ids, collider) in [
            ([0u16, 0, 1], 0x1000),
            ([0, 1, 160], 0x1000),
            ([0, 1, 2], 0x1001),
        ] {
            let h = [ids.as_ptr() as usize, 3];
            let mut t = CollisionDispatch::new(160, 0x1000, 2);
            t.observe(2, collider, h.as_ptr() as usize);
            assert_eq!(t.error, "dispatch_identity_or_indices");
            assert_eq!(t.len, 0);
        }
        let mut t = CollisionDispatch::new(160, 0x1000, 2);
        t.observe(0, 0x1000, 1);
        assert_eq!(t.error, "dispatch_identity_or_indices");
    }
    #[test]
    fn dispatch_scope_suspends_nested_and_restores_on_unwind() {
        let outer = DispatchScope::enter(Some(CollisionDispatch::new(160, 1, 1)));
        std::thread::spawn(|| COLLISION_DISPATCH.with(|t| assert!(t.borrow().is_none())))
            .join()
            .unwrap();
        let inner = DispatchScope::enter(None);
        COLLISION_DISPATCH.with(|t| assert!(t.borrow().is_none()));
        assert!(inner.finish().is_none());
        assert_eq!(outer.finish().unwrap().error, "nested_pass");
        let _ = std::panic::catch_unwind(|| {
            let _scope = DispatchScope::enter(Some(CollisionDispatch::new(160, 1, 1)));
            panic!("test unwind");
        });
        COLLISION_DISPATCH.with(|t| assert!(t.borrow().is_none()));
    }
    #[test]
    fn dispatch_callsite_hook_preserves_relative_call_and_arguments() {
        use windows::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_PROTECTION_FLAGS,
            PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
        };
        let allocation =
            unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        assert!(!allocation.is_null());
        let base = allocation as usize;
        let mut caller = vec![
            0x48, 0x83, 0xEC, 0x28, 0xE8, 0, 0, 0, 0, 0x48, 0x83, 0xC4, 0x28, 0xC3,
        ];
        caller[5..9].copy_from_slice(&(0x100i32 - 9).to_le_bytes());
        let callee = [
            0x48, 0xFF, 0x01, 0x66, 0x0F, 0x7E, 0xD0, 0x48, 0x01, 0xD0, 0x4C, 0x01, 0xC8, 0xC3,
        ];
        unsafe {
            std::ptr::write_bytes(allocation, 0x90, 4096);
            std::ptr::copy_nonoverlapping(caller.as_ptr(), allocation.cast::<u8>(), caller.len());
            std::ptr::copy_nonoverlapping(callee.as_ptr(), (base + 0x100) as *mut u8, callee.len());
        }
        let mut old = PAGE_PROTECTION_FLAGS::default();
        unsafe {
            VirtualProtect(allocation, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
        }
        type F = unsafe extern "C" fn(*mut u64, usize, f32, usize) -> usize;
        let f = unsafe { std::mem::transmute::<usize, F>(base) };
        let mut count = 0;
        let expected = 0.25f32.to_bits() as usize + 31;
        assert_eq!(unsafe { f(&mut count, 11, 0.25, 20) }, expected);
        let hook = unsafe {
            hook_closure_jmp_back(
                base + 4,
                |r| observe_dispatch(0, r),
                CallbackOption::None,
                HookFlags::empty(),
            )
            .unwrap()
        };
        for _ in 0..100 {
            assert_eq!(unsafe { f(&mut count, 11, 0.25, 20) }, expected);
        }
        assert_eq!(count, 101);
        let ids = [0u16, 2, 4];
        let list = [ids.as_ptr() as usize, 3];
        let scope = DispatchScope::enter(Some(CollisionDispatch::new(
            5,
            (&mut count as *mut u64) as usize,
            1,
        )));
        assert_eq!(
            unsafe { f(&mut count, list.as_ptr() as usize, 0.25, 20) },
            0.25f32.to_bits() as usize + list.as_ptr() as usize + 20
        );
        let trace = scope.finish().unwrap();
        assert_eq!(trace.error, "none");
        assert_eq!(trace.len, 1);
        assert_eq!(count, 102);
        drop(hook);
        unsafe {
            VirtualFree(allocation, 0, MEM_RELEASE).unwrap();
        }
    }
    #[test]
    fn contact_first_schedule_covers_all_buckets_without_early_constraint_starvation() {
        let s = Session::new();
        let mut captured = Vec::new();
        // Native order can offer constraints and the enclosing collision pass
        // before narrow contacts. They must not spend the early contact budget.
        for frame in 1..=300 {
            let elapsed = Duration::from_micros((frame - 1) * 16_667);
            let mut this_frame = 0;
            for bucket in 0..4 {
                for stage in [4, 0, 1, 2, 3, 5, 6, 7, 8, 9] {
                    if stage_allowed(stage, elapsed) && s.reserve(bucket, stage, frame).is_some() {
                        captured.push((bucket, stage, elapsed));
                        this_frame += 1;
                    }
                }
            }
            assert!(this_frame <= 1);
        }
        assert_eq!(captured.len(), RECORDS);
        for bucket in 0..4 {
            for stage in [8, 9] {
                assert!(captured.iter().any(|&(b, st, t)| b == bucket
                    && st == stage
                    && t < Duration::from_millis(600)));
            }
            assert!(captured.iter().any(|&(b, st, t)| b == bucket
                && st == 7
                && t >= Duration::from_millis(600)
                && t < Duration::from_millis(1200)));
        }
        assert!(
            captured
                .iter()
                .filter(|&&(_, stage, _)| stage <= 6)
                .all(|&(_, _, t)| t >= Duration::from_millis(1200))
        );
        assert!(!stage_allowed(8, WINDOW));
        assert!(!stage_allowed(STAGES, Duration::ZERO));
    }
    #[test]
    fn capsule_capture_targets_the_branch_observed_in_returned_246() {
        // All four returned roots reached site16036A4,not the old160305B.
        // Keep the existing stage/record cap while moving its detailed sampler.
        assert_eq!(RVAS[8], 0x15F0DE0);
        for stage in 0..STAGES {
            assert!(!dispatch_capture_enabled(stage));
        }
        assert_eq!(
            PROLOGUES[8],
            &[
                0x48, 0x8B, 0xC4, 0x4C, 0x89, 0x48, 0x20, 0x4C, 0x89, 0x40, 0x18, 0x48, 0x89, 0x50,
                0x10
            ]
        );
    }
    #[test]
    fn returned_245_skin_overrun_must_not_cancel_unstarted_collision_capture() {
        // Exact reported failure: first SkinPN observer9585us,zero pair attempts.
        // Drive the same completion seam used by observe_skin,not a timing guess.
        let s = Session::new();
        s.phase.store(2, Ordering::Release);
        s.finish_skin_observation(9585);
        assert_eq!(s.skin_max_us.load(Ordering::Acquire), 9585);
        assert_eq!(
            s.phase.load(Ordering::Acquire),
            2,
            "auxiliary observer killed primary capture"
        );
        assert_eq!(s.reason.load(Ordering::Acquire), 0);
        assert!(s.skin_budget_exceeded.load(Ordering::Acquire));
        assert!(s.admit_skin(Duration::from_millis(1500)).is_none());
        assert!(s.reserve(0, 7, 1).is_some());
        s.stop(1);
        assert_eq!(s.phase.load(Ordering::Acquire), 3);
    }
    #[test]
    fn auxiliary_skin_admission_defers_reads_and_keeps_original_limits() {
        let s = Session::new();
        let queries = crate::memory_query::query_count();
        for phase in [0, 1, 3] {
            s.phase.store(phase, Ordering::Release);
            assert!(s.admit_skin(Duration::from_millis(1500)).is_none());
        }
        s.phase.store(2, Ordering::Release);
        for elapsed in [0, 599, 600, 1199, 5000, 6000] {
            assert!(s.admit_skin(Duration::from_millis(elapsed)).is_none());
        }
        for elapsed in [1200, 4999] {
            assert!(s.admit_skin(Duration::from_millis(elapsed)).is_some());
        }
        s.finish_skin_observation(SKIN_BUDGET_US);
        assert!(s.admit_skin(Duration::from_millis(1200)).is_some());
        s.finish_skin_observation(SKIN_BUDGET_US + 1);
        for _ in 0..1000 {
            assert!(s.admit_skin(Duration::from_millis(1200)).is_none());
        }
        assert_eq!(crate::memory_query::query_count(), queries);
        assert_eq!(COPY_BUDGET_US, 4000);
        assert_eq!(SKIN_BUDGET_US, 2000);
        assert_eq!(RECORDS, 120);
        let mut sink = Vec::new();
        write_footer(&mut sink, &s, 0, 0).unwrap();
        assert!(
            String::from_utf8(sink)
                .unwrap()
                .contains("\"skin_budget_exceeded\":true")
        );
    }
    #[test]
    fn auxiliary_admission_nonblocking_and_unwind_release() {
        let s = Session::new();
        s.phase.store(2, Ordering::Release);
        let time = Duration::from_millis(1200);
        let guard = s.admit_skin(time).unwrap();
        std::thread::scope(|scope| {
            scope
                .spawn(|| assert!(s.admit_skin(time).is_none()))
                .join()
                .unwrap();
        });
        assert_eq!(s.skin_dropped.load(Ordering::Acquire), 1);
        drop(guard);
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = s.admit_skin(time).unwrap();
                panic!("owned observer unwind");
            }))
            .is_err()
        );
        assert!(s.admit_skin(time).is_some());
        let guard = s.admit_skin(time).unwrap();
        s.stop(2); // Pair-budget stop must still stop EVERY observer/channel.
        drop(guard);
        assert!(s.admit_skin(time).is_none());
        assert_eq!(s.reason.load(Ordering::Acquire), 2);
    }
    #[test]
    fn absent_contact_stage_does_not_block_later_windows() {
        assert!(!stage_allowed(4, Duration::ZERO));
        assert!(stage_allowed(7, Duration::from_millis(600)));
        assert!(!stage_allowed(8, Duration::from_millis(600)));
        assert!(stage_allowed(4, Duration::from_millis(1200)));
        assert!(stage_allowed(8, Duration::from_millis(1200)));
    }
    #[test]
    fn capped_skin_keys_only_skip_reads_and_distinguish_roots_and_operators() {
        let mut coverage = vec![SkinCoverage {
            op: 1,
            root: 2,
            ..SkinCoverage::default()
        }];
        for calls in 0..3 {
            coverage[0].calls = calls;
            assert!(skin_needs_observation(&coverage, 1, 2));
        }
        coverage[0].calls = 3;
        assert!(!skin_needs_observation(&coverage, 1, 2));
        assert!(skin_needs_observation(&coverage, 1, 3));
        assert!(skin_needs_observation(&coverage, 3, 2));
    }
    #[test]
    fn guard_samples_keep_first_three_and_preserve_raw_matrix_bits() {
        use crate::body_scale_port::SkinNormalTrace;
        let mut coverage = SkinCoverage::default();
        let rejected = SkinNormalTrace {
            gate: "source_bone_basis",
            matrix_bits: Some([0x7FC00011; 16]),
            ..Default::default()
        };
        let accepted = SkinNormalTrace {
            gate: "accepted",
            ..Default::default()
        };
        let corrected = SkinNormalTrace {
            gate: "corrected",
            ..Default::default()
        };
        for (frame, pre, rows, a, b) in [
            (1, false, None, rejected, None),
            (2, true, None, accepted, Some(rejected)),
            (3, true, Some(347), accepted, Some(corrected)),
        ] {
            assert!(coverage.observe(
                pre,
                rows,
                SkinGuardSample {
                    frame,
                    pre: a,
                    post: b
                }
            ));
        }
        assert!(!coverage.observe(
            true,
            Some(347),
            SkinGuardSample {
                frame: 4,
                pre: accepted,
                post: Some(corrected)
            }
        ));
        assert_eq!(
            (
                coverage.calls,
                coverage.pre_rejected,
                coverage.post_rejected,
                coverage.corrected_calls,
                coverage.corrected_rows
            ),
            (3, 1, 1, 1, 347)
        );
        assert_eq!(
            coverage.guard_samples[0].unwrap().pre.matrix_bits,
            Some([0x7FC00011; 16])
        );
        assert_eq!(coverage.guard_samples[2].unwrap().frame, 3);
        let mut bytes = Vec::new();
        write_skin_trace(&mut bytes, &rejected).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("2143289361"));
        assert!(!text.contains("NaN"));
    }

    #[test]
    fn link_points_require_fresh_correction_and_never_write_source() {
        let memory = [[
            0.2f32.to_bits(),
            0.5f32.to_bits(),
            0.8f32.to_bits(),
            0x7FC00011,
        ]; 5];
        let before = memory;
        let pre = crate::body_scale_port::SkinNormalTrace {
            gate: "accepted",
            expected: 0.5f32.to_bits() as usize,
            ..Default::default()
        };
        let mut post = crate::body_scale_port::SkinNormalTrace {
            gate: "corrected",
            observed: [memory.as_ptr() as usize, 5, 16, 5],
            expected: pre.expected,
            ..Default::default()
        };
        let points = crate::memory_query::scoped(|| read_normal_points(pre, Some(post))).unwrap();
        let expected: [u32; 3] = memory[0][..3].try_into().unwrap();
        assert_eq!(points, [expected; 3]);
        assert_eq!(memory, before);
        let bad = crate::body_scale_port::SkinNormalTrace {
            gate: "source_bone_basis",
            ..pre
        };
        let queries = crate::memory_query::query_count();
        assert!(read_normal_points(bad, Some(post)).is_none());
        assert_eq!(queries, crate::memory_query::query_count());
        post.observed[2] = 8;
        assert!(read_normal_points(pre, Some(post)).is_none());
        post.observed[2] = 16;
        post.observed[3] = 4;
        assert!(read_normal_points(pre, Some(post)).is_none());
        post.observed[3] = 5;
        post.observed[0] = 1;
        assert!(crate::memory_query::scoped(|| read_normal_points(pre, Some(post))).is_none());
    }

    #[test]
    fn link_duplicate_and_frame_budget_cannot_report_stale_points_as_latest() {
        let _lock = crate::body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
        let s = Session::new();
        s.started.set(Instant::now()).unwrap();
        s.frame.store(42, Ordering::Relaxed);
        let memory = [[1f32.to_bits(), 0, 0, 0]; 5];
        let pre = crate::body_scale_port::SkinNormalTrace {
            gate: "accepted",
            observed: [3, 5, 0, 5],
            expected: 0.5f32.to_bits() as usize,
            ..Default::default()
        };
        let post = crate::body_scale_port::SkinNormalTrace {
            gate: "corrected",
            observed: [memory.as_ptr() as usize, 5, 16, 5],
            expected: pre.expected,
            ..Default::default()
        };
        let mut c = SkinCoverage {
            op: 1,
            root: 3,
            buffer: 5,
            ..Default::default()
        };
        let sample = SkinGuardSample {
            frame: 42,
            pre,
            post: Some(post),
        };
        crate::memory_query::scoped(|| update_skin_link(&s, &mut c, sample));
        assert!(c.latest.unwrap().probes.is_some());
        let queries = crate::memory_query::query_count();
        update_skin_link(&s, &mut c, sample);
        assert!(c.latest.unwrap().duplicate);
        assert!(c.latest.unwrap().probes.is_none());
        assert_eq!(queries, crate::memory_query::query_count());
        s.frame.store(43, Ordering::Relaxed);
        s.link_budget
            .store((43 << 20) | LINK_FRAME_BUDGET_US, Ordering::Relaxed);
        update_skin_link(
            &s,
            &mut c,
            SkinGuardSample {
                frame: 43,
                ..sample
            },
        );
        assert_eq!(c.latest.unwrap().sample.frame, 42);
        s.frame.store(44, Ordering::Relaxed);
        crate::memory_query::scoped(|| {
            update_skin_link(
                &s,
                &mut c,
                SkinGuardSample {
                    frame: 44,
                    ..sample
                },
            )
        });
        assert_eq!(c.latest.unwrap().sample.frame, 44);
        assert!(!c.latest.unwrap().duplicate);
        assert!(c.latest.unwrap().probes.is_some());
        let allocated = std::mem::size_of::<Session>()
            + RECORDS
                * (std::mem::size_of::<Slot>()
                    + std::mem::size_of::<Record>()
                    + BYTES
                    + BLOCKS * std::mem::size_of::<Block>())
            + 16 * std::mem::size_of::<SkinCoverage>();
        assert!(
            allocated < 16 * 1024 * 1024,
            "recorder allocation {allocated}"
        );
        let timer = Instant::now();
        for frame in 45..1045 {
            s.frame.store(frame, Ordering::Relaxed);
            crate::memory_query::scoped(|| {
                update_skin_link(&s, &mut c, SkinGuardSample { frame, ..sample })
            });
        }
        println!(
            "LINK-PROBE owned mean_us={:.3},recorder_bytes={allocated}",
            timer.elapsed().as_secs_f64() * 1000.
        );
    }
    #[test]
    fn returned_240_copy_costs_fit_one_record_not_two_per_frame() {
        // Frozen human19476: before/after1094/1042 and819/1405 microseconds.
        // A scheduler regression, NOT an in-game performance benchmark.
        for (before, after) in [(1094, 1042), (819, 1405)] {
            assert!(before + after <= COPY_BUDGET_US);
        }
        let s = Session::new();
        assert!(s.reserve(0, 8, 1).is_some());
        assert!(s.reserve(1, 9, 1).is_none());
    }
    #[test]
    fn production_contact_jsonl_fixture_export() {
        fn add(r: &mut Record, name: &'static str, index: usize, address: usize, raw: &[u8]) {
            r.copy(name, index, raw.as_ptr() as usize, raw.len())
                .unwrap();
            r.blocks.last_mut().unwrap().address = address;
        }
        let base = 0x140000000usize;
        let session = Session::new();
        let mut sink = Vec::new();
        writeln!(sink,"{{\"type\":\"header\",\"schema\":\"er-cloth-sync-3\",\"skin_basis_profile_version\":1,\"contact_profile_version\":1,\"capsule_apply_rva\":23006688,\"dispatch_sets_enabled\":false,\"observer_isolation_version\":1,\"skin_start_ms\":1200,\"skin_budget_us\":2000,\"skin_guard_trace_version\":1,\"skin_link_version\":1,\"collision_dispatch_version\":1,\"base\":{base},\"model\":\"BD_M_9004\",\"synthetic_test\":true}}").unwrap();
        let mut id = 0;
        for n in COUNTS {
            let mut coverage = SkinCoverage {
                op: 0x1000 + n,
                root: 3,
                buffer: 5,
                vertices: if n == 448 { 464 } else { n },
                ..SkinCoverage::default()
            };
            let bad = crate::body_scale_port::SkinNormalTrace {
                gate: "source_bone_basis",
                index: 7,
                observed: [73, 0x9000, 107, 16],
                expected: 0.5f32.to_bits() as usize,
                matrix_bits: Some([0x7FC00011; 16]),
            };
            let ok = crate::body_scale_port::SkinNormalTrace {
                gate: "accepted",
                ..Default::default()
            };
            let corrected = crate::body_scale_port::SkinNormalTrace {
                gate: "corrected",
                observed: [0, n, 16, n],
                ..Default::default()
            };
            assert!(coverage.observe(
                false,
                None,
                SkinGuardSample {
                    frame: 1,
                    pre: bad,
                    post: None
                }
            ));
            assert!(coverage.observe(
                true,
                Some(n),
                SkinGuardSample {
                    frame: 2,
                    pre: ok,
                    post: Some(corrected)
                }
            ));
            session.skin.lock().unwrap().push(coverage);
            for stage in [4, 7, 8, 9] {
                let mut r = Record::new();
                r.id = id;
                r.stage = stage;
                r.frame = id as u64 + 1;
                r.identity = Identity {
                    base,
                    child: 0x1000 + n,
                    count: n,
                    root: 3,
                    scale_bits: 0.5f32.to_bits(),
                    ..Identity::default()
                };
                r.object = if stage == 9 { 0x100A0 } else { 0x10000 };
                r.valid = true;
                r.error = "none";
                let raw = vec![[1., 2., 3., f32::from_bits(0xFFFFFFFF)]; n];
                let bytes =
                    unsafe { std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), n * 16) };
                for (name, address) in [
                    ("current_before", 0x30000),
                    ("current_after", 0x30000),
                    ("previous_before", 0x40000),
                    ("previous_after", 0x40000),
                    ("particle_data", 0x50000),
                ] {
                    add(&mut r, name, 0, address, bytes);
                }
                let mut layout = [0u8; 16];
                layout[..4].copy_from_slice(&2u32.to_le_bytes());
                layout[8..].copy_from_slice(&0x10000usize.to_le_bytes());
                add(&mut r, "consumer_layout", 0, 0x270, &layout);
                for index in 0..2 {
                    let address = 0x10000 + index * 0xA0;
                    let shape_address: usize = 0x20000 + index * 0x1000;
                    let mut collider = [0u8; 0xA0];
                    collider[0x88..0x90].copy_from_slice(&shape_address.to_le_bytes());
                    add(&mut r, "consumer_before", index, address, &collider);
                    add(&mut r, "consumer_after", index, address, &collider);
                    let mut shape = vec![0u8; if index == 0 { 0x60 } else { 0xB0 }];
                    shape[..8].copy_from_slice(
                        &(base + if index == 0 { 0x2D896D0 } else { 0x2D7DBC8 }).to_le_bytes(),
                    );
                    add(&mut r, "consumer_shape", index, shape_address, &shape);
                }
                if stage == 4 {
                    let mut constraint = [0u8; 0x48];
                    constraint[0x44] = 1;
                    add(&mut r, "constraint_header", 0, 0x60000, &constraint);
                    let vertices = if n == 448 { 464 } else { n };
                    let mut rows = [0u8; 16];
                    rows[2..4].copy_from_slice(&((vertices - 1) as u16).to_le_bytes());
                    add(&mut r, "constraint_rows", 0, 0x61000, &rows);
                    let mut header = [0u8; 0x118];
                    header[0x20..0x24].copy_from_slice(&(vertices as u32).to_le_bytes());
                    header[0x24] = 16;
                    header[0x40..0x48].copy_from_slice(&0x64000usize.to_le_bytes());
                    header[0x4C] = 16;
                    add(&mut r, "reference_selector", 0, 0x62000, &header);
                    add(&mut r, "reference_selected", 0, 0x62000, &header);
                    add(
                        &mut r,
                        "reference_positions",
                        0,
                        0x63000,
                        &vec![0u8; vertices * 16],
                    );
                    let normals = [0u32, 1f32.to_bits(), 0, 0x7FC00011]
                        .map(u32::to_le_bytes)
                        .concat()
                        .repeat(vertices);
                    add(&mut r, "reference_normals", 0, 0x64000, &normals);
                    r.skin_read_start_us = 100;
                    r.skin_links[0] = Some(SkinLink {
                        op: n,
                        root: 3,
                        buffer: 0x62000,
                        generation: 0,
                        scale_bits: 0.5f32.to_bits(),
                        captured_us: 99,
                        sample: SkinGuardSample {
                            frame: r.frame,
                            pre: crate::body_scale_port::SkinNormalTrace {
                                gate: "accepted",
                                observed: [3, 0x62000, 0, vertices],
                                expected: 0.5f32.to_bits() as usize,
                                ..Default::default()
                            },
                            post: Some(crate::body_scale_port::SkinNormalTrace {
                                gate: "corrected",
                                observed: [0x64000, vertices, 16, vertices],
                                expected: 0.5f32.to_bits() as usize,
                                ..Default::default()
                            }),
                        },
                        probes: Some([[0, 1f32.to_bits(), 0]; 3]),
                        duplicate: false,
                    });
                } else {
                    if stage == 7 && CAPTURE_DISPATCH_SETS {
                        let mut t = CollisionDispatch::new(n, 0x10000, 2);
                        t.events[0] = DispatchEvent {
                            site: 0,
                            collider: 0,
                            count: 2,
                            ..Default::default()
                        };
                        t.events[0].bits[0] = 1;
                        t.events[0].bits[(n - 1) / 64] |= 1u64 << ((n - 1) % 64);
                        t.len = 1;
                        r.collision_dispatch = Some(t);
                    }
                    add(&mut r, "collision_masks", 0, 0x60000, &vec![0u8; n * 4]);
                    if stage >= 8 {
                        let mut indices = vec![0u8; 4];
                        indices[2..].copy_from_slice(&((n - 1) as u16).to_le_bytes());
                        for (name, header_name, address, data) in [
                            (
                                "contact_indices",
                                "contact_indices_header",
                                0x61000usize,
                                indices,
                            ),
                            (
                                "contact_radii",
                                "contact_radii_header",
                                0x62000,
                                [0.005f32.to_le_bytes(); 2].concat(),
                            ),
                            (
                                "contact_friction",
                                "contact_friction_header",
                                0x63000,
                                [0.5f32.to_le_bytes(); 2].concat(),
                            ),
                        ] {
                            let mut header = [0u8; 16];
                            header[..8].copy_from_slice(&address.to_le_bytes());
                            header[8..12].copy_from_slice(&2i32.to_le_bytes());
                            add(&mut r, header_name, 0, address + 100, &header);
                            add(&mut r, name, 0, address, &data);
                        }
                    }
                }
                write_record(&mut sink, &r).unwrap();
                id += 1;
            }
        }
        session.next.store(id, Ordering::Relaxed);
        session.reason.store(1, Ordering::Relaxed);
        let footer_start = sink.len();
        write_footer(&mut sink, &session, id, id).unwrap();
        if let Ok(path) = std::env::var("ERPS_CONTACT_FIXTURE_PATH") {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap()
                .write_all(&sink)
                .unwrap();
        }
        if let Ok(path) = std::env::var("ERPS_ISOLATION_OVERRUN_FIXTURE_PATH") {
            sink.truncate(footer_start);
            session.finish_skin_observation(9585);
            write_footer(&mut sink, &session, id, id).unwrap();
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap()
                .write_all(&sink)
                .unwrap();
        }
        assert!(sink.len() < 2 * 1024 * 1024);
    }
    #[test]
    fn collision_copies_and_selected_contact_inputs_are_read_only_and_revalidated() {
        let mut memory = vec![0usize; 0x10000 / 8];
        let h = memory.as_mut_ptr() as usize;
        let b = 0x140000000usize;
        let put = |at: usize, v: usize| unsafe { ((h + at) as *mut usize).write(v) };
        for (at, off) in [
            (0x118, 0x800),
            (0x368, 0x1000),
            (0x120, 0x2000),
            (0x130, 0x2100),
            (0x840, 0x2200),
            (0x290, 0x4000),
            (0x4088, 0x5000),
            (0x8E8, 0x6000),
            (0x7000, 0x7200),
            (0x7040, 0x7240),
            (0x7080, 0x7280),
        ] {
            put(at, h + off);
        }
        for (at, v) in [
            (0x100, b + 0x2D872A0),
            (0x800, b + 0x2D8B6F8),
            (0x848, 2),
            (0x288, 1),
            (0x8F0, 2),
            (0x7008, 2),
            (0x7048, 2),
            (0x7088, 2),
        ] {
            put(at, v);
        }
        put(0x7200, 1usize << 16); // Actual selected particle indices 0,1.
        let identity = Identity {
            base: b,
            child: h + 0x100,
            sim: h + 0x800,
            root: h + 0x1000,
            count: 2,
            scale_bits: 0.5f32.to_bits(),
            ..Identity::default()
        };
        for stage in 7..10 {
            put(0x5000, b + if stage == 8 { 0x2D896D0 } else { 0x2D7DBC8 });
            let mut r = Record::new();
            r.identity = identity;
            r.stage = stage;
            r.object = h + 0x4000;
            r.contact = ContactArgs {
                indices: h + 0x7000,
                radii: h + 0x7040,
                friction: h + 0x7080,
            };
            let before = memory.clone();
            crate::memory_query::scoped(|| capture_before(&mut r)).unwrap();
            assert_eq!(memory, before);
            assert!(r.blocks.iter().any(|v| v.name == "consumer_before"
                && v.address == h + 0x4000
                && v.length == 0xA0));
            if stage >= 8 {
                assert!(
                    r.blocks
                        .iter()
                        .any(|v| v.name == "contact_indices" && v.length == 4)
                );
            }
            put(0x2000, 0x3f000000);
            put(0x2100, 0x3e800000);
            let after = memory.clone();
            crate::memory_query::scoped(|| capture_after(&mut r)).unwrap();
            assert_eq!(memory, after);
            assert!(r.blocks.iter().any(|v| v.name == "previous_after"));
            put(0x290, h + 0x4100);
            assert!(crate::memory_query::scoped(|| capture_after(&mut r)).is_none());
            put(0x290, h + 0x4000);
            if stage >= 8 {
                put(0x7008, 1);
                assert!(crate::memory_query::scoped(|| capture_after(&mut r)).is_none());
                put(0x7008, 2);
                r.object = h + 0x4001;
                assert!(crate::memory_query::scoped(|| contact_inputs(&mut r)).is_none());
                r.object = h + 0x4000;
                put(0x7200, 2);
                assert!(crate::memory_query::scoped(|| contact_inputs(&mut r)).is_none());
                put(0x7200, 1usize << 16);
            }
        }
    }
    #[test]
    fn reference_copy_includes_vertices_above_simulation_count() {
        let mut memory = vec![0usize; 0x10000 / 8];
        let h = memory.as_mut_ptr() as usize;
        let put = |at: usize, v: usize| unsafe { ((h + at) as *mut usize).write(v) };
        for (at, off) in [
            (0x1020, 0x2000),
            (0x2000, 0x3000),
            (0x3018, 0x4000),
            (0x3040, 0x6000),
        ] {
            put(at, h + off);
        }
        put(0x1028, 1);
        put(0x3020, 464 | (16usize << 32));
        unsafe {
            *((h + 0x304C) as *mut u8) = 16;
            *((h + 0x144) as *mut u8) = 1;
        }
        let mut r = Record::new();
        r.stage = 4;
        r.object = h + 0x100;
        r.identity = Identity {
            root: h + 0x1000,
            count: 448,
            ..Identity::default()
        };
        crate::memory_query::scoped(|| reference(&mut r)).unwrap();
        for name in ["reference_positions", "reference_normals"] {
            assert_eq!(
                r.blocks.iter().find(|b| b.name == name).unwrap().length,
                464 * 16
            );
        }
        put(0x3020, 1025 | (16usize << 32));
        assert!(crate::memory_query::scoped(|| reference(&mut r)).is_none());
    }
    #[test]
    fn collision_native_prologue_trampolines_forward_stack_float_and_return() {
        // ilhook documents concurrent hook/unhook protection changes as unsafe.
        // Share the existing native-fixture lock, including SkinPN's trampoline.
        let _lock = crate::body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
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
        unsafe extern "C" fn pass(out: usize, a: u8, b: u8, dt: f32, c: u8) -> usize {
            let out = unsafe { std::slice::from_raw_parts_mut(out as *mut usize, 9) };
            out[0] += 1;
            out[1] = a.into();
            out[2] = b.into();
            out[3] = dt.to_bits() as usize;
            out[4] = c.into();
            0x2345
        }
        unsafe extern "C" fn contact(
            a: usize,
            b: usize,
            c: usize,
            d: usize,
            out: usize,
            dt: f32,
            g: usize,
            k: usize,
        ) -> usize {
            let out = unsafe { std::slice::from_raw_parts_mut(out as *mut usize, 9) };
            out[0] += 1;
            out[1..8].copy_from_slice(&[a, b, c, d, dt.to_bits() as usize, g, k]);
            0x6789
        }
        for (stage, prologue) in PROLOGUES.iter().enumerate().skip(7) {
            let mut code = prologue.to_vec();
            code.extend_from_slice(&[0x90; 16]);
            code.extend_from_slice(&[0x49, 0xBB]);
            let callback = if stage == 7 {
                pass as *const () as usize
            } else {
                contact as *const () as usize
            };
            code.extend_from_slice(&callback.to_le_bytes());
            code.extend_from_slice(&[0x41, 0xFF, 0xE3]);
            let allocation =
                unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
            assert!(!allocation.is_null());
            unsafe {
                std::ptr::copy_nonoverlapping(code.as_ptr(), allocation.cast::<u8>(), code.len());
            }
            let mut old = PAGE_PROTECTION_FLAGS::default();
            unsafe {
                VirtualProtect(allocation, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
                FlushInstructionCache(HANDLE(-1isize as *mut _), Some(allocation), 4096).unwrap();
            }
            let hook = unsafe {
                hook_closure_retn(
                    allocation as usize,
                    move |r, o| dispatch(stage, r, o),
                    CallbackOption::None,
                    HookFlags::empty(),
                )
            }
            .unwrap();
            let mut out = [0usize; 9];
            for calls in 1..=100 {
                if stage == 7 {
                    let f: CollisionPass = unsafe { std::mem::transmute(allocation) };
                    assert_eq!(
                        unsafe { f(out.as_mut_ptr() as usize, 0x81, 0x42, 0.03125, 0xC3) },
                        0x2345
                    );
                    assert_eq!(
                        &out[1..5],
                        &[0x81, 0x42, 0.03125f32.to_bits() as usize, 0xC3]
                    );
                } else {
                    let f: ContactApply = unsafe { std::mem::transmute(allocation) };
                    assert_eq!(
                        unsafe {
                            f(
                                0x11,
                                0x22,
                                0x33,
                                0x44,
                                out.as_mut_ptr() as usize,
                                0.015625,
                                0x77,
                                0x88,
                            )
                        },
                        0x6789
                    );
                    assert_eq!(
                        &out[1..8],
                        &[
                            0x11,
                            0x22,
                            0x33,
                            0x44,
                            0.015625f32.to_bits() as usize,
                            0x77,
                            0x88
                        ]
                    );
                }
                assert_eq!(out[0], calls);
            }
            drop(hook);
            unsafe {
                VirtualFree(allocation, 0, MEM_RELEASE).unwrap();
            }
        }
    }
    #[test]
    fn actual_runtime_compatibility_rejects_each_changed_entry_and_vtable() {
        let mut memory = vec![0usize; 0x3300000 / 8];
        let base = memory.as_mut_ptr() as usize;
        for (stage, rva) in RVAS.iter().copied().enumerate() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    PROLOGUES[stage].as_ptr(),
                    (base + rva) as *mut u8,
                    PROLOGUES[stage].len(),
                );
            }
            if VTABLES[stage] != 0 {
                memory[(VTABLES[stage] + 0x30) / 8] = base + rva;
            }
        }
        for &(rva, bytes) in CONTACT_SEAMS.iter().chain(DISPATCH_SITES.iter()) {
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), (base + rva) as *mut u8, bytes.len());
            }
        }
        assert!(crate::memory_query::scoped(|| compatible(base)));
        for (stage, rva) in RVAS.iter().copied().enumerate() {
            unsafe {
                *((base + rva) as *mut u8) ^= 1;
            }
            assert!(
                !crate::memory_query::scoped(|| compatible(base)),
                "stage {stage}"
            );
            unsafe {
                *((base + rva) as *mut u8) ^= 1;
            }
        }
        memory[(VTABLES[1] + 0x30) / 8] ^= 8;
        assert!(!crate::memory_query::scoped(|| compatible(base)));
        memory[(VTABLES[1] + 0x30) / 8] ^= 8;
        for &(rva, _) in CONTACT_SEAMS.iter().chain(DISPATCH_SITES.iter()) {
            unsafe {
                *((base + rva) as *mut u8) ^= 1;
            }
            assert!(!crate::memory_query::scoped(|| compatible(base)));
            unsafe {
                *((base + rva) as *mut u8) ^= 1;
            }
        }
    }
    #[test]
    fn concurrent_reservations_remain_bounded_and_stop_is_not_rearmed() {
        let s = std::sync::Arc::new(Session::new());
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let s = s.clone();
                scope.spawn(move || {
                    for frame in 1..80 {
                        for bucket in 0..4 {
                            for stage in 0..STAGES {
                                s.reserve(bucket, stage, frame);
                            }
                        }
                    }
                });
            }
        });
        assert!(s.next.load(Ordering::Acquire) <= RECORDS);
        assert!(
            s.counts
                .iter()
                .all(|n| n.load(Ordering::Acquire) <= PER_STAGE)
        );
        s.phase.store(2, Ordering::Release);
        s.stop(2);
        s.stop(1);
        assert_eq!(s.reason.load(Ordering::Acquire), 2);
        assert!(
            s.phase
                .compare_exchange(1, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        );
    }
    #[test]
    fn production_jsonl_fixture_export() {
        let mut sink = Vec::new();
        writeln!(sink,"{{\"type\":\"header\",\"schema\":\"er-cloth-sync-1\",\"model\":\"BD_M_9004\",\"synthetic_test\":true}}").unwrap();
        let mut id = 0;
        for n in COUNTS {
            for stage in [0, 1] {
                let mut r = Record::new();
                r.id = id;
                r.stage = stage;
                r.frame = id as u64 / 2 + 1;
                r.identity = Identity {
                    child: 0x1000 + n,
                    count: n,
                    scale_bits: 0.5f32.to_bits(),
                    ..Identity::default()
                };
                r.valid = true;
                r.error = "none";
                let raw = vec![[0., 1., 2., f32::from_bits(0xFFFFFFFF)]; n];
                for name in [
                    "current_before",
                    "current_after",
                    "previous_before",
                    "particle_data",
                ] {
                    r.copy(name, 0, raw.as_ptr() as usize, n * 16).unwrap();
                }
                if stage == 0 {
                    let matrix = [
                        1f32, 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
                    ];
                    let mut collider = [0u8; 0x90];
                    let bytes =
                        unsafe { std::slice::from_raw_parts(matrix.as_ptr() as *const u8, 64) };
                    collider[0x20..0x60].copy_from_slice(bytes);
                    let index = 0u32;
                    r.copy("bone_matrix", 0, matrix.as_ptr() as usize, 64)
                        .unwrap();
                    r.copy("collider_offsets", 0, matrix.as_ptr() as usize, 64)
                        .unwrap();
                    r.copy("collider_indices", 0, &index as *const u32 as usize, 4)
                        .unwrap();
                    for name in ["collider_before", "collider_after"] {
                        r.copy(name, 0, collider.as_ptr() as usize, collider.len())
                            .unwrap();
                    }
                } else {
                    let header = [0u8; 0x48];
                    let rows = [0u8; 12];
                    r.copy(
                        "constraint_header",
                        0,
                        header.as_ptr() as usize,
                        header.len(),
                    )
                    .unwrap();
                    r.copy("constraint_rows", 0, rows.as_ptr() as usize, rows.len())
                        .unwrap();
                }
                write_record(&mut sink, &r).unwrap();
                id += 1;
            }
        }
        writeln!(sink,"{{\"type\":\"footer\",\"written\":8,\"valid\":8,\"attempted\":8,\"reason\":1,\"human_cloth_acceptance\":false}}").unwrap();
        if let Ok(path) = std::env::var("ERPS_SYNC_FIXTURE_PATH") {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap()
                .write_all(&sink)
                .unwrap();
        }
        assert!(sink.len() < 1024 * 1024);
    }
    #[test]
    fn paired_native_boundary_all_stages_and_replaced_buffer_rejection() {
        let mut memory = vec![0usize; 0x10000 / 8];
        let h = memory.as_mut_ptr() as usize;
        let b = 0x140000000usize;
        let put = |m: &mut Vec<usize>, at: usize, v: usize| m[at / 8] = v;
        for (at, offset) in [
            (0x118, 0x800),
            (0x368, 0x1000),
            (0x120, 0x2000),
            (0x130, 0x2100),
            (0x840, 0x2200),
            (0x888, 0x3200),
            (0x3200, 0x3000),
            (0x3028, 0x3100),
            (0x268, 0x4000),
            (0x4000, 0x4100),
            (0x4188, 0x4200),
            (0x1030, 0x5000),
            (0x5000, 0x5100),
            (0x5118, 0x5200),
            (0x8B0, 0x5400),
            (0x8C0, 0x5500),
            (0x1020, 0x6000),
            (0x6000, 0x6100),
            (0x6008, 0x6300),
            (0x6318, 0x6600),
            (0x6340, 0x6700),
        ] {
            put(&mut memory, at, h + offset);
        }
        for (at, v) in [
            (0x100, b + 0x2D872A0),
            (0x800, b + 0x2D8B6F8),
            (0x4100, b + 0x2D8B758),
            (0x4200, b + 0x2D7DBC8),
            (0x848, 2),
            (0x890, 1),
            (0x270, 1),
            (0x3030, 1),
            (0x1038, 1),
            (0x5120, 2),
            (0x8B8, 1),
            (0x8C8, 1),
            (0x1028, 2),
            (0x6220, 0),
            (0x6210, 1),
            (0x6320, 2usize | (16usize << 32)),
        ] {
            put(&mut memory, at, v);
        }
        unsafe {
            ((h + 0x634C) as *mut u8).write(16);
        }
        let i = Identity {
            base: b,
            child: h + 0x100,
            sim: h + 0x800,
            root: h + 0x1000,
            count: 2,
            scale_bits: 0.5f32.to_bits(),
            ..Identity::default()
        };
        let mut regs: Registers = unsafe { std::mem::zeroed() };
        regs.rcx = (h + 0x3000) as u64;
        regs.rdx = i.child as u64;
        regs.xmm2 = 0.375f32.to_bits() as u128;
        regs.r9 = 0x81;
        unsafe extern "C" fn native(_: usize, child: usize, strength: f32, flags: usize) {
            assert_eq!(strength, 0.375);
            assert_eq!(flags, 0x81);
            unsafe {
                let p = *((child + 0x20) as *const usize) as *mut f32;
                *p += 0.25;
            }
        }
        unsafe extern "C" fn native_map(child: usize) {
            unsafe {
                let list = *((child + 0x168) as *const usize);
                let collider = *(list as *const usize);
                *((collider + 0x50) as *mut f32) = 3.;
            }
        }
        for (stage, vtable) in VTABLES.iter().copied().enumerate().take(7) {
            put(&mut memory, 0x3000, b + vtable);
            let source = memory.clone();
            let mut r = Record::new();
            r.identity = i;
            r.stage = stage;
            r.object = h + 0x3000;
            r.strength_bits = regs.xmm2 as u32;
            r.flags = regs.r9 as usize;
            crate::memory_query::scoped(|| capture_before(&mut r)).expect("before boundary");
            assert_eq!(memory, source, "recorder must never write engine memory");
            if stage == 0 {
                regs.rcx = i.child as u64;
                call_original(stage, &regs, native_map as *const () as usize);
            } else {
                regs.rcx = r.object as u64;
                call_original(stage, &regs, native as *const () as usize);
            }
            let native_result = memory.clone();
            crate::memory_query::scoped(|| capture_after(&mut r)).expect("after boundary");
            assert_eq!(
                memory, native_result,
                "after recorder must preserve native result"
            );
            let before = r
                .blocks
                .iter()
                .find(|x| x.name == "current_before")
                .unwrap();
            let after = r.blocks.iter().find(|x| x.name == "current_after").unwrap();
            let value =
                |x: &Block| f32::from_le_bytes(r.data[x.start..x.start + 4].try_into().unwrap());
            assert_eq!(
                value(after) - value(before),
                if stage == 0 { 0. } else { 0.25 }
            );
            put(&mut memory, 0x120, h + 0x2300);
            assert!(crate::memory_query::scoped(|| capture_after(&mut r)).is_none());
            put(&mut memory, 0x120, h + 0x2000);
            put(&mut memory, 0x4000, h + 0x4300);
            put(&mut memory, 0x4300, b + 0x2D8B758);
            assert!(crate::memory_query::scoped(|| capture_after(&mut r)).is_none());
            put(&mut memory, 0x4000, h + 0x4100);
        }
        // Rust's optimized helper detour was code-layout-sensitive. Use owned
        // executable fixtures with the actual seven native prologues instead;
        // no compiler-generated RIP literals / merged Rust function bodies.
        native_prologue_trampolines();
    }
    fn native_prologue_trampolines() {
        let _lock = crate::body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
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
        for (stage, prologue) in PROLOGUES.iter().enumerate().take(7) {
            let mut code = prologue.to_vec();
            code.extend_from_slice(&[0x90; 16]);
            if stage == 0 {
                // mov rax,[rcx+20]; inc qword ptr [rax]
                code.extend_from_slice(&[0x48, 0x8B, 0x41, 0x20, 0x48, 0xFF, 0]);
            } else {
                // inc [rcx]; mov rax,[rdx+20]; movss [rax],xmm2; mov [rax+8],r9
                code.extend_from_slice(&[
                    0x48, 0xFF, 1, 0x48, 0x8B, 0x42, 0x20, 0xF3, 0x0F, 0x11, 0x10, 0x4C, 0x89,
                    0x48, 8,
                ]);
            }
            let epilogue: &[u8] = match stage {
                0 => &[0x48, 0x81, 0xC4, 0x88, 2, 0, 0, 0xC3],
                1..=3 => &[
                    0x0F, 0x28, 0x74, 0x24, 0x40, 0x48, 0x83, 0xC4, 0x50, 0x5F, 0xC3,
                ],
                4 => &[
                    0x0F, 0x28, 0x74, 0x24, 0x40, 0x48, 0x83, 0xC4, 0x58, 0x41, 0x5E, 0x5B, 0xC3,
                ],
                5 => &[
                    0x0F, 0x28, 0x74, 0x24, 0x50, 0x48, 0x83, 0xC4, 0x60, 0x5B, 0xC3,
                ],
                _ => &[
                    0x48, 0x83, 0xC4, 0x30, 0x5E, 0x48, 0x8B, 0x5C, 0x24, 0x18, 0xC3,
                ],
            };
            code.extend_from_slice(epilogue);
            let allocation =
                unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
            assert!(!allocation.is_null());
            unsafe {
                std::ptr::copy_nonoverlapping(code.as_ptr(), allocation.cast::<u8>(), code.len());
            }
            let mut old = PAGE_PROTECTION_FLAGS::default();
            unsafe {
                VirtualProtect(allocation, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
                FlushInstructionCache(HANDLE(-1isize as *mut _), Some(allocation), 4096).unwrap();
            }
            let hook = unsafe {
                hook_closure_retn(
                    allocation as usize,
                    move |r, o| dispatch(stage, r, o),
                    CallbackOption::None,
                    HookFlags::empty(),
                )
            }
            .unwrap();
            let mut output = [0u64; 2];
            let mut child = [0usize; 0x280 / 8];
            child[4] = output.as_mut_ptr() as usize;
            let mut calls = 0usize;
            for expected in 1..=10 {
                unsafe {
                    if stage == 0 {
                        std::mem::transmute::<usize, Map>(allocation as usize)(
                            child.as_ptr() as usize
                        );
                    } else {
                        std::mem::transmute::<usize, Apply>(allocation as usize)(
                            &mut calls as *mut usize as usize,
                            child.as_ptr() as usize,
                            0.375,
                            0x81,
                        );
                    }
                }
                if stage == 0 {
                    assert_eq!(output[0], expected);
                } else {
                    assert_eq!(calls, expected as usize);
                    assert_eq!(output[0] as u32, 0.375f32.to_bits());
                    assert_eq!(output[1], 0x81);
                }
            }
            drop(hook);
            unsafe {
                VirtualFree(allocation, 0, MEM_RELEASE).unwrap();
            }
        }
    }
    #[test]
    fn allocation_and_frame_stage_caps() {
        assert!(RECORDS * (BYTES + BLOCKS * size_of::<Block>()) < 16 * 1024 * 1024);
        let s = Session::new();
        assert!(s.reserve(0, 0, 1).is_some());
        assert!(s.reserve(0, 0, 1).is_none());
        assert!(s.reserve(1, 0, 1).is_none());
        assert!(s.reserve(2, 0, 1).is_none());
        for frame in 2..=PER_STAGE as u64 {
            assert!(s.reserve(0, 0, frame).is_some());
        }
        assert!(s.reserve(0, 0, PER_STAGE as u64 + 1).is_none());
        for bucket in 0..4 {
            for stage in 0..STAGES {
                for f in 10..=15 {
                    s.reserve(bucket, stage, 100 * (bucket * STAGES + stage) as u64 + f);
                }
            }
        }
        assert!(s.next.load(Ordering::Relaxed) <= RECORDS);
    }
    #[test]
    fn bounded_copy_keeps_nonfinite_w_bits_without_writing_source() {
        let data = [
            1.0f32.to_bits(),
            2.0f32.to_bits(),
            3.0f32.to_bits(),
            0xFFFF_FFFF,
        ];
        let mut r = Record::new();
        let cap = r.data.capacity();
        crate::memory_query::scoped(|| {
            assert!(r.copy("w_bits", 0, data.as_ptr() as usize, 16).is_some());
            assert!(r.copy("bad", 0, 0, 16).is_none());
            assert!(
                r.copy("too_large", 0, data.as_ptr() as usize, BYTES)
                    .is_none()
            );
        });
        assert_eq!(r.data.capacity(), cap);
        assert_eq!(&r.data[12..], &[255; 4]);
        assert_eq!(data[0], 1.0f32.to_bits());
    }
    #[test]
    fn native_abi_original_exactly_once_float_and_flags() {
        unsafe extern "C" fn native(object: usize, child: usize, strength: f32, flags: usize) {
            assert_eq!(object, 0x1234);
            assert_eq!(strength.to_bits(), 0.375f32.to_bits());
            assert_eq!(flags, 0x81);
            unsafe {
                *(child as *mut u32) += 1;
            }
        }
        let mut value = 7u32;
        let mut regs: Registers = unsafe { std::mem::zeroed() };
        regs.rcx = 0x1234;
        regs.rdx = &mut value as *mut u32 as u64;
        regs.xmm2 = 0.375f32.to_bits() as u128;
        regs.r9 = 0x81;
        dispatch(4, &mut regs, native as *const () as usize);
        assert_eq!(value, 8);
    }
    #[test]
    fn jsonl_preserves_raw_bytes_and_io_errors() {
        let mut r = Record::new();
        r.error = "none";
        let data = [0xFFu8, 0, 0x81, 0x7F];
        r.copy("raw", 0, data.as_ptr() as usize, 4).unwrap();
        let mut output = Vec::new();
        write_record(&mut output, &r).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("ff00817f"));
        struct Fail;
        impl Write for Fail {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("test"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(write_record(&mut Fail, &r).is_err());
    }
}
