//! Optional private resource snapshot, relocated into owned memory only.
use super::*;
use crate::body_scale_port::test_characters::{BASE, Character, Environment};

#[test]
#[ignore = "requires private replay data; set ER_CHARACTER_SCALE_FIXTURES"]
fn c3185_live_resources_pass_preflight_and_restore() {
    let _environment = Environment::new();
    let text = crate::test_fixtures::text("c3185-cloth-resources.txt");
    let records: Vec<Vec<&str>> = text
        .lines()
        .filter(|s| !s.starts_with('#'))
        .map(|s| s.split_whitespace().collect())
        .collect();
    let number = |s: &str| usize::from_str_radix(s, 16).unwrap();
    let mut heaps = Vec::new();
    for row in records.iter().filter(|r| r[0] == "R") {
        let bytes: Vec<u8> = row[2]
            .as_bytes()
            .chunks_exact(2)
            .map(|s| u8::from_str_radix(std::str::from_utf8(s).unwrap(), 16).unwrap())
            .collect();
        let mut heap = vec![0u128; bytes.len().div_ceil(16)];
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), heap.as_mut_ptr().cast(), bytes.len());
        }
        heaps.push((number(row[1]), bytes.len(), heap));
    }
    let relocated = |old: usize| {
        if old == 0 {
            return 0;
        }
        let (start, _, heap) = heaps
            .iter()
            .find(|(start, len, _)| old >= *start && old - *start < *len)
            .expect("all consumed pointers must refer to a captured allocation");
        heap.as_ptr() as usize + old - start
    };
    for row in records.iter().filter(|r| r[0] == "P" || r[0] == "V") {
        let value = if row[0] == "V" {
            BASE + number(row[2])
        } else {
            relocated(number(row[2]))
        };
        unsafe {
            (relocated(number(row[1])) as *mut usize).write_unaligned(value);
        }
    }
    let sims: Vec<usize> = records
        .iter()
        .filter(|r| r[0] == "S")
        .map(|r| relocated(number(r[1])))
        .collect();
    assert_eq!(sims.len(), 2);
    let character = Character::new(3185, 1, 1.0);
    character.attach_cloth(sims[0]);
    character.put(0xDB48, 2i32);
    character.put(0xDC08, character.at(0xDD20));
    character.put(0xDD38, sims[1]);
    let mut pool = Pool::default();
    let prepared = pool.prepare(&[Consumer {
        identity: character.identity(),
        scale: 0.15,
    }]);
    assert!(
        prepared.rejected.is_empty(),
        "c3185 rejection: {:?}",
        prepared.rejected
    );
    let mut baselines: Vec<_> = prepared.objects.values().map(|b| (**b).clone()).collect();
    assert!(
        baselines.len() > 200,
        "both full cloth graphs must be replayed"
    );
    for scale in [1.0, 0.15, 0.15, 0.85, 2.0, 1.0] {
        for b in &mut baselines {
            b.apply(scale).expect("resource update");
        }
        for b in &baselines {
            for f in &b.fields {
                let expected =
                    scale_dimension_from_baseline(f.baseline, scale, f.power, f.preserve_unbounded)
                        .unwrap();
                assert_eq!(read_f32(f.address).unwrap().to_bits(), expected.to_bits());
            }
            if std::env::var_os("ERCS_CONVEX_NATIVE_REPLAY").is_some()
                && body_scale_port::module_rva(read_usize(b.address).unwrap()) == 0x2D7D580
            {
                let hex = |address: usize, length: usize| -> String {
                    unsafe { std::slice::from_raw_parts(address as *const u8, length) }
                        .iter()
                        .map(|v| format!("{v:02x}"))
                        .collect()
                };
                let mut parts = vec![hex(b.address, 0x120)];
                for (off, stride) in [(0x20, 2), (0x30, 1), (0x40, 64)] {
                    let p = read_usize(b.address + off).unwrap();
                    let n = read_i32(b.address + off + 8).unwrap() as usize;
                    parts.push(if n == 0 {
                        "-".into()
                    } else {
                        hex(p, n * stride)
                    });
                }
                println!("CONVEX-REPLAY {} {} {}", b.address, scale, parts.join(" "));
            }
        }
    }
    println!(
        "c3185 replay: {} resource baselines, {} fields, shrink/change/restore passed",
        baselines.len(),
        baselines.iter().map(|b| b.fields.len()).sum::<usize>()
    );
}
