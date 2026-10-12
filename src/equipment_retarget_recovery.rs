//! Replay the native simulation reset markers on the current owned equipment.
//! Particle coordinates, reference data and shared asset dimensions stay native.
use super::*;

// ER's state transition clears each active child's frame counter and requests
// a group reset; core skin-input processing consumes that group flag once.
pub(super) const RESET_RVA: usize = 0x26B1FC9;
pub(super) const RESET_GUARD: &[u8] = &[
    0x49, 0x8B, 0x46, 0x40, 0x49, 0xFF, 0xC2, 0x4E, 0x8B, 0x04, 0xC0, 0x41, 0x89, 0x68, 0x50, 0x4D,
    0x3B, 0xD3, 0x75, 0xE5, 0xC6, 0x47, 0x4F, 0x01,
];
pub(super) const CONSUME_RVA: usize = 0x26B1BC9;
pub(super) const CONSUME_GUARD: &[u8] = &[
    0x80, 0x7B, 0x4F, 0x00, 0x74, 0x0F, 0x48, 0x8B, 0x0B, 0x48, 0x8B, 0xD1,
];

pub(super) fn request_reset(session: &Session) -> Option<()> {
    let core = session.key.core;
    let begin = ptr(core + 0x28)?;
    let end = ptr(core + 0x30)?;
    let bytes = end.checked_sub(begin)?;
    if !bytes.is_multiple_of(8) || bytes == 0 || bytes / 8 > 32 {
        return None;
    }
    let mut markers = Vec::new();
    let mut groups = Vec::new();
    let mut edges = Vec::new();
    let mut scalars = Vec::new();
    for i in 0..bytes / 8 {
        let holder = ptr(begin + i * 8)?;
        let root = ptr(holder)?;
        let asset = ptr(root + 0x18)?;
        let children = ptr(root + 0x40)?;
        let count = i32::from_le_bytes(pose::bytes::<4>(&read, root + 0x48)?);
        let state_index = i32::from_le_bytes(pose::bytes::<4>(&read, root + 0x114)?);
        let states = ptr(asset + 0x60)?;
        let state_count = i32::from_le_bytes(pose::bytes::<4>(&read, asset + 0x68)?);
        if !(1..=256).contains(&count)
            || !(1..=256).contains(&state_count)
            || state_index < 0
            || state_index >= state_count
        {
            return None;
        }
        let state = ptr(states + state_index as usize * 8)?;
        let indices = ptr(state + 0x50)?;
        let active = i32::from_le_bytes(pose::bytes::<4>(&read, state + 0x58)?);
        scalars.extend([
            (root + 0x48, count.to_le_bytes()),
            (root + 0x114, state_index.to_le_bytes()),
            (asset + 0x68, state_count.to_le_bytes()),
            (state + 0x58, active.to_le_bytes()),
        ]);
        if active < 0 || active > count {
            return None;
        }
        edges.extend([
            (begin + i * 8, holder),
            (holder, root),
            (root + 0x18, asset),
            (root + 0x40, children),
            (asset + 0x60, states),
            (states + state_index as usize * 8, state),
            (state + 0x50, indices),
        ]);
        for k in 0..active as usize {
            let index = i32::from_le_bytes(pose::bytes::<4>(&read, indices + k * 4)?);
            scalars.push((indices + k * 4, index.to_le_bytes()));
            if index < 0 || index >= count {
                return None;
            }
            let child_at = children + index as usize * 8;
            let child = ptr(child_at)?;
            // The engine resets this counter, not the shared SimClothData.
            if !crate::memory_query::accessible_span(child + 0x50, 4, true) {
                return None;
            }
            edges.push((child_at, child));
            markers.push(child + 0x50);
        }
        if active != 0 {
            if !crate::memory_query::accessible_span(holder + 0x4f, 1, true) {
                return None;
            }
            groups.push(holder + 0x4f);
        }
    }
    if groups.is_empty()
        || !session.current()
        || ptr(core + 0x28) != Some(begin)
        || ptr(core + 0x30) != Some(end)
        || edges.iter().any(|&(at, value)| ptr(at) != Some(value))
        || scalars
            .iter()
            .any(|&(at, value)| pose::bytes::<4>(&read, at) != Some(value))
    {
        return None;
    }
    for address in markers {
        unsafe {
            (address as *mut u32).write_unaligned(0);
        }
    }
    for address in groups {
        unsafe {
            (address as *mut u8).write(1);
        }
    }
    Some(())
}
