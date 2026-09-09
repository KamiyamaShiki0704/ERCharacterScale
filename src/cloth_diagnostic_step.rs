//! Ordered extension of the existing owned-copy recorder. No engine writes.
//! Three bounded Simulate invocations; native execution never holds a borrow,
//! memory-query scope, or recorder mutex. The original primary sampler is off.
use super::*;

pub(super) const ENABLED: bool = true;
const STEPS: usize = 3;
const EVENTS: usize = 20;
const START_MS: u64 = 1200;
const SIMULATE: usize = 0x15E7FC0;
const INTEGRATE: usize = 0x15EA330;
// Both sites are outside the action loop. RDI=child, RBP=force array;
// at AFTER, R13=effective gravity, XMM13=dt², XMM14=effective damping.
const FORCE_BEFORE: usize = 0x15EA44F;
const FORCE_AFTER: usize = 0x15EA4DD;
const SEAMS: &[(usize, &[u8])] = &[
    (
        SIMULATE,
        &[
            0x48, 0x8B, 0xC4, 0x4C, 0x89, 0x40, 0x18, 0x48, 0x89, 0x50, 0x10, 0x48, 0x89, 0x48,
            0x08,
        ],
    ),
    (
        INTEGRATE,
        &[
            0x48, 0x8B, 0xC4, 0x55, 0x41, 0x54, 0x48, 0x81, 0xEC, 0xC8, 0, 0, 0, 0x8B, 0x0D, 0x59,
            0xA2, 0x1F, 0x03,
        ],
    ),
    (
        FORCE_BEFORE,
        &[
            0x8B, 0x0D, 0x47, 0xA1, 0x1F, 0x03, 0xFF, 0x15, 0x31, 0x71, 0x62, 0x03, 0x48, 0x85,
            0xC0,
        ],
    ),
    (
        FORCE_AFTER,
        &[
            0x49, 0x8B, 0x4F, 0x40, 0x41, 0x8B, 0xC6, 0x4C, 0x8B, 0x47, 0x20, 0x99, 0x4C, 0x8B,
            0x4F, 0x30,
        ],
    ),
];

struct Event {
    record: Record,
    integration: bool,
    substep: usize,
    force_before: bool,
    force_after: bool,
    force_address: usize,
    coefficients: [u32; 2],
    force_timestamps: [u64; 2],
    force_copy_us: [u64; 2],
    force_read_calls: [u64; 2],
}
impl Event {
    fn new() -> Self {
        Self {
            record: Record::new(),
            integration: false,
            substep: 0,
            force_before: false,
            force_after: false,
            force_address: 0,
            coefficients: [0; 2],
            force_timestamps: [0; 2],
            force_copy_us: [0; 2],
            force_read_calls: [0; 2],
        }
    }
}
struct Step {
    collider_velocities: Vec<crate::cloth_collider_rotation_hook::Witness>,
    collider_velocity_error: &'static str,
    mesh: MeshBoundary,
    outer: Record,
    events: Vec<Event>,
    len: usize,
    integrations: usize,
    open_integration: Option<usize>,
    identity: Identity,
    frame: u64,
    copy_us: u64,
    error: &'static str,
    complete: bool,
    origin: Instant,
}
impl Step {
    fn new() -> Self {
        Self {
            collider_velocities: Vec::with_capacity(64),
            collider_velocity_error: "none",
            mesh: MeshBoundary::new(),
            outer: Record::new(),
            events: (0..EVENTS).map(|_| Event::new()).collect(),
            len: 0,
            integrations: 0,
            open_integration: None,
            identity: Identity::default(),
            frame: 0,
            copy_us: 0,
            error: "not_reached",
            complete: false,
            origin: Instant::now(),
        }
    }
    fn timestamp(&self) -> u64 {
        self.origin.elapsed().as_micros() as u64 + 1
    }
    fn failed(&self) -> bool {
        !matches!(self.error, "none" | "not_reached" | "awaiting_simulate")
    }
    fn sequence_complete(&self) -> bool {
        let order = [0, 4, 6, 1, 5, 3, 7];
        self.len == order.len() * 2
            && self.events[..self.len]
                .iter()
                .enumerate()
                .all(|(index, e)| {
                    e.record.stage == order[index % order.len()]
                        && e.integration == (index % order.len() == 0)
                        && e.substep == index / order.len()
                        && e.record.valid
                })
    }
    fn fail(&mut self, error: &'static str) {
        if !self.failed() {
            self.error = error;
        }
    }
    fn charge(&mut self, timer: Instant) {
        // Cost is evidence. Never discard the rest of a valid native step for
        // crossing the timing target; finite storage/count guards remain hard.
        self.copy_us = self
            .copy_us
            .saturating_add(timer.elapsed().as_micros() as u64);
    }
    fn begin_event(
        &mut self,
        stage: usize,
        child: usize,
        object: usize,
        bits: u32,
        flags: usize,
        integration: bool,
    ) -> Option<usize> {
        if self.failed() || child != self.identity.child {
            return None;
        }
        if self.len == EVENTS {
            self.fail("ordered_event_cap");
            return None;
        }
        if self.open_integration.is_some() || (!integration && self.integrations == 0) {
            self.fail("unexpected_nested_or_pre_integration_event");
            return None;
        }
        let id = self.len;
        self.len += 1;
        if integration {
            self.integrations += 1;
            self.open_integration = Some(id);
        }
        let timer = Instant::now();
        let event = &mut self.events[id];
        event.integration = integration;
        event.substep = self.integrations - 1;
        let r = &mut event.record;
        r.id = id;
        r.stage = stage;
        r.frame = self.frame;
        r.identity = self.identity;
        r.object = object;
        r.strength_bits = bits;
        r.flags = flags;
        r.error = "awaiting_after";
        let queries = crate::memory_query::query_count();
        let reads = copy_read_count();
        let ok = copy_read_scope(|| event_before(r, integration)).is_some();
        r.before_queries = crate::memory_query::query_count() - queries;
        r.before_copy_reads = copy_read_count() - reads;
        r.before_us = timer.elapsed().as_micros() as u64;
        if !ok {
            r.error = "ordered_before_guard";
            self.fail("ordered_before_guard");
        }
        self.charge(timer);
        self.events[id].record.native_enter_us = self.timestamp();
        Some(id)
    }
    fn end_event(&mut self, id: usize) {
        let now = self.timestamp();
        let integration = self.events[id].integration;
        self.events[id].record.native_exit_us = now;
        if integration {
            self.open_integration = None;
        }
        if self.failed() {
            return;
        }
        if integration && !(self.events[id].force_before && self.events[id].force_after) {
            self.events[id].record.error = "missing_force_boundary";
            self.fail("missing_force_boundary");
            return;
        }
        let timer = Instant::now();
        let r = &mut self.events[id].record;
        let queries = crate::memory_query::query_count();
        let reads = copy_read_count();
        let ok = copy_read_scope(|| event_after(r, integration)).is_some();
        r.after_queries = crate::memory_query::query_count() - queries;
        r.after_copy_reads = copy_read_count() - reads;
        r.after_us = timer.elapsed().as_micros() as u64;
        r.valid = ok;
        r.error = if ok { "none" } else { "ordered_after_guard" };
        if ok && r.stage == 4 && f32::from_bits(r.strength_bits) > 0.0 {
            self.mesh.paired = true;
            self.mesh.consumer_id = Some(id);
        }
        if !ok {
            self.fail("ordered_after_guard");
        }
        self.charge(timer);
    }
    fn force(&mut self, after: bool, child: usize, address: usize, gravity: usize) {
        if self.failed() {
            return;
        }
        let Some(id) = self.open_integration else {
            self.fail("force_without_integration");
            return;
        };
        if child != self.identity.child {
            self.fail("force_child_changed");
            return;
        }
        let e = &self.events[id];
        if (!after && e.force_before)
            || (after && (!e.force_before || e.force_after || address != e.force_address))
        {
            self.fail("duplicate_or_reordered_force_boundary");
            return;
        }
        let timer = Instant::now();
        let timestamp = self.timestamp();
        let e = &mut self.events[id];
        let reads = copy_read_count();
        let result = copy_read_scope(|| {
            e.record.copy(
                if after { "force_after" } else { "force_before" },
                0,
                address,
                self.identity.count * 16,
            )?;
            if after {
                e.record.copy("effective_gravity", 0, gravity, 16)?;
                e.record.copy(
                    "arithmetic_current",
                    0,
                    e.record.layout.current,
                    self.identity.count * 16,
                )?;
                e.record.copy(
                    "arithmetic_previous",
                    0,
                    e.record.layout.previous,
                    self.identity.count * 16,
                )?;
                e.record.copy("damping_at_force", 0, child + 0x54, 4)?;
                let dt = f32::from_bits(e.record.strength_bits);
                e.coefficients = [
                    (dt * dt).to_bits(),
                    u32::from_le_bytes(e.record.data[e.record.data.len() - 4..].try_into().ok()?),
                ];
            }
            Some(())
        });
        e.force_copy_us[usize::from(after)] = timer.elapsed().as_micros() as u64;
        e.force_read_calls[usize::from(after)] = copy_read_count() - reads;
        if result.is_none() {
            self.fail("force_copy_guard");
        } else if after {
            e.force_after = true;
            e.force_timestamps[1] = timestamp;
        } else {
            e.force_before = true;
            e.force_address = address;
            e.force_timestamps[0] = timestamp;
        }
        self.charge(timer);
    }
}

pub(super) struct Capture {
    slots: Vec<Mutex<Option<Box<Step>>>>,
    next: AtomicUsize,
    last_frame: AtomicU64,
    pending: AtomicBool,
}
impl Capture {
    pub(super) fn new() -> Self {
        Self {
            slots: (0..STEPS)
                .map(|_| Mutex::new(Some(Box::new(Step::new()))))
                .collect(),
            next: AtomicUsize::new(0),
            last_frame: AtomicU64::new(0),
            pending: AtomicBool::new(false),
        }
    }
    pub(super) fn pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
}
thread_local! { static ACTIVE: RefCell<Option<Box<Step>>> = const { RefCell::new(None) }; }

pub(super) fn collider_velocity_begin(
    child: usize,
    witness: crate::cloth_collider_rotation_hook::Witness,
) -> Option<usize> {
    ACTIVE.with(|a| {
        let mut lock = a.borrow_mut();
        let s = lock.as_mut().filter(|s| s.identity.child == child)?;
        let timer = Instant::now();
        if s.collider_velocities.len() == 64 {
            // Witness overflow is explicit and never truncates the solver step.
            s.collider_velocity_error = "capacity";
            return None;
        }
        let token = s.collider_velocities.len();
        s.collider_velocities.push(witness);
        s.charge(timer);
        Some(token)
    })
}
pub(super) fn collider_velocity_end(child: usize, token: usize, output: Option<[u32; 8]>) {
    ACTIVE.with(|a| {
        let mut lock = a.borrow_mut();
        if let Some(s) = lock.as_mut().filter(|s| s.identity.child == child) {
            if let Some(w) = s.collider_velocities.get_mut(token) {
                w.output = output;
                if output.is_none() {
                    s.collider_velocity_error = "output_unreadable";
                }
            } else {
                s.collider_velocity_error = "token";
            }
        }
    });
}

fn particle_layout(i: Identity) -> Option<Layout> {
    if ptr(i.child + 0x18)? != i.sim || count(i.child + 0x28, 512)? != i.count {
        return None;
    }
    Some(Layout {
        current: ptr(i.child + 0x20)?,
        previous: ptr(i.child + 0x30)?,
        particles: ptr(i.sim + 0x40)?,
        consumer_colliders: ptr(i.child + 0x190)?,
        consumer_count: count(i.child + 0x188, 32)?,
        ..Layout::default()
    })
}
fn positions(r: &mut Record, after: bool) -> Option<()> {
    let l = particle_layout(r.identity)?;
    if after
        && (l.current != r.layout.current
            || l.previous != r.layout.previous
            || l.particles != r.layout.particles
            || l.consumer_colliders != r.layout.consumer_colliders
            || l.consumer_count != r.layout.consumer_count)
    {
        return None;
    }
    if !after {
        r.layout = l;
    }
    r.copy(
        if after {
            "current_after"
        } else {
            "current_before"
        },
        0,
        l.current,
        r.identity.count * 16,
    )?;
    r.copy(
        if after {
            "previous_after"
        } else {
            "previous_before"
        },
        0,
        l.previous,
        r.identity.count * 16,
    )
}
fn actions(r: &mut Record) -> Option<()> {
    let child = r.identity.child;
    let n = count(child + 0xF0, 4)?;
    let array = ptr(child + 0xE8)?;
    r.copy("runtime_action_slots", 0, array, n * 8)?;
    for i in 0..n {
        let action = ptr(array + i * 8)?;
        if action == 0 {
            continue;
        } // The original action loop skips null entries.
        let vt = ptr(action)?;
        r.copy("runtime_action_header", i, action, 16)?;
        if vt == r.identity.base.checked_add(0x329E418)? {
            r.copy("game_action_wrapper", i, action, 0x20)?;
            let delegate = ptr(action + 0x18)?;
            let delegate_vt = ptr(delegate)?;
            r.copy("game_action_delegate_header", i, delegate, 16)?;
            if delegate_vt == r.identity.base.checked_add(0x329E448)? {
                r.copy("game_wind_parameters", i, delegate, 0x98)?;
            }
        } else if vt == r.identity.base.checked_add(0x2D89CE0)? {
            r.copy("runtime_simple_wind", i, action, 0x50)?;
        }
    }
    (ptr(child + 0xE8)? == array && count(child + 0xF0, 4)? == n).then_some(())?;
    r.copy("runtime_actions_complete", 0, 0, 0)
}
fn event_before(r: &mut Record, integration: bool) -> Option<()> {
    positions(r, false)?;
    r.copy(
        "particle_data",
        0,
        r.layout.particles,
        r.identity.count * 16,
    )?;
    r.copy("simulation_header", 0, r.identity.sim, 0x1C0)?;
    if integration {
        r.copy("dynamics_child_header", 0, r.identity.child, 0x270)?;
        let n = count(r.identity.child + 0x48, 512)?;
        if n == r.identity.count {
            r.copy(
                "simulation_normals_at_action",
                0,
                ptr(r.identity.child + 0x40)?,
                n * 16,
            )?;
        }
        actions(r)?;
    } else if (1..=6).contains(&r.stage) {
        constraint_member(r.identity, r.stage, r.object)?;
        r.copy(
            "constraint_header",
            0,
            r.object,
            [0, 0x38, 0x38, 0x38, 0x48, 0x40, 0x50][r.stage],
        )?;
        let n = count(r.object + 0x30, 1280)?;
        r.copy(
            "constraint_rows",
            0,
            ptr(r.object + 0x28)?,
            n * [0, 12, 16, 12, 16, 32, 16][r.stage],
        )?;
        reference(r)?;
        transition_state(r, false)?;
    } else if r.stage == 7 {
        consumer_colliders(r, false)?;
    } else {
        return None;
    }
    Some(())
}
fn event_after(r: &mut Record, integration: bool) -> Option<()> {
    positions(r, true)?;
    if !integration {
        if r.stage == 7 {
            consumer_colliders(r, true)?;
        } else {
            transition_state(r, true)?;
        }
    }
    Some(())
}
fn outer_before(r: &mut Record, info: usize) -> Option<()> {
    positions(r, false)?;
    r.copy("simulate_operator_header", 0, r.object, 0x60)?;
    r.copy("simulate_step_info", 0, info, 0x30)?;
    r.copy("dynamics_child_header", 0, r.identity.child, 0x270)?;
    r.copy("simulation_header", 0, r.identity.sim, 0x1C0)?;
    let n = count(r.identity.sim + 0x58, 512)?;
    r.copy("fixed_particles", 0, ptr(r.identity.sim + 0x50)?, n * 2)?;
    let n = count(r.object + 0x58, 8)?;
    r.copy("native_step_configs", 0, ptr(r.object + 0x50)?, n * 0x30)
}
fn eligible(s: &Session) -> bool {
    s.phase.load(Ordering::Acquire) == 2
        && s.started.get().is_some_and(|t| {
            let elapsed = t.elapsed();
            elapsed >= Duration::from_millis(START_MS) && (elapsed < WINDOW || s.ordered.pending())
        })
        && s.ordered.next.load(Ordering::Acquire) < STEPS
}

/// The time window closes admission between steps. A live native invocation
/// owns its slot until publication; elapsed time never invalidates that step.
pub(super) fn poll_end(s: &Session) {
    if s.phase.load(Ordering::Acquire) != 2 {
        return;
    }
    let Some(slot) = s.ordered.slots.get(s.ordered.next.load(Ordering::Acquire)) else {
        return;
    };
    let Ok(mut lock) = slot.try_lock() else {
        return;
    };
    if s.in_flight.load(Ordering::Acquire) != 0 {
        return;
    }
    if s.ordered.pending() {
        if let Some(step) = lock.as_mut()
            && step.frame != s.frame.load(Ordering::Acquire)
        {
            step.fail("ordered_mesh_not_consumed_in_frame");
            s.ordered.pending.store(false, Ordering::Release);
            s.stop(6);
        }
    } else if s.started.get().is_some_and(|t| t.elapsed() >= WINDOW) {
        s.stop(1);
    }
}

pub(super) fn mesh_begin(s: &Session, source: MeshBoundarySource) {
    if !eligible(s) {
        return;
    }
    let Some(identity) = s
        .targets
        .get()
        .and_then(|t| {
            t.iter().find(|i| {
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
    let frame = s.frame.load(Ordering::Acquire);
    if frame <= s.ordered.last_frame.load(Ordering::Acquire) {
        return;
    }
    let index = s.ordered.next.load(Ordering::Acquire);
    let Some(slot) = s.ordered.slots.get(index) else {
        return;
    };
    let Ok(mut lock) = slot.try_lock() else {
        return;
    };
    // Serialize admission with poll_end, including a writer reaching the
    // window boundary after the first eligibility check.
    if !eligible(s) {
        return;
    }
    let Some(step) = lock.as_mut().filter(|st| st.mesh.source.is_none()) else {
        return;
    };
    let timer = Instant::now();
    step.identity = identity;
    step.frame = frame;
    step.error = "awaiting_simulate";
    step.origin = *s.started.get().unwrap();
    s.ordered.pending.store(true, Ordering::Release);
    if step.mesh.start(source, identity, frame).is_none() {
        step.fail("ordered_mesh_before_guard");
    }
    step.mesh.record.native_enter_us = step.timestamp();
    step.charge(timer);
    step.mesh.copy_us = step.copy_us;
    if step.failed() {
        s.stop(2);
    }
}
pub(super) fn mesh_end(s: &Session, op: usize, output: usize, normals: usize) {
    if !eligible(s) {
        return;
    }
    let index = s.ordered.next.load(Ordering::Acquire);
    let Some(slot) = s.ordered.slots.get(index) else {
        return;
    };
    let Ok(mut lock) = slot.try_lock() else {
        return;
    };
    let Some(step) = lock.as_mut() else {
        return;
    };
    if step.mesh.source.is_none() || step.mesh.complete || step.failed() {
        return;
    }
    if step
        .mesh
        .source
        .is_none_or(|src| src.op != op || src.output_buffer != output)
    {
        return;
    }
    let timer = Instant::now();
    if step
        .mesh
        .finish(op, output, normals, s.frame.load(Ordering::Acquire))
        .is_none()
    {
        step.fail("ordered_mesh_after_guard");
    }
    step.mesh.record.native_exit_us = step.timestamp();
    step.charge(timer);
    step.mesh.copy_us = step.copy_us;
    if step.failed() {
        s.stop(2);
    }
}
fn matching_outer(identity: Identity, op: usize, info: usize) -> bool {
    (|| {
        if ptr(info.checked_add(0x10)?)? != identity.root {
            return None;
        }
        let n = count(identity.root + 0x48, 16)?;
        let index = count(op.checked_add(0x48)?, 15)?;
        (index < n && ptr(ptr(identity.root + 0x40)? + index * 8)? == identity.child).then_some(())
    })()
    .is_some()
}
type Simulate = unsafe extern "C" fn(usize, usize, usize) -> usize;
type Integrate = unsafe extern "C" fn(usize, usize, f32);
pub(super) fn original_simulate(r: &Registers, original: usize) -> usize {
    unsafe {
        std::mem::transmute::<usize, Simulate>(original)(
            r.rcx as usize,
            r.rdx as usize,
            r.r8 as usize,
        )
    }
}
fn outer_dispatch(reg: *mut Registers, original: usize) -> usize {
    let r = unsafe { &*reg };
    let Some(s) = SESSION.get().filter(|s| eligible(s)) else {
        return original_simulate(r, original);
    };
    s.in_flight.fetch_add(1, Ordering::AcqRel);
    let flight = Flight(&s.in_flight);
    // Quiescence guard precedes every read or slot reservation. If stop won
    // the race, this invocation only forwards its original arguments.
    if !eligible(s) {
        return original_simulate(r, original);
    }
    let index = s.ordered.next.load(Ordering::Acquire);
    let Some(slot) = s.ordered.slots.get(index) else {
        return original_simulate(r, original);
    };
    let active = ACTIVE.with(|a| a.borrow().is_some());
    if active {
        return original_simulate(r, original);
    }
    let step = {
        let Ok(mut lock) = slot.try_lock() else {
            return original_simulate(r, original);
        };
        let Some(step) = lock.as_ref() else {
            drop(lock);
            return original_simulate(r, original);
        };
        if !step.mesh.complete || step.failed() {
            drop(lock);
            return original_simulate(r, original);
        }
        let timer = Instant::now();
        let matched =
            copy_read_scope(|| matching_outer(step.identity, r.rcx as usize, r.r8 as usize));
        if !matched {
            drop(lock);
            return original_simulate(r, original);
        }
        let mut step = lock.take().unwrap();
        step.charge(timer);
        step
    };
    let (step, result) = capture_invocation(s, step, r, original, index, identity_current);
    publish_step(s, slot, index, step);
    drop(flight);
    result
}
fn publish_step(s: &Session, slot: &Mutex<Option<Box<Step>>>, index: usize, step: Box<Step>) {
    let frame = step.frame;
    let stop = step.failed();
    // The native call is over; publishing owned data is the only blocking lock.
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(step);
    s.ordered.last_frame.store(frame, Ordering::Release);
    s.ordered.next.store(index + 1, Ordering::Release);
    s.ordered.pending.store(false, Ordering::Release);
    if stop {
        s.stop(2);
    } else if index + 1 == STEPS {
        s.stop(4);
    }
}
fn capture_invocation(
    s: &Session,
    step: Box<Step>,
    r: &Registers,
    original: usize,
    index: usize,
    mut check_identity: impl FnMut(Identity) -> bool,
) -> (Box<Step>, usize) {
    let mut step = step;
    let timer = Instant::now();
    if s.phase.load(Ordering::Acquire) != 2 || step.frame != s.frame.load(Ordering::Acquire) {
        step.fail("ordered_frame_changed");
    }
    step.error = if step.failed() { step.error } else { "none" };
    let identity = step.identity;
    let frame = step.frame;
    step.outer.identity = identity;
    step.outer.frame = frame;
    step.outer.object = r.rcx as usize;
    let queries = crate::memory_query::query_count();
    let reads = copy_read_count();
    if !step.failed() {
        let ok = copy_read_scope(|| {
            let identity_timer = Instant::now();
            let current = check_identity(identity);
            step.outer.identity_before_us = identity_timer.elapsed().as_micros() as u64;
            current
                .then_some(())
                .and_then(|_| outer_before(&mut step.outer, r.r8 as usize))
        })
        .is_some();
        if !ok {
            step.fail("ordered_outer_before_guard");
        }
    }
    step.outer.before_queries = crate::memory_query::query_count() - queries;
    step.outer.before_copy_reads = copy_read_count() - reads;
    step.charge(timer);
    step.outer.before_us = timer.elapsed().as_micros() as u64;
    step.outer.native_enter_us = step.timestamp();
    ACTIVE.with(|a| *a.borrow_mut() = Some(step));
    let result = original_simulate(r, original);
    let mut step = ACTIVE.with(|a| a.borrow_mut().take()).unwrap();
    step.outer.native_exit_us = step.timestamp();
    let timer = Instant::now();
    let queries = crate::memory_query::query_count();
    let reads = copy_read_count();
    if !step.failed() {
        let ok = copy_read_scope(|| {
            let identity_timer = Instant::now();
            let current = check_identity(identity);
            step.outer.identity_after_us = identity_timer.elapsed().as_micros() as u64;
            current
                .then_some(())
                .and_then(|_| positions(&mut step.outer, true))
        })
        .is_some();
        if !ok || frame != s.frame.load(Ordering::Acquire) {
            step.fail("ordered_outer_after_guard");
        }
    }
    step.outer.after_queries = crate::memory_query::query_count() - queries;
    step.outer.after_copy_reads = copy_read_count() - reads;
    step.charge(timer);
    step.outer.after_us = timer.elapsed().as_micros() as u64;
    if step.integrations != 2 || step.open_integration.is_some() {
        step.fail("ordered_substep_count");
    }
    if !step.sequence_complete() {
        step.fail("ordered_event_sequence");
    }
    if index > 0 && frame != s.ordered.last_frame.load(Ordering::Acquire) + 1 {
        step.fail("ordered_nonconsecutive_frames");
    }
    step.complete = !step.failed();
    step.outer.valid = step.complete;
    step.outer.error = step.error;
    (step, result)
}
fn integration_dispatch(reg: *mut Registers, original: usize) -> usize {
    let r = unsafe { &*reg };
    let token = ACTIVE.with(|a| {
        a.borrow_mut()
            .as_mut()
            .and_then(|s| s.begin_event(0, r.rdx as usize, r.rcx as usize, r.xmm2 as u32, 0, true))
    });
    unsafe {
        std::mem::transmute::<usize, Integrate>(original)(
            r.rcx as usize,
            r.rdx as usize,
            f32::from_bits(r.xmm2 as u32),
        );
    }
    if let Some(id) = token {
        ACTIVE.with(|a| {
            if let Some(s) = a.borrow_mut().as_mut() {
                s.end_event(id);
            }
        });
    }
    0
}
fn force_observe(after: bool, reg: *mut Registers) {
    let r = unsafe { &*reg };
    ACTIVE.with(|a| {
        if let Some(s) = a.borrow_mut().as_mut() {
            s.force(after, r.rdi as usize, r.rbp as usize, r.r13 as usize);
        }
    });
}
pub(super) fn constraint_begin(stage: usize, r: &Registers) -> Option<usize> {
    if !(1..=7).contains(&stage) {
        return None;
    }
    let child = if stage == 7 { r.rcx } else { r.rdx } as usize;
    let bits = if stage == 7 { r.xmm3 } else { r.xmm2 } as u32;
    let flags = if stage == 7 {
        (r.rdx as u8 as usize)
            | ((r.r8 as u8 as usize) << 8)
            | ((unsafe { r.get_stack(5) } as u8 as usize) << 16)
    } else {
        r.r9 as usize
    };
    ACTIVE.with(|a| {
        a.borrow_mut()
            .as_mut()
            .and_then(|s| s.begin_event(stage, child, r.rcx as usize, bits, flags, false))
    })
}
pub(super) fn constraint_end(token: Option<usize>) {
    if let Some(id) = token {
        ACTIVE.with(|a| {
            if let Some(s) = a.borrow_mut().as_mut() {
                s.end_event(id);
            }
        });
    }
}
pub(super) fn compatible(base: usize) -> bool {
    SEAMS.iter().all(|&(rva, raw)| {
        crate::memory_query::accessible_region(base + rva, raw.len(), false).is_some()
            && unsafe { std::slice::from_raw_parts((base + rva) as *const u8, raw.len()) == raw }
    })
}
pub(super) fn install(base: usize) -> bool {
    for (rva, callback) in [
        (
            SIMULATE,
            outer_dispatch as fn(*mut Registers, usize) -> usize,
        ),
        (INTEGRATE, integration_dispatch),
    ] {
        let Ok(hook) = (unsafe {
            hook_closure_retn(
                base + rva,
                callback,
                CallbackOption::None,
                HookFlags::empty(),
            )
        }) else {
            return false;
        };
        let _ = Box::leak(Box::new(hook));
    }
    for (rva, after) in [(FORCE_BEFORE, false), (FORCE_AFTER, true)] {
        let Ok(hook) = (unsafe {
            hook_closure_jmp_back(
                base + rva,
                move |r| force_observe(after, r),
                CallbackOption::None,
                HookFlags::empty(),
            )
        }) else {
            return false;
        };
        let _ = Box::leak(Box::new(hook));
    }
    true
}
pub(super) fn write(sink: &mut impl Write, capture: &Capture) -> io::Result<()> {
    write!(
        sink,
        ",\"ordered_steps\":{{\"version\":2,\"budget_policy\":\"complete-step-measure-only\",\"start_ms\":{START_MS},\"max_steps\":{STEPS},\"max_events\":{EVENTS},\"copy_budget_us\":{COPY_BUDGET_US},\"steps\":["
    )?;
    let mut emitted = 0;
    for (index, slot) in capture.slots.iter().enumerate() {
        let lock = slot
            .lock()
            .map_err(|_| io::Error::other("ordered slot poisoned"))?;
        let Some(s) = lock.as_ref().filter(|s| s.mesh.source.is_some()) else {
            continue;
        };
        if emitted != 0 {
            sink.write_all(b",")?;
        }
        emitted += 1;
        write!(
            sink,
            "{{\"index\":{index},\"frame\":{},\"complete\":{},\"error\":\"{}\",\"copy_us\":{},\"copy_target_exceeded\":{},\"mesh_target_exceeded\":{},\"integrations\":{},\"mesh\":",
            s.frame,
            s.complete,
            s.error,
            s.copy_us,
            s.copy_us > COPY_BUDGET_US,
            s.mesh.copy_us > SKIN_BUDGET_US,
            s.integrations
        )?;
        write_mesh_boundary(sink, &s.mesh)?;
        sink.write_all(b",\"outer\":")?;
        write_record_named(sink, &s.outer, "simulate", SIMULATE)?;
        write!(
            sink,
            ",\"collider_velocity_error\":\"{}\",\"collider_velocities\":[",
            s.collider_velocity_error
        )?;
        for (i, w) in s.collider_velocities.iter().enumerate() {
            if i != 0 {
                sink.write_all(b",")?;
            }
            write!(
                sink,
                "{{\"collider\":{},\"dt_bits\":{},\"scale_bits\":{},\"target_bits\":{:?},\"previous_bits\":{:?},\"passed_target_bits\":{:?},\"passed_previous_bits\":{:?},\"applied\":{},\"output_bits\":",
                w.collider,
                w.dt_bits,
                w.scale_bits,
                w.target,
                w.previous,
                w.passed_target,
                w.passed_previous,
                w.applied
            )?;
            if let Some(output) = w.output {
                write!(sink, "{output:?}")?;
            } else {
                sink.write_all(b"null")?;
            }
            sink.write_all(b"}")?;
        }
        sink.write_all(b"]")?;
        sink.write_all(b",\"events\":[")?;
        for (i, e) in s.events[..s.len].iter().enumerate() {
            if i != 0 {
                sink.write_all(b",")?;
            }
            write!(
                sink,
                "{{\"substep\":{},\"force_before\":{},\"force_after\":{},\"integration_coefficient_bits\":{:?},\"coefficient_source\":\"entry_dt_squared_and_child_damping\",\"force_timestamps_us\":{:?},\"force_copy_us\":{:?},\"force_read_calls\":{:?},\"sample\":",
                e.substep,
                e.force_before,
                e.force_after,
                e.coefficients,
                e.force_timestamps,
                e.force_copy_us,
                e.force_read_calls
            )?;
            write_record_named(
                sink,
                &e.record,
                if e.integration {
                    "integration"
                } else {
                    NAMES[e.record.stage]
                },
                if e.integration {
                    INTEGRATE
                } else {
                    RVAS[e.record.stage]
                },
            )?;
            sink.write_all(b"}")?;
        }
        sink.write_all(b"]}")?;
    }
    sink.write_all(b"]}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collider_witness_is_owned_bounded_and_never_stops_solver_capture() {
        let witness = crate::cloth_collider_rotation_hook::Witness {
            collider: 123,
            dt_bits: 0.02f32.to_bits(),
            scale_bits: 0.5f32.to_bits(),
            target: [1; 16],
            previous: [2; 16],
            passed_target: [3; 16],
            passed_previous: [4; 16],
            applied: true,
            output: None,
        };
        assert!(collider_velocity_begin(9, witness).is_none());
        let mut step = Box::new(Step::new());
        step.identity.child = 9;
        step.error = "none";
        ACTIVE.with(|a| *a.borrow_mut() = Some(step));
        assert!(collider_velocity_begin(10, witness).is_none());
        for i in 0..64 {
            assert_eq!(collider_velocity_begin(9, witness), Some(i));
            // A released borrow allows native execution and post-call capture.
            ACTIVE.with(|a| assert!(a.try_borrow_mut().is_ok()));
            collider_velocity_end(9, i, Some([i as u32; 8]));
        }
        assert!(collider_velocity_begin(9, witness).is_none());
        let step = ACTIVE.with(|a| a.borrow_mut().take()).unwrap();
        assert!(!step.failed());
        assert_eq!(step.collider_velocity_error, "capacity");
        assert_eq!(step.collider_velocities.len(), 64);
        assert_eq!(step.collider_velocities[63].output, Some([63; 8]));
        assert_eq!(step.collider_velocities[0].target, [1; 16]);
    }

    #[test]
    fn bounded_copy_preserves_bytes_without_whole_region_queries() {
        let input = vec![0xA5u8; 289 * 16];
        let mut r = Record::new();
        let queries = crate::memory_query::query_count();
        copy_read_scope(|| {
            assert_eq!(byte(input.as_ptr() as usize), Some(0xA5));
            r.copy("bounded", 0, input.as_ptr() as usize, input.len())
                .unwrap();
        });
        assert_eq!(r.data, input);
        assert_eq!(crate::memory_query::query_count() - queries, 0);
    }

    #[test]
    fn bounded_copy_rejects_inaccessible_ranges_without_publishing_partial_data() {
        use windows::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
            VirtualFree, VirtualProtect,
        };
        unsafe {
            let allocation = VirtualAlloc(None, 8192, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
            assert!(!allocation.is_null());
            let base = allocation as usize;
            (base as *mut u8).write(0x5A);
            let mut old = PAGE_READWRITE;
            VirtualProtect((base + 4096) as *const _, 4096, PAGE_NOACCESS, &mut old).unwrap();
            let mut r = Record::new();
            let capacity = r.data.capacity();
            copy_read_scope(|| {
                assert_eq!(byte(base), Some(0x5A));
                assert_eq!(byte(base + 4096), None);
                r.copy("readable", 0, base, 16).unwrap();
                assert!(r.copy("cross_page", 0, base + 4090, 16).is_none());
                assert!(r.copy("null", 0, 0, 16).is_none());
                assert!(r.copy("overflow", 0, usize::MAX, 16).is_none());
                assert!(r.copy("too_large", 0, base, BYTES).is_none());
            });
            assert_eq!(r.data.len(), 16);
            assert_eq!(r.blocks.len(), 1);
            assert_eq!(r.data[0], 0x5A);
            assert_eq!(r.data.capacity(), capacity);
            // Revalidate on each read; no old page permission survives.
            VirtualProtect(allocation, 4096, PAGE_NOACCESS, &mut old).unwrap();
            assert!(copy_read_scope(|| r.copy("revoked", 0, base, 16)).is_none());
            assert_eq!(r.blocks.len(), 1);
            VirtualFree(allocation, 0, MEM_RELEASE).unwrap();
        }
    }

    #[test]
    fn bounded_read_scope_restores_on_nested_scope_and_unwind() {
        assert!(!COPY_READ_ACTIVE.with(Cell::get));
        copy_read_scope(|| {
            assert!(COPY_READ_ACTIVE.with(Cell::get));
            copy_read_scope(|| assert!(COPY_READ_ACTIVE.with(Cell::get)));
            assert!(COPY_READ_ACTIVE.with(Cell::get));
        });
        assert!(!COPY_READ_ACTIVE.with(Cell::get));
        assert!(
            std::panic::catch_unwind(|| copy_read_scope(|| panic!("owned read failure"))).is_err()
        );
        assert!(!COPY_READ_ACTIVE.with(Cell::get));
    }

    pub(super) struct Inputs {
        pub(super) child: Box<[usize; 80]>,
        pub(super) sim: Box<[usize; 64]>,
        current: Vec<[f32; 4]>,
        previous: Vec<[f32; 4]>,
        particles: Vec<[f32; 4]>,
        normals: Vec<[f32; 4]>,
    }
    impl Inputs {
        pub(super) fn new() -> Self {
            let mut f = Self {
                child: Box::new([0; 80]),
                sim: Box::new([0; 64]),
                current: vec![[1., 2., 3., 0.]; 289],
                previous: vec![[0.9, 1.8, 2.7, 0.]; 289],
                particles: vec![[2., 0.5, 0., 0.]; 289],
                normals: vec![[0., 1., 0., 0.]; 289],
            };
            f.child[0x18 / 8] = f.sim.as_ptr() as usize;
            f.child[0x20 / 8] = f.current.as_ptr() as usize;
            f.child[0x28 / 8] = 289;
            f.child[0x30 / 8] = f.previous.as_ptr() as usize;
            f.child[0x40 / 8] = f.normals.as_ptr() as usize;
            f.child[0x48 / 8] = 289;
            f.child[0x50 / 8] = (0.9f32.to_bits() as usize) << 32;
            f.sim[0x40 / 8] = f.particles.as_ptr() as usize;
            f
        }
        pub(super) fn step(&self) -> Box<Step> {
            let mut s = Box::new(Step::new());
            s.error = "none";
            s.identity = Identity {
                child: self.child.as_ptr() as usize,
                sim: self.sim.as_ptr() as usize,
                count: 289,
                scale_bits: 0.5f32.to_bits(),
                ..Identity::default()
            };
            s
        }
    }
    pub(super) unsafe extern "C" fn native_integration(op: usize, child: usize, dt: f32) {
        assert_eq!(op, 0x1234);
        assert_eq!(dt, 0.125);
        ACTIVE.with(|a| assert!(a.try_borrow_mut().is_ok()));
        assert!(!COPY_READ_ACTIVE.with(Cell::get));
        let queries = crate::memory_query::query_count();
        assert!(crate::memory_query::accessible_region(child, 8, false).is_some());
        assert!(crate::memory_query::accessible_region(child, 8, false).is_some());
        assert_eq!(
            crate::memory_query::query_count() - queries,
            2,
            "scope leaked into native execution"
        );
        let mut force = vec![[0f32; 4]; 289];
        let gravity = [0f32, -9.8, 0., 0.];
        let mut regs: Registers = unsafe { std::mem::zeroed() };
        regs.rdi = child as u64;
        regs.rbp = force.as_ptr() as u64;
        regs.r13 = gravity.as_ptr() as u64;
        force_observe(false, &mut regs);
        force.fill([0.1, 0.2, 0.3, 0.]);
        force_observe(true, &mut regs);
        let now = unsafe {
            std::slice::from_raw_parts_mut(ptr(child + 0x20).unwrap() as *mut [f32; 4], 289)
        };
        let old = unsafe {
            std::slice::from_raw_parts_mut(ptr(child + 0x30).unwrap() as *mut [f32; 4], 289)
        };
        for (p, prev) in now.iter_mut().zip(old.iter_mut()) {
            let saved = *p;
            for j in 0..4 {
                p[j] = (p[j] - prev[j]) * 0.9
                    + p[j]
                    + ((2. * gravity[j] + force[0][j]) * 0.5) * (dt * dt);
            }
            *prev = saved;
        }
        force.fill([999.; 4]); // Export must retain the earlier owned values.
    }
    #[test]
    fn integration_dispatch_captures_actual_force_and_preserves_abi_without_borrow_or_scope() {
        let f = Inputs::new();
        ACTIVE.with(|a| *a.borrow_mut() = Some(f.step()));
        let mut r: Registers = unsafe { std::mem::zeroed() };
        r.rcx = 0x1234;
        r.rdx = f.child.as_ptr() as u64;
        r.xmm2 = 0.125f32.to_bits() as u128;
        integration_dispatch(&mut r, native_integration as *const () as usize);
        let s = ACTIVE.with(|a| a.borrow_mut().take()).unwrap();
        assert!(!s.failed(), "{}", s.error);
        assert_eq!(s.len, 1);
        assert!(s.events[0].record.valid);
        assert_eq!(s.events[0].record.before_queries, 0);
        assert_eq!(s.events[0].record.after_queries, 0);
        assert!(s.events[0].record.before_copy_reads > 0);
        assert!(s.events[0].record.after_copy_reads > 0);
        assert!(s.events[0].force_before && s.events[0].force_after);
        assert_eq!(
            s.events[0].coefficients,
            [0.015625f32.to_bits(), 0.9f32.to_bits()]
        );
        assert!(s.open_integration.is_none());
        if let Ok(path) = std::env::var("ERPS_ORDERED_INTEGRATION_FIXTURE") {
            let mut sink = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
                .unwrap();
            write_record_named(&mut sink, &s.events[0].record, "integration", INTEGRATE).unwrap();
        }
    }
    #[test]
    fn ordered_rejects_buffer_replacement_missing_force_duplicate_and_fatal_read_guard() {
        let mut f = Inputs::new();
        let mut s = f.step();
        let id = s.begin_event(0, s.identity.child, 0, 0, 0, true).unwrap();
        s.end_event(id);
        assert_eq!(s.error, "missing_force_boundary");
        let mut s = f.step();
        s.begin_event(0, s.identity.child, 0, 0, 0, true).unwrap();
        let zero = vec![[0f32; 4]; 289];
        s.force(false, s.identity.child, zero.as_ptr() as usize, 0);
        s.force(false, s.identity.child, zero.as_ptr() as usize, 0);
        assert_eq!(s.error, "duplicate_or_reordered_force_boundary");
        let mut r = Record::new();
        r.identity = s.identity;
        assert!(crate::memory_query::scoped(|| positions(&mut r, false)).is_some());
        f.child[0x20 / 8] = f.previous.as_ptr() as usize;
        assert!(crate::memory_query::scoped(|| positions(&mut r, true)).is_none());
        let mut s = f.step();
        s.copy_us = COPY_BUDGET_US + 1;
        s.charge(Instant::now());
        assert!(!s.failed(), "time is measured, not a validity guard");
        s.fail("ordered_before_guard");
        let q = crate::memory_query::query_count();
        assert!(s.begin_event(0, s.identity.child, 0, 0, 0, true).is_none());
        assert_eq!(crate::memory_query::query_count(), q);
        assert_eq!(s.error, "ordered_before_guard");
    }
    #[test]
    fn ordered_mesh_waits_then_uses_fresh_slots_and_owned_writer_has_one_jsonl_line() {
        let session = Session::new();
        session.phase.store(2, Ordering::Release);
        session
            .started
            .set(Instant::now() - Duration::from_millis(START_MS - 10))
            .unwrap();
        assert!(!eligible(&session));
        let session = Session::new();
        session.phase.store(2, Ordering::Release);
        session
            .started
            .set(Instant::now() - Duration::from_millis(START_MS))
            .unwrap();
        assert!(eligible(&session));
        for index in 0..STEPS {
            let mut slot = session.ordered.slots[index].lock().unwrap();
            let s = slot.as_mut().unwrap();
            assert!(s.mesh.source.is_none());
            s.mesh.source = Some(MeshBoundarySource {
                op: index,
                input_buffer: 1,
                output_buffer: 2,
                positions: 0,
                particles: 448,
                frames: 0,
                frame_count: 570,
                binds: 0,
                model: 0,
                owner: 0,
                input: 0,
                generation: 0,
                scale_bits: 0,
            });
            s.len = 1;
            s.events[0].integration = true;
        }
        let mut out = Vec::new();
        write_footer(&mut out, &session, 0, 0).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(out.lines().count(), 1);
        assert_eq!(out.matches("\"stage\":\"integration\"").count(), 3);
        session.ordered.next.store(STEPS, Ordering::Release);
        assert!(!eligible(&session));
    }
    #[test]
    fn ordered_native_seam_guards_reject_each_mutation() {
        let mut image = vec![0u8; FORCE_AFTER + 64];
        for &(rva, expected) in SEAMS {
            image[rva..rva + expected.len()].copy_from_slice(expected);
        }
        let base = image.as_ptr() as usize;
        assert!(crate::memory_query::scoped(|| compatible(base)));
        for &(rva, expected) in SEAMS {
            assert!(expected.len() >= 14);
            image[rva + expected.len() - 1] ^= 1;
            assert!(!crate::memory_query::scoped(|| compatible(base)));
            image[rva + expected.len() - 1] ^= 1;
        }
    }
}

#[cfg(test)]
#[path = "cloth_diagnostic_step_test.rs"]
mod complete_tests;
