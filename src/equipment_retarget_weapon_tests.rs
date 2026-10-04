use super::*;
use crate::equipment_retarget::{Bone, LocalPose, Plan};
use glam::DVec3;

fn from_memory(m: &std::collections::BTreeMap<usize, u8>, at: usize, out: &mut [u8]) -> bool {
    for (i, b) in out.iter_mut().enumerate() {
        let Some(v) = m.get(&(at + i)) else {
            return false;
        };
        *b = *v;
    }
    true
}

#[test]
fn ordinary_two_hand_weapons_keep_their_grip_and_cached_flags_remain_live() {
    let mut cache = GripCache::default();
    for style in [2, 3] {
        let mut m = memory(false, style);
        let cold = std::cell::Cell::new(0);
        assert_eq!(
            cache.style(
                &|a, b| {
                    cold.set(cold.get() + 1);
                    from_memory(&m, a, b)
                },
                0x1000,
                0
            ),
            style
        );
        let warm = std::cell::Cell::new(0);
        assert_eq!(
            cache.style(
                &|a, b| {
                    warm.set(warm.get() + 1);
                    from_memory(&m, a, b)
                },
                0x1000,
                0
            ),
            style
        );
        assert!(
            warm.get() <= 17,
            "per-frame lookup unexpectedly grew: {}",
            warm.get()
        );
        m.insert(0x627c, 2);
        assert_eq!(cache.style(&|a, b| from_memory(&m, a, b), 0x1000, 0), 0);
        m.insert(0x627c, 0);
        assert_eq!(cache.style(&|a, b| from_memory(&m, a, b), 0x1000, 0), style);
        // Invalid selected slot, missing row or unload must not borrow a
        // previous weapon's cached result and move an unrelated hand.
        m.insert(0x200c + (style as usize - 2) * 4, 3);
        assert_eq!(cache.style(&|a, b| from_memory(&m, a, b), 0x1000, 0), 0);
        assert_eq!(cache.style(&|_, _| false, 0x1000, 0), 0);
    }
}

#[test]
fn malformed_or_replaced_weapon_table_cannot_reuse_a_shared_grip() {
    let mut cache = GripCache::default();
    let mut m = memory(false, 3);
    assert_eq!(cache.style(&|a, b| from_memory(&m, a, b), 0x1000, 0), 3);
    // Same outer resource, different file pointer: invalidate the old row.
    for (i, v) in 0x9000usize.to_le_bytes().iter().enumerate() {
        m.insert(0x5080 + i, *v);
    }
    assert_eq!(cache.style(&|a, b| from_memory(&m, a, b), 0x1000, 0), 0);
    for (at, value) in [
        (0x602d, 0),
        (0x602c, 1),
        (0x6904, 255),
        (0x6040, 0),
        (0x6048, 0xff),
    ] {
        let mut m = memory(false, 3);
        m.insert(at, value);
        if at == 0x6048 {
            m.insert(at + 1, 0xff);
        }
        assert_eq!(
            GripCache::default().style(&|a, b| from_memory(&m, a, b), 0x1000, 0),
            0,
            "malformed field {at:x}"
        );
    }
}

// Sparse owned memory emulates the native repository, loaded weapon table,
// selected ChrAsm slots and a reinforced paired weapon. No game assets.
fn memory(dual: bool, style: u32) -> std::collections::BTreeMap<usize, u8> {
    let mut m = std::collections::BTreeMap::new();
    let mut put = |a: usize, bytes: &[u8]| {
        for (i, &b) in bytes.iter().enumerate() {
            m.insert(a + i, b);
        }
    };
    for (a, p) in [
        (0x1638, 0x2000usize),
        (0x3D85F58, 0x3000),
        (0x3000, 0x2BB84C8),
        (0x3088, 0x4000),
        (0x4080, 0x5000),
        (0x5080, 0x6000),
    ] {
        put(a, &p.to_le_bytes());
    }
    put(0x5080 - 8, &0x1000usize.to_le_bytes());
    put(0x3080, &1u32.to_le_bytes());
    put(0x2008, &style.to_le_bytes());
    put(0x200c, &0u32.to_le_bytes());
    put(0x2010, &1u32.to_le_bytes());
    put(0x207c, &3230010i32.to_le_bytes());
    put(0x2088, &3230010i32.to_le_bytes());
    put(0x5ff0, &[0; 16]);
    put(0x5ff0, &0x900u32.to_le_bytes());
    put(0x5ff4, &1u32.to_le_bytes());
    put(0x6000, &[0; 64]);
    put(0x600a, &1u16.to_le_bytes());
    put(0x6010, &0x800u64.to_le_bytes());
    put(0x602d, &[0x85]);
    put(0x6800, b"EQUIP_PARAM_WEAPON_ST\0");
    put(0x6040, &[0; 24]);
    put(0x6040, &3230000u32.to_le_bytes());
    put(0x6048, &0x100u64.to_le_bytes());
    put(0x627c, &[if dual { 2 } else { 0 }]);
    put(0x6900, &3230000u32.to_le_bytes());
    put(0x6904, &0u32.to_le_bytes());
    m
}

#[test]
fn paired_weapon_in_both_hand_styles_must_not_raise_either_arm() {
    independent_arm_regression(true, false);
}

#[test]
fn empty_hand_in_both_hand_styles_must_not_raise_either_arm() {
    independent_arm_regression(false, true);
}

fn independent_arm_regression(paired: bool, empty: bool) {
    let mut source = Vec::new();
    for (side, x) in [("L", 0.3), ("R", -0.3)] {
        let start = source.len();
        for (index, part, translation) in [
            (0, "UpperArm", DVec3::new(x, 1.4, 0.0)),
            (1, "Forearm", DVec3::new(0.0, -0.3, 0.0)),
            (2, "Hand", DVec3::new(0.0, -0.25, 0.0)),
        ] {
            source.push(Bone {
                name: format!("{side}_{part}"),
                parent: (index > 0).then(|| start + index - 1),
                reference: LocalPose {
                    translation,
                    ..LocalPose::IDENTITY
                },
            });
        }
    }
    let mut target = source.clone();
    for b in &mut target {
        b.reference.translation *= 0.75;
    }
    let plan = Plan::new(source.clone(), target, &[]).unwrap();
    let mut frame = plan.new_frame();
    let mut cache = GripCache::default();
    for style in [2, 3] {
        let mut m = memory(paired, style);
        if empty {
            // Captured left two-hand empty slot resolves to weapon 110000,
            // whose isDualBlade flag is unset. Both sides must stay independent.
            for at in [0x207c, 0x2088, 0x6040, 0x6900] {
                for (i, value) in 110000u32.to_le_bytes().into_iter().enumerate() {
                    m.insert(at + i, value);
                }
            }
        }
        let read = |at: usize, out: &mut [u8]| {
            for (i, b) in out.iter_mut().enumerate() {
                let Some(v) = m.get(&(at + i)) else {
                    return false;
                };
                *b = *v;
            }
            true
        };
        let input: Vec<_> = source.iter().map(|b| b.reference).collect();
        plan.prepare(&input, 0, &mut frame).unwrap();
        let expected = frame.outputs().unwrap().model.to_vec();
        plan.constrain(&mut frame, cache.style(&read, 0x1000, 0), false)
            .unwrap();
        for (i, (a, b)) in frame
            .outputs()
            .unwrap()
            .model
            .iter()
            .zip(expected)
            .enumerate()
        {
            assert!(
                a.to_cols_array()
                    .iter()
                    .zip(b.to_cols_array())
                    .all(|(x, y)| (x - y).abs() < 1e-8),
                "independent style {style} (empty={empty}) unexpectedly moved arm joint {i}"
            );
        }
    }
}
