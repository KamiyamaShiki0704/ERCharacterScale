//! Recover a failed owned simulation from its immediately preceding finite
//! state. Run only after the synchronous native solve returns. Never seed a
//! different instance, reuse stale state, or alter shared asset dimensions.
use super::*;
use glam::DVec3;
use std::time::{Duration, Instant};

const MAX_TOTAL_PARTICLES: usize = 65_536;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Layout {
    list: usize,
    end: usize,
    holder_slot: usize,
    holder: usize,
    root: usize,
    asset: usize,
    children: usize,
    member: usize,
    active_entry: usize,
    index: i32,
    state: i32,
    child: usize,
    sim: usize,
    current: usize,
    previous: usize,
    count: usize,
}

impl Layout {
    fn current(&self, session: &Session) -> bool {
        let base = BASE.load(Ordering::Acquire);
        ptr(session.key.core + 0x28) == Some(self.list)
            && ptr(session.key.core + 0x30) == Some(self.end)
            && ptr(self.holder_slot) == Some(self.holder)
            && ptr(self.holder) == Some(self.root)
            && ptr(self.root + 0x18) == Some(self.asset)
            && ptr(self.root + 0x40) == Some(self.children)
            && pose::bytes::<4>(&read, self.root + 0x114) == Some(self.state.to_le_bytes())
            && pose::bytes::<4>(&read, self.active_entry) == Some(self.index.to_le_bytes())
            && ptr(self.member) == Some(self.child)
            && ptr(self.child) == Some(base + crate::compatibility::rva(0x2D872A0))
            && ptr(self.child + 0x18) == Some(self.sim)
            && ptr(self.child + 0x268) == Some(self.root)
            && ptr(self.sim) == Some(base + crate::compatibility::rva(0x2D8B6F8))
            && ptr(self.child + 0x20) == Some(self.current)
            && ptr(self.child + 0x30) == Some(self.previous)
            && [self.sim + 0x48, self.child + 0x28, self.child + 0x38]
                .iter()
                .all(|&at| pose::bytes::<4>(&read, at) == Some((self.count as i32).to_le_bytes()))
    }
}

fn count(at: usize, max: usize) -> Option<usize> {
    usize::try_from(i32::from_le_bytes(pose::bytes(&read, at)?))
        .ok()
        .filter(|&n| n <= max)
}

fn layouts(session: &Session, result: &mut Vec<Layout>) -> Option<()> {
    result.clear();
    let list = ptr(session.key.core + 0x28)?;
    let end = ptr(session.key.core + 0x30)?;
    let bytes = end.checked_sub(list)?;
    if !bytes.is_multiple_of(8) || bytes / 8 > 32 {
        return None;
    }
    let mut total = 0usize;
    for g in 0..bytes / 8 {
        let holder_slot = list + g * 8;
        let holder = ptr(holder_slot)?;
        let root = ptr(holder)?;
        let asset = ptr(root + 0x18)?;
        let children = ptr(root + 0x40)?;
        let child_count = count(root + 0x48, 256)?;
        let state = i32::from_le_bytes(pose::bytes(&read, root + 0x114)?);
        let states = ptr(asset + 0x60)?;
        let state_count = count(asset + 0x68, 256)?;
        if state < 0 || state as usize >= state_count {
            return None;
        }
        let active_state = ptr(states + state as usize * 8)?;
        let active = ptr(active_state + 0x50)?;
        let n = count(active_state + 0x58, child_count)?;
        for i in 0..n {
            let active_entry = active + i * 4;
            let index = i32::from_le_bytes(pose::bytes(&read, active_entry)?);
            if index < 0 || index as usize >= child_count {
                return None;
            }
            let member = children + index as usize * 8;
            let child = ptr(member)?;
            let sim = ptr(child + 0x18)?;
            let count = count(sim + 0x48, pose::LIMIT)?;
            if count == 0 {
                continue;
            }
            total = total.checked_add(count)?;
            if total > MAX_TOTAL_PARTICLES {
                return None;
            }
            let layout = Layout {
                list,
                end,
                holder_slot,
                holder,
                root,
                asset,
                children,
                member,
                active_entry,
                index,
                state,
                child,
                sim,
                current: ptr(child + 0x20)?,
                previous: ptr(child + 0x30)?,
                count,
            };
            if !layout.current(session) {
                return None;
            }
            if result.len() >= 256 {
                return None;
            }
            result.push(layout);
        }
    }
    // No aliased histories, other children, or object headers may be written.
    for (i, a) in result.iter().enumerate() {
        for x in [a.current, a.previous] {
            let end = x.checked_add(a.count * 16)?;
            for (j, b) in result.iter().enumerate() {
                for y in [b.current, b.previous] {
                    if i == j && x == y {
                        continue;
                    }
                    if x < y.checked_add(b.count * 16)? && y < end {
                        return None;
                    }
                }
                for (at, len) in [(b.child, 0x270), (b.sim, 0x200), (b.root, 0x160)] {
                    if x < at.checked_add(len)? && at < end {
                        return None;
                    }
                }
            }
            if a.current == a.previous {
                return None;
            }
        }
    }
    Some(())
}

fn placement(session: &Session) -> Option<DMat4> {
    let raw = pose::bytes::<64>(&read, session.key.owner + 0x60)?;
    let m = DMat4::from_cols_array(&std::array::from_fn(|i| {
        f32::from_le_bytes(raw[i * 4..i * 4 + 4].try_into().unwrap()) as f64
    }));
    (m.is_finite()
        && m.determinant().abs() > 1e-10
        && m.x_axis.w.abs() < 1e-5
        && m.y_axis.w.abs() < 1e-5
        && m.z_axis.w.abs() < 1e-5
        && (m.w_axis.w - 1.0).abs() < 1e-5)
        .then_some(m)
}

fn as_bytes(values: &mut [[f32; 4]]) -> &mut [u8] {
    // Every bit pattern is a valid float representation; validation follows.
    unsafe { std::slice::from_raw_parts_mut(values.as_mut_ptr().cast(), values.len() * 16) }
}

struct History {
    layout: Layout,
    good_current: Vec<[f32; 4]>,
    good_previous: Vec<[f32; 4]>,
    current: Vec<[f32; 4]>,
    previous: Vec<[f32; 4]>,
    frame: Option<u64>,
    time: Instant,
    placement: DMat4,
}

impl History {
    fn new(layout: Layout) -> Self {
        Self {
            layout,
            good_current: Vec::new(),
            good_previous: Vec::new(),
            current: Vec::new(),
            previous: Vec::new(),
            frame: None,
            time: Instant::now(),
            placement: DMat4::IDENTITY,
        }
    }
    fn observe(&mut self, session: &Session, generation: u64, placement: DMat4) -> Option<bool> {
        let l = self.layout;
        self.current.resize(l.count, [0.0; 4]);
        self.previous.resize(l.count, [0.0; 4]);
        if !read(l.current, as_bytes(&mut self.current))
            || !read(l.previous, as_bytes(&mut self.previous))
            || !l.current(session)
            || !session.current()
        {
            return None;
        }
        let finite = |v: &Vec<[f32; 4]>| v.iter().flatten().all(|x| x.is_finite());
        if finite(&self.current) && finite(&self.previous) {
            std::mem::swap(&mut self.good_current, &mut self.current);
            std::mem::swap(&mut self.good_previous, &mut self.previous);
            self.frame = Some(generation);
            self.time = Instant::now();
            self.placement = placement;
            return Some(false);
        }
        let age = generation.checked_sub(self.frame?)?;
        if age > 2
            || self.time.elapsed() > Duration::from_millis(100)
            || self.good_current.len() != l.count
            || self.good_previous.len() != l.count
        {
            return None;
        }
        let delta = placement * self.placement.inverse();
        if !delta.is_finite() {
            return None;
        }
        for i in 0..l.count {
            let old = self.good_current[i];
            let p = delta.transform_point3(DVec3::new(old[0] as f64, old[1] as f64, old[2] as f64));
            self.current[i] = [p.x as f32, p.y as f32, p.z as f32, old[3]];
            self.previous[i] = [p.x as f32, p.y as f32, p.z as f32, self.good_previous[i][3]];
        }
        if !finite(&self.current)
            || !finite(&self.previous)
            || !session.current()
            || !l.current(session)
            || !crate::memory_query::accessible_span(l.current, l.count * 16, true)
            || !crate::memory_query::accessible_span(l.previous, l.count * 16, true)
            || !crate::memory_query::accessible_span(l.child + 0x50, 4, true)
            || !render_output_is_private(&session.key, l.current, l.count * 16)
            || !render_output_is_private(&session.key, l.previous, l.count * 16)
        {
            return None;
        }
        // Clear the failed Verlet velocity while preserving each stream's W.
        // No engine calls or canonical skeleton/asset writes occur here.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.current.as_ptr(),
                l.current as *mut [f32; 4],
                l.count,
            );
            std::ptr::copy_nonoverlapping(
                self.previous.as_ptr(),
                l.previous as *mut [f32; 4],
                l.count,
            );
            ((l.child + 0x50) as *mut f32).write_unaligned(0.0);
        }
        Some(true)
    }
}

#[derive(Default)]
pub(super) struct Monitor {
    layouts: Vec<Layout>,
    history: Vec<History>,
}

impl Monitor {
    pub(super) fn clear(&mut self) {
        self.history.clear();
        self.layouts.clear();
    }
    pub(super) fn observe(&mut self, session: &Session, generation: u64) -> Option<usize> {
        let placement = placement(session)?;
        layouts(session, &mut self.layouts)?;
        self.history.retain(|h| self.layouts.contains(&h.layout));
        let mut repaired = 0;
        for l in &self.layouts {
            let index = if let Some(i) = self.history.iter().position(|h| h.layout == *l) {
                i
            } else {
                self.history.push(History::new(*l));
                self.history.len() - 1
            };
            repaired += usize::from(
                self.history[index]
                    .observe(session, generation, placement)
                    .unwrap_or(false),
            );
        }
        Some(repaired)
    }
}
