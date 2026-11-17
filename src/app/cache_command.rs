//! `parakit cache` subcommands: dir and list.

use super::*;
use crate::cli::{CacheCli, CacheCommand};

/// Run a cache subcommand, defaulting to `list` when none is given.
///
/// # Arguments
/// * `cache` - Requested cache subcommand.
/// * `quiet` - Suppress normal output.
///
/// # Returns
/// Success after the requested cache operation.
///
/// # Errors
///
/// Returns an error when the model-cache path cannot be resolved or output
/// cannot be written.
pub(super) fn run_cache_command(cache: &CacheCli, quiet: bool) -> Result<()> {
    match cache.command.as_ref().unwrap_or(&CacheCommand::List) {
        CacheCommand::Dir => {
            let dir = model::models_dir()?;
            if !quiet {
                outln!("{}", dir.display())?;
            }
        }
        CacheCommand::List => print_cache_list(quiet)?,
    }
    Ok(())
}

fn print_cache_list(quiet: bool) -> Result<()> {
    let dir = model::models_dir()?;
    if quiet {
        return Ok(());
    }
    outln!("parakit cache")?;
    outln!("  dir: {}", dir.display())?;
    if !dir.is_dir() {
        outln!("  models: none")?;
        return Ok(());
    }

    let top_level = list_gguf_files(&dir);
    let mut extra = list_nested_gguf_files(&dir.join("hub"));
    extra.extend(list_nested_gguf_files(&dir.join("url")));
    extra.sort();

    if top_level.is_empty() && extra.is_empty() {
        outln!("  models: none")?;
        return Ok(());
    }

    outln!("  models:")?;
    for path in &top_level {
        print_cache_entry(&dir, path, &model_file_name(path))?;
    }
    for path in &extra {
        print_cache_entry(&dir, path, &relative_cache_display(&dir, path))?;
    }
    Ok(())
}

/// List `.gguf` files directly inside `dir`, sorted.
fn list_gguf_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
        })
        .collect();
    entries.sort();
    entries
}

/// List `.gguf` files one level below `parent` (`parent/*/*.gguf`), sorted.
/// Used for revision-keyed Hub entries and URL-keyed direct downloads.
fn list_nested_gguf_files(parent: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .flat_map(|sub| list_gguf_files(&sub))
        .collect();
    entries.sort();
    entries
}

/// Display a `hub/…`/`url/…` cache entry by its path relative to `dir`,
/// with `/` separators regardless of platform.
fn relative_cache_display(dir: &Path, path: &Path) -> String {
    path.strip_prefix(dir)
        .map(model::slash_joined)
        .unwrap_or_else(|_| model_file_name(path))
}

fn print_cache_entry(dir: &Path, path: &Path, display_name: &str) -> Result<()> {
    let dtype = gguf::dtype_label(path);
    let size = path
        .metadata()
        .map(|meta| format_file_size(meta.len()))
        .unwrap_or_else(|_| "unknown size".to_string());
    let is_default_q8 = path == fetch::preset_cache_path(dir, ModelPreset::default());
    let default_marker = if is_default_q8 { " default" } else { "" };
    outln!("    {display_name}{default_marker}: {dtype}, {size}")
}
