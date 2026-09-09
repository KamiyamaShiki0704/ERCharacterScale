use crate::{body_scale_port, log};

const PARTICLES_AABB_OFFSET: usize = 0x60;
const COLLISION_PARTICLES_AABB_OFFSET: usize = 0x80;
const LANDSCAPE_COLLISION_PARTICLES_AABB_OFFSET: usize = 0xB0;
const MIN_MEANINGFUL_SPAN: f32 = 1.0e-4;
const MAX_PADDING_COMPONENT: f32 = 100.0;
// ER can keep retired cloth generations in the same core after an equipment
// or scale transition.  Their child transforms may still carry the requested
// basis even though their particles are frozen at an old world position.  A
// live clothing child is spatially close to the instance's current main root.
const MAX_CHILD_ROOT_DISTANCE_PER_SCALE: f32 = 2.5;
const MAX_CHILD_EXTENT_DISTANCE_FACTOR: f32 = 3.0;

#[derive(Clone, Copy, Debug)]
struct AabbPadding {
    minimum: [f32; 3],
    maximum: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
struct ClothInstanceAabbBaseline {
    child: usize,
    child_vtable: usize,
    sim_data: usize,
    collision_padding: AabbPadding,
    landscape_padding: Option<AabbPadding>,
}

impl ClothInstanceAabbBaseline {
    fn identity_matches(&self, child: &body_scale_port::ClothChildSnapshot) -> bool {
        self.child == child.child
            && self.child_vtable == child.child_vtable
            && self.sim_data == child.sim_data
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClothInstanceAabbRetry {
    child: usize,
    child_vtable: usize,
    sim_data: usize,
}

impl ClothInstanceAabbRetry {
    fn from_child(child: &body_scale_port::ClothChildSnapshot) -> Self {
        Self {
            child: child.child,
            child_vtable: child.child_vtable,
            sim_data: child.sim_data,
        }
    }

    fn identity_matches(&self, child: &body_scale_port::ClothChildSnapshot) -> bool {
        *self == Self::from_child(child)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RejectionStage {
    Baseline,
    Target,
    Write,
}

#[derive(Default)]
pub struct ClothInstanceAabbScaleState {
    target_input: usize,
    last_requested_scale_bits: u32,
    baselines: Vec<ClothInstanceAabbBaseline>,
    immediate_retries: Vec<ClothInstanceAabbRetry>,
    last_report: Option<(u32, ClothInstanceAabbScaleResult)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClothInstanceAabbScaleResult {
    pub topology_readable: bool,
    pub instances: usize,
    pub children: usize,
    pub active_children: usize,
    pub captured_children: usize,
    pub dropped_children: usize,
    pub aabbs_written: usize,
    pub fields_written: usize,
    pub already_current: usize,
    pub non_target_generation_children: usize,
    pub detached_children: usize,
    pub native_managed_children: usize,
    pub rejected_children: usize,
    pub baseline_rejected_children: usize,
    pub target_rejected_children: usize,
    pub write_rejected_children: usize,
    pub deferred_rejected_children: usize,
    pub work_pending: bool,
}

pub fn rebuild_target_cloth_instance_aabbs(
    target_input: usize,
    requested_scale: f32,
    state: &mut ClothInstanceAabbScaleState,
) -> ClothInstanceAabbScaleResult {
    crate::memory_query::scoped(|| {
        rebuild_target_cloth_instance_aabbs_inner(target_input, requested_scale, state)
    })
}

fn rebuild_target_cloth_instance_aabbs_inner(
    target_input: usize,
    requested_scale: f32,
    state: &mut ClothInstanceAabbScaleState,
) -> ClothInstanceAabbScaleResult {
    if target_input == 0 || !valid_scale(requested_scale) {
        clear_cloth_instance_aabb_state(state);
        return ClothInstanceAabbScaleResult::default();
    }
    if state.target_input != 0 && state.target_input != target_input {
        clear_cloth_instance_aabb_state(state);
    }
    state.target_input = target_input;

    let requested_bits = requested_scale.to_bits();
    if state.last_requested_scale_bits != requested_bits {
        state.last_requested_scale_bits = requested_bits;
        state.immediate_retries.clear();
    }
    let instances = body_scale_port::cloth_instance_snapshots(target_input);
    let mut result = ClothInstanceAabbScaleResult::default();
    let mut live_children = Vec::new();

    for instance in instances.iter().filter(|instance| instance.valid) {
        result.instances += 1;
        let instance_is_active =
            instance.applied_scale_bits == requested_bits && instance.pending_scale_bits == 0;

        let (summary, children) = body_scale_port::cloth_child_snapshots(instance.core);
        if !summary.topology_readable || summary.truncated {
            result.rejected_children += 1;
            result.work_pending = true;
            continue;
        }
        result.topology_readable = true;

        for child in children.iter().filter(|child| child.valid) {
            result.children += 1;
            live_children.push((child.child, child.child_vtable, child.sim_data));

            if !instance_is_active {
                if child.transform_entries.readable && child.transform_entries.count > 0 {
                    result.non_target_generation_children += 1;
                }
                continue;
            }

            // The transform basis is not a generation discriminator: the
            // returned ER trace showed one requested-basis child frozen in
            // world space while unit-basis children followed the player.  Use
            // the current particle center relative to the live instance root
            // so retired generations cannot receive collision-cache writes.
            if !child_is_attached_to_instance_root(
                instance.matrix_60,
                child.current_positions,
                requested_scale,
            ) {
                result.detached_children += 1;
                continue;
            }
            result.active_children += 1;

            let profile =
                body_scale_port::cloth_local_simulation_profile(child.child, child.sim_data);
            if !profile.collision_particles_aabb.readable {
                // Active ER children without this optional cache already have
                // a native particle AABB centered exactly on their live
                // particles.  Do not manufacture a collision AABB for a
                // different simulation mode.
                result.native_managed_children += 1;
                resolve_retry(state, child);
                continue;
            }

            let baseline_index = state
                .baselines
                .iter()
                .position(|baseline| baseline.identity_matches(child));
            let baseline_index = match baseline_index {
                Some(index) => index,
                None => {
                    let Some(baseline) = capture_baseline(child, profile, requested_scale) else {
                        record_rejection(state, child, RejectionStage::Baseline, &mut result);
                        continue;
                    };
                    state.baselines.push(baseline);
                    result.captured_children += 1;
                    state.baselines.len() - 1
                }
            };
            let baseline = state.baselines[baseline_index];
            let Some(targets) = target_aabbs(child.current_positions, baseline, requested_scale)
            else {
                record_rejection(state, child, RejectionStage::Target, &mut result);
                continue;
            };

            let current = [
                Some(profile.particles_aabb),
                Some(profile.collision_particles_aabb),
                baseline
                    .landscape_padding
                    .map(|_| profile.landscape_collision_particles_aabb),
            ];
            let changed = current
                .iter()
                .zip(targets.iter())
                .filter(|(before, after)| {
                    matches!((before, after), (Some(before), Some(after)) if !bounds_nearly_equal(*before, *after))
                })
                .count();
            if changed == 0 {
                result.already_current += 1;
                resolve_retry(state, child);
                continue;
            }

            match body_scale_port::write_cloth_instance_aabbs(
                child.child,
                child.child_vtable,
                child.sim_data,
                [
                    (PARTICLES_AABB_OFFSET, targets[0]),
                    (COLLISION_PARTICLES_AABB_OFFSET, targets[1]),
                    (LANDSCAPE_COLLISION_PARTICLES_AABB_OFFSET, targets[2]),
                ],
            ) {
                Some(writes) => {
                    result.aabbs_written += writes;
                    result.fields_written += writes * 6;
                    resolve_retry(state, child);
                }
                None => {
                    record_rejection(state, child, RejectionStage::Write, &mut result);
                }
            }
        }
    }

    let before = state.baselines.len();
    state.baselines.retain(|baseline| {
        live_children.contains(&(baseline.child, baseline.child_vtable, baseline.sim_data))
    });
    state
        .immediate_retries
        .retain(|retry| live_children.contains(&(retry.child, retry.child_vtable, retry.sim_data)));
    result.dropped_children = before.saturating_sub(state.baselines.len());
    report_result(requested_scale, state, result);
    result
}

fn record_rejection(
    state: &mut ClothInstanceAabbScaleState,
    child: &body_scale_port::ClothChildSnapshot,
    stage: RejectionStage,
    result: &mut ClothInstanceAabbScaleResult,
) {
    result.rejected_children += 1;
    match stage {
        RejectionStage::Baseline => result.baseline_rejected_children += 1,
        RejectionStage::Target => result.target_rejected_children += 1,
        RejectionStage::Write => result.write_rejected_children += 1,
    }

    // A target calculation that fails against an existing identity baseline
    // depends on the live solver buffer changing. Re-reading the same buffer
    // every frame cannot repair it, so leave it to the bounded audit. Baseline
    // and guarded-write failures get one immediate retry per unresolved child.
    let retryable = stage != RejectionStage::Target;
    let retry = ClothInstanceAabbRetry::from_child(child);
    if retryable && !state.immediate_retries.contains(&retry) {
        state.immediate_retries.push(retry);
        result.work_pending = true;
    } else {
        result.deferred_rejected_children += 1;
    }
}

fn resolve_retry(
    state: &mut ClothInstanceAabbScaleState,
    child: &body_scale_port::ClothChildSnapshot,
) {
    state
        .immediate_retries
        .retain(|retry| !retry.identity_matches(child));
}

pub fn clear_cloth_instance_aabb_state(state: &mut ClothInstanceAabbScaleState) {
    if !state.baselines.is_empty() {
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-INSTANCE-AABB-CLEAR] target=0x{:X} cached_children={}",
            state.target_input,
            state.baselines.len(),
        ));
    }
    *state = ClothInstanceAabbScaleState::default();
}

fn capture_baseline(
    child: &body_scale_port::ClothChildSnapshot,
    profile: body_scale_port::ClothLocalSimulationProfile,
    requested_scale: f32,
) -> Option<ClothInstanceAabbBaseline> {
    if child.child == 0
        || child.child_vtable == 0
        || child.sim_data == 0
        || child.particle_count == 0
        || !child.current_positions.readable
        || !profile.particles_aabb.readable
        || !profile.collision_particles_aabb.readable
    {
        return None;
    }

    let stored_scale = infer_stored_aabb_scale(
        child.current_positions,
        profile.particles_aabb,
        requested_scale,
    )?;
    let collision_padding = capture_padding(
        profile.particles_aabb,
        profile.collision_particles_aabb,
        stored_scale,
    )?;
    let landscape_padding = profile
        .landscape_collision_particles_aabb
        .readable
        .then(|| {
            capture_padding(
                profile.particles_aabb,
                profile.landscape_collision_particles_aabb,
                stored_scale,
            )
        })
        .flatten();

    Some(ClothInstanceAabbBaseline {
        child: child.child,
        child_vtable: child.child_vtable,
        sim_data: child.sim_data,
        collision_padding,
        landscape_padding,
    })
}

fn infer_stored_aabb_scale(
    positions: body_scale_port::ClothPositionBounds,
    particles_aabb: body_scale_port::ClothPositionBounds,
    requested_scale: f32,
) -> Option<f32> {
    let position_spans = [positions.span_x(), positions.span_y(), positions.span_z()];
    let aabb_spans = [
        particles_aabb.span_x(),
        particles_aabb.span_y(),
        particles_aabb.span_z(),
    ];
    if !position_spans
        .iter()
        .chain(aabb_spans.iter())
        .all(|span| span.is_finite() && *span >= 0.0)
    {
        return None;
    }
    let position_extent = vector_length(position_spans);
    let aabb_extent = vector_length(aabb_spans);
    if position_extent <= MIN_MEANINGFUL_SPAN || aabb_extent <= MIN_MEANINGFUL_SPAN {
        return None;
    }
    // Particle positions and the cached AABB can straddle a child-space
    // rotation, so component-wise X/Y/Z ratios are not a stable scale signal.
    // Diagonal extent remains invariant when those axes are permuted.
    let observed_ratio = aabb_extent / position_extent;
    let already_scaled_error = (observed_ratio - 1.0).abs();
    let authored_error = (observed_ratio - requested_scale.recip()).abs();
    let (stored_scale, expected_ratio, error) = if already_scaled_error <= authored_error {
        (requested_scale, 1.0, already_scaled_error)
    } else {
        (1.0, requested_scale.recip(), authored_error)
    };
    let tolerance = (expected_ratio.abs() * 0.15).max(0.05);
    (error <= tolerance).then_some(stored_scale)
}

fn vector_length(vector: [f32; 3]) -> f32 {
    vector[0]
        .mul_add(
            vector[0],
            vector[1].mul_add(vector[1], vector[2] * vector[2]),
        )
        .sqrt()
}

fn capture_padding(
    particles: body_scale_port::ClothPositionBounds,
    collision: body_scale_port::ClothPositionBounds,
    stored_scale: f32,
) -> Option<AabbPadding> {
    if !particles.readable || !collision.readable || !valid_scale(stored_scale) {
        return None;
    }
    let particle_minimum = [particles.min_x, particles.min_y, particles.min_z];
    let particle_maximum = [particles.max_x, particles.max_y, particles.max_z];
    let collision_minimum = [collision.min_x, collision.min_y, collision.min_z];
    let collision_maximum = [collision.max_x, collision.max_y, collision.max_z];
    let minimum = std::array::from_fn(|index| {
        (particle_minimum[index] - collision_minimum[index]) / stored_scale
    });
    let maximum = std::array::from_fn(|index| {
        (collision_maximum[index] - particle_maximum[index]) / stored_scale
    });
    minimum
        .iter()
        .chain(maximum.iter())
        .all(|value| value.is_finite() && value.abs() <= MAX_PADDING_COMPONENT)
        .then_some(AabbPadding { minimum, maximum })
}

fn target_aabbs(
    positions: body_scale_port::ClothPositionBounds,
    baseline: ClothInstanceAabbBaseline,
    requested_scale: f32,
) -> Option<[Option<body_scale_port::ClothPositionBounds>; 3]> {
    if !positions.readable || positions.count == 0 || !valid_scale(requested_scale) {
        return None;
    }
    let particles = Some(positions);
    let collision = Some(expand_bounds(
        positions,
        baseline.collision_padding,
        requested_scale,
    )?);
    let landscape = match baseline.landscape_padding {
        Some(padding) => Some(expand_bounds(positions, padding, requested_scale)?),
        None => None,
    };
    Some([particles, collision, landscape])
}

fn expand_bounds(
    positions: body_scale_port::ClothPositionBounds,
    padding: AabbPadding,
    scale: f32,
) -> Option<body_scale_port::ClothPositionBounds> {
    let minimum = [positions.min_x, positions.min_y, positions.min_z];
    let maximum = [positions.max_x, positions.max_y, positions.max_z];
    let expanded_minimum: [f32; 3] =
        std::array::from_fn(|index| minimum[index] - padding.minimum[index] * scale);
    let expanded_maximum: [f32; 3] =
        std::array::from_fn(|index| maximum[index] + padding.maximum[index] * scale);
    expanded_minimum
        .iter()
        .chain(expanded_maximum.iter())
        .all(|value| value.is_finite())
        .then_some(body_scale_port::ClothPositionBounds {
            readable: true,
            count: 2,
            min_x: expanded_minimum[0],
            min_y: expanded_minimum[1],
            min_z: expanded_minimum[2],
            max_x: expanded_maximum[0],
            max_y: expanded_maximum[1],
            max_z: expanded_maximum[2],
        })
}

fn child_is_attached_to_instance_root(
    root: body_scale_port::ClothMatrixSummary,
    positions: body_scale_port::ClothPositionBounds,
    scale: f32,
) -> bool {
    if !root.readable || !positions.readable || positions.count == 0 || !valid_scale(scale) {
        return false;
    }
    let center = [
        (positions.min_x + positions.max_x) * 0.5,
        (positions.min_y + positions.max_y) * 0.5,
        (positions.min_z + positions.max_z) * 0.5,
    ];
    let root = [root.translation_x, root.translation_y, root.translation_z];
    if !center
        .iter()
        .chain(root.iter())
        .all(|value| value.is_finite())
    {
        return false;
    }
    let distance = vector_length(std::array::from_fn(|index| center[index] - root[index]));
    let extent = vector_length([positions.span_x(), positions.span_y(), positions.span_z()]);
    let distance_limit = (MAX_CHILD_ROOT_DISTANCE_PER_SCALE * scale.max(1.0))
        .max(MAX_CHILD_EXTENT_DISTANCE_FACTOR * extent);
    distance.is_finite() && extent.is_finite() && distance <= distance_limit
}

fn bounds_nearly_equal(
    left: body_scale_port::ClothPositionBounds,
    right: body_scale_port::ClothPositionBounds,
) -> bool {
    if !left.readable || !right.readable {
        return false;
    }
    [
        (left.min_x, right.min_x),
        (left.min_y, right.min_y),
        (left.min_z, right.min_z),
        (left.max_x, right.max_x),
        (left.max_y, right.max_y),
        (left.max_z, right.max_z),
    ]
    .into_iter()
    .all(|(left, right)| (left - right).abs() <= 1.0e-5)
}

fn valid_scale(scale: f32) -> bool {
    scale.is_finite() && scale > 0.0
}

fn report_result(
    requested_scale: f32,
    state: &mut ClothInstanceAabbScaleState,
    result: ClothInstanceAabbScaleResult,
) {
    let report = (requested_scale.to_bits(), result);
    if state.last_report != Some(report) {
        log::line(format_args!(
            "[DEBUG-ERPS-CLOTH-INSTANCE-AABB-SCALE] requested={requested_scale:.3} topology_readable={} instances={} children={} active_children={} cached_children={} captured_children={} dropped_children={} aabbs_written={} fields_written={} already_current={} non_target_generation_children={} detached_children={} native_managed_children={} rejected_children={} baseline_rejected_children={} target_rejected_children={} write_rejected_children={} deferred_rejected_children={} work_pending={}",
            result.topology_readable,
            result.instances,
            result.children,
            result.active_children,
            state.baselines.len(),
            result.captured_children,
            result.dropped_children,
            result.aabbs_written,
            result.fields_written,
            result.already_current,
            result.non_target_generation_children,
            result.detached_children,
            result.native_managed_children,
            result.rejected_children,
            result.baseline_rejected_children,
            result.target_rejected_children,
            result.write_rejected_children,
            result.deferred_rejected_children,
            result.work_pending,
        ));
    }
    state.last_report = Some(report);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(minimum: [f32; 3], maximum: [f32; 3]) -> body_scale_port::ClothPositionBounds {
        body_scale_port::ClothPositionBounds {
            readable: true,
            count: 2,
            min_x: minimum[0],
            min_y: minimum[1],
            min_z: minimum[2],
            max_x: maximum[0],
            max_y: maximum[1],
            max_z: maximum[2],
        }
    }

    #[test]
    fn detects_authored_aabb_around_half_scale_particles() {
        let positions = bounds([0.0, 0.0, 0.0], [0.3, 0.4, 0.5]);
        let particles = bounds([0.0, 0.0, 0.0], [0.6, 0.8, 1.0]);
        assert_eq!(
            infer_stored_aabb_scale(positions, particles, 0.5),
            Some(1.0)
        );
    }

    #[test]
    fn detects_an_aabb_that_is_already_at_requested_scale() {
        let positions = bounds([0.0, 0.0, 0.0], [0.3, 0.4, 0.5]);
        assert_eq!(
            infer_stored_aabb_scale(positions, positions, 0.5),
            Some(0.5)
        );
    }

    #[test]
    fn detects_axis_reordered_aabb_extents_at_requested_scale() {
        let positions = bounds([0.0, 0.0, 0.0], [0.2, 0.4, 0.8]);
        let axis_reordered = bounds([0.0, 0.0, 0.0], [0.8, 0.2, 0.4]);
        assert_eq!(
            infer_stored_aabb_scale(positions, axis_reordered, 0.5),
            Some(0.5)
        );
    }

    #[test]
    fn collision_padding_scales_once_around_live_particle_bounds() {
        let particles = bounds([10.0, 20.0, 30.0], [10.6, 20.8, 31.0]);
        let collision = bounds([9.8, 19.8, 29.8], [10.8, 21.0, 31.2]);
        let padding = capture_padding(particles, collision, 1.0).unwrap();
        let current = bounds([3.0, 4.0, 5.0], [3.3, 4.4, 5.5]);
        let target = expand_bounds(current, padding, 0.5).unwrap();

        assert!((target.min_x - 2.9).abs() < 1.0e-5);
        assert!((target.max_x - 3.4).abs() < 1.0e-5);
        assert!((target.min_y - 3.9).abs() < 1.0e-5);
        assert!((target.max_y - 4.5).abs() < 1.0e-5);
        assert!((target.min_z - 4.9).abs() < 1.0e-5);
        assert!((target.max_z - 5.6).abs() < 1.0e-5);
    }

    #[test]
    fn root_attached_child_is_selected_even_with_a_unit_transform_basis() {
        let root = body_scale_port::ClothMatrixSummary {
            readable: true,
            basis_x: 0.5,
            basis_y: 0.5,
            basis_z: 0.5,
            translation_x: 4.57,
            translation_y: 1.52,
            translation_z: -4.71,
        };
        let positions = bounds([4.35, 1.75, -4.95], [4.75, 2.25, -4.55]);

        assert!(child_is_attached_to_instance_root(root, positions, 0.5));
    }

    #[test]
    fn requested_basis_child_frozen_at_an_old_world_position_is_rejected() {
        let root = body_scale_port::ClothMatrixSummary {
            readable: true,
            basis_x: 0.5,
            basis_y: 0.5,
            basis_z: 0.5,
            translation_x: 4.57,
            translation_y: 1.52,
            translation_z: -4.71,
        };
        // Minimized from Build 2.22 child 0x1260EB5E800. Its transform basis
        // matched 0.5, but the particle center remained around 7.87,2.32,1.70
        // while the instance root moved with the player.
        let positions = bounds([7.71, 2.14, 1.54], [8.03, 2.50, 1.87]);

        assert!(!child_is_attached_to_instance_root(root, positions, 0.5));
    }

    #[test]
    fn unresolved_child_failures_get_at_most_one_immediate_retry() {
        let child = body_scale_port::ClothChildSnapshot {
            valid: true,
            child: 0x1000,
            child_vtable: 0x2000,
            sim_data: 0x3000,
            ..body_scale_port::ClothChildSnapshot::default()
        };
        let mut state = ClothInstanceAabbScaleState::default();

        let mut first_baseline_failure = ClothInstanceAabbScaleResult::default();
        record_rejection(
            &mut state,
            &child,
            RejectionStage::Baseline,
            &mut first_baseline_failure,
        );
        assert!(first_baseline_failure.work_pending);
        assert_eq!(first_baseline_failure.baseline_rejected_children, 1);

        let mut repeated_baseline_failure = ClothInstanceAabbScaleResult::default();
        record_rejection(
            &mut state,
            &child,
            RejectionStage::Baseline,
            &mut repeated_baseline_failure,
        );
        assert!(!repeated_baseline_failure.work_pending);
        assert_eq!(repeated_baseline_failure.deferred_rejected_children, 1);

        resolve_retry(&mut state, &child);
        let mut later_write_failure = ClothInstanceAabbScaleResult::default();
        record_rejection(
            &mut state,
            &child,
            RejectionStage::Write,
            &mut later_write_failure,
        );
        assert!(later_write_failure.work_pending);
    }

    #[test]
    fn target_calculation_failure_defers_to_periodic_audit() {
        let child = body_scale_port::ClothChildSnapshot {
            valid: true,
            child: 0x1000,
            child_vtable: 0x2000,
            sim_data: 0x3000,
            ..body_scale_port::ClothChildSnapshot::default()
        };
        let mut state = ClothInstanceAabbScaleState::default();
        let mut result = ClothInstanceAabbScaleResult::default();

        record_rejection(&mut state, &child, RejectionStage::Target, &mut result);

        assert!(!result.work_pending);
        assert_eq!(result.target_rejected_children, 1);
        assert_eq!(result.deferred_rejected_children, 1);
        assert!(state.immediate_retries.is_empty());
    }
}
