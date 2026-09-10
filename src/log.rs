use std::{
    ffi::{CString, c_void},
    fmt::Arguments,
    fs::{File, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use windows::{
    Win32::{
        Foundation::HMODULE,
        System::{Diagnostics::Debug::OutputDebugStringA, LibraryLoader::GetModuleFileNameW},
    },
    core::PCSTR,
};

static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
static LOG_FILE: Mutex<Option<BufWriter<File>>> = Mutex::new(None);
pub fn sibling_path(name: &str) -> Option<PathBuf> {
    Some(LOG_PATH.get()?.with_file_name(name))
}
const MIRROR_TO_DEBUGGER: bool = false;

pub fn initialize(module: usize) {
    let path = log_path_from_module(module).unwrap_or_else(fallback_log_path);
    let _ = LOG_PATH.set(path.clone());

    if let Ok(mut sink) = LOG_FILE.lock()
        && let Ok(file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
    {
        let mut writer = BufWriter::with_capacity(64 * 1024, file);
        let _ = writeln!(writer, "=== player-scale-no-bone log session ===");
        let _ = writeln!(writer, "log_path={}", path.display());
        let _ = writer.flush();
        *sink = Some(writer);
    }

    line(format_args!("logger initialized"));
}

pub fn line(args: Arguments<'_>) {
    let mut text = args.to_string();
    text.push('\n');
    text.retain(|c| c != '\0');

    if MIRROR_TO_DEBUGGER {
        write_debug_string(&text);
    }
    write_log_file(&text);
}

pub(crate) fn log_path_from_module(module: usize) -> Option<PathBuf> {
    let mut buffer = [0u16; 32768];
    let len = unsafe { GetModuleFileNameW(Some(HMODULE(module as *mut c_void)), &mut buffer) };

    if len == 0 {
        return None;
    }

    let module_path = PathBuf::from(String::from_utf16_lossy(&buffer[..len as usize]));
    Some(module_path.with_file_name("player_scale_no_bone.log"))
}

fn fallback_log_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .map(|path| path.with_file_name("player_scale_no_bone.log"))
        .unwrap_or_else(|| PathBuf::from("player_scale_no_bone.log"))
}

fn write_debug_string(text: &str) {
    let Ok(cstr) = CString::new(text) else {
        return;
    };

    unsafe {
        OutputDebugStringA(PCSTR(cstr.as_ptr().cast()));
    }
}

fn write_log_file(text: &str) {
    let Ok(mut sink) = LOG_FILE.lock() else {
        return;
    };
    if let Some(writer) = sink.as_mut() {
        let _ = writer.write_all(text.as_bytes());
        // Keep runtime evidence durable even if the game exits abnormally.
        // The file handle remains open, so this is one flush rather than the
        // previous open/write/close cycle for every diagnostic line.
        let _ = writer.flush();
    }
}
