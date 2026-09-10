//! Loaded character identity, bounded enumeration, and native enemy relations.
use crate::{body_scale_port, cloth_owner_scope, config, memory_query};
use eldenring::cs::{
    CSChrPhysicsModule, ChrCtrl, ChrIns, ChrInsModuleContainer, ChrSet, ChrSetEntry, WorldChrMan,
};
use std::{
    collections::{HashMap, HashSet},
    mem::{offset_of, size_of},
    sync::Arc,
};

// WW2.7.1.0: native ChrIns checks at 3F0716/3FE742 compare entry+8
// against 4; the activation path at 3F8C56 writes 4. The pinned fsrs
// ChrLoadStatus labels do not match this executable. entry+9 is not a
// reliable network-role discriminator (loaded local actors also contain 4).
const ACTIVE_ENTRY_STATE: u8 = 4;

pub(crate) fn entry_state_supported(base: usize) -> bool {
    ENTRY_STATE_CHECKS.iter().all(|(rva, expected)| {
        let Some(address) = base.checked_add(*rva) else {
            return false;
        };
        memory_query::accessible_region(address, expected.len(), false).is_some()
            && unsafe { std::slice::from_raw_parts(address as *const u8, expected.len()) }
                == *expected
    })
}

pub(crate) enum TickStatus {
    MainPlayerUnavailable,
    PlayerIdentityRejected,
    Ready,
}

impl TickStatus {
    pub fn stage(self) -> &'static str {
        match self {
            Self::MainPlayerUnavailable => "main-player-unavailable",
            Self::PlayerIdentityRejected => "player-identity-rejected",
            Self::Ready => "local-player-ready",
        }
    }
}

pub(crate) fn read<T: Copy>(address: usize) -> Option<T> {
    memory_query::accessible_region(address, size_of::<T>(), false)?;
    Some(unsafe { (address as *const T).read_unaligned() })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Identity {
    pub address: usize,
    pub role: i32,
    pub vtable: usize,
    pub handle: u64,
    pub entry: usize,
    pub model: usize,
    pub control: usize,
    pub physics: usize,
    pub effects: usize,
    pub pose: usize,
    pub alternate_pose: usize,
    pub cloth_pose: usize,
    pub skeleton: usize,
    pub character_id: u32,
    pub npc_param_id: i32,
    pub entity_id: u32,
}

impl Identity {
    /// Called only from the engine's pre-physics phase. Permission checks are
    /// not lifetime locks; the engine phase and reciprocal owners are required.
    pub fn capture(address: usize) -> Option<Self> {
        Self::capture_checked(address, true)
    }
    fn capture_checked(address: usize, active: bool) -> Option<Self> {
        memory_query::accessible_region(address, size_of::<ChrIns>(), true)?;
        let entry = read::<usize>(address + offset_of!(ChrIns, chr_set_entry))?;
        if read::<usize>(entry)? != address
            || (active && read::<u8>(entry + 8)? != ACTIVE_ENTRY_STATE)
        {
            return None;
        }
        let control = read::<usize>(address + offset_of!(ChrIns, chr_ctrl))?;
        let modules = read::<usize>(address + offset_of!(ChrIns, modules))?;
        memory_query::accessible_region(modules, size_of::<ChrInsModuleContainer>(), false)?;
        let physics = read::<usize>(modules + offset_of!(ChrInsModuleContainer, physics))?;
        let effects = read::<usize>(address + offset_of!(ChrIns, special_effect))?;
        memory_query::accessible_region(control, size_of::<ChrCtrl>(), true)?;
        memory_query::accessible_region(physics, size_of::<CSChrPhysicsModule>(), true)?;
        memory_query::accessible_region(effects, 0x18, false)?;
        if read::<usize>(control + offset_of!(ChrCtrl, owner))? != address
            || read::<usize>(physics + offset_of!(CSChrPhysicsModule, owner))? != address
            || read::<usize>(effects + 0x10)? != address
        {
            return None;
        }
        memory_query::accessible_region(control, size_of::<ChrCtrl>(), true)?;
        memory_query::accessible_region(physics, size_of::<CSChrPhysicsModule>(), true)?;
        let model = read::<usize>(address + offset_of!(ChrIns, chr_model_ins))?;
        if model == 0 {
            return None;
        }
        for offset in [
            offset_of!(ChrCtrl, scale_size_x),
            offset_of!(ChrCtrl, scale_size_y),
            offset_of!(ChrCtrl, scale_size_z),
        ] {
            let value = read::<f32>(control + offset)?;
            if !(value * 3.0).is_finite() || value <= 0.0 {
                return None;
            }
        }
        for offset in [
            offset_of!(CSChrPhysicsModule, chr_hit_height),
            offset_of!(CSChrPhysicsModule, chr_hit_radius),
            offset_of!(CSChrPhysicsModule, hit_height),
            offset_of!(CSChrPhysicsModule, hit_radius),
            offset_of!(CSChrPhysicsModule, weight),
        ] {
            let value = read::<f32>(physics + offset)?;
            if !(value * 3.0).is_finite() || value < 0.0 {
                return None;
            }
        }
        Some(Self {
            address,
            role: read(address + offset_of!(ChrIns, chr_type))?,
            vtable: read(address)?,
            handle: read(address + offset_of!(ChrIns, field_ins_handle))?,
            entry,
            model,
            control,
            physics,
            effects,
            pose: read(address + 0x398)?,
            alternate_pose: read(address + 0x3A0)?,
            cloth_pose: read(address + 0x3A8)?,
            skeleton: read(address + 0x3B0)?,
            character_id: read(address + offset_of!(ChrIns, character_id))?,
            npc_param_id: read(address + offset_of!(ChrIns, npc_param_id))?,
            entity_id: read(address + offset_of!(ChrIns, event_entity_id))?,
        })
    }
    pub fn current(self) -> bool {
        memory_query::scoped(|| Self::capture(self.address) == Some(self))
    }
    fn instance_marker_current(self) -> bool {
        memory_query::scoped(|| {
            memory_query::accessible_region(self.address, size_of::<ChrIns>(), false).is_some()
                && read::<u64>(self.address + offset_of!(ChrIns, field_ins_handle))
                    == Some(self.handle)
                && read::<usize>(self.address + offset_of!(ChrIns, chr_set_entry))
                    == Some(self.entry)
                && read::<usize>(self.entry) == Some(self.address)
                && read::<u32>(self.address + offset_of!(ChrIns, character_id))
                    == Some(self.character_id)
                && read::<u32>(self.address + offset_of!(ChrIns, event_entity_id))
                    == Some(self.entity_id)
        })
    }
    pub(crate) fn same_instance(self, other: Self) -> bool {
        self.address == other.address
            && self.handle == other.handle
            && self.entry == other.entry
            && self.character_id == other.character_id
            && self.npc_param_id == other.npc_param_id
            && self.entity_id == other.entity_id
    }
    pub fn facts(self, kind: config::TargetKind) -> config::UnitFacts {
        config::UnitFacts {
            kind,
            character_id: self.character_id,
            npc_param_id: self.npc_param_id,
            entity_id: self.entity_id,
        }
    }
    pub fn resolve(
        self,
        kind: config::TargetKind,
        config: &config::Config,
        is_hostile: impl FnMut() -> Option<bool>,
    ) -> Result<config::Selection, &'static str> {
        let facts = self.facts(kind);
        let mut effects: Option<Result<HashSet<i32>, &'static str>> = None;
        let selected = config.resolve_with_relation(
            facts,
            |id| {
                effects
                    .get_or_insert_with(|| self.effect_ids())
                    .as_ref()
                    .is_ok_and(|effects| effects.contains(&id))
            },
            is_hostile,
        );
        if let Some(Err(reason)) = effects {
            return Err(reason);
        }
        Ok(selected)
    }
    fn effect_ids(self) -> Result<HashSet<i32>, &'static str> {
        let mut result = HashSet::new();
        let mut seen = HashSet::new();
        let mut node = read::<usize>(self.effects + 8).ok_or("effect-list-unreadable")?;
        while node != 0 {
            if !seen.insert(node) || seen.len() > 2048 {
                return Err("effect-list-cycle-or-capacity");
            }
            memory_query::accessible_region(node, 0x38, false).ok_or("effect-entry-unreadable")?;
            result.insert(read(node + 8).ok_or("effect-entry-unreadable")?);
            node = read(node + 0x30).ok_or("effect-link-unreadable")?;
        }
        Ok(result)
    }
}

fn append_set(set: usize, output: &mut Vec<usize>, seen: &mut HashSet<usize>) -> Option<()> {
    append_set_mode(set, output, seen, false)
}
fn append_set_mode(
    set: usize,
    output: &mut Vec<usize>,
    seen: &mut HashSet<usize>,
    include_remote: bool,
) -> Option<()> {
    memory_query::accessible_region(set, size_of::<ChrSet<ChrIns>>(), false)?;
    let capacity = read::<u32>(set + offset_of!(ChrSet<ChrIns>, capacity))? as usize;
    let entries = read::<usize>(set + offset_of!(ChrSet<ChrIns>, entries))?;
    if capacity > 8192 {
        return None;
    }
    if capacity == 0 {
        return Some(());
    }
    memory_query::accessible_region(
        entries,
        capacity.checked_mul(size_of::<ChrSetEntry<ChrIns>>())?,
        false,
    )?;
    for i in 0..capacity {
        let slot = entries + i * size_of::<ChrSetEntry<ChrIns>>();
        if read::<u8>(slot + 8) != Some(ACTIVE_ENTRY_STATE) {
            continue;
        }
        let address = read::<usize>(slot)?;
        if address != 0
            && !seen.contains(&address)
            && memory_query::accessible_region(address, size_of::<ChrIns>(), false).is_some()
            && read::<usize>(address + offset_of!(ChrIns, chr_set_entry)) == Some(slot)
        {
            // Filter using ChrIns::chr_type, not an unrelated entry byte.
            // EnemyApi::kind additionally validates class/vtable before writes.
            if !include_remote
                && !matches!(
                    read::<i32>(address + offset_of!(ChrIns, chr_type)),
                    Some(0 | 5 | 6 | 7 | 19 | 20 | 21)
                )
            {
                continue;
            }
            seen.insert(address);
            output.push(address);
        }
    }
    Some(())
}

/// Non-c0000 map characters live in EnemyIns collections, not the player set.
/// c0000 NPC opponents are considered separately and retain PlayerIns layout.
pub(crate) fn candidates(world: &WorldChrMan, local_player: usize) -> Vec<usize> {
    let base = world as *const WorldChrMan as usize;
    candidates_at(base, local_player)
}

fn candidates_at(base: usize, local_player: usize) -> Vec<usize> {
    let mut excluded = Vec::new();
    let mut seen = HashSet::new();
    let _ = append_set_mode(
        base + offset_of!(WorldChrMan, ghost_chr_set),
        &mut excluded,
        &mut seen,
        true,
    );
    seen.insert(local_player);
    let mut result = Vec::new();
    for set in [
        base + offset_of!(WorldChrMan, open_field_chr_set),
        base + offset_of!(WorldChrMan, player_chr_set),
        base + offset_of!(WorldChrMan, debug_chr_set),
        base + offset_of!(WorldChrMan, summon_buddy_chr_set),
    ] {
        let _ = append_set(set, &mut result, &mut seen);
    }
    for i in 0..196 {
        if let Some(set) =
            read::<usize>(base + offset_of!(WorldChrMan, chr_sets) + i * 8).filter(|p| *p != 0)
        {
            let _ = append_set(set, &mut result, &mut seen);
        }
    }
    result
}

fn all_consumers(world: &WorldChrMan) -> Vec<usize> {
    let base = world as *const WorldChrMan as usize;
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for offset in [
        offset_of!(WorldChrMan, ghost_chr_set),
        offset_of!(WorldChrMan, summon_buddy_chr_set),
        offset_of!(WorldChrMan, open_field_chr_set),
        offset_of!(WorldChrMan, player_chr_set),
        offset_of!(WorldChrMan, debug_chr_set),
    ] {
        let _ = append_set_mode(base + offset, &mut result, &mut seen, true);
    }
    for i in 0..196 {
        if let Some(set) =
            read::<usize>(base + offset_of!(WorldChrMan, chr_sets) + i * 8).filter(|p| *p != 0)
        {
            let _ = append_set_mode(set, &mut result, &mut seen, true);
        }
    }
    result
}

pub(crate) struct EnemyApi {
    base: usize,
}
impl EnemyApi {
    pub fn validate(base: usize) -> Option<Self> {
        if !NATIVE_CHECKS.iter().all(|(rva, bytes)| {
            memory_query::accessible_region(base + *rva, bytes.len(), false).is_some()
                && unsafe { std::slice::from_raw_parts((base + *rva) as *const u8, bytes.len()) }
                    == *bytes
        }) {
            return None;
        }
        Some(Self { base })
    }
    pub fn kind(&self, address: usize) -> Option<bool> {
        // true = EnemyIns (including local debug/spirit units), false = local NPC
        // PlayerIns. Network players/phantoms and replay classes are rejected.
        memory_query::accessible_region(address, size_of::<ChrIns>(), false)?;
        let vtable = read::<usize>(address)?;
        let role = read::<i32>(address + offset_of!(ChrIns, chr_type))?;
        let model = read::<u32>(address + offset_of!(ChrIns, character_id))?;
        if model == 8000 {
            return None;
        } // Torrent is never an enemy scaling target.
        if vtable == self.base + cloth_owner_scope::ENEMY_VTABLE_RVA && [5, 6, 7].contains(&role) {
            Some(true)
        } else if vtable == self.base + PLAYER_VTABLE_RVA && [5, 19, 20, 21].contains(&role) {
            Some(false)
        } else {
            None
        }
    }
    fn effective_team(&self, identity: Identity) -> Option<u8> {
        if !identity.current() {
            return None;
        }
        let mut team = 255u8;
        let native: unsafe extern "C" fn(usize, *mut u8) -> usize =
            unsafe { std::mem::transmute(self.base + 0x3F1C90) };
        let result = unsafe { native(identity.address, &mut team) };
        (result == (&team as *const u8 as usize) && team < 79).then_some(team)
    }
    pub fn hostile(&self, subject: Identity, player: Identity) -> Option<bool> {
        let subject_team = self.effective_team(subject)?;
        let player_team = self.effective_team(player)?;
        let object = read::<usize>(
            self.base + 0x3B283F0 + (subject_team as usize * 79 + player_team as usize) * 8,
        )?;
        let vtable = read::<usize>(object)?;
        let method = read::<usize>(vtable)?;
        // Native 51B5D0 queries this live matrix with relation flags [1,0,0].
        // 51B590 = friend, 51B580 = neutral, 51B5A0 = enemy, 51B5B0 = rival.
        match (
            vtable.checked_sub(self.base)?,
            method.checked_sub(self.base)?,
        ) {
            (0x2A4F878, 0x51B590) | (0x2A4F868, 0x51B580) => Some(false),
            (0x2A4F888, 0x51B5A0) | (0x2A4F898, 0x51B5B0) => Some(true),
            _ => None,
        }
    }
}

pub(crate) struct UnitRecord {
    pub identity: Identity,
    pub state: crate::ScaleState,
    pub hooks: Arc<body_scale_port::UnitState>,
    pub seen: u64,
}
impl UnitRecord {
    pub fn new(identity: Identity, frame: u64) -> Self {
        let hooks = Arc::new(body_scale_port::UnitState::default());
        hooks.set_identity(identity);
        body_scale_port::register_unit(&hooks);
        Self {
            identity,
            state: crate::ScaleState::default(),
            hooks,
            seen: frame,
        }
    }
}

pub(crate) struct Runtime {
    config: config::Config,
    api: Option<EnemyApi>,
    records: HashMap<usize, UnitRecord>,
    rejected: HashMap<usize, &'static str>,
    cloth_pool: crate::cloth_local_scale::shared::Pool,
}

impl Runtime {
    pub fn new(config: config::Config, base: usize) -> Self {
        let api = if config.enemies.enabled {
            EnemyApi::validate(base)
        } else {
            None
        };
        if config.enemies.enabled {
            crate::log::line(format_args!(
                "[ERCS-ENEMIES] native_routes_ready={} includes_non_c0000=true",
                api.is_some()
            ));
        }
        Self {
            config,
            api,
            records: HashMap::new(),
            rejected: HashMap::new(),
            cloth_pool: Default::default(),
        }
    }

    fn rejected(&mut self, address: usize, reason: &'static str) {
        if self.rejected.insert(address, reason) != Some(reason) {
            crate::log::line(format_args!(
                "[ERCS-UNIT] chr=0x{address:X} rejected={reason}"
            ));
        }
    }

    pub fn tick(&mut self, world: &WorldChrMan, frame: u64) -> TickStatus {
        let Some(pointer) = world.main_player.as_ref() else {
            self.suspend();
            return TickStatus::MainPlayerUnavailable;
        };
        let player_address = pointer.as_ptr() as usize;
        let Some(player) = memory_query::scoped(|| Identity::capture(player_address)) else {
            self.rejected(player_address, "main-player-identity-not-ready");
            self.suspend();
            return TickStatus::PlayerIdentityRejected;
        };
        let mut addresses = vec![player_address];
        if self.api.is_some() {
            addresses.extend(memory_query::scoped(|| candidates(world, player_address)));
        }
        let mut pending = Vec::new();
        let mut consumers = HashMap::new();
        if self.api.is_some() {
            // Include even excluded consumers in read-only shared resource checks.
            // Selected local units replace baseline with their own requested scale.
            for address in memory_query::scoped(|| all_consumers(world)) {
                if let Some(identity) = Identity::capture(address) {
                    consumers.insert(
                        address,
                        crate::cloth_local_scale::shared::Consumer {
                            identity,
                            scale: 1.0,
                        },
                    );
                }
            }
        }
        consumers.insert(
            player_address,
            crate::cloth_local_scale::shared::Consumer {
                identity: player,
                scale: 1.0,
            },
        );
        for address in addresses {
            let is_player = address == player_address;
            if is_player && !self.config.player.enabled {
                continue;
            }
            let enemy_layout = if is_player {
                false
            } else {
                let Some(layout) = self.api.as_ref().and_then(|api| api.kind(address)) else {
                    continue;
                };
                layout
            };
            let Some(identity) = memory_query::scoped(|| Identity::capture(address)) else {
                self.rejected(address, "identity-not-active-or-inconsistent");
                continue;
            };
            if !enemy_layout
                && memory_query::accessible_region(
                    address,
                    size_of::<eldenring::cs::PlayerIns>(),
                    true,
                )
                .is_none()
            {
                self.rejected(address, "player-layout-span-unavailable");
                continue;
            }
            let kind = if is_player {
                config::TargetKind::Player
            } else {
                config::TargetKind::Enemy
            };
            let mut relation_unavailable = false;
            let result = identity.resolve(kind, &self.config, || {
                let relation = self
                    .api
                    .as_ref()
                    .and_then(|api| api.hostile(identity, player));
                relation_unavailable = relation.is_none();
                relation
            });
            if relation_unavailable {
                self.rejected(address, "hostile-only-rule-relation-unavailable");
            }
            let selection = match result {
                Ok(selected) => selected,
                Err(reason) => {
                    self.rejected(address, reason);
                    config::Selection::default()
                }
            };
            if !is_player && selection.scale == 1.0 && !self.records.contains_key(&address) {
                continue;
            }
            let record = self.records.entry(address).or_insert_with(|| {
                let mut record = UnitRecord::new(identity, frame);
                if is_player {
                    body_scale_port::unregister_unit(&record.hooks);
                    record.hooks = body_scale_port::default_unit_state();
                    record.hooks.set_identity(identity);
                    body_scale_port::register_unit(&record.hooks);
                }
                record
            });
            if record.identity != identity {
                rebind_record(record, identity);
            }
            record.seen = frame;
            record.state.task_frames = frame;
            if !identity.current() {
                continue;
            }
            body_scale_port::register_unit(&record.hooks);
            // Validate the eventual pose route before shared consumers settle
            // their scale. This publishes state only; geometry is still untouched.
            if self.api.is_some() {
                let binding = body_scale_port::with_unit_state(&record.hooks, || {
                    body_scale_port::bind_local_player(address, selection.scale)
                });
                record.state.cached_binding = Some(binding);
                consumers.insert(
                    address,
                    crate::cloth_local_scale::shared::Consumer {
                        identity,
                        scale: if binding.ready { selection.scale } else { 1.0 },
                    },
                );
            }
            pending.push((identity, is_player, enemy_layout, kind, selection));
        }
        // Temporarily inactive instances retain their original baseline. Their
        // hooks are unregistered, so an old pointer cannot authorize callbacks.
        let dormant: Vec<_> = self
            .records
            .iter()
            .filter_map(|(address, record)| (record.seen != frame).then_some(*address))
            .collect();
        for address in dormant {
            let record = self.records.get_mut(&address).expect("listed record");
            let next = Identity::capture_checked(address, false);
            if let Some(next) = next.filter(|next| record.identity.same_instance(*next)) {
                if record.identity != next {
                    rebind_record(record, next);
                }
                if read::<u8>(record.identity.entry + 8) == Some(ACTIVE_ENTRY_STATE) {
                    // Still active but no longer eligible (for example a remote
                    // role): restore its own baseline instead of parking it.
                    let mut record = self.records.remove(&address).expect("listed record");
                    release_record(&mut record);
                    continue;
                }
            }
            if record.identity.instance_marker_current() {
                body_scale_port::detach_unit(&record.hooks);
                record.state.cached_binding = None;
                if self.api.is_some() {
                    consumers.insert(
                        address,
                        crate::cloth_local_scale::shared::Consumer {
                            identity: record.identity,
                            scale: record
                                .state
                                .last_selection
                                .map_or(1.0, |selection| selection.scale),
                        },
                    );
                }
            } else {
                let mut record = self.records.remove(&address).expect("listed record");
                release_record(&mut record);
                self.rejected.remove(&address);
            }
        }
        let prepared = if self.api.is_some() {
            self.cloth_pool
                .prepare(&consumers.into_values().collect::<Vec<_>>())
        } else {
            Arc::new(crate::cloth_local_scale::shared::Prepared::default())
        };
        for (identity, is_player, enemy_layout, kind, selection) in pending {
            let address = identity.address;
            let scale = if let Some(reason) = prepared.rejected.get(&address) {
                self.rejected(address, reason);
                1.0
            } else {
                selection.scale
            };
            let record = self.records.get_mut(&address).expect("prepared record");
            if !identity.current() {
                continue;
            }
            let hooks = record.hooks.clone();
            let mut apply = || {
                body_scale_port::with_unit_state(&hooks, || {
                    let applied = if is_player || !enemy_layout {
                        // Only verified PlayerIns can access the PlayerIns tail.
                        crate::apply_player_scale(
                            unsafe { &mut *(address as *mut eldenring::cs::PlayerIns) },
                            &mut record.state,
                            scale,
                        )
                    } else {
                        crate::apply_character_scale(
                            unsafe { &mut *(address as *mut ChrIns) },
                            &mut record.state,
                            scale,
                        )
                    };
                    if is_player && crate::ENABLE_SYNC_DIAGNOSTIC {
                        crate::cloth_diagnostic::tick(frame);
                    }
                    applied
                })
            };
            let applied = if self.api.is_some() {
                crate::cloth_local_scale::shared::with_prepared(&prepared, apply)
            } else {
                apply()
            };
            let effective = config::Selection {
                scale: applied,
                ..selection
            };
            if record.state.last_selection != Some(effective) {
                let facts = identity.facts(kind);
                let name = selection
                    .rule
                    .map(|i| self.config.rules[i].name.as_str())
                    .unwrap_or("baseline");
                crate::log::line(format_args!(
                    "[ERCS-UNIT] chr=0x{address:X} handle={:#X} kind={kind:?} character=c{:04} npc_param={} entity={} rule={name:?} requested={} applied={applied} ready={}",
                    identity.handle,
                    facts.character_id,
                    facts.npc_param_id,
                    facts.entity_id,
                    selection.scale,
                    record
                        .state
                        .cached_binding
                        .is_some_and(|binding| binding.ready)
                ));
                record.state.last_selection = Some(effective);
            }
        }
        body_scale_port::refresh_unit_registry();
        TickStatus::Ready
    }

    pub(crate) fn suspend(&mut self) {
        self.records.retain(|_, record| {
            body_scale_port::detach_unit(&record.hooks);
            record.state.cached_binding = None;
            record.identity.instance_marker_current()
        });
        body_scale_port::refresh_unit_registry();
    }

    pub fn clear(&mut self) {
        for (_, mut record) in self.records.drain() {
            release_record(&mut record);
        }
        self.rejected.clear();
        self.cloth_pool = Default::default();
        body_scale_port::refresh_unit_registry();
    }
}

fn rebind_record(record: &mut UnitRecord, next: Identity) {
    let previous = record.identity;
    let same_instance = previous.same_instance(next);
    if !same_instance {
        body_scale_port::unregister_unit(&record.hooks);
        record.state = crate::ScaleState::default();
        record.hooks = Arc::new(body_scale_port::UnitState::default());
    } else {
        // Replacing armor/pose must not recapture already-scaled dimensions as
        // a new baseline on an unchanged physics/control instance.
        if previous.control != next.control {
            record.state.visual_baseline = None;
        }
        if previous.physics != next.physics {
            record.state.baseline = None;
        }
        record.state.cached_binding = None;
        record.state.last_selection = None;
        if previous.model != next.model
            || previous.cloth_pose != next.cloth_pose
            || previous.pose != next.pose
        {
            record.state.cloth_local_scale_state = Default::default();
            record.state.cloth_instance_aabb_scale_state = Default::default();
            record.state.cloth_local_last_target = 0;
            record.state.cloth_aabb_last_target = 0;
        }
    }
    record.identity = next;
    record.hooks.set_identity(next);
}

fn release_record(record: &mut UnitRecord) {
    // Lost/recycled identities are discarded without touching their old data.
    if record.identity.current() {
        body_scale_port::with_unit_state(&record.hooks, || {
            let chr = unsafe { &mut *(record.identity.address as *mut ChrIns) };
            if record.state.visual_baseline.is_some() {
                crate::apply_visual_scale(chr, &mut record.state, 1.0);
            }
            if record.state.baseline.is_some() {
                crate::apply_physics_scale(chr, &mut record.state, 1.0);
            }
            body_scale_port::restore_unit_pose();
            record.state.reset();
        });
    }
    body_scale_port::unregister_unit(&record.hooks);
}

// Exact native witnesses are generated from the already pinned original PE.
include!("unit_native_checks.rs");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body_scale_port::test_characters::{BASE, Character, Environment};

    #[test]
    fn model_rules_reach_debug_and_summon_sets_but_exclude_ghosts_and_remote_players() {
        let _environment = Environment::new();
        let debug = Character::new(9520, 1, 1.0);
        debug.put(offset_of!(ChrIns, chr_type), 7i32);
        let buddy = Character::new(9520, 2, 1.0);
        buddy.put(offset_of!(ChrIns, chr_type), 6i32);
        let friendly_npc = Character::new(0, 3, 1.0);
        friendly_npc.put(0, BASE + PLAYER_VTABLE_RVA);
        friendly_npc.put(offset_of!(ChrIns, chr_type), 19i32);
        let ghost = Character::new(9520, 4, 1.0);
        let remote = Character::new(0, 5, 1.0);
        remote.put(0, BASE + PLAYER_VTABLE_RVA);
        remote.put(offset_of!(ChrIns, chr_type), 1i32);
        // Raw owned bytes exercise the actual bounded reader without constructing
        // a fake &WorldChrMan containing invalid Rust references or enum values.
        let mut world = vec![0u128; size_of::<WorldChrMan>().div_ceil(16)];
        let base = world.as_mut_ptr() as usize;
        let set = |offset: usize, actor: &Character| unsafe {
            ((base + offset + offset_of!(ChrSet<ChrIns>, capacity)) as *mut u32).write(1);
            ((base + offset + offset_of!(ChrSet<ChrIns>, entries)) as *mut usize)
                .write(actor.at(0xA000));
        };
        set(offset_of!(WorldChrMan, debug_chr_set), &debug);
        set(offset_of!(WorldChrMan, summon_buddy_chr_set), &buddy);
        set(offset_of!(WorldChrMan, open_field_chr_set), &friendly_npc);
        set(offset_of!(WorldChrMan, ghost_chr_set), &ghost);
        set(offset_of!(WorldChrMan, player_chr_set), &remote);
        for (i, offset) in [
            offset_of!(WorldChrMan, debug_chr_set),
            offset_of!(WorldChrMan, summon_buddy_chr_set),
            offset_of!(WorldChrMan, ghost_chr_set),
        ]
        .into_iter()
        .enumerate()
        {
            unsafe {
                ((base + offset_of!(WorldChrMan, chr_sets) + i * 8) as *mut usize)
                    .write(base + offset);
            }
        }
        let found = candidates_at(base, 0);
        assert_eq!(
            found,
            vec![friendly_npc.address, debug.address, buddy.address]
        );
        let api = EnemyApi { base: BASE };
        let config = config::Config::parse("version=1\nenabled=true\n[player]\nenabled=true\n[enemies]\nenabled=true\n[[rules]]\nname='c9520'\ntarget='enemy'\ncharacter_ids=[9520]\nmode='constant'\nscale=0.6").unwrap();
        for actor in [&debug, &buddy, &friendly_npc] {
            assert!(api.kind(actor.address).is_some());
            let identity = actor.identity();
            let selected = identity
                .resolve(config::TargetKind::Enemy, &config, || {
                    panic!("model rule must not query teams")
                })
                .unwrap();
            assert_eq!(
                selected.scale,
                if actor.address == friendly_npc.address {
                    1.0
                } else {
                    0.6
                }
            );
        }
        assert_eq!(api.kind(remote.address), None);
        buddy.put(offset_of!(ChrIns, character_id), 8000u32);
        assert_eq!(api.kind(buddy.address), None);
    }

    #[test]
    fn loaded_c9520_role_seven_reaches_candidate_enumeration() {
        let _environment = Environment::new();
        // Live WW2.7.1.0 c9520 in debug_chr_set (also chr_sets[133]):
        // EnemyIns, role 7, reciprocal active entry [4,4].
        let actor = Character::new(9520, 1, 1.0);
        actor.put(offset_of!(ChrIns, chr_type), 7i32);
        assert!(Identity::capture(actor.address).is_some());
        let mut set = vec![0usize; size_of::<ChrSet<ChrIns>>().div_ceil(8)];
        set[offset_of!(ChrSet<ChrIns>, capacity) / 8] = 1;
        set[offset_of!(ChrSet<ChrIns>, entries) / 8] = actor.at(0xA000);
        let mut output = Vec::new();
        let mut seen = HashSet::new();
        append_set(set.as_ptr() as usize, &mut output, &mut seen).unwrap();
        assert_eq!(output, vec![actor.address]);
        // The same entry referenced from a second collection is processed once.
        append_set(set.as_ptr() as usize, &mut output, &mut seen).unwrap();
        assert_eq!(output, vec![actor.address]);
    }

    #[test]
    fn loaded_c9520_role_seven_requires_verified_enemy_class() {
        let _environment = Environment::new();
        let api = EnemyApi { base: BASE };
        let actor = Character::new(9520, 1, 1.0);
        actor.put(offset_of!(ChrIns, chr_type), 7i32);
        assert_eq!(api.kind(actor.address), Some(true));
        actor.put(0, BASE + PLAYER_VTABLE_RVA);
        assert_eq!(api.kind(actor.address), None);
        actor.put(0, BASE + cloth_owner_scope::ENEMY_VTABLE_RVA);
        for role in [1i32, 2, 3, 4, 8, 19, 22] {
            actor.put(offset_of!(ChrIns, chr_type), role);
            assert_eq!(api.kind(actor.address), None);
        }
        actor.put(offset_of!(ChrIns, chr_type), 7i32);
        actor.put(offset_of!(ChrIns, character_id), 8000u32);
        assert_eq!(api.kind(actor.address), None);
    }

    #[test]
    fn loaded_player_and_enemy_with_native_entry_state_four_reach_identity_and_enumeration() {
        let _environment = Environment::new();
        // WW2.7.1.0: a loaded local player's reciprocal ChrSetEntry has
        // bytes [4, 4] at +8/+9. The adjacent byte is not a network role.
        let player = Character::new(0, 1, 1.0);
        player.put(0, BASE + PLAYER_VTABLE_RVA);
        player.put(offset_of!(ChrIns, chr_type), 0i32);
        player.put(0xA008, 4u8);
        player.put(0xA009, 4u8);
        assert!(
            Identity::capture(player.address).is_some(),
            "loaded main player must not suspend all unit scaling"
        );
        let enemy = Character::new(9520, 2, 1.0);
        enemy.put(0xA008, 4u8);
        enemy.put(0xA009, 4u8);
        assert!(Identity::capture(enemy.address).is_some());
        let mut set = vec![0usize; size_of::<ChrSet<ChrIns>>().div_ceil(8)];
        set[offset_of!(ChrSet<ChrIns>, capacity) / 8] = 1;
        set[offset_of!(ChrSet<ChrIns>, entries) / 8] = enemy.at(0xA000);
        let mut output = Vec::new();
        append_set(set.as_ptr() as usize, &mut output, &mut HashSet::new()).unwrap();
        assert_eq!(output, vec![enemy.address]);
        for state in [0u8, 1, 2, 3, 5, 255] {
            enemy.put(0xA008, state);
            assert!(
                Identity::capture(enemy.address).is_none(),
                "entry state {state} must not authorize scaling"
            );
            output.clear();
            append_set(set.as_ptr() as usize, &mut output, &mut HashSet::new()).unwrap();
            assert!(output.is_empty());
        }
    }

    #[test]
    fn effects_belong_to_the_subject_and_constant_mode_ignores_a_broken_effect_list() {
        let _environment = Environment::new();
        let a = Character::new(2010, 1, 1.0);
        let b = Character::new(2010, 2, 1.0);
        let mut config = config::Config::parse(
            &config::DEFAULT_TEXT.replace("target = \"player\"", "target = \"enemy\""),
        )
        .unwrap();
        config.enemies.enabled = true;
        a.put(0x9008, a.at(0x9100));
        a.put(0x9108, 8020400i32);
        assert_eq!(
            a.identity()
                .resolve(config::TargetKind::Enemy, &config, || None)
                .unwrap()
                .scale,
            0.5
        );
        assert_eq!(
            b.identity()
                .resolve(config::TargetKind::Enemy, &config, || None)
                .unwrap()
                .scale,
            1.0
        );
        a.put(0x9130, a.at(0x9100));
        assert!(
            a.identity()
                .resolve(config::TargetKind::Enemy, &config, || None)
                .is_err()
        );
        let fixed=config::Config::parse("version=1\nenabled=true\n[player]\nenabled=true\n[enemies]\nenabled=true\n[[rules]]\nname='fixed'\ntarget='enemy'\nmode='constant'\nscale=0.75").unwrap();
        assert_eq!(
            a.identity()
                .resolve(config::TargetKind::Enemy, &fixed, || None)
                .unwrap()
                .scale,
            0.75
        );
        a.put(0x9008, usize::MAX);
        assert!(
            a.identity()
                .resolve(config::TargetKind::Enemy, &config, || None)
                .is_err()
        );
    }

    #[test]
    fn native_type_checks_include_non_player_models_and_exclude_remote_roles_and_mount() {
        let _environment = Environment::new();
        let api = EnemyApi { base: BASE };
        let actor = Character::new(3250, 1, 1.0);
        assert_eq!(api.kind(actor.address), Some(true));
        actor.put(offset_of!(ChrIns, chr_type), 3i32);
        assert_eq!(api.kind(actor.address), None);
        actor.put(offset_of!(ChrIns, chr_type), 5i32);
        actor.put(offset_of!(ChrIns, character_id), 8000u32);
        assert_eq!(api.kind(actor.address), None);
        actor.put(offset_of!(ChrIns, character_id), 0u32);
        actor.put(0, BASE + PLAYER_VTABLE_RVA);
        for role in [5i32, 19, 20, 21] {
            actor.put(offset_of!(ChrIns, chr_type), role);
            assert_eq!(api.kind(actor.address), Some(false));
        }
        actor.put(offset_of!(ChrIns, chr_type), 1i32);
        assert_eq!(api.kind(actor.address), None);
        assert_eq!(api.kind(usize::MAX), None);
    }

    #[test]
    fn dormant_instances_and_model_replacement_preserve_baselines_but_new_handles_do_not() {
        let _environment = Environment::new();
        let actor = Character::new(2010, 1, 2.0);
        let mut record = UnitRecord::new(actor.identity(), 1);
        let hooks = record.hooks.clone();
        body_scale_port::with_unit_state(&hooks, || {
            crate::apply_character_scale(
                unsafe { &mut *(actor.address as *mut ChrIns) },
                &mut record.state,
                0.5,
            );
        });
        actor.put(0xA008, 2u8);
        assert!(Identity::capture(actor.address).is_none());
        assert_eq!(
            Identity::capture_checked(actor.address, false),
            Some(record.identity)
        );
        actor.put(0xA008, 4u8);
        actor.put(0xE000, BASE + cloth_owner_scope::CHARACTER_MODEL_VTABLE_RVA);
        actor.put(offset_of!(ChrIns, chr_model_ins), actor.at(0xE000));
        rebind_record(&mut record, actor.identity());
        let hooks = record.hooks.clone();
        body_scale_port::with_unit_state(&hooks, || {
            crate::apply_character_scale(
                unsafe { &mut *(actor.address as *mut ChrIns) },
                &mut record.state,
                1.0,
            );
        });
        assert_eq!(
            actor.get::<f32>(0x2000 + offset_of!(ChrCtrl, scale_size_x)),
            2.0
        );
        actor.put(offset_of!(ChrIns, field_ins_handle), 900u64);
        rebind_record(&mut record, actor.identity());
        assert!(record.state.visual_baseline.is_none());
        assert!(record.state.baseline.is_none());
        assert!(!Arc::ptr_eq(&hooks, &record.hooks));
        body_scale_port::unregister_unit(&record.hooks);
    }

    #[test]
    fn bad_owners_overflow_and_nonfinite_geometry_fail_before_writes() {
        let _environment = Environment::new();
        let actor = Character::new(2010, 1, 1.0);
        actor.put(offset_of!(ChrIns, modules), usize::MAX);
        assert!(Identity::capture(actor.address).is_none());
        actor.put(offset_of!(ChrIns, modules), actor.at(0x8000));
        actor.put(offset_of!(ChrIns, chr_ctrl), usize::MAX);
        assert!(Identity::capture(actor.address).is_none());
        actor.put(offset_of!(ChrIns, chr_ctrl), actor.at(0x2000));
        actor.put(0x2000 + offset_of!(ChrCtrl, scale_size_x), f32::NAN);
        assert!(Identity::capture(actor.address).is_none());
        actor.put(0x2000 + offset_of!(ChrCtrl, scale_size_x), f32::MAX);
        assert!(Identity::capture(actor.address).is_none());
        actor.put(0x2000 + offset_of!(ChrCtrl, scale_size_x), 1.0f32);
        actor.put(0x9010, actor.address + 16);
        assert!(Identity::capture(actor.address).is_none());
        assert!(Identity::capture(usize::MAX).is_none());
    }

    #[test]
    fn bounded_map_set_enumeration_deduplicates_and_keeps_remote_consumers_read_only() {
        let _environment = Environment::new();
        let actor = Character::new(3250, 1, 1.0);
        let mut set = vec![0usize; size_of::<ChrSet<ChrIns>>().div_ceil(8)];
        set[offset_of!(ChrSet<ChrIns>, capacity) / 8] = 1;
        set[offset_of!(ChrSet<ChrIns>, entries) / 8] = actor.at(0xA000);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        append_set(set.as_ptr() as usize, &mut out, &mut seen).unwrap();
        append_set(set.as_ptr() as usize, &mut out, &mut seen).unwrap();
        assert_eq!(out, vec![actor.address]);
        actor.put(offset_of!(ChrIns, chr_type), 3i32);
        seen.clear();
        out.clear();
        append_set(set.as_ptr() as usize, &mut out, &mut seen).unwrap();
        assert!(out.is_empty());
        append_set_mode(set.as_ptr() as usize, &mut out, &mut seen, true).unwrap();
        assert_eq!(out, vec![actor.address]);
        set[offset_of!(ChrSet<ChrIns>, capacity) / 8] = 8193;
        assert!(append_set(set.as_ptr() as usize, &mut out, &mut seen).is_none());
    }
}
