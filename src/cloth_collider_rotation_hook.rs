//! Original Simulate -> SetTransform velocity seam, WW2.7.1.0 only.
//! Engine inputs are never edited; owned aligned copies live across one call.
use crate::cloth_collider_rotation::{Transform, rigid_pair};
use crate::{body_scale_port, cloth_diagnostic, log, memory_query};
use ilhook::x64::{CallbackOption, HookFlags, Registers, hook_closure_retn};
use std::sync::atomic::{AtomicBool, Ordering};

const ENTRY: usize = 0x15DF650;
const RETURN: usize = 0x15E82CF;
const SEAMS: &[(usize, &[u8])] = &[
    (
        0x15DF650,
        &[
            0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x81, 0xec, 0x80, 0, 0, 0, 0x41, 0x0f, 0x28,
            0,
        ],
    ),
    (
        0x15E81B0,
        &[
            0x48, 0x8b, 0x86, 0x68, 0x01, 0, 0, 0x48, 0x8d, 0x94, 0x24, 0x80, 0, 0, 0, 0xc6, 0x44,
            0x24, 0x20, 0,
        ],
    ),
    (
        0x15E81C4,
        &[
            0x4d, 0x8b, 0x0c, 0x07, 0x48, 0x8b, 0x85, 0xb0, 0, 0, 0, 0x4d, 0x8d, 0x41, 0x20,
        ],
    ),
    (0x15E824A, &[0x49, 0x8b, 0xc9]),
    (0x15E82A7, &[0x41, 0x0f, 0x28, 0xdc]),
    (
        0x15E82CA,
        &[
            0xe8, 0x81, 0x73, 0xff, 0xff, 0xff, 0xc7, 0x49, 0x83, 0xc7, 0x08, 0x49, 0x83, 0xc6,
            0x04,
        ],
    ),
];
static APPLIED_LOGGED: AtomicBool = AtomicBool::new(false);
static REJECTED_LOGGED: AtomicBool = AtomicBool::new(false);
type Native = unsafe extern "C" fn(usize, usize, usize, f32, u8);

#[derive(Clone, Copy)]
pub(crate) struct Witness {
    pub collider: usize,
    pub dt_bits: u32,
    pub scale_bits: u32,
    pub target: [u32; 16],
    pub previous: [u32; 16],
    pub passed_target: [u32; 16],
    pub passed_previous: [u32; 16],
    pub applied: bool,
    pub output: Option<[u32; 8]>,
}

fn read_array<const N: usize>(address: usize) -> Option<[f32; N]> {
    memory_query::accessible_region(address, N.checked_mul(4)?, false)?;
    Some(unsafe { (address as *const [f32; N]).read_unaligned() })
}

fn callsite(r: &Registers, base: usize) -> bool {
    base.checked_add(RETURN) == Some(unsafe { r.get_stack(0) } as usize)
        && r.rcx.checked_add(0x20) == Some(r.r8)
        && unsafe { r.get_stack(5) } as u8 == 0
        && f32::from_bits(r.xmm3 as u32).is_finite()
        && (1e-5..=0.25).contains(&f32::from_bits(r.xmm3 as u32))
}

fn prepare(r: &Registers, base: usize) -> Option<([Transform; 2], Witness)> {
    if !callsite(r, base) {
        return None;
    }
    let child = r.rsi as usize;
    // RBP belongs to this child in the pinned Simulate caller, not another
    // invocation that happens to share a collider address.
    memory_query::accessible_region(child.checked_add(0x18)?, 8, false)?;
    if unsafe { ((child + 0x18) as *const usize).read_unaligned() } != r.rbp as usize {
        return None;
    }
    let scale = body_scale_port::collider_rotation_scope(child, r.rcx as usize)?;
    let target = read_array::<16>(r.rdx as usize)?;
    let previous = read_array::<16>(r.r8 as usize)?;
    let pair = rigid_pair(target, previous, scale);
    let (a, b) = pair.unwrap_or((Transform(target), Transform(previous)));
    // Revalidate ownership after reading inputs. This is still the original
    // synchronous engine phase, with no page/identity cache across native code.
    if body_scale_port::collider_rotation_scope(child, r.rcx as usize) != Some(scale) {
        return None;
    }
    Some((
        [a, b],
        Witness {
            collider: r.rcx as usize,
            dt_bits: r.xmm3 as u32,
            scale_bits: scale.to_bits(),
            target: target.map(f32::to_bits),
            previous: previous.map(f32::to_bits),
            passed_target: a.0.map(f32::to_bits),
            passed_previous: b.0.map(f32::to_bits),
            applied: pair.is_some(),
            output: None,
        },
    ))
}

fn invoke(r: &Registers, original: usize, prepared: Option<([Transform; 2], Witness)>) {
    let token = prepared
        .as_ref()
        .and_then(|(_, w)| cloth_diagnostic::collider_velocity_begin(r.rsi as usize, *w));
    let (target, previous) = match prepared.as_ref().filter(|(_, w)| w.applied) {
        Some((pair, _)) => (pair[0].0.as_ptr() as usize, pair[1].0.as_ptr() as usize),
        None => (r.rdx as usize, r.r8 as usize),
    };
    unsafe {
        std::mem::transmute::<usize, Native>(original)(
            r.rcx as usize,
            target,
            previous,
            f32::from_bits(r.xmm3 as u32),
            r.get_stack(5) as u8,
        );
    }
    // No recorder borrow or memory-query scope survives the native call.
    if let Some(token) = token {
        let output = (r.rcx as usize)
            .checked_add(0x60)
            .and_then(read_array::<8>)
            .map(|a| a.map(f32::to_bits));
        cloth_diagnostic::collider_velocity_end(r.rsi as usize, token, output);
    }
    if let Some((_, w)) = prepared {
        let flag = if w.applied {
            &APPLIED_LOGGED
        } else {
            &REJECTED_LOGGED
        };
        if !flag.swap(true, Ordering::AcqRel) {
            log::line(format_args!(
                "[ERPS-COLLIDER-ROTATION] applied={} scale={} native=15DF650 owned_copy=true",
                w.applied,
                f32::from_bits(w.scale_bits)
            ));
        }
    }
}

fn dispatch(base: usize, reg: *mut Registers, original: usize) -> usize {
    let r = unsafe { &*reg };
    let prepared = memory_query::scoped(|| prepare(r, base));
    invoke(r, original, prepared);
    0 // audited native signature is void; the caller never consumes RAX
}

pub(crate) fn install(base: usize) -> bool {
    if !memory_query::scoped(|| {
        SEAMS.iter().all(|&(rva, raw)| {
            base.checked_add(rva).is_some_and(|p| {
                memory_query::accessible_region(p, raw.len(), false).is_some()
                    && unsafe { std::slice::from_raw_parts(p as *const u8, raw.len()) == raw }
            })
        })
    }) {
        return false;
    }
    let result = unsafe {
        hook_closure_retn(
            base + ENTRY,
            move |r, original| dispatch(base, r, original),
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    let Ok(hook) = result else {
        return false;
    };
    let _ = Box::leak(Box::new(hook));
    log::line(format_args!(
        "[ERPS-COLLIDER-ROTATION] ready entry=15DF650 caller_return=15E82CF geometry_and_translation_retained=true"
    ));
    true
}

#[cfg(test)]
pub(crate) fn exercise_owned_route(base: usize, child: usize, sim: usize, collider: usize) {
    use std::cell::RefCell;
    #[derive(Debug)]
    struct Result {
        collider: usize,
        pointers: [usize; 2],
        values: [[f32; 16]; 2],
        dt: u32,
        flag: u8,
        queries: u64,
    }
    thread_local! { static RESULTS: RefCell<Vec<Result>> = const { RefCell::new(Vec::new()) }; }
    unsafe extern "C" fn native(c: usize, a: usize, b: usize, dt: f32, flag: u8) {
        let count = memory_query::query_count();
        let values = [read_array::<16>(a).unwrap(), read_array::<16>(b).unwrap()];
        RESULTS.with(|v| {
            v.borrow_mut().push(Result {
                collider: c,
                pointers: [a, b],
                values,
                dt: dt.to_bits(),
                flag,
                queries: memory_query::query_count() - count,
            })
        });
    }
    let unit = [
        1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 7., -3., 2., 1.,
    ];
    let mut target = Transform(unit);
    for i in [0, 5, 10] {
        target.0[i] = 0.5;
    }
    // The previous input aliases the real owned collider+20 as in the caller.
    // Save/restore this fixture region; the production callback only reads it.
    let prior = unsafe { ((collider + 0x20) as *const [f32; 16]).read_unaligned() };
    unsafe { ((collider + 0x20) as *mut [f32; 16]).write_unaligned(unit) };
    let mut stack = [0u64; 6];
    stack[0] = (base + RETURN) as u64;
    let mut r: Registers = unsafe { std::mem::zeroed() };
    r.rsp = stack.as_ptr() as u64;
    r.rsi = child as u64;
    r.rbp = sim as u64;
    r.rcx = collider as u64;
    r.rdx = target.0.as_ptr() as u64;
    r.r8 = (collider + 0x20) as u64;
    r.xmm3 = 0.02f32.to_bits() as u128;
    RESULTS.with(|v| v.borrow_mut().clear());
    dispatch(base, &mut r, native as *const () as usize);
    // A different caller, sim, dt or fifth argument must forward exact pointers.
    for mode in 0..4 {
        let before = (stack[0], stack[5], r.rbp, r.xmm3);
        match mode {
            0 => stack[0] += 1,
            1 => r.rbp += 8,
            2 => r.xmm3 = 0f32.to_bits() as u128,
            _ => stack[5] = 1,
        }
        dispatch(base, &mut r, native as *const () as usize);
        (stack[0], stack[5], r.rbp, r.xmm3) = before;
    }
    // A numerical guard rejects both matrices as a pair without changing either.
    target.0[5] = 0.3;
    r.rdx = std::hint::black_box(&target).0.as_ptr() as u64;
    dispatch(base, &mut r, native as *const () as usize);
    RESULTS.with(|v| {
        let results = v.borrow();
        assert_eq!(results.len(), 6);
        let good = &results[0];
        assert_eq!(good.values, [unit, unit]);
        assert_ne!(good.pointers, [r.rdx as usize, r.r8 as usize]);
        for result in results.iter() {
            assert_eq!(result.collider, collider);
            assert_eq!(result.queries, 2, "permission scope crossed native call");
        }
        for (i, result) in results[1..].iter().enumerate() {
            assert_eq!(result.pointers, [r.rdx as usize, r.r8 as usize]);
            assert_eq!(result.flag, if i == 3 { 1 } else { 0 });
            assert_eq!(
                result.dt,
                if i == 2 {
                    0f32.to_bits()
                } else {
                    0.02f32.to_bits()
                }
            );
        }
    });
    unsafe { ((collider + 0x20) as *mut [f32; 16]).write_unaligned(prior) };
}
