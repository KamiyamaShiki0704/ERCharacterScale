//! Hand-relative weapon motion. Never inspect or alter dummy tables.
use super::{Qs, pose};
use crate::equipment_retarget::{Bone, LocalPose};
use glam::{DMat4, DQuat, DVec3};

struct Candidate {
    bone: usize,
    rest: LocalPose,
    previous: Option<LocalPose>,
}
struct Destination {
    slot: usize,
    parent: usize,
    rest: LocalPose,
}
pub(super) struct Weapons {
    source_hand: usize,
    target_hand: usize,
    candidates: Vec<Candidate>,
    targets: Vec<Destination>,
    selected: Option<usize>,
    basis: DQuat,
}

// Require a recognized weapon name AND direct hand ownership. Accessory,
// sheath, body and chained weapon bones must not be guessed from a number.
fn weapon(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let name = name
        .strip_prefix("l_")
        .or_else(|| name.strip_prefix("r_"))
        .unwrap_or(&name);
    ["weapon", "sword", "shield", "spear", "axe", "bow", "staff"]
        .iter()
        .any(|base| {
            name.strip_prefix(base)
                .is_some_and(|suffix| suffix.chars().all(|c| c.is_ascii_digit() || c == '_'))
        })
}
pub(super) fn target_indices(bones: &[Bone]) -> Vec<usize> {
    bones
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            weapon(&b.name)
                && b.parent
                    .is_some_and(|p| matches!(bones[p].name.as_str(), "L_Hand" | "R_Hand"))
        })
        .map(|(i, _)| i)
        .collect()
}
fn reference(bones: &[Bone]) -> Option<Vec<DMat4>> {
    let mut world = Vec::with_capacity(bones.len());
    for (i, b) in bones.iter().enumerate() {
        if b.parent.is_some_and(|p| p >= i) {
            return None;
        }
        world.push(b.parent.map_or(DMat4::IDENTITY, |p| world[p]) * pose::matrix(b.reference));
    }
    Some(world)
}
impl Weapons {
    pub(super) fn new(
        side: &str,
        source: &[Bone],
        target: &[Bone],
        indices: &[i16],
        tracks: &[i16],
    ) -> Option<Self> {
        let hand = format!("{side}_Hand");
        let sh = source.iter().position(|b| b.name == hand)?;
        let th = target.iter().position(|b| b.name == hand)?;
        let sw = reference(source)?;
        let tw = reference(target)?;
        let si = sw[sh].inverse();
        let ti = tw[th].inverse();
        let candidates = source
            .iter()
            .enumerate()
            .filter(|(i, b)| {
                b.parent == Some(sh) && weapon(&b.name) && tracks.contains(&(*i as i16))
            })
            .map(|(i, _)| {
                Some(Candidate {
                    bone: i,
                    rest: pose::local(si * sw[i])?,
                    previous: None,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let targets = target
            .iter()
            .enumerate()
            .filter(|(_, b)| b.parent == Some(th) && weapon(&b.name))
            .map(|(i, b)| {
                Some(Destination {
                    slot: indices.iter().position(|&j| j as usize == i)?,
                    parent: b.parent?,
                    rest: pose::local(ti * tw[i])?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        if candidates.is_empty() || targets.is_empty() {
            return None;
        }
        Some(Self {
            source_hand: sh,
            target_hand: th,
            candidates,
            targets,
            selected: None,
            basis: (tw[th]
                .to_scale_rotation_translation()
                .1
                .normalize()
                .inverse()
                * sw[sh].to_scale_rotation_translation().1.normalize())
            .normalize(),
        })
    }
    pub(super) fn apply(
        &mut self,
        source: &[DMat4],
        target: &[DMat4],
        samples: &mut [Qs],
    ) -> Option<()> {
        let inverse = source[self.source_hand].inverse();
        let mut moving = None;
        let mut ambiguous = false;
        let mut posed = None;
        let mut pose_ambiguous = false;
        for (i, c) in self.candidates.iter_mut().enumerate() {
            let current = pose::local(inverse * source[c.bone])?;
            if changed(c.rest, current) {
                if posed.is_some() {
                    pose_ambiguous = true;
                }
                posed = Some(i);
            }
            if c.previous.is_some_and(|old| changed(old, current)) {
                if moving.is_some() {
                    ambiguous = true;
                }
                moving = Some(i);
            }
            c.previous = Some(current);
        }
        if self.candidates.len() == 1 {
            self.selected = Some(0);
        } else if !ambiguous && moving.is_some() {
            self.selected = moving;
        } else if self.selected.is_none() && !pose_ambiguous {
            self.selected = posed;
        }
        // Keep selection through pauses; simultaneous tracks never switch it.
        // Before any unique motion, preserve each target's own grip reference.
        let (translation, rotation) = if let Some(i) = self.selected {
            let c = &self.candidates[i];
            let now = c.previous?;
            (
                self.basis * (now.translation - c.rest.translation),
                (self.basis * now.rotation * c.rest.rotation.inverse() * self.basis.inverse())
                    .normalize(),
            )
        } else {
            (DVec3::ZERO, DQuat::IDENTITY)
        };
        for destination in &self.targets {
            let mut local = destination.rest;
            local.translation += translation;
            local.rotation = (rotation * local.rotation).normalize();
            // Multiple target sockets retain their distinct authored grip. Only
            // their independent motion is shared; no source grip/scale is copied.
            let local = pose::local(
                target[destination.parent].inverse()
                    * target[self.target_hand]
                    * pose::matrix(local),
            )?;
            samples[destination.slot].0 = pose::qs_output(local)?;
        }
        Some(())
    }
}
fn changed(a: LocalPose, b: LocalPose) -> bool {
    a.translation.distance_squared(b.translation) > 1e-8
        || a.rotation.dot(b.rotation).abs() < 1.0 - 1e-8
}

#[cfg(test)]
#[path = "animation_retarget_weapons_tests.rs"]
mod tests;
