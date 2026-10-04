//! Anatomical grip calibration from matching finger landmarks. Keep the native
//! weapon's grip residual and orientation; transfer the animated finger region
//! instead of assuming that the target palm has the original wrist offset.
use super::*;

pub(super) struct Grip {
    hand: usize,
    source: [usize; 12],
    target: [usize; 12],
}

impl Grip {
    pub(super) fn new(source: &[Bone], target: &[Bone], side: &str) -> Option<Self> {
        fn indices(bones: &[Bone], side: &str) -> Option<(usize, [usize; 12])> {
            let hand = bones
                .iter()
                .position(|b| b.name == format!("{side}_Hand"))?;
            let mut indices = [0; 12];
            for finger in 1..=4 {
                for segment in 0..3 {
                    let suffix = if segment == 0 {
                        String::new()
                    } else {
                        segment.to_string()
                    };
                    let name = format!("{side}_Finger{finger}{suffix}");
                    let i = bones.iter().position(|b| b.name == name)?;
                    let mut ancestor = bones[i].parent;
                    while ancestor != Some(hand) {
                        ancestor = bones[ancestor?].parent;
                    }
                    indices[(finger - 1) * 3 + segment] = i;
                }
            }
            Some((hand, indices))
        }
        let (_, s) = indices(source, side)?;
        let (hand, t) = indices(target, side)?;
        Some(Self {
            hand,
            source: s,
            target: t,
        })
    }

    pub(super) fn offset(
        &self,
        hand: usize,
        source_hand: DMat4,
        target_hand: DMat4,
        frame: &Frame,
    ) -> Option<DVec3> {
        if hand != self.hand {
            return None;
        }
        let center = |indices: &[usize; 12], matrices: &[DMat4]| {
            indices
                .iter()
                .map(|&i| matrices[i].w_axis.truncate())
                .sum::<DVec3>()
                / 12.0
        };
        let (_, sr, sp) = source_hand.to_scale_rotation_translation();
        let (_, tr, tp) = target_hand.to_scale_rotation_translation();
        let delta = (tr.normalize() * sr.normalize().inverse()).normalize();
        Some(
            center(&self.target, &frame.target_world)
                - tp
                - delta * (center(&self.source, &frame.source_world) - sp),
        )
    }
}
