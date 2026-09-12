use std::{collections::HashMap, mem::size_of};

use crate::{
    body_scale_port::{self, ClothConstraintKind},
    log,
};

pub(crate) mod shared;

#[cfg(test)]
#[path = "test_support/convex_dimensions.rs"]
mod convex_dimensions;

const ER_HCL_SIM_CLOTH_DATA_VTABLE_RVA: usize = 0x2D8B6F8;
const ER_HCL_COLLIDABLE_VTABLE_RVA: usize = 0x2D8B758;
const ER_HCL_CAPSULE_SHAPE_VTABLE_RVA: usize = 0x2D896D0;
const ER_HCL_TAPERED_CAPSULE_SHAPE_VTABLE_RVA: usize = 0x2D7DBC8;
const ER_HCL_SPHERE_SHAPE_VTABLE_RVA: usize = 0x2D89D20;
const ER_HCL_PLANE_SHAPE_VTABLE_RVA: usize = 0x2D84968;
const ER_HCL_CONVEX_GEOMETRY_SHAPE_VTABLE_RVA: usize = 0x2D7D580;

const MAX_PARTICLES: usize = 16_384;
const MAX_POSES: usize = 256;
const MAX_CONSTRAINT_SETS: usize = 256;
const MAX_CONSTRAINT_ELEMENTS: usize = 65_536;
const MAX_COLLIDABLES: usize = 256;
const MAX_PINCHING_DATA: usize = 16_384;
const MAX_VIRTUAL_COLLISION_BLOCKS: usize = 65_536;
const MAX_FIELDS_PER_OBJECT: usize = 1_000_000;
const MAX_NEW_OBJECTS_PER_PASS: usize = 1;
const MAX_FIELDS_WRITTEN_PER_PASS: usize = 4_096;
const MAX_CAPTURE_ATTEMPTS_PER_EPOCH: u8 = 2;
const SCALE_COLLIDABLE_TRANSFORM_MAP_OFFSETS: bool = false;
const SCALE_VIRTUAL_COLLISION_SAFE_RADIUS: bool = false;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DimensionPower {
    Linear,
    Squared,
    Inverse,
    InverseSquared,
}

#[derive(Clone, Copy, Debug, Default)]
struct DimensionBounds {
    minimum_nonzero: f32,
    maximum: f32,
}

impl DimensionBounds {
    fn include(&mut self, value: f32, preserve_unbounded: bool) {
        let value = value.abs();
        if value == 0.0 || (preserve_unbounded && value >= f32::MAX * 0.5) {
            return;
        }
        self.maximum = self.maximum.max(value);
        if self.minimum_nonzero == 0.0 || value < self.minimum_nonzero {
            self.minimum_nonzero = value;
        }
    }

    fn supports(self, scale: f32, power: DimensionPower) -> bool {
        self.maximum == 0.0
            || [self.minimum_nonzero, self.maximum]
                .into_iter()
                .all(|value| scale_dimension_from_baseline(value, scale, power, false).is_some())
    }
}

#[derive(Clone, Copy, Debug)]
struct DimensionFieldBaseline {
    address: usize,
    baseline: f32,
    power: DimensionPower,
    preserve_unbounded: bool,
}

#[derive(Clone, Copy, Debug)]
struct RootIdentity {
    address: usize,
    vtable: usize,
}

#[derive(Clone, Copy, Debug)]
struct PointerIdentity {
    address: usize,
    value: usize,
}

#[derive(Clone, Copy, Debug)]
struct HkArrayIdentity {
    object: usize,
    pointer_offset: usize,
    count_offset: usize,
    stride: usize,
    maximum: usize,
    begin: usize,
    count: usize,
}

#[derive(Clone, Copy, Debug)]
struct FieldMemorySpan {
    region_start: usize,
    region_end: usize,
    used_start: usize,
    used_end: usize,
}

#[derive(Clone, Debug)]
struct DimensionObjectBaseline {
    address: usize,
    roots: Vec<RootIdentity>,
    pointers: Vec<PointerIdentity>,
    arrays: Vec<HkArrayIdentity>,
    field_spans: Vec<FieldMemorySpan>,
    field_kinds: HashMap<usize, (DimensionPower, bool)>,
    fields: Vec<DimensionFieldBaseline>,
    numeric_bounds: [DimensionBounds; 4],
    applied_scale_bits: u32,
    pending_scale_bits: u32,
    next_field: usize,
    unsupported_layouts: usize,
    collidable_map_entries: usize,
    collidable_offset_fields: usize,
    virtual_collision_blocks: usize,
    virtual_collision_radius_fields: usize,
    active_update_failed: bool,
}

impl DimensionObjectBaseline {
    fn new(address: usize, expected_vtable_rva: usize) -> Option<Self> {
        let vtable = read_usize(address)?;
        if body_scale_port::module_rva(vtable) != expected_vtable_rva {
            return None;
        }
        Some(Self {
            address,
            roots: vec![RootIdentity { address, vtable }],
            pointers: Vec::new(),
            arrays: Vec::new(),
            field_spans: Vec::new(),
            field_kinds: HashMap::new(),
            fields: Vec::new(),
            numeric_bounds: [DimensionBounds::default(); 4],
            applied_scale_bits: 1.0f32.to_bits(),
            pending_scale_bits: 0,
            next_field: 0,
            unsupported_layouts: 0,
            collidable_map_entries: 0,
            collidable_offset_fields: 0,
            virtual_collision_blocks: 0,
            virtual_collision_radius_fields: 0,
            active_update_failed: false,
        })
    }

    fn capture_root(&mut self, address: usize) -> Option<()> {
        let vtable = read_usize(address)?;
        if body_scale_port::module_rva(vtable) == 0 {
            return None;
        }
        self.roots.push(RootIdentity { address, vtable });
        Some(())
    }

    fn capture_pointer(&mut self, address: usize) -> Option<usize> {
        let value = read_usize(address)?;
        self.pointers.push(PointerIdentity { address, value });
        Some(value)
    }

    fn capture_array(
        &mut self,
        object: usize,
        pointer_offset: usize,
        count_offset: usize,
        stride: usize,
        maximum: usize,
    ) -> Option<(usize, usize)> {
        let (begin, count) =
            bounded_hk_array_span(object, pointer_offset, count_offset, stride, maximum)?;
        self.arrays.push(HkArrayIdentity {
            object,
            pointer_offset,
            count_offset,
            stride,
            maximum,
            begin,
            count,
        });
        Some((begin, count))
    }

    fn capture_field(
        &mut self,
        address: usize,
        power: DimensionPower,
        preserve_unbounded: bool,
    ) -> Option<()> {
        if let Some(&(existing_power, existing_preserve_unbounded)) = self.field_kinds.get(&address)
        {
            return (existing_power == power && existing_preserve_unbounded == preserve_unbounded)
                .then_some(());
        }
        if self.fields.len() == MAX_FIELDS_PER_OBJECT {
            return None;
        }
        let field_end = address.checked_add(size_of::<f32>())?;
        if let Some(span_index) = self
            .field_spans
            .iter()
            .rposition(|span| address >= span.region_start && field_end <= span.region_end)
        {
            let span = &mut self.field_spans[span_index];
            span.used_start = span.used_start.min(address);
            span.used_end = span.used_end.max(field_end);
        } else {
            let (region_start, region_end) =
                accessible_memory_region(address, size_of::<f32>(), true)?;
            self.field_spans.push(FieldMemorySpan {
                region_start,
                region_end,
                used_start: address,
                used_end: field_end,
            });
        }
        // The containing writable region was just validated or came from the
        // capture cache above, so avoid a VirtualQuery call per scalar.
        let baseline = unsafe { (address as *const f32).read_unaligned() };
        if !baseline.is_finite() {
            return None;
        }
        self.numeric_bounds[power as usize].include(baseline, preserve_unbounded);
        self.fields.push(DimensionFieldBaseline {
            address,
            baseline,
            power,
            preserve_unbounded,
        });
        self.field_kinds
            .insert(address, (power, preserve_unbounded));
        Some(())
    }

    fn capture_vec3(&mut self, address: usize, power: DimensionPower) -> Option<()> {
        for component in 0..3 {
            self.capture_field(
                address.checked_add(component * size_of::<f32>())?,
                power,
                false,
            )?;
        }
        Some(())
    }

    fn capture_vec4(&mut self, address: usize, power: DimensionPower) -> Option<()> {
        for component in 0..4 {
            self.capture_field(
                address.checked_add(component * size_of::<f32>())?,
                power,
                false,
            )?;
        }
        Some(())
    }

    fn identity_matches(&self) -> bool {
        self.roots
            .iter()
            .all(|root| read_usize(root.address) == Some(root.vtable))
            && self
                .pointers
                .iter()
                .all(|pointer| read_usize(pointer.address) == Some(pointer.value))
            && self.arrays.iter().all(|array| {
                bounded_hk_array_span(
                    array.object,
                    array.pointer_offset,
                    array.count_offset,
                    array.stride,
                    array.maximum,
                ) == Some((array.begin, array.count))
            })
    }

    fn apply(&mut self, scale: f32) -> Option<usize> {
        let (writes, complete) = self.apply_budgeted(scale, usize::MAX)?;
        complete.then_some(writes)
    }

    fn refresh_writable_field_spans(&mut self) -> bool {
        let spans_writable = |spans: &[FieldMemorySpan]| {
            spans.iter().all(|span| {
                is_memory_accessible(
                    span.used_start,
                    span.used_end.saturating_sub(span.used_start),
                    true,
                )
            })
        };
        if spans_writable(&self.field_spans) {
            return true;
        }
        // Several unrelated heap allocations may initially share one OS
        // region. Releasing a NON-field gap later splits it without invalidating
        // any cloth array. Repartition by current regions, checking every actual
        // field before any writes. Never use the old region cache as permission
        // to access an unverified field. This slow path runs only on a span miss.
        let mut refreshed: Vec<FieldMemorySpan> = Vec::with_capacity(self.field_spans.len());
        for field in &self.fields {
            let Some(end) = field.address.checked_add(size_of::<f32>()) else {
                return false;
            };
            if let Some(span) = refreshed
                .iter_mut()
                .find(|span| field.address >= span.region_start && end <= span.region_end)
            {
                span.used_start = span.used_start.min(field.address);
                span.used_end = span.used_end.max(end);
            } else {
                let Some((region_start, region_end)) =
                    accessible_memory_region(field.address, size_of::<f32>(), true)
                else {
                    return false;
                };
                refreshed.push(FieldMemorySpan {
                    region_start,
                    region_end,
                    used_start: field.address,
                    used_end: end,
                });
            }
        }
        if !spans_writable(&refreshed) {
            return false;
        }
        self.field_spans = refreshed;
        true
    }

    fn supports_scale(&self, scale: f32) -> bool {
        valid_scale(scale)
            && [
                DimensionPower::Linear,
                DimensionPower::Squared,
                DimensionPower::Inverse,
                DimensionPower::InverseSquared,
            ]
            .into_iter()
            .all(|power| self.numeric_bounds[power as usize].supports(scale, power))
    }

    fn rebuild_numeric_bounds(&mut self) {
        self.numeric_bounds = [DimensionBounds::default(); 4];
        for field in &self.fields {
            self.numeric_bounds[field.power as usize]
                .include(field.baseline, field.preserve_unbounded);
        }
    }

    fn apply_budgeted(&mut self, scale: f32, maximum_fields: usize) -> Option<(usize, bool)> {
        // A numeric rejection is request-specific, not a permanent layout failure.
        // Check before any field is written or a pending transition is changed.
        if !self.supports_scale(scale) {
            return None;
        }

        // A departing instance must not restore data still used at a non-neutral
        // scale by another instance. The preflight resolves all consumers first.
        if scale == 1.0 && shared::retained_by_scaled_consumer(self) {
            return Some((0, true));
        }
        if self.applied_scale_bits == scale.to_bits() && self.pending_scale_bits == 0 {
            self.pending_scale_bits = 0;
            self.next_field = 0;
            return Some((0, true));
        }
        if self.active_update_failed && scale.to_bits() != 1.0f32.to_bits() {
            return None;
        }
        if self.pending_scale_bits != scale.to_bits() {
            self.pending_scale_bits = 0;
            self.next_field = 0;
            if !valid_scale(scale) || !self.identity_matches() {
                if scale.to_bits() != 1.0f32.to_bits() {
                    self.active_update_failed = true;
                }
                return None;
            }
            if !self.refresh_writable_field_spans() {
                if scale.to_bits() != 1.0f32.to_bits() {
                    self.active_update_failed = true;
                }
                return None;
            }
            // Four cached extrema pairs cover every field without a second
            // unbudgeted pass over particle arrays at each scale transition.
            self.pending_scale_bits = scale.to_bits();
        }

        let end = self
            .next_field
            .saturating_add(maximum_fields)
            .min(self.fields.len());
        for field in &self.fields[self.next_field..end] {
            let value = scale_dimension_from_baseline(
                field.baseline,
                scale,
                field.power,
                field.preserve_unbounded,
            )?;
            write_f32(field.address, value);
        }
        let writes = end.saturating_sub(self.next_field);
        self.next_field = end;
        let complete = self.next_field == self.fields.len();
        if complete {
            self.applied_scale_bits = scale.to_bits();
            self.pending_scale_bits = 0;
            self.next_field = 0;
            self.active_update_failed = false;
        }
        Some((writes, complete))
    }
}

#[derive(Default)]
pub struct ClothLocalScaleState {
    target_input: usize,
    simulations: Vec<DimensionObjectBaseline>,
    collidables: Vec<DimensionObjectBaseline>,
    shapes: Vec<DimensionObjectBaseline>,
    rejected_simulations: Vec<RejectedObjectCapture>,
    rejected_collidables: Vec<RejectedObjectCapture>,
    rejected_shapes: Vec<RejectedObjectCapture>,
    rejection_epoch: Option<(usize, u32, u64)>,
    last_report: Option<(u32, ClothLocalScaleResult)>,
    rejected_updates: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RejectedObjectCapture {
    address: usize,
    attempts: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClothLocalScaleResult {
    pub topology_readable: bool,
    pub simulations: usize,
    pub collidables: usize,
    pub supported_shapes: usize,
    pub unsupported_shapes: usize,
    pub captured_objects: usize,
    pub restored_objects: usize,
    pub fields_written: usize,
    pub rejected_objects: usize,
    pub topology_rejected_objects: usize,
    pub capture_rejected_objects: usize,
    pub apply_rejected_objects: usize,
    pub stale_restore_rejected_objects: usize,
    pub deferred_rejected_objects: usize,
    pub unsupported_layouts: usize,
    pub work_pending: bool,
}

pub fn scale_target_cloth_local_dimensions(
    target_input: usize,
    requested_scale: f32,
    topology_generation: u64,
    state: &mut ClothLocalScaleState,
) -> ClothLocalScaleResult {
    crate::memory_query::scoped(|| {
        scale_target_cloth_local_dimensions_inner(
            target_input,
            requested_scale,
            topology_generation,
            state,
        )
    })
}

fn scale_target_cloth_local_dimensions_inner(
    target_input: usize,
    requested_scale: f32,
    topology_generation: u64,
    state: &mut ClothLocalScaleState,
) -> ClothLocalScaleResult {
    if target_input == 0 || !valid_scale(requested_scale) {
        restore_cloth_local_dimensions(state);
        return ClothLocalScaleResult::default();
    }
    if state.target_input != 0 && state.target_input != target_input {
        restore_cloth_local_dimensions(state);
    }
    state.target_input = target_input;

    let rejection_epoch = (target_input, requested_scale.to_bits(), topology_generation);
    if state.rejection_epoch != Some(rejection_epoch) {
        state.rejected_simulations.clear();
        state.rejected_collidables.clear();
        state.rejected_shapes.clear();
        state.rejection_epoch = Some(rejection_epoch);
    }

    let live = body_scale_port::target_cloth_simulation_set(target_input);
    if !live.readable || live.truncated {
        state.rejected_updates = state.rejected_updates.saturating_add(1);
        let result = ClothLocalScaleResult {
            rejected_objects: 1,
            topology_rejected_objects: 1,
            ..ClothLocalScaleResult::default()
        };
        report_scale_result(requested_scale, state, result);
        return result;
    }
    let live_simulations = &live.sim_data[..live.count];

    let Some((live_collidables, live_shapes, unsupported_shapes)) =
        collect_live_collision_roots(live_simulations)
    else {
        state.rejected_updates = state.rejected_updates.saturating_add(1);
        let result = ClothLocalScaleResult {
            simulations: live.count,
            rejected_objects: 1,
            topology_rejected_objects: 1,
            ..ClothLocalScaleResult::default()
        };
        report_scale_result(requested_scale, state, result);
        return result;
    };

    let mut result = ClothLocalScaleResult {
        topology_readable: true,
        simulations: live.count,
        collidables: live_collidables.len(),
        supported_shapes: live_shapes.len(),
        unsupported_shapes,
        ..ClothLocalScaleResult::default()
    };

    for (restored, rejected) in [
        remove_stale_objects(&mut state.simulations, live_simulations),
        remove_stale_objects(&mut state.collidables, &live_collidables),
        remove_stale_objects(&mut state.shapes, &live_shapes),
    ] {
        result.restored_objects += restored;
        result.rejected_objects += rejected;
        result.stale_restore_rejected_objects += rejected;
    }
    state
        .rejected_simulations
        .retain(|rejected| live_simulations.contains(&rejected.address));
    state
        .rejected_collidables
        .retain(|rejected| live_collidables.contains(&rejected.address));
    state
        .rejected_shapes
        .retain(|rejected| live_shapes.contains(&rejected.address));

    let mut captures_remaining = MAX_NEW_OBJECTS_PER_PASS;
    for &sim_data in live_simulations {
        if state
            .simulations
            .iter()
            .any(|baseline| baseline.address == sim_data)
        {
            continue;
        }
        if !capture_attempt_allowed(&state.rejected_simulations, sim_data) {
            result.rejected_objects += 1;
            result.deferred_rejected_objects += 1;
            continue;
        }
        if captures_remaining == 0 {
            break;
        }
        captures_remaining -= 1;
        match shared::capture(sim_data, || capture_simulation_baseline(sim_data)) {
            Some(baseline) => {
                state.simulations.push(baseline);
                resolve_capture_rejection(&mut state.rejected_simulations, sim_data);
                result.captured_objects += 1;
            }
            None => {
                result.work_pending |=
                    record_capture_rejection(&mut state.rejected_simulations, sim_data);
                result.rejected_objects += 1;
                result.capture_rejected_objects += 1;
            }
        }
    }

    // This is an active-topology property, not merely a capture-event
    // counter. Preserve it on later scale-transition reports so the runtime
    // checker cannot mistake a partially unsupported simulation for success.
    result.unsupported_layouts = state
        .simulations
        .iter()
        .map(|baseline| baseline.unsupported_layouts)
        .sum();
    for &collidable in &live_collidables {
        if state
            .collidables
            .iter()
            .any(|baseline| baseline.address == collidable)
        {
            continue;
        }
        if !capture_attempt_allowed(&state.rejected_collidables, collidable) {
            result.rejected_objects += 1;
            result.deferred_rejected_objects += 1;
            continue;
        }
        if captures_remaining == 0 {
            break;
        }
        captures_remaining -= 1;
        match shared::capture(collidable, || capture_collidable_baseline(collidable)) {
            Some(baseline) => {
                state.collidables.push(baseline);
                resolve_capture_rejection(&mut state.rejected_collidables, collidable);
                result.captured_objects += 1;
            }
            None => {
                result.work_pending |=
                    record_capture_rejection(&mut state.rejected_collidables, collidable);
                result.rejected_objects += 1;
                result.capture_rejected_objects += 1;
            }
        }
    }
    for &shape in &live_shapes {
        if state
            .shapes
            .iter()
            .any(|baseline| baseline.address == shape)
        {
            continue;
        }
        if !capture_attempt_allowed(&state.rejected_shapes, shape) {
            result.rejected_objects += 1;
            result.deferred_rejected_objects += 1;
            continue;
        }
        if captures_remaining == 0 {
            break;
        }
        captures_remaining -= 1;
        match shared::capture(shape, || capture_shape_baseline(shape)) {
            Some(baseline) => {
                state.shapes.push(baseline);
                resolve_capture_rejection(&mut state.rejected_shapes, shape);
                result.captured_objects += 1;
            }
            None => {
                result.work_pending |= record_capture_rejection(&mut state.rejected_shapes, shape);
                result.rejected_objects += 1;
                result.capture_rejected_objects += 1;
            }
        }
    }

    let mut field_budget = MAX_FIELDS_WRITTEN_PER_PASS;
    for baseline in state
        .simulations
        .iter_mut()
        .chain(state.collidables.iter_mut())
        .chain(state.shapes.iter_mut())
    {
        if field_budget == 0 {
            result.work_pending = true;
            break;
        }
        match baseline.apply_budgeted(requested_scale, field_budget) {
            Some((writes, complete)) => {
                result.fields_written += writes;
                field_budget = field_budget.saturating_sub(writes);
                result.work_pending |= !complete;
            }
            None => {
                result.rejected_objects += 1;
                result.apply_rejected_objects += 1;
            }
        }
    }

    result.work_pending |= live_simulations.iter().any(|address| {
        object_capture_pending(&state.simulations, &state.rejected_simulations, *address)
    }) || live_collidables.iter().any(|address| {
        object_capture_pending(&state.collidables, &state.rejected_collidables, *address)
    }) || live_shapes
        .iter()
        .any(|address| object_capture_pending(&state.shapes, &state.rejected_shapes, *address))
        || state
            .simulations
            .iter()
            .chain(state.collidables.iter())
            .chain(state.shapes.iter())
            .any(|baseline| baseline.applied_scale_bits != requested_scale.to_bits());

    if result.rejected_objects > 0 {
        state.rejected_updates = state
            .rejected_updates
            .saturating_add(result.rejected_objects as u64);
    }
    report_scale_result(requested_scale, state, result);
    result
}

fn report_scale_result(
    requested_scale: f32,
    state: &mut ClothLocalScaleState,
    result: ClothLocalScaleResult,
) {
    let report = (requested_scale.to_bits(), result);
    if state.last_report != Some(report) {
        let simulation_fields = state
            .simulations
            .iter()
            .map(|baseline| baseline.fields.len())
            .sum::<usize>();
        let collidable_fields = state
            .collidables
            .iter()
            .map(|baseline| baseline.fields.len())
            .sum::<usize>();
        let shape_fields = state
            .shapes
            .iter()
            .map(|baseline| baseline.fields.len())
            .sum::<usize>();
        let collidable_map_entries = state
            .simulations
            .iter()
            .map(|baseline| baseline.collidable_map_entries)
            .sum::<usize>();
        let collidable_offset_fields = state
            .simulations
            .iter()
            .map(|baseline| baseline.collidable_offset_fields)
            .sum::<usize>();
        let virtual_collision_blocks = state
            .simulations
            .iter()
            .map(|baseline| baseline.virtual_collision_blocks)
            .sum::<usize>();
        let virtual_collision_radius_fields = state
            .simulations
            .iter()
            .map(|baseline| baseline.virtual_collision_radius_fields)
            .sum::<usize>();
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-LOCAL-SCALE] requested={requested_scale:.3} topology_readable={} simulations={} collidables={} supported_shapes={} unsupported_shapes={} captured_objects={} restored_objects={} fields_written={} simulation_fields={} collidable_fields={} shape_fields={} collidable_map_entries={} collidable_offset_fields={} virtual_collision_blocks={} virtual_collision_radius_fields={} rejected_objects={} topology_rejected_objects={} capture_rejected_objects={} apply_rejected_objects={} stale_restore_rejected_objects={} deferred_rejected_objects={} unsupported_layouts={} work_pending={} rejected_updates={}",
            result.topology_readable,
            result.simulations,
            result.collidables,
            result.supported_shapes,
            result.unsupported_shapes,
            result.captured_objects,
            result.restored_objects,
            result.fields_written,
            simulation_fields,
            collidable_fields,
            shape_fields,
            collidable_map_entries,
            collidable_offset_fields,
            virtual_collision_blocks,
            virtual_collision_radius_fields,
            result.rejected_objects,
            result.topology_rejected_objects,
            result.capture_rejected_objects,
            result.apply_rejected_objects,
            result.stale_restore_rejected_objects,
            result.deferred_rejected_objects,
            result.unsupported_layouts,
            result.work_pending,
            state.rejected_updates,
        ));
    }
    state.last_report = Some(report);
}

fn capture_attempt_allowed(rejected: &[RejectedObjectCapture], address: usize) -> bool {
    rejected
        .iter()
        .find(|entry| entry.address == address)
        .is_none_or(|entry| entry.attempts < MAX_CAPTURE_ATTEMPTS_PER_EPOCH)
}

fn record_capture_rejection(rejected: &mut Vec<RejectedObjectCapture>, address: usize) -> bool {
    if let Some(entry) = rejected.iter_mut().find(|entry| entry.address == address) {
        entry.attempts = entry.attempts.saturating_add(1);
        return entry.attempts < MAX_CAPTURE_ATTEMPTS_PER_EPOCH;
    }
    rejected.push(RejectedObjectCapture {
        address,
        attempts: 1,
    });
    true
}

fn resolve_capture_rejection(rejected: &mut Vec<RejectedObjectCapture>, address: usize) {
    rejected.retain(|entry| entry.address != address);
}

fn object_capture_pending(
    objects: &[DimensionObjectBaseline],
    rejected: &[RejectedObjectCapture],
    address: usize,
) -> bool {
    !objects.iter().any(|baseline| baseline.address == address)
        && capture_attempt_allowed(rejected, address)
}

pub fn restore_cloth_local_dimensions(state: &mut ClothLocalScaleState) -> usize {
    crate::memory_query::scoped(|| restore_cloth_local_dimensions_inner(state))
}

fn restore_cloth_local_dimensions_inner(state: &mut ClothLocalScaleState) -> usize {
    let mut restored = 0usize;
    for baseline in state
        .simulations
        .iter_mut()
        .chain(state.collidables.iter_mut())
        .chain(state.shapes.iter_mut())
    {
        if baseline.apply(1.0).is_some() {
            restored += 1;
        }
    }
    if !state.simulations.is_empty() || !state.collidables.is_empty() || !state.shapes.is_empty() {
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-LOCAL-RESTORE] target=0x{:X} simulations={} collidables={} shapes={} restored_objects={restored}",
            state.target_input,
            state.simulations.len(),
            state.collidables.len(),
            state.shapes.len(),
        ));
    }
    *state = ClothLocalScaleState::default();
    restored
}

fn remove_stale_objects(
    objects: &mut Vec<DimensionObjectBaseline>,
    live: &[usize],
) -> (usize, usize) {
    let mut restored = 0usize;
    let mut rejected = 0usize;
    let mut index = 0usize;
    while index < objects.len() {
        if live.contains(&objects[index].address) {
            index += 1;
            continue;
        }
        if objects[index].apply(1.0).is_some() {
            restored += 1;
        } else {
            rejected += 1;
        }
        objects.remove(index);
    }
    (restored, rejected)
}

fn collect_live_collision_roots(simulations: &[usize]) -> Option<(Vec<usize>, Vec<usize>, usize)> {
    let mut collidables = Vec::new();
    let mut shapes = Vec::new();
    let mut unsupported_shapes = Vec::new();
    for &sim_data in simulations {
        if body_scale_port::module_rva(read_usize(sim_data)?) != ER_HCL_SIM_CLOTH_DATA_VTABLE_RVA {
            return None;
        }
        let (array, count) =
            bounded_hk_array_span(sim_data, 0xD0, 0xD8, size_of::<usize>(), MAX_COLLIDABLES)?;
        for index in 0..count {
            let collidable = read_usize(array.checked_add(index * size_of::<usize>())?)?;
            if body_scale_port::module_rva(read_usize(collidable)?) != ER_HCL_COLLIDABLE_VTABLE_RVA
            {
                return None;
            }
            if !collidables.contains(&collidable) {
                collidables.push(collidable);
            }
            let shape = read_usize(collidable.checked_add(0x88)?)?;
            let shape_vtable_rva = body_scale_port::module_rva(read_usize(shape)?);
            if is_supported_shape_vtable(shape_vtable_rva) {
                if !shapes.contains(&shape) {
                    shapes.push(shape);
                }
            } else if !unsupported_shapes.contains(&shape) {
                unsupported_shapes.push(shape);
            }
        }
    }
    Some((collidables, shapes, unsupported_shapes.len()))
}

fn capture_simulation_baseline(sim_data: usize) -> Option<DimensionObjectBaseline> {
    let mut baseline = DimensionObjectBaseline::new(sim_data, ER_HCL_SIM_CLOTH_DATA_VTABLE_RVA)?;

    let (particles, particle_count) =
        baseline.capture_array(sim_data, 0x40, 0x48, 0x10, MAX_PARTICLES)?;
    for index in 0..particle_count {
        baseline.capture_field(
            particles
                .checked_add(index.checked_mul(0x10)?)?
                .checked_add(0x08)?,
            DimensionPower::Linear,
            false,
        )?;
    }
    baseline.capture_field(sim_data.checked_add(0xE0)?, DimensionPower::Linear, false)?;

    let (poses, pose_count) =
        baseline.capture_array(sim_data, 0x78, 0x80, size_of::<usize>(), MAX_POSES)?;
    for index in 0..pose_count {
        let pose_pointer = poses.checked_add(index.checked_mul(size_of::<usize>())?)?;
        let pose = baseline.capture_pointer(pose_pointer)?;
        baseline.capture_root(pose)?;
        let (positions, position_count) =
            baseline.capture_array(pose, 0x20, 0x28, 0x10, MAX_PARTICLES)?;
        for position_index in 0..position_count {
            baseline.capture_vec3(
                positions.checked_add(position_index.checked_mul(0x10)?)?,
                DimensionPower::Linear,
            )?;
        }
    }

    for (pointer_offset, count_offset) in [(0x88, 0x90), (0x98, 0xA0)] {
        let (sets, set_count) = baseline.capture_array(
            sim_data,
            pointer_offset,
            count_offset,
            size_of::<usize>(),
            MAX_CONSTRAINT_SETS,
        )?;
        for index in 0..set_count {
            let set_pointer = sets.checked_add(index.checked_mul(size_of::<usize>())?)?;
            let set = baseline.capture_pointer(set_pointer)?;
            baseline.capture_root(set)?;
            capture_constraint_dimensions(&mut baseline, set)?;
        }
    }

    let (collidables, collidable_count) =
        baseline.capture_array(sim_data, 0xD0, 0xD8, size_of::<usize>(), MAX_COLLIDABLES)?;
    for index in 0..collidable_count {
        baseline
            .capture_pointer(collidables.checked_add(index.checked_mul(size_of::<usize>())?)?)?;
    }

    let collidable_map_entries = capture_collidable_transform_map_dimensions(
        &mut baseline,
        sim_data,
        collidable_count,
        SCALE_COLLIDABLE_TRANSFORM_MAP_OFFSETS,
    )?;
    baseline.collidable_map_entries = collidable_map_entries;
    baseline.collidable_offset_fields = if SCALE_COLLIDABLE_TRANSFORM_MAP_OFFSETS {
        collidable_map_entries.checked_mul(3)?
    } else {
        0
    };

    let virtual_collision_blocks = capture_virtual_collision_point_dimensions(
        &mut baseline,
        sim_data,
        SCALE_VIRTUAL_COLLISION_SAFE_RADIUS,
    )?;
    baseline.virtual_collision_blocks = virtual_collision_blocks;
    baseline.virtual_collision_radius_fields = if SCALE_VIRTUAL_COLLISION_SAFE_RADIUS {
        virtual_collision_blocks
    } else {
        0
    };

    // hclSimClothData::LandscapeCollisionData is inline at +0x140.
    for offset in [0x140, 0x150, 0x154] {
        let address = sim_data.checked_add(offset)?;
        if read_f32(address)? >= 0.0 {
            baseline.capture_field(address, DimensionPower::Linear, true)?;
        }
    }

    let (pinching_data, pinching_count) =
        baseline.capture_array(sim_data, 0x198, 0x1A0, 0x08, MAX_PINCHING_DATA)?;
    for index in 0..pinching_count {
        let address = pinching_data
            .checked_add(index.checked_mul(0x08)?)?
            .checked_add(0x04)?;
        if read_f32(address)? >= 0.0 {
            baseline.capture_field(address, DimensionPower::Linear, true)?;
        }
    }

    Some(baseline)
}

fn capture_collidable_transform_map_dimensions(
    baseline: &mut DimensionObjectBaseline,
    sim_data: usize,
    expected_collidable_count: usize,
    scale_offsets: bool,
) -> Option<usize> {
    let transform_set_index = read_i32(sim_data.checked_add(0xA8)?)?;
    let (_, transform_index_count) =
        baseline.capture_array(sim_data, 0xB0, 0xB8, size_of::<u32>(), MAX_COLLIDABLES)?;
    let (offsets, offset_count) =
        baseline.capture_array(sim_data, 0xC0, 0xC8, 0x40, MAX_COLLIDABLES)?;
    if transform_index_count != expected_collidable_count
        || offset_count != expected_collidable_count
    {
        return None;
    }
    // Live BD_M_1690 uses -1 for an absent map. Its particle/pose/constraint
    // dimensions still need scaling. Only admit this exact empty topology;
    // capture the empty arrays above so later population invalidates it.
    let absent_map = transform_set_index == -1 && expected_collidable_count == 0;
    if !absent_map
        && (transform_set_index < 0 || transform_set_index as usize > MAX_CONSTRAINT_SETS)
    {
        return None;
    }

    for index in 0..offset_count {
        let matrix = offsets.checked_add(index.checked_mul(0x40)?)?;
        let mut values = [0.0f32; 16];
        for (component, value) in values.iter_mut().enumerate() {
            *value = read_f32(matrix.checked_add(component.checked_mul(size_of::<f32>())?)?)?;
            if !value.is_finite() {
                return None;
            }
        }
        if values[3].abs() > 0.001
            || values[7].abs() > 0.001
            || values[11].abs() > 0.001
            || (values[15] - 1.0).abs() > 0.001
        {
            return None;
        }
        if scale_offsets {
            baseline.capture_vec3(matrix.checked_add(0x30)?, DimensionPower::Linear)?;
        }
    }
    Some(offset_count)
}

fn capture_virtual_collision_point_dimensions(
    baseline: &mut DimensionObjectBaseline,
    sim_data: usize,
    scale_safe_radius: bool,
) -> Option<usize> {
    // hclVirtualCollisionPointsData is inline at hclSimClothData+0x1B0.
    // Its first member is hkArray<Block>; each 8-byte block begins with the
    // only dimensional member, m_safeDisplacementRadius.
    let (blocks, block_count) =
        baseline.capture_array(sim_data, 0x1B0, 0x1B8, 0x08, MAX_VIRTUAL_COLLISION_BLOCKS)?;
    for index in 0..block_count {
        let radius = blocks.checked_add(index.checked_mul(0x08)?)?;
        if read_f32(radius)? < 0.0 {
            return None;
        }
        if scale_safe_radius {
            baseline.capture_field(radius, DimensionPower::Linear, false)?;
        }
    }
    Some(block_count)
}

fn capture_constraint_dimensions(baseline: &mut DimensionObjectBaseline, set: usize) -> Option<()> {
    let kind = body_scale_port::constraint_kind_from_vtable_rva(body_scale_port::module_rva(
        read_usize(set)?,
    ));
    let layout: Option<(usize, &[usize], DimensionPower, bool)> = match kind {
        ClothConstraintKind::StandardLink | ClothConstraintKind::StretchLink => {
            Some((0x0C, &[0x04], DimensionPower::Linear, false))
        }
        ClothConstraintKind::CompressibleLink => {
            Some((0x10, &[0x04, 0x08], DimensionPower::Linear, false))
        }
        ClothConstraintKind::LocalRange => {
            Some((0x10, &[0x04, 0x08, 0x0C], DimensionPower::Linear, true))
        }
        ClothConstraintKind::Transition => Some((0x10, &[0x0C], DimensionPower::Linear, true)),
        ClothConstraintKind::BonePlanes => Some((0x20, &[0x0C], DimensionPower::Linear, false)),
        ClothConstraintKind::BendLink => Some((0x14, &[0x04, 0x08], DimensionPower::Linear, false)),
        ClothConstraintKind::BendStiffness => {
            baseline.capture_field(set.checked_add(0x38)?, DimensionPower::Squared, false)?;
            Some((0x20, &[0x14], DimensionPower::Inverse, false))
        }
        ClothConstraintKind::AntiPinch => {
            baseline.capture_field(set.checked_add(0x40)?, DimensionPower::Linear, true)?;
            let _ = baseline.capture_array(set, 0x28, 0x30, 0x04, MAX_CONSTRAINT_ELEMENTS)?;
            return Some(());
        }
        ClothConstraintKind::VolumeMx => {
            return capture_volume_constraint_dimensions(baseline, set);
        }
        _ => {
            baseline.unsupported_layouts = baseline.unsupported_layouts.saturating_add(1);
            return Some(());
        }
    };
    let (stride, offsets, power, preserve_unbounded) = layout?;
    let (elements, count) =
        baseline.capture_array(set, 0x28, 0x30, stride, MAX_CONSTRAINT_ELEMENTS)?;
    for index in 0..count {
        let element = elements.checked_add(index.checked_mul(stride)?)?;
        for &offset in offsets {
            baseline.capture_field(element.checked_add(offset)?, power, preserve_unbounded)?;
        }
    }
    Some(())
}

fn capture_volume_constraint_dimensions(
    baseline: &mut DimensionObjectBaseline,
    set: usize,
) -> Option<()> {
    // ER 15E4C50 / 15EA160: A = sum(w*(p-c)*frame.xyz^T).
    // Keep A invariant under p,c -> s*(p,c) by frame.xyz -> frame.xyz/s.
    // ER 15E4820 / 15EA080: target = R*apply.xyz+c, so apply.xyz -> s*apply.xyz.
    // This preserves the native nonlinear frame solve and its caches, without
    // scaling weights, stiffness, W, indices or touching simulation particles.
    // Layout: reflected hk2018 registry + ER 1552E00 array destruction strides.
    let mut total_vectors = 0usize;
    for (pointer_offset, count_offset, stride, lanes, power) in [
        (0x28, 0x30, 0x160, 16, DimensionPower::Inverse),
        (0x38, 0x40, 0x20, 1, DimensionPower::Inverse),
        (0x48, 0x50, 0x160, 16, DimensionPower::Linear),
        (0x58, 0x60, 0x20, 1, DimensionPower::Linear),
    ] {
        let (elements, count) = baseline.capture_array(
            set,
            pointer_offset,
            count_offset,
            stride,
            MAX_CONSTRAINT_ELEMENTS / lanes,
        )?;
        total_vectors = total_vectors.checked_add(count.checked_mul(lanes)?)?;
        if total_vectors > MAX_CONSTRAINT_ELEMENTS {
            return None;
        }
        for block in 0..count {
            let element = elements.checked_add(block.checked_mul(stride)?)?;
            for lane in 0..lanes {
                baseline.capture_vec3(element.checked_add(lane.checked_mul(0x10)?)?, power)?;
            }
        }
    }
    Some(())
}

fn capture_collidable_baseline(collidable: usize) -> Option<DimensionObjectBaseline> {
    let mut baseline = DimensionObjectBaseline::new(collidable, ER_HCL_COLLIDABLE_VTABLE_RVA)?;
    // hkTransform starts at +0x20 and stores translation after three rotation
    // columns, so its local translation is at hclCollidable+0x50.
    baseline.capture_vec3(collidable.checked_add(0x50)?, DimensionPower::Linear)?;
    let pinch_radius = read_f32(collidable.checked_add(0x98)?)?;
    if pinch_radius >= 0.0 {
        baseline.capture_field(collidable.checked_add(0x98)?, DimensionPower::Linear, true)?;
    }
    let _ = baseline.capture_pointer(collidable.checked_add(0x88)?)?;
    Some(baseline)
}

fn capture_shape_baseline(shape: usize) -> Option<DimensionObjectBaseline> {
    let vtable_rva = body_scale_port::module_rva(read_usize(shape)?);
    let mut baseline = DimensionObjectBaseline::new(shape, vtable_rva)?;
    match vtable_rva {
        ER_HCL_SPHERE_SHAPE_VTABLE_RVA => {
            // hkSphere stores center.xyz and radius in w.
            baseline.capture_vec4(shape.checked_add(0x20)?, DimensionPower::Linear)?;
        }
        ER_HCL_CAPSULE_SHAPE_VTABLE_RVA => {
            baseline.capture_vec3(shape.checked_add(0x20)?, DimensionPower::Linear)?;
            baseline.capture_vec3(shape.checked_add(0x30)?, DimensionPower::Linear)?;
            baseline.capture_field(shape.checked_add(0x50)?, DimensionPower::Linear, false)?;
            baseline.capture_field(
                shape.checked_add(0x54)?,
                DimensionPower::InverseSquared,
                false,
            )?;
        }
        ER_HCL_TAPERED_CAPSULE_SHAPE_VTABLE_RVA => {
            for offset in [0x20, 0x30, 0x40] {
                baseline.capture_vec3(shape.checked_add(offset)?, DimensionPower::Linear)?;
            }
            // lVec/dVec are scalar broadcasts for FOUR independent contacts,
            // not positions with an unused W. ER 15D19E5/15D19E9 use every lane
            // to select cone vs end caps. Keeping W at baseline makes lane 3
            // disagree with the other particles after either shrink or growth.
            for offset in [0x60, 0x70] {
                baseline.capture_vec4(shape.checked_add(offset)?, DimensionPower::Linear)?;
            }
            for offset in [0x90, 0x94, 0x98, 0x9C] {
                baseline.capture_field(
                    shape.checked_add(offset)?,
                    DimensionPower::Linear,
                    false,
                )?;
            }
        }
        ER_HCL_PLANE_SHAPE_VTABLE_RVA => {
            // A plane's normal xyz is dimensionless; only equation.w is a distance.
            baseline.capture_field(shape.checked_add(0x2C)?, DimensionPower::Linear, false)?;
        }
        ER_HCL_CONVEX_GEOMETRY_SHAPE_VTABLE_RVA => {
            capture_convex_geometry_dimensions(&mut baseline, shape)?;
        }
        _ => return None,
    }
    Some(baseline)
}

fn capture_convex_geometry_dimensions(
    baseline: &mut DimensionObjectBaseline,
    shape: usize,
) -> Option<()> {
    // ER reflection: tetrahedraGrid +20, gridCells +30, tetrahedraEquations +40.
    // 15CE950 uses ushort cell offsets and byte tetrahedron indices; 15CED00
    // handles the unpartitioned form. Observe indices without modifying them.
    baseline.capture_array(shape, 0x20, 0x28, 2, 65_536)?;
    baseline.capture_array(shape, 0x30, 0x38, 1, 65_536)?;
    let (equations, count) = baseline.capture_array(shape, 0x40, 0x48, 0x40, 65_535)?;
    for index in 0..count {
        // Four transposed plane equations: XYZ are unit normals, the final
        // vector contains FOUR distances. Scale every lane, not just XYZ.
        baseline.capture_vec4(
            equations.checked_add(index * 0x40 + 0x30)?,
            DimensionPower::Linear,
        )?;
    }
    // Local AABB and centroid scale as positions. Grid lookup computes
    // (position - aabb.min) * invCellSize, so its inverse dimensions divide by s.
    for offset in [0xD0, 0xE0, 0xF0] {
        baseline.capture_vec3(shape.checked_add(offset)?, DimensionPower::Linear)?;
    }
    baseline.capture_vec3(shape.checked_add(0x100)?, DimensionPower::Inverse)?;
    // 15CFC40 copies the shape then replaces localFromWorld/worldFromLocal
    // from the current collidable transform. Do not scale their rotation axes.
    Some(())
}

fn is_supported_shape_vtable(vtable_rva: usize) -> bool {
    matches!(
        vtable_rva,
        ER_HCL_CAPSULE_SHAPE_VTABLE_RVA
            | ER_HCL_TAPERED_CAPSULE_SHAPE_VTABLE_RVA
            | ER_HCL_SPHERE_SHAPE_VTABLE_RVA
            | ER_HCL_PLANE_SHAPE_VTABLE_RVA
            | ER_HCL_CONVEX_GEOMETRY_SHAPE_VTABLE_RVA
    )
}

fn scale_dimension_from_baseline(
    baseline: f32,
    scale: f32,
    power: DimensionPower,
    preserve_unbounded: bool,
) -> Option<f32> {
    if !baseline.is_finite() || !valid_scale(scale) {
        return None;
    }
    if preserve_unbounded && baseline.abs() >= f32::MAX * 0.5 {
        return Some(baseline);
    }
    if baseline == 0.0 {
        return Some(baseline);
    }
    let factor = match power {
        DimensionPower::Linear => scale,
        DimensionPower::Squared => scale * scale,
        DimensionPower::Inverse => scale.recip(),
        DimensionPower::InverseSquared => (scale * scale).recip(),
    };
    let direct = baseline * factor;
    if factor.is_normal() && crate::scale_math::representable(baseline, direct) {
        return Some(direct);
    }
    // Avoid false rejection when only s*s or 1/(s*s) exceeds f32.
    // Keep the established rounding for the ordinary finite fast path above.
    let wide = f64::from(scale);
    let factor = match power {
        DimensionPower::Linear => wide,
        DimensionPower::Squared => wide * wide,
        DimensionPower::Inverse => wide.recip(),
        DimensionPower::InverseSquared => (wide * wide).recip(),
    };
    let result = (f64::from(baseline) * factor) as f32;
    crate::scale_math::representable(baseline, result).then_some(result)
}

fn valid_scale(scale: f32) -> bool {
    crate::scale_math::valid(scale)
}

fn bounded_hk_array_span(
    object: usize,
    pointer_offset: usize,
    count_offset: usize,
    stride: usize,
    maximum: usize,
) -> Option<(usize, usize)> {
    if object == 0 || stride == 0 {
        return None;
    }
    let raw_count = read_i32(object.checked_add(count_offset)?)?;
    if raw_count < 0 {
        return None;
    }
    let count = usize::try_from(raw_count).ok()?;
    if count > maximum {
        return None;
    }
    let begin = read_usize(object.checked_add(pointer_offset)?)?;
    let bytes = count.checked_mul(stride)?;
    if count > 0 && (begin == 0 || !is_memory_accessible(begin, bytes, false)) {
        return None;
    }
    Some((begin, count))
}

fn read_i32(address: usize) -> Option<i32> {
    is_memory_accessible(address, size_of::<i32>(), false)
        .then(|| unsafe { (address as *const i32).read_unaligned() })
}

fn read_f32(address: usize) -> Option<f32> {
    is_memory_accessible(address, size_of::<f32>(), false)
        .then(|| unsafe { (address as *const f32).read_unaligned() })
}

fn read_usize(address: usize) -> Option<usize> {
    is_memory_accessible(address, size_of::<usize>(), false)
        .then(|| unsafe { (address as *const usize).read_unaligned() })
}

fn write_f32(address: usize, value: f32) {
    unsafe { (address as *mut f32).write_unaligned(value) };
}

fn is_memory_accessible(address: usize, length: usize, write: bool) -> bool {
    crate::memory_query::accessible_span(address, length, write)
}

fn accessible_memory_region(address: usize, length: usize, write: bool) -> Option<(usize, usize)> {
    crate::memory_query::accessible_region(address, length, write)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires private replay data; set ER_CHARACTER_SCALE_FIXTURES"]
    fn bd9004_tapered_collision_four_lane_scale_coherence() {
        let _module_guard = body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
        let mut mismatches = Vec::new();
        let mut shape_count = 0;
        for (index, line) in crate::test_fixtures::text("bd9004-tapered-shapes.txt")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.is_empty())
            .enumerate()
        {
            shape_count += 1;
            let bytes: Vec<u8> = line
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            assert_eq!(bytes.len(), 0x90);
            let mut shape = [0u64; 0xB0 / 8];
            shape[0] = ER_HCL_TAPERED_CAPSULE_SHAPE_VTABLE_RVA as u64;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    (shape.as_mut_ptr() as *mut u8).add(0x20),
                    bytes.len(),
                );
            }
            let original = shape;
            let address = shape.as_mut_ptr() as usize;
            let mut baseline = capture_shape_baseline(address).unwrap();
            for scale in [1.0f32, 0.5, 0.5, 3.0, 1.0] {
                assert!(baseline.apply(scale).is_some());
                if std::env::var_os("ERPS_TEST_DUMP_TAPERED").is_some() {
                    let raw =
                        unsafe { std::slice::from_raw_parts(shape.as_ptr() as *const u8, 0xB0) };
                    let hex: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
                    println!("[ERPS-TEST-TAPERED] index={index} scale={scale} raw={hex}");
                }
                // Native ER 15D1760 computes four independent particle contacts.
                // +0x60/+0x70 are scalar broadcasts, not spatial XYZ vectors.
                for (vector, scalar) in [(0x60, 0x98), (0x70, 0x9C)] {
                    let expected = read_f32(address + scalar).unwrap();
                    for lane in 0..4 {
                        let actual = read_f32(address + vector + lane * 4).unwrap();
                        if (actual - expected).abs() > 1e-6 {
                            mismatches.push((index, scale, vector, lane, actual, expected));
                        }
                    }
                }
                let mut expected = original;
                let expected_address = expected.as_mut_ptr() as usize;
                for offset in (0x20..0x2C)
                    .chain(0x30..0x3C)
                    .chain(0x40..0x4C)
                    .chain(0x60..0x80)
                    .chain(0x90..0xA0)
                    .step_by(4)
                {
                    let before = read_f32(original.as_ptr() as usize + offset).unwrap();
                    write_f32(expected_address + offset, before * scale);
                }
                // Includes all real spatial W lanes, axes, angles and padding.
                assert_eq!(
                    shape, expected,
                    "unexpected byte change on shape {index}, scale {scale}"
                );
            }
            assert_eq!(shape, original, "restoration must be byte exact");
        }
        assert_eq!(
            shape_count, 11,
            "all BD9004 tapered shapes must be exercised"
        );
        assert!(
            mismatches.is_empty(),
            "four-particle collision disagrees with scalar shape: {mismatches:?}"
        );
    }

    #[test]
    fn tapered_collision_rejects_nonfinite_fourth_lane_and_stale_identity() {
        let _module_guard = body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
        let mut shape = [0u64; 0xB0 / 8];
        shape[0] = ER_HCL_TAPERED_CAPSULE_SHAPE_VTABLE_RVA as u64;
        let address = shape.as_mut_ptr() as usize;
        write_f32(address + 0x6C, f32::NAN);
        let untouched = shape;
        assert!(capture_shape_baseline(address).is_none());
        assert_eq!(shape, untouched);
        write_f32(address + 0x6C, 2.0);
        let mut baseline = capture_shape_baseline(address).unwrap();
        assert!(baseline.apply(0.5).is_some());
        assert_eq!(read_f32(address + 0x6C), Some(1.0));
        shape[0] = 0;
        let stale = shape;
        assert!(baseline.apply(1.0).is_none());
        assert_eq!(shape, stale, "stale shape may not receive even a restore");
    }

    #[test]
    fn released_nonfield_gap_does_not_disable_live_dimension_arrays() {
        use windows::Win32::System::Memory::{
            MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc,
            VirtualFree,
        };
        struct TestAllocation(*mut core::ffi::c_void);
        impl Drop for TestAllocation {
            fn drop(&mut self) {
                unsafe { VirtualFree(self.0, 0, MEM_RELEASE).unwrap() };
            }
        }
        let allocation = TestAllocation(unsafe {
            VirtualAlloc(None, 0x3000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE)
        });
        assert!(!allocation.0.is_null());
        let left = allocation.0 as usize;
        let right = left + 0x2000;
        write_f32(left, 2.0);
        write_f32(right, 6.0);
        let mut baseline = empty_test_baseline(left);
        baseline
            .capture_field(left, DimensionPower::Linear, false)
            .unwrap();
        baseline
            .capture_field(right, DimensionPower::Linear, false)
            .unwrap();
        assert_eq!(baseline.field_spans.len(), 1);
        // Owned test allocation only: remove the unrelated middle page,
        // leaving both real dimension fields writable and at the same address.
        unsafe { VirtualFree((left + 0x1000) as *mut _, 0x1000, MEM_DECOMMIT).unwrap() };
        assert_eq!(
            baseline.apply(0.5),
            Some(2),
            "nonfield hole must not reject valid cloth fields"
        );
        assert_eq!(read_f32(left), Some(1.0));
        assert_eq!(read_f32(right), Some(3.0));
        assert_eq!(baseline.field_spans.len(), 2);
        assert_eq!(baseline.apply(1.0), Some(2));
        // Negative control: an actual field disappearing still rejects the
        // entire update BEFORE any surviving field is changed.
        unsafe { VirtualFree(right as *mut _, 0x1000, MEM_DECOMMIT).unwrap() };
        assert_eq!(baseline.apply(3.0), None);
        assert_eq!(read_f32(left), Some(2.0));
    }

    #[test]
    fn empty_collision_map_does_not_drop_simulation_dimensions() {
        let _module_guard = body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
        // Minimized live BD_M_1690: valid authored particle/pose/link arrays,
        // but no collidables and transform-set sentinel -1. Exercise the full
        // production capture/apply path, not just a standalone index predicate.
        let mut particles = [0.0f32, 0.0, 2.0, 0.0];
        let mut positions = [2.0f32, -4.0, 6.0, 0.0];
        let mut pose = [0usize; 0x30 / 8];
        pose[0] = 0x2D8B6F8;
        pose[0x20 / 8] = positions.as_mut_ptr() as usize;
        pose[0x28 / 8] = 1;
        let poses = [pose.as_ptr() as usize];
        let mut link = [0u32, 4.0f32.to_bits(), 0];
        let mut constraint = [0usize; 0x38 / 8];
        constraint[0] = 0x2D89D58;
        constraint[0x28 / 8] = link.as_mut_ptr() as usize;
        constraint[0x30 / 8] = 1;
        let constraints = [constraint.as_ptr() as usize];
        let mut sim = vec![0usize; 0x1C0 / 8];
        sim[0] = ER_HCL_SIM_CLOTH_DATA_VTABLE_RVA;
        sim[0x40 / 8] = particles.as_mut_ptr() as usize;
        sim[0x48 / 8] = 1;
        sim[0x78 / 8] = poses.as_ptr() as usize;
        sim[0x80 / 8] = 1;
        sim[0x88 / 8] = constraints.as_ptr() as usize;
        sim[0x90 / 8] = 1;
        sim[0xA8 / 8] = u32::MAX as usize;
        let mut baseline = capture_simulation_baseline(sim.as_ptr() as usize)
            .expect("empty collision map must not discard cloth size baseline");
        for scale in [0.5f32, 0.5, 3.0, 1.0] {
            assert!(baseline.apply(scale).is_some());
            assert_eq!(particles[2], 2.0 * scale);
            assert_eq!(positions, [2.0 * scale, -4.0 * scale, 6.0 * scale, 0.0]);
            assert_eq!(f32::from_bits(link[1]), 4.0 * scale);
        }
        // An empty map becoming nonempty invalidates the captured topology.
        sim[0xB8 / 8] = 1;
        assert_eq!(baseline.apply(0.5), None);
        assert_eq!(particles[2], 2.0);
    }

    #[test]
    fn empty_collision_map_does_not_admit_malformed_maps() {
        let mut sim = vec![0usize; 0xE0 / 8];
        for (index, expected_count, indices, offsets) in [
            (-2i32, 0, 0, 0),
            (257, 0, 0, 0),
            (-1, 1, 0, 0),
            (-1, 0, 1, 0),
            (-1, 0, 0, 1),
            (0, 1, 0, 0),
        ] {
            sim[0xA8 / 8] = index as u32 as usize;
            sim[0xB8 / 8] = indices;
            sim[0xC8 / 8] = offsets;
            let address = sim.as_ptr() as usize;
            let mut baseline = empty_test_baseline(address);
            assert_eq!(
                capture_collidable_transform_map_dimensions(
                    &mut baseline,
                    address,
                    expected_count,
                    false
                ),
                None
            );
        }
    }

    fn volume_fixture_arrays(text: &str) -> Vec<Vec<u8>> {
        text.split('\n')
            .take(4)
            .map(|line| {
                line.trim()
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect()
            })
            .collect()
    }

    #[test]
    #[ignore = "requires private replay data; set ER_CHARACTER_SCALE_FIXTURES"]
    fn volume_constraint_frozen_arrays_follow_native_scale_equivariance() {
        let _module_guard = body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
        for fixture in [
            crate::test_fixtures::text("volume-2.27/BD_M_1690-volume-arrays.txt"),
            crate::test_fixtures::text("volume-2.27/HR_A_0106-volume-arrays.txt"),
        ] {
            let mut arrays = volume_fixture_arrays(&fixture);
            let original = arrays.clone();
            let mut set = [0usize; 0x68 / 8];
            set[0] = 0x2D88C18;
            for (i, array) in arrays.iter_mut().enumerate() {
                let stride = if i % 2 == 0 { 0x160 } else { 0x20 };
                set[(0x28 + i * 16) / 8] = array.as_mut_ptr() as usize;
                set[(0x30 + i * 16) / 8] = array.len() / stride;
            }
            let mut baseline = empty_test_baseline(set.as_ptr() as usize);
            capture_constraint_dimensions(&mut baseline, set.as_ptr() as usize).unwrap();
            assert_eq!(
                baseline.unsupported_layouts, 0,
                "active volume constraint must be supported"
            );
            assert!(!baseline.fields.is_empty());
            for scale in [0.5f32, 0.5, 3.0, 1.0] {
                assert!(
                    baseline.apply(scale).is_some(),
                    "volume apply rejected scale={scale} identity={} inaccessible_spans={:?}",
                    baseline.identity_matches(),
                    baseline
                        .field_spans
                        .iter()
                        .filter(|span| !is_memory_accessible(
                            span.used_start,
                            span.used_end - span.used_start,
                            true
                        ))
                        .map(|span| (
                            span.used_start,
                            span.used_end,
                            accessible_memory_region(span.used_start, 4, true)
                        ))
                        .collect::<Vec<_>>()
                );
                for (i, array) in arrays.iter().enumerate() {
                    let stride = if i % 2 == 0 { 0x160 } else { 0x20 };
                    let lanes = if i % 2 == 0 { 16 } else { 1 };
                    let mut expected = original[i].clone();
                    for block in 0..array.len() / stride {
                        for lane in 0..lanes {
                            for component in 0..3 {
                                let off = block * stride + lane * 16 + component * 4;
                                let q = f32::from_le_bytes(
                                    original[i][off..off + 4].try_into().unwrap(),
                                );
                                let factor = if i < 2 { scale.recip() } else { scale };
                                expected[off..off + 4].copy_from_slice(&(q * factor).to_le_bytes());
                                let actual =
                                    f32::from_le_bytes(array[off..off + 4].try_into().unwrap());
                                if i < 2 {
                                    // Native cross moment (p-c)*q*w must be invariant
                                    // when p and c scale. Preserves its nonlinear solver input.
                                    let old = 0.375f64 * q as f64;
                                    let new = 0.375 * scale as f64 * actual as f64;
                                    assert!((old - new).abs() <= 1e-6 * (1.0 + old.abs()));
                                } else {
                                    // Native reconstructed target R*q+c must scale by s.
                                    let target = 1.25f64 * actual as f64 + 2.0 * scale as f64;
                                    let expected_target = (1.25 * q as f64 + 2.0) * scale as f64;
                                    assert!((target - expected_target).abs() < 1e-6);
                                }
                            }
                        }
                    }
                    // Includes W, particle indices, stiffness/weight and all padding.
                    assert_eq!(array, &expected);
                }
            }
            assert_eq!(arrays, original);
            set[0x30 / 8] += 1;
            std::hint::black_box(&set);
            assert_eq!(baseline.apply(0.5), None);
            assert_eq!(arrays, original);
        }
    }

    #[test]
    fn volume_constraint_rejects_invalid_arrays_before_writes() {
        let _module_guard = body_scale_port::MODULE_TEST_LOCK.lock().unwrap();
        let mut values = [f32::NAN, 1.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let mut set = [0usize; 0x68 / 8];
        set[0] = 0x2D88C18;
        set[0x38 / 8] = values.as_mut_ptr() as usize;
        set[0x40 / 8] = 1;
        let mut baseline = empty_test_baseline(set.as_ptr() as usize);
        assert_eq!(
            capture_constraint_dimensions(&mut baseline, set.as_ptr() as usize),
            None
        );
        assert!(values[0].is_nan());
        assert_eq!(values[1], 1.0);
        set[0x40 / 8] = MAX_CONSTRAINT_ELEMENTS + 1;
        let mut baseline = empty_test_baseline(set.as_ptr() as usize);
        assert_eq!(
            capture_constraint_dimensions(&mut baseline, set.as_ptr() as usize),
            None
        );
    }

    #[test]
    fn cancelling_partial_dimension_transition_restores_written_prefix() {
        let mut values = [1.0f32, 2.0, 3.0];
        let mut baseline = empty_test_baseline(values.as_ptr() as usize);
        for value in &mut values {
            baseline
                .capture_field(value as *mut f32 as usize, DimensionPower::Linear, false)
                .unwrap();
        }
        assert_eq!(baseline.apply_budgeted(0.5, 1), Some((1, false)));
        assert_eq!(values, [0.5, 2.0, 3.0]);
        assert!(baseline.apply(1.0).is_some());
        assert_eq!(values, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn collidable_transform_map_scales_only_offset_translations() {
        let mut transform_indices = [3u32, 8u32];
        let mut offsets = [
            [
                1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, -4.0, 6.0, 1.0,
            ],
            [
                0.0f32, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -3.0, 5.0, 7.0, 1.0,
            ],
        ];
        let mut sim_data = vec![0u8; 0xE0];
        unsafe {
            (sim_data.as_mut_ptr().add(0xB0) as *mut usize)
                .write_unaligned(transform_indices.as_mut_ptr() as usize);
            (sim_data.as_mut_ptr().add(0xB8) as *mut i32)
                .write_unaligned(transform_indices.len() as i32);
            (sim_data.as_mut_ptr().add(0xC0) as *mut usize)
                .write_unaligned(offsets.as_mut_ptr() as usize);
            (sim_data.as_mut_ptr().add(0xC8) as *mut i32).write_unaligned(offsets.len() as i32);
        }

        let mut baseline = DimensionObjectBaseline {
            address: sim_data.as_ptr() as usize,
            roots: Vec::new(),
            pointers: Vec::new(),
            arrays: Vec::new(),
            field_spans: Vec::new(),
            field_kinds: HashMap::new(),
            fields: Vec::new(),
            numeric_bounds: [DimensionBounds::default(); 4],
            applied_scale_bits: 1.0f32.to_bits(),
            pending_scale_bits: 0,
            next_field: 0,
            unsupported_layouts: 0,
            collidable_map_entries: 0,
            collidable_offset_fields: 0,
            virtual_collision_blocks: 0,
            virtual_collision_radius_fields: 0,
            active_update_failed: false,
        };
        let entries = capture_collidable_transform_map_dimensions(
            &mut baseline,
            sim_data.as_ptr() as usize,
            2,
            true,
        )
        .unwrap();

        assert_eq!(entries, 2);
        assert_eq!(baseline.fields.len(), 6);
        assert_eq!(baseline.apply(0.5), Some(6));
        assert_eq!(
            &offsets[0][0..12],
            &[1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        );
        assert_eq!(&offsets[0][12..16], &[1.0, -2.0, 3.0, 1.0]);
        assert_eq!(&offsets[1][12..16], &[-1.5, 2.5, 3.5, 1.0]);
        assert_eq!(baseline.apply(1.0), Some(6));
        assert_eq!(&offsets[0][12..16], &[2.0, -4.0, 6.0, 1.0]);
        assert_eq!(&offsets[1][12..16], &[-3.0, 5.0, 7.0, 1.0]);
    }

    #[test]
    fn virtual_collision_points_scale_only_safe_displacement_radius() {
        let mut blocks = [
            [0.25f32, f32::from_bits(0x0003_0001)],
            [0.75f32, f32::from_bits(0x0005_0004)],
        ];
        let mut sim_data = vec![0u8; 0x1C0];
        unsafe {
            (sim_data.as_mut_ptr().add(0x1B0) as *mut usize)
                .write_unaligned(blocks.as_mut_ptr() as usize);
            (sim_data.as_mut_ptr().add(0x1B8) as *mut i32).write_unaligned(blocks.len() as i32);
        }

        let mut baseline = DimensionObjectBaseline {
            address: sim_data.as_ptr() as usize,
            roots: Vec::new(),
            pointers: Vec::new(),
            arrays: Vec::new(),
            field_spans: Vec::new(),
            field_kinds: HashMap::new(),
            fields: Vec::new(),
            numeric_bounds: [DimensionBounds::default(); 4],
            applied_scale_bits: 1.0f32.to_bits(),
            pending_scale_bits: 0,
            next_field: 0,
            unsupported_layouts: 0,
            collidable_map_entries: 0,
            collidable_offset_fields: 0,
            virtual_collision_blocks: 0,
            virtual_collision_radius_fields: 0,
            active_update_failed: false,
        };
        let count = capture_virtual_collision_point_dimensions(
            &mut baseline,
            sim_data.as_ptr() as usize,
            true,
        )
        .unwrap();

        assert_eq!(count, 2);
        assert_eq!(baseline.fields.len(), 2);
        assert_eq!(baseline.apply(0.5), Some(2));
        assert_eq!(blocks[0][0], 0.125);
        assert_eq!(blocks[1][0], 0.375);
        assert_eq!(blocks[0][1].to_bits(), 0x0003_0001);
        assert_eq!(blocks[1][1].to_bits(), 0x0005_0004);
        assert_eq!(baseline.apply(1.0), Some(2));
        assert_eq!(blocks[0][0], 0.25);
        assert_eq!(blocks[1][0], 0.75);
    }

    #[test]
    fn collidable_transform_map_observation_preserves_offsets() {
        let mut transform_indices = [3u32];
        let mut offsets = [[
            1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, -4.0, 6.0, 1.0,
        ]];
        let expected = offsets;
        let mut sim_data = vec![0u8; 0xE0];
        unsafe {
            (sim_data.as_mut_ptr().add(0xB0) as *mut usize)
                .write_unaligned(transform_indices.as_mut_ptr() as usize);
            (sim_data.as_mut_ptr().add(0xB8) as *mut i32).write_unaligned(1);
            (sim_data.as_mut_ptr().add(0xC0) as *mut usize)
                .write_unaligned(offsets.as_mut_ptr() as usize);
            (sim_data.as_mut_ptr().add(0xC8) as *mut i32).write_unaligned(1);
        }
        let mut baseline = empty_test_baseline(sim_data.as_ptr() as usize);

        assert_eq!(
            capture_collidable_transform_map_dimensions(
                &mut baseline,
                sim_data.as_ptr() as usize,
                1,
                false,
            ),
            Some(1)
        );
        assert!(baseline.fields.is_empty());
        assert_eq!(baseline.apply(0.5), Some(0));
        assert_eq!(offsets, expected);
    }

    #[test]
    fn virtual_collision_observation_preserves_safe_radius() {
        let mut blocks = [[0.25f32, f32::from_bits(0x0003_0001)]];
        let expected = blocks;
        let mut sim_data = vec![0u8; 0x1C0];
        unsafe {
            (sim_data.as_mut_ptr().add(0x1B0) as *mut usize)
                .write_unaligned(blocks.as_mut_ptr() as usize);
            (sim_data.as_mut_ptr().add(0x1B8) as *mut i32).write_unaligned(1);
        }
        let mut baseline = empty_test_baseline(sim_data.as_ptr() as usize);

        assert_eq!(
            capture_virtual_collision_point_dimensions(
                &mut baseline,
                sim_data.as_ptr() as usize,
                false,
            ),
            Some(1)
        );
        assert!(baseline.fields.is_empty());
        assert_eq!(baseline.apply(0.5), Some(0));
        assert_eq!(blocks, expected);
    }

    fn empty_test_baseline(address: usize) -> DimensionObjectBaseline {
        DimensionObjectBaseline {
            address,
            roots: Vec::new(),
            pointers: Vec::new(),
            arrays: Vec::new(),
            field_spans: Vec::new(),
            field_kinds: HashMap::new(),
            fields: Vec::new(),
            numeric_bounds: [DimensionBounds::default(); 4],
            applied_scale_bits: 1.0f32.to_bits(),
            pending_scale_bits: 0,
            next_field: 0,
            unsupported_layouts: 0,
            collidable_map_entries: 0,
            collidable_offset_fields: 0,
            virtual_collision_blocks: 0,
            virtual_collision_radius_fields: 0,
            active_update_failed: false,
        }
    }

    #[test]
    fn scales_cloth_dimensions_absolutely_from_their_baseline() {
        assert_eq!(
            scale_dimension_from_baseline(2.0, 0.5, DimensionPower::Linear, false),
            Some(1.0)
        );
        assert_eq!(
            scale_dimension_from_baseline(2.0, 3.0, DimensionPower::Linear, false),
            Some(6.0)
        );
        assert_eq!(
            scale_dimension_from_baseline(2.0, 1.0, DimensionPower::Linear, false),
            Some(2.0)
        );
    }

    #[test]
    fn applies_the_correct_length_power_to_cached_cloth_values() {
        assert_eq!(
            scale_dimension_from_baseline(4.0, 0.5, DimensionPower::Squared, false),
            Some(1.0)
        );
        assert_eq!(
            scale_dimension_from_baseline(4.0, 0.5, DimensionPower::Inverse, false),
            Some(8.0)
        );
        assert_eq!(
            scale_dimension_from_baseline(4.0, 0.5, DimensionPower::InverseSquared, false),
            Some(16.0)
        );
    }

    #[test]
    fn preserves_unbounded_local_range_sentinels() {
        assert_eq!(
            scale_dimension_from_baseline(f32::MAX, 0.5, DimensionPower::Linear, true,),
            Some(f32::MAX)
        );
    }

    #[test]
    fn dimension_batches_accept_wide_transitions_and_reject_before_first_write() {
        let mut values = [2.0f32, 3.0, 4.0, 5.0, f32::MAX];
        let original = values;
        let powers = [
            DimensionPower::Linear,
            DimensionPower::Squared,
            DimensionPower::Inverse,
            DimensionPower::InverseSquared,
            DimensionPower::Linear,
        ];
        let mut object = empty_test_baseline(values.as_ptr() as usize);
        for (i, power) in powers.into_iter().enumerate() {
            object
                .capture_field(values.as_mut_ptr() as usize + i * 4, power, i == 4)
                .unwrap();
        }
        for scale in [0.1f32, 10.0, 0.25, 4.0, 1.0] {
            assert_eq!(object.apply(scale), Some(values.len()));
            for i in 0..4 {
                let s = f64::from(scale);
                let factor = match powers[i] {
                    DimensionPower::Linear => s,
                    DimensionPower::Squared => s * s,
                    DimensionPower::Inverse => 1.0 / s,
                    DimensionPower::InverseSquared => 1.0 / (s * s),
                };
                assert!(
                    (f64::from(values[i]) / (f64::from(original[i]) * factor) - 1.0).abs() < 0.0001
                );
            }
            assert_eq!(values[4], f32::MAX);
        }
        assert_eq!(values, original);
        for extreme in [f32::MAX, f32::from_bits(1)] {
            assert_eq!(object.apply_budgeted(extreme, 1), None);
            assert_eq!(values, original, "no partial write on numeric failure");
            assert_eq!(object.pending_scale_bits, 0);
        }
        assert_eq!(
            object.apply(0.25),
            Some(5),
            "numeric failures must not latch layout rejection"
        );
        assert_eq!(object.apply(1.0), Some(5));
        assert_eq!(values, original);
    }

    #[test]
    fn dimensions_with_extreme_intermediates_use_representable_final_values() {
        for (baseline, scale, power) in [
            (1e-20f32, 1e20f32, DimensionPower::Squared),
            (1e20, 1e20, DimensionPower::InverseSquared),
            (1e-20, 1e-20, DimensionPower::InverseSquared),
        ] {
            let value = scale_dimension_from_baseline(baseline, scale, power, false).unwrap();
            let s = f64::from(scale);
            let expected = f64::from(baseline)
                * if power == DimensionPower::Squared {
                    s * s
                } else {
                    1.0 / (s * s)
                };
            assert!((f64::from(value) / expected - 1.0).abs() < 0.0001);
        }
        assert_eq!(
            scale_dimension_from_baseline(0.0, f32::MAX, DimensionPower::Squared, false),
            Some(0.0)
        );
        assert_eq!(
            scale_dimension_from_baseline(f32::from_bits(1), 0.1, DimensionPower::Linear, false),
            None
        );
    }

    #[test]
    fn baseline_object_transitions_do_not_compound_and_restore_exactly() {
        let mut value = 2.0f32;
        let mut baseline = DimensionObjectBaseline {
            address: 0x1000,
            roots: Vec::new(),
            pointers: Vec::new(),
            arrays: Vec::new(),
            field_spans: Vec::new(),
            field_kinds: HashMap::new(),
            fields: Vec::new(),
            numeric_bounds: [DimensionBounds::default(); 4],
            applied_scale_bits: 1.0f32.to_bits(),
            pending_scale_bits: 0,
            next_field: 0,
            unsupported_layouts: 0,
            collidable_map_entries: 0,
            collidable_offset_fields: 0,
            virtual_collision_blocks: 0,
            virtual_collision_radius_fields: 0,
            active_update_failed: false,
        };
        baseline
            .capture_field(
                &mut value as *mut f32 as usize,
                DimensionPower::Linear,
                false,
            )
            .unwrap();
        assert_eq!(baseline.field_spans.len(), 1);

        assert_eq!(baseline.apply(0.5), Some(1));
        assert_eq!(value, 1.0);
        assert_eq!(baseline.apply(0.5), Some(0));
        assert_eq!(value, 1.0);
        assert_eq!(baseline.apply(3.0), Some(1));
        assert_eq!(value, 6.0);
        assert_eq!(baseline.apply(1.0), Some(1));
        assert_eq!(value, 2.0);
    }

    #[test]
    fn baseline_object_applies_a_scale_transition_across_bounded_field_chunks() {
        let mut values = [1.0f32, 2.0, 3.0];
        let mut baseline = DimensionObjectBaseline {
            address: 0x1000,
            roots: Vec::new(),
            pointers: Vec::new(),
            arrays: Vec::new(),
            field_spans: Vec::new(),
            field_kinds: HashMap::new(),
            fields: Vec::new(),
            numeric_bounds: [DimensionBounds::default(); 4],
            applied_scale_bits: 1.0f32.to_bits(),
            pending_scale_bits: 0,
            next_field: 0,
            unsupported_layouts: 0,
            collidable_map_entries: 0,
            collidable_offset_fields: 0,
            virtual_collision_blocks: 0,
            virtual_collision_radius_fields: 0,
            active_update_failed: false,
        };
        for value in &mut values {
            baseline
                .capture_field(value as *mut f32 as usize, DimensionPower::Linear, false)
                .unwrap();
        }

        let first = baseline.apply_budgeted(0.5, 2).unwrap();
        assert_eq!(first, (2, false));
        assert_eq!(values, [0.5, 1.0, 3.0]);

        let second = baseline.apply_budgeted(0.5, 2).unwrap();
        assert_eq!(second, (1, true));
        assert_eq!(values, [0.5, 1.0, 1.5]);
        assert_eq!(baseline.apply_budgeted(0.5, 2), Some((0, true)));
    }

    #[test]
    fn transient_capture_failure_gets_one_bounded_retry_per_epoch() {
        let mut rejected = Vec::new();

        assert!(capture_attempt_allowed(&rejected, 0x1234));
        assert!(record_capture_rejection(&mut rejected, 0x1234));
        assert!(capture_attempt_allowed(&rejected, 0x1234));

        assert!(!record_capture_rejection(&mut rejected, 0x1234));
        assert!(!capture_attempt_allowed(&rejected, 0x1234));

        resolve_capture_rejection(&mut rejected, 0x1234);
        assert!(capture_attempt_allowed(&rejected, 0x1234));
    }
}
