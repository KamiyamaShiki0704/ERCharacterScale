//! Copied render-output reconciliation, never canonical pose/solver mutation.
//! ER 26B4280 native writeback masks -> B455A0 mapped affine48 range.
//! No magnitude-based row selection, allocation, global state, or game calls.

pub const MAX_TRANSFORMS: usize = 4096;
const MAX_ENTRIES: usize = 32;

pub struct SourceMask {
    count: usize,
    bits: [u64; MAX_TRANSFORMS / 64],
}

impl SourceMask {
    pub fn new(count: usize) -> Option<Self> {
        (count > 0 && count <= MAX_TRANSFORMS).then_some(Self {
            count,
            bits: [0; MAX_TRANSFORMS / 64],
        })
    }

    pub fn insert(&mut self, index: usize) -> Option<()> {
        if index >= self.count {
            return None;
        }
        self.bits[index / 64] |= 1 << (index % 64);
        Some(())
    }

    pub fn contains(&self, index: usize) -> bool {
        index < self.count && self.bits[index / 64] & (1 << (index % 64)) != 0
    }
}

fn read<const N: usize>(
    address: usize,
    memory: &impl Fn(usize, &mut [u8]) -> bool,
) -> Option<[u8; N]> {
    let mut bytes = [0; N];
    (address != 0 && address.checked_add(N).is_some() && memory(address, &mut bytes))
        .then_some(bytes)
}

fn u64_at(bytes: &[u8], offset: usize) -> usize {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap()) as usize
}

fn count_at(bytes: &[u8], offset: usize, limit: usize) -> Option<usize> {
    let count = i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let count = usize::try_from(count).ok()?;
    (count <= limit).then_some(count)
}

/// Native masks already describe selected writebacks. Do not infer them from
/// child positions, scale lanes, or an independently cached activity heuristic.
pub fn capture_mask(
    core: usize,
    source_count: usize,
    memory: &impl Fn(usize, &mut [u8]) -> bool,
) -> Option<SourceMask> {
    let mut result = SourceMask::new(source_count)?;
    let owner = u64_at(&read::<8>(core.checked_add(0x18)?, memory)?, 0);
    if owner == 0 {
        return None;
    }
    if read::<1>(owner.checked_add(0x68)?, memory)?[0] == 0 {
        return Some(result);
    }
    let header = read::<12>(core.checked_add(0x40)?, memory)?;
    let entries = u64_at(&header, 0);
    let count = count_at(&header, 8, MAX_ENTRIES)?;
    if count > 0 && entries == 0 {
        return None;
    }
    let mut words = [0u8; MAX_TRANSFORMS / 8];
    let mut indices = [0u8; MAX_TRANSFORMS * 2];
    for i in 0..count {
        let entry_address = entries.checked_add(i.checked_mul(0x38)?)?;
        let entry = read::<0x38>(entry_address, memory)?;
        let word_count = count_at(&entry, 0x20, MAX_TRANSFORMS / 32)?;
        let bit_count = count_at(&entry, 0x28, MAX_TRANSFORMS)?;
        if word_count != bit_count.div_ceil(32) {
            return None;
        }
        if bit_count == 0 {
            continue;
        }
        let word_pointer = u64_at(&entry, 0x18);
        let output = u64_at(&entry, 0);
        let mapping = u64_at(&entry, 8);
        if word_pointer == 0 || output == 0 || mapping == 0 {
            return None;
        }
        let output_header = read::<4>(output.checked_add(0x20)?, memory)?;
        let output_count = count_at(&output_header, 0, MAX_TRANSFORMS)?;
        // Native output loop accesses one mask bit for every output row.
        if bit_count < output_count {
            return None;
        }
        let map_header = read::<16>(mapping.checked_add(0x10)?, memory)?;
        let map_count = count_at(&map_header, 0, MAX_TRANSFORMS)?;
        let map_pointer = u64_at(&map_header, 8);
        let n = output_count.min(map_count);
        if !memory(word_pointer, &mut words[..word_count * 4])
            || (n > 0 && (map_pointer == 0 || !memory(map_pointer, &mut indices[..n * 2])))
        {
            return None;
        }
        for j in 0..n {
            let word =
                u32::from_le_bytes(words[(j / 32) * 4..(j / 32) * 4 + 4].try_into().unwrap());
            if word & (1 << (j % 32)) != 0 {
                let source = i16::from_le_bytes(indices[j * 2..j * 2 + 2].try_into().unwrap());
                if source >= 0 {
                    result.insert(source as usize)?;
                }
            }
        }
        if read::<0x38>(entry_address, memory)? != entry
            || read::<16>(mapping + 0x10, memory)? != map_header
            || read::<4>(output + 0x20, memory)? != output_header
        {
            return None;
        }
    }
    (read::<12>(core + 0x40, memory)? == header).then_some(result)
}

fn converted(mut value: [f32; 12], scale: f32) -> Option<[f32; 12]> {
    // Native cloth writeback removes root rotation/translation, but not its
    // scale from T; its S *does* divide by root scale. The render root later
    // applies scale to both. Reconcile this asymmetric contract on the copy.
    for (i, lane) in value.iter_mut().enumerate() {
        *lane = if i % 4 == 3 {
            *lane / scale
        } else {
            *lane * scale
        };
    }
    value.iter().all(|x| x.is_finite()).then_some(value)
}

/// Caller supplies a fresh original B455A0 result, not a persistent corrected
/// buffer. Validate the whole selected range before the first output write.
pub fn reconcile_range(
    output: &mut [[f32; 12]],
    mapping: &[i16],
    start: usize,
    mask: &SourceMask,
    scale: f32,
) -> Option<usize> {
    if !scale.is_finite()
        || !(0.5..=3.0).contains(&scale)
        || output.len() > MAX_TRANSFORMS
        || mapping.len() > MAX_TRANSFORMS
        || start.checked_add(output.len())? > mapping.len()
    {
        return None;
    }
    if scale == 1.0 {
        return Some(0);
    }
    let range = &mapping[start..start + output.len()];
    for (value, &index) in output.iter().zip(range) {
        if index >= 0 {
            if index as usize >= mask.count {
                return None;
            }
            if mask.contains(index as usize) {
                converted(*value, scale)?;
            }
        }
    }
    let mut count = 0;
    for (value, &index) in output.iter_mut().zip(range) {
        if index >= 0 && mask.contains(index as usize) {
            *value = converted(*value, scale)?;
            count += 1;
        }
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: [f32; 12] = [2., 0., 0., 0.4, 0., 2., 0., 0.8, 0., 0., 2., 0.2];

    #[test]
    fn selected_source_only_and_negative_fallback_are_preserved() {
        let mut mask = SourceMask::new(4).unwrap();
        mask.insert(2).unwrap();
        let mut out = [ROW; 4];
        assert_eq!(
            reconcile_range(&mut out, &[1, 2, -1, 3], 0, &mask, 0.5),
            Some(1)
        );
        assert_eq!(out[0], ROW);
        assert_eq!(out[2], ROW);
        assert_eq!(out[3], ROW);
        assert_eq!(out[1], [1., 0., 0., 0.8, 0., 1., 0., 1.6, 0., 0., 1., 0.4]);
    }

    #[test]
    fn range_start_repeat_fresh_output_and_restore_are_stateless() {
        let mut mask = SourceMask::new(4).unwrap();
        mask.insert(2).unwrap();
        for scale in [0.5, 0.5, 1., 3., 1., 0.5] {
            let mut out = [ROW; 2];
            assert_eq!(
                reconcile_range(&mut out, &[0, 0, 2, -1], 2, &mask, scale),
                Some(usize::from(scale != 1.))
            );
            assert_eq!(out[1], ROW);
            for (i, original) in ROW.into_iter().enumerate() {
                assert_eq!(
                    out[0][i],
                    if i % 4 == 3 {
                        original / scale
                    } else {
                        original * scale
                    }
                );
            }
        }
    }

    #[test]
    fn invalid_late_row_or_scale_does_not_partially_write() {
        let mut mask = SourceMask::new(4).unwrap();
        mask.insert(2).unwrap();
        let mut out = [ROW; 2];
        out[1][11] = f32::MAX;
        let saved = out;
        assert_eq!(reconcile_range(&mut out, &[2, 2], 0, &mask, 0.5), None);
        assert_eq!(out, saved);
        for scale in [0., 0.49, 3.01, f32::NAN, f32::INFINITY] {
            assert_eq!(reconcile_range(&mut out, &[2, 2], 0, &mask, scale), None);
            assert_eq!(out, saved);
        }
        assert_eq!(reconcile_range(&mut out, &[2, 4], 0, &mask, 3.), None);
        assert_eq!(out, saved);
        assert_eq!(
            reconcile_range(&mut out, &[2], usize::MAX, &mask, 0.5),
            None
        );
    }

    fn put(memory: &mut [u8], address: usize, bytes: &[u8]) {
        memory[address..address + bytes.len()].copy_from_slice(bytes);
    }

    fn native_fixture() -> Vec<u8> {
        let mut m = vec![0; 0x1000];
        for (a, v) in [
            (0x118, 0x200u64),
            (0x140, 0x300),
            (0x300, 0x400),
            (0x308, 0x500),
            (0x318, 0x600),
            (0x518, 0x700),
        ] {
            put(&mut m, a, &v.to_le_bytes());
        }
        m[0x268] = 1;
        for (a, v) in [
            (0x148, 1i32),
            (0x320, 1),
            (0x328, 4),
            (0x420, 4),
            (0x510, 4),
        ] {
            put(&mut m, a, &v.to_le_bytes());
        }
        put(&mut m, 0x600, &0b1101u32.to_le_bytes());
        for (i, v) in [3i16, 1, -1, 5].into_iter().enumerate() {
            put(&mut m, 0x700 + i * 2, &v.to_le_bytes());
        }
        m
    }

    fn reader(m: &[u8]) -> impl Fn(usize, &mut [u8]) -> bool + '_ {
        |a, out| match a.checked_add(out.len()).and_then(|end| m.get(a..end)) {
            Some(bytes) => {
                out.copy_from_slice(bytes);
                true
            }
            None => false,
        }
    }

    #[test]
    fn native_bitmask_selection_not_magnitude_or_all_mapped_rows() {
        let m = native_fixture();
        let mask = capture_mask(0x100, 6, &reader(&m)).unwrap();
        assert_eq!(
            (0..6).filter(|&i| mask.contains(i)).collect::<Vec<_>>(),
            vec![3, 5]
        );
        assert!(!mask.contains(4096));
    }

    #[test]
    fn native_disabled_invalid_header_and_out_of_domain_fail_closed() {
        let mut m = native_fixture();
        m[0x268] = 0;
        assert!(!(0..6).any(|i| capture_mask(0x100, 6, &reader(&m)).unwrap().contains(i)));
        m[0x268] = 1;
        assert!(capture_mask(0x100, 5, &reader(&m)).is_none());
        put(&mut m, 0x320, &2i32.to_le_bytes());
        assert!(capture_mask(0x100, 6, &reader(&m)).is_none());
        assert!(capture_mask(usize::MAX, 6, &reader(&m)).is_none());
        assert!(SourceMask::new(0).is_none());
        assert!(SourceMask::new(4097).is_none());
    }

    #[test]
    fn changed_owner_entry_during_capture_is_rejected() {
        let m = native_fixture();
        let reads = std::cell::Cell::new(0);
        let memory = |a, out: &mut [u8]| {
            if !reader(&m)(a, out) {
                return false;
            }
            if a == 0x300 {
                reads.set(reads.get() + 1);
                if reads.get() > 1 {
                    out[0] ^= 1;
                }
            }
            true
        };
        assert!(capture_mask(0x100, 6, &memory).is_none());
    }
}
