//! Local body dummy queries, including each record of native batches.
//! Keep native tables immutable; replace only the caller's returned matrix.
use super::*;
use std::cell::Cell;

pub(super) const RVAS: [usize; 2] = [0xB697D0, 0xB69850];
pub(super) const GUARD: &[u8] = &[
    0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x74, 0x24, 0x10, 0x57, 0x48, 0x81, 0xec, 0xa0, 0, 0,
    0,
];
pub(super) const AFFINE_RVAS: [usize; 2] = [0xB69600, 0xB69750];
pub(super) const AFFINE_GUARDS: [&[u8]; 2] = [
    &[
        0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x81, 0xec, 0x80, 0, 0, 0, 0x48, 0x8b, 0x49, 0x08,
    ],
    &[
        0x48, 0x89, 0x5c, 0x24, 0x08, 0x48, 0x89, 0x74, 0x24, 0x10, 0x57, 0x48, 0x81, 0xec, 0x80,
        0, 0, 0,
    ],
];
// Read-only diagnostics for local verification; no file output. Each pair is
// matched/applied, first matrix64 then affine48. Not a stable public API.
#[unsafe(no_mangle)]
static ERCS_DUMMY_QUERY_COUNTS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
static ROUTES: Mutex<Vec<Arc<Route>>> = Mutex::new(Vec::new());
thread_local! {static WEAPON_DEPTH: Cell<u32> = const {Cell::new(0)};}
pub(super) struct WeaponScope;
impl WeaponScope {
    pub(super) fn enter() -> Self {
        WEAPON_DEPTH.with(|d| d.set(d.get() + 1));
        Self
    }
}
impl Drop for WeaponScope {
    fn drop(&mut self) {
        WEAPON_DEPTH.with(|d| d.set(d.get() - 1));
    }
}

struct Record {
    header: [u8; 16],
    source: usize,
    target: usize,
    anchor: usize,
    ratio: f64,
    weapon: bool,
}
struct Route {
    session: Arc<Session>,
    array: usize,
    records: usize,
    entries: Vec<Option<Record>>,
    edges: Vec<(usize, usize)>,
    array_count: usize,
    skeleton: usize,
    count: u16,
    table: usize,
}
impl Route {
    fn current(&self) -> bool {
        self.session.active.load(Ordering::Acquire)
            && self.edges.iter().all(|&(at, v)| ptr(at) == Some(v))
            && pose::bytes::<4>(&read, self.table)
                == Some((self.entries.len() as i32).to_le_bytes())
            && pose::bytes::<4>(&read, self.array_count)
                == Some(i32::from(self.count).to_le_bytes())
            && pose::bytes::<2>(&read, self.skeleton) == Some(self.count.to_le_bytes())
    }
    fn record(&self, at: usize) -> Option<&Record> {
        let offset = at.checked_sub(self.records)?;
        if !offset.is_multiple_of(80) {
            return None;
        }
        let r = self.entries.get(offset / 80)?.as_ref()?;
        (pose::bytes::<16>(&read, at) == Some(r.header)).then_some(r)
    }
}
fn capture(session: Arc<Session>, base: usize) -> Option<Route> {
    let mut edges = Vec::new();
    let mut get = |at| {
        let v = ptr(at)?;
        edges.push((at, v));
        Some(v)
    };
    let model = get(session.key.player + 0x50)?;
    if get(model)? != base + 0x2B35C68 {
        return None;
    }
    let item = get(model + 0x10)?;
    let provider = get(item + 0x650)?;
    if get(provider)? != base + 0x2B70598 {
        return None;
    }
    let holder = get(provider + 0x50)?;
    let array = get(holder)?;
    if get(array)? != base + 0x2B6EB88 {
        return None;
    }
    let worlds = get(array + 0x70)?;
    let count = i32::from_le_bytes(pose::bytes(&read, array + 0x68)?);
    if count <= 0 || count as usize > pose::LIMIT {
        return None;
    }
    let holder = get(array + 0x50)?;
    let modifier = get(holder)?;
    if get(modifier)? != base + 0x2B708D0 {
        return None;
    }
    let holder = get(modifier + 0x70)?;
    let animation = get(holder)?;
    if get(animation)? != base + 0x2B6F2C8 {
        return None;
    }
    let skeleton = get(animation + 0x68)?;
    let bones = get(skeleton + 8)?;
    if u16::from_le_bytes(pose::bytes(&read, skeleton)?) as i32 != count {
        return None;
    }
    let table = get(provider + 0x68)?;
    let records = get(table + 8)?;
    let size = i32::from_le_bytes(pose::bytes(&read, table)?);
    if size <= 0 || size as usize > pose::LIMIT {
        return None;
    }
    let names = (0..count as usize)
        .map(|i| pose::text(&read, ptr(bones + i * 64 + 0x20)?, true))
        .collect::<Option<Vec<_>>>()?;
    let parents = (0..count as usize)
        .map(|i| {
            let p = i16::from_le_bytes(pose::bytes(&read, bones + i * 64 + 0x2C)?);
            if p < -1 || p >= count as i16 {
                return None;
            }
            Some((p >= 0).then_some(p as usize))
        })
        .collect::<Option<Vec<_>>>()?;
    let entries = (0..size as usize)
        .map(|i| -> Option<Record> {
            let header = pose::bytes::<16>(&read, records + i * 80)?;
            let bone = i32::from_le_bytes(header[4..8].try_into().ok()?);
            let resolved = crate::equipment_retarget::attachment_anchor(
                &session.source_bones,
                &session.mesh_bones,
                &names,
                &parents,
                usize::try_from(bone).ok()?,
            )?;
            let (source, target, native) = (resolved.source, resolved.target, resolved.native);
            let weapon = resolved.weapon || session.mesh_plan.weapon_attachment_source(source);
            let ratio = session.mesh_plan.attachment_offset_ratio(target)?;
            Some(Record {
                header,
                source,
                target,
                anchor: worlds + native * 48,
                ratio,
                weapon,
            })
        })
        .collect();
    Some(Route {
        session,
        array,
        records,
        entries,
        edges,
        array_count: array + 0x68,
        skeleton,
        count: count as u16,
        table,
    })
}
pub(super) fn refresh(sessions: &[Arc<Session>], base: usize) {
    let Ok(mut routes) = ROUTES.lock() else {
        return;
    };
    routes.retain(|r| r.current() && sessions.iter().any(|s| Arc::ptr_eq(s, &r.session)));
    // Multiple full-body profiles cannot unambiguously own a common body point.
    let mut eligible = sessions.iter().filter(|s| s.motion.is_some());
    let Some(session) = eligible.next() else {
        routes.clear();
        return;
    };
    if eligible.next().is_some() {
        routes.clear();
        return;
    }
    if routes.is_empty()
        && let Some(route) = capture(session.clone(), base)
    {
        routes.push(Arc::new(route));
    }
}
pub(super) fn clear() {
    if let Ok(mut routes) = ROUTES.lock() {
        routes.clear();
    }
}
pub(super) fn hook(registers: *mut Registers, original: usize) -> usize {
    query_hook(registers, original, false)
}
pub(super) fn affine_hook(registers: *mut Registers, original: usize) -> usize {
    query_hook(registers, original, true)
}
fn query_hook(registers: *mut Registers, original: usize, affine: bool) -> usize {
    let r = unsafe { &*registers };
    let native: unsafe extern "C" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original) };
    let result = unsafe { native(r.rcx as usize, r.rdx as usize, r.r8 as usize) };
    if result == 0 || !READY.load(Ordering::Acquire) || WEAPON_DEPTH.with(|d| d.get() != 0) {
        return result;
    }
    // The native function just read this stack wrapper's +8 field itself.
    // Match its immutable upstream before any VirtualQuery/skeleton work so
    // unrelated actors pay only one pointer read and the small route lookup.
    let wrapper = r.rcx as usize;
    let array = unsafe { ((wrapper + 8) as *const usize).read_unaligned() };
    let route = {
        let Ok(routes) = ROUTES.lock() else {
            return result;
        };
        let Some(route) = routes.iter().find(|route| route.array == array) else {
            return result;
        };
        route.clone()
    };
    let counter = if affine { 2 } else { 0 };
    ERCS_DUMMY_QUERY_COUNTS[counter].fetch_add(1, Ordering::Relaxed);
    let stride = if affine { 48 } else { 64 };
    let _ = crate::memory_query::scoped(|| -> Option<()> {
        // Unmapped entries keep the native result. Consult the immutable route
        // first; only mapped records need fresh ownership/pose validation.
        let record = route.record(r.r8 as usize)?;
        let base = BASE.load(Ordering::Acquire);
        if ptr(wrapper) != Some(base + 0x2B753E0) || ptr(wrapper + 8) != Some(array) {
            return None;
        }
        if !route.current() || !route.session.current() {
            return None;
        }
        let address = r.rdx as usize;
        if !crate::memory_query::accessible_span(address, stride, true)
            || !render_output_is_private(&route.session.key, address, stride)
            || overlaps(address, stride, route.records, route.entries.len() * 80)
            || overlaps(
                address,
                stride,
                ptr(array + 0x70)?,
                usize::from(route.count) * 48,
            )
        {
            return None;
        }
        let attachment = if affine {
            pose::affine(&pose::bytes::<48>(&read, address)?)?
        } else {
            let raw = pose::bytes::<64>(&read, address)?;
            DMat4::from_cols_array(&std::array::from_fn(|i| {
                f64::from(f32::from_le_bytes(
                    raw[i * 4..i * 4 + 4].try_into().unwrap(),
                ))
            }))
        };
        let anchor = pose::affine(&pose::bytes::<48>(&read, record.anchor)?)?;
        let mut work = route.session.work.lock().ok()?;
        route.session.prepare(&mut work)?;
        let source = *work.source.model.get(record.source)?;
        let weapon = record.weapon || attachments::owns_record(r.r8 as usize);
        let target = if weapon {
            route
                .session
                .mesh_plan
                .attachment_pose(record.target, &work.mesh)?
        } else {
            route
                .session
                .mesh_plan
                .bone_attachment_pose(record.target, &work.mesh)?
        };
        let corrected = crate::equipment_retarget::body_attachment_frame(
            source,
            target,
            anchor,
            attachment,
            if weapon { 1.0 } else { record.ratio },
        )?;
        let mut output = [0.0f32; 16];
        if affine {
            output[..12].copy_from_slice(&pose::affine_output(corrected)?);
        } else {
            output = corrected.to_cols_array().map(|v| v as f32);
        }
        if output.iter().any(|v| !v.is_finite()) || !route.current() {
            return None;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(output.as_ptr(), address as *mut f32, stride / 4);
        }
        ERCS_DUMMY_QUERY_COUNTS[counter + 1].fetch_add(1, Ordering::Relaxed);
        Some(())
    });
    result
}
fn overlaps(a: usize, n: usize, b: usize, m: usize) -> bool {
    a < b.saturating_add(m) && b < a.saturating_add(n)
}
