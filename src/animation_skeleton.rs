//! Bounded, read-only ER 2018 TAG0 skeleton reader. Runs on the asset worker,
//! never on the animation thread. No game assets are embedded in the DLL.
use crate::{equipment_retarget::Bone, equipment_retarget_pose as pose};
use std::{io::Read, path::Path};

const MAX_ASSET: usize = 128 * 1024 * 1024;
fn u32_at(b: &[u8], p: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(p..p.checked_add(4)?)?.try_into().ok()?) as usize)
}
fn u64_at(b: &[u8], p: usize) -> Option<usize> {
    usize::try_from(u64::from_le_bytes(
        b.get(p..p.checked_add(8)?)?.try_into().ok()?,
    ))
    .ok()
}
fn chunk<'a>(b: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    let mut p = 0usize;
    while p < b.len() {
        let n = (u32::from_be_bytes(b.get(p..p + 4)?.try_into().ok()?) & 0x3fff_ffff) as usize;
        if n < 8 {
            return None;
        }
        let data = b.get(p + 8..p.checked_add(n)?)?;
        if b.get(p + 4..p + 8)? == name {
            return Some(data);
        }
        p += n;
    }
    None
}

struct Tag<'a> {
    data: &'a [u8],
    items: &'a [u8],
}
impl<'a> Tag<'a> {
    fn item(&self, index: usize, stride: usize) -> Option<(&'a [u8], usize)> {
        if index == 0 {
            return None;
        }
        let at = index.checked_mul(12)?;
        let offset = u32_at(self.items, at + 4)?;
        let count = u32_at(self.items, at + 8)?;
        if count > pose::LIMIT * 16 {
            return None;
        }
        Some((
            self.data
                .get(offset..offset.checked_add(count.checked_mul(stride)?)?)?,
            count,
        ))
    }
    fn string(&self, index: usize) -> Option<String> {
        let (s, n) = self.item(index, 1)?;
        if !(2..=256).contains(&n) || *s.last()? != 0 || s[..n - 1].contains(&0) {
            return None;
        }
        Some(std::str::from_utf8(&s[..n - 1]).ok()?.to_owned())
    }
    fn skeleton(&self, index: usize, expected: &str) -> Option<Vec<Bone>> {
        let (s, count) = self.item(index, 0x90)?;
        if count != 1 || self.string(u64_at(s, 0x18)?)? != expected {
            return None;
        }
        let (parents, n) = self.item(u64_at(s, 0x20)?, 2)?;
        let (names, nn) = self.item(u64_at(s, 0x30)?, 16)?;
        let (refs, nr) = self.item(u64_at(s, 0x40)?, 48)?;
        if n == 0 || n > pose::LIMIT || nn != n || nr != n {
            return None;
        }
        let mut result = Vec::with_capacity(n);
        for i in 0..n {
            let parent = i16::from_le_bytes(parents[i * 2..i * 2 + 2].try_into().ok()?);
            if parent < -1 || parent as usize == i || (parent >= 0 && parent as usize >= n) {
                return None;
            }
            result.push(Bone {
                name: self.string(u64_at(names, i * 16)?)?,
                parent: (parent >= 0).then_some(parent as usize),
                reference: pose::qs(&refs[i * 48..i * 48 + 48])?,
            });
        }
        // Plan validates duplicate names, cycles and reference transforms.
        crate::equipment_retarget::Plan::new(result.clone(), result.clone(), &[]).ok()?;
        Some(result)
    }
}

pub(crate) fn parse(bytes: &[u8], expected: &str) -> Option<Vec<Bone>> {
    let tag = chunk(bytes, b"TAG0")?;
    if chunk(tag, b"SDKV")? != b"20180100" {
        return None;
    }
    let data = chunk(tag, b"DATA")?;
    let items = chunk(chunk(tag, b"INDX")?, b"ITEM")?;
    if items.len() % 12 != 0 {
        return None;
    }
    let reader = Tag { data, items };
    let (root, _) = reader.item(1, 16)?;
    let (variants, n) = reader.item(u64_at(root, 0)?, 24)?;
    if n > 64 {
        return None;
    }
    let mut result = None;
    for v in variants.chunks_exact(24) {
        if reader.string(u64_at(v, 8)?)?.as_str() != "hkaAnimationContainer" {
            continue;
        }
        let (container, _) = reader.item(u64_at(v, 16)?, 0x68)?;
        let (skeletons, count) = reader.item(u64_at(container, 0x18)?, 8)?;
        if count > 32 {
            return None;
        }
        for s in skeletons.chunks_exact(8) {
            if let Some(bones) = reader.skeleton(u64_at(s, 0)?, expected) {
                if result.is_some() {
                    return None;
                } // Ambiguous source is not guessed.
                result = Some(bones);
            }
        }
    }
    result
}

pub(crate) fn source_name(name: &str) -> bool {
    name.len() == 5
        && name.starts_with('c')
        && name.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        && name != "c0000"
}

fn bounded_file(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > MAX_ASSET as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_ASSET as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_ASSET).then_some(bytes)
}

/// DFLT uses the linked Rust decoder; KRAK uses the game's shipped Oodle.
fn decompress(bytes: Vec<u8>, game_dir: &Path) -> Option<Vec<u8>> {
    if !bytes.starts_with(b"DCX\0") {
        return Some(bytes);
    }
    let codec = bytes.get(0x28..0x2c)?;
    if !matches!(codec, b"KRAK" | b"DFLT") || bytes.get(0x44..0x48)? != b"DCA\0" {
        return None;
    }
    let be = |p| Some(u32::from_be_bytes(bytes.get(p..p + 4)?.try_into().ok()?) as usize);
    let size = be(0x1c)?;
    let compressed = be(0x20)?;
    if size == 0 || size > MAX_ASSET || compressed == 0 {
        return None;
    }
    let input = bytes.get(0x4c..0x4cusize.checked_add(compressed)?)?;
    if codec == b"DFLT" {
        // One spare byte detects a stream larger than its declared size. A
        // successful EOF/checksum and exact input/output sizes are mandatory.
        let mut output = vec![0; size.checked_add(1)?];
        let mut decoder = flate2::Decompress::new(true);
        let status = decoder
            .decompress(input, &mut output, flate2::FlushDecompress::Finish)
            .ok()?;
        if status != flate2::Status::StreamEnd
            || decoder.total_out() != size as u64
            || decoder.total_in() != compressed as u64
        {
            return None;
        }
        output.truncate(size);
        return Some(output);
    }
    use windows::{
        Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW},
        core::{PCSTR, PCWSTR},
    };
    let path: Vec<u16> = game_dir
        .join("oo2core_6_win64.dll")
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    use std::os::windows::ffi::OsStrExt;
    let module = unsafe { LoadLibraryW(PCWSTR(path.as_ptr())).ok()? };
    struct Library(windows::Win32::Foundation::HMODULE);
    impl Drop for Library {
        fn drop(&mut self) {
            unsafe {
                let _ = windows::Win32::Foundation::FreeLibrary(self.0);
            }
        }
    }
    let _library = Library(module);
    let address = unsafe { GetProcAddress(module, PCSTR(c"OodleLZ_Decompress".as_ptr().cast()))? };
    type Decode = unsafe extern "C" fn(
        *const u8,
        isize,
        *mut u8,
        isize,
        i32,
        i32,
        i32,
        usize,
        isize,
        usize,
        usize,
        usize,
        isize,
        i32,
    ) -> isize;
    let decode: Decode = unsafe { std::mem::transmute(address) };
    let mut output = vec![0; size];
    let actual = unsafe {
        decode(
            input.as_ptr(),
            input.len() as isize,
            output.as_mut_ptr(),
            size as isize,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            3,
        )
    };
    (actual == size as isize).then_some(output)
}

fn binder_skeleton(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(..4)? != b"BND4" || *bytes.get(9)? != 0 {
        return None;
    }
    let raw = *bytes.get(0x31)?;
    let bits = *bytes.get(10)? == 0;
    let format = if bits || raw & 1 != 0 && raw & 0x80 == 0 {
        raw
    } else {
        raw.reverse_bits()
    };
    if format & 1 != 0 || format & 0xc == 0 {
        return None;
    }
    let stride = u64_at(bytes, 0x20)?;
    let count = u32_at(bytes, 0xc)?;
    if count > 32768 || !(24..=48).contains(&stride) {
        return None;
    }
    let headers = bytes.get(0x40..0x40usize.checked_add(count.checked_mul(stride)?)?)?;
    let mut result = None;
    for h in headers.chunks_exact(stride) {
        let size = u64_at(h, 8)?;
        let offset_at = if format & 0x20 != 0 { 24 } else { 16 };
        let (offset, mut name_at) = if format & 0x10 != 0 {
            (u64_at(h, offset_at)?, offset_at + 8)
        } else {
            (u32_at(h, offset_at)?, offset_at + 4)
        };
        if format & 2 != 0 {
            name_at += 4;
        }
        let at = u32_at(h, name_at)?;
        let mut name = String::new();
        for i in 0..1024 {
            let c = if bytes[0x30] != 0 {
                u16::from_le_bytes(bytes.get(at + i * 2..at + i * 2 + 2)?.try_into().ok()?)
            } else {
                *bytes.get(at + i)? as u16
            };
            if c == 0 {
                break;
            }
            name.push(char::from_u32(c as u32)?);
        }
        let leaf = name.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
        if leaf == "skeleton.hkx" || leaf.starts_with("skeleton_") && leaf.ends_with(".hkx") {
            if result.is_some() {
                return None;
            }
            result = Some(bytes.get(offset..offset.checked_add(size)?)?);
        }
    }
    result
}

pub(crate) fn load(name: &str, skeleton_dir: &Path, game_dir: &Path) -> Option<Vec<Bone>> {
    if !source_name(name) {
        return None;
    }
    // An explicitly supplied SK is authoritative, including invalid input.
    let loose = skeleton_dir.join(format!("{name}.hkx"));
    if loose.exists() {
        return parse(&bounded_file(&loose)?, name);
    }
    let pack = skeleton_dir.join(format!("{name}.anibnd.dcx"));
    let pack = if pack.exists() {
        pack
    } else {
        game_dir.join("chr").join(format!("{name}.anibnd.dcx"))
    };
    let bytes = decompress(bounded_file(&pack)?, game_dir)?;
    let sk = decompress(binder_skeleton(&bytes)?.to_vec(), game_dir)?;
    parse(&sk, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dflt_source_packages_decode_without_oodle_and_reject_bad_streams() {
        use std::io::Write;
        let original = b"BND4 source skeleton payload";
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(original).unwrap();
        let data = encoder.finish().unwrap();
        let mut dcx = vec![0; 0x4c];
        dcx[..4].copy_from_slice(b"DCX\0");
        dcx[0x1c..0x20].copy_from_slice(&(original.len() as u32).to_be_bytes());
        dcx[0x20..0x24].copy_from_slice(&(data.len() as u32).to_be_bytes());
        dcx[0x28..0x2c].copy_from_slice(b"DFLT");
        dcx[0x44..0x48].copy_from_slice(b"DCA\0");
        dcx.extend_from_slice(&data);
        let missing = Path::new("no-game-runtime-required-for-dflt");
        assert_eq!(decompress(dcx.clone(), missing).unwrap(), original);
        for size in [0, original.len() - 1, original.len() + 1, MAX_ASSET + 1] {
            let mut invalid = dcx.clone();
            invalid[0x1c..0x20].copy_from_slice(&(size as u32).to_be_bytes());
            assert!(decompress(invalid, missing).is_none());
        }
        let mut bad = dcx.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(decompress(bad, missing).is_none());
        let mut truncated = dcx.clone();
        truncated.pop();
        truncated[0x20..0x24].copy_from_slice(&((data.len() - 1) as u32).to_be_bytes());
        assert!(decompress(truncated, missing).is_none());
    }
    #[test]
    fn source_names_cannot_escape_asset_directory() {
        for s in [
            "c0000",
            "../c3010",
            "c3010.hkx",
            "Master",
            "C3010",
            "c30100",
            "c３０１０",
        ] {
            assert!(!source_name(s));
        }
        assert!(source_name("c3010"));
    }
    #[test]
    fn malformed_tag_is_rejected_without_panicking() {
        for n in 0..256 {
            assert!(parse(&vec![0; n], "c3010").is_none());
        }
        assert!(parse(b"\x00\x00\x00\x00TAG0", "c3010").is_none());
    }
    #[test]
    #[ignore = "requires private ER assets; set ERCS_TEST_GAME_DIR"]
    fn original_c3010_anibnd_loads_without_external_conversion() {
        let game = std::env::var("ERCS_TEST_GAME_DIR").unwrap();
        let bones = load(
            "c3010",
            Path::new(&game).join("missing-skeletons").as_path(),
            Path::new(&game),
        )
        .unwrap();
        assert_eq!(bones.len(), 122);
        assert_eq!(bones[0].name, "Master");
        assert!(bones.iter().any(|b| b.name == "L_Hand"));
    }

    #[test]
    #[ignore = "requires private source override; set ERCS_TEST_SKELETON_DIR and ERCS_TEST_GAME_DIR"]
    fn selected_c3010_source_override_loads() {
        let game = std::env::var("ERCS_TEST_GAME_DIR").unwrap();
        let source = std::env::var("ERCS_TEST_SKELETON_DIR").unwrap();
        let bones = load("c3010", Path::new(&source), Path::new(&game))
            .expect("selected c3010 source must load");
        println!("source_bones={} root={}", bones.len(), bones[0].name);
    }
}
