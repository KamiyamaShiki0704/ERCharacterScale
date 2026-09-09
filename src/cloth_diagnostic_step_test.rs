//! Production capture routing with a synthetic engine. Tests completeness,
//! ABI and byte ownership under forced read stalls, not physical cloth motion.
use super::tests::{Inputs, native_integration};
use super::*;

thread_local! {
    static NATIVE_CALLS: Cell<usize> = const { Cell::new(0) };
    static DELAY_MID_STEP: Cell<bool> = const { Cell::new(false) };
}
fn put<const N: usize>(buffer: &mut [u8], offset: usize, raw: [u8; N]) {
    buffer[offset..offset + N].copy_from_slice(&raw);
}
struct Graph {
    data: Inputs,
    root: Box<[usize; 64]>,
    children: Box<[usize; 1]>,
    op: Box<[usize; 12]>,
    info: Box<[usize; 6]>,
    config: Box<[usize; 6]>,
    constraints: Vec<[u8; 0x60]>,
    constraint_slots: Vec<usize>,
    buffers: Box<[usize; 4]>,
    mesh_input: Box<[u8; 0x118]>,
    mesh_output: Box<[u8; 0x118]>,
    mesh_bytes: Vec<u8>,
}
impl Graph {
    fn new() -> Self {
        let mut g = Self {
            data: Inputs::new(),
            root: Box::new([0; 64]),
            children: Box::new([0]),
            op: Box::new([0; 12]),
            info: Box::new([0; 6]),
            config: Box::new([0; 6]),
            constraints: vec![[0; 0x60]; 6],
            constraint_slots: vec![0; 6],
            buffers: Box::new([0; 4]),
            mesh_input: Box::new([0; 0x118]),
            mesh_output: Box::new([0; 0x118]),
            mesh_bytes: crate::test_fixtures::bytes("mesh_boundary_249.bin"),
        };
        g.children[0] = g.data.child.as_ptr() as usize;
        g.root[0x40 / 8] = g.children.as_ptr() as usize;
        g.root[0x48 / 8] = 1;
        g.root[0x20 / 8] = g.buffers.as_ptr() as usize;
        g.root[0x28 / 8] = 4;
        g.buffers[3] = g.mesh_output.as_ptr() as usize;
        g.info[0x10 / 8] = g.root.as_ptr() as usize;
        g.op[0x50 / 8] = g.config.as_ptr() as usize;
        g.op[0x58 / 8] = 1;
        g.data.child[0x268 / 8] = g.root.as_ptr() as usize;
        for (stage, vtable) in VTABLES.iter().enumerate().take(7).skip(1) {
            let row = &mut g.constraints[stage - 1];
            put(row.as_mut_slice(), 0, vtable.to_le_bytes());
            if stage == 4 {
                put(row.as_mut_slice(), 0x38, 3u32.to_le_bytes());
                row[0x44] = 1;
            }
            if stage == 6 {
                put(row.as_mut_slice(), 0x48, 3u32.to_le_bytes());
            }
            g.constraint_slots[stage - 1] = row.as_ptr() as usize;
        }
        g.data.sim[0x88 / 8] = g.constraint_slots.as_ptr() as usize;
        g.data.sim[0x90 / 8] = 6;
        let address = g.mesh_bytes.as_ptr() as usize;
        put(g.mesh_input.as_mut_slice(), 0x18, address.to_le_bytes());
        put(g.mesh_input.as_mut_slice(), 0x20, 448u32.to_le_bytes());
        g.mesh_input[0x24] = 16;
        put(
            g.mesh_output.as_mut_slice(),
            0x18,
            (address + 80128).to_le_bytes(),
        );
        put(g.mesh_output.as_mut_slice(), 0x20, 289u32.to_le_bytes());
        put(
            g.mesh_output.as_mut_slice(),
            0x40,
            (address + 84752).to_le_bytes(),
        );
        g.mesh_output[0x24] = 16;
        g.mesh_output[0x4C] = 16;
        put(g.mesh_output.as_mut_slice(), 0x110, 3u32.to_le_bytes());
        for header in [&mut g.mesh_input, &mut g.mesh_output] {
            for base in [0x90, 0xD0] {
                for offset in [0, 20, 40, 60] {
                    put(header.as_mut_slice(), base + offset, 1f32.to_le_bytes());
                }
            }
        }
        g
    }
    fn identity(&self) -> Identity {
        Identity {
            root: self.root.as_ptr() as usize,
            model: 1,
            owner: 2,
            input: 3,
            generation: 1,
            child: self.data.child.as_ptr() as usize,
            sim: self.data.sim.as_ptr() as usize,
            count: 289,
            scale_bits: 0.5f32.to_bits(),
            ..Identity::default()
        }
    }
    fn mesh_source(&self) -> MeshBoundarySource {
        let address = self.mesh_bytes.as_ptr() as usize;
        MeshBoundarySource {
            op: 0x1110,
            input_buffer: self.mesh_input.as_ptr() as usize,
            output_buffer: self.mesh_output.as_ptr() as usize,
            positions: address,
            particles: 448,
            frames: address + 7168,
            frame_count: 570,
            binds: address + 43648,
            model: 1,
            owner: 2,
            input: 3,
            generation: 1,
            scale_bits: 0.5f32.to_bits(),
        }
    }
}
unsafe extern "C" fn integration(op: usize, child: usize, dt: f32) {
    NATIVE_CALLS.with(|n| n.set(n.get() + 1));
    assert_eq!(
        op,
        ACTIVE.with(|a| a.borrow().as_ref().unwrap().outer.object)
    );
    unsafe {
        native_integration(0x1234, child, dt);
    }
}
unsafe extern "C" fn constraint(_object: usize, _child: usize, _strength: f32, flags: usize) {
    NATIVE_CALLS.with(|n| n.set(n.get() + 1));
    assert_eq!(flags, 1);
    assert!(!COPY_READ_ACTIVE.with(Cell::get));
}
unsafe extern "C" fn collision(_child: usize, a: u8, b: u8, strength: f32, c: u8) {
    NATIVE_CALLS.with(|n| n.set(n.get() + 1));
    assert_eq!((a, b, c), (1, 1, 1));
    assert_eq!(strength, 1.);
    assert!(!COPY_READ_ACTIVE.with(Cell::get));
}
unsafe extern "C" fn simulate(op: usize, tag: usize, info: usize) -> usize {
    assert_eq!(tag, 0x123456);
    assert!(!COPY_READ_ACTIVE.with(Cell::get));
    ACTIVE.with(|a| assert!(a.try_borrow_mut().is_ok()));
    let root = ptr(info + 0x10).unwrap();
    let child = ptr(ptr(root + 0x40).unwrap()).unwrap();
    let sim = ptr(child + 0x18).unwrap();
    let slots = ptr(sim + 0x88).unwrap();
    for substep in 0..2 {
        let mut r: Registers = unsafe { std::mem::zeroed() };
        r.rcx = op as u64;
        r.rdx = child as u64;
        r.xmm2 = 0.125f32.to_bits() as u128;
        integration_dispatch(&mut r, integration as *const () as usize);
        for stage in [4, 6, 1, 5, 3, 7] {
            if substep == 0 && stage == 1 && DELAY_MID_STEP.with(Cell::get) {
                COPY_TEST_DELAY_MS.with(|v| v.set(6));
            }
            r.rcx = if stage == 7 {
                child
            } else {
                ptr(slots + (stage - 1) * 8).unwrap()
            } as u64;
            r.rdx = child as u64;
            r.r9 = 1;
            r.xmm2 = (if stage == 4 { substep as f32 } else { 1.0 }).to_bits() as u128;
            if stage == 7 {
                let mut stack = [0u64; 8];
                stack[5] = 1;
                r.rsp = stack.as_mut_ptr() as u64;
                r.rdx = 1;
                r.r8 = 1;
                r.xmm3 = 1f32.to_bits() as u128;
                dispatch(stage, &mut r, collision as *const () as usize);
            } else {
                dispatch(stage, &mut r, constraint as *const () as usize);
            }
        }
    }
    tag ^ op
}

fn run_three_steps(mesh_delay: bool) {
    let g = Graph::new();
    let session = Session::new();
    session.phase.store(2, Ordering::Release);
    session
        .started
        .set(Instant::now() - Duration::from_millis(START_MS))
        .unwrap();
    session.targets.set([g.identity(); 4]).unwrap();
    DELAY_MID_STEP.with(|v| v.set(!mesh_delay));
    NATIVE_CALLS.with(|v| v.set(0));
    for index in 0..3 {
        session.frame.store(40 + index as u64, Ordering::Release);
        if mesh_delay {
            COPY_TEST_DELAY_MS.with(|v| v.set(6));
        }
        let source = g.mesh_source();
        crate::memory_query::scoped(|| mesh_begin(&session, source));
        crate::memory_query::scoped(|| {
            mesh_end(
                &session,
                source.op,
                source.output_buffer,
                g.mesh_bytes.as_ptr() as usize + 84752,
            )
        });
        let slot = &session.ordered.slots[index];
        let step = slot.lock().unwrap().take().unwrap();
        assert!(step.mesh.complete, "mesh stopped: {}", step.error);
        let mut r: Registers = unsafe { std::mem::zeroed() };
        r.rcx = g.op.as_ptr() as u64;
        r.rdx = 0x123456;
        r.r8 = g.info.as_ptr() as u64;
        assert!(copy_read_scope(|| matching_outer(
            step.identity,
            r.rcx as usize,
            r.r8 as usize
        )));
        let mut identity_checks = 0;
        let (step, result) = capture_invocation(
            &session,
            step,
            &r,
            simulate as *const () as usize,
            index,
            |id| {
                identity_checks += 1;
                id == g.identity()
            },
        );
        assert_eq!(result, 0x123456 ^ g.op.as_ptr() as usize);
        assert_eq!(
            step.len, 14,
            "captured only{} events: {}",
            step.len, step.error
        );
        assert_eq!(identity_checks, 2);
        assert!(step.complete, "{}", step.error);
        assert!(
            step.copy_us > COPY_BUDGET_US,
            "delay did not reach measured copy path"
        );
        assert!(step.events[..step.len].iter().all(|e| e.record.valid));
        assert!(step.mesh.paired);
        publish_step(&session, slot, index, step);
        assert!(!session.ordered.pending());
    }
    assert_eq!(NATIVE_CALLS.with(Cell::get), 42);
    DELAY_MID_STEP.with(|v| v.set(false));
    session.stop(4);
    let mut out = Vec::new();
    write_footer(&mut out, &session, 0, 0).unwrap();
    assert_eq!(String::from_utf8_lossy(&out).lines().count(), 1);
    if !mesh_delay && let Ok(path) = std::env::var("ERPS_COMPLETE_STEP_FIXTURE") {
        let mut sink = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        sink.write_all(&out).unwrap();
    }
}
#[test]
#[ignore = "requires private replay data; set ER_CHARACTER_SCALE_FIXTURES"]
fn slow_middle_read_still_records_three_complete_steps() {
    run_three_steps(false);
}
#[test]
#[ignore = "requires private replay data; set ER_CHARACTER_SCALE_FIXTURES"]
fn slow_mesh_read_still_records_three_complete_steps() {
    run_three_steps(true);
}

#[test]
fn window_closes_between_steps_and_stale_mesh_waits_for_native_quiescence() {
    let session = Session::new();
    session.phase.store(2, Ordering::Release);
    session.started.set(Instant::now() - WINDOW).unwrap();
    session.frame.store(40, Ordering::Release);
    session.ordered.slots[0]
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .frame = 40;
    session.ordered.pending.store(true, Ordering::Release);
    assert!(eligible(&session), "admitted step must finish after window");
    poll_end(&session);
    assert_eq!(session.phase.load(Ordering::Acquire), 2);
    session.in_flight.store(1, Ordering::Release);
    session.frame.store(41, Ordering::Release);
    poll_end(&session);
    assert_eq!(session.phase.load(Ordering::Acquire), 2);
    session.in_flight.store(0, Ordering::Release);
    poll_end(&session);
    assert_eq!(session.reason.load(Ordering::Acquire), 6);
    assert_eq!(
        session.ordered.slots[0]
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .error,
        "ordered_mesh_not_consumed_in_frame"
    );

    let idle = Session::new();
    idle.phase.store(2, Ordering::Release);
    idle.started.set(Instant::now() - WINDOW).unwrap();
    assert!(!eligible(&idle));
    poll_end(&idle);
    assert_eq!(idle.reason.load(Ordering::Acquire), 1);
}
