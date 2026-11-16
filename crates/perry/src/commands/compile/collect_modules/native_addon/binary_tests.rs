use super::validate_node_api_binary;

/// A minimal ELF64 shared object that imports one symbol: a header, a
/// `.dynsym` holding that undefined symbol, and the two string tables. It has
/// no dynamic section, which the reach reader takes as open.
pub(super) fn elf_import(name: &str, aarch64: bool) -> Vec<u8> {
    let dynstr = [&[0u8][..], name.as_bytes(), &[0]].concat();
    let shstrtab = b"\0.dynsym\0.dynstr\0.shstrtab\0".to_vec();
    let mut dynsym = vec![0u8; 24]; // the null symbol
    dynsym.extend(1u32.to_le_bytes()); // st_name: skips the initial NUL
    dynsym.extend([0x12, 0]); // STB_GLOBAL | STT_FUNC, default visibility
    dynsym.extend(0u16.to_le_bytes()); // SHN_UNDEF
    dynsym.extend([0u8; 16]); // st_value, st_size
    let dynsym_at = 64u64;
    let dynstr_at = dynsym_at + dynsym.len() as u64;
    let shstrtab_at = dynstr_at + dynstr.len() as u64;
    let shoff = (shstrtab_at + shstrtab.len() as u64).next_multiple_of(8);
    let mut bytes = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0]; // 64-bit, little-endian
    bytes.extend([0u8; 8]);
    bytes.extend(3u16.to_le_bytes()); // ET_DYN
    bytes.extend((if aarch64 { 183u16 } else { 62 }).to_le_bytes()); // EM_AARCH64, EM_X86_64
    bytes.extend(1u32.to_le_bytes());
    bytes.extend(0u64.to_le_bytes()); // e_entry
    bytes.extend(0u64.to_le_bytes()); // e_phoff
    bytes.extend(shoff.to_le_bytes());
    bytes.extend(0u32.to_le_bytes()); // e_flags
    // e_ehsize, e_phentsize, e_phnum, e_shentsize, e_shnum, e_shstrndx
    for half in [64u16, 0, 0, 64, 4, 3] {
        bytes.extend(half.to_le_bytes());
    }
    assert_eq!(bytes.len(), 64);
    bytes.extend(&dynsym);
    bytes.extend(&dynstr);
    bytes.extend(&shstrtab);
    bytes.resize(shoff as usize, 0);
    let mut section = |name: u32, kind: u32, offset: u64, size: u64, link: u32, entsize: u64| {
        bytes.extend(name.to_le_bytes());
        bytes.extend(kind.to_le_bytes());
        bytes.extend([0u8; 16]); // sh_flags, sh_addr
        bytes.extend(offset.to_le_bytes());
        bytes.extend(size.to_le_bytes());
        bytes.extend(link.to_le_bytes());
        bytes.extend(1u32.to_le_bytes()); // sh_info: one local symbol, the null one
        bytes.extend(1u64.to_le_bytes()); // sh_addralign
        bytes.extend(entsize.to_le_bytes());
    };
    section(0, 0, 0, 0, 0, 0);
    section(1, 11, dynsym_at, dynsym.len() as u64, 2, 24); // SHT_DYNSYM, names in section 2
    section(9, 3, dynstr_at, dynstr.len() as u64, 0, 0); // SHT_STRTAB
    section(17, 3, shstrtab_at, shstrtab.len() as u64, 0, 0);
    bytes
}

/// One imported symbol in a minimal two-level Mach-O image. Keep the symbol
/// range separate from its library ordinal: dynamic lookup uses 0xfe, and
/// executable lookup uses 0xff, neither of which is a dylib-table index.
pub(super) fn macho_import(name: &str, ordinal: u8, is_64: bool, symbol_type: u8) -> Vec<u8> {
    let header_size = if is_64 { 32 } else { 28 };
    let symbol_size = if is_64 { 16 } else { 12 };
    let symbol_offset = header_size + 104;
    let string_offset = symbol_offset + symbol_size;
    let mut bytes = Vec::new();
    let header = [
        if is_64 { 0xfeedfacfu32 } else { 0xfeedface },
        if is_64 { 0x01000007 } else { 7 },
        3, // CPU subtype
        6, // MH_DYLIB
        2, // LC_SYMTAB and LC_DYSYMTAB
        104,
        0x80, // MH_TWOLEVEL
    ];
    for word in header {
        bytes.extend(word.to_le_bytes());
    }
    if is_64 {
        bytes.extend(0u32.to_le_bytes());
    }
    for word in [
        2u32,
        24,
        symbol_offset,
        1,
        string_offset,
        name.len() as u32 + 2,
    ] {
        bytes.extend(word.to_le_bytes());
    }
    let mut dysymtab = [0u32; 20];
    dysymtab[0] = 11;
    dysymtab[1] = 80;
    dysymtab[7] = 1; // nundefsym; iundefsym is zero
    for word in dysymtab {
        bytes.extend(word.to_le_bytes());
    }
    assert_eq!(bytes.len(), symbol_offset as usize);
    bytes.extend(1u32.to_le_bytes()); // n_strx skips the initial NUL
    bytes.extend([symbol_type, 0, 0, ordinal]);
    bytes.extend(vec![0; if is_64 { 8 } else { 4 }]);
    bytes.push(0);
    bytes.extend(name.as_bytes());
    bytes.push(0);
    bytes
}

fn validate(bytes: &[u8]) -> anyhow::Result<()> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("addon.node");
    std::fs::write(&path, bytes).unwrap();
    validate_node_api_binary(&path).map(|_| ())
}

fn reach(bytes: &[u8]) -> std::collections::BTreeSet<perry_runtime_manifest::NodeApiReach> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("addon.node");
    std::fs::write(&path, bytes).unwrap();
    validate_node_api_binary(&path).unwrap().0
}

fn node_api_imports(bytes: &[u8]) -> std::collections::BTreeSet<String> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("addon.node");
    std::fs::write(&path, bytes).unwrap();
    validate_node_api_binary(&path).unwrap().1
}

/// The fixture above is read as an ELF addon: its Node-API import is found,
/// an unsupported import is refused, and with no dynamic section to read the
/// linked libraries from, it counts as open.
#[test]
fn elf_imports_are_read_and_checked() {
    for aarch64 in [false, true] {
        assert_eq!(
            node_api_imports(&elf_import("napi_create_int32", aarch64)),
            ["napi_create_int32".to_string()].into()
        );
        let error = validate(&elf_import("uv_run", aarch64))
            .unwrap_err()
            .to_string();
        assert!(error.contains("imports unsupported symbol"), "{error}");
    }
    assert_eq!(
        reach(&elf_import("napi_create_int32", false)),
        [perry_runtime_manifest::NodeApiReach::World].into()
    );
}

/// The host exports what its addons import: the addon's Node-API imports,
/// as C spells them, and nothing else it imports.
#[test]
fn node_api_imports_are_c_names() {
    for is_64 in [false, true] {
        assert_eq!(
            node_api_imports(&macho_import("_napi_create_int32", 0xfe, is_64, 1)),
            ["napi_create_int32".to_string()].into()
        );
        assert!(node_api_imports(&macho_import("_dlsym", 0xfe, is_64, 1)).is_empty());
    }
}

/// A native addon reaches what its Node-API imports reach: one that only
/// makes and fills values is closed, one that can read a global or a
/// property, or look a symbol up at run time, is open.
#[test]
fn addon_reach_is_its_node_api_imports() {
    use perry_runtime_manifest::NodeApiReach;
    for is_64 in [false, true] {
        for name in ["_napi_create_int32", "_napi_set_named_property"] {
            assert!(
                reach(&macho_import(name, 0xfe, is_64, 1)).is_empty(),
                "{name}"
            );
        }
        for name in [
            "_napi_get_global",
            "_napi_get_named_property",
            "_napi_run_script",
            "_napi_call_function",
            "_napi_unknown_future_export",
            "_dlsym",
            "_dlopen",
        ] {
            assert_eq!(
                reach(&macho_import(name, 0xfe, is_64, 1)),
                [NodeApiReach::World].into(),
                "{name}"
            );
        }
        assert_eq!(
            reach(&macho_import("_napi_create_date", 0xfe, is_64, 1)),
            [NodeApiReach::Date].into()
        );
    }
}

#[test]
fn macho_dynamic_and_executable_lookup_accept_node_api() {
    for is_64 in [false, true] {
        for ordinal in [0xfe, 0xff] {
            validate(&macho_import("_napi_create_int32", ordinal, is_64, 1))
                .unwrap_or_else(|error| panic!("64-bit={is_64}, ordinal={ordinal}: {error}"));
        }
    }
}

#[test]
fn macho_special_ordinals_still_reject_unsupported_imports() {
    for is_64 in [false, true] {
        for name in [
            "_uv_run",
            "__ZN2v812some_function",
            "__ZN4node8Function",
            "Nan::Call",
        ] {
            for ordinal in [0xfe, 0xff] {
                let error = validate(&macho_import(name, ordinal, is_64, 1))
                    .unwrap_err()
                    .to_string();
                assert!(
                    error.contains("imports unsupported symbol"),
                    "{name}: {error}"
                );
                assert!(error.contains(name), "{error}");
            }
        }
    }
}

#[test]
fn macho_prebound_undefined_import_is_not_skipped() {
    let error = validate(&macho_import("_uv_run", 0xfe, true, 0x0d))
        .unwrap_err()
        .to_string();
    assert!(error.contains("imports unsupported symbol"), "{error}");
}

#[test]
fn macho_invalid_import_range_is_rejected() {
    let mut bytes = macho_import("_napi_create_int32", 0, true, 1);
    bytes[84..88].copy_from_slice(&2u32.to_le_bytes());
    let error = validate(&bytes).unwrap_err().to_string();
    assert!(error.contains("symbol index"), "{error}");
}
