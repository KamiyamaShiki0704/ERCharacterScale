//! Owned heaps only. No game process and no native function execution.
use super::*;
use eldenring::cs::{CSChrPhysicsModule, ChrCtrl, ChrIns, ChrInsModuleContainer};
use std::mem::{offset_of, size_of};
use std::sync::Arc;

pub(crate) const BASE: usize = 0x140000000;
pub(crate) struct Environment {
    _lock: std::sync::MutexGuard<'static, ()>,
    base: usize,
    ready: bool,
}
impl Environment {
    pub fn new() -> Self {
        let lock = MODULE_TEST_LOCK.lock().unwrap();
        Self {
            _lock: lock,
            base: MODULE_BASE.swap(BASE, Ordering::AcqRel),
            ready: HOOKS_READY.swap(true, Ordering::AcqRel),
        }
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        MODULE_BASE.store(self.base, Ordering::Release);
        HOOKS_READY.store(self.ready, Ordering::Release);
    }
}

pub(crate) struct Character {
    _heap: Vec<u128>,
    pub address: usize,
}
impl Character {
    pub fn new(character_id: u32, entity: u32, native_scale: f32) -> Self {
        Self::in_heap(character_id, entity, native_scale, 0x10000)
    }
    pub fn in_heap(character_id: u32, entity: u32, native_scale: f32, bytes: usize) -> Self {
        assert!(
            size_of::<ChrIns>() < 0x2000
                && size_of::<ChrCtrl>() < 0x2000
                && size_of::<CSChrPhysicsModule>() < 0x2000
        );
        assert!(bytes >= 0x10000 && bytes.is_multiple_of(16));
        let mut heap = vec![0u128; bytes / 16];
        for index in (0..heap.len()).step_by(4096 / 16) {
            unsafe { heap.as_mut_ptr().add(index).write_volatile(0) };
        }
        let result = Self {
            address: heap.as_ptr() as usize,
            _heap: heap,
        };
        result.put(0, BASE + crate::cloth_owner_scope::ENEMY_VTABLE_RVA);
        result.put(
            offset_of!(ChrIns, field_ins_handle),
            entity as u64 + 0x100000000,
        );
        result.put(offset_of!(ChrIns, character_id), character_id);
        result.put(offset_of!(ChrIns, npc_param_id), 1234i32);
        result.put(offset_of!(ChrIns, event_entity_id), entity);
        result.put(offset_of!(ChrIns, chr_type), 5i32);
        result.put(offset_of!(ChrIns, chr_set_entry), result.at(0xA000));
        result.put(0xA000, result.address);
        result.put(0xA008, 4u8);
        result.put(0xA009, 4u8);
        result.put(offset_of!(ChrIns, chr_ctrl), result.at(0x2000));
        result.put(offset_of!(ChrIns, modules), result.at(0x8000));
        result.put(offset_of!(ChrIns, special_effect), result.at(0x9000));
        result.put(0x2000 + offset_of!(ChrCtrl, owner), result.address);
        result.put(
            0x8000 + offset_of!(ChrInsModuleContainer, physics),
            result.at(0x6000),
        );
        result.put(
            0x6000 + offset_of!(CSChrPhysicsModule, owner),
            result.address,
        );
        result.put(0x9010, result.address);
        for offset in [
            offset_of!(ChrCtrl, scale_size_x),
            offset_of!(ChrCtrl, scale_size_y),
            offset_of!(ChrCtrl, scale_size_z),
        ] {
            result.put(0x2000 + offset, native_scale);
        }
        for offset in [
            offset_of!(CSChrPhysicsModule, chr_hit_height),
            offset_of!(CSChrPhysicsModule, chr_hit_radius),
            offset_of!(CSChrPhysicsModule, hit_height),
            offset_of!(CSChrPhysicsModule, hit_radius),
            offset_of!(CSChrPhysicsModule, weight),
        ] {
            result.put(0x6000 + offset, 4.0f32);
        }
        result.put(offset_of!(ChrIns, chr_model_ins), result.at(0xD000));
        result.put(
            0xD000,
            BASE + crate::cloth_owner_scope::CHARACTER_MODEL_VTABLE_RVA,
        );
        result.put(0x398, result.at(0xB000));
        result.put(0x3B0, result.at(0xC100));
        result.put(0xC100, BASE + ER_ANIM_SKELETON_VTABLE_RVA);
        result.put(0xB000, result.at(0xC000));
        result.put(0xC018, BASE + ER_POSE_IMPORTER_UPDATE_RVA);
        result.put(0xB048, result.at(0xB100));
        result.put(0xB060, result.at(0xB200));
        result.put(0xB080, 1u8);
        result.put(0xB138, 1i32);
        result.put(0xB200, [2.0f32, 4.0, 6.0, 0.0]);
        result.put(0xB220, [1.0f32, 1.0, 1.0, 0.0]);
        result
    }
    pub fn at(&self, offset: usize) -> usize {
        self.address + offset
    }
    pub fn put<T: Copy>(&self, offset: usize, value: T) {
        assert!(offset + size_of::<T>() <= 0x10000);
        unsafe {
            (self.at(offset) as *mut T).write_unaligned(value);
        }
    }
    pub fn get<T: Copy>(&self, offset: usize) -> T {
        crate::unit_runtime::read(self.at(offset)).unwrap()
    }
    pub fn identity(&self) -> crate::unit_runtime::Identity {
        crate::unit_runtime::Identity::capture(self.address).unwrap()
    }
    pub fn attach_cloth(&self, sim: usize) {
        self.put(0xD130, self.at(0xD200));
        self.put(0xD200, BASE + ER_CLOTH_MODEL_VTABLE_RVA);
        self.put(0xD320, self.at(0xD400));
        self.put(0xD240, self.at(0xD600));
        self.put(0xD400, BASE + ER_POSE_IMPORTER_VTABLE_RVA);
        self.put(0xD600, BASE + ER_CLOTH_INNER_VTABLE_RVA);
        self.put(0xD630, self.at(0xD800));
        self.put(0xD828, self.at(0xDA00));
        self.put(0xD830, self.at(0xDA08));
        self.put(0xDA00, self.at(0xDA20));
        self.put(0xDA20, self.at(0xDB00));
        self.put(0xDB40, self.at(0xDC00));
        self.put(0xDB48, 1i32);
        self.put(0xDC00, self.at(0xDD00));
        self.put(0xDD18, sim);
    }
}

unsafe extern "C" fn cached_pose(this: usize) -> usize {
    this
}

#[test]
fn non_c0000_units_keep_native_baselines_and_dispatch_separate_pose_outputs() {
    let _environment = Environment::new();
    let chars = [
        Character::new(2010, 100, 2.0),
        Character::new(2010, 101, 1.0),
        Character::new(3250, 102, 1.25),
    ];
    let states: [Arc<UnitState>; 3] = std::array::from_fn(|_| Arc::new(UnitState::default()));
    let mut runtime_states: [crate::ScaleState; 3] = std::array::from_fn(|_| Default::default());
    let scales = [0.5f32, 1.5, 3.0];
    for i in 0..3 {
        assert!(chars[i].identity().character_id != 0);
        register_unit(&states[i]);
        with_unit_state(&states[i], || {
            assert_eq!(
                crate::apply_character_scale(
                    unsafe { &mut *(chars[i].address as *mut ChrIns) },
                    &mut runtime_states[i],
                    scales[i]
                ),
                scales[i]
            );
        });
    }
    refresh_unit_registry();
    for _ in 0..600 {
        for i in 0..3 {
            with_unit_state(&states[i], || {
                crate::apply_character_scale(
                    unsafe { &mut *(chars[i].address as *mut ChrIns) },
                    &mut runtime_states[i],
                    scales[i],
                );
            });
            let mut registers: Registers = unsafe { std::mem::zeroed() };
            registers.rcx = chars[i].at(0xB000) as u64;
            assert_eq!(
                units::pose(&mut registers, cached_pose as *const () as usize),
                chars[i].at(0xB000)
            );
            assert_eq!(chars[i].get::<f32>(0xB200), 2.0 * scales[i]);
            assert_eq!(
                chars[i].get::<f32>(0x2000 + offset_of!(ChrCtrl, scale_size_x)),
                [2.0, 1.0, 1.25][i] * scales[i]
            );
            assert_eq!(
                chars[i].get::<f32>(0x6000 + offset_of!(CSChrPhysicsModule, hit_height)),
                4.0 * scales[i]
            );
        }
    }
    // Suspending callbacks must preserve the ratio for this materialized pose.
    detach_unit(&states[0]);
    register_unit(&states[0]);
    refresh_unit_registry();
    let mut registers: Registers = unsafe { std::mem::zeroed() };
    registers.rcx = chars[0].at(0xB000) as u64;
    units::pose(&mut registers, cached_pose as *const () as usize);
    assert_eq!(chars[0].get::<f32>(0xB200), 1.0);
    for i in 0..3 {
        with_unit_state(&states[i], || {
            crate::apply_character_scale(
                unsafe { &mut *(chars[i].address as *mut ChrIns) },
                &mut runtime_states[i],
                1.0,
            );
            restore_unit_pose();
        });
        assert_eq!(chars[i].get::<f32>(0xB200), 2.0);
        assert_eq!(
            chars[i].get::<f32>(0x6000 + offset_of!(CSChrPhysicsModule, hit_height)),
            4.0
        );
        unregister_unit(&states[i]);
    }
}

#[test]
fn non_c0000_cloth_reaches_the_253_rotation_fix_and_rejects_recycled_identity() {
    let _environment = Environment::new();
    let chr = Character::new(3250, 1, 1.0);
    chr.attach_cloth(chr.at(0xE000));
    chr.put(0xE000, BASE + 0x2D8B6F8);
    chr.put(0xDD00, BASE + 0x2D872A0);
    chr.put(0xDD00 + 0x268, chr.at(0xDB00));
    chr.put(0xDD00 + 0x168, chr.at(0xE300));
    chr.put(0xDD00 + 0x170, 1i32);
    chr.put(0xE300, chr.at(0xE400));
    chr.put(0xE400, BASE + 0x2D8B758);
    let state = Arc::new(UnitState::default());
    state.set_identity(chr.identity());
    register_unit(&state);
    with_unit_state(&state, || {
        assert!(bind_local_player(chr.address, 0.5).ready);
        refresh_owned_cloth_inputs(chr.address, chr.at(0xB000));
    });
    let slot = (0..CLOTH_INSTANCE_SLOTS)
        .find(|&slot| state.cloth_instance_owners[slot].load(Ordering::Acquire) == chr.at(0xD200))
        .unwrap();
    state.cloth_instance_cores[slot].store(chr.at(0xD800), Ordering::Release);
    state.cloth_instance_applied_scale_bits[slot].store(0.5f32.to_bits(), Ordering::Release);
    state.cloth_instance_pending_scale_bits[slot]
        .store(NO_PENDING_CLOTH_SCALE_BITS, Ordering::Release);
    refresh_unit_registry();
    with_collider_unit(chr.at(0xDD00), || {
        assert_eq!(current_scale(), 0.5);
        crate::cloth_collider_rotation_hook::exercise_owned_route(
            BASE,
            chr.at(0xDD00),
            chr.at(0xE000),
            chr.at(0xE400),
        );
    });
    chr.put(offset_of!(ChrIns, field_ins_handle), 999u64);
    assert_eq!(with_collider_unit(chr.at(0xDD00), current_scale), 1.0);
    unregister_unit(&state);
}

#[test]
fn configured_small_and_large_scales_reach_body_collision_and_cached_pose() {
    let _environment = Environment::new();
    for (model, target, kind) in [
        (0, "player", crate::config::TargetKind::Player),
        (8250, "enemy", crate::config::TargetKind::Enemy),
    ] {
        let actor = Character::new(model, model + 1, 2.0);
        let hooks = Arc::new(UnitState::default());
        hooks.set_identity(actor.identity());
        register_unit(&hooks);
        let mut state = crate::ScaleState::default();
        for scale in [0.1f32, 10.0, 0.25, 4.0, 0.49, 3.01, 1.0] {
            let config = crate::config::Config::parse(&format!("version=1\nenabled=true\n[player]\nenabled=true\n[enemies]\nenabled=true\n[[rules]]\nname='wide'\ntarget='{target}'\ncharacter_ids=[{model}]\nmode='constant'\nscale={scale:e}")).unwrap();
            let selected = actor
                .identity()
                .resolve(kind, &config, || panic!("model rule queries no team"))
                .unwrap();
            assert_eq!(selected.scale, scale);
            with_unit_state(&hooks, || {
                assert_eq!(
                    crate::apply_character_scale(
                        unsafe { &mut *(actor.address as *mut ChrIns) },
                        &mut state,
                        selected.scale
                    ),
                    scale
                );
            });
            refresh_unit_registry();
            for _ in 0..3 {
                let mut r: Registers = unsafe { std::mem::zeroed() };
                r.rcx = actor.at(0xB000) as u64;
                units::pose(&mut r, cached_pose as *const () as usize);
                let actual: f32 = actor.get(0xB200);
                assert!(
                    (actual / (2.0 * scale) - 1.0).abs() < 0.0001,
                    "model={model} scale={scale} actual={actual}"
                );
            }
            assert_eq!(
                actor.get::<f32>(0x2000 + offset_of!(ChrCtrl, scale_size_x)),
                2.0 * scale
            );
            assert_eq!(
                actor.get::<f32>(0x6000 + offset_of!(CSChrPhysicsModule, hit_height)),
                4.0 * scale
            );
            assert_eq!(
                actor.get::<f32>(0x6000 + offset_of!(CSChrPhysicsModule, weight)),
                4.0
            );
        }
        // Overflow is rejected before any non-neutral target is published.
        with_unit_state(&hooks, || {
            assert_eq!(
                crate::apply_character_scale(
                    unsafe { &mut *(actor.address as *mut ChrIns) },
                    &mut state,
                    f32::MAX
                ),
                1.0
            );
            assert_eq!(current_scale(), 1.0);
        });
        assert_eq!(
            actor.get::<f32>(0x2000 + offset_of!(ChrCtrl, scale_size_x)),
            2.0
        );
        assert_eq!(
            actor.get::<f32>(0x6000 + offset_of!(CSChrPhysicsModule, hit_height)),
            4.0
        );
        unregister_unit(&hooks);
    }
}
