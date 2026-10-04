//! Private, length-preserving limb constraints, evaluated before cloth input.
//! Native animation provides contacts and grip trajectories; never write back
//! into the player's shared skeleton or change a target bone's local length.
use super::*;

#[derive(Clone, Copy)]
struct Limb {
    joints: [usize; 3],
    source_end: usize,
    source_reference_rotation: DQuat,
    source_height: f64,
    target_height: f64,
}

#[derive(Default)]
pub(super) struct Constraints {
    arms: [Option<Limb>; 2],
    legs: [Option<Limb>; 2],
}

impl Constraints {
    pub(super) fn new(source: &[Bone], target: &[Bone], sw: &[DMat4], tw: &[DMat4]) -> Self {
        let limb = |side, names: [&str; 3]| -> Option<Limb> {
            let names = names.map(|name| format!("{side}_{name}"));
            let joints = names.map(|name| target.iter().position(|b| b.name == name));
            let joints = [joints[0]?, joints[1]?, joints[2]?];
            // Unsupported intermediary chains retain ordinary retargeting.
            if target[joints[1]].parent != Some(joints[0])
                || target[joints[2]].parent != Some(joints[1])
            {
                return None;
            }
            let source_end = source
                .iter()
                .position(|b| b.name == target[joints[2]].name)?;
            Some(Limb {
                joints,
                source_end,
                source_reference_rotation: sw[source_end]
                    .to_scale_rotation_translation()
                    .1
                    .normalize(),
                source_height: sw[source_end].w_axis.y,
                target_height: tw[joints[2]].w_axis.y,
            })
        };
        Self {
            arms: ["L", "R"].map(|s| limb(s, ["UpperArm", "Forearm", "Hand"])),
            legs: ["L", "R"].map(|s| limb(s, ["Thigh", "Calf", "Foot"])),
        }
    }

    pub(super) fn apply(
        &self,
        plan: &Plan,
        frame: &mut Frame,
        style: u32,
        grounded: bool,
    ) -> Result<(), Error> {
        if !frame.ready {
            return Err(Error::InvalidPose);
        }
        if let Some(primary) = match style {
            2 => Some(0),
            3 => Some(1),
            _ => None,
        } && let (Some(main), Some(other)) = (self.arms[primary], self.arms[1 - primary])
        {
            let source = frame.source_world[main.source_end];
            let canonical = plan
                .attachment_pose(main.joints[2], frame)
                .ok_or(Error::InvalidPose)?;
            let (_, sr, sp) = source.to_scale_rotation_translation();
            let (_, tr, tp) = canonical.to_scale_rotation_translation();
            let delta = (tr * sr.inverse()).normalize();
            // Preserve the current animation's relative grip, including its
            // release motion. No fixed second-hand point is cached from idle.
            let other_attachment = plan
                .attachment_pose(other.joints[2], frame)
                .ok_or(Error::InvalidPose)?;
            let other_grip_offset = other_attachment.w_axis.truncate()
                - frame.target_world[other.joints[2]].w_axis.truncate();
            let goal = tp
                + delta
                    * (frame.source_world[other.source_end].w_axis.truncate()
                        - sp
                        - other_grip_offset);
            let rotation = (delta * frame.target_rotation[other.joints[2]]).normalize();
            solve(plan, frame, other, goal, rotation)?;
        }
        if grounded {
            for limb in self.legs.into_iter().flatten() {
                let source = frame.source_world[limb.source_end];
                let rotation = (frame.source_rotation[limb.source_end]
                    * limb.source_reference_rotation.inverse())
                .normalize();
                let normal = rotation * DVec3::Y;
                // Native ankle orientation supplies the local contact plane.
                // Preserve foot lift from animation; only replace the authored
                // ankle-to-ground offset and sample at the new foot position.
                if normal.y <= 0.3 {
                    continue;
                }
                let mut goal = frame.target_world[limb.joints[2]].w_axis.truncate();
                let contact = source.w_axis.truncate() - normal * limb.source_height;
                let target_offset = normal * limb.target_height;
                let xz = goal - target_offset - contact;
                goal.y =
                    contact.y + target_offset.y - (normal.x * xz.x + normal.z * xz.z) / normal.y;
                solve(
                    plan,
                    frame,
                    limb,
                    goal,
                    frame.target_rotation[limb.joints[2]],
                )?;
            }
        }
        Ok(())
    }
}

fn rebuild(plan: &Plan, frame: &mut Frame) -> Result<(), Error> {
    forward(
        &plan.target,
        &plan.target_order,
        &frame.target_local,
        &mut frame.target_world,
        &mut frame.target_rotation,
    )?;
    for &i in &plan.target_order {
        frame.skinning[i] = frame.target_world[i] * plan.inverse_bind[i];
    }
    Ok(())
}

fn set_rotation(plan: &Plan, frame: &mut Frame, joint: usize, world: DQuat) -> Result<(), Error> {
    let parent = plan.target[joint]
        .parent
        .map_or(DQuat::IDENTITY, |p| frame.target_rotation[p]);
    frame.target_local[joint].rotation = (parent.inverse() * world).normalize();
    rebuild(plan, frame)
}

fn solve(
    plan: &Plan,
    frame: &mut Frame,
    limb: Limb,
    goal: DVec3,
    end_rotation: DQuat,
) -> Result<(), Error> {
    let [a, b, c] = limb.joints;
    if [a, b, c].iter().any(|&i| {
        let (s, _, _) = frame.target_world[i].to_scale_rotation_translation();
        s.min_element() <= 0.0 || s.max_element() - s.min_element() > s.max_element() * 1e-5
    }) {
        return Ok(());
    }
    let point = |i: usize| frame.target_world[i].w_axis.truncate();
    let (root, middle, end) = (point(a), point(b), point(c));
    let (upper, lower) = (root.distance(middle), middle.distance(end));
    let direction = goal - root;
    let distance = direction.length();
    if !goal.is_finite() || upper <= EPSILON || lower <= EPSILON || distance <= EPSILON {
        return Ok(());
    }
    let axis = direction / distance;
    let slack = upper.min(lower) * 1e-6;
    let distance = distance.clamp((upper - lower).abs() + slack, upper + lower - slack);
    let along = (upper * upper + distance * distance - lower * lower) / (2.0 * distance);
    let height = (upper * upper - along * along).max(0.0).sqrt();
    let mut bend = middle - root - axis * (middle - root).dot(axis);
    if bend.length_squared() < 1e-12 {
        // Deterministic fallback at a straight limb; prefer its animated basis.
        bend = frame.target_rotation[a] * DVec3::Z;
        bend -= axis * bend.dot(axis);
        if bend.length_squared() < 1e-12 {
            bend = axis.any_orthonormal_vector();
        }
    }
    let knee = root + axis * along + bend.normalize() * height;
    let delta = DQuat::from_rotation_arc((middle - root).normalize(), (knee - root).normalize());
    set_rotation(
        plan,
        frame,
        a,
        (delta * frame.target_rotation[a]).normalize(),
    )?;
    let middle = frame.target_world[b].w_axis.truncate();
    let end = frame.target_world[c].w_axis.truncate();
    let reachable = root + axis * distance;
    let delta =
        DQuat::from_rotation_arc((end - middle).normalize(), (reachable - middle).normalize());
    set_rotation(
        plan,
        frame,
        b,
        (delta * frame.target_rotation[b]).normalize(),
    )?;
    set_rotation(plan, frame, c, end_rotation)
}
