use super::*;

fn append_fingers(bones: &mut Vec<Bone>, hand: usize, side: &str, size: f64) -> Vec<usize> {
    let mut indices = Vec::new();
    for finger in 1..=4 {
        let mut parent = hand;
        for segment in 0..3 {
            let name = format!(
                "{side}_Finger{finger}{}",
                if segment == 0 {
                    String::new()
                } else {
                    segment.to_string()
                }
            );
            let offset = if segment == 0 {
                DVec3::new(0.1, (2.5 - f64::from(finger)) * 0.02, 0.0)
            } else {
                DVec3::new(0.02, 0.0, 0.02)
            };
            indices.push(bones.len());
            bones.push(bone(&name, Some(parent), offset * size));
            parent = bones.len() - 1;
        }
    }
    indices
}

#[test]
fn weapon_grip_tracks_shorter_fingers_instead_of_preserving_old_wrist_offset() {
    let mut source = vec![bone("R_Hand", None, DVec3::Y)];
    let fingers = append_fingers(&mut source, 0, "R", 1.0);
    let mut target = source.clone();
    for &i in &fingers {
        target[i].reference.translation *= 0.5;
    }
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    plan.prepare(&pose(&source), 0, &mut frame).unwrap();
    let center = |matrices: &[DMat4]| {
        fingers
            .iter()
            .map(|&i| matrices[i].w_axis.truncate())
            .sum::<DVec3>()
            / fingers.len() as f64
    };
    let source_center = center(&frame.source_world);
    let target_center = center(&frame.target_world);
    let local_residual = DVec3::new(-0.015, 0.005, 0.01);
    let weapon = DMat4::from_translation(source_center + local_residual);
    let mapped = attachment_frame(
        frame.source_world[0],
        plan.attachment_pose(0, &frame).unwrap(),
        frame.source_world[0],
        weapon,
    )
    .unwrap();
    near(mapped.w_axis.truncate(), target_center + local_residual);
    near(frame.target_world[0].w_axis.truncate(), DVec3::Y);
    assert!((mapped.x_axis.length() - 1.0).abs() < 1e-10);
}

#[test]
fn retained_weapon_bones_use_the_calibrated_hand_and_keep_animated_weapon_motion() {
    let mut source = vec![bone("R_Hand", None, DVec3::Y)];
    let fingers = append_fingers(&mut source, 0, "R", 1.0);
    let weapon = source.len();
    source.push(bone("R_Weapon", Some(0), DVec3::new(0.14, 0.03, 0.05)));
    let child = source.len();
    source.push(bone("WeaponTip", Some(weapon), DVec3::X));
    let mut target = source.clone();
    for i in fingers {
        target[i].reference.translation *= 0.5;
    }
    target[weapon].reference.translation *= 2.0;
    target[weapon].reference.rotation = DQuat::from_rotation_z(0.4);
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    for step in 0..20 {
        let mut input = pose(&source);
        input[weapon].rotation = DQuat::from_rotation_y(step as f64 * 0.05);
        plan.prepare(&input, step, &mut frame).unwrap();
        let hand = plan.attachment_pose(0, &frame).unwrap();
        for index in [weapon, child] {
            let expected = hand * frame.source_world[0].inverse() * frame.source_world[index];
            matrix_near(plan.attachment_pose(index, &frame).unwrap(), expected);
        }
    }
}

#[test]
fn native_only_dummy_helpers_follow_the_nearest_common_parent() {
    let source = vec![
        bone("Master", None, DVec3::ZERO),
        bone("R_Hand", Some(0), DVec3::Y),
        bone("R_Weapon", Some(1), DVec3::X),
    ];
    let target = source[..2].to_vec();
    let names = ["Master", "R_Hand", "R_Weapon", "NativePropOnly"].map(str::to_owned);
    let parents = [None, Some(0), Some(1), Some(2)];
    let resolved = attachment_anchor(&source, &target, &names, &parents, 3).unwrap();
    assert_eq!(
        resolved,
        AttachmentAnchor {
            source: 1,
            target: 1,
            native: 1,
            weapon: true
        }
    );
    let mut invalid = parents;
    invalid[3] = Some(3);
    assert!(attachment_anchor(&source, &target, &names, &invalid, 3).is_none());
    assert!(attachment_anchor(&source, &target, &names, &parents, 10).is_none());
    assert!(attachment_anchor(&source, &target, &names, &parents[..2], 3).is_none());
}

#[test]
fn two_hand_grips_remain_coherent_with_different_palm_sizes_and_finger_motion() {
    let mut source = vec![
        bone("L_UpperArm", None, DVec3::new(-0.3, 1.0, 0.0)),
        bone("L_Forearm", Some(0), DVec3::new(0.3, -0.2, 0.0)),
        bone("L_Hand", Some(1), DVec3::new(-0.1, -0.2, 0.0)),
        bone("R_UpperArm", None, DVec3::new(0.3, 1.0, 0.0)),
        bone("R_Forearm", Some(3), DVec3::new(-0.3, -0.2, 0.0)),
        bone("R_Hand", Some(4), DVec3::new(0.1, -0.2, 0.0)),
    ];
    let left = append_fingers(&mut source, 2, "L", 1.0);
    let right = append_fingers(&mut source, 5, "R", 1.0);
    let mut target = source.clone();
    for b in &mut target[..6] {
        b.reference.translation *= 0.8;
    }
    for &i in &left {
        target[i].reference.translation *= 0.45;
    }
    for &i in &right {
        target[i].reference.translation *= 0.6;
    }
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let mut frame = plan.new_frame();
    for style in [2, 3] {
        for angle in [-0.3, 0.0, 0.3] {
            let mut input = pose(&source);
            for &i in left.iter().chain(&right) {
                input[i].rotation = DQuat::from_rotation_y(angle);
            }
            plan.prepare(&input, 0, &mut frame).unwrap();
            let (main, other) = if style == 2 { (2, 5) } else { (5, 2) };
            let sm = frame.source_world[main];
            let so = frame.source_world[other];
            let desired =
                attachment_frame(sm, plan.attachment_pose(main, &frame).unwrap(), sm, so).unwrap();
            plan.constrain(&mut frame, style, None).unwrap();
            matrix_near(plan.attachment_pose(other, &frame).unwrap(), desired);
            for (local, b) in frame.target_local.iter().zip(&target) {
                assert_eq!(local.translation, b.reference.translation);
                assert_eq!(local.scale, b.reference.scale);
            }
        }
    }
}

#[test]
fn two_hand_constraints_preserve_animated_grip_without_stretching_new_arms() {
    let source = vec![
        bone("L_UpperArm", None, DVec3::new(-0.3, 1.0, 0.0)),
        bone("L_Forearm", Some(0), DVec3::new(0.3, -0.2, 0.0)),
        bone("L_Hand", Some(1), DVec3::new(-0.1, -0.2, 0.0)),
        bone("R_UpperArm", None, DVec3::new(0.3, 1.0, 0.0)),
        bone("R_Forearm", Some(3), DVec3::new(-0.3, -0.2, 0.0)),
        bone("R_Hand", Some(4), DVec3::new(0.1, -0.2, 0.0)),
    ];
    let mut target = source.clone();
    for b in &mut target {
        b.reference.translation *= 0.8;
    }
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let mut frame = plan.new_frame();
    for style in [2, 3] {
        for angle in [0.0, 0.1, -0.1] {
            let mut pose: Vec<_> = source.iter().map(|b| b.reference).collect();
            pose[1].rotation = DQuat::from_rotation_z(angle);
            plan.prepare(&pose, 7, &mut frame).unwrap();
            let (main, other) = if style == 2 { (2, 5) } else { (5, 2) };
            let expected = frame.target_world[main].w_axis.truncate()
                + frame.source_world[other].w_axis.truncate()
                - frame.source_world[main].w_axis.truncate();
            let original_main = frame.target_world[main];
            plan.constrain(&mut frame, style, None).unwrap();
            assert!(
                frame.target_world[other]
                    .w_axis
                    .truncate()
                    .distance(expected)
                    < 1e-8
            );
            matrix_near(frame.target_world[main], original_main);
            for (local, bone) in frame.target_local.iter().zip(&target) {
                assert_eq!(local.translation, bone.reference.translation);
                assert_eq!(local.scale, bone.reference.scale);
            }
            // Rebuilding from animation never accumulates the correction.
            let first = frame.target_world.clone();
            plan.prepare(&pose, 8, &mut frame).unwrap();
            plan.constrain(&mut frame, style, None).unwrap();
            for (a, b) in frame.target_world.iter().zip(first) {
                matrix_near(*a, b);
            }
        }
    }
    let pose: Vec<_> = source.iter().map(|b| b.reference).collect();
    plan.prepare(&pose, 9, &mut frame).unwrap();
    let before = frame.target_world.clone();
    plan.constrain(&mut frame, 1, None).unwrap();
    assert_eq!(frame.target_world, before);
}

#[test]
fn weapon_attachment_ignores_authored_joint_axis_changes() {
    let source = vec![bone("Hand", None, DVec3::Y)];
    let mut target = source.clone();
    target[0].reference.rotation = DQuat::from_rotation_z(0.7);
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    plan.prepare(&[source[0].reference], 0, &mut frame).unwrap();
    matrix_near(
        plan.attachment_pose(0, &frame).unwrap(),
        DMat4::from_translation(DVec3::Y),
    );
}

#[test]
fn foot_constraint_keeps_native_contact_height_with_shorter_legs() {
    let source = vec![
        bone("L_Thigh", None, DVec3::Y),
        bone("L_Calf", Some(0), DVec3::new(0.05, -0.45, 0.0)),
        bone("L_Foot", Some(1), DVec3::new(-0.05, -0.4, 0.0)),
    ];
    let mut target = source.clone();
    for b in &mut target {
        b.reference.translation *= 0.8;
    }
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let mut frame = plan.new_frame();
    let mut pose: Vec<_> = source.iter().map(|b| b.reference).collect();
    pose[1].rotation = DQuat::from_rotation_x(0.25);
    plan.prepare(&pose, 0, &mut frame).unwrap();
    let source_foot = frame.source_world[2].w_axis.truncate();
    let normal = frame.source_rotation[2] * DVec3::Y;
    let contact = source_foot - normal * 0.15;
    plan.constrain(&mut frame, 0, Some(normal)).unwrap();
    let new_contact = frame.target_world[2].w_axis.truncate() - normal * 0.12;
    assert!((new_contact - contact).dot(normal).abs() < 1e-8);
    for (local, bone) in frame.target_local.iter().zip(&target) {
        assert_eq!(local.translation, bone.reference.translation);
    }
}

#[test]
fn foot_animation_rotation_does_not_change_the_measured_ground_plane() {
    let source = vec![
        bone("L_Thigh", None, DVec3::Y),
        bone("L_Calf", Some(0), DVec3::new(0.1, -0.45, 0.0)),
        bone("L_Foot", Some(1), DVec3::new(-0.1, -0.4, 0.0)),
    ];
    let mut target = source.clone();
    for bone in &mut target {
        bone.reference.translation *= 0.8;
    }
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    for angle in [-1.2, -0.4, 0.0, 0.7, 1.2] {
        let mut input = pose(&source);
        input[2].rotation = DQuat::from_rotation_x(angle);
        plan.prepare(&input, 0, &mut frame).unwrap();
        plan.constrain(&mut frame, 0, Some(DVec3::Y)).unwrap();
        assert!((frame.target_world[2].w_axis.y - 0.12).abs() < 1e-8);
        assert!(frame.target_rotation[2].dot(input[2].rotation).abs() > 1.0 - 1e-10);
    }
}

#[test]
fn rigid_weapon_follows_new_hand_in_world_without_changing_weapon_size() {
    let source =
        DMat4::from_rotation_translation(DQuat::from_rotation_z(0.3), DVec3::new(0.4, 1.0, 0.1));
    let target =
        DMat4::from_rotation_translation(DQuat::from_rotation_x(-0.7), DVec3::new(0.2, 0.7, 0.1));
    let grip =
        DMat4::from_rotation_translation(DQuat::from_rotation_y(1.0), DVec3::new(0.01, 0.03, 0.0));
    for scale in [0.5, 1.0, 2.0] {
        let world = DMat4::from_scale_rotation_translation(
            DVec3::splat(scale),
            DQuat::from_rotation_y(1.5),
            DVec3::new(100.0, 20.0, -30.0),
        );
        let original = world * source * grip;
        let result = attachment_frame(source, target, world * source, original).unwrap();
        matrix_near(result, world * target * grip);
        assert!((result.x_axis.length() - original.x_axis.length()).abs() < 1e-8);
    }
    assert!(attachment_frame(source, DMat4::ZERO, source, source).is_none());
}

#[test]
fn animation_reference_drops_placement_without_changing_authored_bone_lengths() {
    let mut source = vec![
        bone("Master", None, DVec3::new(-0.2, 0.08, 0.0)),
        bone("RootPos", Some(0), DVec3::Y),
        bone("RootRotY", Some(1), DVec3::ZERO),
        bone("RootRotXZ", Some(2), DVec3::ZERO),
        bone("Head", Some(3), DVec3::Y * 0.5),
    ];
    source[0].reference.rotation = DQuat::from_rotation_y(1.5);
    let normalized = animation_reference(source.clone());
    assert_eq!(normalized[0].reference.translation, DVec3::ZERO);
    assert_eq!(
        normalized[0].reference.rotation,
        source[0].reference.rotation
    );
    assert_eq!(
        normalized[1..]
            .iter()
            .map(|b| b.reference)
            .collect::<Vec<_>>(),
        source[1..].iter().map(|b| b.reference).collect::<Vec<_>>()
    );
    assert_eq!(source[0].reference.translation, DVec3::new(-0.2, 0.08, 0.0));
}

#[test]
fn split_body_roots_rotate_about_the_target_hip_not_the_old_leg_height() {
    let source = vec![
        bone("RootPos", None, DVec3::Y),
        bone("Pelvis", Some(0), DVec3::ZERO),
        bone("RootRotXZ", Some(0), DVec3::ZERO),
        bone("Spine", Some(2), DVec3::Y * 0.2),
        bone("Head", Some(3), DVec3::Y * 0.4),
    ];
    let target = vec![
        bone("Pelvis", None, DVec3::Y * 0.5),
        bone("Spine", None, DVec3::Y * 0.6),
        bone("Head", Some(1), DVec3::Y * 0.3),
    ];
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    let mut input = pose(&source);
    for angle in [-0.5, 0.0, 0.5] {
        let rotation = DQuat::from_rotation_x(angle);
        input[2].rotation = rotation;
        plan.prepare(&input, 1, &mut frame).unwrap();
        let out = frame.outputs().unwrap();
        let hip = out.model[0].w_axis.truncate();
        let spine = out.model[1].w_axis.truncate();
        near(spine - hip, rotation * (DVec3::Y * 0.1));
    }
}

#[test]
fn locomotion_profile_uses_body_chains_and_leaves_non_translation_lanes_alone() {
    let source = vec![
        bone("Head", None, DVec3::Y * 2.0),
        bone("L_Thigh", None, DVec3::Y),
        bone("L_Calf", Some(1), -DVec3::Y * 0.5),
        bone("L_Foot", Some(2), -DVec3::Y * 0.5),
        bone("R_Thigh", None, DVec3::Y),
        bone("R_Calf", Some(4), -DVec3::Y * 0.5),
        bone("R_Foot", Some(5), -DVec3::Y * 0.5),
        bone("tall_ears", Some(0), DVec3::Y * 100.0),
    ];
    let mut target = source.clone();
    target[0].reference.translation *= 0.6;
    for i in [2, 3, 5, 6] {
        target[i].reference.translation *= 0.75;
    }
    let profile = MotionProfile::new(&source, &target).unwrap();
    assert_eq!(profile.leg_ratio, 0.75);
    assert_eq!(profile.height_ratio, 0.6);
    assert_eq!(
        profile.displacement([2.0, -4.0, 8.0, 123.0]),
        Some([1.5, -3.0, 6.0, 123.0])
    );
    assert_eq!(profile.camera_height(2.0), Some(1.2));
    assert!(profile.displacement([f32::NAN, 0.0, 0.0, 1.0]).is_none());
    assert!(MotionProfile::new(&source, &target[..3]).is_none());
}

#[test]
#[ignore = "requires private ERCS_RETARGET_SOURCE_TOML and ERCS_RETARGET_MESH_TOML"]
fn real_player_reference_to_large_equipment_and_physics_alignment() {
    #[derive(serde::Deserialize)]
    struct Source {
        bones: Vec<SourceBone>,
    }
    #[derive(serde::Deserialize)]
    struct SourceBone {
        name: String,
        parent: i32,
        translation: [f64; 3],
        rotation: [f64; 4],
        scale: [f64; 3],
    }
    #[derive(serde::Deserialize)]
    struct Mesh {
        nodes: Vec<MeshBone>,
    }
    #[derive(serde::Deserialize)]
    struct MeshBone {
        name: String,
        parent: i32,
        local_row_major: [f64; 16],
    }
    let source: Source = toml::from_str(
        &std::fs::read_to_string(std::env::var("ERCS_RETARGET_SOURCE_TOML").unwrap()).unwrap(),
    )
    .unwrap();
    let source: Vec<_> = source
        .bones
        .into_iter()
        .map(|b| Bone {
            name: b.name,
            parent: (b.parent >= 0).then_some(b.parent as usize),
            reference: LocalPose {
                translation: DVec3::from_array(b.translation),
                rotation: DQuat::from_array(b.rotation).normalize(),
                scale: DVec3::from_array(b.scale),
            },
        })
        .collect();
    let mesh: Mesh = toml::from_str(
        &std::fs::read_to_string(std::env::var("ERCS_RETARGET_MESH_TOML").unwrap()).unwrap(),
    )
    .unwrap();
    let mut mesh: Vec<_> = mesh
        .nodes
        .into_iter()
        .map(|b| Bone {
            name: b.name,
            parent: (b.parent >= 0).then_some(b.parent as usize),
            reference: crate::equipment_retarget_pose::local(DMat4::from_cols_array(
                &b.local_row_major,
            ))
            .unwrap(),
        })
        .collect();
    let original = equipment_mesh_reference(&source, &mesh).unwrap();
    let detected = proportion_difference(&source, &original).unwrap();
    println!("loaded_reference_proportions={detected:?}");
    for b in &mut mesh {
        if ["L_Forearm", "L_Hand", "R_Forearm", "R_Hand"].contains(&b.name.as_str()) {
            b.reference.translation *= 0.7;
        }
    }
    let target = equipment_mesh_reference(&source, &mesh).unwrap();
    assert!(proportion_difference(&source, &target).unwrap().changed >= 4);
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let physical = equipment_physics_reference(&target, &source).unwrap();
    let physics_plan = Plan::new(source.clone(), physical.clone(), &[]).unwrap();
    let mapping: Vec<_> = physical
        .iter()
        .map(|b| target.iter().position(|t| t.name == b.name))
        .collect();
    let matched = mapping.iter().flatten().count();
    assert!(matched > 80);
    let mut frame = plan.new_frame();
    let mut physics_frame = physics_plan.new_frame();
    let mut input = pose(&source);
    let base = input.clone();
    let pelvis = source.iter().position(|b| b.name == "Pelvis").unwrap();
    let master = source.iter().position(|b| b.name == "Master").unwrap();
    let started = std::time::Instant::now();
    for generation in 0..600 {
        let phase = (generation as f64 * 0.02).sin();
        input[pelvis].rotation = base[pelvis].rotation * DQuat::from_rotation_z(phase * 0.4);
        input[master].translation = base[master].translation + DVec3::new(phase, phase * 0.1, 0.0);
        plan.prepare(&input, generation, &mut frame).unwrap();
        physics_plan
            .prepare(&input, generation, &mut physics_frame)
            .unwrap();
        let output = frame.outputs().unwrap();
        physics_plan
            .align_model(output.model, &mapping, &mut physics_frame)
            .unwrap();
        for (i, m) in mapping.iter().enumerate() {
            if let Some(m) = m {
                matrix_near(physics_frame.outputs().unwrap().model[i], output.model[*m]);
            }
        }
        for (bone, local) in target.iter().zip(output.local) {
            if bone.parent.is_some() {
                near(bone.reference.translation, local.translation);
            }
        }
    }
    println!(
        "actual_reference_source={} target={} matched={} frames=600 elapsed_ms={:.3}; authored data + synthetic motion, not native simulation",
        source.len(),
        target.len(),
        matched,
        started.elapsed().as_secs_f64() * 1000.0
    );
}

#[test]
#[ignore = "requires private ERCS_RETARGET_MESH_TOML; no game or physical simulation"]
fn real_flver_reference_replay_and_pose_cost() {
    #[derive(serde::Deserialize)]
    struct Mesh {
        nodes: Vec<Node>,
    }
    #[derive(serde::Deserialize)]
    struct Node {
        name: String,
        parent: i32,
        local_row_major: [f64; 16],
    }
    let path = std::env::var("ERCS_RETARGET_MESH_TOML").expect("fixture path");
    let mesh: Mesh = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let source: Vec<_> = mesh
        .nodes
        .into_iter()
        .map(|n| Bone {
            name: n.name,
            parent: (n.parent >= 0).then_some(n.parent as usize),
            reference: crate::equipment_retarget_pose::local(DMat4::from_cols_array(
                &n.local_row_major,
            ))
            .unwrap(),
        })
        .collect();
    assert!(source.len() > 512, "exercise large equipment skeleton");
    let mut target = source.clone();
    let mut changed = 0;
    for bone in &mut target {
        if ["L_Forearm", "R_Forearm", "L_Hand", "R_Hand"].contains(&bone.name.as_str()) {
            bone.reference.translation *= 0.7;
            changed += 1;
        }
    }
    assert_eq!(changed, 4);
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let mut input = pose(&source);
    let original = input.clone();
    let mut frame = plan.new_frame();
    plan.prepare(&input, 0, &mut frame).unwrap();
    for &matrix in frame.outputs().unwrap().skinning {
        matrix_near(matrix, DMat4::IDENTITY);
    }
    let rotation_indices: Vec<_> = source
        .iter()
        .enumerate()
        .filter(|(_, b)| b.name.ends_with("UpperArm"))
        .map(|(i, _)| i)
        .collect();
    let start = std::time::Instant::now();
    for generation in 1..=600 {
        for &i in &rotation_indices {
            input[i].rotation = original[i].rotation
                * DQuat::from_rotation_z((generation as f64 * 0.02).sin() * 0.5);
        }
        let before = input.clone();
        plan.prepare(&input, generation, &mut frame).unwrap();
        assert_eq!(input, before);
        for (bone, local) in target.iter().zip(frame.outputs().unwrap().local) {
            if bone.parent.is_some() {
                near(local.translation, bone.reference.translation);
            }
        }
    }
    println!(
        "real_flver_bones={} frames=600 elapsed_ms={:.3} (includes assertions; not game-frame performance)",
        source.len(),
        start.elapsed().as_secs_f64() * 1000.0
    );
}

fn bone(name: &str, parent: Option<usize>, translation: DVec3) -> Bone {
    Bone {
        name: name.into(),
        parent,
        reference: LocalPose {
            translation,
            ..LocalPose::IDENTITY
        },
    }
}

fn arm(upper: f64, lower: f64) -> Vec<Bone> {
    vec![
        bone("shoulder", None, DVec3::ZERO),
        bone("elbow", Some(0), DVec3::X * upper),
        bone("wrist", Some(1), DVec3::X * lower),
    ]
}

fn named_arm() -> Vec<Bone> {
    vec![
        bone("L_UpperArm", None, DVec3::ZERO),
        bone("L_Forearm", Some(0), DVec3::X * 2.0),
        bone("L_Hand", Some(1), DVec3::X),
        bone("hair", Some(2), DVec3::Y * 0.2),
    ]
}

#[test]
fn automatic_detection_ignores_reference_pose_noise_and_cloth_only_changes() {
    let source = named_arm();
    let mut target = source.clone();
    target[0].reference.translation = DVec3::new(12.0, 10.0, -9.0);
    target[0].reference.rotation = DQuat::from_rotation_z(1.2);
    target[1].reference.rotation = DQuat::from_rotation_y(0.8);
    target[2].reference.translation *= 1.00001;
    target[3].reference.translation *= 19.0;
    assert_eq!(
        proportion_difference(&source, &target).unwrap(),
        ProportionDifference {
            compared: 2,
            changed: 0
        }
    );
}

#[test]
fn automatic_detection_finds_short_limbs_and_uniform_authored_scale() {
    let source = named_arm();
    let mut target = source.clone();
    target[1].reference.translation *= 0.7;
    assert_eq!(proportion_difference(&source, &target).unwrap().changed, 1);
    for scale in [0.001, 0.5, 1.2, 200.0] {
        let mut target = source.clone();
        target[0].reference.scale = DVec3::splat(scale);
        assert_eq!(proportion_difference(&source, &target).unwrap().changed, 2);
    }
}

#[test]
fn automatic_detection_handles_reordered_split_roots_and_missing_helpers() {
    let source = vec![
        bone("Pelvis", None, DVec3::Y),
        bone("helper", Some(0), DVec3::ZERO),
        bone("Spine", Some(1), DVec3::Y * 0.5),
        bone("Spine1", Some(2), DVec3::Y * 0.2),
    ];
    let mesh = vec![
        bone("Spine1", Some(1), DVec3::Y * 0.2),
        bone("Spine", None, DVec3::Y * 1.5),
        bone("Pelvis", None, DVec3::Y),
    ];
    let target = equipment_mesh_reference(&source, &mesh).unwrap();
    assert_eq!(
        proportion_difference(&source, &target).unwrap(),
        ProportionDifference {
            compared: 2,
            changed: 0
        }
    );
    let mut mesh = mesh;
    mesh[0].reference.translation *= 0.6;
    let target = equipment_mesh_reference(&source, &mesh).unwrap();
    assert_eq!(proportion_difference(&source, &target).unwrap().changed, 1);
}

#[test]
fn automatic_detection_does_not_infer_body_proportions_from_unmapped_or_invalid_data() {
    let source = named_arm();
    let target = vec![bone("custom_hair", None, DVec3::X * 20.0)];
    assert_eq!(
        proportion_difference(&source, &target).unwrap(),
        ProportionDifference {
            compared: 0,
            changed: 0
        }
    );
    let mut invalid = source.clone();
    invalid[1].reference.translation.x = f64::NAN;
    assert!(proportion_difference(&source, &invalid).is_err());
}

#[test]
fn split_flver_roots_preserve_target_lengths_and_follow_pelvis_motion() {
    let source = vec![
        bone("Master", None, DVec3::ZERO),
        bone("Pelvis", Some(0), DVec3::Y),
        bone("Spine", Some(1), DVec3::Y * 0.5),
    ];
    let mesh = vec![
        bone("Spine", None, DVec3::Y * 1.1),
        bone("Pelvis", None, DVec3::Y * 0.8),
    ];
    let target = equipment_mesh_reference(&source, &mesh).unwrap();
    assert_eq!(target[0].parent, Some(1));
    near(target[0].reference.translation, DVec3::Y * 0.3);
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let mut frame = plan.new_frame();
    let mut input = pose(&source);
    input[0].translation = DVec3::new(4.0, 2.0, 0.0);
    input[1].rotation = DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2);
    plan.prepare(&input, 1, &mut frame).unwrap();
    let output = frame.outputs().unwrap();
    near(output.model[1].w_axis.truncate(), DVec3::new(4.0, 2.8, 0.0));
    near(output.model[0].w_axis.truncate(), DVec3::new(3.7, 2.8, 0.0));
    let cloth = equipment_physics_reference(&target, &source).unwrap();
    let cloth_plan = Plan::new(source, cloth, &[]).unwrap();
    let mut cloth_frame = cloth_plan.new_frame();
    cloth_plan.prepare(&input, 1, &mut cloth_frame).unwrap();
    cloth_plan
        .align_model(output.model, &[None, Some(1), Some(0)], &mut cloth_frame)
        .unwrap();
    let physical = cloth_frame.outputs().unwrap();
    matrix_near(physical.model[1], output.model[1]);
    matrix_near(physical.model[2], output.model[0]);
    assert!(
        ((output.model[0].w_axis.truncate() - output.model[1].w_axis.truncate()).length() - 0.3)
            .abs()
            < 1e-8
    );
}

fn pose(bones: &[Bone]) -> Vec<LocalPose> {
    bones.iter().map(|b| b.reference).collect()
}

#[test]
fn equipment_rootpos_child_preserves_crouch_motion_and_limb_lengths() {
    let source = vec![
        bone("Master", None, DVec3::ZERO),
        bone("RootPos", Some(0), DVec3::Y),
        bone("RootRotY", Some(1), DVec3::ZERO),
        bone("Pelvis", Some(2), DVec3::ZERO),
        bone("Spine", Some(2), DVec3::Y * 0.2),
        bone("L_Thigh", Some(3), DVec3::X * 0.1),
        bone("L_Calf", Some(5), -DVec3::Y * 0.5),
        bone("L_Foot", Some(6), -DVec3::Y * 0.5),
    ];
    let target = vec![
        bone("Master", None, DVec3::ZERO),
        bone("RootPos", Some(0), DVec3::Y * 0.8),
        bone("Pelvis", Some(1), DVec3::Y * 0.4),
        bone("Spine", Some(2), DVec3::Y * 0.15),
        bone("L_Thigh", Some(2), DVec3::X * 0.08),
        bone("L_Calf", Some(4), -DVec3::Y * 0.6),
        bone("L_Foot", Some(5), -DVec3::Y * 0.6),
    ];
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let mut frame = plan.new_frame();
    let mut input = pose(&source);
    for (generation, height) in [1.0, 0.6, 0.1, 0.8, 1.0].into_iter().enumerate() {
        input[1].translation.y = height;
        input[0].translation = DVec3::new(4.0, 0.0, -2.0);
        input[0].rotation = DQuat::from_rotation_y(0.7);
        input[6].rotation = DQuat::from_rotation_x((1.0 - height) * 0.8);
        plan.prepare(&input, generation as u64, &mut frame).unwrap();
        let out = frame.outputs().unwrap();
        let hip = out.model[2].w_axis.truncate();
        let expected_height = 1.2 + height - 1.0;
        assert!(
            (hip.y - expected_height).abs() < 1e-8,
            "equipment hip must follow the RootPos child translation; got {}, expected {expected_height}",
            hip.y
        );
        near(out.local[5].translation, target[5].reference.translation);
        near(out.local[6].translation, target[6].reference.translation);
        near(
            out.model[3].w_axis.truncate() - hip,
            out.model[2].transform_vector3(target[3].reference.translation),
        );
    }
}

#[test]
#[ignore = "requires private ERCS_RETARGET_LOWER_BODY fixture directory"]
fn captured_equipment_rootpos_crouch_reaches_mesh_and_physics() {
    use crate::equipment_retarget_pose as decoder;
    let directory = std::path::PathBuf::from(std::env::var("ERCS_RETARGET_LOWER_BODY").unwrap());
    let raw = std::fs::read(directory.join("pose.bin")).unwrap();
    let word = |bytes: &[u8]| usize::from_le_bytes(bytes.try_into().unwrap());
    let source_context = word(&raw[..8]);
    let mut blocks = Vec::new();
    let mut offset = 24;
    while offset < raw.len() {
        let address = word(&raw[offset..offset + 8]);
        let length = word(&raw[offset + 8..offset + 16]);
        blocks.push((address, raw[offset + 16..offset + 16 + length].to_vec()));
        offset += 16 + length;
    }
    let read = |address: usize, output: &mut [u8]| {
        let Some((start, bytes)) = blocks.iter().find(|(start, bytes)| {
            address >= *start && address - start + output.len() <= bytes.len()
        }) else {
            return false;
        };
        output.copy_from_slice(&bytes[address - start..address - start + output.len()]);
        true
    };
    let metadata = decoder::pointer(&read, source_context).unwrap();
    let (identity, source) = decoder::skeleton(&read, metadata).unwrap();
    let source = animation_reference(source);
    #[derive(serde::Deserialize)]
    struct Node {
        name: String,
        parent: i16,
        local_row_major: [f64; 16],
    }
    #[derive(serde::Deserialize)]
    struct Mesh {
        nodes: Vec<Node>,
    }
    let text = std::fs::read_to_string(directory.join("mesh/nodes.toml")).unwrap();
    let nodes: Mesh = toml::from_str(&text).unwrap();
    let mesh: Vec<_> = nodes
        .nodes
        .into_iter()
        .map(|node| Bone {
            name: node.name,
            parent: (node.parent >= 0).then_some(node.parent as usize),
            reference: decoder::local(DMat4::from_cols_array(&node.local_row_major)).unwrap(),
        })
        .collect();
    let target = equipment_mesh_reference(&source, &mesh).unwrap();
    let mut scratch = decoder::PoseScratch::default();
    scratch
        .capture(&read, source_context, &identity, &source, 1.0)
        .unwrap();
    let pelvis = target
        .iter()
        .position(|bone| bone.name == "Pelvis")
        .unwrap();
    let root = source
        .iter()
        .position(|bone| bone.name == "RootPos")
        .unwrap();
    let drop = scratch.local[root].translation.y - source[root].reference.translation.y;
    assert!(drop < -0.5, "fixture must contain the actual deep crouch");
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let mut frame = plan.new_frame();
    plan.prepare(&pose(&source), 0, &mut frame).unwrap();
    let standing = frame.outputs().unwrap().model[pelvis].w_axis.y;
    plan.prepare(&scratch.local, 1, &mut frame).unwrap();
    let crouching = frame.outputs().unwrap().model[pelvis].w_axis.y;
    assert!(
        (crouching - standing - drop).abs() < 1e-4,
        "live crouch displacement lost: standing={standing}, crouching={crouching}, source_drop={drop}"
    );
    for grounded in [false, true] {
        plan.prepare(&scratch.local, 1, &mut frame).unwrap();
        plan.constrain(&mut frame, 0, grounded.then_some(DVec3::Y))
            .unwrap();
        assert!((frame.outputs().unwrap().model[pelvis].w_axis.y - crouching).abs() < 1e-8);
        for side in ["L", "R"] {
            for joint in ["Calf", "Foot"] {
                let name = format!("{side}_{joint}");
                let i = target.iter().position(|bone| bone.name == name).unwrap();
                near(
                    frame.outputs().unwrap().local[i].translation,
                    target[i].reference.translation,
                );
            }
        }
    }
    let cloth_context = word(&raw[8..16]);
    let cloth_metadata = decoder::pointer(&read, cloth_context).unwrap();
    let (_, cloth_bones) = decoder::skeleton(&read, cloth_metadata).unwrap();
    let physical = equipment_physics_reference(&target, &cloth_bones).unwrap();
    let map: Vec<_> = physical
        .iter()
        .map(|b| target.iter().position(|t| t.name == b.name))
        .collect();
    let physics = Plan::new(source.clone(), physical, &[]).unwrap();
    let mut physical_frame = physics.new_frame();
    physics
        .prepare(&scratch.local, 1, &mut physical_frame)
        .unwrap();
    physics
        .align_model(frame.outputs().unwrap().model, &map, &mut physical_frame)
        .unwrap();
    for (i, mesh_index) in map
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.map(|m| (i, m)))
    {
        matrix_near(
            physical_frame.outputs().unwrap().model[i],
            frame.outputs().unwrap().model[mesh_index],
        );
    }
    println!(
        "source_bones={} mesh_bones={} source_drop={drop:.6} equipment_drop={:.6}",
        source.len(),
        target.len(),
        crouching - standing
    );
}

fn near(a: DVec3, b: DVec3) {
    assert!((a - b).length() < 1e-8, "{a:?} != {b:?}");
}

fn matrix_near(a: DMat4, b: DMat4) {
    for (x, y) in a.to_cols_array().iter().zip(b.to_cols_array()) {
        assert!((x - y).abs() < 1e-8, "{a:?} != {b:?}");
    }
}

#[test]
fn reference_pose_restores_authored_mesh_instead_of_source_lengths() {
    let source = arm(2.0, 1.0);
    let target = arm(0.7, 0.4);
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    plan.prepare(&pose(&source), 1, &mut frame).unwrap();
    let out = frame.outputs().unwrap();
    near(out.model[2].w_axis.truncate(), DVec3::X * 1.1);
    // A bind-pose vertex at the authored wrist must remain at its authored
    // position. Directly reusing the source pose moves it to x=3 (regression).
    near(
        out.skinning[2].transform_point3(DVec3::X * 1.1),
        DVec3::X * 1.1,
    );
    for matrix in out.skinning {
        matrix_near(*matrix, DMat4::IDENTITY);
    }
}

#[test]
fn bending_uses_target_lengths_and_preserves_source_across_repeated_frames() {
    let source = arm(2.0, 1.0);
    let plan = Plan::new(source.clone(), arm(0.7, 0.4), &[]).unwrap();
    let mut input = pose(&source);
    input[0].rotation = DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2);
    input[1].rotation = DQuat::from_rotation_z(-std::f64::consts::FRAC_PI_2);
    let original = input.clone();
    let mut frame = plan.new_frame();
    for generation in 0..600 {
        plan.prepare(&input, generation, &mut frame).unwrap();
        let out = frame.outputs().unwrap();
        near(out.model[1].w_axis.truncate(), DVec3::new(0.0, 0.7, 0.0));
        near(out.model[2].w_axis.truncate(), DVec3::new(0.4, 0.7, 0.0));
        assert_eq!(out.generation, generation);
        assert_eq!(input, original);
    }
    plan.prepare(&pose(&source), 601, &mut frame).unwrap();
    near(
        frame.outputs().unwrap().model[2].w_axis.truncate(),
        DVec3::X * 1.1,
    );
}

#[test]
fn target_reference_axes_and_unmapped_child_are_preserved() {
    let source = arm(2.0, 1.0);
    let mut target = arm(0.7, 0.4);
    target[1].reference.rotation = DQuat::from_rotation_x(0.8);
    target.push(bone("target_accessory", Some(2), DVec3::Y * 0.2));
    let plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    let mut frame = plan.new_frame();
    plan.prepare(&pose(&source), 0, &mut frame).unwrap();
    let out = frame.outputs().unwrap();
    for matrix in out.skinning {
        matrix_near(*matrix, DMat4::IDENTITY);
    }
    let mut animation = pose(&source);
    animation[0].rotation = DQuat::from_rotation_z(0.4);
    plan.prepare(&animation, 1, &mut frame).unwrap();
    let out = frame.outputs().unwrap();
    let expected =
        DQuat::from_rotation_z(0.4) * DVec3::new(1.1, 0.2 * 0.8f64.cos(), 0.2 * 0.8f64.sin());
    near(out.model[3].w_axis.truncate(), expected);
}

#[test]
fn reordered_bones_map_by_name_without_relying_on_source_indices() {
    let source = arm(2.0, 1.0);
    let target = vec![
        bone("wrist", Some(2), DVec3::X * 0.4),
        bone("shoulder", None, DVec3::ZERO),
        bone("elbow", Some(1), DVec3::X * 0.7),
    ];
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    plan.prepare(&pose(&source), 1, &mut frame).unwrap();
    near(
        frame.outputs().unwrap().model[0].w_axis.truncate(),
        DVec3::X * 1.1,
    );
}

#[test]
fn root_motion_is_not_scaled_by_leg_or_arm_length() {
    let source = arm(2.0, 1.0);
    let plan = Plan::new(source.clone(), arm(0.7, 0.4), &[]).unwrap();
    let mut input = pose(&source);
    input[0].translation = DVec3::new(12.0, 0.25, -3.0);
    let mut frame = plan.new_frame();
    plan.prepare(&input, 1, &mut frame).unwrap();
    near(
        frame.outputs().unwrap().model[2].w_axis.truncate(),
        DVec3::new(13.1, 0.25, -3.0),
    );
}

#[test]
fn merged_player_cloth_skeleton_is_calibrated_to_equipment_reference() {
    let source = arm(2.0, 1.0);
    let mesh = arm(0.7, 0.4);
    let mut native_cloth = source.clone();
    native_cloth.push(bone("sleeve_anchor", Some(2), DVec3::Y * 0.05));
    let target_cloth = equipment_physics_reference(&mesh, &native_cloth).unwrap();
    let plan = Plan::new(source.clone(), target_cloth, &[]).unwrap();
    let mut frame = plan.new_frame();
    plan.prepare(&pose(&source), 1, &mut frame).unwrap();
    let output = frame.outputs().unwrap();
    near(output.model[2].w_axis.truncate(), DVec3::X * 1.1);
    near(
        output.model[3].w_axis.truncate(),
        DVec3::new(1.1, 0.05, 0.0),
    );
    // The original game skeleton and target-authored attachment dimensions
    // remain unchanged; only a private pose calibration is created.
    near(native_cloth[1].reference.translation, DVec3::X * 2.0);
    near(native_cloth[3].reference.translation, DVec3::Y * 0.05);
}

#[test]
fn animated_translation_is_distinct_from_reference_offset() {
    let source = arm(2.0, 1.0);
    let target = arm(0.7, 0.4);
    let rules = [BoneRule {
        target: "elbow".into(),
        source: "elbow".into(),
        translation: TranslationMode::Scaled,
    }];
    let plan = Plan::new(source.clone(), target, &rules).unwrap();
    let mut input = pose(&source);
    input[1].translation += DVec3::Y * 0.2;
    let mut frame = plan.new_frame();
    plan.prepare(&input, 1, &mut frame).unwrap();
    near(
        frame.outputs().unwrap().local[1].translation,
        DVec3::new(0.7, 0.07, 0.0),
    );
}

#[test]
fn render_and_authored_cloth_attachment_share_the_same_motion() {
    let source = arm(2.0, 1.0);
    let plan = Plan::new(source.clone(), arm(0.7, 0.4), &[]).unwrap();
    let mut input = pose(&source);
    let mut frame = plan.new_frame();
    let offset = DVec3::new(0.0, 0.05, 0.0);
    let binding = PhysicsBinding {
        bone: 2,
        local: DMat4::from_translation(offset),
    };
    let vertex_in_bind = DVec3::X * 1.1 + offset;
    let mut collider = [DMat4::IDENTITY];
    for generation in 0..600 {
        let time = generation as f64 / 60.0;
        input[0].rotation = DQuat::from_rotation_z(time.sin());
        input[1].rotation = DQuat::from_rotation_z(time.cos());
        plan.prepare(&input, generation, &mut frame).unwrap();
        let out = frame.outputs().unwrap();
        out.physics_frames(&[binding], &mut collider).unwrap();
        near(
            collider[0].transform_point3(DVec3::ZERO),
            out.skinning[2].transform_point3(vertex_in_bind),
        );
        assert!(rigid(collider[0]));
        // Target-local collider/attachment distances are stable in motion.
        near(
            collider[0]
                .inverse()
                .transform_point3(out.model[2].w_axis.truncate()),
            -offset,
        );
    }
}

#[test]
fn invalid_batch_cannot_publish_stale_pose_or_partially_write_physics() {
    let source = arm(2.0, 1.0);
    let plan = Plan::new(source.clone(), arm(0.7, 0.4), &[]).unwrap();
    let mut input = pose(&source);
    let mut frame = plan.new_frame();
    plan.prepare(&input, 1, &mut frame).unwrap();
    let valid = PhysicsBinding {
        bone: 1,
        local: DMat4::IDENTITY,
    };
    let invalid = PhysicsBinding {
        bone: 999,
        local: DMat4::IDENTITY,
    };
    let mut output = [DMat4::from_translation(DVec3::splat(123.0)); 2];
    let saved = output;
    assert_eq!(
        frame
            .outputs()
            .unwrap()
            .physics_frames(&[valid, invalid], &mut output),
        Err(Error::InvalidBinding)
    );
    assert_eq!(output, saved);
    input[2].rotation.x = f64::NAN;
    assert_eq!(plan.prepare(&input, 2, &mut frame), Err(Error::InvalidPose));
    assert!(frame.outputs().is_none());
}

#[test]
fn invalid_topology_duplicate_names_and_nonrigid_physics_are_rejected() {
    let source = arm(2.0, 1.0);
    let mut target = arm(0.7, 0.4);
    target[0].parent = Some(2);
    assert!(matches!(
        Plan::new(source.clone(), target, &[]),
        Err(Error::InvalidSkeleton)
    ));
    let mut target = arm(0.7, 0.4);
    target[2].name = "elbow".into();
    assert!(matches!(
        Plan::new(source.clone(), target, &[]),
        Err(Error::DuplicateName)
    ));
    let mut target = arm(0.7, 0.4);
    target[1].reference.scale.x = 2.0;
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    plan.prepare(&pose(&source), 1, &mut frame).unwrap();
    assert_eq!(
        frame.outputs().unwrap().physics_frames(
            &[PhysicsBinding {
                bone: 1,
                local: DMat4::IDENTITY
            }],
            &mut [DMat4::IDENTITY]
        ),
        Err(Error::NonRigidPhysicsFrame)
    );
}

#[test]
fn body_dummy_offset_follows_bone_length_without_resizing_effect_or_double_global_scale() {
    let source = DMat4::from_translation(DVec3::X * 2.0);
    let target = DMat4::from_rotation_translation(DQuat::from_rotation_z(0.4), DVec3::X * 0.7);
    let world = DMat4::from_scale_rotation_translation(
        DVec3::splat(3.0),
        DQuat::from_rotation_y(0.6),
        DVec3::new(5.0, 6.0, 7.0),
    );
    let native = world * source;
    let effect =
        native * DMat4::from_rotation_translation(DQuat::from_rotation_x(0.3), DVec3::X * 0.5);
    let out = body_attachment_frame(source, target, native, effect, 0.4).unwrap();
    let expected = world
        * target
        * DMat4::from_rotation_translation(DQuat::from_rotation_x(0.3), DVec3::X * 0.2);
    for (a, b) in out
        .to_cols_array()
        .into_iter()
        .zip(expected.to_cols_array())
    {
        assert!((a - b).abs() < 1e-8);
    }
    assert_eq!(
        body_attachment_frame(source, target, native, effect, f64::NAN),
        None
    );
}

#[test]
fn arm_bind_swing_is_calibrated_without_widening_shoulders_or_lengthening_arms() {
    for side in ["L", "R"] {
        let sign = if side == "L" { 1.0 } else { -1.0 };
        let source = vec![
            bone(
                &format!("{side}_UpperArm"),
                None,
                DVec3::new(sign * 0.17, 1.4, 0.0),
            ),
            bone(&format!("{side}_Forearm"), Some(0), DVec3::X * 0.28),
            bone(&format!("{side}_Hand"), Some(1), DVec3::X * 0.23),
        ];
        let mut target = source.clone();
        target[0].reference.translation.x *= 0.52;
        target[0].reference.rotation = DQuat::from_rotation_z(sign * 0.075);
        target[1].reference.rotation = DQuat::from_rotation_y(0.15) * DQuat::from_rotation_x(0.4);
        target[1].reference.translation *= 0.72;
        target[2].reference.translation *= 0.73;
        let mut plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
        let bind = plan.inverse_bind.clone();
        plan.calibrate_limb_directions();
        plan.calibrate_limb_directions();
        let mut frame = plan.new_frame();
        for step in 0..60 {
            let mut input = pose(&source);
            input[0].rotation =
                DQuat::from_rotation_z(step as f64 * 0.04) * DQuat::from_rotation_x(0.3);
            input[1].rotation = DQuat::from_rotation_y(step as f64 * 0.03);
            plan.prepare(&input, step, &mut frame).unwrap();
            near(
                frame.target_world[0].w_axis.truncate(),
                target[0].reference.translation,
            );
            for i in 0..2 {
                let a = (frame.source_world[i + 1].w_axis - frame.source_world[i].w_axis)
                    .truncate()
                    .normalize();
                let b = (frame.target_world[i + 1].w_axis - frame.target_world[i].w_axis)
                    .truncate()
                    .normalize();
                assert!(
                    a.distance(b) < 1e-8,
                    "reference arm swing leaked into animation: {a:?} vs {b:?}"
                );
            }
            for (i, b) in target.iter().enumerate() {
                assert_eq!(frame.target_local[i].translation, b.reference.translation);
                assert_eq!(frame.target_local[i].scale, b.reference.scale);
            }
            assert_eq!(plan.inverse_bind, bind);
        }
    }
}

#[test]
fn weapon_dummy_classification_follows_weapon_subtree_and_preserves_full_length_offsets() {
    let source = vec![
        bone("R_Hand", None, DVec3::Y),
        bone("R_Weapon", Some(0), DVec3::X * 0.1),
        bone("BladeTip", Some(1), DVec3::X * 2.0),
        bone("R_Finger1", Some(0), DVec3::X * 0.05),
    ];
    let mut target = source.clone();
    target[0].reference.translation *= 0.7;
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    assert!(!plan.weapon_attachment_source(0));
    assert!(plan.weapon_attachment_source(1));
    assert!(plan.weapon_attachment_source(2));
    assert!(!plan.weapon_attachment_source(3));
    let mut frame = plan.new_frame();
    plan.prepare(&pose(&source), 0, &mut frame).unwrap();
    let source = frame.source_world[0];
    let target = plan.attachment_pose(0, &frame).unwrap();
    let dummy = source * DMat4::from_translation(DVec3::X * 2.1);
    let mapped = body_attachment_frame(source, target, source, dummy, 1.0).unwrap();
    near(
        mapped.w_axis.truncate(),
        target.transform_point3(DVec3::X * 2.1),
    );
}

#[test]
fn arm_calibration_carries_twist_helpers_without_freezing_their_animation() {
    let source = vec![
        bone("R_UpperArm", None, DVec3::Y),
        bone("R_Forearm", Some(0), DVec3::X),
        bone("R_Hand", Some(1), DVec3::X),
        bone("R_UpperArmTwist", Some(0), DVec3::X * 0.3),
    ];
    let mut target = source.clone();
    target[0].reference.rotation = DQuat::from_rotation_z(0.3);
    target[1].reference.translation *= 0.7;
    target[2].reference.translation *= 0.8;
    target[3].reference.translation *= 0.7;
    target[3].reference.rotation = DQuat::from_rotation_x(0.2);
    let mut plan = Plan::new(source.clone(), target, &[]).unwrap();
    plan.calibrate_limb_directions();
    let mut f = plan.new_frame();
    for i in 0..30 {
        let mut input = pose(&source);
        input[0].rotation = DQuat::from_rotation_y(0.5);
        input[3].rotation = DQuat::from_rotation_x(i as f64 * 0.01);
        plan.prepare(&input, i, &mut f).unwrap();
        let expected = f.target_rotation[0] * input[3].rotation * DQuat::from_rotation_x(0.2);
        assert!(
            f.target_rotation[3].dot(expected).abs() > 1.0 - 1e-10,
            "twist helper retained old arm swing"
        );
    }
}

#[test]
#[ignore = "requires private ERCS_RETARGET_LOWER_BODY fixture directory"]
fn captured_sitting_limb_swing_matches_animation() {
    use crate::equipment_retarget_pose as decoder;
    let directory = std::path::PathBuf::from(std::env::var("ERCS_RETARGET_LOWER_BODY").unwrap());
    let raw = std::fs::read(directory.join("pose.bin")).unwrap();
    let word = |bytes: &[u8]| usize::from_le_bytes(bytes.try_into().unwrap());
    let source_context = word(&raw[..8]);
    let mut blocks = Vec::new();
    let mut offset = 24;
    while offset < raw.len() {
        let address = word(&raw[offset..offset + 8]);
        let length = word(&raw[offset + 8..offset + 16]);
        blocks.push((address, raw[offset + 16..offset + 16 + length].to_vec()));
        offset += 16 + length;
    }
    let read = |address: usize, output: &mut [u8]| {
        let Some((start, bytes)) = blocks.iter().find(|(start, bytes)| {
            address >= *start && address - start + output.len() <= bytes.len()
        }) else {
            return false;
        };
        output.copy_from_slice(&bytes[address - start..address - start + output.len()]);
        true
    };
    let metadata = decoder::pointer(&read, source_context).unwrap();
    let (identity, source) = decoder::skeleton(&read, metadata).unwrap();
    let source = animation_reference(source);
    #[derive(serde::Deserialize)]
    struct Node {
        name: String,
        parent: i16,
        local_row_major: [f64; 16],
    }
    #[derive(serde::Deserialize)]
    struct Mesh {
        nodes: Vec<Node>,
    }
    let text = std::fs::read_to_string(directory.join("mesh/nodes.toml")).unwrap();
    let nodes: Mesh = toml::from_str(&text).unwrap();
    let mesh: Vec<_> = nodes
        .nodes
        .into_iter()
        .map(|node| Bone {
            name: node.name,
            parent: (node.parent >= 0).then_some(node.parent as usize),
            reference: decoder::local(DMat4::from_cols_array(&node.local_row_major)).unwrap(),
        })
        .collect();
    let target = equipment_mesh_reference(&source, &mesh).unwrap();
    let mut scratch = decoder::PoseScratch::default();
    scratch
        .capture(&read, source_context, &identity, &source, 1.0)
        .unwrap();
    let mut plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    plan.calibrate_limb_directions();
    let mut frame = plan.new_frame();
    plan.prepare(&scratch.local, 1, &mut frame).unwrap();
    let mut maximum_angle = 0.0f64;
    for side in ["L", "R"] {
        for [a, b] in [["Thigh", "Calf"], ["Calf", "Foot"]] {
            let index = |bones: &[Bone], part: &str| {
                bones
                    .iter()
                    .position(|x| x.name == format!("{side}_{part}"))
                    .unwrap()
            };
            let sp = &scratch.model;
            let tp = frame.outputs().unwrap().model;
            let from = (sp[index(&source, b)].w_axis - sp[index(&source, a)].w_axis)
                .truncate()
                .normalize();
            let to = (tp[index(&target, b)].w_axis - tp[index(&target, a)].w_axis)
                .truncate()
                .normalize();
            let angle = from.dot(to).clamp(-1.0, 1.0).acos().to_degrees();
            println!("{side}_{a} animation segment deviation={angle:.6}deg");
            maximum_angle = maximum_angle.max(angle);
        }
    }
    let before = frame.outputs().unwrap().model.to_vec();
    #[derive(serde::Deserialize)]
    struct Ground {
        normal: [f64; 4],
        orientation: [f64; 4],
    }
    let ground: Ground =
        toml::from_str(&std::fs::read_to_string(directory.join("ground.toml")).unwrap()).unwrap();
    let normal = DQuat::from_array(ground.orientation).normalize().inverse()
        * DVec3::new(ground.normal[0], ground.normal[1], ground.normal[2]).normalize();
    plan.constrain(&mut frame, 0, Some(normal)).unwrap();
    for side in ["L", "R"] {
        let i = target
            .iter()
            .position(|b| b.name == format!("{side}_Foot"))
            .unwrap();
        println!(
            "{side}_Foot before={:?} after={:?}",
            before[i].w_axis,
            frame.outputs().unwrap().model[i].w_axis
        );
        let s = source
            .iter()
            .position(|b| b.name == target[i].name)
            .unwrap();
        let (sw, _) = reference_world(&source, &plan.source_order).unwrap();
        let (tw, _) = reference_world(&target, &plan.target_order).unwrap();
        let source_contact = scratch.model[s].w_axis.truncate() - normal * sw[s].w_axis.y;
        let target_contact =
            frame.outputs().unwrap().model[i].w_axis.truncate() - normal * tw[i].w_axis.y;
        assert!(
            (target_contact - source_contact).dot(normal).abs() < 1e-4,
            "measured surface/lift mismatch"
        );
        assert!(
            frame.outputs().unwrap().model[i].w_axis.y < 0.25,
            "seated foot incorrectly lifted"
        );
    }
    assert!(
        maximum_angle < 0.01,
        "limb swing differs from animated source: {maximum_angle}deg"
    );
}

#[test]
#[ignore = "requires private ERCS_RETARGET_LOWER_BODY fixture directory"]
fn captured_retained_weapon_branch_matches_anatomical_grip() {
    use crate::equipment_retarget_pose as decoder;
    let directory = std::path::PathBuf::from(std::env::var("ERCS_RETARGET_LOWER_BODY").unwrap());
    let raw = std::fs::read(directory.join("pose.bin")).unwrap();
    let word = |bytes: &[u8]| usize::from_le_bytes(bytes.try_into().unwrap());
    let source_context = word(&raw[..8]);
    let mut blocks = Vec::new();
    let mut offset = 24;
    while offset < raw.len() {
        let address = word(&raw[offset..offset + 8]);
        let length = word(&raw[offset + 8..offset + 16]);
        blocks.push((address, raw[offset + 16..offset + 16 + length].to_vec()));
        offset += 16 + length;
    }
    let read = |address: usize, output: &mut [u8]| {
        let Some((start, bytes)) = blocks.iter().find(|(start, bytes)| {
            address >= *start && address - start + output.len() <= bytes.len()
        }) else {
            return false;
        };
        output.copy_from_slice(&bytes[address - start..address - start + output.len()]);
        true
    };
    let metadata = decoder::pointer(&read, source_context).unwrap();
    let (identity, source) = decoder::skeleton(&read, metadata).unwrap();
    let source = animation_reference(source);
    #[derive(serde::Deserialize)]
    struct Node {
        name: String,
        parent: i16,
        local_row_major: [f64; 16],
    }
    #[derive(serde::Deserialize)]
    struct Mesh {
        nodes: Vec<Node>,
    }
    let text = std::fs::read_to_string(directory.join("mesh/nodes.toml")).unwrap();
    let nodes: Mesh = toml::from_str(&text).unwrap();
    let mesh: Vec<_> = nodes
        .nodes
        .into_iter()
        .map(|node| Bone {
            name: node.name,
            parent: (node.parent >= 0).then_some(node.parent as usize),
            reference: decoder::local(DMat4::from_cols_array(&node.local_row_major)).unwrap(),
        })
        .collect();
    let target = equipment_mesh_reference(&source, &mesh).unwrap();
    let mut scratch = decoder::PoseScratch::default();
    scratch
        .capture(&read, source_context, &identity, &source, 1.0)
        .unwrap();
    let mut plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    plan.calibrate_limb_directions();
    let mut frame = plan.new_frame();
    plan.prepare(&scratch.local, 1, &mut frame).unwrap();
    for side in ["L", "R"] {
        let index = |bones: &[Bone], part: &str| {
            bones
                .iter()
                .position(|x| x.name == format!("{side}_{part}"))
                .unwrap()
        };
        let hand = index(&target, "Hand");
        let sh = index(&source, "Hand");
        let weapon = index(&target, "Weapon");
        let sw = index(&source, "Weapon");
        let expected = plan.attachment_pose(hand, &frame).unwrap()
            * frame.source_world[sh].inverse()
            * frame.source_world[sw];
        let corrected = plan.attachment_pose(weapon, &frame).unwrap();
        matrix_near(corrected, expected);
        let legacy = plan.bone_attachment_pose(weapon, &frame).unwrap();
        println!(
            "{side}_Weapon old anatomical offset={} corrected_offset={}",
            legacy
                .w_axis
                .truncate()
                .distance(expected.w_axis.truncate()),
            corrected
                .w_axis
                .truncate()
                .distance(expected.w_axis.truncate())
        );
    }
}

#[test]
#[ignore = "requires private ERCS_RETARGET_LOWER_BODY fixture directory"]
fn captured_motion_scale_matches_live_limb_proportions() {
    use crate::equipment_retarget_pose as decoder;
    let directory = std::path::PathBuf::from(std::env::var("ERCS_RETARGET_LOWER_BODY").unwrap());
    let raw = std::fs::read(directory.join("pose.bin")).unwrap();
    let word = |bytes: &[u8]| usize::from_le_bytes(bytes.try_into().unwrap());
    let source_context = word(&raw[..8]);
    let mut blocks = Vec::new();
    let mut offset = 24;
    while offset < raw.len() {
        let address = word(&raw[offset..offset + 8]);
        let length = word(&raw[offset + 8..offset + 16]);
        blocks.push((address, raw[offset + 16..offset + 16 + length].to_vec()));
        offset += 16 + length;
    }
    let read = |address: usize, output: &mut [u8]| {
        let Some((start, bytes)) = blocks.iter().find(|(start, bytes)| {
            address >= *start && address - start + output.len() <= bytes.len()
        }) else {
            return false;
        };
        output.copy_from_slice(&bytes[address - start..address - start + output.len()]);
        true
    };
    let (identity, source) =
        decoder::skeleton(&read, decoder::pointer(&read, source_context).unwrap()).unwrap();
    let source = animation_reference(source);
    #[derive(serde::Deserialize)]
    struct Node {
        name: String,
        parent: i16,
        local_row_major: [f64; 16],
    }
    #[derive(serde::Deserialize)]
    struct Mesh {
        nodes: Vec<Node>,
    }
    let nodes: Mesh =
        toml::from_str(&std::fs::read_to_string(directory.join("mesh/nodes.toml")).unwrap())
            .unwrap();
    let mesh: Vec<_> = nodes
        .nodes
        .into_iter()
        .map(|n| Bone {
            name: n.name,
            parent: (n.parent >= 0).then_some(n.parent as usize),
            reference: decoder::local(DMat4::from_cols_array(&n.local_row_major)).unwrap(),
        })
        .collect();
    let target = equipment_mesh_reference(&source, &mesh).unwrap();
    let mut scratch = decoder::PoseScratch::default();
    scratch
        .capture(&read, source_context, &identity, &source, 1.0)
        .unwrap();
    let mut plan = Plan::new(source.clone(), target.clone(), &[]).unwrap();
    plan.calibrate_limb_directions();
    let mut frame = plan.new_frame();
    plan.prepare(&scratch.local, 1, &mut frame).unwrap();
    let length = |bones: &[Bone], world: &[DMat4]| {
        ["L", "R"]
            .into_iter()
            .map(|side| {
                let ids = ["Thigh", "Calf", "Foot"].map(|part| {
                    bones
                        .iter()
                        .position(|b| b.name == format!("{side}_{part}"))
                        .unwrap()
                });
                world[ids[0]]
                    .w_axis
                    .truncate()
                    .distance(world[ids[1]].w_axis.truncate())
                    + world[ids[1]]
                        .w_axis
                        .truncate()
                        .distance(world[ids[2]].w_axis.truncate())
            })
            .sum::<f64>()
    };
    let live_source = length(&source, &scratch.model);
    let live_target = length(&target, frame.outputs().unwrap().model);
    let profile = MotionProfile::new(&source, &target).unwrap();
    let expected = live_target / live_source;
    println!(
        "motion ratio={} live ratio={} source legs={} target legs={}",
        profile.leg_ratio, expected, live_source, live_target
    );
    assert!(
        (profile.leg_ratio / expected - 1.0).abs() < 1e-4,
        "motion multiplier exceeds actual limb ratio"
    );
}
