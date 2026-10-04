use super::*;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
    VirtualAlloc, VirtualFree, VirtualProtect,
};
static LOCK: Mutex<()> = Mutex::new(());

#[test]
fn actual_scatter_trampoline_preserves_all_thirteen_arguments() {
    let _lock = LOCK.lock().unwrap();
    struct Page(*mut std::ffi::c_void);
    impl Drop for Page {
        fn drop(&mut self) {
            unsafe {
                VirtualFree(self.0, 0, MEM_RELEASE).unwrap();
            }
        }
    }
    let page = Page(unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) });
    assert!(!page.0.is_null());
    let mut code = vec![0x90; 32];
    code.extend([
        0x49, 0x89, 0x08, 0x49, 0x89, 0x50, 0x08, 0x4d, 0x89, 0x48, 0x10,
    ]);
    for i in 0..9u8 {
        code.extend([
            0x48,
            0x8b,
            0x44,
            0x24,
            0x28 + i * 8,
            0x49,
            0x89,
            0x40,
            0x18 + i * 8,
        ]);
    }
    code.push(0xc3);
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), page.0.cast(), code.len());
    }
    let mut old = PAGE_PROTECTION_FLAGS::default();
    unsafe {
        VirtualProtect(page.0, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
    }
    let native: Native = unsafe { std::mem::transmute(page.0) };
    let hook = unsafe {
        hook_closure_retn(
            page.0 as usize,
            hook,
            CallbackOption::None,
            HookFlags::empty(),
        )
        .unwrap()
    };
    let mut out = [0usize; 12];
    unsafe {
        native(
            7,
            0x2222,
            out.as_mut_ptr() as usize,
            0x4444,
            0x5555,
            6,
            0x7777,
            0x8888,
            9,
            0xaaaa,
            0xbbbb,
            0xcccc,
            13,
        );
    }
    drop(hook);
    assert_eq!(&out[..4], &[7, 0x2222, 0x4444, 0x5555]);
    assert_eq!(out[4] as i16, 6);
    assert_eq!(out[5], 0x7777);
    assert_eq!(out[6], 0x8888);
    assert_eq!(out[7] as u8, 9);
    assert_eq!(&out[8..11], &[0xaaaa, 0xbbbb, 0xcccc]);
    assert_eq!(out[11] as u8, 13);
}

#[test]
fn runtime_binding_reorders_samples_and_reuses_owned_buffers() {
    let _lock = LOCK.lock().unwrap();
    use crate::equipment_retarget::LocalPose;
    use glam::{DQuat, DVec3};
    let names = [
        "Master",
        "Pelvis",
        "Spine",
        "Head",
        "L_Thigh",
        "R_Thigh",
        "L_UpperArm",
        "R_UpperArm",
    ];
    let source: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(i, n)| Bone {
            name: (*n).into(),
            parent: (i != 0).then_some(0),
            reference: LocalPose {
                translation: DVec3::new(i as f64, 1., 0.),
                rotation: DQuat::IDENTITY,
                scale: DVec3::ONE,
            },
        })
        .collect();
    let mut target = source.clone();
    for b in &mut target[1..] {
        b.reference.translation *= 0.5;
    }
    target.swap(6, 7);
    let mut animation = [0u8; 0x40];
    animation[0x20..0x24].copy_from_slice(&8i32.to_le_bytes());
    let indices = [7i16, 6, 5, 4, 3, 2, 1, 0];
    let mut binding = [0u8; 0x60];
    for (at, value) in [
        (0, 0x2D4D2B0),
        (0x18, c"c3010".as_ptr() as usize),
        (0x20, animation.as_ptr() as usize),
        (0x28, indices.as_ptr() as usize),
    ] {
        binding[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    binding[0x30..0x34].copy_from_slice(&8i32.to_le_bytes());
    let samples: Vec<_> = indices
        .iter()
        .flat_map(|&i| {
            pose::qs_output(source[i as usize].reference)
                .unwrap()
                .into_iter()
                .flat_map(f32::to_le_bytes)
        })
        .collect();
    BASE.store(0, Ordering::Release);
    *TARGETS.write().unwrap() = Some(HashMap::from([(
        0x4321,
        Registered {
            header: [0; 0x50],
            target: Arc::new(Target {
                name: "c0000".into(),
                bones: target.clone(),
                active: AtomicBool::new(true),
            }),
        },
    )]));
    *SOURCES.lock().unwrap() = Some(HashMap::from([(
        "c3010".into(),
        Source::Ready(Arc::new(source)),
    )]));
    let args = Args {
        count: 8,
        samples: samples.as_ptr() as usize,
        output: 0,
        indices: binding.as_ptr() as usize + 0x28,
        partitions: 0,
        partition_count: 0,
        mapper: 0,
        reference: 0,
        additive: 0,
        skeleton: 0x4321,
        mirrored: 0,
        mask: 0,
        mirror: 1,
    };
    let mut cache = Cache::default();
    let first = crate::memory_query::scoped(|| prepare(args, &mut cache)).unwrap();
    assert_eq!(first.count, 8);
    assert_ne!(first.samples, args.samples);
    assert_ne!(first.indices, args.indices);
    // Root-motion lookup is keyed by this generator's binding and target,
    // never the last foreign clip seen on another worker or character.
    let mut generator = [0u8; 0x100];
    generator[0xf0..0xf8].copy_from_slice(&(binding.as_ptr() as usize).to_le_bytes());
    cache
        .bindings
        .get_mut(&(0x4321, binding.as_ptr() as usize))
        .unwrap()
        .plan
        .as_mut()
        .unwrap()
        .motion = Some(crate::equipment_retarget::MotionProfile {
        leg_ratio: 0.5,
        height_ratio: 0.5,
    });
    let profile = crate::memory_query::scoped(|| {
        motion::ratio(&read, generator.as_ptr() as usize, 0x4321, &cache)
    })
    .unwrap();
    assert_eq!(
        profile.displacement([2., -4., 6., 9.]),
        Some([1., -2., 3., 9.])
    );
    assert!(
        crate::memory_query::scoped(|| motion::ratio(
            &read,
            generator.as_ptr() as usize,
            0x1234,
            &cache
        ))
        .is_none()
    );
    generator[0xe8..0xf0].copy_from_slice(&1usize.to_le_bytes());
    assert!(
        crate::memory_query::scoped(|| motion::ratio(
            &read,
            generator.as_ptr() as usize,
            0x4321,
            &cache
        ))
        .is_none()
    );
    generator[0xe8..0xf0].fill(0);
    cache.generation = cache.generation.wrapping_add(1);
    assert!(
        crate::memory_query::scoped(|| motion::ratio(
            &read,
            generator.as_ptr() as usize,
            0x4321,
            &cache
        ))
        .is_none()
    );
    cache.generation = GENERATION.load(Ordering::Acquire);
    let mut stack = [0usize; 64];
    stack[0x1b0 / 8] = 0x4321;
    let mut registers: Registers = unsafe { std::mem::zeroed() };
    registers.rsp = stack.as_ptr() as u64;
    registers.rsi = generator.as_ptr() as u64;
    CACHE.with(|c| {
        let previous = c.replace(std::mem::take(&mut cache));
        let mut delta = [2., -4., 6., 9.];
        motion::callback(&mut registers, &mut delta);
        assert_eq!(delta, [1., -2., 3., 9.]);
        stack[0x1c8 / 8] = 1; // native additive mode
        delta = [2., -4., 6., 9.];
        motion::callback(&mut registers, &mut delta);
        assert_eq!(delta, [2., -4., 6., 9.]);
        stack[0x1c8 / 8] = 0;
        stack[0x1a0 / 8] = 1; // native mirror adapter
        motion::callback(&mut registers, &mut delta);
        assert_eq!(delta, [2., -4., 6., 9.]);
        stack[0x1a0 / 8] = 0;
        cache = c.replace(previous);
    });
    let original = binding;
    for _ in 0..100 {
        let next = crate::memory_query::scoped(|| prepare(args, &mut cache)).unwrap();
        assert_eq!(next.samples, first.samples);
        assert_eq!(next.indices, first.indices);
    }
    assert_eq!(binding, original);
    // The exact same binding pointer can be sampled for another character.
    // Each target owns different proportions and a different root-motion ratio.
    let mut larger = target.clone();
    for b in &mut larger[1..] {
        b.reference.translation *= 3.0;
    }
    let second_target = Arc::new(Target {
        name: "c3100".into(),
        bones: larger,
        active: AtomicBool::new(true),
    });
    TARGETS.write().unwrap().as_mut().unwrap().insert(
        0x8765,
        Registered {
            header: [0; 0x50],
            target: second_target.clone(),
        },
    );
    let other = Args {
        skeleton: 0x8765,
        ..args
    };
    let next = crate::memory_query::scoped(|| prepare(other, &mut cache)).unwrap();
    assert_ne!(next.samples, first.samples);
    let other_qs = unsafe {
        std::slice::from_raw_parts(next.samples as *const crate::animation_retarget::Qs, 8)
    };
    assert!((other_qs[1].0[0] - 1.5).abs() < 1e-6);
    assert_eq!(
        crate::memory_query::scoped(|| prepare(args, &mut cache))
            .unwrap()
            .samples,
        first.samples
    );
    second_target.active.store(false, Ordering::Release);
    assert!(crate::memory_query::scoped(|| prepare(other, &mut cache)).is_none());
    assert!(
        crate::memory_query::scoped(|| motion::ratio(
            &read,
            generator.as_ptr() as usize,
            0x8765,
            &cache
        ))
        .is_none()
    );
    TARGETS.write().unwrap().as_mut().unwrap().insert(
        0x8765,
        Registered {
            header: [0; 0x50],
            target: Arc::new(Target {
                name: "c3010".into(),
                bones: target.clone(),
                active: AtomicBool::new(true),
            }),
        },
    );
    assert!(
        crate::memory_query::scoped(|| prepare(other, &mut cache)).is_none(),
        "native clips must bypass retargeting even when another target uses this binding"
    );
    let qs = unsafe {
        std::slice::from_raw_parts(first.samples as *const crate::animation_retarget::Qs, 8)
    };
    for (q, b) in qs.iter().zip(&target) {
        assert!((q.0[0] as f64 - b.reference.translation.x).abs() < 1e-6);
    }
    let mut invalid = args;
    invalid.additive = 1;
    assert_eq!(
        crate::memory_query::scoped(|| prepare(invalid, &mut cache))
            .unwrap()
            .count,
        0
    );
    *SOURCES.lock().unwrap() = Some(HashMap::from([("c3010".into(), Source::Failed)]));
    cache.bindings.clear();
    assert_eq!(
        crate::memory_query::scoped(|| prepare(args, &mut cache))
            .unwrap()
            .count,
        0
    );
    suspend();
    *TARGETS.write().unwrap() = None;
    *SOURCES.lock().unwrap() = None;
}
