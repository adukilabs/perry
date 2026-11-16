use std::process::Command;

pub(super) fn add_link_args(
    cmd: &mut Command,
    darwin: bool,
    direct_apple: bool,
    xlinker_apple: bool,
) {
    return;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(darwin: bool, direct: bool, xlinker: bool) -> Vec<String> {
        let mut cmd = Command::new("linker");
        add_link_args(&mut cmd, darwin, direct, xlinker);
        cmd.get_args()
            .map(|arg| arg.to_str().unwrap().to_owned())
            .collect()
    }

    fn symbols() -> Vec<String> {
        perry_abi::NATIVE_LAYOUT_DESCRIPTORS
            .iter()
            .filter(|name| !perry_abi::NATIVE_LAYOUT_RESERVED.contains(name))
            .map(|name| format!("_perry_runtime_layout_{name}"))
            .collect()
    }

    #[test]
    fn driver_admits_exact_catalog_symbols_without_reserved_rows() {
        let expected: Vec<_> = symbols()
            .iter()
            .map(|name| format!("-Wl,-U,{name}"))
            .collect();
        assert!(!expected.is_empty());
        assert_eq!(arguments(true, false, false), expected);
    }

    #[test]
    fn direct_and_swift_linkers_render_named_admission() {
        let symbols = symbols();
        assert_eq!(
            arguments(true, true, false),
            symbols
                .iter()
                .flat_map(|name| ["-U".to_string(), name.clone()])
                .collect::<Vec<_>>()
        );
        assert_eq!(
            arguments(true, false, true),
            symbols
                .iter()
                .flat_map(|name| [
                    "-Xlinker".to_string(),
                    "-U".to_string(),
                    "-Xlinker".to_string(),
                    name.clone()
                ])
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn non_darwin_links_receive_no_admissions() {
        for direct in [false, true] {
            for xlinker in [false, true] {
                assert!(arguments(false, direct, xlinker).is_empty());
            }
        }
    }

}
