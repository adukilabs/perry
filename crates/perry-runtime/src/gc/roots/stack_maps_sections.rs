//! Locating the compact GC-map section in the running image, per object file
//! format: Mach-O on Apple platforms, ELF on Linux, PE on Windows, and a stub
//! everywhere else.
//!
//! Its own file for the same reason `stack_maps_verify.rs` is: the parent is at
//! the 2000-line cap, and pure code motion is the cheapest way to stay under it.

use super::decode::{read_u16, read_u32, read_u64};
use std::ffi::c_void;



/// ELF (#7173, #8075): the `.perry_gcmap` sections of every loaded image.
///
/// Linker-provided `__start_`/`__stop_` symbols would need weak linkage
/// (unstable in Rust) or `-rdynamic` (not guaranteed), so instead: read
/// each `dl_iterate_phdr` image's ELF section headers for `.perry_gcmap`
/// (`sh_addr`, `sh_size`) and add that image's `dlpi_addr` load bias. The
/// executable has an empty `dlpi_name`, for which `/proc/self/exe` is the
/// stable path. Reading only that first image is unsound when the runtime is
/// a provider and generated code lives in an app dylib: its live native roots
/// disappear from the collector exactly when a full collection evacuates.
pub(super) fn loaded_stack_map_sections() -> Result<Vec<&'static [u8]>, String> {
    crate::snap_funnel!(StackMaps);
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    #[repr(C)]
    struct DlPhdrInfo {
        dlpi_addr: usize,
        dlpi_name: *const std::os::raw::c_char,
        dlpi_phdr: *const ElfProgramHeader,
        dlpi_phnum: u16,
    }
    #[repr(C)]
    struct ElfProgramHeader {
        p_type: u32,
        _p_flags: u32,
        _p_offset: u64,
        p_vaddr: u64,
        _p_paddr: u64,
        _p_filesz: u64,
        p_memsz: u64,
        _p_align: u64,
    }
    struct SectionScan {
        sections: Vec<&'static [u8]>,
        unreadable_images: Vec<String>,
    }
    #[allow(clashing_extern_declarations)]
    unsafe extern "C" {
        fn dl_iterate_phdr(
            callback: unsafe extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> i32,
            data: *mut c_void,
        ) -> i32;
    }
    unsafe extern "C" fn collect(info: *mut DlPhdrInfo, _size: usize, data: *mut c_void) -> i32 {
        let Some(info) = info.as_ref() else {
            return 0;
        };
        let image_name = if info.dlpi_name.is_null() {
            &[][..]
        } else {
            CStr::from_ptr(info.dlpi_name).to_bytes()
        };
        // The kernel-provided vDSO has no backing file. It cannot contain
        // Perry-generated code, so it is the sole unreadable-image exception.
        if image_name == b"linux-vdso.so.1" || image_name == b"linux-gate.so.1" {
            return 0;
        }
        let path = if image_name.is_empty() {
            Path::new("/proc/self/exe")
        } else {
            Path::new(std::ffi::OsStr::from_bytes(image_name))
        };
        // Open and `pread` the three ranges the section walk needs, NOT
        // `fs::read` of the whole image.
        //
        // `/proc/self/exe` reports a size of 0, so `read_to_end` doubled its
        // buffer all the way to the image's real length: MEASURED at 42.8 ms
        // across 23 `read` calls, plus ~347 MB of transient RSS and the
        // `memmove` traffic of the doublings, for one claude-code startup —
        // to parse a few kilobytes of section headers. That is roughly half
        // the cost of GC-map initialization and none of it is the GC map.
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) => {
                let scan = &mut *data.cast::<SectionScan>();
                scan.unreadable_images
                    .push(format!("{} ({error})", path.display()));
                return 0;
            }
        };
        // A real I/O error is reported like an unopenable image, because the
        // index must never silently omit one image's native roots. A file
        // that is merely not a usable ELF64 — truncated, or not ELF at all —
        // is skipped, which is what a failed parse of a fully-read image did.
        let table = match read_elf_section_table(&file) {
            Ok(Some(table)) => table,
            Ok(None) => return 0,
            Err(error) => {
                let scan = &mut *data.cast::<SectionScan>();
                scan.unreadable_images
                    .push(format!("{} ({error})", path.display()));
                return 0;
            }
        };
        let Some((addr, size)) = elf_section_vaddr(&table, b".perry_gcmap") else {
            return 0;
        };
        let Some(start) = info.dlpi_addr.checked_add(addr) else {
            return 0;
        };
        let Some(section_end) = addr.checked_add(size) else {
            return 0;
        };
        const PT_LOAD: u32 = 1;
        let mapped = !info.dlpi_phdr.is_null()
            && std::slice::from_raw_parts(info.dlpi_phdr, usize::from(info.dlpi_phnum))
                .iter()
                .filter(|header| header.p_type == PT_LOAD)
                .any(|header| {
                    let Ok(segment_start) = usize::try_from(header.p_vaddr) else {
                        return false;
                    };
                    let Some(segment_end) = usize::try_from(header.p_memsz)
                        .ok()
                        .and_then(|size| segment_start.checked_add(size))
                    else {
                        return false;
                    };
                    addr >= segment_start && section_end <= segment_end
                });
        // The on-disk path can be replaced after dlopen. Validate its claimed
        // address against the loader's actual PT_LOAD ranges before turning
        // it into a slice, so a stale or hostile section table cannot make GC
        // initialization read outside the mapped image.
        if mapped && start != 0 && size != 0 {
            let scan = &mut *data.cast::<SectionScan>();
            scan.sections
                .push(std::slice::from_raw_parts(start as *const u8, size));
        }
        0
    }

    let mut scan = SectionScan {
        sections: Vec::new(),
        unreadable_images: Vec::new(),
    };
    unsafe {
        dl_iterate_phdr(collect, (&mut scan as *mut SectionScan).cast::<c_void>());
    }
    if scan.unreadable_images.is_empty() {
        Ok(scan.sections)
    } else {
        Err(format!(
            "unreadable loaded ELF image(s): {}",
            scan.unreadable_images.join(", ")
        ))
    }
}

/// The ELF64 file header, and one `Elf64_Shdr`.
///
/// A claimed `e_shentsize` below the latter would make the walk read
/// `sh_flags`/`sh_addr`/`sh_size` out of the entry it believes it is reading.
const ELF64_HEADER_BYTES: usize = 0x40;
const ELF64_SECTION_HEADER_BYTES: usize = 0x40;

/// The most this walk may read of one image's section-header table and
/// section-name table.
///
/// The walk needs exactly three ranges and nothing else, so these caps only
/// reject an image whose own header claims something absurd — `e_shnum` and
/// `e_shentsize` are `u16`s whose product can name 4 GB, and the name table
/// is sized by a `u64` field. A real toolchain produces a few kilobytes.
const MAX_SECTION_HEADER_TABLE_BYTES: usize = 8 << 20;
const MAX_SECTION_NAME_TABLE_BYTES: usize = 8 << 20;

/// The parts of an ELF64 image the `.perry_gcmap` lookup reads — and nothing
/// else, which is the entire point of this type.
struct ElfSectionTable {
    /// `shnum` entries of `shentsize` bytes, read from `e_shoff`.
    headers: Vec<u8>,
    shentsize: usize,
    shnum: usize,
    /// The section `e_shstrndx` names, bounded by ITS `sh_size` rather than by
    /// the end of the file. Reading the whole image made that distinction
    /// invisible: a `sh_name` pointing past the string table used to compare
    /// against whatever bytes followed it in the file.
    names: Vec<u8>,
}

impl ElfSectionTable {
    /// Bytes read from the image to build this — the property the walk exists
    /// to bound, so the regression test can pin it.
    #[cfg(test)]
    fn bytes_read(&self) -> usize {
        ELF64_HEADER_BYTES + self.headers.len() + self.names.len()
    }
}

/// Read an image's ELF header, section-header table and section-name table.
///
/// `Ok(None)` is "not an ELF64 image with a usable section table": a non-ELF
/// file, or one truncated before the ranges its own header names. The caller
/// skips those, exactly as it skipped an image whose fully-read bytes failed
/// to parse. `Err` is a genuine I/O error, which the caller reports like an
/// unopenable image so an unreadable image cannot pass for one with no roots.
fn read_elf_section_table(file: &std::fs::File) -> std::io::Result<Option<ElfSectionTable>> {
    use std::os::unix::fs::FileExt;

    /// `Ok(false)` for a short read at EOF — a truncated image, skipped —
    /// and `Err` for every other failure.
    fn read_at(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<bool> {
        match file.read_exact_at(buffer, offset) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
            Err(error) => Err(error),
        }
    }

    let mut header = [0u8; ELF64_HEADER_BYTES];
    if !read_at(file, &mut header, 0)? {
        return Ok(None);
    }
    if header.get(..4) != Some(&b"\x7fELF"[..]) || header[4] != 2 {
        return Ok(None); // not ELF64
    }
    let (Some(shoff), Some(shentsize), Some(shnum), Some(shstrndx)) = (
        read_u64(&header, 0x28),
        read_u16(&header, 0x3A),
        read_u16(&header, 0x3C),
        read_u16(&header, 0x3E),
    ) else {
        return Ok(None);
    };
    let shentsize = shentsize as usize;
    let shnum = shnum as usize;
    let shstrndx = shstrndx as usize;
    if shentsize < ELF64_SECTION_HEADER_BYTES || shnum == 0 || shstrndx >= shnum {
        return Ok(None);
    }
    // `e_shoff` of 0 means "no section-header table", in which case `e_shnum`
    // is 0 too and the check above already returned. A header claiming both
    // would have the ELF header itself read back as section entries.
    if shoff < ELF64_HEADER_BYTES as u64 {
        return Ok(None);
    }
    let Some(table_bytes) = shnum.checked_mul(shentsize) else {
        return Ok(None);
    };
    if table_bytes > MAX_SECTION_HEADER_TABLE_BYTES {
        return Ok(None);
    }
    let mut headers = vec![0u8; table_bytes];
    if !read_at(file, &mut headers, shoff)? {
        return Ok(None);
    }
    let Some(names_header) = shstrndx.checked_mul(shentsize) else {
        return Ok(None);
    };
    let (Some(names_offset), Some(names_size)) = (
        names_header
            .checked_add(0x18)
            .and_then(|at| read_u64(&headers, at)),
        names_header
            .checked_add(0x20)
            .and_then(|at| read_u64(&headers, at)),
    ) else {
        return Ok(None);
    };
    let Ok(names_size) = usize::try_from(names_size) else {
        return Ok(None);
    };
    if names_size > MAX_SECTION_NAME_TABLE_BYTES {
        return Ok(None);
    }
    let mut names = vec![0u8; names_size];
    if !read_at(file, &mut names, names_offset)? {
        return Ok(None);
    }
    Ok(Some(ElfSectionTable {
        headers,
        shentsize,
        shnum,
        names,
    }))
}

/// Minimal ELF64 section-header walk: returns (sh_addr, sh_size) for the
/// named section. Same defensive read style as the stack-map parser.
///
/// An entry whose `sh_name` does not resolve inside the name table is SKIPPED
/// rather than ending the search. Bounding the name table by its own `sh_size`
/// is what makes that distinction reachable at all — the previous whole-file
/// read left ~350 MB of slack after the table, so a short name near its end
/// always resolved — and ending the search there would drop `.perry_gcmap`
/// whenever it happened to sit after such an entry, which is precisely the
/// silent-missing-roots failure this file exists to avoid.
fn elf_section_vaddr(table: &ElfSectionTable, name: &[u8]) -> Option<(usize, usize)> {
    for index in 0..table.shnum {
        let Some(header) = index.checked_mul(table.shentsize) else {
            continue;
        };
        let Some(name_offset) = read_u32(&table.headers, header).map(|offset| offset as usize)
        else {
            continue;
        };
        let Some(name_end) = name_offset.checked_add(name.len()) else {
            continue;
        };
        let Some(candidate) = table.names.get(name_offset..name_end) else {
            continue;
        };
        // `unwrap_or(1)`: a name running to the very end of the table with no
        // NUL after it is not this section, it is a truncated table.
        let terminator = table.names.get(name_end).copied().unwrap_or(1);
        if candidate == name && terminator == 0 {
            // Only an SHF_ALLOC section has a runtime virtual address. Refuse
            // a file-only namesake before constructing a slice from sh_addr.
            const SHF_ALLOC: u64 = 0x2;
            if read_u64(&table.headers, header.checked_add(0x08)?)? & SHF_ALLOC == 0 {
                return None;
            }
            let addr = read_u64(&table.headers, header.checked_add(0x10)?)? as usize;
            let size = read_u64(&table.headers, header.checked_add(0x20)?)? as usize;
            return Some((addr, size));
        }
    }
    None
}



/// Bounded-read tests for the ELF loader.
///
/// Linux-only because the reader is: the Mach-O and PE loaders walk structures
/// the loader has already mapped and never open a file at all.
#[cfg(all(test, target_os = "linux"))]
mod elf_section_table_tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::FileExt;

    /// Removes the fixture even when an assertion unwinds, so a failing test
    /// does not leave a sparse multi-megabyte file behind in `/tmp`.
    struct Fixture(std::path::PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Where the fixture puts the two tables. Both are far enough into the
    /// file that a reader which slurps the whole image cannot pass the
    /// `bytes_read` bound below.
    const NAME_TABLE_AT: u64 = 8 << 20;
    const SECTION_TABLE_AT: u64 = 16 << 20;

    fn shdr(name: u32, flags: u64, addr: u64, offset: u64, size: u64) -> [u8; 0x40] {
        let mut entry = [0u8; 0x40];
        entry[0x00..0x04].copy_from_slice(&name.to_le_bytes());
        entry[0x08..0x10].copy_from_slice(&flags.to_le_bytes());
        entry[0x10..0x18].copy_from_slice(&addr.to_le_bytes());
        entry[0x18..0x20].copy_from_slice(&offset.to_le_bytes());
        entry[0x20..0x28].copy_from_slice(&size.to_le_bytes());
        entry
    }

    /// A sparse ELF64 image with `sections` section headers and `names` as its
    /// section-name table, both placed megabytes into the file.
    fn elf_image(tag: &str, names: &[u8], sections: &[[u8; 0x40]]) -> Fixture {
        let path = std::env::temp_dir().join(format!(
            "perry-elf-section-table-{}-{tag}.bin",
            std::process::id()
        ));
        let fixture = Fixture(path.clone());
        let file = std::fs::File::create(&path).expect("create fixture");
        let mut header = [0u8; ELF64_HEADER_BYTES];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2; // ELFCLASS64
        header[5] = 1; // little-endian
        header[6] = 1; // EV_CURRENT
        header[0x28..0x30].copy_from_slice(&SECTION_TABLE_AT.to_le_bytes());
        header[0x3A..0x3C].copy_from_slice(&(ELF64_SECTION_HEADER_BYTES as u16).to_le_bytes());
        header[0x3C..0x3E].copy_from_slice(&(sections.len() as u16).to_le_bytes());
        header[0x3E..0x40].copy_from_slice(&1u16.to_le_bytes()); // e_shstrndx
        file.write_all_at(&header, 0).expect("write header");
        file.write_all_at(names, NAME_TABLE_AT)
            .expect("write names");
        for (index, entry) in sections.iter().enumerate() {
            file.write_all_at(entry, SECTION_TABLE_AT + (index * 0x40) as u64)
                .expect("write section header");
        }
        fixture
    }

    /// `\0` `.shstrtab\0` `.perry_gcmap\0` `zz\0` — the trailing short name is
    /// what the skip test needs.
    const NAMES: &[u8] = b"\0.shstrtab\0.perry_gcmap\0zz\0";
    const SHSTRTAB_NAME: u32 = 1;
    const GCMAP_NAME: u32 = 11;
    const SHORT_NAME: u32 = 24;
    const SHF_ALLOC: u64 = 0x2;

    fn table_of(fixture: &Fixture) -> ElfSectionTable {
        let file = std::fs::File::open(&fixture.0).expect("open fixture");
        read_elf_section_table(&file)
            .expect("no I/O error")
            .expect("a usable ELF64 section table")
    }

    /// The property this change exists for: locating the section reads the
    /// header, the section table and the name table — not the image.
    #[test]
    fn reads_only_the_three_ranges_the_walk_needs() {
        let fixture = elf_image(
            "bounded",
            NAMES,
            &[
                shdr(0, 0, 0, 0, 0),
                shdr(SHSTRTAB_NAME, 0, 0, NAME_TABLE_AT, NAMES.len() as u64),
                shdr(GCMAP_NAME, SHF_ALLOC, 0x1000, 0, 0x40),
            ],
        );
        let table = table_of(&fixture);
        assert_eq!(
            elf_section_vaddr(&table, b".perry_gcmap"),
            Some((0x1000, 0x40))
        );
        // Header + three 64-byte entries + a 27-byte name table.
        assert_eq!(
            table.bytes_read(),
            ELF64_HEADER_BYTES + 3 * 0x40 + NAMES.len()
        );
        assert!(
            (table.bytes_read() as u64) < NAME_TABLE_AT,
            "the walk read {} bytes; the section it is looking for starts {NAME_TABLE_AT} bytes \
             into the image, so anything near that means the whole file is being read again",
            table.bytes_read()
        );
    }

    /// Bounding the name table by its own `sh_size` makes an unresolvable
    /// `sh_name` reachable for the first time. It must skip that entry, not
    /// end the search: ending it would drop the GC map of any image that lays
    /// a short name out after one, and the collector would find no native
    /// roots with no diagnostic at all.
    #[test]
    fn an_entry_whose_name_runs_past_the_name_table_is_skipped_not_fatal() {
        let fixture = elf_image(
            "skip",
            NAMES,
            &[
                shdr(0, 0, 0, 0, 0),
                shdr(SHSTRTAB_NAME, 0, 0, NAME_TABLE_AT, NAMES.len() as u64),
                // `sh_name` 24 names "zz": resolving 12 bytes there runs past
                // the end of the 27-byte table.
                shdr(SHORT_NAME, 0, 0, 0, 0),
                shdr(GCMAP_NAME, SHF_ALLOC, 0x2000, 0, 0x80),
            ],
        );
        let table = table_of(&fixture);
        assert_eq!(
            elf_section_vaddr(&table, b".perry_gcmap"),
            Some((0x2000, 0x80))
        );
    }

    /// Unchanged from the whole-file reader: a namesake with no runtime
    /// address is refused rather than turned into a slice.
    #[test]
    fn a_file_only_namesake_is_refused() {
        let fixture = elf_image(
            "noalloc",
            NAMES,
            &[
                shdr(0, 0, 0, 0, 0),
                shdr(SHSTRTAB_NAME, 0, 0, NAME_TABLE_AT, NAMES.len() as u64),
                shdr(GCMAP_NAME, 0, 0x1000, 0, 0x40),
            ],
        );
        let table = table_of(&fixture);
        assert_eq!(elf_section_vaddr(&table, b".perry_gcmap"), None);
    }

    /// A non-ELF file and one truncated before the table its header names are
    /// both "nothing to read here", not I/O errors — the caller skips those
    /// and reports only genuine errors, which is what reading the whole image
    /// and failing to parse it did.
    #[test]
    fn unusable_images_are_skipped_rather_than_reported_as_errors() {
        let path =
            std::env::temp_dir().join(format!("perry-elf-not-elf-{}.bin", std::process::id()));
        let fixture = Fixture(path.clone());
        std::fs::File::create(&path)
            .expect("create")
            .write_all(b"#!/bin/sh\necho not an elf\n")
            .expect("write");
        let file = std::fs::File::open(&fixture.0).expect("open");
        assert!(read_elf_section_table(&file)
            .expect("no I/O error")
            .is_none());

        // Valid header, section table beyond the end of the file.
        let truncated = elf_image("truncated", NAMES, &[]);
        let mut header = [0u8; ELF64_HEADER_BYTES];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2;
        header[0x28..0x30].copy_from_slice(&SECTION_TABLE_AT.to_le_bytes());
        header[0x3A..0x3C].copy_from_slice(&(ELF64_SECTION_HEADER_BYTES as u16).to_le_bytes());
        header[0x3C..0x3E].copy_from_slice(&4u16.to_le_bytes());
        header[0x3E..0x40].copy_from_slice(&1u16.to_le_bytes());
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&truncated.0)
            .expect("reopen");
        file.write_all_at(&header, 0).expect("rewrite header");
        file.set_len(ELF64_HEADER_BYTES as u64).expect("truncate");
        let file = std::fs::File::open(&truncated.0).expect("open truncated");
        assert!(read_elf_section_table(&file)
            .expect("no I/O error")
            .is_none());
    }
}
