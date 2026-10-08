use super::*;
use crate::body_scale_port::test_characters::{BASE as TEST_BASE, Character, Environment};
use crate::equipment_retarget::LocalPose;
use glam::DVec3;

#[test]
fn camera_updates_consumed_height_only_and_respects_explicit_camera_override() {
    let mut fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    Arc::get_mut(&mut REGISTRY.write().unwrap()[0])
        .unwrap()
        .motion = Some(MotionProfile {
        leg_ratio: 0.75,
        height_ratio: 0.6,
    });
    let world = fixture._heap.alloc(0x1F000);
    let camera = fixture._heap.alloc(0x100);
    let follow = fixture._heap.alloc(0x200);
    let manager = fixture._heap.alloc(0x100);
    fixture
        ._heap
        .put(world + 0x1E508, fixture.character.address);
    fixture._heap.put(world + 0x1ECE0, camera);
    fixture._heap.put(camera + 0x60, follow);
    fixture._heap.put(follow, TEST_BASE + 0x2A2ACA0);
    fixture._heap.put(follow + 0x194, 1.42f32);
    fixture._heap.put(manager + 0x50, -1i32);
    let mut r: Registers = unsafe { std::mem::zeroed() };
    r.r15 = fixture.character.address as u64;
    r.rbx = follow as u64;
    let high = 0x1234_5678_9ABC_DEF0_1234_5678_0000_0000u128;
    for _ in 0..20 {
        r.xmm3 = high | u128::from(1.42f32.to_bits());
        adjust_camera(&mut r, TEST_BASE, world, manager).unwrap();
        assert!((f32::from_bits(r.xmm3 as u32) - 0.852).abs() < 1e-6);
        assert_eq!(r.xmm3 & !u128::from(u32::MAX), high);
        assert_eq!(
            pose::bytes::<4>(&read, follow + 0x194),
            Some(1.42f32.to_le_bytes())
        );
    }
    fixture._heap.put(manager + 0x50, 220i32);
    r.xmm3 = high | u128::from(1.42f32.to_bits());
    assert!(adjust_camera(&mut r, TEST_BASE, world, manager).is_none());
    assert_eq!(f32::from_bits(r.xmm3 as u32), 1.42);
    fixture._heap.put(manager + 0x50, -1i32);
    r.r15 += 8;
    assert!(adjust_camera(&mut r, TEST_BASE, world, manager).is_none());
}

#[test]
fn motion_consumer_scales_fresh_animation_only_and_revokes_on_unequip() {
    let fixture = NativeTest::new();
    refresh(fixture.character.address, 2.0, 1);
    {
        let mut registry = REGISTRY.write().unwrap();
        Arc::get_mut(&mut registry[0]).unwrap().motion = Some(MotionProfile {
            leg_ratio: 0.75,
            height_ratio: 0.6,
        });
    }
    let controller = ptr(fixture.character.address + 0x58).unwrap();
    let mut registers: Registers = unsafe { std::mem::zeroed() };
    registers.rbx = controller as u64;
    registers.rdi = (controller + 0x140) as u64;
    for frame in 0..50 {
        fixture.character.put(0x2140, [4.0f32, 2.0, -8.0, 123.0]);
        fixture.character.put(0x2150, [0.0f32, 0.2, 0.0, 0.98]);
        motion_hook(&mut registers);
        assert_eq!(
            fixture.character.get::<[f32; 4]>(0x2140),
            [3.0, 1.5, -6.0, 123.0],
            "frame {frame}"
        );
        assert_eq!(
            fixture.character.get::<[f32; 4]>(0x2150),
            [0.0, 0.2, 0.0, 0.98]
        );
    }
    fixture._heap.put(fixture.slot, 0usize);
    fixture.character.put(0x2140, [4.0f32, 2.0, -8.0, 123.0]);
    motion_hook(&mut registers);
    assert_eq!(
        fixture.character.get::<[f32; 4]>(0x2140),
        [4.0, 2.0, -8.0, 123.0]
    );
}

struct Heap {
    data: Vec<u128>,
    cursor: usize,
}

#[test]
fn repeated_session_validation_reuses_permissions_but_reads_identity_fresh() {
    let fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    let session = REGISTRY.read().unwrap()[0].clone();
    crate::memory_query::scoped(|| {
        assert!(session.current());
        let before = crate::memory_query::query_count();
        assert!(session.current());
        let repeated = crate::memory_query::query_count() - before;
        assert_eq!(
            repeated, 0,
            "nested identity validation discarded the current operation's permissions"
        );
        // Fresh data is still required even with permissions already checked.
        fixture._heap.put(fixture.slot, 0usize);
        assert!(!session.current());
    });
}

#[test]
fn equipment_without_cloth_retargets_through_first_mapper() {
    let fixture = NativeTest::new();
    let equipment = ptr(fixture.slot).unwrap();
    let item = ptr(equipment + 0x10).unwrap();
    let exporter = ptr(item + 0x658).unwrap();
    let holder = ptr(exporter + 0x70).unwrap();
    let owner = ptr(equipment + 0x130).unwrap();
    fixture._heap.put(exporter + 0x70, 0usize);
    fixture._heap.put(equipment + 0x130, 0usize);
    refresh(fixture.character.address, 1.0, 1);
    assert_eq!(
        REGISTRY.read().unwrap().len(),
        1,
        "equipment without cloth must bind"
    );
    let out = fixture.draw();
    assert!((out[2][3] - 1.1).abs() < 1e-5);
    assert!(REGISTRY.read().unwrap()[0].cloth_plan.is_none());
    assert!(solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).is_none());
    fixture._heap.put(exporter + 0x70, holder);
    assert!(!REGISTRY.read().unwrap()[0].key.current(TEST_BASE));
    refresh(fixture.character.address, 1.0, 2);
    assert_eq!(REGISTRY.read().unwrap().len(), 1);
    assert!((fixture.draw()[2][3] - 1.1).abs() < 1e-5);
    // A cloth owner appearing later must revoke the mesh-only session and
    // restore the ordinary requirement that physics consumes the target pose.
    fixture._heap.put(equipment + 0x130, owner);
    assert!(!REGISTRY.read().unwrap()[0].key.current(TEST_BASE));
    refresh(fixture.character.address, 1.0, 3);
    assert_eq!(fixture.draw(), [[99.0; 12]; 4]);
    let mut input = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
    input.context();
    input.completed();
    assert!((fixture.draw()[2][3] - 1.1).abs() < 1e-5);
}

#[test]
fn empty_mapper_lists_and_unreadable_nonempty_lists_cannot_bind() {
    let fixture = NativeTest::new();
    let equipment = ptr(fixture.slot).unwrap();
    let exporter = ptr(ptr(equipment + 0x10).unwrap() + 0x658).unwrap();
    let first = ptr(exporter + 0x50).unwrap();
    fixture._heap.put(equipment + 0x130, 0usize);
    fixture._heap.put(exporter + 0x50, 0usize);
    fixture._heap.put(exporter + 0x70, 0usize);
    refresh(fixture.character.address, 1.0, 1);
    assert!(REGISTRY.read().unwrap().is_empty());
    fixture._heap.put(exporter + 0x50, first);
    fixture._heap.put(exporter + 0x70, 1usize);
    refresh(fixture.character.address, 1.0, 2);
    assert!(
        REGISTRY.read().unwrap().is_empty(),
        "unreadable list is not an absent list"
    );
    fixture._heap.put(exporter + 0x70, 0usize);
    refresh(fixture.character.address, 1.0, 3);
    assert_eq!(REGISTRY.read().unwrap().len(), 1);
}

#[test]
fn invalid_mesh_counts_do_not_get_cached_as_boneless_models() {
    let fixture = NativeTest::new();
    let equipment = ptr(fixture.slot).unwrap();
    let item = ptr(equipment + 0x10).unwrap();
    let exporter = ptr(item + 0x658).unwrap();
    let resource = ptr(item + 0x68).unwrap();
    fixture._heap.put(exporter + 0x70, 0usize);
    fixture._heap.put(equipment + 0x130, 0usize);
    for count in [-1, pose::LIMIT as i32 + 1] {
        fixture._heap.put(resource + 0x1C, count);
        refresh(fixture.character.address, 1.0, 1);
        let native_count = NATIVE.lock().unwrap().len();
        assert_eq!(native_count, 0);
        assert!(REGISTRY.read().unwrap().is_empty());
        assert_eq!(REJECTED.lock().unwrap().len(), 1);
        suspend();
    }
}

#[test]
fn equipment_without_bones_stays_native_until_resource_changes() {
    let fixture = NativeTest::new();
    let equipment = ptr(fixture.slot).unwrap();
    let item = ptr(equipment + 0x10).unwrap();
    let exporter = ptr(item + 0x658).unwrap();
    let resource = ptr(item + 0x68).unwrap();
    let inverse = ptr(resource + 0x2F8).unwrap();
    let skeleton = ptr(fixture.mapper + 0x90).unwrap();
    let bones = ptr(skeleton + 8).unwrap();
    fixture._heap.put(exporter + 0x70, 0usize);
    fixture._heap.put(equipment + 0x130, 0usize);
    fixture._heap.put(resource + 0x1C, 0i32);
    fixture._heap.put(resource + 0x2F8, 0usize);
    fixture._heap.put(skeleton + 8, 0usize);
    refresh(fixture.character.address, 1.0, 1);
    assert!(REGISTRY.read().unwrap().is_empty());
    let native_count = NATIVE.lock().unwrap().len();
    assert_eq!(native_count, 1, "boneless model must cache native behavior");
    assert!(
        REJECTED.lock().unwrap().is_empty(),
        "boneless model must not queue retries"
    );
    for frame in 2..362 {
        refresh(fixture.character.address, 1.0, frame);
        assert_eq!(NATIVE.lock().unwrap().len(), 1);
        assert!(REJECTED.lock().unwrap().is_empty());
    }
    assert_eq!(fixture.draw(), [[99.0; 12]; 4]);
    fixture._heap.put(resource + 0x2F8, inverse);
    fixture._heap.put(skeleton + 8, bones);
    fixture._heap.put(resource + 0x1C, 4i32);
    refresh(fixture.character.address, 1.0, 362);
    assert!(NATIVE.lock().unwrap().is_empty());
    assert_eq!(REGISTRY.read().unwrap().len(), 1);
    assert!((fixture.draw()[2][3] - 1.1).abs() < 1e-5);
}
impl Heap {
    fn new() -> Self {
        Self {
            data: vec![0; 0x10000],
            cursor: 0x100,
        }
    }
    fn alloc(&mut self, size: usize) -> usize {
        let at = self.data.as_ptr() as usize + self.cursor;
        self.cursor += (size + 15) & !15;
        assert!(self.cursor < self.data.len() * 16);
        at
    }
    fn put<T: Copy>(&self, at: usize, value: T) {
        assert!(
            at >= self.data.as_ptr() as usize
                && at + std::mem::size_of::<T>()
                    <= self.data.as_ptr() as usize + self.data.len() * 16
        );
        unsafe {
            (at as *mut T).write_unaligned(value);
        }
    }
    fn text(&mut self, name: &str, wide: bool) -> usize {
        let raw: Vec<u8> = if wide {
            name.encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect()
        } else {
            name.bytes().chain([0]).collect()
        };
        let at = self.alloc(raw.len());
        for (i, v) in raw.iter().enumerate() {
            self.put(at + i, *v);
        }
        at
    }
}

fn skeleton_input(heap: &mut Heap, lengths: &[f64], names: &[&str]) -> (usize, usize, usize) {
    let n = lengths.len();
    let input = heap.alloc(0x100);
    let meta = heap.alloc(0x80);
    let parents = heap.alloc(n * 2);
    let descriptors = heap.alloc(n * 16);
    let refs = heap.alloc(n * 48);
    let locals = heap.alloc(n * 48);
    let models = heap.alloc(n * 48);
    let flags = heap.alloc(n * 4);
    heap.put(input, TEST_BASE + INPUT_VTABLE);
    heap.put(input + 0x48, meta);
    for (offset, ptr) in [(0x20, parents), (0x30, descriptors), (0x40, refs)] {
        heap.put(meta + offset, ptr);
        heap.put(meta + offset + 8, n as i32);
    }
    for (offset, ptr) in [(8, locals), (0x18, models), (0x28, flags)] {
        heap.put(input + 0x48 + offset, ptr);
        heap.put(input + 0x48 + offset + 8, n as i32);
    }
    let mut x = 0.0;
    for (i, (&length, name)) in lengths.iter().zip(names).enumerate() {
        heap.put(parents + i * 2, if i == 0 { -1i16 } else { i as i16 - 1 });
        let name_at = heap.text(name, false);
        heap.put(descriptors + i * 16, name_at);
        let local = LocalPose {
            translation: DVec3::X * length,
            ..LocalPose::IDENTITY
        };
        heap.put(refs + i * 48, pose::qs_output(local).unwrap());
        heap.put(locals + i * 48, pose::qs_output(local).unwrap());
        x += length;
        heap.put(
            models + i * 48,
            pose::qs_output(LocalPose {
                translation: DVec3::X * x,
                ..LocalPose::IDENTITY
            })
            .unwrap(),
        );
    }
    (input, meta, models)
}

struct NativeTest {
    _environment: Environment,
    _heap: Heap,
    character: Character,
    slot: usize,
    mapper: usize,
    inner: usize,
    transform: usize,
    input: usize,
    input_models: usize,
    source_models: usize,
    base: usize,
    ready: bool,
}

impl NativeTest {
    fn new() -> Self {
        let environment = Environment::new();
        let character = Character::new(0, 8877, 1.0);
        let mut heap = Heap::new();
        let names = ["L_UpperArm", "L_Forearm", "L_Hand", "hair"];
        let (source, _, source_models) = skeleton_input(&mut heap, &[0.0, 2.0, 1.0], &names[..3]);
        let (input, _, input_models) = skeleton_input(&mut heap, &[0.0, 2.0, 1.0, 0.2], &names);
        character.put(0x398, source);
        let assembly = heap.alloc(0x180);
        let equipment = heap.alloc(0x200);
        let item = heap.alloc(0x800);
        let owner = heap.alloc(0x180);
        let inner = heap.alloc(0x80);
        let core = heap.alloc(0x100);
        let resource = heap.alloc(0x600);
        let exporter = heap.alloc(0x180);
        let mapper = heap.alloc(0xA0);
        let holder = heap.alloc(16);
        let skeleton = heap.alloc(16);
        let bones = heap.alloc(4 * 64);
        let inverse = heap.alloc(4 * 48);
        character.put(0x648, assembly);
        heap.put(
            assembly,
            TEST_BASE + crate::cloth_owner_scope::ASSEMBLY_VTABLE_RVA,
        );
        heap.put(assembly + 8, character.at(0xD000));
        heap.put(assembly + 0x28, equipment);
        heap.put(
            equipment,
            TEST_BASE + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA,
        );
        heap.put(equipment + 0x10, item);
        heap.put(equipment + 0x130, owner);
        heap.put(item + 0x68, resource);
        heap.put(item + 0x658, exporter);
        let name = heap.text("BD_M_TEST", true);
        heap.put(item + 0x6C8, name);
        heap.put(item + 0x6D8, 9usize);
        heap.put(item + 0x6E0, 16usize);
        heap.put(exporter, TEST_BASE + 0x2B701A0);
        heap.put(exporter + 0x100, resource);
        for offset in [0x50, 0x70] {
            heap.put(exporter + offset, holder);
        }
        heap.put(holder, mapper);
        heap.put(mapper, TEST_BASE + MAPPER_VTABLE);
        heap.put(mapper + 0x90, skeleton);
        heap.put(skeleton + 8, bones);
        heap.put(resource + 0x1C, 4i32);
        heap.put(resource + 0x2F8, inverse);
        for (i, x) in [0.0, 0.7, 1.1, 1.3].into_iter().enumerate() {
            let name = heap.text(names[i], true);
            heap.put(bones + i * 64 + 0x20, name);
            heap.put(
                bones + i * 64 + 0x2C,
                if i == 0 { -1i16 } else { i as i16 - 1 },
            );
            heap.put(
                inverse + i * 48,
                pose::affine_output(DMat4::from_translation(DVec3::X * -x)).unwrap(),
            );
        }
        heap.put(owner, TEST_BASE + 0x2B92A60);
        heap.put(owner + 0x120, input);
        heap.put(owner + 0x40, inner);
        heap.put(owner + 0x68, 1u8);
        heap.put(inner, TEST_BASE + 0x329A2F8);
        heap.put(inner + 0x60, 1u8);
        heap.put(inner + 0x30, core);
        heap.put(core, TEST_BASE + 0x329E3D0);
        heap.put(core + 0x18, owner);
        let entries = heap.alloc(0x38);
        let output = heap.alloc(0x28);
        let map = heap.alloc(0x20);
        let indices = heap.alloc(8);
        let mask = heap.alloc(4);
        heap.put(core + 0x40, entries);
        heap.put(core + 0x48, 1i32);
        heap.put(entries, output);
        heap.put(entries + 8, map);
        heap.put(output + 0x20, 4i32);
        heap.put(map + 0x10, 4u32);
        heap.put(map + 0x18, indices);
        for i in 0..4 {
            heap.put(indices + i * 2, i as i16);
        }
        heap.put(entries + 0x18, mask);
        heap.put(entries + 0x20, 1i32);
        heap.put(entries + 0x28, 4i32);
        heap.put(mask, 8u32);
        initialize(Settings::default());
        let base = BASE.swap(TEST_BASE, Ordering::AcqRel);
        let ready = READY.swap(true, Ordering::AcqRel);
        REGISTRY.write().unwrap().clear();
        REJECTED.lock().unwrap().clear();
        NATIVE.lock().unwrap().clear();
        Self {
            _environment: environment,
            _heap: heap,
            character,
            slot: assembly + 0x28,
            mapper,
            inner,
            transform: owner + 0x60,
            input,
            input_models,
            source_models,
            base,
            ready,
        }
    }
    fn draw(&self) -> [[f32; 12]; 4] {
        let mut out = [[0.0; 12]; 4];
        let mut registers: Registers = unsafe { std::mem::zeroed() };
        registers.rcx = self.mapper as u64;
        registers.rdx = out.as_mut_ptr() as u64;
        registers.r8 = 4;
        assert_eq!(
            render_hook(&mut registers, original_render as *const () as usize),
            4
        );
        out
    }
}

impl Drop for NativeTest {
    fn drop(&mut self) {
        attachments::clear();
        dummies::clear();
        REGISTRY.write().unwrap().clear();
        NATIVE.lock().unwrap().clear();
        REJECTED.lock().unwrap().clear();
        BASE.store(self.base, Ordering::Release);
        READY.store(self.ready, Ordering::Release);
    }
}

#[test]
fn actual_weapon_dummy_route_follows_target_hand_and_rejects_a_rebuilt_record() {
    let mut fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    {
        let mut sessions = REGISTRY.write().unwrap();
        Arc::get_mut(&mut sessions[0]).unwrap().motion = Some(MotionProfile {
            leg_ratio: 0.8,
            height_ratio: 0.8,
        });
    }
    let h = &mut fixture._heap;
    let eq = h.alloc(0x180);
    let item = h.alloc(0x800);
    let transform = h.alloc(0xb0);
    let node = h.alloc(0xc0);
    let record = h.alloc(80);
    let provider = h.alloc(0x68);
    let array = h.alloc(0x80);
    let modifier = h.alloc(0x90);
    let animation = h.alloc(0x90);
    let skeleton = h.alloc(0x20);
    let bones = h.alloc(3 * 64);
    let worlds = h.alloc(3 * 48);
    h.put(fixture.slot + 7 * 8, eq);
    h.put(
        eq,
        TEST_BASE + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA,
    );
    h.put(eq + 0x10, item);
    h.put(eq + 0x20, transform);
    h.put(transform, TEST_BASE + 0x2B6ED78);
    let name: Vec<u16> = "WP_TEST".encode_utf16().collect();
    for (i, v) in name.iter().enumerate() {
        h.put(item + 0x6c8 + i * 2, *v);
    }
    h.put(item + 0x6d8, name.len());
    h.put(item + 0x6e0, 7usize);
    h.put(node, TEST_BASE + 0x2B6F878);
    h.put(node + 0x78, record);
    h.put(node + 0x80, 1i32);
    h.put(node + 0xa8, provider);
    h.put(record + 4, 2i32);
    h.put(provider, TEST_BASE + 0x2B70598);
    h.put(array, TEST_BASE + 0x2B6EB88);
    h.put(array + 0x70, worlds);
    h.put(array + 0x68, 3i32);
    h.put(modifier, TEST_BASE + 0x2B708D0);
    h.put(animation, TEST_BASE + 0x2B6F2C8);
    h.put(animation + 0x68, skeleton);
    h.put(skeleton, 3u16);
    h.put(skeleton + 8, bones);
    for (at, value) in [
        (transform + 0x50, node),
        (provider + 0x50, array),
        (array + 0x50, modifier),
        (modifier + 0x70, animation),
    ] {
        let holder = h.alloc(8);
        h.put(holder, value);
        h.put(at, holder);
    }
    for (i, name) in ["L_UpperArm", "L_Forearm", "L_Hand"].iter().enumerate() {
        let text = h.text(name, true);
        h.put(bones + i * 64 + 0x20, text);
        h.put(
            worlds + i * 48,
            pose::affine_output(DMat4::from_translation(DVec3::X * [0.0, 2.0, 3.0][i])).unwrap(),
        );
    }
    attachments::refresh(&REGISTRY.read().unwrap(), TEST_BASE);
    unsafe extern "C" fn original(_: usize, out: usize, _: u32) -> usize {
        let data = DMat4::from_translation(DVec3::X * 4.0)
            .to_cols_array()
            .map(|v| v as f32);
        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), out as *mut f32, 16);
        }
        1
    }
    let mut out = [0.0f32; 16];
    let mut registers: Registers = unsafe { std::mem::zeroed() };
    registers.rcx = node as u64;
    registers.rdx = out.as_mut_ptr() as u64;
    for _ in 0..20 {
        assert_eq!(
            attachments::hook(&mut registers, original as *const () as usize),
            1
        );
        assert!(
            (out[12] - 2.1).abs() < 1e-5,
            "weapon stayed at original hand: {out:?}"
        );
        assert_eq!([out[0], out[5], out[10]], [1.0; 3]);
    }
    fixture._heap.put(record + 4, 0i32);
    attachments::hook(&mut registers, original as *const () as usize);
    assert_eq!(out[12], 4.0);
}

#[test]
fn unchanged_proportions_cache_native_path_until_reference_resource_is_rebuilt() {
    let mut fixture = NativeTest::new();
    let equipment = ptr(fixture.slot).unwrap();
    let item = ptr(equipment + 0x10).unwrap();
    let resource = ptr(item + 0x68).unwrap();
    let inverse = ptr(resource + 0x2F8).unwrap();
    for (i, x) in [0.0, 2.0, 3.0, 3.2].into_iter().enumerate() {
        fixture._heap.put(
            inverse + i * 48,
            pose::affine_output(DMat4::from_translation(DVec3::X * -x)).unwrap(),
        );
    }
    refresh(fixture.character.address, 1.0, 1);
    assert!(REGISTRY.read().unwrap().is_empty());
    assert_eq!(NATIVE.lock().unwrap().len(), 1);
    // Live animation is not reference geometry and must never trigger detection.
    fixture._heap.put(
        fixture.source_models + 48,
        pose::qs_output(LocalPose {
            translation: DVec3::X * 17.0,
            ..LocalPose::IDENTITY
        })
        .unwrap(),
    );
    let cached_source = NATIVE.lock().unwrap()[0].source.clone();
    for frame in 2..602 {
        refresh(fixture.character.address, 1.0, frame);
        assert!(REGISTRY.read().unwrap().is_empty());
        assert_eq!(NATIVE.lock().unwrap().len(), 1);
    }
    assert!(cached_source.current(&read));
    assert!(solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).is_none());
    assert_eq!(fixture.draw(), [[99.0; 12]; 4]);
    // A replacement bind resource is detected without a name/configuration change.
    let replacement = fixture._heap.alloc(4 * 48);
    for (i, x) in [0.0, 0.7, 1.1, 1.3].into_iter().enumerate() {
        fixture._heap.put(
            replacement + i * 48,
            pose::affine_output(DMat4::from_translation(DVec3::X * -x)).unwrap(),
        );
    }
    fixture._heap.put(resource + 0x2F8, replacement);
    refresh(fixture.character.address, 1.0, 602);
    assert!(NATIVE.lock().unwrap().is_empty());
    assert_eq!(REGISTRY.read().unwrap().len(), 1);
    assert!(solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).is_some());
    suspend();
    assert!(NATIVE.lock().unwrap().is_empty());
}

#[test]
fn optional_model_overrides_are_case_insensitive_and_legacy_models_force() {
    let s: Settings =
        toml::from_str("force_models = ['BD_M_CUSTOM']\nexclude_models = ['HD_F_CUSTOM']").unwrap();
    assert!(s.validate().is_ok());
    assert_eq!(s.policy("BD_M_Other"), Some(false));
    assert_eq!(s.policy("bd_m_custom"), Some(true));
    assert_eq!(s.policy("hd_f_custom"), None);
    let legacy: Settings = toml::from_str("models = ['BD_M_CUSTOM']").unwrap();
    assert_eq!(legacy.policy("bd_m_custom"), Some(true));
    let conflicting: Settings =
        toml::from_str("force_models = ['BD_M_CUSTOM']\nexclude_models = ['bd_m_custom']").unwrap();
    assert!(conflicting.validate().is_err());
    let disabled: Settings = toml::from_str("enabled = false").unwrap();
    assert_eq!(disabled.policy("BD_M_CUSTOM"), None);
}

unsafe extern "C" fn original_render(_: usize, out: usize, count: u32, start: u32) -> usize {
    for i in start as usize..(start + count) as usize {
        unsafe {
            (out as *mut [f32; 12]).add(i).write([99.0; 12]);
        }
    }
    count as usize
}

unsafe extern "C" fn original_matrix_render(_: usize, out: usize, count: u32, start: u32) -> usize {
    for i in start as usize..(start + count) as usize {
        unsafe {
            (out as *mut [f32; 16]).add(i).write([99.0; 16]);
        }
    }
    count as usize
}

#[test]
fn full_matrix_consumer_agrees_with_affine_consumer_without_packing_subranges() {
    let fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    let mut input = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
    input.context();
    input.completed();
    let expected = fixture.draw();
    let mut out = [[-17.0f32; 16]; 4];
    let mut r: Registers = unsafe { std::mem::zeroed() };
    r.rcx = fixture.mapper as u64;
    r.rdx = out.as_mut_ptr() as u64;
    r.r8 = 2;
    r.r9 = 1;
    assert_eq!(
        matrix_render_hook(&mut r, original_matrix_render as *const () as usize),
        2
    );
    assert_eq!(out[0], [-17.0; 16]);
    assert_eq!(out[3], [-17.0; 16]);
    for i in 1..3 {
        let matrix = glam::Mat4::from_cols_array(&out[i]).as_dmat4();
        assert_eq!(pose::affine_output(matrix).unwrap(), expected[i]);
        assert_eq!(
            [out[i][3], out[i][7], out[i][11], out[i][15]],
            [0.0, 0.0, 0.0, 1.0]
        );
    }
}

#[test]
fn native_subranges_preserve_other_bones_and_reject_rebuilt_routes() {
    let fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    let mut input = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
    input.context();
    input.completed();
    let mut out = [[-17.0f32; 12]; 4];
    let mut registers: Registers = unsafe { std::mem::zeroed() };
    registers.rcx = fixture.mapper as u64;
    registers.rdx = out.as_mut_ptr() as u64;
    registers.r8 = 1;
    registers.r9 = 2;
    assert_eq!(
        render_hook(&mut registers, original_render as *const () as usize),
        1
    );
    assert_eq!(out[0], [-17.0; 12]);
    assert_eq!(out[1], [-17.0; 12]);
    assert!((out[2][3] - 1.1).abs() < 1e-5);
    assert_eq!(out[3], [-17.0; 12]);
    fixture._heap.put(fixture.mapper + 0x90, 0usize);
    assert_eq!(fixture.draw(), [[99.0; 12]; 4]);
    assert!(solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).is_none());
    refresh(fixture.character.address, 1.0, 2);
    assert!(REGISTRY.read().unwrap().is_empty());
}

#[test]
fn private_solver_buffers_are_reused_nested_calls_isolated_and_inactive_calls_rejected() {
    let fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    assert!(solver_input(fixture.inner, fixture.input + 0x48, fixture.transform + 16).is_none());
    fixture._heap.put(fixture.inner + 0x60, 0u8);
    assert!(solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).is_none());
    fixture._heap.put(fixture.inner + 0x60, 1u8);
    let mut first = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
    let first_context = first.context();
    let mut nested = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
    assert_ne!(first_context, nested.context());
    drop(nested);
    drop(first);
    let mut reused = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
    assert_eq!(first_context, reused.context());
    let session = REGISTRY.read().unwrap()[0].clone();
    assert!(!render_output_is_private(
        &session.key,
        fixture.input_models,
        48
    ));
    assert!(!render_output_is_private(
        &session.key,
        fixture.source_models + 32,
        48
    ));
}

#[test]
fn failed_bindings_back_off_and_retry_without_retaining_an_invalid_session() {
    let fixture = NativeTest::new();
    // Invalid finite reference cannot be accepted or rebuilt every frame.
    let meta = ptr(fixture.character.at(0x398))
        .and_then(|p| ptr(p + 0x48))
        .unwrap();
    let refs = ptr(meta + 0x40).unwrap();
    let original = pose::bytes::<48>(&read, refs).unwrap();
    fixture._heap.put(refs, f32::NAN);
    refresh(fixture.character.address, 1.0, 1);
    assert!(REGISTRY.read().unwrap().is_empty());
    assert_eq!(REJECTED.lock().unwrap().len(), 1);
    fixture._heap.put(refs, original);
    refresh(fixture.character.address, 1.0, 2);
    assert!(REGISTRY.read().unwrap().is_empty());
    refresh(fixture.character.address, 1.0, 121);
    assert_eq!(REGISTRY.read().unwrap().len(), 1);
}

#[test]
fn actual_adapter_routes_owned_solver_pose_then_render_and_revokes_on_unequip() {
    let fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    assert_eq!(
        REGISTRY.read().unwrap().len(),
        1,
        "cold equipment binding failed"
    );
    assert_eq!(
        fixture.draw(),
        [[99.0; 12]; 4],
        "render must wait for cloth input consumption"
    );
    let source_before = pose::bytes::<144>(&read, fixture.source_models).unwrap();
    let input_before = pose::bytes::<192>(&read, fixture.input_models).unwrap();
    let mut private = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform)
        .expect("private retarget input");
    let context = private.context();
    assert_eq!(context % 16, 0);
    for offset in [0x10, 0x20, 0x30] {
        assert_eq!(ptr(context + offset).unwrap(), 4 | (0x8000_0004usize << 32));
    }
    let models = ptr(context + 0x18).unwrap();
    assert_eq!(models % 16, 0);
    assert_ne!(models, fixture.input_models);
    let wrist = pose::qs(&pose::bytes::<48>(&read, models + 2 * 48).unwrap()).unwrap();
    assert!((wrist.translation.x - 1.1).abs() < 1e-5);
    assert_eq!(
        source_before,
        pose::bytes::<144>(&read, fixture.source_models).unwrap()
    );
    assert_eq!(
        input_before,
        pose::bytes::<192>(&read, fixture.input_models).unwrap()
    );
    private.completed();
    // Emulate only native writeback storage, NOT cloth integration: the
    // existing solver has written the hair joint and set its native mask bit.
    fixture._heap.put(
        fixture.input_models + 3 * 48,
        pose::qs_output(LocalPose {
            translation: DVec3::new(1.3, 0.15, 0.0),
            ..LocalPose::IDENTITY
        })
        .unwrap(),
    );
    let out = fixture.draw();
    assert!((out[2][3] - 1.1).abs() < 1e-5);
    assert!((out[3][7] - 0.15).abs() < 1e-5);
    refresh(fixture.character.address, 1.0, 2);
    assert_eq!(
        fixture.draw(),
        [[99.0; 12]; 4],
        "prior-frame cloth completion cannot authorize next frame"
    );
    fixture._heap.put(fixture.slot, 0usize);
    refresh(fixture.character.address, 1.0, 3);
    assert!(REGISTRY.read().unwrap().is_empty());
    assert!(solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).is_none());
    private.completed();
    assert_eq!(fixture.draw(), [[99.0; 12]; 4]);
}

#[test]
fn nonuniform_parent_and_rotated_physics_helper_use_native_lazy_model_cache() {
    let fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    {
        let mut registry = REGISTRY.write().unwrap();
        let session = Arc::get_mut(&mut registry[0]).unwrap();
        session.mesh_bones[2].reference.scale.x = 0.8;
        session.cloth_bones[2].reference.scale.x = 0.8;
        session.cloth_bones[3].name = "physics_only_helper".into();
        session.cloth_bones[3].reference.rotation = glam::DQuat::from_rotation_z(0.4);
        session.mesh_to_cloth[3] = None;
        session.cloth_to_mesh[3] = None;
        session.mesh_plan = Plan::new(
            session.source_bones.clone(),
            session.mesh_bones.clone(),
            &[],
        )
        .unwrap();
        session.cloth_plan = Some(
            Plan::new(
                session.source_bones.clone(),
                session.cloth_bones.clone(),
                &[],
            )
            .unwrap(),
        );
        let work = session.work.get_mut().unwrap();
        work.mesh = session.mesh_plan.new_frame();
        work.cloth = Some(session.cloth_plan.as_ref().unwrap().new_frame());
    }
    let original = pose::bytes::<192>(&read, fixture.input_models).unwrap();
    let mut private = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform)
        .expect("one sheared physics-only helper must not disable the whole armor");
    let context = private.context();
    let flags = ptr(context + 0x28).unwrap();
    assert_eq!(
        pose::bytes::<16>(&read, flags).unwrap(),
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0]
    );
    assert_eq!(pose::bytes::<2>(&read, context + 0x38), Some([1, 0]));
    let locals = ptr(context + 8).unwrap();
    let local = pose::qs(&pose::bytes::<48>(&read, locals + 3 * 48).unwrap()).unwrap();
    assert!(
        local
            .rotation
            .angle_between(glam::DQuat::from_rotation_z(0.4))
            < 1e-6
    );
    assert_eq!(
        pose::bytes::<192>(&read, fixture.input_models).unwrap(),
        original
    );
    private.completed();
    assert_ne!(fixture.draw(), [[99.0; 12]; 4]);
}

#[test]
fn configuration_is_automatic_without_a_retarget_table_and_rejects_ambiguous_names() {
    let example =
        crate::config::Config::parse(include_str!("../examples/ERCharacterScale.retarget.toml"))
            .unwrap();
    assert!(example.retarget.enabled);
    assert!(example.rules.is_empty());
    assert!(Settings::default().enabled);
    assert!(example.retarget.force_models.is_empty());
    assert!(
        crate::config::Config::parse(crate::config::DEFAULT_TEXT)
            .unwrap()
            .retarget
            .enabled
    );
    assert!(
        Settings {
            enabled: true,
            ..Settings::default()
        }
        .validate()
        .is_ok()
    );
    assert!(
        Settings {
            enabled: true,
            force_models: vec!["BD_M_1000".into(), "bd_m_1000".into()],
            ..Settings::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Settings {
            enabled: true,
            force_models: vec!["BD_M_1000".into()],
            ..Settings::default()
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn native_core_member_is_not_an_equipment_owner_backpointer() {
    let fixture = NativeTest::new();
    let core = ptr(fixture.inner + 0x30).unwrap();
    // Live dev.3 evidence: core+18 is a distinct native object, not owner.
    // The validated ownership chain is equipment -> owner -> inner -> core.
    fixture._heap.put(core + 0x18, fixture.mapper);
    refresh(fixture.character.address, 1.0, 1);
    assert_eq!(
        REGISTRY.read().unwrap().len(),
        1,
        "valid native equipment rejected before retarget binding"
    );
    assert!(solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).is_some());
    fixture._heap.put(core, TEST_BASE + 0x1234);
    refresh(fixture.character.address, 1.0, 2);
    assert!(REGISTRY.read().unwrap().is_empty());
}

#[test]
fn simulated_model_pose_does_not_require_a_shear_free_local_reconstruction() {
    let fixture = NativeTest::new();
    refresh(fixture.character.address, 1.0, 1);
    let mut input = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
    input.context();
    input.completed();
    // Actual cloth snapshots have independently scaled/rotated model rows.
    // Their relative matrix can contain shear although both native Qs rows
    // are valid. Rendering consumes model rows, not this relative matrix.
    let parent = LocalPose {
        translation: DVec3::X * 1.1,
        scale: DVec3::new(1.003, 1.0, 1.0),
        ..LocalPose::IDENTITY
    };
    let simulated = LocalPose {
        translation: DVec3::new(1.3, 0.15, 0.0),
        rotation: glam::DQuat::from_rotation_z(0.15),
        scale: DVec3::new(1.001, 0.999, 1.0),
    };
    assert!(pose::local(pose::matrix(parent).inverse() * pose::matrix(simulated)).is_none());
    fixture
        ._heap
        .put(fixture.input_models + 96, pose::qs_output(parent).unwrap());
    fixture._heap.put(
        fixture.input_models + 144,
        pose::qs_output(simulated).unwrap(),
    );
    let output = fixture.draw();
    assert!(
        (output[2][3] - 1.1).abs() < 1e-6,
        "retargeted body was discarded with cloth"
    );
    let expected = pose::affine_output(pose::matrix(simulated)).unwrap();
    for (actual, expected) in output[3].into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-6);
    }
}

#[test]
fn overall_scale_conventions_remain_coherent_across_switches_and_suspend_revokes_inflight() {
    let fixture = NativeTest::new();
    let started = std::time::Instant::now();
    for (generation, scale) in [0.85f32, 1.0, 1.12, 2.0, 1.0].into_iter().enumerate() {
        let s = f64::from(scale);
        // Source convention: canonical animation includes uniform root scale.
        for (i, x) in [0.0, 2.0, 3.0].into_iter().enumerate() {
            fixture._heap.put(
                fixture.source_models + i * 48,
                pose::qs_output(LocalPose {
                    translation: DVec3::X * x * s,
                    scale: DVec3::splat(s),
                    ..LocalPose::IDENTITY
                })
                .unwrap(),
            );
        }
        refresh(fixture.character.address, scale, generation as u64 + 1);
        let before = pose::bytes::<144>(&read, fixture.source_models).unwrap();
        let mut input =
            solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
        let private_model = ptr(input.context() + 0x18).unwrap();
        let wrist = pose::qs(&pose::bytes::<48>(&read, private_model + 96).unwrap()).unwrap();
        assert!((wrist.translation.x - 1.1 * s).abs() < 1e-6);
        assert_eq!(wrist.scale, DVec3::ONE);
        input.completed();
        // Emulate the observed native writeback contract; this is NOT a
        // native solver test, and cannot establish temporal cloth behavior.
        fixture._heap.put(
            fixture.input_models + 3 * 48,
            pose::qs_output(LocalPose {
                translation: DVec3::new(1.3, 0.15, 0.0) * s,
                scale: DVec3::splat(1.0 / s),
                ..LocalPose::IDENTITY
            })
            .unwrap(),
        );
        let output = fixture.draw();
        assert!((output[2][3] - 1.1).abs() < 1e-5);
        assert!((output[3][3] - 1.3).abs() < 1e-5);
        assert!((output[3][7] - 0.15).abs() < 1e-5);
        assert!((output[3][0] - 1.0).abs() < 1e-5);
        assert_eq!(
            before,
            pose::bytes::<144>(&read, fixture.source_models).unwrap()
        );
    }
    let mut pending = solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
    pending.context();
    suspend();
    pending.completed();
    assert!(!pending.session.current());
    assert_eq!(fixture.draw(), [[99.0; 12]; 4]);
    println!(
        "owned_adapter_scale_switches=5 elapsed_ms={:.3}",
        started.elapsed().as_secs_f64() * 1000.0
    );
}

#[test]
fn body_dummy_batches_follow_local_body_without_weapons_and_revoke_stale_records() {
    let mut fixture = NativeTest::new();
    let h = &mut fixture._heap;
    let model = h.alloc(0x80);
    let item = h.alloc(0x680);
    let provider = h.alloc(0x80);
    let array = h.alloc(0x80);
    let modifier = h.alloc(0x80);
    let animation = h.alloc(0x80);
    let skeleton = h.alloc(0x20);
    let bones = h.alloc(4 * 64);
    let worlds = h.alloc(4 * 48);
    let table = h.alloc(0x20);
    let records = h.alloc(4 * 80);
    let wrapper = h.alloc(0x10);
    unsafe {
        (fixture.character.address as *mut u8)
            .add(0x50)
            .cast::<usize>()
            .write_unaligned(model);
    }
    h.put(model, TEST_BASE + 0x2B35C68);
    h.put(model + 0x10, item);
    h.put(item + 0x650, provider);
    h.put(provider, TEST_BASE + 0x2B70598);
    h.put(provider + 0x68, table);
    h.put(table, 4i32);
    h.put(table + 8, records);
    h.put(array, TEST_BASE + 0x2B6EB88);
    h.put(array + 0x68, 4i32);
    h.put(array + 0x70, worlds);
    h.put(modifier, TEST_BASE + 0x2B708D0);
    h.put(animation, TEST_BASE + 0x2B6F2C8);
    h.put(animation + 0x68, skeleton);
    h.put(skeleton, 4u16);
    h.put(skeleton + 8, bones);
    h.put(wrapper, TEST_BASE + 0x2B753E0);
    h.put(wrapper + 8, array);
    for (at, value) in [
        (provider + 0x50, array),
        (array + 0x50, modifier),
        (modifier + 0x70, animation),
    ] {
        let holder = h.alloc(8);
        h.put(holder, value);
        h.put(at, holder);
    }
    for (i, name) in [
        "L_UpperArm",
        "L_Forearm",
        "L_Hand",
        "IndependentNativePoint",
    ]
    .iter()
    .enumerate()
    {
        let text = h.text(name, true);
        h.put(bones + i * 64 + 0x20, text);
        h.put(
            worlds + i * 48,
            pose::affine_output(DMat4::from_translation(DVec3::X * [0.0, 2.0, 3.0, 99.0][i]))
                .unwrap(),
        );
        h.put(
            records + i * 80,
            [if i == 0 { 199i32 } else { 200 }, i as i32, -1, 0],
        );
        h.put(
            records + i * 80 + 16,
            DMat4::IDENTITY.to_cols_array().map(|v| v as f32),
        );
    }
    h.put(bones + 3 * 64 + 0x2c, -1i16);
    refresh(fixture.character.address, 1.0, 1);
    Arc::get_mut(&mut REGISTRY.write().unwrap()[0])
        .unwrap()
        .motion = Some(MotionProfile {
        leg_ratio: 0.8,
        height_ratio: 0.8,
    });
    dummies::refresh(&REGISTRY.read().unwrap(), TEST_BASE);
    unsafe extern "C" fn original(_: usize, out: usize, record: usize) -> usize {
        let index = unsafe { ((record + 4) as *const i32).read_unaligned() } as usize;
        let m = DMat4::from_translation(DVec3::X * [0.0, 2.0, 3.0, 99.0][index])
            .to_cols_array()
            .map(|v| v as f32);
        unsafe {
            std::ptr::copy_nonoverlapping(m.as_ptr(), out as *mut f32, 16);
        }
        1
    }
    let mut out = [0f32; 16];
    let mut r: Registers = unsafe { std::mem::zeroed() };
    r.rcx = wrapper as u64;
    r.rdx = out.as_mut_ptr() as u64;
    let native_queries = crate::memory_query::query_count();
    r.r8 = (records + 3 * 80) as u64;
    for _ in 0..120 {
        dummies::hook(&mut r, original as *const () as usize);
        assert_eq!(out[12], 99.0, "independent native point must stay native");
    }
    let extra_queries = crate::memory_query::query_count() - native_queries;
    assert_eq!(
        extra_queries, 0,
        "unmapped dummy repeatedly validates unrelated pose storage"
    );
    for _ in 0..20 {
        for (i, x) in [0.0, 0.7, 1.1].into_iter().enumerate() {
            r.r8 = (records + i * 80) as u64;
            dummies::hook(&mut r, original as *const () as usize);
            assert!(
                (out[12] - x).abs() < 1e-5,
                "body dummy stayed on original skeleton: {out:?}"
            );
        }
    }
    let started = std::time::Instant::now();
    // B47060 also selects effect/prop dummies. An unregistered consumer must
    // not suppress the nested generic correction.
    unsafe extern "C" fn effect_selector(node: usize, out: usize, _: u32) -> usize {
        let args = unsafe { &*(node as *const [usize; 2]) };
        let mut nested: Registers = unsafe { std::mem::zeroed() };
        nested.rcx = args[0] as u64;
        nested.rdx = out as u64;
        nested.r8 = args[1] as u64;
        dummies::hook(&mut nested, original as *const () as usize)
    }
    let selector = [wrapper, records + 160];
    let mut outer: Registers = unsafe { std::mem::zeroed() };
    outer.rcx = selector.as_ptr() as u64;
    outer.rdx = out.as_mut_ptr() as u64;
    attachments::hook(&mut outer, effect_selector as *const () as usize);
    assert!(
        (out[12] - 1.1).abs() < 1e-5,
        "unregistered effect/prop selector suppresses correction: {}",
        out[12]
    );
    outer.r8 = 1;
    attachments::hook(&mut outer, effect_selector as *const () as usize);
    assert!((out[12] - 1.1).abs() < 1e-5);
    // Register another selector on this same body provider as an equipped
    // weapon. Its nested query and outer correction must apply exactly once.
    let eq = h.alloc(0x180);
    let weapon_item = h.alloc(0x800);
    let transform = h.alloc(0xb0);
    let node = h.alloc(0xc0);
    let holder = h.alloc(8);
    h.put(fixture.slot + 7 * 8, eq);
    h.put(
        eq,
        TEST_BASE + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA,
    );
    h.put(eq + 0x10, weapon_item);
    h.put(eq + 0x20, transform);
    for (i, v) in "WP_TEST".encode_utf16().enumerate() {
        h.put(weapon_item + 0x6c8 + i * 2, v);
    }
    h.put(weapon_item + 0x6d8, 7usize);
    h.put(weapon_item + 0x6e0, 7usize);
    h.put(transform, TEST_BASE + 0x2B6ED78);
    h.put(transform + 0x50, holder);
    h.put(holder, node);
    h.put(node, TEST_BASE + 0x2B6F878);
    h.put(node + 0x78, records + 160);
    h.put(node + 0x80, 1i32);
    h.put(node + 0x90, wrapper);
    h.put(node + 0xa8, provider);
    attachments::refresh(&REGISTRY.read().unwrap(), TEST_BASE);
    unsafe extern "C" fn weapon_selector(node: usize, out: usize, _: u32) -> usize {
        let args = [ptr(node + 0x90).unwrap(), ptr(node + 0x78).unwrap()];
        unsafe { effect_selector(args.as_ptr() as usize, out, 0) }
    }
    outer.rcx = node as u64;
    outer.r8 = 0;
    attachments::hook(&mut outer, weapon_selector as *const () as usize);
    assert!((out[12] - 1.1).abs() < 1e-5, "weapon corrected twice");
    let queries = crate::memory_query::query_count();
    for _ in 0..600 {
        for i in 0..34 {
            r.r8 = (records + (i % 3) * 80) as u64;
            dummies::hook(&mut r, original as *const () as usize);
        }
    }
    println!(
        "dummy_callback_frames=600 records_per_frame=34 elapsed_ms={:.3} os_queries={}",
        started.elapsed().as_secs_f64() * 1000.0,
        crate::memory_query::query_count() - queries
    );
    unsafe extern "C" fn affine_original(_: usize, out: usize, record: usize) -> usize {
        let index = unsafe { ((record + 4) as *const i32).read_unaligned() } as usize;
        let matrix =
            pose::affine_output(DMat4::from_translation(DVec3::X * [0.0, 2.0, 3.0][index]))
                .unwrap();
        unsafe {
            std::ptr::copy_nonoverlapping(matrix.as_ptr(), out as *mut f32, 12);
        }
        1
    }
    let mut affine = [12345.0f32; 14];
    r.rdx = unsafe { affine.as_mut_ptr().add(1) } as u64;
    for (i, x) in [0.0, 0.7, 1.1].into_iter().enumerate() {
        r.r8 = (records + i * 80) as u64;
        dummies::affine_hook(&mut r, affine_original as *const () as usize);
        assert!(
            (affine[4] - x).abs() < 1e-5,
            "affine body dummy still uses old skeleton"
        );
        assert_eq!(affine[0], 12345.0);
        assert_eq!(affine[13], 12345.0);
    }
    r.rdx = out.as_mut_ptr() as u64;
    // The outer weapon selector owns its palm calibration; nested generic
    // queries must stay native, otherwise its delta would be applied twice.
    {
        let _weapon = dummies::WeaponScope::enter();
        r.r8 = (records + 160) as u64;
        dummies::hook(&mut r, original as *const () as usize);
        assert_eq!(out[12], 3.0);
    }
    r.r8 = (records + 160) as u64;
    dummies::hook(&mut r, original as *const () as usize);
    assert!((out[12] - 1.1).abs() < 1e-5);
    h.put(wrapper + 8, 0usize);
    r.r8 = (records + 160) as u64;
    dummies::hook(&mut r, original as *const () as usize);
    assert_eq!(out[12], 3.0);
    h.put(wrapper + 8, array);
    h.put(records + 160, 201i32);
    dummies::hook(&mut r, original as *const () as usize);
    assert_eq!(out[12], 3.0);
    dummies::clear();
}

#[test]
fn unscaled_animation_at_small_overall_scale_does_not_lift_equipment_root() {
    let fixture = NativeTest::new();
    for (generation, scale) in [0.3f32, 0.55, 0.85, 2.0, 1.0].into_iter().enumerate() {
        for (i, x) in [0.0, 2.0, 3.0].into_iter().enumerate() {
            fixture._heap.put(
                fixture.source_models + i * 48,
                pose::qs_output(LocalPose {
                    translation: DVec3::new(x, 0.2, 0.0),
                    ..LocalPose::IDENTITY
                })
                .unwrap(),
            );
        }
        refresh(fixture.character.address, scale, generation as u64 + 1);
        let mut private =
            solver_input(fixture.inner, fixture.input + 0x48, fixture.transform).unwrap();
        let models = ptr(private.context() + 0x18).unwrap();
        let root = pose::qs(&pose::bytes::<48>(&read, models).unwrap()).unwrap();
        assert!(
            (root.translation.y - 0.2 * f64::from(scale)).abs() < 1e-6,
            "requested={scale}: unscaled native root was lifted to {}",
            root.translation.y
        );
        let wrist = pose::qs(&pose::bytes::<48>(&read, models + 96).unwrap()).unwrap();
        assert!((wrist.translation.x - 1.1 * f64::from(scale)).abs() < 1e-6);
        private.completed();
        let output = fixture.draw();
        assert!((output[0][7] - 0.2).abs() < 1e-6);
    }
}
