//! Select a shared grip only for weapons whose native definition shares one.
use crate::equipment_retarget_pose::{self as pose, Reader};

#[derive(Default)]
pub(super) struct GripCache {
    key: Option<(usize, usize, u32, u16, u32)>,
    row: Option<usize>,
}

impl GripCache {
    pub(super) fn style(&mut self, read: &Reader<'_>, player: usize, base: usize) -> u32 {
        self.resolve(read, player, base).unwrap_or(0)
    }

    fn resolve(&mut self, read: &Reader<'_>, player: usize, base: usize) -> Option<u32> {
        let assembly = pose::pointer(read, player + 0x638)?;
        let style = u32::from_le_bytes(pose::bytes(read, assembly + 8)?);
        let side = match style {
            2 => 0,
            3 => 1,
            _ => return Some(0),
        };
        let slot = u32::from_le_bytes(pose::bytes(read, assembly + 12 + side * 4)?) as usize;
        if slot >= 3 {
            return None;
        }
        let id = i32::from_le_bytes(pose::bytes(read, assembly + 0x7c + (slot * 2 + side) * 4)?);
        if id < 0 {
            return None;
        }
        // Same reinforcement normalization as SoloParamRepository::get_item.
        let id = id as u32 / 100 * 100;
        // Empty slots resolve to the native bare-hand weapon, not a negative
        // ID. It has no isDualBlade bit but still cannot share a rigid grip.
        // Keep the animated hands independent in either two-hand style.
        if id == 110000 {
            return Some(0);
        }
        // WW2.7.1.0 constructor D220F2 stores the SoloParamRepository here.
        let repo = pose::pointer(read, base + 0x3D85F58)?;
        if pose::pointer(read, repo)? != base + 0x2BB84C8 {
            return None;
        }
        let holders = u32::from_le_bytes(pose::bytes(read, repo + 0x80)?);
        if !(1..=8).contains(&holders) {
            return None;
        }
        let cap = pose::pointer(read, repo + 0x88)?;
        let resource = pose::pointer(read, cap + 0x80)?;
        let size = pose::pointer(read, resource + 0x78)?;
        let file = pose::pointer(read, resource + 0x80)?;
        if !(0x40..=256 * 1024 * 1024).contains(&size) {
            return None;
        }
        let header = pose::bytes::<48>(read, file)?;
        // Restrict decoding to the verified little-endian, extended, 64-bit
        // offset format. Unknown tables keep the ordinary retargeted arms.
        if header[0x2c] != 0 || header[0x2d] != 0x85 {
            return None;
        }
        let count = u16::from_le_bytes(header[10..12].try_into().ok()?);
        let metadata = pose::bytes::<8>(read, file.checked_sub(16)?)?;
        let end = u32::from_le_bytes(metadata[..4].try_into().ok()?);
        if count == 0
            || u32::from(count) != u32::from_le_bytes(metadata[4..].try_into().ok()?)
            || end as usize > size
        {
            return None;
        }
        let key = (file, size, end, count, id);
        if self.key != Some(key) {
            self.key = Some(key);
            self.row = None;
            let name = u64::from_le_bytes(header[16..24].try_into().ok()?) as usize;
            if name.checked_add(22)? > size
                || pose::bytes::<22>(read, file.checked_add(name)?)? != *b"EQUIP_PARAM_WEAPON_ST\0"
            {
                return None;
            }
            self.row = find_row(read, file, size, end, count, id);
        }
        let flag = pose::bytes::<1>(read, file.checked_add(self.row?)?.checked_add(0x17c)?)?[0];
        // isDualBlade is bit 1 of EquipParamWeapon +17C. Read the actual flag
        // each frame; only the row lookup is cached. Swaps/table replacement
        // invalidate the key, including left versus right selected slots.
        Some(if flag & 2 == 0 { style } else { 0 })
    }
}

fn find_row(
    read: &Reader<'_>,
    file: usize,
    size: usize,
    end: u32,
    count: u16,
    id: u32,
) -> Option<usize> {
    // The engine appends a sorted ID -> row-index table after the file bytes.
    let lookup = file.checked_add((end as usize).checked_add(15)? & !15)?;
    let (mut lo, mut hi) = (0usize, usize::from(count));
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let entry = pose::bytes::<8>(read, lookup.checked_add(mid * 8)?)?;
        let found = u32::from_le_bytes(entry[..4].try_into().ok()?);
        match found.cmp(&id) {
            std::cmp::Ordering::Less => lo = mid + 1,
            std::cmp::Ordering::Greater => hi = mid,
            std::cmp::Ordering::Equal => {
                let index = u32::from_le_bytes(entry[4..].try_into().ok()?) as usize;
                if index >= usize::from(count) || 0x40 + index * 24 + 24 > size {
                    return None;
                }
                let descriptor = pose::bytes::<24>(read, file.checked_add(0x40 + index * 24)?)?;
                if u32::from_le_bytes(descriptor[..4].try_into().ok()?) != id {
                    return None;
                }
                let offset = u64::from_le_bytes(descriptor[8..16].try_into().ok()?) as usize;
                return (offset >= 0x40 + usize::from(count) * 24
                    && offset.checked_add(0x17d)? <= size)
                    .then_some(offset);
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "equipment_retarget_weapon_tests.rs"]
mod tests;
