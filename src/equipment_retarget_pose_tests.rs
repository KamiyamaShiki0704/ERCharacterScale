use super::*;

#[test]
fn solver_cache_preserves_exact_rows_and_defers_shear_descendants() {
    let exact = DMat4::from_translation(DVec3::new(1.0, 2.0, 3.0));
    let (row, flags) = solver_model_cache(exact, false, 0.5).unwrap();
    assert_eq!(flags, 0);
    assert_eq!(&row[..3], &[0.5, 1.0, 1.5]);
    assert_eq!(solver_model_cache(exact, true, 1.0).unwrap().1, 2);
    let shear = DMat4::from_scale(DVec3::new(0.8, 1.0, 1.0)) * DMat4::from_rotation_z(0.4);
    assert!(local(shear).is_none());
    assert_eq!(solver_model_cache(shear, false, 1.0).unwrap().1, 2);
    for bad in [
        DMat4::ZERO,
        DMat4::from_cols_array(&[f64::NAN; 16]),
        DMat4::from_translation(DVec3::splat(f64::MAX)),
    ] {
        assert!(solver_model_cache(bad, false, 1.0).is_none());
    }
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(solver_model_cache(exact, true, scale).is_none());
    }
}

fn put(memory: &mut [u8], address: usize, bytes: &[u8]) {
    memory[address..address + bytes.len()].copy_from_slice(bytes);
}

fn fixture() -> Vec<u8> {
    let mut m = vec![0u8; 0x1000];
    // Two joints, exact native headers; all target allocations are owned.
    for (at, value) in [
        (0x120, 0x200usize),
        (0x130, 0x300),
        (0x140, 0x400),
        (0x300, 0x500),
        (0x310, 0x510),
        (0x600, 0x100),
        (0x608, 0x700),
        (0x618, 0x800),
        (0x628, 0x900),
    ] {
        put(&mut m, at, &value.to_le_bytes());
    }
    for at in [0x128, 0x138, 0x148, 0x610, 0x620, 0x630] {
        put(&mut m, at, &2u32.to_le_bytes());
    }
    put(&mut m, 0x200, &(-1i16).to_le_bytes());
    put(&mut m, 0x202, &0i16.to_le_bytes());
    put(&mut m, 0x500, b"root\0");
    put(&mut m, 0x510, b"child\0");
    for (index, translation) in [DVec3::ZERO, DVec3::X].into_iter().enumerate() {
        let pose = qs_output(LocalPose {
            translation,
            ..LocalPose::IDENTITY
        })
        .unwrap();
        let bytes: Vec<_> = pose.iter().flat_map(|v| v.to_le_bytes()).collect();
        for at in [0x400, 0x700, 0x800] {
            put(&mut m, at + 48 * index, &bytes);
        }
    }
    m
}

fn reader(memory: &[u8]) -> impl Fn(usize, &mut [u8]) -> bool + '_ {
    |at, out| {
        let Some(data) = at
            .checked_add(out.len())
            .and_then(|end| memory.get(at..end))
        else {
            return false;
        };
        out.copy_from_slice(data);
        true
    }
}

#[test]
fn native_skeleton_metadata_and_owned_lazy_ancestor_resolution() {
    let mut memory = fixture();
    let (identity, bones) = skeleton(&reader(&memory), 0x100).unwrap();
    assert_eq!(bones[1].name, "child");
    // Invalid model cache must not be read when MODEL_DIRTY is set.
    memory[0x800..0x860].fill(0xff);
    for at in [0x900, 0x904] {
        put(&mut memory, at, &2u32.to_le_bytes());
    }
    put(&mut memory, 0x700, &2f32.to_le_bytes());
    let original = memory.clone();
    let mut scratch = PoseScratch::default();
    scratch
        .capture(&reader(&memory), 0x600, &identity, &bones, 1.0)
        .unwrap();
    assert!((scratch.model[1].w_axis.x - 3.0).abs() < 1e-8);
    assert!((scratch.local[1].translation.x - 1.0).abs() < 1e-8);
    assert_eq!(memory, original);
    // LOCAL_DIRTY means derive it from the valid model cache instead.
    let mut memory = fixture();
    memory[0x700..0x760].fill(0xff);
    for at in [0x900, 0x904] {
        put(&mut memory, at, &1u32.to_le_bytes());
    }
    scratch
        .capture(&reader(&memory), 0x600, &identity, &bones, 1.0)
        .unwrap();
    assert!((scratch.local[1].translation.x - 1.0).abs() < 1e-8);
}

#[test]
fn native_bad_sizes_changed_identity_and_cycles_are_rejected() {
    let mut m = fixture();
    let (identity, bones) = skeleton(&reader(&m), 0x100).unwrap();
    put(&mut m, 0x610, &1u32.to_le_bytes());
    assert!(
        PoseScratch::default()
            .capture(&reader(&m), 0x600, &identity, &bones, 1.0)
            .is_none()
    );
    let mut m = fixture();
    put(&mut m, 0x120, &0x210usize.to_le_bytes());
    assert!(!identity.current(&reader(&m)));
    let mut m = fixture();
    put(&mut m, 0x202, &1i16.to_le_bytes());
    assert!(skeleton(&reader(&m), 0x100).is_none());
    let mut m = fixture();
    put(&mut m, 0x904, &3u32.to_le_bytes());
    assert!(
        PoseScratch::default()
            .capture(&reader(&m), 0x600, &identity, &bones, 1.0)
            .is_none()
    );
}

#[test]
fn overall_scale_is_removed_once_before_retarget() {
    let mut m = fixture();
    let (identity, bones) = skeleton(&reader(&m), 0x100).unwrap();
    for i in 0..2 {
        let p = LocalPose {
            translation: DVec3::X * i as f64 * 1.12,
            scale: DVec3::splat(1.12),
            ..LocalPose::IDENTITY
        };
        let raw: Vec<_> = qs_output(p)
            .unwrap()
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        put(&mut m, 0x800 + i * 48, &raw);
    }
    let mut scratch = PoseScratch::default();
    scratch
        .capture(&reader(&m), 0x600, &identity, &bones, 1.12f32 as f64)
        .unwrap();
    assert!((scratch.local[1].translation.x - 1.0).abs() < 1e-7);
    assert!((scratch.local[0].scale - DVec3::ONE).length() < 1e-7);
}

#[test]
fn matrix_format_round_trip_preserves_axes_translation_and_rejects_shear() {
    let pose = LocalPose {
        translation: DVec3::new(1.0, 2.0, 3.0),
        rotation: DQuat::from_rotation_y(0.7),
        ..LocalPose::IDENTITY
    };
    let m = matrix(pose);
    let raw: Vec<_> = affine_output(m)
        .unwrap()
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let back = affine(&raw).unwrap();
    assert!((back.transform_point3(DVec3::X) - m.transform_point3(DVec3::X)).length() < 1e-6);
    let mut shear = DMat4::IDENTITY;
    shear.y_axis.x = 0.2;
    assert!(local(shear).is_none());
    let mut enormous = DMat4::IDENTITY;
    enormous.w_axis.x = f64::MAX;
    assert!(affine_output(enormous).is_none());
}

#[test]
fn animation_source_scale_conventions_preserve_root_motion_and_authored_bone_lengths() {
    let mut scratch = PoseScratch::default();
    for requested in [0.1f32, 0.3, 0.55, 0.85, 1.0, 1.12, 2.0, 10.0] {
        for supplied in [1.0f64, f64::from(requested)] {
            let mut m = fixture();
            let (identity, bones) = skeleton(&reader(&m), 0x100).unwrap();
            for i in 0..2 {
                let pose = LocalPose {
                    translation: DVec3::new(i as f64, 0.2, -0.1) * supplied,
                    scale: DVec3::splat(supplied),
                    ..LocalPose::IDENTITY
                };
                let raw: Vec<_> = qs_output(pose)
                    .unwrap()
                    .iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect();
                put(&mut m, 0x800 + i * 48, &raw);
            }
            let before = m.clone();
            scratch
                .capture(&reader(&m), 0x600, &identity, &bones, f64::from(requested))
                .unwrap();
            assert!(
                scratch.local[0]
                    .translation
                    .distance(DVec3::new(0., 0.2, -0.1))
                    < 1e-6
            );
            assert!(scratch.local[0].scale.distance(DVec3::ONE) < 1e-6);
            assert!(scratch.local[1].translation.distance(DVec3::X) < 1e-6);
            assert_eq!(m, before);
        }
    }
    let mut m = fixture();
    let (identity, bones) = skeleton(&reader(&m), 0x100).unwrap();
    put(&mut m, 0x800 + 32, &2f32.to_le_bytes());
    assert!(
        scratch
            .capture(&reader(&m), 0x600, &identity, &bones, 0.3)
            .is_none(),
        "mixed root scale must not be guessed"
    );
    for v in [0., -1., f64::NAN, f64::INFINITY] {
        assert!(
            scratch
                .capture(&reader(&m), 0x600, &identity, &bones, v)
                .is_none()
        );
    }
}
