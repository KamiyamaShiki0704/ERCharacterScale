//! Preserve the rider's visual basis after the native parent-location update.
//! B43B00 imports the mount attachment and overwrites +70 without ChrCtrl scale.
//! The synchronous return seam runs before child model/cloth consumers.
use crate::{equipment_retarget_pose as pose, memory_query};
use ilhook::x64::{CallbackOption, HookFlags, Registers, hook_closure_retn};
use std::sync::atomic::{AtomicUsize, Ordering};

static LOCATION: AtomicUsize = AtomicUsize::new(0);
const TAIL: usize = 0xB43D07;
const GUARDS: &[(usize, &[u8])] = &[
    (
        0xB43B00,
        &[
            0x40, 0x53, 0x48, 0x81, 0xec, 0xd0, 0, 0, 0, 0x48, 0x8b, 0xd9, 0x48, 0x83, 0xc1, 0x48,
            0xe8, 0x3b, 0xd2, 0, 0,
        ],
    ),
    (
        0xB43C9D,
        &[
            0x0f, 0x28, 0, 0x0f, 0x29, 0x43, 0x70, 0x0f, 0x28, 0x48, 0x10, 0x0f, 0x29, 0x8b, 0x80,
            0, 0, 0,
        ],
    ),
    (TAIL, &[0x48, 0x81, 0xc4, 0xd0, 0, 0, 0, 0x5b, 0xc3]),
];

pub(crate) fn publish(location: usize) {
    LOCATION.store(location, Ordering::Release);
}

fn read(address: usize, out: &mut [u8]) -> bool {
    if !memory_query::accessible_span(address, out.len(), false) {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(address as *const u8, out.as_mut_ptr(), out.len());
    }
    true
}

fn scaled_basis(mut matrix: [f32; 16], scale: [f32; 3]) -> Option<[f32; 16]> {
    if !matrix.iter().all(|x| x.is_finite())
        || !scale.iter().all(|x| x.is_finite() && *x > 0.)
        || matrix[3] != 0.
        || matrix[7] != 0.
        || matrix[11] != 0.
        || matrix[15] != 1.
    {
        return None;
    }
    for (axis, requested) in scale.iter().enumerate() {
        let start = axis * 4;
        let length = matrix[start..start + 3]
            .iter()
            .map(|x| (*x as f64).powi(2))
            .sum::<f64>()
            .sqrt();
        if length <= 1e-12 || !length.is_finite() {
            return None;
        }
        for x in &mut matrix[start..start + 3] {
            *x = (*x as f64 * *requested as f64 / length) as f32;
        }
    }
    matrix.iter().all(|x| x.is_finite()).then_some(matrix)
}

fn prepare(
    read: &impl Fn(usize, &mut [u8]) -> bool,
    location: usize,
    base: usize,
) -> Option<[f32; 16]> {
    if pose::pointer(read, location)? != base + 0x2B6EC58 {
        return None;
    }
    let player = pose::pointer(read, location.checked_add(0xb0)?)?;
    let world = pose::pointer(read, base + 0x3D69FF8)?;
    if pose::pointer(read, world.checked_add(0x1e508)?)? != player {
        return None;
    }
    let ctrl = pose::pointer(read, player.checked_add(0x58)?)?;
    if pose::pointer(read, ctrl.checked_add(0x10)?)? != player
        || pose::pointer(read, ctrl + 0x2f8)? != location
    {
        return None;
    }
    let parent_link = pose::pointer(read, location + 0x50)?;
    let parent = pose::pointer(read, parent_link)?;
    if parent == 0 || parent == location {
        return None;
    }
    let modules = pose::pointer(read, player + 0x190)?;
    let ride = pose::pointer(read, modules.checked_add(0xe8)?)?;
    if pose::pointer(read, ride.checked_add(8)?)? != player {
        return None;
    }
    let node = pose::pointer(read, ride + 0x10)?;
    let state = i32::from_le_bytes(pose::bytes(read, node.checked_add(0x50)?)?);
    // Include the native mount/dismount transitions, but exclude cutscene and
    // other parent attachments. The detached path keeps its original matrix.
    if !matches!(state, 3 | 5 | 7) {
        return None;
    }
    let scale_bytes: [u8; 12] = pose::bytes(read, ctrl + 0x2d4)?;
    let scale = std::array::from_fn(|i| {
        f32::from_le_bytes(scale_bytes[i * 4..i * 4 + 4].try_into().unwrap())
    });
    let matrix_bytes: [u8; 64] = pose::bytes(read, location + 0x70)?;
    let matrix = std::array::from_fn(|i| {
        f32::from_le_bytes(matrix_bytes[i * 4..i * 4 + 4].try_into().unwrap())
    });
    scaled_basis(matrix, scale)
}

fn correct(location: usize, base: usize) {
    if location == 0 || location != LOCATION.load(Ordering::Acquire) {
        return;
    }
    memory_query::scoped(|| {
        let Some(matrix) = prepare(&read, location, base) else {
            return;
        };
        if LOCATION.load(Ordering::Acquire) == location
            && memory_query::accessible_span(location + 0x70, 48, true)
        {
            unsafe {
                std::ptr::copy_nonoverlapping(matrix.as_ptr(), (location + 0x70) as *mut f32, 12);
            }
        }
    });
}

// Entry detour instead of patching the short epilogue: a native branch jumps
// directly into that epilogue. Preserve the complete original control flow.
unsafe fn install_at(
    address: usize,
    base: usize,
) -> Result<ilhook::x64::ClosureHookPoint<'static>, ilhook::HookError> {
    unsafe {
        hook_closure_retn(
            address,
            move |r: *mut Registers, original| {
                let location = (*r).rcx as usize;
                let native: unsafe extern "win64" fn(usize) -> usize =
                    std::mem::transmute(original);
                let result = native(location);
                correct(location, base);
                result
            },
            CallbackOption::None,
            HookFlags::empty(),
        )
    }
}

pub(crate) fn install(base: usize) -> bool {
    if !memory_query::scoped(|| {
        GUARDS.iter().all(|(rva, bytes)| {
            let mut actual = vec![0; bytes.len()];
            read(base + rva, &mut actual) && actual == *bytes
        })
    }) {
        return false;
    }
    let hook = unsafe { install_at(base + 0xB43B00, base) };
    match hook {
        Ok(hook) => {
            std::mem::forget(hook);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        location: Box<[u8; 0x110]>,
        player: Box<[u8; 0x200]>,
        ctrl: Box<[u8; 0x380]>,
        modules: Box<[u8; 0x100]>,
        ride: Box<[u8; 0x1e0]>,
        node: Box<[u8; 0x60]>,
        world: Box<[u8; 0x1e510]>,
        global: Box<usize>,
        link: Box<usize>,
    }
    fn put<T: Copy>(buffer: &mut [u8], offset: usize, value: T) {
        assert!(offset + std::mem::size_of::<T>() <= buffer.len());
        unsafe {
            (buffer.as_mut_ptr().add(offset) as *mut T).write_unaligned(value);
        }
    }
    impl Fixture {
        fn new() -> Self {
            let mut f = Self {
                location: Box::new([0; 0x110]),
                player: Box::new([0; 0x200]),
                ctrl: Box::new([0; 0x380]),
                modules: Box::new([0; 0x100]),
                ride: Box::new([0; 0x1e0]),
                node: Box::new([0; 0x60]),
                world: Box::new([0; 0x1e510]),
                global: Box::new(0),
                link: Box::new(0),
            };
            let player = f.player.as_ptr() as usize;
            let loc = f.location.as_ptr() as usize;
            *f.global = f.world.as_ptr() as usize;
            *f.link = 0x123456;
            let base = f.base();
            put(&mut f.location[..], 0, base + 0x2B6EC58);
            put(&mut f.location[..], 0xb0, player);
            put(&mut f.location[..], 0x50, &*f.link as *const usize as usize);
            put(&mut f.location[..], 0x70, ROOT);
            put(&mut f.location[..], 0xc0, ROOT);
            put(&mut f.player[..], 0x58, f.ctrl.as_ptr() as usize);
            put(&mut f.player[..], 0x190, f.modules.as_ptr() as usize);
            put(&mut f.ctrl[..], 0x10, player);
            put(&mut f.ctrl[..], 0x2f8, loc);
            put(&mut f.ctrl[..], 0x2d4, [0.55f32; 3]);
            put(&mut f.modules[..], 0xe8, f.ride.as_ptr() as usize);
            put(&mut f.ride[..], 8, player);
            put(&mut f.ride[..], 0x10, f.node.as_ptr() as usize);
            put(&mut f.node[..], 0x50, 5i32);
            put(&mut f.world[..], 0x1e508, player);
            f
        }
        fn base(&self) -> usize {
            &*self.global as *const usize as usize - 0x3D69FF8
        }
        fn result(&self) -> Option<[f32; 16]> {
            memory_query::scoped(|| prepare(&read, self.location.as_ptr() as usize, self.base()))
        }
    }
    #[test]
    fn mounted_ownership_transitions_and_repeated_updates() {
        let mut f = Fixture::new();
        for state in [3, 5, 7] {
            put(&mut f.node[..], 0x50, state);
            let after = f.result().unwrap();
            assert!((after[0] - ROOT[0] * 0.55).abs() < 2e-6);
            assert_eq!(after[12..], ROOT[12..]);
            put(&mut f.location[..], 0x70, after);
            for _ in 0..100 {
                let next = f.result().unwrap();
                assert!(next.iter().zip(after).all(|(a, b)| (a - b).abs() < 1e-6));
                put(&mut f.location[..], 0x70, next);
            }
        }
        put(&mut f.ctrl[..], 0x2d4, [1.0f32; 3]);
        assert!((f.result().unwrap()[0] - ROOT[0]).abs() < 3e-6);
        put(&mut f.node[..], 0x50, 0i32);
        assert!(f.result().is_none());
        put(&mut f.node[..], 0x50, 5i32);
        put(&mut f.world[..], 0x1e508, 0usize);
        assert!(f.result().is_none());
        put(&mut f.world[..], 0x1e508, f.player.as_ptr() as usize);
        put(&mut f.ctrl[..], 0x2f8, 0usize);
        assert!(f.result().is_none());
    }
    #[test]
    fn malformed_roots_are_rejected_and_large_valid_scales_are_preserved() {
        for scale in [[0.; 3], [-1.; 3], [f32::NAN; 3], [f32::INFINITY; 3]] {
            assert!(scaled_basis(ROOT, scale).is_none());
        }
        let mut bad = ROOT;
        bad[0] = f32::NAN;
        assert!(scaled_basis(bad, [0.55; 3]).is_none());
        bad = ROOT;
        bad[..3].fill(0.);
        assert!(scaled_basis(bad, [0.55; 3]).is_none());
        let large = scaled_basis(ROOT, [100.; 3]).unwrap();
        assert_eq!(large[12..], ROOT[12..]);
        for scale in [0.55, 1., 2.5] {
            let out = scaled_basis(ROOT, [scale; 3]).unwrap();
            assert!((out[..3].iter().map(|x| x * x).sum::<f32>().sqrt() - scale).abs() < 1e-6);
        }
    }
    #[test]
    fn executed_parent_update_hook_corrects_root_after_native_overwrite() {
        use windows::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_PROTECTION_FLAGS,
            PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
        };
        struct Page(*mut std::ffi::c_void);
        impl Drop for Page {
            fn drop(&mut self) {
                unsafe {
                    VirtualFree(self.0, 0, MEM_RELEASE).unwrap();
                }
            }
        }
        let f = Fixture::new();
        let page =
            Page(unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) });
        assert!(!page.0.is_null());
        // Same prologue/epilogue and one-argument ABI as B43B00. The body copies
        // a native unit basis on every call, then the actual detour repairs it.
        let mut code = vec![
            0x40, 0x53, 0x48, 0x81, 0xec, 0xd0, 0, 0, 0, 0x48, 0x8b, 0xd9,
        ];
        code.extend([0x90; 16]);
        for (src, dst) in [(0xc0u32, 0x70u32), (0xd0, 0x80), (0xe0, 0x90), (0xf0, 0xa0)] {
            code.extend([0x0f, 0x10, 0x83]);
            code.extend(src.to_le_bytes());
            code.extend([0x0f, 0x11, 0x83]);
            code.extend(dst.to_le_bytes());
        }
        code.extend([
            0xb8, 0x34, 0x12, 0, 0, 0x48, 0x81, 0xc4, 0xd0, 0, 0, 0, 0x5b, 0xc3,
        ]);
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), page.0.cast(), code.len());
        }
        let mut old = PAGE_PROTECTION_FLAGS::default();
        unsafe {
            VirtualProtect(page.0, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
        }
        let native: unsafe extern "win64" fn(usize) -> usize =
            unsafe { std::mem::transmute(page.0) };
        let loc = f.location.as_ptr() as usize;
        let hook = unsafe { install_at(page.0 as usize, f.base()) }.unwrap();
        publish(loc);
        for _ in 0..100 {
            assert_eq!(unsafe { native(loc) }, 0x1234);
            let matrix = unsafe { ((loc + 0x70) as *const [f32; 16]).read_unaligned() };
            assert!((matrix[0] - ROOT[0] * 0.55).abs() < 2e-6);
            assert_eq!(matrix[12..], ROOT[12..]);
        }
        publish(0);
        unsafe {
            native(loc);
        }
        assert_eq!(
            unsafe { ((loc + 0x70) as *const [f32; 16]).read_unaligned() },
            ROOT
        );
        drop(hook);
    }
    // Captured from PID 23652 while mounted: controller 0.55, final location 1.
    const ROOT: [f32; 16] = [
        -0.9887858,
        -0.00044898308,
        0.14932437,
        0.,
        -0.003950118,
        0.9997241,
        -0.023150954,
        0.,
        -0.14927286,
        -0.023481118,
        -0.988515,
        0.,
        5.531307,
        6.400741,
        -11.286057,
        1.,
    ];
    #[test]
    fn captured_mount_root_retains_requested_basis_and_attachment_position() {
        let after = scaled_basis(ROOT, [0.55; 3]).unwrap();
        for i in 0..3 {
            let length = after[i * 4..i * 4 + 3]
                .iter()
                .map(|x| x * x)
                .sum::<f32>()
                .sqrt();
            assert!((length - 0.55).abs() < 1e-6, "rider scale lost: {length}");
            assert_eq!(after[i * 4 + 3], ROOT[i * 4 + 3]);
        }
        assert_eq!(after[12..], ROOT[12..]);
    }
}
