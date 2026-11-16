//! Windows-specific link helpers for the Win32 link step (`is_windows`
//! branch of `build_and_run_link`). Holds the system-library link line and the
//! comctl32 v6 application-manifest embed. Extracted from `link/mod.rs` to keep
//! that file under the 2000-line CI gate (`scripts/check_file_size.sh`).
//!
//! Compiled Windows UI apps bound comctl32 v5 because Perry embedded no
//! application manifest into the linked `.exe`, so every common control
//! (buttons, list views, edit boxes…) rendered in the unthemed Win95/classic
//! style regardless of the OS theme. Embedding a manifest that declares the
//! `Microsoft.Windows.Common-Controls` v6 side-by-side dependency activates
//! visual styles (the Fluent look) on Windows 10/11. See issue #4681 /
//! discussion #3486.





#[cfg(test)]
mod tests {

}




