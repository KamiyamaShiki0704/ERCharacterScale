//! Equipment-local animation retargeting. This module owns no game pointers.
//! A prepared frame is shared by skinning and cloth consumers; source animation
//! is immutable. Runtime installation is a separate, layout-validated adapter.

use glam::{DMat4, DQuat, DVec3};
use std::collections::{HashMap, HashSet};

const MAX_BONES: usize = crate::cloth_render_scale::MAX_TRANSFORMS;
const EPSILON: f64 = 1e-10;
#[path = "equipment_retarget_grip.rs"]
mod grip;
#[path = "equipment_retarget_ik.rs"]
mod ik;

/// Authored proportions only; excludes the live pose and uniform character
/// scale. A complete body is required before changing player locomotion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MotionProfile {
    pub leg_ratio: f64,
    pub height_ratio: f64,
}

impl MotionProfile {
    pub(crate) fn new(source: &[Bone], target: &[Bone]) -> Option<Self> {
        fn measures(bones: &[Bone]) -> Option<(f64, f64)> {
            let order = validate(bones).ok()?;
            let (world, _) = reference_world(bones, &order).ok()?;
            let joint = |name: &str| {
                bones
                    .iter()
                    .position(|b| b.name == name)
                    .map(|i| world[i].w_axis.truncate())
            };
            let mut legs = 0.0;
            for side in ["L", "R"] {
                let hip = joint(&format!("{side}_Thigh"))?;
                let knee = joint(&format!("{side}_Calf"))?;
                let ankle = joint(&format!("{side}_Foot"))?;
                let length = hip.distance(knee) + knee.distance(ankle);
                if length <= EPSILON {
                    return None;
                }
                legs += length;
            }
            // ER model space is Y-up, with the authored ground at Y=0.
            // Head is a body joint: accessory bounds never alter the camera.
            let height = joint("Head")?.y;
            (height > EPSILON).then_some((legs, height))
        }
        let (source_legs, source_height) = measures(source)?;
        let (target_legs, target_height) = measures(target)?;
        let profile = Self {
            leg_ratio: target_legs / source_legs,
            height_ratio: target_height / source_height,
        };
        [profile.leg_ratio, profile.height_ratio]
            .into_iter()
            .all(|v| v.is_finite() && v > 0.0 && (v as f32).is_finite())
            .then_some(profile)
    }

    pub(crate) fn displacement(self, mut delta: [f32; 4]) -> Option<[f32; 4]> {
        for component in &mut delta[..3] {
            *component = (f64::from(*component) * self.leg_ratio) as f32;
            if !component.is_finite() {
                return None;
            }
        }
        Some(delta)
    }

    pub(crate) fn camera_height(self, height: f32) -> Option<f32> {
        let height = (f64::from(height) * self.height_ratio) as f32;
        height.is_finite().then_some(height)
    }
}

/// Runtime c0000 reference skeletons may bake a placement into Master, while
/// the animation's Master translation is already expressed from model origin.
/// The authored ground/leg heights must not inherit that placement. Work on a
/// private reference copy; keep rotations, bone lengths and native data intact.
pub(crate) fn animation_reference(mut bones: Vec<Bone>) -> Vec<Bone> {
    if ["RootPos", "RootRotY", "RootRotXZ"]
        .iter()
        .all(|name| bones.iter().any(|b| b.name == *name))
        && let Some(master) = bones
            .iter_mut()
            .find(|b| b.name == "Master" && b.parent.is_none())
    {
        master.reference.translation = DVec3::ZERO;
    }
    bones
}

/// Move a rigid attachment between body poses while preserving its original
/// local grip/sheath transform and size. `native_anchor` includes character
/// placement and the existing overall scale, exactly once.
pub(crate) fn attachment_frame(
    source: DMat4,
    target: DMat4,
    native_anchor: DMat4,
    attachment: DMat4,
) -> Option<DMat4> {
    if [source, target, native_anchor, attachment]
        .iter()
        .any(|m| !m.is_finite() || m.determinant().abs() < EPSILON)
    {
        return None;
    }
    let (_, source_rotation, source_position) = source.to_scale_rotation_translation();
    let (_, target_rotation, target_position) = target.to_scale_rotation_translation();
    let rotation =
        (target_rotation.normalize() * source_rotation.normalize().inverse()).normalize();
    let delta =
        DMat4::from_rotation_translation(rotation, target_position - rotation * source_position);
    let world = native_anchor * source.inverse();
    let result = world * delta * world.inverse() * attachment;
    result.is_finite().then_some(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProportionDifference {
    pub compared: usize,
    pub changed: usize,
}

fn body_joint(name: &str) -> bool {
    if matches!(
        name,
        "Pelvis" | "Spine" | "Spine1" | "Spine2" | "Neck" | "Head"
    ) {
        return true;
    }
    let Some(name) = name.strip_prefix("L_").or_else(|| name.strip_prefix("R_")) else {
        return false;
    };
    matches!(
        name,
        "Clavicle"
            | "UpperArm"
            | "Forearm"
            | "Hand"
            | "Thigh"
            | "Calf"
            | "Foot"
            | "Toe0"
            | "Finger0"
            | "Finger01"
            | "Finger02"
            | "Finger1"
            | "Finger11"
            | "Finger12"
            | "Finger2"
            | "Finger21"
            | "Finger22"
            | "Finger3"
            | "Finger31"
            | "Finger32"
            | "Finger4"
            | "Finger41"
            | "Finger42"
    )
}

/// Compare authored body-chain lengths, never live animation or cloth bones.
/// Edge lengths are invariant to reference joint rotations and rigid offsets.
/// The caller first repairs missing equipment root links by source ancestry.
pub(crate) fn proportion_difference(
    source: &[Bone],
    target: &[Bone],
) -> Result<ProportionDifference, Error> {
    let source_order = validate(source)?;
    let target_order = validate(target)?;
    let (source_world, _) = reference_world(source, &source_order)?;
    let (target_world, _) = reference_world(target, &target_order)?;
    let source_names: HashMap<_, _> = source
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let target_names: HashMap<_, _> = target
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let mut result = ProportionDifference {
        compared: 0,
        changed: 0,
    };
    for (t, bone) in target
        .iter()
        .enumerate()
        .filter(|(_, b)| body_joint(&b.name))
    {
        let Some(&s) = source_names.get(bone.name.as_str()) else {
            continue;
        };
        let mut ancestor = source[s].parent;
        let mut source_length = 0.0;
        let mut child = s;
        while let Some(a) = ancestor {
            source_length += source_world[child]
                .w_axis
                .truncate()
                .distance(source_world[a].w_axis.truncate());
            if body_joint(&source[a].name)
                && let Some(&target_parent) = target_names.get(source[a].name.as_str())
            {
                let mut target_length = 0.0;
                let mut target_child = t;
                while target_child != target_parent {
                    let Some(p) = target[target_child].parent else {
                        break;
                    };
                    target_length += target_world[target_child]
                        .w_axis
                        .truncate()
                        .distance(target_world[p].w_axis.truncate());
                    target_child = p;
                }
                if target_child == target_parent {
                    result.compared += 1;
                    // Only a numeric noise tolerance, never a supported-scale limit.
                    let tolerance = 1e-5 + 1e-3 * source_length.max(target_length);
                    if (source_length - target_length).abs() > tolerance {
                        result.changed += 1;
                    }
                }
                break;
            }
            child = a;
            ancestor = source[a].parent;
        }
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalPose {
    pub translation: DVec3,
    pub rotation: DQuat,
    pub scale: DVec3,
}

impl LocalPose {
    pub const IDENTITY: Self = Self {
        translation: DVec3::ZERO,
        rotation: DQuat::IDENTITY,
        scale: DVec3::ONE,
    };

    fn valid(self) -> bool {
        self.translation.is_finite()
            && self.rotation.is_finite()
            && (self.rotation.length_squared() - 1.0).abs() < 1e-6
            && self.scale.is_finite()
            && self.scale.min_element() > EPSILON
    }

    fn matrix(self) -> DMat4 {
        DMat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

#[derive(Clone, Debug)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    pub reference: LocalPose,
}

/// Reference: keep authored offsets; Animated: transfer translation deltas;
/// Scaled: also scale deltas by this joint's reference-offset length ratio.
/// Root motion uses Animated with a ratio of one, independently of leg length.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranslationMode {
    Reference,
    Animated,
    Scaled,
}

#[derive(Clone, Debug)]
pub struct BoneRule {
    pub target: String,
    pub source: String,
    pub translation: TranslationMode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidSkeleton,
    DuplicateName,
    InvalidPose,
    InvalidRule,
    NoMapping,
    PoseLength,
    InvalidScale,
    NonRigidPhysicsFrame,
    InvalidBinding,
}

#[derive(Clone, Copy)]
struct Mapping {
    source: usize,
    translation: TranslationMode,
    translation_ratio: f64,
    parent_alignment: DQuat,
    rotation_alignment: DQuat,
    root_anchor: Option<RootAnchor>,
}

#[derive(Clone, Copy)]
struct RootAnchor {
    source: usize,
    inverse_rotation: DQuat,
    target_position: DVec3,
}

/// Prepared immutable mapping. Names and reference calibration are resolved
/// once at equipment binding, never searched in the per-frame loop.
pub struct Plan {
    constraints: ik::Constraints,
    grips: [Option<grip::Grip>; 2],
    source: Vec<Bone>,
    target: Vec<Bone>,
    source_order: Vec<usize>,
    target_order: Vec<usize>,
    mapping: Vec<Option<Mapping>>,
    inverse_bind: Vec<DMat4>,
    source_reference_positions: Vec<DVec3>,
}

/// Owned scratch; a failed prepare invalidates the whole frame. Consumers must
/// obtain slices through Frame::outputs, never partially prepared storage.
#[derive(Default)]
pub struct Frame {
    source_world: Vec<DMat4>,
    source_rotation: Vec<DQuat>,
    target_local: Vec<LocalPose>,
    target_world: Vec<DMat4>,
    target_rotation: Vec<DQuat>,
    skinning: Vec<DMat4>,
    ready: bool,
    generation: u64,
}

pub struct Outputs<'a> {
    pub local: &'a [LocalPose],
    pub model: &'a [DMat4],
    pub skinning: &'a [DMat4],
    pub generation: u64,
}

impl Frame {
    pub(crate) fn source_model(&self) -> Option<&[DMat4]> {
        self.ready.then_some(&self.source_world)
    }
    pub fn outputs(&self) -> Option<Outputs<'_>> {
        self.ready.then_some(Outputs {
            local: &self.target_local,
            model: &self.target_world,
            skinning: &self.skinning,
            generation: self.generation,
        })
    }
}

fn validate(skeleton: &[Bone]) -> Result<Vec<usize>, Error> {
    if skeleton.is_empty() || skeleton.len() > MAX_BONES {
        return Err(Error::InvalidSkeleton);
    }
    let mut names = HashSet::new();
    let mut children = vec![Vec::new(); skeleton.len()];
    let mut order = Vec::with_capacity(skeleton.len());
    for (index, bone) in skeleton.iter().enumerate() {
        if bone.name.is_empty() || !names.insert(&bone.name) {
            return Err(Error::DuplicateName);
        }
        if !bone.reference.valid() {
            return Err(Error::InvalidPose);
        }
        if let Some(parent) = bone.parent {
            if parent >= skeleton.len() || parent == index {
                return Err(Error::InvalidSkeleton);
            }
            children[parent].push(index);
        } else {
            order.push(index);
        }
    }
    let mut next = 0;
    while next < order.len() {
        let bone = order[next];
        order.extend_from_slice(&children[bone]);
        next += 1;
    }
    if order.len() != skeleton.len() {
        return Err(Error::InvalidSkeleton);
    }
    Ok(order)
}

fn forward(
    skeleton: &[Bone],
    order: &[usize],
    local: &[LocalPose],
    world: &mut [DMat4],
    rotation: &mut [DQuat],
) -> Result<(), Error> {
    for &index in order {
        let pose = local[index];
        if !pose.valid() {
            return Err(Error::InvalidPose);
        }
        (world[index], rotation[index]) = match skeleton[index].parent {
            Some(parent) => (
                world[parent] * pose.matrix(),
                (rotation[parent] * pose.rotation).normalize(),
            ),
            None => (pose.matrix(), pose.rotation),
        };
        if !world[index].is_finite() || world[index].determinant().abs() < EPSILON {
            return Err(Error::InvalidPose);
        }
    }
    Ok(())
}

fn reference_world(skeleton: &[Bone], order: &[usize]) -> Result<(Vec<DMat4>, Vec<DQuat>), Error> {
    let mut matrices = vec![DMat4::IDENTITY; skeleton.len()];
    let mut rotations = vec![DQuat::IDENTITY; skeleton.len()];
    let local: Vec<_> = skeleton.iter().map(|bone| bone.reference).collect();
    forward(skeleton, order, &local, &mut matrices, &mut rotations)?;
    Ok((matrices, rotations))
}

/// FLVER equipment commonly stores Spine and Pelvis as separate roots, even
/// though animation drives Spine below Pelvis. Recover missing root links from
/// named source ancestry while preserving every target bind-space matrix.
pub(crate) fn equipment_mesh_reference(source: &[Bone], mesh: &[Bone]) -> Result<Vec<Bone>, Error> {
    validate(source)?;
    let order = validate(mesh)?;
    let (world, _) = reference_world(mesh, &order)?;
    let sources: HashMap<_, _> = source
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let targets: HashMap<_, _> = mesh
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let mut result = mesh.to_vec();
    for (i, bone) in mesh.iter().enumerate().filter(|(_, b)| b.parent.is_none()) {
        let Some(&s) = sources.get(bone.name.as_str()) else {
            continue;
        };
        let mut ancestor = source[s].parent;
        while let Some(a) = ancestor {
            if let Some(&parent) = targets.get(source[a].name.as_str()) {
                let local = world[parent].inverse() * world[i];
                let (scale, rotation, translation) = local.to_scale_rotation_translation();
                let reference = LocalPose {
                    scale,
                    rotation: rotation.normalize(),
                    translation,
                };
                if !reference.valid()
                    || reference
                        .matrix()
                        .to_cols_array()
                        .iter()
                        .zip(local.to_cols_array())
                        .any(|(x, y)| (x - y).abs() > 1e-6 * (1.0 + y.abs()))
                {
                    return Err(Error::InvalidPose);
                }
                result[i].parent = Some(parent);
                result[i].reference = reference;
                break;
            }
            ancestor = source[a].parent;
        }
    }
    validate(&result)?;
    Ok(result)
}

/// The game's equipment pose can merge player and garment bones while keeping
/// player reference lengths. Calibrate its common joints from the target mesh
/// on a private skeleton; preserve authored local offsets of extra cloth bones.
pub(crate) fn equipment_physics_reference(
    mesh: &[Bone],
    cloth: &[Bone],
) -> Result<Vec<Bone>, Error> {
    let mesh_order = validate(mesh)?;
    let cloth_order = validate(cloth)?;
    let (mesh_world, _) = reference_world(mesh, &mesh_order)?;
    let names: HashMap<_, _> = mesh
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let mut target = cloth.to_vec();
    let mut world = vec![DMat4::IDENTITY; cloth.len()];
    let mut matched = 0;
    for index in cloth_order {
        let bone = &cloth[index];
        let parent = bone.parent.map_or(DMat4::IDENTITY, |p| world[p]);
        world[index] = if let Some(&model_index) = names.get(bone.name.as_str()) {
            matched += 1;
            mesh_world[model_index]
        } else {
            parent * bone.reference.matrix()
        };
        let local = parent.inverse() * world[index];
        let (scale, rotation, translation) = local.to_scale_rotation_translation();
        let reference = LocalPose {
            scale,
            rotation: rotation.normalize(),
            translation,
        };
        if !reference.valid()
            || reference
                .matrix()
                .to_cols_array()
                .iter()
                .zip(local.to_cols_array())
                .any(|(a, b)| (a - b).abs() > 1e-6 * (1.0 + b.abs()))
        {
            return Err(Error::InvalidPose);
        }
        target[index].reference = reference;
    }
    if matched == 0 {
        return Err(Error::NoMapping);
    }
    Ok(target)
}

impl Plan {
    pub(crate) fn weapon_attachment_source(&self, mut index: usize) -> bool {
        loop {
            let Some(bone) = self.source.get(index) else {
                return false;
            };
            if matches!(bone.name.as_str(), "L_Weapon" | "R_Weapon") {
                return true;
            }
            let Some(parent) = bone.parent else {
                return false;
            };
            index = parent;
        }
    }

    /// Align limb swing to the animation reference, keeping authored lengths,
    /// shoulder positions, skin binding and twist. Run once at binding.
    pub(crate) fn calibrate_arm_directions(&mut self) {
        let Ok((sw, sr)) = reference_world(&self.source, &self.source_order) else {
            return;
        };
        let Ok((tw, tr)) = reference_world(&self.target, &self.target_order) else {
            return;
        };
        for side in ["L", "R"] {
            let joints = ["UpperArm", "Forearm", "Hand"].map(|part| {
                self.target
                    .iter()
                    .position(|b| b.name == format!("{side}_{part}"))
            });
            let [Some(a), Some(b), Some(c)] = joints else {
                continue;
            };
            let joints = [a, b, c];
            let sources = joints.map(|i| self.mapping[i].as_ref().map(|m| m.source));
            let [Some(sa), Some(sb), Some(sc)] = sources else {
                continue;
            };
            let sources = [sa, sb, sc];
            if self.target[b].parent != Some(a)
                || self.target[c].parent != Some(b)
                || self.source[sb].parent != Some(sa)
                || self.source[sc].parent != Some(sb)
            {
                continue;
            }
            for segment in 0..2 {
                let (t, tc, s, sc) = (
                    joints[segment],
                    joints[segment + 1],
                    sources[segment],
                    sources[segment + 1],
                );
                let uniform = |m: DMat4| {
                    let scale = m.to_scale_rotation_translation().0;
                    scale.min_element() > 0.0
                        && scale.max_element() - scale.min_element() < scale.max_element() * 1e-5
                };
                if !uniform(sw[s]) || !uniform(tw[t]) {
                    continue;
                }
                let Some(from) = (tw[tc].w_axis - tw[t].w_axis).truncate().try_normalize() else {
                    continue;
                };
                let Some(to) = (sw[sc].w_axis - sw[s].w_axis).truncate().try_normalize() else {
                    continue;
                };
                // Opposite references have no unique swing axis; retain native
                // reference mapping instead of choosing an arbitrary twist.
                if from.dot(to) < -0.99999 {
                    continue;
                }
                let swing = DQuat::from_rotation_arc(from, to);
                self.mapping[t].as_mut().unwrap().rotation_alignment =
                    (sr[s].inverse() * swing * tr[t]).normalize();
                // The segment's mapped twist/helper branches inherit this
                // correction while retaining their own animated rotation.
                // Stop at the next main joint, which has its own calibration.
                for (index, reference_rotation) in tr.iter().enumerate() {
                    if index == t || index == tc {
                        continue;
                    }
                    let mut parent = self.target[index].parent;
                    while let Some(p) = parent {
                        if p == tc {
                            break;
                        }
                        if p == t {
                            if let Some(mapping) = self.mapping[index].as_mut() {
                                mapping.rotation_alignment =
                                    (sr[mapping.source].inverse() * swing * *reference_rotation)
                                        .normalize();
                            }
                            break;
                        }
                        parent = self.target[p].parent;
                    }
                }
            }
        }
    }

    pub(crate) fn constrain(
        &self,
        frame: &mut Frame,
        arm_style: u32,
        grounded: bool,
    ) -> Result<(), Error> {
        let result = self.constraints.apply(self, frame, arm_style, grounded);
        if result.is_err() {
            frame.ready = false;
        }
        result
    }

    /// Attachment consumers retain native joint axes. Authored FLVER axis
    /// changes are removed here; actual IK rotation remains in the result.
    pub(crate) fn bone_attachment_pose(&self, index: usize, frame: &Frame) -> Option<DMat4> {
        if !frame.ready {
            return None;
        }
        let mapping = self.mapping.get(index)?.as_ref()?;
        let (scale, rotation, position) = frame
            .target_world
            .get(index)?
            .to_scale_rotation_translation();
        Some(DMat4::from_scale_rotation_translation(
            scale,
            (rotation * mapping.rotation_alignment.inverse()).normalize(),
            position,
        ))
    }

    pub(crate) fn attachment_pose(&self, index: usize, frame: &Frame) -> Option<DMat4> {
        let mut result = self.bone_attachment_pose(index, frame)?;
        let mapping = self.mapping.get(index)?.as_ref()?;
        if let Some(offset) = self
            .grips
            .iter()
            .flatten()
            .find_map(|g| g.offset(index, frame.source_world[mapping.source], result, frame))
        {
            result.w_axis += offset.extend(0.0);
        }
        Some(result)
    }
    /// Scale the local distance of body points; never scale an effect matrix.
    pub(crate) fn attachment_offset_ratio(&self, index: usize) -> Option<f64> {
        let source = self.mapping.get(index)?.as_ref()?.source;
        let outgoing = self
            .target
            .iter()
            .enumerate()
            .filter_map(|(i, b)| {
                if b.parent != Some(index) {
                    return None;
                }
                let si = self.mapping[i].as_ref()?.source;
                if self.source[si].parent != Some(source) {
                    return None;
                }
                let a = self.source[si].reference.translation.length();
                let b = b.reference.translation.length();
                (a > 1e-6 && b > 1e-6).then_some((a, b / a))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let ratio = outgoing.map(|(_, r)| r).unwrap_or_else(|| {
            let a = self.source[source].reference.translation.length();
            let b = self.target[index].reference.translation.length();
            if a > 1e-6 && b > 1e-6 { b / a } else { 1.0 }
        });
        (ratio.is_finite() && ratio > 0.0).then_some(ratio)
    }
    /// Align common physical joints to the exact mesh pose. Equipment input
    /// may retain animation ancestors omitted from the FLVER hierarchy.
    pub(crate) fn align_model(
        &self,
        model: &[DMat4],
        mapping: &[Option<usize>],
        frame: &mut Frame,
    ) -> Result<(), Error> {
        if !frame.ready || mapping.len() != self.target.len() {
            frame.ready = false;
            return Err(Error::PoseLength);
        }
        frame.ready = false;
        for &i in &self.target_order {
            let parent = self.target[i]
                .parent
                .map_or(DMat4::IDENTITY, |p| frame.target_world[p]);
            let world = match mapping[i] {
                Some(index) => *model.get(index).ok_or(Error::InvalidBinding)?,
                None => parent * frame.target_local[i].matrix(),
            };
            let local = parent.inverse() * world;
            let (scale, rotation, translation) = local.to_scale_rotation_translation();
            let pose = LocalPose {
                scale,
                rotation: rotation.normalize(),
                translation,
            };
            if !pose.valid()
                || pose
                    .matrix()
                    .to_cols_array()
                    .iter()
                    .zip(local.to_cols_array())
                    .any(|(a, b)| (a - b).abs() > 1e-6 * (1.0 + b.abs()))
            {
                return Err(Error::InvalidPose);
            }
            frame.target_local[i] = pose;
            frame.target_world[i] = world;
            frame.skinning[i] = world * self.inverse_bind[i];
        }
        frame.ready = true;
        Ok(())
    }

    pub(crate) fn target_order(&self) -> &[usize] {
        &self.target_order
    }

    /// Same-name bones map automatically. Extra target bones retain authored
    /// local transforms. Rules override a mapping or its translation policy.
    pub fn new(source: Vec<Bone>, target: Vec<Bone>, rules: &[BoneRule]) -> Result<Self, Error> {
        let source_order = validate(&source)?;
        let target_order = validate(&target)?;
        let (source_reference, source_rotations) = reference_world(&source, &source_order)?;
        let (target_reference, target_rotations) = reference_world(&target, &target_order)?;
        let source_names: HashMap<_, _> = source
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.as_str(), i))
            .collect();
        let target_names: HashMap<_, _> = target
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name.as_str(), i))
            .collect();
        let mut overrides = HashMap::new();
        for rule in rules {
            let Some(&index) = target_names.get(rule.target.as_str()) else {
                return Err(Error::InvalidRule);
            };
            if !source_names.contains_key(rule.source.as_str())
                || overrides.insert(index, rule).is_some()
            {
                return Err(Error::InvalidRule);
            }
        }
        let mut mapping = Vec::with_capacity(target.len());
        // c0000 drives Pelvis and Spine through separate RootPos branches.
        // FLVER can omit that common helper. Keep the target hip as their
        // common translation anchor instead of rotating the whole leg-height
        // difference about RootRotXZ (which creates an artificial torso lean).
        let body_anchor = source_names.get("RootPos").copied().and_then(|s| {
            let &t = target_names.get("Pelvis")?;
            target[t].parent.is_none().then_some(RootAnchor {
                source: s,
                inverse_rotation: source_rotations[s].inverse(),
                target_position: target_reference[t].w_axis.truncate(),
            })
        });
        for (index, bone) in target.iter().enumerate() {
            let rule = overrides.get(&index);
            let source_name = rule.map_or(bone.name.as_str(), |rule| rule.source.as_str());
            let mapped = source_names.get(source_name).map(|&s| {
                let mode = rule.map_or(
                    if bone.parent.is_none() {
                        TranslationMode::Animated
                    } else {
                        TranslationMode::Reference
                    },
                    |rule| rule.translation,
                );
                let source_length = source[s].reference.translation.length();
                let ratio = if mode == TranslationMode::Scaled && source_length > EPSILON {
                    bone.reference.translation.length() / source_length
                } else {
                    1.0
                };
                let source_parent = source[s]
                    .parent
                    .map_or(DQuat::IDENTITY, |p| source_rotations[p]);
                let target_parent = bone.parent.map_or(DQuat::IDENTITY, |p| target_rotations[p]);
                let root_anchor = body_anchor.filter(|anchor| {
                    if bone.parent.is_some() {
                        return false;
                    }
                    let mut parent = source[s].parent;
                    while let Some(p) = parent {
                        if p == anchor.source {
                            return true;
                        }
                        parent = source[p].parent;
                    }
                    false
                });
                Mapping {
                    source: s,
                    translation: mode,
                    translation_ratio: ratio,
                    parent_alignment: target_parent.inverse() * source_parent,
                    rotation_alignment: source_rotations[s].inverse() * target_rotations[index],
                    root_anchor,
                }
            });
            mapping.push(mapped);
        }
        if mapping.iter().all(Option::is_none) {
            return Err(Error::NoMapping);
        }
        let inverse_bind: Vec<_> = target_reference.iter().map(|m| m.inverse()).collect();
        if inverse_bind.iter().any(|m| !m.is_finite()) {
            return Err(Error::InvalidPose);
        }
        Ok(Self {
            grips: ["L", "R"].map(|side| grip::Grip::new(&source, &target, side)),
            constraints: ik::Constraints::new(
                &source,
                &target,
                &source_reference,
                &target_reference,
            ),
            source,
            target,
            source_order,
            target_order,
            mapping,
            inverse_bind,
            source_reference_positions: source_reference
                .iter()
                .map(|m| m.w_axis.truncate())
                .collect(),
        })
    }

    pub fn new_frame(&self) -> Frame {
        Frame {
            source_world: vec![DMat4::IDENTITY; self.source.len()],
            source_rotation: vec![DQuat::IDENTITY; self.source.len()],
            target_local: vec![LocalPose::IDENTITY; self.target.len()],
            target_world: vec![DMat4::IDENTITY; self.target.len()],
            target_rotation: vec![DQuat::IDENTITY; self.target.len()],
            skinning: vec![DMat4::IDENTITY; self.target.len()],
            ready: false,
            generation: 0,
        }
    }

    /// Allocation-free after new_frame. Source is unscaled model-local TRS;
    /// character world transform and existing overall scale belong outside it.
    /// Every bone is rebuilt from fresh animation, not the preceding output.
    pub fn prepare(
        &self,
        source_pose: &[LocalPose],
        generation: u64,
        frame: &mut Frame,
    ) -> Result<(), Error> {
        frame.ready = false;
        if source_pose.len() != self.source.len()
            || frame.source_world.len() != self.source.len()
            || frame.target_world.len() != self.target.len()
        {
            return Err(Error::PoseLength);
        }
        forward(
            &self.source,
            &self.source_order,
            source_pose,
            &mut frame.source_world,
            &mut frame.source_rotation,
        )?;
        for &index in &self.target_order {
            let bone = &self.target[index];
            let mut pose = bone.reference;
            if let Some(mapping) = self.mapping[index] {
                let source = source_pose[mapping.source];
                let desired_world_rotation =
                    frame.source_rotation[mapping.source] * mapping.rotation_alignment;
                let parent_rotation = bone
                    .parent
                    .map_or(DQuat::IDENTITY, |p| frame.target_rotation[p]);
                pose.rotation = (parent_rotation.inverse() * desired_world_rotation).normalize();
                if mapping.translation != TranslationMode::Reference {
                    if bone.parent.is_none() {
                        // Include omitted ancestors' displacement. Rotate the
                        // authored root offset only by ancestors: rotating the
                        // pelvis itself must not orbit a shorter target pelvis
                        // around the source pelvis's reference position.
                        let parent_delta = self.source[mapping.source]
                            .parent
                            .map_or(DQuat::IDENTITY, |p| frame.source_rotation[p])
                            * mapping.parent_alignment.inverse();
                        let correction = if let Some(anchor) = mapping.root_anchor {
                            let source_anchor = self.source_reference_positions[anchor.source];
                            let anchor_delta =
                                frame.source_rotation[anchor.source] * anchor.inverse_rotation;
                            anchor_delta * (anchor.target_position - source_anchor)
                                + parent_delta
                                    * (bone.reference.translation
                                        - anchor.target_position
                                        - (self.source_reference_positions[mapping.source]
                                            - source_anchor))
                        } else {
                            parent_delta
                                * (bone.reference.translation
                                    - self.source_reference_positions[mapping.source])
                        };
                        let position =
                            frame.source_world[mapping.source].w_axis.truncate() + correction;
                        pose.translation +=
                            (position - bone.reference.translation) * mapping.translation_ratio;
                    } else {
                        let delta =
                            source.translation - self.source[mapping.source].reference.translation;
                        pose.translation +=
                            mapping.parent_alignment * delta * mapping.translation_ratio;
                    }
                }
            }
            if !pose.valid() {
                return Err(Error::InvalidPose);
            }
            frame.target_local[index] = pose;
            let (world, rotation) = match bone.parent {
                Some(parent) => (
                    frame.target_world[parent] * pose.matrix(),
                    (frame.target_rotation[parent] * pose.rotation).normalize(),
                ),
                None => (pose.matrix(), pose.rotation),
            };
            if !world.is_finite() {
                return Err(Error::InvalidPose);
            }
            let skinning = world * self.inverse_bind[index];
            if !skinning.is_finite() {
                return Err(Error::InvalidPose);
            }
            frame.target_world[index] = world;
            frame.target_rotation[index] = rotation;
            frame.skinning[index] = skinning;
        }
        frame.generation = generation;
        frame.ready = true;
        Ok(())
    }
}

/// An attachment or rigid cloth collider authored in the target bone's space.
/// Keeping this distinct from world matrices prevents applying retarget twice.
#[derive(Clone, Copy, Debug)]
pub struct PhysicsBinding {
    pub bone: usize,
    pub local: DMat4,
}

impl Outputs<'_> {
    /// Consumer-owned output, with no access to the player's gameplay skeleton.
    /// This supplies transforms, not a substitute cloth solver. Native solver
    /// integration must preserve free-particle state and use matching assets.
    pub fn physics_frames(
        &self,
        bindings: &[PhysicsBinding],
        output: &mut [DMat4],
    ) -> Result<(), Error> {
        if bindings.len() != output.len() {
            return Err(Error::PoseLength);
        }
        // Preflight the entire batch before its first write.
        for binding in bindings {
            let Some(bone) = self.model.get(binding.bone) else {
                return Err(Error::InvalidBinding);
            };
            if !rigid(binding.local) || !rigid(*bone * binding.local) {
                return Err(Error::NonRigidPhysicsFrame);
            }
        }
        for (binding, matrix) in bindings.iter().zip(output) {
            *matrix = self.model[binding.bone] * binding.local;
        }
        Ok(())
    }
}

fn rigid(matrix: DMat4) -> bool {
    if !matrix.is_finite() {
        return false;
    }
    let x = matrix.x_axis.truncate();
    let y = matrix.y_axis.truncate();
    let z = matrix.z_axis.truncate();
    (x.length_squared() - 1.0).abs() < 1e-6
        && (y.length_squared() - 1.0).abs() < 1e-6
        && (z.length_squared() - 1.0).abs() < 1e-6
        && x.dot(y).abs() < 1e-6
        && y.dot(z).abs() < 1e-6
        && z.dot(x).abs() < 1e-6
        && (matrix.determinant() - 1.0).abs() < 1e-6
        && matrix.x_axis.w.abs() < 1e-6
        && matrix.y_axis.w.abs() < 1e-6
        && matrix.z_axis.w.abs() < 1e-6
        && (matrix.w_axis.w - 1.0).abs() < 1e-6
}

#[cfg(test)]
#[path = "equipment_retarget_tests.rs"]
mod tests;

/// Retarget a body dummy's offset while preserving its orientation/size lanes.
pub(crate) fn body_attachment_frame(
    source: DMat4,
    target: DMat4,
    native_anchor: DMat4,
    attachment: DMat4,
    ratio: f64,
) -> Option<DMat4> {
    if !ratio.is_finite() || ratio <= 0.0 {
        return None;
    }
    let mut result = attachment_frame(source, target, native_anchor, attachment)?;
    let target_world = native_anchor * source.inverse() * target.w_axis;
    result.w_axis = target_world + (result.w_axis - target_world) * ratio;
    result.is_finite().then_some(result)
}
