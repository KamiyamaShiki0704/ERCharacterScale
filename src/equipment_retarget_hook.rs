//! Mid-instruction hooks must preserve live volatile vector registers too.
use ilhook::x64::{CallbackOption, HookFlags, HookPoint, HookType, Hooker, Registers};

pub(super) unsafe fn install(
    address: usize,
    callback: extern "win64" fn(*mut Registers),
) -> Result<HookPoint, ilhook::HookError> {
    unsafe {
        Hooker::new(
            address,
            HookType::JmpBack(preserve_vectors),
            CallbackOption::None,
            callback as usize,
            HookFlags::empty(),
        )
        .hook()
    }
}

// ilhook 2.3 saves XMM0..3, not XMM4/5. Windows permits a normal Rust
// callback to clobber XMM4/5, but both may be live at an instruction hook.
// Enter directly (no closure wrapper before the saves). XMM6..15 are ABI
// nonvolatile. Provide full shadow space and preserve MXCSR as well.
#[unsafe(naked)]
unsafe extern "win64" fn preserve_vectors(_registers: *mut Registers, _callback: usize) {
    std::arch::naked_asm!(
        "sub rsp, 0x58",
        "movdqu [rsp + 0x20], xmm4",
        "movdqu [rsp + 0x30], xmm5",
        "stmxcsr [rsp + 0x40]",
        "call rdx",
        "ldmxcsr [rsp + 0x40]",
        "movdqu xmm4, [rsp + 0x20]",
        "movdqu xmm5, [rsp + 0x30]",
        "add rsp, 0x58",
        "ret",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_PROTECTION_FLAGS,
        PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
    };

    extern "win64" fn clobber_volatile_vectors(registers: *mut Registers) {
        unsafe {
            (*registers).xmm3 =
                ((*registers).xmm3 & !u128::from(u32::MAX)) | u128::from(0.85f32.to_bits());
            std::arch::asm!("pcmpeqd xmm4, xmm4", "pcmpeqd xmm5, xmm5", out("xmm4") _, out("xmm5") _, options(nostack));
        }
    }

    #[test]
    fn executed_mid_hook_preserves_live_camera_vectors() {
        struct Page(*mut std::ffi::c_void);
        impl Drop for Page {
            fn drop(&mut self) {
                unsafe {
                    VirtualFree(self.0, 0, MEM_RELEASE).unwrap();
                }
            }
        }
        let page =
            Page(unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) });
        assert!(!page.0.is_null());
        // Load XMM4/5/3, invoke a real installed trampoline between the loads
        // and consumers, then store all lanes. This catches ABI corruption that
        // calling the Rust callback with a fake Registers structure cannot.
        let mut code: Vec<u8> = vec![
            0x48, 0x83, 0xec, 0x28, 0x0f, 0x10, 0x21, 0x0f, 0x10, 0x69, 0x10, 0x0f, 0x10, 0x59,
            0x20,
        ];
        let offset = code.len();
        code.extend([0x90; 32]);
        code.extend([
            0x0f, 0x11, 0x21, 0x0f, 0x11, 0x69, 0x10, 0x0f, 0x11, 0x59, 0x20, 0x48, 0x83, 0xc4,
            0x28, 0xc3,
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
        let hook = unsafe { install(page.0 as usize + offset, clobber_volatile_vectors) }.unwrap();
        let mut data = before;
        for _ in 0..100 {
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
            "live camera XMM5 changed across native trampoline"
        );
        assert_eq!(data[2], [0.85, 7.0, 8.0, 9.0]);
    }
}
