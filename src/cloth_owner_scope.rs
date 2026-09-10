//! ER 2.7 equipment ownership, not pose-input equality or spatial proximity.
//! Native 9EAA40 walks [assembly+28, assembly+100), stride 8; 9EA590
//! destroys and clears those 27 slots. 9EA1AF stores the model back-reference.
//! Equipment 9F1448 loads model+130 before calling the cloth input setter.

pub const ASSEMBLY_VTABLE_RVA: usize = 0x2B35980;
pub const EQUIPMENT_VTABLE_RVA: usize = 0x2B36718;
// WW2.7.1.0 RTTI: EnemyIns@CS and CSChrModelIns@CS. The latter's
// +0x130 cloth member is cleared by native 9F1C60 and used by 9F1448.
pub const ENEMY_VTABLE_RVA: usize = 0x2A47090;
pub const CHARACTER_MODEL_VTABLE_RVA: usize = 0x2B35C68;
pub const MODEL_SLOT_COUNT: usize = 27;
const OWNER_VTABLE_RVA: usize = 0x2B92A60;
const INPUT_VTABLE_RVA: usize = 0x2B70360;
const INNER_VTABLE_RVA: usize = 0x329A2F8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Route {
    pub slot_address: usize,
    pub model: usize,
    pub owner: usize,
    pub input: usize,
    pub inner: usize,
    pub core: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    pub player: usize,
    pub player_model: usize,
    pub assembly: usize,
    pub anchor: usize,
    pub routes: [Route; MODEL_SLOT_COUNT],
}

impl Scope {
    pub fn capture(
        player: usize,
        anchor: usize,
        base: usize,
        read: impl Fn(usize) -> Option<usize>,
    ) -> Self {
        let mut result = Self::default();
        if player == 0 || anchor == 0 || base == 0 {
            return result;
        }
        if read(player) == Some(base + ENEMY_VTABLE_RVA) {
            return Self::capture_character(player, anchor, base, read);
        }
        let assembly = read(player + 0x648).unwrap_or(0);
        let model = read(player + 0x50).unwrap_or(0);
        if assembly == 0
            || model == 0
            || read(assembly) != Some(base + ASSEMBLY_VTABLE_RVA)
            || read(assembly + 8) != Some(model)
        {
            return result;
        }
        result.player = player;
        result.player_model = model;
        result.assembly = assembly;
        result.anchor = anchor;
        for index in 0..MODEL_SLOT_COUNT {
            let slot_address = assembly + 0x28 + index * 8;
            let model = read(slot_address).unwrap_or(0);
            if model == 0 || read(model) != Some(base + EQUIPMENT_VTABLE_RVA) {
                continue;
            }
            let owner = read(model + 0x130).unwrap_or(0);
            if owner == 0 || read(owner) != Some(base + OWNER_VTABLE_RVA) {
                continue;
            }
            let input = read(owner + 0x120).unwrap_or(0);
            let inner = read(owner + 0x40).unwrap_or(0);
            let core = read(inner + 0x30).unwrap_or(0);
            if input == 0
                || inner == 0
                || core == 0
                || read(input) != Some(base + INPUT_VTABLE_RVA)
                || read(inner) != Some(base + INNER_VTABLE_RVA)
            {
                continue;
            }
            let route = Route {
                slot_address,
                model,
                owner,
                input,
                inner,
                core,
            };
            if result.route_is_current(route, base, &read)
                && !result.routes.iter().any(|previous| previous.owner == owner)
            {
                result.routes[index] = route;
            }
        }
        if !result.root_is_current(base, &read) {
            return Self::default();
        }
        result
    }

    pub fn find(&self, owner: usize, input: usize) -> Option<Route> {
        self.routes
            .iter()
            .copied()
            .find(|route| owner != 0 && input != 0 && route.owner == owner && route.input == input)
    }

    fn capture_character(
        character: usize,
        anchor: usize,
        base: usize,
        read: impl Fn(usize) -> Option<usize>,
    ) -> Self {
        let model = read(character + 0x50).unwrap_or(0);
        if model == 0 || read(model) != Some(base + CHARACTER_MODEL_VTABLE_RVA) {
            return Self::default();
        }
        let mut scope = Self {
            player: character,
            player_model: model,
            anchor,
            ..Self::default()
        };
        let owner = read(model + 0x130).unwrap_or(0);
        if owner == 0 {
            return scope;
        } // A character may legitimately have no cloth.
        let input = read(owner + 0x120).unwrap_or(0);
        let inner = read(owner + 0x40).unwrap_or(0);
        let core = if inner != 0 {
            read(inner + 0x30).unwrap_or(0)
        } else {
            0
        };
        let route = Route {
            slot_address: character + 0x50,
            model,
            owner,
            input,
            inner,
            core,
        };
        if input != 0 && inner != 0 && core != 0 && scope.route_is_current(route, base, &read) {
            scope.routes[0] = route;
        }
        scope
    }

    pub fn route_is_current(
        &self,
        route: Route,
        base: usize,
        read: impl Fn(usize) -> Option<usize>,
    ) -> bool {
        route.owner != 0
            && self.root_is_current(base, &read)
            && read(route.slot_address) == Some(route.model)
            && read(route.model)
                == Some(
                    base + if self.assembly == 0 {
                        CHARACTER_MODEL_VTABLE_RVA
                    } else {
                        EQUIPMENT_VTABLE_RVA
                    },
                )
            && read(route.model + 0x130) == Some(route.owner)
            && read(route.owner) == Some(base + OWNER_VTABLE_RVA)
            && read(route.owner + 0x120) == Some(route.input)
            && read(route.owner + 0x40) == Some(route.inner)
            && read(route.input) == Some(base + INPUT_VTABLE_RVA)
            && read(route.inner) == Some(base + INNER_VTABLE_RVA)
            && read(route.inner + 0x30) == Some(route.core)
    }

    fn root_is_current(&self, base: usize, read: impl Fn(usize) -> Option<usize>) -> bool {
        if self.assembly == 0 {
            return self.player != 0
                && self.player_model != 0
                && read(self.player) == Some(base + ENEMY_VTABLE_RVA)
                && read(self.player + 0x50) == Some(self.player_model)
                && read(self.player_model) == Some(base + CHARACTER_MODEL_VTABLE_RVA);
        }
        self.player != 0
            && self.assembly != 0
            && self.player_model != 0
            && read(self.player + 0x648) == Some(self.assembly)
            && read(self.player + 0x50) == Some(self.player_model)
            && read(self.assembly) == Some(base + ASSEMBLY_VTABLE_RVA)
            && read(self.assembly + 8) == Some(self.player_model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    #[test]
    fn non_player_model_route_never_reads_player_equipment_tail_and_rejects_replacement() {
        let (base, chr, model, owner, input, inner, core, anchor) = (
            0x100000, 0x20000, 0x30000, 0x40000, 0x50000, 0x60000, 0x70000, 0x80000,
        );
        let mut memory = HashMap::from([
            (chr, base + ENEMY_VTABLE_RVA),
            (chr + 0x50, model),
            (model, base + CHARACTER_MODEL_VTABLE_RVA),
            (model + 0x130, owner),
            (owner, base + OWNER_VTABLE_RVA),
            (owner + 0x120, input),
            (owner + 0x40, inner),
            (input, base + INPUT_VTABLE_RVA),
            (inner, base + INNER_VTABLE_RVA),
            (inner + 0x30, core),
        ]);
        let read = |address| {
            assert_ne!(address, chr + 0x648, "enemy must not use PlayerIns tail");
            memory.get(&address).copied()
        };
        let scope = Scope::capture(chr, anchor, base, read);
        let route = scope.find(owner, input).unwrap();
        assert_eq!(scope.assembly, 0);
        assert!(scope.route_is_current(route, base, |a| memory.get(&a).copied()));
        memory.insert(chr + 0x50, model + 0x1000);
        assert!(!scope.route_is_current(route, base, |a| memory.get(&a).copied()));
    }
}
