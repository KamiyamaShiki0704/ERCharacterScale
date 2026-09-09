//! ER 2.7 equipment ownership, not pose-input equality or spatial proximity.
//! Native 9EAA40 walks [assembly+28, assembly+100), stride 8; 9EA590
//! destroys and clears those 27 slots. 9EA1AF stores the model back-reference.
//! Equipment 9F1448 loads model+130 before calling the cloth input setter.

pub const ASSEMBLY_VTABLE_RVA: usize = 0x2B35980;
pub const EQUIPMENT_VTABLE_RVA: usize = 0x2B36718;
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

    pub fn route_is_current(
        &self,
        route: Route,
        base: usize,
        read: impl Fn(usize) -> Option<usize>,
    ) -> bool {
        route.owner != 0
            && self.root_is_current(base, &read)
            && read(route.slot_address) == Some(route.model)
            && read(route.model) == Some(base + EQUIPMENT_VTABLE_RVA)
            && read(route.model + 0x130) == Some(route.owner)
            && read(route.owner) == Some(base + OWNER_VTABLE_RVA)
            && read(route.owner + 0x120) == Some(route.input)
            && read(route.owner + 0x40) == Some(route.inner)
            && read(route.input) == Some(base + INPUT_VTABLE_RVA)
            && read(route.inner) == Some(base + INNER_VTABLE_RVA)
            && read(route.inner + 0x30) == Some(route.core)
    }

    fn root_is_current(&self, base: usize, read: impl Fn(usize) -> Option<usize>) -> bool {
        self.player != 0
            && self.assembly != 0
            && self.player_model != 0
            && read(self.player + 0x648) == Some(self.assembly)
            && read(self.player + 0x50) == Some(self.player_model)
            && read(self.assembly) == Some(base + ASSEMBLY_VTABLE_RVA)
            && read(self.assembly + 8) == Some(self.player_model)
    }
}
