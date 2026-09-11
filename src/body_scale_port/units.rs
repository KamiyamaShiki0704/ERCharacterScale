//! Route native invocations to independently retained character states.
//! Address matches select a candidate only; existing use-time ownership checks
//! still authorize every write. Shared/ambiguous keys always select neutrality.
use super::*;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

#[derive(Default)]
struct Registry {
    states: Vec<Arc<UnitState>>,
    keys: HashMap<usize, Option<Arc<UnitState>>>,
}
static REGISTRY: std::sync::LazyLock<RwLock<Registry>> =
    std::sync::LazyLock::new(|| RwLock::new(Registry::default()));
static NEUTRAL: OnceLock<Arc<UnitState>> = OnceLock::new();

pub(crate) fn register_unit(state: &Arc<UnitState>) {
    if let Ok(mut registry) = REGISTRY.write()
        && !registry.states.iter().any(|old| Arc::ptr_eq(old, state))
    {
        registry.states.push(state.clone());
    }
}

pub(crate) fn unregister_unit(state: &Arc<UnitState>) {
    with_unit_state(state, clear_target);
    detach_unit(state);
}

pub(crate) fn detach_unit(state: &Arc<UnitState>) {
    if let Ok(mut registry) = REGISTRY.write() {
        registry.states.retain(|old| !Arc::ptr_eq(old, state));
        // Remove all its cached identities before an address can be reused.
        registry
            .keys
            .retain(|_, old| old.as_ref().is_some_and(|old| !Arc::ptr_eq(old, state)));
    }
}

fn insert_key(
    keys: &mut HashMap<usize, Option<Arc<UnitState>>>,
    key: usize,
    state: &Arc<UnitState>,
) {
    if key == 0 {
        return;
    }
    keys.entry(key)
        .and_modify(|old| {
            if old.as_ref().is_some_and(|old| !Arc::ptr_eq(old, state)) {
                *old = None;
            }
        })
        .or_insert_with(|| Some(state.clone()));
}

/// One publication per pre-physics pass, after all units have refreshed their
/// identities. Only pointer tables are read; particle/vertex arrays are not.
pub(crate) fn refresh_unit_registry() {
    let states = match REGISTRY.read() {
        Ok(registry) => registry.states.clone(),
        Err(_) => return,
    };
    let mut keys = HashMap::new();
    crate::memory_query::scoped(|| {
        for state in &states {
            for key in [
                state.target_pose_importer.load(Ordering::Acquire),
                state.target_cloth_pose_importer.load(Ordering::Acquire),
                state.target_anim_skeleton.load(Ordering::Acquire),
            ] {
                insert_key(&mut keys, key, state);
            }
            let Ok(scope) = state.target_cloth_scope.try_read() else {
                continue;
            };
            for route in scope
                .routes
                .iter()
                .copied()
                .filter(|route| route.owner != 0)
            {
                if !scope.route_is_current(route, MODULE_BASE.load(Ordering::Acquire), read_usize) {
                    continue;
                }
                for key in [route.owner, route.input, route.inner, route.core] {
                    insert_key(&mut keys, key, state);
                }
                if let Some((groups, count)) =
                    bounded_vector_span(route.core, 0x28, 0x30, 8, MAX_CLOTH_GROUPS)
                {
                    for i in 0..count {
                        let root = read_usize(groups + i * 8).and_then(read_usize).unwrap_or(0);
                        if root == 0 {
                            continue;
                        }
                        insert_key(&mut keys, root, state);
                        if let Some((buffers, count)) =
                            bounded_vector_span(root, 0x20, 0x28, 8, 128)
                        {
                            for b in 0..count {
                                insert_key(
                                    &mut keys,
                                    read_usize(buffers + b * 8).unwrap_or(0),
                                    state,
                                );
                            }
                        }
                    }
                }
            }
            // Exact-selected-input owners can exist without an equipment route.
            for i in 0..CLOTH_INSTANCE_SLOTS {
                let owner = state.cloth_instance_owners[i].load(Ordering::Acquire);
                let input = state.cloth_instance_inputs[i].load(Ordering::Acquire);
                if owner != 0
                    && input != 0
                    && input == state.target_cloth_pose_importer.load(Ordering::Acquire)
                    && read_usize(owner + 0x120) == Some(input)
                {
                    insert_key(&mut keys, owner, state);
                    insert_key(&mut keys, read_usize(owner + 0x40).unwrap_or(0), state);
                }
            }
        }
    });
    if let Ok(mut registry) = REGISTRY.write() {
        registry.keys = keys;
    }
}

fn selected(keys: &[usize]) -> Arc<UnitState> {
    let neutral = || {
        NEUTRAL
            .get_or_init(|| Arc::new(UnitState::default()))
            .clone()
    };
    let Ok(registry) = REGISTRY.try_read() else {
        return neutral();
    };
    let mut result: Option<Arc<UnitState>> = None;
    for key in keys.iter().filter(|key| **key != 0) {
        if let Some(candidate) = registry.keys.get(key) {
            let Some(candidate) = candidate else {
                return neutral();
            };
            if result
                .as_ref()
                .is_some_and(|previous| !Arc::ptr_eq(previous, candidate))
            {
                return neutral();
            }
            result = Some(candidate.clone());
        }
    }
    result
        .filter(|state| state.identity_current())
        .unwrap_or_else(neutral)
}

fn with_keys<T>(keys: &[usize], run: impl FnOnce() -> T) -> T {
    with_unit_state(&selected(keys), run)
}

pub(crate) fn with_collider_unit<T>(child: usize, run: impl FnOnce() -> T) -> T {
    let root = child.checked_add(0x268).and_then(read_usize).unwrap_or(0);
    with_keys(&[root], run)
}

macro_rules! this_hook {
    ($name:ident, $hook:ident) => {
        pub(super) fn $name(r: *mut Registers, original: usize) -> usize {
            with_keys(&[unsafe { (*r).rcx as usize }], || $hook(r, original))
        }
    };
}
this_hook!(pose, pose_importer_update_hook);
this_hook!(secondary, cloth_secondary_reference_submit_hook);
this_hook!(commit, cloth_inner_commit_hook);
this_hook!(affine, anim_skeleton_get_affine_hook);
this_hook!(matrix, anim_skeleton_get_hook);
this_hook!(range, anim_skeleton_get_range_hook);

pub(super) fn setter(r: *mut Registers, original: usize) -> usize {
    let args = unsafe { &*r };
    with_keys(&[args.rcx as usize, args.rdx as usize], || {
        cloth_input_setter_hook(r, original)
    })
}
pub(super) fn render(r: *mut Registers, original: usize) -> usize {
    let this = unsafe { (*r).rcx as usize };
    let input = this
        .checked_add(0x70)
        .and_then(read_usize)
        .and_then(read_usize)
        .unwrap_or(0);
    with_keys(&[input], || cloth_render_range_hook(r, original))
}
pub(super) fn skin(r: *mut Registers, original: usize) -> usize {
    let context = unsafe { (*r).rdx as usize };
    let root = context.checked_add(0x10).and_then(read_usize).unwrap_or(0);
    with_keys(&[root], || cloth_skin_normal_hook(r, original))
}
pub(super) fn mesh_pn(r: *mut Registers, original: usize) -> usize {
    let input = unsafe { (*r).r8 as usize };
    with_keys(&[input], || cloth_mesh_pn_hook(r, original))
}
pub(super) fn mesh_frame(r: *mut Registers) {
    let input = unsafe { (*r).rdi as usize };
    with_keys(&[input], || cloth_mesh_frame_hook(r))
}
pub(super) fn affine_item(r: *mut Registers) {
    // Disabled legacy hook: retaining the current invocation is safe because
    // its own exact target checks still reject unrelated providers.
    anim_skeleton_get_affine_range_item_commit_hook(r);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pose_dispatch_in_large_resident_heap_avoids_region_walk_per_callback() {
        use super::super::test_characters::{Character, Environment};
        let _environment = Environment::new();
        let actor = Character::in_heap(9520, 1, 1.0, 128 * 1024 * 1024);
        let state = Arc::new(UnitState::default());
        state.set_identity(actor.identity());
        with_unit_state(&state, || {
            assert!(bind_local_player(actor.address, 0.85).ready)
        });
        register_unit(&state);
        refresh_unit_registry();
        unsafe extern "C" fn cached_native(this: usize) -> usize {
            this
        }
        let mut registers: Registers = unsafe { std::mem::zeroed() };
        registers.rcx = actor.at(0xB000) as u64;
        let clock = std::time::Instant::now();
        for _ in 0..128 {
            assert!(crate::memory_query::accessible_region(actor.address, 8, false).is_some());
        }
        let region_time = clock.elapsed();
        let clock = std::time::Instant::now();
        for _ in 0..128 {
            assert_eq!(
                pose(&mut registers, cached_native as *const () as usize),
                actor.at(0xB000)
            );
        }
        let callback_time = clock.elapsed();
        println!(
            "POSE-DISPATCH callbacks=128 region_query_us={} complete_callback_us={}",
            region_time.as_micros(),
            callback_time.as_micros()
        );
        let applied = actor.get::<[f32; 4]>(0xB200);
        assert!((applied[0] - 1.7).abs() < 0.0001);
        // Every invocation still checks live identity, including address reuse.
        actor.put(8, 0x1234u64);
        assert_eq!(with_keys(&[actor.at(0xB000)], current_scale), 1.0);
        detach_unit(&state);
        // The full unoptimized Rust dispatcher has debug bounds/borrow checks;
        // enforce the tighter performance margin on the shipped release path.
        let margin = if cfg!(debug_assertions) { 1 } else { 4 };
        assert!(
            callback_time * margin < region_time,
            "complete dispatcher still scans the surrounding resident heap"
        );
    }
    #[test]
    fn nested_and_unwinding_contexts_restore_their_parent() {
        let a = Arc::new(UnitState::default());
        let b = Arc::new(UnitState::default());
        a.target_scale_bits
            .store(0.5f32.to_bits(), Ordering::Release);
        b.target_scale_bits
            .store(1.5f32.to_bits(), Ordering::Release);
        let before = current_unit_state();
        with_unit_state(&a, || {
            assert_eq!(current_scale(), 0.5);
            let _ = std::panic::catch_unwind(|| {
                with_unit_state(&b, || {
                    assert_eq!(current_scale(), 1.5);
                    panic!("unwind");
                })
            });
            assert_eq!(current_scale(), 0.5);
        });
        assert!(Arc::ptr_eq(&before, &current_unit_state()));
    }
    #[test]
    fn parallel_threads_and_cloth_slots_do_not_share_scale_or_pending_state() {
        let mut threads = Vec::new();
        for scale in [0.5f32, 1.5, 0.75, 3.0] {
            threads.push(std::thread::spawn(move || {
                let state = Arc::new(UnitState::default());
                with_unit_state(&state, || {
                    state
                        .target_scale_bits
                        .store(scale.to_bits(), Ordering::Release);
                    state.cloth_instance_applied_scale_bits[0]
                        .store(scale.to_bits(), Ordering::Release);
                    for _ in 0..1000 {
                        std::thread::yield_now();
                        assert_eq!(current_scale(), scale);
                    }
                    assert_eq!(
                        state.cloth_instance_applied_scale_bits[0].load(Ordering::Acquire),
                        scale.to_bits()
                    );
                });
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
    }
    #[test]
    fn ambiguous_keys_are_neutral_and_removing_a_unit_does_not_clear_another() {
        let _lock = MODULE_TEST_LOCK.lock().unwrap();
        let a = Arc::new(UnitState::default());
        let b = Arc::new(UnitState::default());
        a.target_pose_importer.store(101, Ordering::Release);
        b.target_pose_importer.store(102, Ordering::Release);
        a.target_scale_bits
            .store(0.5f32.to_bits(), Ordering::Release);
        b.target_scale_bits
            .store(1.5f32.to_bits(), Ordering::Release);
        register_unit(&a);
        register_unit(&b);
        refresh_unit_registry();
        assert_eq!(with_keys(&[101], current_scale), 0.5);
        assert_eq!(with_keys(&[102], current_scale), 1.5);
        assert_eq!(with_keys(&[101, 102], current_scale), 1.0);
        assert_eq!(with_keys(&[999], current_scale), 1.0);
        b.target_cloth_pose_importer.store(101, Ordering::Release);
        refresh_unit_registry();
        assert_eq!(with_keys(&[101], current_scale), 1.0);
        unregister_unit(&a);
        refresh_unit_registry();
        assert_eq!(with_keys(&[102], current_scale), 1.5);
        unregister_unit(&b);
    }
}
