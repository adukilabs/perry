//! The default hot-text order on Mach-O (macOS, iOS, tvOS, watchOS,
//! visionOS): the same list as the ELF order (`hot_text_order`), resolved
//! against the Mach-O runtime archives of the link and passed to ld64 (or
//! ld64.lld) as an `-order_file`.
//!
//! ld64 lays out the atoms an order file names first in their section, in
//! file order, and takes exact symbol names, so the hash-free list keys are
//! resolved to the archives' own names before the link (the leading `_` of
//! Mach-O symbols does not enter the key). A key the archives no longer hold
//! writes no line; a name that the link does not pull in is ignored by the
//! linker. Nothing else changes: no IR, no object, no section.
//!
//! OFF by default: the gain on Apple targets (16 KiB pages, a different
//! fault-around) is not measured yet. `PERRY_HOT_TEXT_ORDER=on` (or `1`)
//! turns it on; unset or `off` keeps today's layout.






/// The order file's lines: every archive symbol the list names, by tier
/// and rank (one line per name), and how much of the list resolved.
#[derive(Debug, Default, PartialEq, Eq)]
struct Placement {
    names: Vec<String>,
    listed: usize,
    resolved: usize,
    core_listed: usize,
    core_resolved: usize,
}




#[cfg(test)]
mod tests {






}
