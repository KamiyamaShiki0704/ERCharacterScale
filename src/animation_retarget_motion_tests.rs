use super::*;
use windows::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
    VirtualAlloc, VirtualFree, VirtualProtect,
};

extern "win64" fn clobber_volatile_vectors(_registers: *mut Registers, translation: *mut [f32; 4]) {
    unsafe {
        for v in &mut (&mut *translation)[..3] {
            *v *= 0.5;
        }
        std::arch::asm!("pcmpeqd xmm4, xmm4", "pcmpeqd xmm5, xmm5", out("xmm4") _, out("xmm5") _, options(nostack));
    }
}

#[test]
fn executed_motion_hook_changes_only_translation_xyz() {
    struct Page(*mut std::ffi::c_void);
    impl Drop for Page {
        fn drop(&mut self) {
            unsafe {
                VirtualFree(self.0, 0, MEM_RELEASE).unwrap();
            }
        }
    }
    let page = Page(unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) });
    assert!(!page.0.is_null());
    // Load XMM4/5/7, invoke a real installed trampoline between the loads
    // and consumers, then store all lanes. This catches ABI corruption that
    // calling the Rust callback with a fake Registers structure cannot.
    let mut code: Vec<u8> = vec![
        0x48, 0x83, 0xec, 0x38, 0x0f, 0x11, 0x7c, 0x24, 0x20, 0x0f, 0x10, 0x21, 0x0f, 0x10, 0x69,
        0x10, 0x0f, 0x10, 0x79, 0x20,
    ];
    let offset = code.len();
    code.extend([0x90; 32]);
    code.extend([
        0x0f, 0x11, 0x21, 0x0f, 0x11, 0x69, 0x10, 0x0f, 0x11, 0x79, 0x20, 0x0f, 0x10, 0x7c, 0x24,
        0x20, 0x48, 0x83, 0xc4, 0x38, 0xc3,
    ]);
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), page.0.cast(), code.len());
    }
    let mut old = PAGE_PROTECTION_FLAGS::default();
    unsafe {
        VirtualProtect(page.0, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
    }
    let entry: unsafe extern "win64" fn(*mut [f32; 4]) = unsafe { std::mem::transmute(page.0) };
    let before = [
        [-3.0, 2.0, 11.0, 1.0],
        [0.0, -0.25, 0.5, 0.75],
        [1.42, 7.0, 8.0, 9.0],
    ];
    let hook = unsafe { install_at(page.0 as usize + offset, clobber_volatile_vectors) }.unwrap();
    let mut data = before;
    for _ in 0..100 {
        data = before;
        unsafe {
            entry(data.as_mut_ptr());
        }
    }
    drop(hook);
    assert_eq!(
        data[0], before[0],
        "live XMM4 changed across native trampoline"
    );
    assert_eq!(
        data[1], before[1],
        "live XMM5 changed across native trampoline"
    );
    assert_eq!(data[2], [0.71, 3.5, 4.0, 9.0]);
}
