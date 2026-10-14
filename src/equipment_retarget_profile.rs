//! Cached authored skeleton identity for equipment sharing body attachments.
//! Mesh-only decoration is irrelevant unless it is an ancestor of a mapped bone.
use super::*;

struct Joint {
    name: String,
    parent: Option<String>,
    reference: DMat4,
}

pub(super) struct AttachmentProfile(Vec<Joint>);

impl AttachmentProfile {
    pub(super) fn new(source: &[Bone], target: &[Bone]) -> Option<Self> {
        let names: HashSet<_> = source.iter().map(|b| b.name.as_str()).collect();
        let mut needed = vec![false; target.len()];
        for (i, bone) in target.iter().enumerate() {
            if !names.contains(bone.name.as_str()) {
                continue;
            }
            let mut current = Some(i);
            while let Some(index) = current {
                if *needed.get(index)? {
                    break;
                }
                needed[index] = true;
                current = target.get(index)?.parent;
            }
        }
        let mut joints = Vec::new();
        for (_, bone) in target.iter().enumerate().filter(|(i, _)| needed[*i]) {
            joints.push(Joint {
                name: bone.name.clone(),
                parent: bone.parent.map(|p| target[p].name.clone()),
                reference: DMat4::from_scale_rotation_translation(
                    bone.reference.scale,
                    bone.reference.rotation,
                    bone.reference.translation,
                ),
            });
        }
        joints.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        (!joints.is_empty()).then_some(Self(joints))
    }

    fn equivalent(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().zip(&other.0).all(|(a, b)| {
                a.name == b.name
                    && a.parent == b.parent
                    && a.reference.abs_diff_eq(b.reference, 1e-5)
            })
    }
}

/// Only identical authored body profiles may share a common attachment owner.
/// Keep conflicting bodies separate so existing ambiguity rejection remains.
/// Slot order is deterministic, independent of registry/discovery order.
pub(super) fn attachment_sessions(sessions: &[Arc<Session>]) -> Vec<&Arc<Session>> {
    let mut eligible: Vec<_> = sessions.iter().filter(|s| s.motion.is_some()).collect();
    eligible.sort_unstable_by_key(|s| s.key.slot);
    let mut unique: Vec<&Arc<Session>> = Vec::new();
    for session in eligible {
        if !unique.iter().any(|previous| {
            previous.key.player == session.key.player
                && previous.key.assembly == session.key.assembly
                && previous.key.source == session.key.source
                && previous.identity == session.identity
                && previous.motion.zip(session.motion).is_some_and(|(a, b)| {
                    (a.leg_ratio - b.leg_ratio).abs() <= 1e-5
                        && (a.height_ratio - b.height_ratio).abs() <= 1e-5
                })
                && previous
                    .attachment_profile
                    .equivalent(&session.attachment_profile)
        }) {
            unique.push(session);
        }
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::equipment_retarget::LocalPose;
    use glam::{DQuat, DVec3};

    fn bones() -> Vec<Bone> {
        vec![
            Bone {
                name: "L_UpperArm".into(),
                parent: None,
                reference: LocalPose::IDENTITY,
            },
            Bone {
                name: "L_Hand".into(),
                parent: Some(0),
                reference: LocalPose {
                    translation: DVec3::X,
                    ..LocalPose::IDENTITY
                },
            },
        ]
    }

    #[test]
    fn profile_matches_named_reference_across_bone_order_and_mesh_only_decoration() {
        let source = bones();
        let target = bones();
        let expected = AttachmentProfile::new(&source, &target).unwrap();
        let mut reordered = vec![target[1].clone(), target[0].clone()];
        reordered[0].parent = Some(1);
        reordered[0].reference.rotation = -reordered[0].reference.rotation;
        reordered.push(Bone {
            name: "CapeTip".into(),
            parent: Some(0),
            reference: LocalPose {
                translation: DVec3::Y,
                ..LocalPose::IDENTITY
            },
        });
        assert!(expected.equivalent(&AttachmentProfile::new(&source, &reordered).unwrap()));
    }

    #[test]
    fn profile_rejects_changed_length_axes_parent_and_missing_mapped_bone() {
        let source = bones();
        let expected = AttachmentProfile::new(&source, &source).unwrap();
        let mut target = bones();
        target[1].reference.translation.x = 1.02;
        assert!(!expected.equivalent(&AttachmentProfile::new(&source, &target).unwrap()));
        target = bones();
        target[1].reference.rotation = DQuat::from_rotation_z(0.1);
        assert!(!expected.equivalent(&AttachmentProfile::new(&source, &target).unwrap()));
        target = bones();
        target[1].parent = None;
        assert!(!expected.equivalent(&AttachmentProfile::new(&source, &target).unwrap()));
        target = bones();
        target.pop();
        assert!(!expected.equivalent(&AttachmentProfile::new(&source, &target).unwrap()));
    }
}
