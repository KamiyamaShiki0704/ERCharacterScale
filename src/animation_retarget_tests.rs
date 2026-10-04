use super::*;
use glam::{DQuat, DVec3};
fn body() -> Vec<Bone> {
    [
        "Master",
        "Pelvis",
        "Spine",
        "Head",
        "L_Thigh",
        "R_Thigh",
        "L_UpperArm",
        "R_UpperArm",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, n)| Bone {
        name: n.into(),
        parent: (i != 0).then_some(0),
        reference: LocalPose {
            translation: DVec3::new(i as f64, 1., 0.),
            rotation: DQuat::IDENTITY,
            scale: DVec3::ONE,
        },
    })
    .collect()
}
fn raw(poses: impl Iterator<Item = LocalPose>) -> Vec<u8> {
    poses
        .flat_map(|p| {
            pose::qs_output(p)
                .unwrap()
                .into_iter()
                .flat_map(f32::to_le_bytes)
        })
        .collect()
}
#[test]
fn shuffled_sparse_tracks_keep_target_lengths_and_map_names() {
    let source = body();
    let mut target = source.clone();
    for b in &mut target[1..] {
        b.reference.translation *= 0.4;
    }
    target.swap(6, 7);
    let mut p = ClipPlan::new(source.clone(), target.clone(), vec![7, 2, 6]).unwrap();
    let mut arm = source[7].reference;
    arm.rotation = DQuat::from_rotation_z(0.3);
    p.prepare(&raw(
        [arm, source[2].reference, source[6].reference].into_iter()
    ))
    .unwrap();
    for (&i, s) in p.indices.iter().zip(&p.sampled) {
        let q = pose::qs(
            &s.0.into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(
            q.translation
                .distance(target[i as usize].reference.translation)
                < 1e-6
        );
        if target[i as usize].name == "R_UpperArm" {
            assert!(q.rotation.angle_between(arm.rotation) < 1e-6);
        }
    }
    assert!(p.prepare(&[0; 48]).is_none());
}
#[test]
fn invalid_bindings_and_nonfinite_samples_reject() {
    let b = body();
    for tracks in [vec![1, 1], vec![300], vec![-2]] {
        assert!(ClipPlan::new(b.clone(), b.clone(), tracks).is_none());
    }
    let mut p = ClipPlan::new(b.clone(), b, vec![0]).unwrap();
    assert!(p.prepare(&[0xff; 48]).is_none());
}
#[test]
#[ignore = "requires ERCS_TEST_SOURCE_SK and ERCS_TEST_TARGET_REFERENCE"]
fn actual_source_sk_and_target_reference_make_runtime_plan() {
    #[derive(serde::Deserialize)]
    struct Sk {
        bones: Vec<Joint>,
    }
    #[derive(serde::Deserialize)]
    struct Joint {
        name: String,
        parent: i32,
        translation: [f64; 3],
        rotation: [f64; 4],
        scale: [f64; 3],
    }
    let bytes = std::fs::read(std::env::var("ERCS_TEST_SOURCE_SK").unwrap()).unwrap();
    let source = crate::animation_skeleton::parse(&bytes, "c3010").unwrap();
    let text =
        std::fs::read_to_string(std::env::var("ERCS_TEST_TARGET_REFERENCE").unwrap()).unwrap();
    let target: Sk = toml::from_str(&text).unwrap();
    let target: Vec<_> = target
        .bones
        .into_iter()
        .map(|b| Bone {
            name: b.name,
            parent: (b.parent >= 0).then_some(b.parent as usize),
            reference: LocalPose {
                translation: DVec3::from_array(b.translation),
                rotation: DQuat::from_array(b.rotation),
                scale: DVec3::from_array(b.scale),
            },
        })
        .collect();
    assert!(biped(&target));
    let mut plan = ClipPlan::new(
        source.clone(),
        target.clone(),
        (0..source.len() as i16).collect(),
    )
    .unwrap();
    plan.prepare(&raw(source.iter().map(|b| b.reference)))
        .unwrap();
    assert_eq!(plan.indices.len(), 98);
    for (&i, q) in plan.indices.iter().zip(&plan.sampled) {
        let output = pose::qs(
            &q.0.into_iter()
                .flat_map(f32::to_le_bytes)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(
            output
                .translation
                .distance(target[i as usize].reference.translation)
                < 1e-5
        );
        assert!(
            output
                .rotation
                .angle_between(target[i as usize].reference.rotation.normalize())
                < 1e-5,
            "{}",
            target[i as usize].name
        );
    }
    if let Ok(directory) = std::env::var("ERCS_TEST_CLIPS") {
        let directory = std::path::Path::new(&directory);
        let start = std::time::Instant::now();
        let mut frames = 0;
        for name in ["a000_000000", "a000_002000", "a000_003000"] {
            let indices = std::fs::read(directory.join(format!("{name}.indices"))).unwrap();
            let indices: Vec<_> = indices
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes(b.try_into().unwrap()))
                .collect();
            let poses = std::fs::read(directory.join(format!("{name}.poses"))).unwrap();
            let expected = std::fs::read(directory.join(format!("{name}.result"))).unwrap();
            let n = u32::from_le_bytes(poses[..4].try_into().unwrap()) as usize;
            let mut plan = ClipPlan::new(source.clone(), target.clone(), indices.clone()).unwrap();
            for frame in 0..n {
                let samples = indices.iter().map(|&i| {
                    let at = 8 + (frame * source.len() + i as usize) * 80;
                    let v: [f64; 10] = std::array::from_fn(|j| {
                        f64::from_le_bytes(poses[at + j * 8..at + j * 8 + 8].try_into().unwrap())
                    });
                    LocalPose {
                        translation: DVec3::new(v[0], v[1], v[2]),
                        rotation: DQuat::from_xyzw(v[3], v[4], v[5], v[6]).normalize(),
                        scale: DVec3::new(v[7], v[8], v[9]),
                    }
                });
                plan.prepare(&raw(samples)).unwrap();
                let stride = (source.len() + target.len() * 2) * 12 + target.len() * 40;
                let locals = 12 + frame * stride + (source.len() + target.len() * 2) * 12;
                for (&i, q) in plan.indices.iter().zip(&plan.sampled) {
                    // Foot controls now follow target FK before native ground IK;
                    // the previously approved preview remains the body baseline.
                    if target[i as usize].name.contains("Foot_Target")
                        || weapons::target_indices(&target).contains(&(i as usize))
                    {
                        continue;
                    }
                    let at = locals + i as usize * 40;
                    let e: [f32; 10] = std::array::from_fn(|j| {
                        f32::from_le_bytes(expected[at + j * 4..at + j * 4 + 4].try_into().unwrap())
                    });
                    let mut e = e;
                    if matches!(target[i as usize].name.as_str(), "Master" | "RootPos") {
                        let ratio = plan.motion.unwrap().leg_ratio as f32;
                        for (axis, value) in e[..3].iter_mut().enumerate() {
                            let reference = target[i as usize].reference.translation[axis] as f32;
                            *value = reference + (*value - reference) * ratio;
                        }
                    }
                    let got = [
                        q.0[0], q.0[1], q.0[2], q.0[4], q.0[5], q.0[6], q.0[7], q.0[8], q.0[9],
                        q.0[10],
                    ];
                    for j in 0..10 {
                        assert!(
                            (got[j] - e[j]).abs() < 1e-4,
                            "{name} frame {frame} bone {} channel {j}: {} vs {}",
                            target[i as usize].name,
                            got[j],
                            e[j]
                        );
                    }
                }
                frames += 1;
            }
        }
        assert_eq!(frames, 233);
        eprintln!(
            "real_runtime_track_replay: {frames} body frames match accepted preview (foot controls intentionally rebuilt), elapsed_ms={:.3}",
            start.elapsed().as_secs_f64() * 1000.
        );
    }
}

#[test]
fn foreign_foot_controls_follow_target_feet_without_changing_body() {
    let mut source = body();
    for side in ["L", "R"] {
        let thigh = source
            .iter()
            .position(|b| b.name == format!("{side}_Thigh"))
            .unwrap();
        source[thigh].reference.translation =
            DVec3::new(if side == "L" { 0.2 } else { -0.2 }, 1.0, 0.0);
        let calf = source.len();
        source.push(Bone {
            name: format!("{side}_Calf"),
            parent: Some(thigh),
            reference: LocalPose {
                translation: DVec3::new(0., -0.5, 0.),
                ..LocalPose::IDENTITY
            },
        });
        source.push(Bone {
            name: format!("{side}_Foot"),
            parent: Some(calf),
            reference: LocalPose {
                translation: DVec3::new(0., -0.4, 0.1),
                ..LocalPose::IDENTITY
            },
        });
        let root = source.len();
        for (i, name, y) in [
            (0, "Foot_Target2", 1.0),
            (1, "Foot_Target1", -0.5),
            (2, "Foot_Target", -0.4),
        ] {
            source.push(Bone {
                name: format!("{side}_{name}"),
                parent: Some(if i == 0 { 0 } else { root + i - 1 }),
                reference: LocalPose {
                    translation: DVec3::new(
                        if i == 0 {
                            source[thigh].reference.translation.x
                        } else {
                            0.
                        },
                        y,
                        if i == 2 { 0.1 } else { 0. },
                    ),
                    ..LocalPose::IDENTITY
                },
            });
        }
    }
    let mut target = source.clone();
    for b in &mut target[1..] {
        b.reference.translation *= 0.65;
    }
    let mut input: Vec<_> = source.iter().map(|b| b.reference).collect();
    for (i, b) in source.iter().enumerate() {
        if b.name.ends_with("Foot_Target2") {
            input[i].translation += DVec3::new(0.5, 0.8, 0.3);
        }
        if b.name.ends_with("Calf") {
            input[i].rotation = DQuat::from_rotation_x(0.35);
        }
    }
    let mut plan = ClipPlan::new(
        source.clone(),
        target.clone(),
        (0..source.len() as i16).collect(),
    )
    .unwrap();
    plan.prepare(&raw(input.into_iter())).unwrap();
    let baseline = plan.frame.outputs().unwrap();
    let mut local: Vec<_> = target.iter().map(|b| b.reference).collect();
    for (&i, s) in plan.indices.iter().zip(&plan.sampled) {
        local[i as usize] =
            pose::qs(&s.0.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<_>>()).unwrap();
    }
    let mut world = vec![glam::DMat4::IDENTITY; target.len()];
    for (i, b) in target.iter().enumerate() {
        world[i] = b.parent.map_or(glam::DMat4::IDENTITY, |p| world[p]) * pose::matrix(local[i]);
    }
    for side in ["L", "R"] {
        let find = |name: &str| {
            target
                .iter()
                .position(|b| b.name == format!("{side}_{name}"))
                .unwrap()
        };
        let foot = find("Foot");
        let helper = find("Foot_Target");
        assert!(
            world[foot]
                .w_axis
                .truncate()
                .distance(world[helper].w_axis.truncate())
                < 1e-5,
            "{side} foot control is outside the retargeted body trajectory"
        );
        // The engine rebuilds the goal from the auxiliary root and its
        // authored probe-chain displacement. Bending the probe as a real knee
        // makes that reconstruction disagree even when our FK endpoint agrees.
        let a = find("Foot_Target2");
        let b = find("Foot_Target1");
        let rest_offset = pose::matrix(target[a].reference)
            * pose::matrix(target[b].reference)
            * pose::matrix(target[helper].reference);
        let root_rest = target[a].reference.translation;
        let displacement = rest_offset.w_axis.truncate() - root_rest;
        let parent = target[a].parent.unwrap();
        let rebuilt = world[a].w_axis.truncate() + world[parent].transform_vector3(displacement);
        assert!(
            rebuilt.distance(world[foot].w_axis.truncate()) < 1e-5,
            "{side}: native reference probe reconstruction moved the foot by {}",
            rebuilt.distance(world[foot].w_axis.truncate())
        );
        for name in ["Thigh", "Calf", "Foot"] {
            let i = find(name);
            assert!(
                world[i]
                    .w_axis
                    .truncate()
                    .distance(baseline.model[i].w_axis.truncate())
                    < 1e-5
            );
        }
    }
}

#[test]
fn native_interpolated_samples_normalize_before_retargeting() {
    let b = body();
    let mut p = ClipPlan::new(b.clone(), b.clone(), (0..b.len() as i16).collect()).unwrap();
    let mut values: Vec<_> = b.iter().map(|b| b.reference).collect();
    values[6].rotation = DQuat::from_rotation_x(0.63);
    let canonical = raw(values.into_iter());
    p.prepare(&canonical).unwrap();
    let expected: Vec<_> = p.sampled.iter().map(|q| q.0).collect();
    let mut interpolated = canonical.clone();
    // Native samples captured before Havok normalization have norm^2 down to
    // 0.9288. A finite nonzero quaternion represents the same rotation.
    for q in interpolated.chunks_exact_mut(48) {
        for component in q[16..32].chunks_exact_mut(4) {
            let v = f32::from_le_bytes(component.try_into().unwrap()) * 0.964;
            component.copy_from_slice(&v.to_le_bytes());
        }
        assert!(
            pose::qs(q).is_none(),
            "reference/physics decoder must remain strict"
        );
    }
    p.prepare(&interpolated)
        .expect("native interpolation must not neutralize the clip");
    for (q, expected) in p.sampled.iter().zip(expected) {
        for (got, want) in q.0.into_iter().zip(expected) {
            assert!((got - want).abs() < 1e-6);
        }
    }
    for bad in [0.0f32, f32::NAN, f32::INFINITY] {
        let mut raw = canonical.clone();
        for part in raw[16..32].chunks_exact_mut(4) {
            part.copy_from_slice(&bad.to_le_bytes());
        }
        assert!(p.prepare(&raw).is_none());
    }
}

#[test]
fn foreign_root_displacement_follows_target_leg_length() {
    let mut source = body();
    source[0].reference = LocalPose::IDENTITY;
    let root = source.len();
    source.push(Bone {
        name: "Root".into(),
        parent: Some(0),
        reference: LocalPose {
            translation: DVec3::Y,
            ..LocalPose::IDENTITY
        },
    });
    source[1].parent = Some(root);
    for side in ["L", "R"] {
        let thigh = source
            .iter()
            .position(|b| b.name == format!("{side}_Thigh"))
            .unwrap();
        let calf = source.len();
        source.push(Bone {
            name: format!("{side}_Calf"),
            parent: Some(thigh),
            reference: LocalPose {
                translation: -DVec3::Y,
                ..LocalPose::IDENTITY
            },
        });
        source.push(Bone {
            name: format!("{side}_Foot"),
            parent: Some(calf),
            reference: LocalPose {
                translation: -DVec3::Y,
                ..LocalPose::IDENTITY
            },
        });
    }
    let mut target = source.clone();
    target[root].name = "RootPos".into();
    for b in &mut target[1..] {
        b.reference.translation *= 0.5;
    }
    let mut p = ClipPlan::new(
        source.clone(),
        target.clone(),
        (0..source.len() as i16).collect(),
    )
    .unwrap();
    let mut input: Vec<_> = source.iter().map(|b| b.reference).collect();
    input[root].translation += DVec3::new(0.2, -0.6, 0.4);
    input[0].translation += DVec3::new(0.0, 0.1, 0.0);
    p.prepare(&raw(input.clone().into_iter())).unwrap();
    for (bone, delta) in [
        (root, DVec3::new(0.1, -0.3, 0.2)),
        (0, DVec3::new(0., 0.05, 0.)),
    ] {
        let slot = p.indices.iter().position(|&i| i as usize == bone).unwrap();
        let q = p.sampled[slot].0;
        let got = DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64);
        assert!(
            got.distance(target[bone].reference.translation + delta) < 1e-6,
            "root displacement must use target/source legs: {got:?}"
        );
    }
    target[root].name = "Root".into();
    let mut enemy = ClipPlan::new(
        source.clone(),
        target.clone(),
        (0..source.len() as i16).collect(),
    )
    .unwrap();
    enemy.prepare(&raw(input.into_iter())).unwrap();
    let slot = enemy
        .indices
        .iter()
        .position(|&i| i as usize == root)
        .unwrap();
    let q = enemy.sampled[slot].0;
    assert!(
        DVec3::new(q[0] as f64, q[1] as f64, q[2] as f64)
            .distance(target[root].reference.translation + DVec3::new(0.1, -0.3, 0.2))
            < 1e-6,
        "same-name enemy root must also translate"
    );
}
