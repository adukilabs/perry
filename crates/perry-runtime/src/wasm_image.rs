//! Linear-memory counterparts of the native image sections.
//!
//! Rust's Wasm custom sections cannot contain relocations. Keep the actual
//! records in linear memory and enumerate their addresses through constructors
//! instead. WASI's command/reactor initialization runs these before JS starts.
//! These are immutable image addresses, never pointers into the managed heap.
//!
//! This covers the runtime's own images, which live outside the `perry_strings`
//! section. Program string literals, atoms, descriptors and realm cells use
//! ordinary linear-memory sections and linker-synthesized `__start_/__stop_`
//! bounds. This inventory enumerates runtime records only; program records
//! never need constructors or a runtime address registry.


macro_rules! wasm_atom {
    ($name:ident) => {
        #[cfg(target_family = "wasm")]
        inventory::submit! { crate::wasm_image::Atom(std::ptr::addr_of!($name)) }
    };
}
macro_rules! wasm_string_image {
    ($name:ident) => {
        #[cfg(target_family = "wasm")]
        inventory::submit! {
            crate::wasm_image::Image {
                start: std::ptr::addr_of!($name).cast(),
                len: std::mem::size_of_val(&$name),
            }
        }
    };
}
