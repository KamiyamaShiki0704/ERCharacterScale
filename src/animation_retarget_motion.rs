//! Scale a foreign clip's extracted translation before world composition and
//! native blending. Never scale the final blended player delta by a stale clip.
use super::*;
use ilhook::x64::{HookPoint, HookType, Hooker};

const RVA: usize = 0x14A3506;
const GUARD: &[u8] = &[
    0x0f, 0x28, 0x57, 0x10, 0x0f, 0x28, 0xe7, 0x4c, 0x8b, 0x84, 0x24, 0xd0, 0x01, 0, 0,
];

#[unsafe(no_mangle)]
pub static ERCS_ANIMATION_MOTION_DIAGNOSTICS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

pub(super) fn guard(base: usize) -> bool {
    let mut bytes = [0; GUARD.len()];
    read(base + RVA, &mut bytes) && bytes == GUARD
}

pub(super) unsafe fn install(base: usize) -> Result<HookPoint, ilhook::HookError> {
    unsafe { install_at(base + RVA, callback) }
}

unsafe fn install_at(
    address: usize,
    callback: extern "win64" fn(*mut Registers, *mut [f32; 4]),
) -> Result<HookPoint, ilhook::HookError> {
    unsafe {
        Hooker::new(
            address,
            HookType::JmpBack(bridge),
            CallbackOption::None,
            callback as usize,
            HookFlags::empty(),
        )
        .hook()
    }
}

// XMM7 contains clip-local extracted translation on both native time branches.
// ilhook's Registers exposes only XMM0..3. Explicitly expose XMM7 through a
// private stack copy, while preserving XMM4/5, MXCSR and all other live lanes.
#[unsafe(naked)]
unsafe extern "win64" fn bridge(_r: *mut Registers, _callback: usize) {
    std::arch::naked_asm!(
        "sub rsp, 0x68",
        "movdqu [rsp + 0x20], xmm4",
        "movdqu [rsp + 0x30], xmm5",
        "movdqu [rsp + 0x40], xmm7",
        "stmxcsr [rsp + 0x50]",
        "mov r8, rdx",
        "lea rdx, [rsp + 0x40]",
        "call r8",
        "ldmxcsr [rsp + 0x50]",
        "movdqu xmm4, [rsp + 0x20]",
        "movdqu xmm5, [rsp + 0x30]",
        "movdqu xmm7, [rsp + 0x40]",
        "add rsp, 0x68",
        "ret",
    );
}

pub(super) fn ratio(
    read: &impl Fn(usize, &mut [u8]) -> bool,
    clip: usize,
    skeleton: usize,
    cache: &Cache,
) -> Option<crate::equipment_retarget::MotionProfile> {
    if skeleton == 0
        || cache.generation != GENERATION.load(Ordering::Acquire)
        || pose::pointer(read, clip.checked_add(0xe8)?)? != 0
    {
        return None;
    }
    let binding = pose::pointer(read, clip.checked_add(0xf0)?)?;
    let entry = cache.bindings.get(&(skeleton, binding))?;
    if !entry.target.active.load(Ordering::Acquire)
        || entry.name == entry.target.name
        || entry.rejected
        || !animation_skeleton::source_name(&entry.name)
        || pose::bytes::<0x60>(read, binding)? != entry.header
        || entry.header[0x58] != 0
    {
        return None;
    }
    entry.plan.as_ref()?.motion
}

pub(super) extern "win64" fn callback(registers: *mut Registers, translation: *mut [f32; 4]) {
    let r = unsafe { &*registers };
    let _ = crate::memory_query::scoped(|| -> Option<()> {
        // Same target skeleton that this generator supplies to the track
        // scatter at 14A4036. These are verified native stack slots, not TLS
        // "last animation" state shared between characters or worker jobs.
        let skeleton = ptr((r.rsp as usize).checked_add(0x1b0)?)?;
        let target = target(skeleton)?;
        let generation = GENERATION.load(Ordering::Acquire);
        if ptr(r.rsp as usize + 0x1a0)? != 0 {
            return None;
        }
        let mode = i32::from_le_bytes(pose::bytes(&read, r.rsp as usize + 0x1c8)?);
        if matches!(mode, 1 | 2) {
            return None;
        }
        let profile =
            CACHE.with(|c| ratio(&read, r.rsi as usize, skeleton, &*c.try_borrow().ok()?))?;
        let before = unsafe { *translation };
        let after = profile.displacement(before)?;
        if !target.active.load(Ordering::Acquire)
            || generation != GENERATION.load(Ordering::Acquire)
        {
            return None;
        }
        unsafe {
            *translation = after;
        }
        ERCS_ANIMATION_MOTION_DIAGNOSTICS[0].fetch_add(1, Ordering::Relaxed);
        ERCS_ANIMATION_MOTION_DIAGNOSTICS[1].store(profile.leg_ratio.to_bits(), Ordering::Relaxed);
        Some(())
    });
}

#[cfg(test)]
#[path = "animation_retarget_motion_tests.rs"]
mod tests;
