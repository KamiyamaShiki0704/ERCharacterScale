//! Private BD1220 resource replay, relocated into owned test allocations.
use super::*;
use crate::body_scale_port::test_characters::{BASE, Character, Environment};

#[test]
#[ignore = "requires private ERCS_BD1220_RESOURCES snapshot"]
fn captured_bd1220_resources_are_not_rebuilt_every_frame() {
    let _environment = Environment::new();
    let text = std::fs::read_to_string(std::env::var("ERCS_BD1220_RESOURCES").unwrap()).unwrap();
    let rows: Vec<Vec<_>> = text
        .lines()
        .filter(|s| !s.starts_with('#'))
        .map(|s| s.split_whitespace().collect())
        .collect();
    let number = |s: &str| usize::from_str_radix(s, 16).unwrap();
    let mut heaps = Vec::new();
    for row in rows.iter().filter(|r| r[0] == "R") {
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
            .find(|(start, n, _)| old >= *start && old - *start < *n)
            .expect("captured pointer coverage");
        heap.as_ptr() as usize + old - start
    };
    for row in rows.iter().filter(|r| matches!(r[0], "P" | "V")) {
        let value = if row[0] == "V" {
            BASE + number(row[2])
        } else {
            relocated(number(row[2]))
        };
        unsafe {
            (relocated(number(row[1])) as *mut usize).write_unaligned(value);
        }
    }
    let sims: Vec<_> = rows
        .iter()
        .filter(|r| r[0] == "S")
        .map(|r| relocated(number(r[1])))
        .collect();
    let character = Character::new(1220, 1, 1.0);
    character.attach_cloth(sims[0]);
    for (i, &sim) in sims.iter().enumerate() {
        let child = character.at(0xDD00 + i * 0x40);
        character.put(0xDC00 + i * 8, child);
        character.put(0xDD18 + i * 0x40, sim);
    }
    character.put(0xDB48, sims.len() as i32);
    let consumer = Consumer {
        identity: character.identity(),
        scale: 1.0,
    };
    for &sim in &sims {
        let started = std::time::Instant::now();
        let result = crate::memory_query::scoped(|| capture_simulation_baseline(sim));
        println!(
            "BD1220 sim={sim:x} baseline={} unsupported={:?} fields={:?} capture_us={}",
            result.is_some(),
            result.as_ref().map(|b| b.unsupported_layouts),
            result.as_ref().map(|b| b.fields.len()),
            started.elapsed().as_micros()
        );
        assert!(
            result.is_some(),
            "BD1220 simulation must produce a cacheable baseline"
        );
    }
    let mut pool = Pool::default();
    let initial = pool.prepare(&[consumer]);
    println!(
        "BD1220 initial resources={} rejected={:?}",
        pool.resources.len(),
        initial.rejected
    );
    let cached: Vec<_> = sims
        .iter()
        .map(|sim| pool.resources.get(sim).unwrap().baseline.clone())
        .collect();
    let expected_rejection = initial.rejected.clone();
    let start = std::time::Instant::now();
    for _ in 0..120 {
        let prepared = pool.prepare(&[consumer]);
        assert_eq!(prepared.rejected, expected_rejection);
        for (sim, original) in sims.iter().zip(&cached) {
            assert!(
                Arc::ptr_eq(original, &pool.resources[sim].baseline),
                "resource recaptured"
            );
        }
        std::hint::black_box(prepared);
    }
    println!("BD1220 120 passes us={}", start.elapsed().as_micros());
    assert!(
        initial.rejected.is_empty(),
        "native armor baseline rejected: {:?}",
        initial.rejected
    );
}
