use super::*;
fn bone(name: &str, parent: Option<usize>, x: f64) -> Bone {
    Bone {
        name: name.into(),
        parent,
        reference: LocalPose {
            translation: DVec3::new(x, 0., 0.),
            rotation: DQuat::IDENTITY,
            scale: DVec3::ONE,
        },
    }
}
#[test]
fn independent_motion_aliases_preserve_grip_and_hold_selection() {
    let source = vec![
        bone("L_Hand", None, 0.),
        bone("L_Weapon01", Some(0), 1.),
        bone("L_Weapon02", Some(0), 1.),
    ];
    let target = vec![
        bone("L_Hand", None, 0.),
        bone("L_Shield", Some(0), 0.2),
        bone("L_Weapon03", Some(0), 0.4),
    ];
    let mut plan = Weapons::new("L", &source, &target, &[0, 1, 2], &[0, 1, 2]).unwrap();
    let mut source = reference(&source).unwrap();
    let target = reference(&target).unwrap();
    let mut out = vec![Qs([0.; 12]); 3];
    plan.apply(&source, &target, &mut out).unwrap();
    for m in &mut source {
        *m = DMat4::from_rotation_z(0.8) * *m;
    }
    plan.apply(&source, &target, &mut out).unwrap();
    assert_eq!(
        plan.selected, None,
        "inherited hand motion cannot select a weapon"
    );
    source[2] *= DMat4::from_translation(DVec3::X * 0.3) * DMat4::from_rotation_y(0.5);
    plan.apply(&source, &target, &mut out).unwrap();
    assert_eq!(plan.selected, Some(1));
    assert!((out[1].0[0] - 0.5).abs() < 1e-6);
    assert!((out[2].0[0] - 0.7).abs() < 1e-6);
    let expected = out[1].0;
    for _ in 0..100 {
        plan.apply(&source, &target, &mut out).unwrap();
        assert_eq!(plan.selected, Some(1));
        assert_eq!(out[1].0, expected);
    }
    for m in &mut source[1..] {
        *m *= DMat4::from_rotation_z(0.2);
    }
    plan.apply(&source, &target, &mut out).unwrap();
    assert_eq!(
        plan.selected,
        Some(1),
        "ambiguous simultaneous motion cannot switch selection"
    );
}
#[test]
fn single_sword_maps_without_warmup_and_filters_wrong_hand() {
    let source = vec![
        bone("R_Hand", None, 0.),
        bone("R_Sword", Some(0), 1.),
        bone("Dummy199", Some(0), 2.),
    ];
    let target = vec![bone("R_Hand", None, 0.), bone("R_Weapon01", Some(0), 0.2)];
    let mut plan = Weapons::new("R", &source, &target, &[0, 1], &[0, 1, 2]).unwrap();
    assert!(Weapons::new("L", &source, &target, &[0, 1], &[0, 1, 2]).is_none());
    let mut source = reference(&source).unwrap();
    source[1] *= DMat4::from_translation(DVec3::Y * 0.5);
    let mut out = vec![Qs([0.; 12]); 2];
    plan.apply(&source, &reference(&target).unwrap(), &mut out)
        .unwrap();
    assert_eq!(plan.selected, Some(0));
    assert!((out[1].0[1] - 0.5).abs() < 1e-6);
    assert_eq!(
        target_indices(&[bone("R_Hand", None, 0.), bone("Dummy199", Some(0), 0.)]),
        Vec::<usize>::new()
    );
}

#[test]
fn static_authored_weapon_pose_selects_only_unique_reference_delta() {
    let source = vec![
        bone("R_Hand", None, 0.),
        bone("R_Weapon01", Some(0), 1.),
        bone("R_Weapon02", Some(0), 1.),
    ];
    let target = vec![bone("R_Hand", None, 0.), bone("R_Sword", Some(0), 0.2)];
    let make = || Weapons::new("R", &source, &target, &[0, 1], &[0, 1, 2]).unwrap();
    let mut plan = make();
    let mut world = reference(&source).unwrap();
    let tw = reference(&target).unwrap();
    let mut out = vec![Qs([0.; 12]); 2];
    world[2] *= DMat4::from_rotation_z(0.7);
    plan.apply(&world, &tw, &mut out).unwrap();
    assert_eq!(plan.selected, Some(1));
    let q = DQuat::from_xyzw(
        out[1].0[4] as f64,
        out[1].0[5] as f64,
        out[1].0[6] as f64,
        out[1].0[7] as f64,
    )
    .normalize();
    assert!(q.dot(DQuat::from_rotation_z(0.7)).abs() > 1. - 1e-8);
    let mut ambiguous = make();
    world[1] *= DMat4::from_rotation_y(0.8);
    ambiguous.apply(&world, &tw, &mut out).unwrap();
    assert_eq!(ambiguous.selected, None);
}
