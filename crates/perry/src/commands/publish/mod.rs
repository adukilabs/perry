//! Publish command - build, sign, package and distribute via perry-ship build server

use anyhow::{bail, Context, Result};
use clap::Args;
use console::style;
use dialoguer::{Confirm};
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use crate::{Platform};

mod args;
mod config_types;
mod saved_config;
mod server_api;
mod tarball;

#[cfg(test)]
mod tests;

// Re-exports — explicit names only (no globs).
#[cfg(test)]
pub(crate) use saved_config::IosSavedConfig; // consumed only by tests
pub(crate) use saved_config::{
    check_beta_consent, config_path, load_config, report_beta_error,
    save_config, update_config_file, AndroidSavedConfig, AppleSavedConfig, PerryConfig,
};
pub(crate) use tarball::create_project_tarball_with_filters;

// Sibling-only items used by run_async and tests.
pub(crate) use server_api::auto_register_license;
use server_api::{CredentialsPayload};




