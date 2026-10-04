//! Weapon-only dummy consumers. Bind against the local assembly; never replace
//! shared dummy queries used by unrelated actors, effects, or gameplay probes.
use super::*;

pub(super) const RVA: usize = 0xB47060;
pub(super) const GUARD: &[u8] = &[
    0x40, 0x53, 0x48, 0x83, 0xEC, 0x30, 0x48, 0x8B, 0x99, 0xA8, 0, 0, 0, 0x45, 0x33, 0xD2,
];
static ROUTES: Mutex<Vec<Arc<Route>>> = Mutex::new(Vec::new());

#[derive(Clone)]
struct Route {
    session: Arc<Session>,
    node: usize,
    slot: usize,
    record: usize,
    record_header: [u8; 16],
    anchor_world: usize,
    source_anchor: usize,
    target_anchor: usize,
    array_count: usize,
    skeleton_count: usize,
    count: u16,
    edges: Vec<(usize, usize)>,
}

impl Route {
    fn current(&self) -> bool {
        self.session.active.load(Ordering::Acquire)
            && self.edges.iter().all(|&(at, value)| ptr(at) == Some(value))
            && pose::bytes(&read, self.record) == Some(self.record_header)
            && pose::bytes::<4>(&read, self.node + 0x80) == Some(1i32.to_le_bytes())
            && pose::bytes::<4>(&read, self.array_count)
                == Some(i32::from(self.count).to_le_bytes())
            && pose::bytes::<2>(&read, self.skeleton_count) == Some(self.count.to_le_bytes())
    }
}

fn capture(session: Arc<Session>, slot: usize, base: usize) -> Option<Route> {
    let equipment = ptr(slot)?;
    if ptr(equipment) != Some(base + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA) {
        return None;
    }
    let item = ptr(equipment + 0x10)?;
    if !model_name(item)?.starts_with("WP_") {
        return None;
    }
    let transform = ptr(equipment + 0x20)?;
    if ptr(transform) != Some(base + 0x2B6ED78) {
        return None;
    }
    let mut edges = vec![
        (slot, equipment),
        (
            equipment,
            base + crate::cloth_owner_scope::EQUIPMENT_VTABLE_RVA,
        ),
        (equipment + 0x10, item),
        (equipment + 0x20, transform),
        (transform, base + 0x2B6ED78),
    ];
    let mut get = |at| {
        let value = ptr(at)?;
        edges.push((at, value));
        Some(value)
    };
    let holder = get(transform + 0x50)?;
    let node = get(holder)?;
    if get(node)? != base + 0x2B6F878 {
        return None;
    }
    let record = get(node + 0x78)?;
    if i32::from_le_bytes(pose::bytes(&read, node + 0x80)?) != 1 {
        return None;
    }
    let dummy_provider = get(node + 0xA8)?;
    if get(dummy_provider)? != base + 0x2B70598 {
        return None;
    }
    let holder = get(dummy_provider + 0x50)?;
    let array = get(holder)?;
    if get(array)? != base + 0x2B6EB88 {
        return None;
    }
    let world_array = get(array + 0x70)?;
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
    let bone_count = u16::from_le_bytes(pose::bytes(&read, skeleton)?) as usize;
    if bone_count != count as usize {
        return None;
    }
    let record_header = pose::bytes::<16>(&read, record)?;
    let index = i32::from_le_bytes(record_header[4..8].try_into().unwrap());
    if index < 0 || index as usize >= bone_count {
        return None;
    }
    let names = (0..bone_count)
        .map(|i| pose::text(&read, ptr(bones + i * 64 + 0x20)?, true))
        .collect::<Option<Vec<_>>>()?;
    let parents = (0..bone_count)
        .map(|i| {
            let p = i16::from_le_bytes(pose::bytes(&read, bones + i * 64 + 0x2C)?);
            if p < -1 || p as usize >= bone_count && p != -1 {
                return None;
            }
            Some((p >= 0).then_some(p as usize))
        })
        .collect::<Option<Vec<_>>>()?;
    let anchor = crate::equipment_retarget::attachment_anchor(
        &session.source_bones,
        &session.mesh_bones,
        &names,
        &parents,
        index as usize,
    )?;
    Some(Route {
        session,
        node,
        slot,
        record,
        record_header,
        anchor_world: world_array + anchor.native * 48,
        source_anchor: anchor.source,
        target_anchor: anchor.target,
        array_count: array + 0x68,
        skeleton_count: skeleton,
        count: bone_count as u16,
        edges,
    })
}

pub(super) fn refresh(sessions: &[Arc<Session>], base: usize) {
    let Ok(mut routes) = ROUTES.lock() else {
        return;
    };
    routes.retain(|r| r.current() && sessions.iter().any(|s| Arc::ptr_eq(s, &r.session)));
    for session in sessions {
        // Body profiles require both legs and Head. An arm-only replacement
        // must not accidentally move all of the player's weapon slots.
        if session.motion.is_none() {
            continue;
        }
        for index in 7..21 {
            let slot = session.key.assembly + 0x28 + index * 8;
            if routes
                .iter()
                .any(|r| r.slot == slot && Arc::ptr_eq(&r.session, session))
            {
                continue;
            }
            if ptr(slot).is_none_or(|v| v == 0) {
                continue;
            }
            if let Some(route) = capture(session.clone(), slot, base) {
                routes.push(Arc::new(route));
            }
        }
    }
}

pub(super) fn clear() {
    if let Ok(mut routes) = ROUTES.lock() {
        routes.clear();
    }
}

pub(super) fn hook(registers: *mut Registers, original: usize) -> usize {
    let r = unsafe { &*registers };
    let native: unsafe extern "C" fn(usize, usize, u32) -> usize =
        unsafe { std::mem::transmute(original) };
    // This native selector is shared by weapons, props and effects. Suppress
    // nested correction only when this outer call actually owns a valid
    // weapon correction; other selectors must reach the generic body hook.
    let route = (|| {
        if r.r8 as u32 != 0 || !READY.load(Ordering::Acquire) {
            return None;
        }
        let routes = ROUTES.lock().ok()?;
        let mut matches = routes.iter().filter(|a| a.node == r.rcx as usize);
        let route = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(route.clone())
    })()
    .filter(|route| crate::memory_query::scoped(|| route.current() && route.session.current()));
    let result = {
        let _scope = route.as_ref().map(|_| dummies::WeaponScope::enter());
        unsafe { native(r.rcx as usize, r.rdx as usize, r.r8 as u32) }
    };
    let Some(route) = route.filter(|_| result == 1) else {
        return result;
    };
    let _ = crate::memory_query::scoped(|| -> Option<()> {
        if !route.current() || !route.session.current() {
            return None;
        }
        let address = r.rdx as usize;
        if !crate::memory_query::accessible_span(address, 64, true)
            || !render_output_is_private(&route.session.key, address, 64)
        {
            return None;
        }
        let raw = pose::bytes::<64>(&read, address)?;
        let attachment = DMat4::from_cols_array(&std::array::from_fn(|i| {
            f64::from(f32::from_le_bytes(
                raw[i * 4..i * 4 + 4].try_into().unwrap(),
            ))
        }));
        let anchor = pose::affine(&pose::bytes::<48>(&read, route.anchor_world)?)?;
        let mut work = route.session.work.lock().ok()?;
        route.session.prepare(&mut work)?;
        let source = *work.source.model.get(route.source_anchor)?;
        let target = route
            .session
            .mesh_plan
            .attachment_pose(route.target_anchor, &work.mesh)?;
        let corrected =
            crate::equipment_retarget::attachment_frame(source, target, anchor, attachment)?;
        let corrected = corrected.to_cols_array().map(|v| v as f32);
        if corrected.iter().any(|v| !v.is_finite()) || !route.current() {
            return None;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(corrected.as_ptr(), address as *mut f32, 16);
        }
        Some(())
    });
    result
}

/// Also used by effects querying a currently selected weapon/sheath record.
pub(super) fn owns_record(record: usize) -> bool {
    ROUTES.lock().is_ok_and(|routes| {
        routes
            .iter()
            .any(|r| r.record == record && r.session.active.load(Ordering::Acquire))
    })
}
