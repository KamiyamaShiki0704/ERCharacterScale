//! Foreign clip tracks -> target reference proportions, before native blending.
//! Native sampling, event timing, extraction, IK and physics remain engine work.
use crate::equipment_retarget::{Bone, BoneRule, Frame, LocalPose, Plan, TranslationMode};
use crate::equipment_retarget_pose as pose;
#[path = "animation_retarget_weapons.rs"]
mod weapons;

pub(crate) fn biped(bones: &[Bone]) -> bool {
    let find = |name: &str| bones.iter().position(|b| b.name == name);
    if ["Pelvis", "Spine", "Head"]
        .iter()
        .any(|n| find(n).is_none())
    {
        return false;
    }
    for side in ["L", "R"] {
        for chain in [["Thigh", "Calf", "Foot"], ["UpperArm", "Forearm", "Hand"]] {
            let joints = chain.map(|part| find(&format!("{side}_{part}")));
            let [Some(a), Some(b), Some(c)] = joints else {
                return false;
            };
            if bones[b].parent != Some(a) || bones[c].parent != Some(b) {
                return false;
            }
        }
    }
    true
}

#[repr(C, align(16))]
#[derive(Clone, Copy)]
pub(crate) struct Qs(pub [f32; 12]);

pub(crate) struct ClipPlan {
    pub motion: Option<crate::equipment_retarget::MotionProfile>,
    roots: Vec<usize>,
    plan: Plan,
    frame: Frame,
    feet: Vec<FootControls>,
    weapons: Vec<weapons::Weapons>,
    reference: Vec<LocalPose>,
    source: Vec<LocalPose>,
    track_bones: Vec<i16>,
    pub indices: Vec<i16>,
    pub sampled: Vec<Qs>,
}
impl ClipPlan {
    pub fn new(source: Vec<Bone>, target: Vec<Bone>, tracks: Vec<i16>) -> Option<Self> {
        if source.is_empty()
            || target.len() > i16::MAX as usize
            || tracks.is_empty()
            || tracks
                .iter()
                .any(|&i| i < -1 || i >= 0 && i as usize >= source.len())
        {
            return None;
        }
        let mut seen = std::collections::HashSet::new();
        if tracks.iter().filter(|&&i| i >= 0).any(|&i| !seen.insert(i)) {
            return None;
        }
        let mut rules = Vec::new();
        let alias = !source.iter().any(|b| b.name == "RootPos")
            && source.iter().any(|b| b.name == "Root")
            && target.iter().any(|b| b.name == "RootPos");
        if alias {
            rules.push(BoneRule {
                source: "Root".into(),
                target: "RootPos".into(),
                translation: TranslationMode::Animated,
            });
        }
        for b in &target {
            if matches!(b.name.as_str(), "Root" | "RootPos")
                && source.iter().any(|s| s.name == b.name)
            {
                rules.push(BoneRule {
                    source: b.name.clone(),
                    target: b.name.clone(),
                    translation: TranslationMode::Animated,
                });
            }
            if b.name.contains("Foot_Target") && source.iter().any(|s| s.name == b.name) {
                rules.push(BoneRule {
                    source: b.name.clone(),
                    target: b.name.clone(),
                    translation: TranslationMode::Scaled,
                });
            }
        }
        let weapon_indices = weapons::target_indices(&target);
        let indices: Vec<_> = target
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                source.iter().any(|s| s.name == t.name)
                    || alias && t.name == "RootPos"
                    || weapon_indices.contains(i)
            })
            .map(|(i, _)| i as i16)
            .collect();
        // Require an actual shared body, not one accidentally matching helper.
        for name in [
            "Pelvis",
            "Spine",
            "Head",
            "L_Thigh",
            "R_Thigh",
            "L_UpperArm",
            "R_UpperArm",
        ] {
            if !source.iter().any(|b| b.name == name) || !target.iter().any(|b| b.name == name) {
                return None;
            }
        }
        let motion = crate::equipment_retarget::MotionProfile::new(&source, &target);
        let roots = source
            .iter()
            .enumerate()
            .filter(|(_, b)| matches!(b.name.as_str(), "Master" | "Root" | "RootPos"))
            .map(|(i, _)| i)
            .collect();
        let reference: Vec<_> = source.iter().map(|b| b.reference).collect();
        let target_controls = target.clone();
        let weapons = ["L", "R"]
            .into_iter()
            .filter_map(|side| weapons::Weapons::new(side, &source, &target, &indices, &tracks))
            .collect();
        let plan = Plan::new(source, target, &rules).ok()?;
        let mut frame = plan.new_frame();
        plan.prepare(&reference, 0, &mut frame).ok()?;
        let rest = frame.outputs()?;
        let feet: Vec<_> = ["L", "R"]
            .into_iter()
            .filter_map(|side| FootControls::new(side, &target_controls, rest.model, &indices))
            .collect();
        let sampled = vec![Qs([0.; 12]); indices.len()];
        Some(Self {
            motion,
            roots,
            plan,
            frame,
            feet,
            weapons,
            source: reference.clone(),
            reference,
            track_bones: tracks,
            indices,
            sampled,
        })
    }
    pub fn prepare(&mut self, raw: &[u8]) -> Option<()> {
        if raw.len() != self.track_bones.len() * 48 {
            return None;
        }
        self.source.copy_from_slice(&self.reference);
        for (&bone, qs) in self.track_bones.iter().zip(raw.chunks_exact(48)) {
            let q = sampled_pose(qs)?;
            if bone >= 0 {
                self.source[bone as usize] = q;
            }
        }
        if let Some(profile) = self.motion {
            for &i in &self.roots {
                self.source[i].translation = self.reference[i].translation
                    + (self.source[i].translation - self.reference[i].translation)
                        * profile.leg_ratio;
            }
        }
        self.plan.prepare(&self.source, 0, &mut self.frame).ok()?;
        let output = self.frame.outputs()?;
        for (&index, sample) in self.indices.iter().zip(&mut self.sampled) {
            sample.0 = pose::qs_output(output.local[index as usize])?;
        }
        for feet in &self.feet {
            feet.apply(output.model, &mut self.sampled)?;
        }
        for weapons in &mut self.weapons {
            weapons.apply(self.frame.source_model()?, output.model, &mut self.sampled)?;
        }
        Some(())
    }
}

// This hook runs before Havok normalizes interpolated rotation tracks. Unlike
// reference skeletons and completed physics poses, these finite nonunit values
// are valid animation samples. Normalize only at this boundary; keep the shared
// pose decoder strict and continue rejecting degenerate/nonfinite rotations.
fn sampled_pose(raw: &[u8]) -> Option<LocalPose> {
    let mut normalized: [u8; 48] = raw.try_into().ok()?;
    let component = |i| f32::from_le_bytes(raw[i..i + 4].try_into().unwrap()) as f64;
    let q = glam::DQuat::from_xyzw(component(16), component(20), component(24), component(28));
    if !q.is_finite() || q.length_squared() <= 1e-12 {
        return None;
    }
    for (value, out) in q
        .normalize()
        .to_array()
        .into_iter()
        .zip(normalized[16..32].chunks_exact_mut(4))
    {
        out.copy_from_slice(&(value as f32).to_le_bytes());
    }
    pose::qs(&normalized)
}

// Native foot controls form an auxiliary leg chain, not the visible leg.
// A foreign clip's authored contact trajectory can be outside a shorter target's
// reach. Build controls from the retargeted visible leg before native ground IK.
struct FootControls {
    body: [usize; 3],
    slots: [usize; 3],
    parent: usize,
    reference: [LocalPose; 3],
    alignment: [glam::DQuat; 3],
}
impl FootControls {
    fn new(side: &str, bones: &[Bone], rest: &[glam::DMat4], indices: &[i16]) -> Option<Self> {
        let find = |part| {
            bones
                .iter()
                .position(|b| b.name == format!("{side}_{part}"))
        };
        let body = [find("Thigh")?, find("Calf")?, find("Foot")?];
        let helpers = [
            find("Foot_Target2")?,
            find("Foot_Target1")?,
            find("Foot_Target")?,
        ];
        if bones[helpers[1]].parent != Some(helpers[0])
            || bones[helpers[2]].parent != Some(helpers[1])
        {
            return None;
        }
        let parent = bones[helpers[0]].parent?;
        // Never modify an ancestor of a visible body joint.
        for joint in body {
            let mut ancestor = Some(joint);
            while let Some(i) = ancestor {
                if helpers.contains(&i) {
                    return None;
                }
                ancestor = bones[i].parent;
            }
        }
        let slot = |i| indices.iter().position(|&v| v as usize == i);
        Some(Self {
            body,
            slots: [slot(helpers[0])?, slot(helpers[1])?, slot(helpers[2])?],
            parent,
            reference: helpers.map(|i| bones[i].reference),
            alignment: std::array::from_fn(|i| {
                (rest[body[i]]
                    .to_scale_rotation_translation()
                    .1
                    .normalize()
                    .inverse()
                    * rest[helpers[i]]
                        .to_scale_rotation_translation()
                        .1
                        .normalize())
                .normalize()
            }),
        })
    }
    fn apply(&self, world: &[glam::DMat4], samples: &mut [Qs]) -> Option<()> {
        let parent = world[self.parent];
        let mut previous = parent;
        let mut local = self.reference;
        for (i, p) in local.iter_mut().enumerate() {
            // Target2/1 are the native ground probe, not a second bent leg.
            // Ground IK reconstructs their endpoint from the reference probe
            // displacement. Preserve these axes; only the foot has an animated
            // orientation. Otherwise native reconstruction moves the goal again.
            if i == 2 {
                let rotation = world[self.body[i]]
                    .to_scale_rotation_translation()
                    .1
                    .normalize()
                    * self.alignment[i];
                p.rotation = (previous
                    .to_scale_rotation_translation()
                    .1
                    .normalize()
                    .inverse()
                    * rotation)
                    .normalize();
            }
            previous *= pose::matrix(*p);
        }
        // Match the visible foot exactly while keeping native helper lengths.
        // Only this independent helper root is translated; body/FK is untouched.
        let delta = world[self.body[2]].w_axis.truncate() - previous.w_axis.truncate();
        local[0].translation += parent.inverse().transform_vector3(delta);
        for (p, &slot) in local.into_iter().zip(&self.slots) {
            samples[slot].0 = pose::qs_output(p)?;
        }
        Some(())
    }
}

#[cfg(test)]
#[path = "animation_retarget_tests.rs"]
mod tests;
