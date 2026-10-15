use super::*;

// Exercise the actual callback around a native producer, not only division.
#[test]
fn bone_space_normal_callback_corrects_fresh_output_once_and_keeps_positions() {
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    let f = Fixture::new(&[Matrix([1.; 16])]);
    f.put(0x3200, f.image + bone_space_skin::VTABLE);
    for (at, off) in [
        (0x2520, 0x2600),
        (0x2600, 0x2700),
        (0x2718, 0x4000),
        (0x2740, 0x4200),
    ] {
        f.put(at, f.heap() + off);
    }
    for (at, v) in [
        (0x2528, 1),
        (0x2720, 3),
        (0x2748, 3),
        (0x3258, 0),
        (0x3260, 0),
        (0x27F0, 0),
        (0x32B0, 2 << 16),
        (0x32B8, f.heap() + 0x5000),
        (0x32C0, 1),
        (0x32A0, f.heap() + 0x6000),
        (0x32A8, 1),
    ] {
        f.put(at, v);
    }
    unsafe {
        *((f.heap() + 0x2724) as *mut u8) = 16;
        *((f.heap() + 0x274c) as *mut u8) = 16;
    }
    unsafe extern "C" fn produce(op: usize, ctx: usize) -> usize {
        let heap = ctx - 0x2300;
        let s = current_scale();
        assert_eq!(op, heap + 0x3200);
        for i in 0..3 {
            unsafe {
                ((heap + 0x4200 + i * 16) as *mut [f32; 4]).write([0.3 * s, 0.4 * s, 0., 99.]);
                ((heap + 0x4000 + i * 16) as *mut [f32; 4]).write([7. * s, 8. * s, 9. * s, 1.]);
            }
        }
        if read_usize(heap + 0x2340) == Some(1) {
            current_unit_state()
                .cloth_topology_generation
                .fetch_add(1, Ordering::AcqRel);
        }
        0xBEEF
    }
    let mut r: Registers = unsafe { std::mem::zeroed() };
    r.rcx = (f.heap() + 0x3200) as u64;
    r.rdx = (f.heap() + 0x2300) as u64;
    for s in [3., 3., 0.3, 0.55, 1., 10., 3.] {
        f.scale(s);
        assert_eq!(
            bone_space_skin::hook(&mut r, produce as *const () as usize),
            0xBEEF
        );
        for i in 0..3 {
            let n = unsafe { ((f.heap() + 0x4200 + i * 16) as *const [f32; 4]).read() };
            assert!(
                (n[0] - 0.3).abs() < 1e-6 && (n[1] - 0.4).abs() < 1e-6,
                "fresh direction scale {s}: {n:?}"
            );
            assert_eq!(n[3], 99.);
            assert_eq!(read_f32(f.heap() + 0x4000 + i * 16), Some(7. * s));
        }
    }
    for (at, value) in [(0x2900, 0), (0x32B4, 1), (0x274c, 20), (0x32B0, 4 << 16)] {
        let old = read_usize(f.heap() + at).unwrap();
        f.put(at, value);
        assert_eq!(
            bone_space_skin::hook(&mut r, produce as *const () as usize),
            0xBEEF
        );
        assert_eq!(read_f32(f.heap() + 0x4200), Some(0.3f32 * 3.));
        f.put(at, old);
    }
    f.put(0x2340, 1);
    bone_space_skin::hook(&mut r, produce as *const () as usize);
    assert_eq!(read_f32(f.heap() + 0x4200), Some(0.3f32 * 3.));
    f.put(0x2340, 0);
    // Execute the real stolen entry instructions through ilhook. They select
    // the buffer in RDX; the owned producer adapter restores its fixture ctx.
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
        let mut code = bone_space_skin::ENTRY_BYTES[..16].to_vec();
        code.extend_from_slice(&[0x48, 0xBA]);
        code.extend_from_slice(&(f.heap() + 0x2300).to_le_bytes());
        code.extend_from_slice(&[0x48, 0xB8]);
        code.extend_from_slice(&(produce as *const () as usize).to_le_bytes());
        code.extend_from_slice(&[0xFF, 0xE0]);
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
        let hooked = unsafe {
            hook_closure_retn(
                allocation as usize,
                bone_space_skin::hook,
                CallbackOption::None,
                HookFlags::empty(),
            )
        }
        .unwrap();
        let call: unsafe extern "C" fn(usize, usize) -> usize = unsafe { transmute(allocation) };
        for s in [3., 0.3, 1., 3.] {
            f.scale(s);
            assert_eq!(
                unsafe { call(f.heap() + 0x3200, f.heap() + 0x2300) },
                0xBEEF
            );
            assert!((read_f32(f.heap() + 0x4200).unwrap() - 0.3).abs() < 1e-6);
        }
        drop(hooked);
        unsafe {
            VirtualFree(allocation, 0, MEM_RELEASE).unwrap();
        }
    }
}

#[test]
#[ignore = "private live BoneSpace reference normals; explicit replay only"]
fn bone_space_private_callback_exports_real_corrected_normals() {
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    let f = Fixture::new(&vec![Matrix([1.; 16]); 4096]);
    f.put(0x3200, f.image + bone_space_skin::VTABLE);
    for (at, off) in [
        (0x2520, 0x2600),
        (0x2600, 0x2700),
        (0x32B8, 0x8000),
        (0x32A0, 0x6000),
    ] {
        f.put(at, f.heap() + off);
    }
    f.put(0x2528, 1);
    f.put(0x3258, 0);
    unsafe {
        *((f.heap() + 0x32B4) as *mut u8) = 0;
    }
    unsafe {
        *((f.heap() + 0x274c) as *mut u8) = 16;
    }
    unsafe extern "C" fn produce(_op: usize, ctx: usize) -> usize {
        let heap = ctx - 0x2300;
        let src = read_usize(heap + 0x2348).unwrap();
        let out = read_usize(heap + 0x2740).unwrap();
        let n = read_usize(heap + 0x2720).unwrap();
        for i in 0..n {
            let mut row = unsafe { ((src + i * 16) as *const [f32; 4]).read_unaligned() };
            for v in &mut row[..3] {
                *v *= current_scale();
            }
            unsafe {
                ((out + i * 16) as *mut [f32; 4]).write_unaligned(row);
            }
        }
        7
    }
    let output =
        std::path::PathBuf::from(std::env::var_os("ER_CHARACTER_SCALE_REPLAY_OUTPUT").unwrap());
    std::fs::create_dir_all(&output).unwrap();
    let mut r: Registers = unsafe { std::mem::zeroed() };
    r.rcx = (f.heap() + 0x3200) as u64;
    r.rdx = (f.heap() + 0x2300) as u64;
    for group in 0..10 {
        let raw = crate::test_fixtures::bytes(&format!("normals-{group}.bin"));
        let n = raw.len() / 16;
        assert_eq!(n * 16, raw.len());
        let mut normals = vec![0u128; n];
        f.put(0x2740, normals.as_mut_ptr() as usize);
        f.put(0x2720, n);
        f.put(0x2748, n);
        unsafe {
            *((f.heap() + 0x274C) as *mut u8) = 16;
        }
        f.put(0x32B0, (n - 1) << 16);
        f.put(0x32C0, n.div_ceil(16));
        f.put(0x32A8, n.div_ceil(16));
        f.put(0x2348, raw.as_ptr() as usize);
        for scale in [0.3f32, 0.55, 3., 10.] {
            f.scale(scale);
            assert_eq!(
                bone_space_skin::hook(&mut r, produce as *const () as usize),
                7
            );
            let bytes =
                unsafe { std::slice::from_raw_parts(normals.as_ptr().cast::<u8>(), n * 16) };
            for (old, new) in raw.chunks_exact(4).zip(bytes.chunks_exact(4)) {
                let a = f32::from_le_bytes(old.try_into().unwrap());
                let b = f32::from_le_bytes(new.try_into().unwrap());
                assert!(
                    (a - b).abs() < 1e-6 || old == new,
                    "group={group} scale={scale} old={a:?} new={b:?}"
                );
            }
            std::fs::write(output.join(format!("normals-{group}-{scale}.bin")), bytes).unwrap();
        }
    }
}

struct Fixture {
    memory: Vec<u128>,
    image: usize,
    slot: usize,
}
impl Fixture {
    fn new(binds: &[Matrix]) -> Self {
        clear_target();
        let image = 0x140000000;
        let memory = vec![0u128; (0x8000 + binds.len() * 64) / 16];
        let mut f = Self {
            memory,
            image,
            slot: 0,
        };
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
            (0x2518, 0x2800),
            (0x2850, 0x2900),
            (0x2900, 0x3200),
            (0x3250, 0x3400),
            (0x3260, 0x8000),
        ] {
            f.put(at, f.heap() + off);
        }
        for (at, rva) in [
            (0x800, crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA),
            (0xA00, crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA),
            (0xC00, ER_CLOTH_MODEL_VTABLE_RVA),
            (0xE00, ER_CLOTH_INNER_VTABLE_RVA),
            (0x1200, ER_POSE_IMPORTER_VTABLE_RVA),
            (0x2800, 0x2D8B738),
            (0x3200, VTABLE),
        ] {
            f.put(at, image + rva);
        }
        f.put(0x2858, 1);
        f.put(0x3258, binds.len());
        f.put(0x3268, binds.len());
        unsafe {
            std::ptr::copy_nonoverlapping(
                binds.as_ptr(),
                (f.heap() + 0x8000) as *mut Matrix,
                binds.len(),
            );
        }
        MODULE_BASE.store(image, Ordering::Release);
        HOOKS_READY.store(true, Ordering::Release);
        let state = current_unit_state();
        state
            .target_cloth_pose_importer
            .store(f.heap() + 0x2280, Ordering::Release);
        state
            .target_pose_importer
            .store(f.heap() + 0x2280, Ordering::Release);
        refresh_owned_cloth_inputs(f.heap() + 0x100, f.heap() + 0x2280);
        f.slot = (0..CLOTH_INSTANCE_SLOTS)
            .find(|&s| state.cloth_instance_inputs[s].load(Ordering::Acquire) == f.heap() + 0x1200)
            .unwrap();
        state.cloth_instance_cores[f.slot].store(f.heap() + 0x1000, Ordering::Release);
        state.cloth_instance_pending_scale_bits[f.slot]
            .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
        f.scale(3.);
        f
    }
    fn heap(&self) -> usize {
        self.memory.as_ptr() as usize
    }
    fn put(&self, at: usize, value: usize) {
        unsafe { ((self.heap() + at) as *mut usize).write_unaligned(value) };
    }
    fn scale(&self, scale: f32) {
        let state = current_unit_state();
        state
            .target_scale_bits
            .store(scale.to_bits(), Ordering::Release);
        state.cloth_instance_applied_scale_bits[self.slot]
            .store(scale.to_bits(), Ordering::Release);
    }
    fn prepare(&self, scratch: &mut Vec<Matrix>) -> Option<Operator> {
        crate::memory_query::scoped(|| prepare(self.heap() + 0x3200, self.heap() + 0x2300, scratch))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        clear_target();
        MODULE_BASE.store(0, Ordering::Release);
        HOOKS_READY.store(false, Ordering::Release);
    }
}

#[test]
fn scoped_binds_preserve_asset_and_reject_stale_routes() {
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    let binds = [Matrix(std::array::from_fn(|i| i as f32 - 5.))];
    let f = Fixture::new(&binds);
    let mut scratch = Vec::new();
    for scale in [3., 3., 0.3, 0.55, 1., 1.12, 10., 3.] {
        f.scale(scale);
        let private = f.prepare(&mut scratch);
        if scale == 1. {
            assert!(private.is_none());
            continue;
        }
        let private = private.unwrap();
        assert_eq!(scratch.as_ptr() as usize % 16, 0);
        assert_eq!(private.0[12], scratch.as_ptr() as u64);
        for i in 0..16 {
            let expected = if i % 4 == 2 {
                binds[0].0[i] / scale
            } else {
                binds[0].0[i]
            };
            assert_eq!(scratch[0].0[i], expected);
        }
        assert_eq!(
            unsafe { ((f.heap() + 0x8000) as *const Matrix).read() }.0,
            binds[0].0
        );
        assert_eq!(read_usize(f.heap() + 0x3260), Some(f.heap() + 0x8000));
    }
    for (at, value) in [
        (0x2900, 0),
        (0x2858, 257),
        (0x2518, 0),
        (0x828, 0),
        (0xD20, 0),
        (0x3268, 8193),
        (0x3258, 2),
        (0x3200, f.image + VTABLE + 8),
    ] {
        let before = read_usize(f.heap() + at).unwrap();
        f.put(at, value);
        assert!(f.prepare(&mut scratch).is_none(), "offset {at:x}");
        f.put(at, before);
    }
    let state = current_unit_state();
    state.cloth_instance_pending_scale_bits[f.slot].store(1f32.to_bits(), Ordering::Release);
    assert!(f.prepare(&mut scratch).is_none());
}

unsafe extern "C" fn native_witness(op: usize, ctx: usize) -> usize {
    let heap = ctx - 0x2300;
    let original = heap + 0x3200;
    let binds = read_usize(op + 0x60).unwrap();
    let scale = current_scale();
    if scale != 1. {
        assert_ne!(op, original);
        assert_ne!(binds, heap + 0x8000);
        assert_eq!(read_f32(binds + 14 * 4), Some(2. / scale));
    } else {
        assert_eq!(op, original);
    }
    // Reentrant invocations must have distinct owned buffers and restore outer.
    let depth = read_usize(heap + 0x2340).unwrap();
    if depth == 0 {
        unsafe {
            ((heap + 0x2340) as *mut usize).write(1);
        }
        let before = unsafe { (binds as *const [f32; 16]).read_unaligned() };
        let mut r: Registers = unsafe { std::mem::zeroed() };
        r.rcx = original as u64;
        r.rdx = ctx as u64;
        assert_eq!(hook(&mut r, native_witness as *const () as usize), 0xACE);
        assert_eq!(
            unsafe { (binds as *const [f32; 16]).read_unaligned() },
            before
        );
        unsafe {
            ((heap + 0x2340) as *mut usize).write(0);
        }
    }
    0xACE
}

#[test]
fn native_invocation_reentrant_scratch_and_unchanged_neutral_call() {
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    let f = Fixture::new(&[Matrix([2.; 16])]);
    let mut r: Registers = unsafe { std::mem::zeroed() };
    r.rcx = (f.heap() + 0x3200) as u64;
    r.rdx = (f.heap() + 0x2300) as u64;
    for scale in [3., 0.3, 1., 10., 3.] {
        f.scale(scale);
        assert_eq!(hook(&mut r, native_witness as *const () as usize), 0xACE);
    }
    let start = std::time::Instant::now();
    for _ in 0..2000 {
        hook(&mut r, native_witness as *const () as usize);
    }
    println!(
        "simple-bone two nested callbacks mean_us={:0.3}",
        start.elapsed().as_secs_f64() * 1e6 / 2000.
    );
}

#[test]
fn inverse_area_coefficients_reject_late_nonfinite_and_overflow_transactionally() {
    for invalid in [f32::NAN, f32::INFINITY, f32::MAX] {
        let mut rows = [Matrix([1.; 16]), Matrix([1.; 16])];
        rows[1].0[14] = invalid;
        let before = rows.map(|r| r.0.map(f32::to_bits));
        assert!(compensate(&mut rows, 0.3).is_none());
        assert_eq!(rows.map(|r| r.0.map(f32::to_bits)), before);
    }
    for scale in [0., -1., f32::NAN, f32::INFINITY] {
        assert!(compensate(&mut [Matrix([1.; 16])], scale).is_none());
    }
}

#[test]
fn installed_entry_trampoline_uses_private_operator_and_preserves_return() {
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
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    let f = Fixture::new(&[Matrix([2.; 16])]);
    // The first 16 real entry bytes are exactly the stolen instructions.
    // They touch RAX/R8/R10/R11 and read the actual operator/context layouts.
    let mut code = ENTRY_BYTES[..16].to_vec();
    code.extend_from_slice(&[0x48, 0xB8]);
    code.extend_from_slice(&(native_witness as *const () as usize).to_le_bytes());
    code.extend_from_slice(&[0xFF, 0xE0]);
    let allocation = unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    assert!(!allocation.is_null());
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), allocation.cast::<u8>(), code.len());
    }
    let mut old = PAGE_PROTECTION_FLAGS::default();
    unsafe {
        VirtualProtect(allocation, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
        FlushInstructionCache(HANDLE(-1isize as *mut _), Some(allocation), 4096).unwrap();
    }
    let hooked = unsafe {
        hook_closure_retn(
            allocation as usize,
            hook,
            CallbackOption::None,
            HookFlags::empty(),
        )
    }
    .unwrap();
    let call: unsafe extern "C" fn(usize, usize) -> usize = unsafe { transmute(allocation) };
    for scale in [3., 0.3, 1., 3.] {
        f.scale(scale);
        assert_eq!(unsafe { call(f.heap() + 0x3200, f.heap() + 0x2300) }, 0xACE);
    }
    drop(hooked);
    unsafe {
        VirtualFree(allocation, 0, MEM_RELEASE).unwrap();
    }
}

#[test]
#[ignore = "private authored bind fixture; explicit detached replay only"]
fn simple_bone_private_fixture_exports_scoped_production_arrays() {
    let _lock = MODULE_TEST_LOCK.lock().unwrap();
    let raw = crate::test_fixtures::bytes("simple-bone-binds.bin");
    assert_eq!(raw.len() % 64, 0);
    let rows: Vec<_> = raw
        .chunks_exact(64)
        .map(|b| {
            Matrix(std::array::from_fn(|i| {
                f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())
            }))
        })
        .collect();
    let f = Fixture::new(&rows);
    let mut scratch = Vec::new();
    let output =
        std::path::PathBuf::from(std::env::var_os("ER_CHARACTER_SCALE_REPLAY_OUTPUT").unwrap());
    std::fs::create_dir_all(&output).unwrap();
    for scale in [0.3f32, 0.55, 1.12, 3., 10.] {
        f.scale(scale);
        let private = f.prepare(&mut scratch).unwrap();
        assert_ne!(private.0[12] as usize, f.heap() + 0x8000);
        let raw = unsafe {
            std::slice::from_raw_parts(scratch.as_ptr().cast::<u8>(), scratch.len() * 64)
        };
        std::fs::write(output.join(format!("simple-bone-{scale}.bin")), raw).unwrap();
    }
}
