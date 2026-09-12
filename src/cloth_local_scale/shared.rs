//! Preflight mutable cloth resources across characters before the first write.
//! Shared immutable baselines prevent a newly loaded instance from capturing
//! another instance's scaled data. Conflicting consumers remain at baseline.
use super::*;
use crate::unit_runtime::Identity;
use std::{cell::RefCell, collections::HashSet, sync::Arc};

#[cfg(test)]
#[path = "../test_support/convex_replay.rs"]
mod convex_replay;

#[derive(Clone, Copy)]
pub(crate) struct Consumer {
    pub identity: Identity,
    pub scale: f32,
}

struct Resource {
    baseline: Arc<DimensionObjectBaseline>,
    owners: HashSet<Identity>,
    spans: Vec<(usize, usize)>,
}

#[derive(Default)]
pub(crate) struct Pool {
    resources: HashMap<usize, Resource>,
}

#[derive(Default)]
pub(crate) struct Prepared {
    objects: HashMap<usize, Arc<DimensionObjectBaseline>>,
    spans: HashMap<usize, Vec<(usize, usize)>>,
    scales: HashMap<usize, f32>,
    protected: Vec<(usize, usize)>,
    pub rejected: HashMap<usize, &'static str>,
}

thread_local! {
    static PREPARED: RefCell<Option<Arc<Prepared>>> = const { RefCell::new(None) };
}

pub(crate) fn with_prepared<T>(prepared: &Arc<Prepared>, run: impl FnOnce() -> T) -> T {
    struct Guard(Option<Arc<Prepared>>);
    impl Drop for Guard {
        fn drop(&mut self) {
            PREPARED.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let _guard = Guard(PREPARED.with(|slot| slot.replace(Some(prepared.clone()))));
    run()
}

pub(super) fn capture(
    address: usize,
    fallback: impl FnOnce() -> Option<DimensionObjectBaseline>,
) -> Option<DimensionObjectBaseline> {
    PREPARED.with(|slot| match slot.borrow().as_ref() {
        Some(prepared) => prepared
            .objects
            .get(&address)
            .filter(|object| object.identity_matches())
            .map(|object| (**object).clone()),
        None => fallback(),
    })
}

pub(super) fn retained_by_scaled_consumer(object: &DimensionObjectBaseline) -> bool {
    PREPARED.with(|slot| {
        slot.borrow().as_ref().is_some_and(|prepared| {
            if prepared
                .scales
                .get(&object.address)
                .is_some_and(|scale| *scale != 1.0)
            {
                return true;
            }
            if prepared.protected.is_empty() {
                return false;
            }
            // Current resources already have validated immutable spans. A root
            // that departed this pass still needs its old spans for alias checks.
            let departed;
            let spans = if let Some(spans) = prepared.spans.get(&object.address) {
                spans.as_slice()
            } else {
                departed = resource_spans(object);
                &departed
            };
            spans.iter().any(|&(begin, end)| {
                prepared
                    .protected
                    .iter()
                    .any(|&(other_begin, other_end)| begin < other_end && other_begin < end)
            })
        })
    })
}

impl Pool {
    pub fn prepare(&mut self, consumers: &[Consumer]) -> Arc<Prepared> {
        crate::memory_query::scoped(|| self.prepare_inner(consumers))
    }

    fn prepare_inner(&mut self, consumers: &[Consumer]) -> Arc<Prepared> {
        let mut prepared = Prepared::default();
        let mut owners: HashMap<usize, HashSet<Identity>> = HashMap::new();
        let mut requested = HashMap::new();
        for consumer in consumers {
            requested.insert(consumer.identity.address, consumer.scale);
            let Some(simulations) = simulations(consumer.identity) else {
                prepared.rejected.insert(
                    consumer.identity.address,
                    "cloth-consumer-topology-unreadable",
                );
                continue;
            };
            // Keep direct roots even if a nested resource cannot be read.
            for &root in &simulations {
                owners.entry(root).or_default().insert(consumer.identity);
            }
            let Some((collidables, shapes, unsupported)) =
                collect_live_collision_roots(&simulations)
            else {
                prepared.rejected.insert(
                    consumer.identity.address,
                    "cloth-consumer-collisions-unreadable",
                );
                continue;
            };
            if unsupported != 0 {
                prepared.rejected.insert(
                    consumer.identity.address,
                    "cloth-consumer-shape-unsupported",
                );
            }
            for root in collidables.into_iter().chain(shapes) {
                owners.entry(root).or_default().insert(consumer.identity);
            }
        }
        // Retain an original only while at least one exact consumer continues
        // owning the same validated layout. A recycled address is a new epoch.
        self.resources.retain(|root, resource| {
            owners.get(root).is_some_and(|now| {
                now.iter().any(|owner| {
                    resource
                        .owners
                        .iter()
                        .any(|previous| previous.same_instance(*owner))
                })
            })
        });
        let mut canonical = HashMap::new();
        if owners.keys().any(|root| !self.resources.contains_key(root)) {
            for resource in self
                .resources
                .values()
                .filter(|resource| resource.baseline.identity_matches())
            {
                for field in &resource.baseline.fields {
                    canonical.entry(field.address).or_insert(*field);
                }
            }
        }
        for (&root, root_owners) in &owners {
            if let Some(resource) = self.resources.get_mut(&root) {
                resource.owners = root_owners.clone();
                if !resource.baseline.identity_matches() {
                    for owner in root_owners {
                        prepared
                            .rejected
                            .insert(owner.address, "cloth-resource-layout-changed");
                    }
                }
                continue;
            }
            let baseline = match read_usize(root).map(body_scale_port::module_rva) {
                Some(ER_HCL_SIM_CLOTH_DATA_VTABLE_RVA) => capture_simulation_baseline(root),
                Some(ER_HCL_COLLIDABLE_VTABLE_RVA) => capture_collidable_baseline(root),
                Some(_) => capture_shape_baseline(root),
                None => None,
            };
            let Some(mut baseline) = baseline.filter(|baseline| baseline.unsupported_layouts == 0)
            else {
                for owner in root_owners {
                    prepared
                        .rejected
                        .insert(owner.address, "cloth-shared-baseline-unavailable");
                }
                continue;
            };
            let mut coherent = true;
            for field in &mut baseline.fields {
                if let Some(original) = canonical.get(&field.address) {
                    if original.power != field.power
                        || original.preserve_unbounded != field.preserve_unbounded
                    {
                        coherent = false;
                        break;
                    }
                    field.baseline = original.baseline;
                }
            }
            if !coherent {
                for owner in root_owners {
                    prepared
                        .rejected
                        .insert(owner.address, "cloth-shared-field-semantics-conflict");
                }
                continue;
            }
            for field in &baseline.fields {
                canonical.entry(field.address).or_insert(*field);
            }
            baseline.rebuild_numeric_bounds();
            let spans = resource_spans(&baseline);
            self.resources.insert(
                root,
                Resource {
                    baseline: Arc::new(baseline),
                    owners: root_owners.clone(),
                    spans,
                },
            );
        }

        // Join consumers through both identical roots and overlapping mutable
        // arrays. Whole arrays are conservative and avoid a per-particle hash
        // table every frame. Inline scalar spans cover shape/constraint fields.
        let addresses: Vec<_> = consumers
            .iter()
            .map(|consumer| consumer.identity.address)
            .collect();
        let indices: HashMap<_, _> = addresses
            .iter()
            .enumerate()
            .map(|(i, address)| (*address, i))
            .collect();
        let mut groups = Groups::new(addresses.len());
        let mut spans = Vec::new();
        for (&root, root_owners) in &owners {
            let Some(first) = root_owners
                .iter()
                .next()
                .and_then(|owner| indices.get(&owner.address))
                .copied()
            else {
                continue;
            };
            for owner in root_owners {
                groups.join(first, indices[&owner.address]);
            }
            spans.push((root, root.saturating_add(8), first));
            if let Some(resource) = self.resources.get(&root) {
                spans.extend(
                    resource
                        .spans
                        .iter()
                        .map(|&(start, end)| (start, end, first)),
                );
            }
        }
        let mut poses: HashMap<usize, usize> = HashMap::new();
        for (i, consumer) in consumers.iter().enumerate() {
            for key in [
                consumer.identity.pose,
                consumer.identity.cloth_pose,
                consumer.identity.skeleton,
            ]
            .into_iter()
            .filter(|key| *key != 0)
            {
                if let Some(previous) = poses.insert(key, i).filter(|previous| *previous != i) {
                    groups.join(previous, i);
                    prepared.rejected.insert(
                        consumer.identity.address,
                        "pose-instance-shared-between-characters",
                    );
                }
            }
        }
        spans.sort_unstable();
        let mut previous: Option<(usize, usize)> = None;
        for (begin, end, owner) in spans {
            match previous {
                Some((previous_end, previous_owner)) if begin < previous_end => {
                    groups.join(owner, previous_owner);
                    previous = Some((previous_end.max(end), previous_owner));
                }
                _ => previous = Some((end, owner)),
            }
        }
        for resource in self.resources.values() {
            for owner in &resource.owners {
                if !resource.baseline.supports_scale(requested[&owner.address]) {
                    prepared
                        .rejected
                        .insert(owner.address, "cloth-scale-numeric-overflow-or-underflow");
                }
            }
        }
        let mut group_scale = HashMap::new();
        let mut blocked = HashSet::new();
        for (i, address) in addresses.iter().enumerate() {
            let group = groups.root(i);
            let scale = requested[address].to_bits();
            if prepared.rejected.contains_key(address) {
                blocked.insert(group);
            }
            if group_scale
                .insert(group, scale)
                .is_some_and(|old| old != scale)
            {
                blocked.insert(group);
            }
        }
        for (i, address) in addresses.iter().enumerate() {
            if blocked.contains(&groups.root(i)) {
                prepared
                    .rejected
                    .entry(*address)
                    .or_insert("shared-cloth-consumers-require-different-scales");
            }
        }
        for (&root, resource) in &self.resources {
            let owner = resource
                .owners
                .iter()
                .next()
                .expect("resource has a consumer");
            prepared.scales.insert(
                root,
                if prepared.rejected.contains_key(&owner.address) {
                    1.0
                } else {
                    requested[&owner.address]
                },
            );
            prepared.objects.insert(root, resource.baseline.clone());
            prepared.spans.insert(root, resource.spans.clone());
            if prepared.scales[&root] != 1.0 {
                prepared.protected.extend_from_slice(&resource.spans);
            }
        }
        Arc::new(prepared)
    }
}

fn resource_spans(baseline: &DimensionObjectBaseline) -> Vec<(usize, usize)> {
    #[cfg(test)]
    SPAN_BUILDS.with(|calls| calls.set(calls.get() + 1));
    let mut spans: Vec<_> = baseline
        .arrays
        .iter()
        .filter(|array| array.count != 0)
        .map(|array| (array.begin, array.begin + array.count * array.stride))
        .collect();
    for field in &baseline.fields {
        if !spans
            .iter()
            .any(|&(begin, end)| field.address >= begin && field.address < end)
        {
            spans.push((field.address, field.address + 4));
        }
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (begin, end) in spans {
        if let Some(previous) = merged.last_mut().filter(|previous| begin <= previous.1) {
            previous.1 = previous.1.max(end);
        } else {
            merged.push((begin, end));
        }
    }
    merged
}

#[cfg(test)]
thread_local! { static SPAN_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

struct Groups(Vec<usize>);
impl Groups {
    fn new(count: usize) -> Self {
        Self((0..count).collect())
    }
    fn root(&mut self, mut index: usize) -> usize {
        while self.0[index] != index {
            self.0[index] = self.0[self.0[index]];
            index = self.0[index];
        }
        index
    }
    fn join(&mut self, a: usize, b: usize) {
        let a = self.root(a);
        let b = self.root(b);
        self.0[b] = a;
    }
}

/// Bounded pointer topology only; no simulation particle values are read here.
fn simulations(identity: Identity) -> Option<Vec<usize>> {
    use crate::unit_runtime::read;
    let base = body_scale_port::image_base();
    let anchor = if identity.cloth_pose != 0 {
        identity.cloth_pose
    } else {
        identity.pose
    };
    let scope = crate::cloth_owner_scope::Scope::capture(identity.address, anchor, base, read);
    if scope.player != identity.address {
        return None;
    }
    let mut result = Vec::new();
    for route in scope
        .routes
        .iter()
        .copied()
        .filter(|route| route.owner != 0)
    {
        if !scope.route_is_current(route, base, read) {
            return None;
        }
        let begin = read::<usize>(route.core.checked_add(0x28)?)?;
        let end = read::<usize>(route.core.checked_add(0x30)?)?;
        let bytes = end.checked_sub(begin)?;
        if bytes % 8 != 0 || bytes / 8 > 32 {
            return None;
        }
        for i in 0..bytes / 8 {
            let holder = read::<usize>(begin.checked_add(i * 8)?)?;
            let root = read::<usize>(holder)?;
            let (children, count) = bounded_hk_array_span(root, 0x40, 0x48, 8, 256)?;
            for j in 0..count {
                let child = read::<usize>(children.checked_add(j * 8)?)?;
                let sim = read::<usize>(child.checked_add(0x18)?)?;
                if sim != 0 && !result.contains(&sim) {
                    result.push(sim);
                }
            }
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body_scale_port::test_characters::{BASE, Character, Environment};

    fn simulation(particles: usize) -> Vec<usize> {
        let mut sim = vec![0usize; 0x200 / 8];
        sim[0] = BASE + ER_HCL_SIM_CLOTH_DATA_VTABLE_RVA;
        sim[0x40 / 8] = particles;
        sim[0x48 / 8] = 1;
        sim[0xA8 / 8] = u32::MAX as usize;
        sim
    }

    #[test]
    fn restoring_dimensions_reuses_prepared_resource_spans() {
        let _environment = Environment::new();
        let mut particles = vec![[0.0f32, 0.0, 2.0, 0.0]; 8192];
        let mut independent = vec![[0.0f32, 0.0, 4.0, 0.0]; 8192];
        let mut a = simulation(particles.as_mut_ptr() as usize);
        let mut b = simulation(independent.as_mut_ptr() as usize);
        a[0x48 / 8] = particles.len();
        b[0x48 / 8] = independent.len();
        let chars = [Character::new(9520, 1, 1.0), Character::new(2010, 2, 1.0)];
        chars[0].attach_cloth(a.as_ptr() as usize);
        chars[1].attach_cloth(b.as_ptr() as usize);
        let mut pool = Pool::default();
        let prepared = pool.prepare(&[
            Consumer {
                identity: chars[0].identity(),
                scale: 1.0,
            },
            Consumer {
                identity: chars[1].identity(),
                scale: 0.6,
            },
        ]);
        assert!(prepared.rejected.is_empty());
        let mut baseline =
            with_prepared(&prepared, || capture(a.as_ptr() as usize, || None)).unwrap();
        baseline.apply(0.6).unwrap();
        let before = SPAN_BUILDS.with(std::cell::Cell::get);
        let started = std::time::Instant::now();
        for _ in 0..40 {
            with_prepared(&prepared, || baseline.apply_budgeted(1.0, 4096)).unwrap();
        }
        let builds = SPAN_BUILDS.with(std::cell::Cell::get) - before;
        println!(
            "RESTORE-SPANS passes=40 rebuilds={builds} elapsed_us={}",
            started.elapsed().as_micros()
        );
        assert!(particles.iter().all(|p| p[2] == 2.0));
        assert_eq!(
            builds, 0,
            "full cloth field scans repeat during restoration"
        );
    }

    #[test]
    fn shared_arrays_keep_originals_for_late_consumers_and_isolate_conflicts() {
        let _environment = Environment::new();
        let mut particles = [0.0f32, 0.0, 2.0, 0.0];
        let mut independent = [0.0f32, 0.0, 4.0, 0.0];
        let sims = [
            simulation(particles.as_mut_ptr() as usize),
            simulation(particles.as_mut_ptr() as usize),
            simulation(independent.as_mut_ptr() as usize),
        ];
        let chars = [
            Character::new(2010, 1, 1.0),
            Character::new(2010, 2, 1.0),
            Character::new(3250, 3, 1.0),
        ];
        for i in 0..3 {
            chars[i].attach_cloth(sims[i].as_ptr() as usize);
        }
        let consumer = |index: usize, scale| Consumer {
            identity: chars[index].identity(),
            scale,
        };
        let root = |index: usize| sims[index].as_ptr() as usize;
        let mut pool = Pool::default();
        let first = pool.prepare(&[consumer(0, 0.5)]);
        assert!(first.rejected.is_empty(), "{:?}", first.rejected);
        let mut a = with_prepared(&first, || capture(root(0), || None)).unwrap();
        with_prepared(&first, || a.apply(0.5)).unwrap();
        assert_eq!(particles[2], 1.0);
        // B is newly loaded after A has already modified their shared array.
        let second = pool.prepare(&[consumer(0, 0.5), consumer(1, 0.5), consumer(2, 3.0)]);
        assert!(second.rejected.is_empty(), "{:?}", second.rejected);
        let mut b = with_prepared(&second, || capture(root(1), || None)).unwrap();
        let mut c = with_prepared(&second, || capture(root(2), || None)).unwrap();
        with_prepared(&second, || {
            b.apply(0.5).unwrap();
            c.apply(3.0).unwrap();
        });
        assert_eq!(particles[2], 1.0, "late B must not compound A's scale");
        assert_eq!(independent[2], 12.0);
        let conflict = pool.prepare(&[consumer(0, 0.5), consumer(1, 3.0), consumer(2, 3.0)]);
        assert!(conflict.rejected.contains_key(&chars[0].address));
        assert!(conflict.rejected.contains_key(&chars[1].address));
        assert!(!conflict.rejected.contains_key(&chars[2].address));
        with_prepared(&conflict, || {
            a.apply(1.0).unwrap();
            b.apply(1.0).unwrap();
        });
        assert_eq!(particles[2], 2.0);
        assert_eq!(
            independent[2], 12.0,
            "unrelated character continues scaling"
        );
        let same = pool.prepare(&[consumer(0, 0.5), consumer(1, 0.5)]);
        with_prepared(&same, || {
            a.apply(0.5).unwrap();
            b.apply(0.5).unwrap();
        });
        let departed = pool.prepare(&[consumer(1, 0.5)]);
        with_prepared(&departed, || a.apply(1.0)).unwrap();
        assert_eq!(
            particles[2], 1.0,
            "departing root cannot restore B's overlapping array"
        );
        let neutral = pool.prepare(&[consumer(1, 1.0)]);
        with_prepared(&neutral, || b.apply(1.0)).unwrap();
        assert_eq!(particles[2], 2.0);
    }

    #[test]
    fn numeric_preflight_uses_original_shared_baselines_and_isolates_other_units() {
        let _environment = Environment::new();
        let mut particles = [0.0f32, 0.0, 2.0, 0.0];
        let mut independent = [0.0f32, 0.0, 4.0, 0.0];
        let sims = [
            simulation(particles.as_mut_ptr() as usize),
            simulation(particles.as_mut_ptr() as usize),
            simulation(independent.as_mut_ptr() as usize),
        ];
        let chars = [
            Character::new(9520, 1, 1.0),
            Character::new(9520, 2, 1.0),
            Character::new(8250, 3, 1.0),
        ];
        for i in 0..3 {
            chars[i].attach_cloth(sims[i].as_ptr() as usize);
        }
        let consumer = |i: usize, scale| Consumer {
            identity: chars[i].identity(),
            scale,
        };
        let root = |i: usize| sims[i].as_ptr() as usize;
        let mut pool = Pool::default();
        let first = pool.prepare(&[consumer(0, 0.1)]);
        let mut a = with_prepared(&first, || capture(root(0), || None)).unwrap();
        with_prepared(&first, || a.apply(0.1)).unwrap();
        // Root B is first captured from A's shrunken mutable array. Its numeric
        // envelope must be rebuilt after recovering A's immutable original.
        let large = pool.prepare(&[
            consumer(0, f32::MAX),
            consumer(1, f32::MAX),
            consumer(2, 10.0),
        ]);
        assert!(large.rejected.contains_key(&chars[0].address));
        assert!(large.rejected.contains_key(&chars[1].address));
        assert!(!large.rejected.contains_key(&chars[2].address));
        assert!(!large.objects[&root(1)].supports_scale(f32::MAX));
        assert_eq!(particles[2], 0.2, "preflight must not write");
        let valid = pool.prepare(&[consumer(0, 10.0), consumer(1, 10.0), consumer(2, 0.25)]);
        assert!(valid.rejected.is_empty());
        let mut b = with_prepared(&valid, || capture(root(1), || None)).unwrap();
        with_prepared(&valid, || b.apply(10.0)).unwrap();
        assert_eq!(particles[2], 20.0);
    }

    #[test]
    fn malformed_and_recycled_cloth_is_rejected_without_writing() {
        let _environment = Environment::new();
        let mut particles = [0.0f32, 0.0, 2.0, 0.0];
        let mut sim = simulation(particles.as_mut_ptr() as usize);
        let chr = Character::new(3250, 1, 1.0);
        chr.attach_cloth(sim.as_ptr() as usize);
        let mut pool = Pool::default();
        let original = pool.prepare(&[Consumer {
            identity: chr.identity(),
            scale: 0.5,
        }]);
        let mut captured =
            with_prepared(&original, || capture(sim.as_ptr() as usize, || None)).unwrap();
        sim[0x48 / 8] = usize::MAX;
        assert!(captured.apply(0.5).is_none());
        let invalid = pool.prepare(&[Consumer {
            identity: chr.identity(),
            scale: 0.5,
        }]);
        assert!(invalid.rejected.contains_key(&chr.address));
        assert_eq!(particles[2], 2.0);
        assert!(with_prepared(&invalid, || capture(sim.as_ptr() as usize, || None)).is_none());
    }
}
