//! Rust-owned address values, never references to complete native C++ structures.
//! Used only inside the retained pre-physics callback with coherent reciprocal owners.
//! Readable pages do not establish lifetime, exclusive access, or gameplay-role authority.
use shikifsrs_elden_ring::storage_facts::NativeStorageField as Field;

pub(crate) const fn offset(field: Field) -> usize {
    field.location().offset as usize
}

#[derive(Clone, Copy)]
pub(crate) struct CharacterAccess {
    pub address: usize,
}
impl CharacterAccess {
    pub fn at(address: usize) -> Self {
        Self { address }
    }
    fn pointer(&self, base: usize, field: Field) -> Option<usize> {
        crate::unit_runtime::read(base.checked_add(offset(field))?).filter(|v| *v != 0)
    }
    pub fn controller_address(&self) -> usize {
        self.pointer(self.address, Field::CharacterController)
            .unwrap_or(0)
    }
    pub fn physics_address(&self) -> usize {
        self.pointer(self.address, Field::CharacterModules)
            .and_then(|p| self.pointer(p, Field::ModulesPhysics))
            .unwrap_or(0)
    }
    pub fn controller_storage(&self) -> usize {
        self.pointer(self.controller_address(), Field::ControllerStorage)
            .unwrap_or(0)
    }
    pub fn ragdoll_address(&self) -> usize {
        self.pointer(self.controller_address(), Field::ControllerRagdoll)
            .unwrap_or(0)
    }
    pub fn scales(&self) -> Option<[f32; 3]> {
        let base = self.controller_address();
        Some([
            read_float(base, Field::ControllerScaleX)?,
            read_float(base, Field::ControllerScaleY)?,
            read_float(base, Field::ControllerScaleZ)?,
        ])
    }
    pub fn physics(&self) -> Option<crate::PhysicsBaseline> {
        let base = self.physics_address();
        Some(crate::PhysicsBaseline {
            chr_hit_height: read_float(base, Field::PhysicsCharacterHeight)?,
            chr_hit_radius: read_float(base, Field::PhysicsCharacterRadius)?,
            hit_height: read_float(base, Field::PhysicsMapHeight)?,
            hit_radius: read_float(base, Field::PhysicsMapRadius)?,
            weight: read_float(base, Field::PhysicsWeight)?,
        })
    }
    pub fn write_scales(&self, values: [f32; 3]) -> bool {
        store_floats(
            self.controller_address(),
            &[
                (Field::ControllerScaleX, values[0]),
                (Field::ControllerScaleY, values[1]),
                (Field::ControllerScaleZ, values[2]),
            ],
        )
    }
    pub fn write_physics(&self, values: crate::PhysicsBaseline) -> bool {
        store_floats(
            self.physics_address(),
            &[
                (Field::PhysicsCharacterHeight, values.chr_hit_height),
                (Field::PhysicsCharacterRadius, values.chr_hit_radius),
                (Field::PhysicsMapHeight, values.hit_height),
                (Field::PhysicsMapRadius, values.hit_radius),
                (Field::PhysicsWeight, values.weight),
            ],
        )
    }
}
fn read_float(base: usize, field: Field) -> Option<f32> {
    if base == 0 {
        return None;
    }
    crate::unit_runtime::read(base.checked_add(offset(field))?)
}
fn store_floats(base: usize, values: &[(Field, f32)]) -> bool {
    if base == 0
        || !values.iter().all(|(field, value)| {
            value.is_finite()
                && base
                    .checked_add(offset(*field))
                    .is_some_and(|p| crate::memory_query::accessible_span(p, 4, true))
        })
    {
        return false;
    }
    for (field, value) in values {
        // SAFETY: The retained pre-physics owner supplies exclusive coherent graph lifetime;
        // all destinations were checked before the first store. No native references exist.
        unsafe {
            ((base + offset(*field)) as *mut f32).write_unaligned(*value);
        }
    }
    true
}

#[derive(Clone, Copy)]
pub(crate) struct WorldAccess {
    pub address: usize,
}
impl WorldAccess {
    pub fn primary_player(&self) -> Option<usize> {
        crate::unit_runtime::read(
            self.address
                .checked_add(offset(Field::WorldPrimaryPlayer))?,
        )
        .filter(|v| *v != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body_scale_port::test_characters::{Character, Environment};
    #[test]
    fn scale_baselines_restore_native_fields_without_compounding_or_touching_other_storage() {
        let _environment = Environment::new();
        let actor = Character::new(9520, 77, 1.5);
        let mut access = CharacterAccess::at(actor.address);
        let original = access.physics().unwrap();
        let mut state = crate::ScaleState::default();
        crate::apply_visual_scale(&mut access, &mut state, 2.0);
        crate::apply_visual_scale(&mut access, &mut state, 2.0);
        assert_eq!(access.scales().unwrap(), [3.0; 3]);
        crate::apply_physics_scale(&mut access, &mut state, 2.0);
        assert_eq!(
            access.physics().unwrap().hit_radius,
            original.hit_radius * 2.0
        );
        assert_eq!(access.physics().unwrap().weight, original.weight);
        crate::apply_visual_scale(&mut access, &mut state, 1.0);
        crate::apply_physics_scale(&mut access, &mut state, 1.0);
        assert_eq!(access.scales().unwrap(), [1.5; 3]);
        assert_eq!(access.physics().unwrap().hit_radius, original.hit_radius);
        assert_eq!(actor.identity().entity_id, 77);
    }
    #[test]
    fn bad_last_value_rejects_whole_store_before_any_native_write() {
        let _environment = Environment::new();
        let actor = Character::new(9520, 77, 1.5);
        let access = CharacterAccess::at(actor.address);
        assert!(!access.write_scales([7.0, 8.0, f32::NAN]));
        assert_eq!(access.scales().unwrap(), [1.5; 3]);
        let mut physics = access.physics().unwrap();
        physics.chr_hit_height = 999.0;
        physics.weight = f32::INFINITY;
        let before = access.physics().unwrap().chr_hit_height;
        assert!(!access.write_physics(physics));
        assert_eq!(access.physics().unwrap().chr_hit_height, before);
        assert!(!CharacterAccess::at(0).write_scales([1.0; 3]));
    }
}
