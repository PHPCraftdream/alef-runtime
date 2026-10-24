// SPDX-License-Identifier: MIT OR Apache-2.0
//! Export and import of the data of one application: a `.zip` archive of its data directory
//! with a Ktav manifest (`alef-data.ktav`) that lists every file with its size and SHA-256.
//! The Servo browser profile and file locks never travel: they are runtime-owned, not user data.
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::zip::{ZipArchive, ZipWriter};

/// The archive file that carries the manifest.
pub const MANIFEST_NAME: &str = "alef-data.ktav";
const FORMAT: u32 = 1;
const MAX_FILES: usize = 100_000;
const MAX_TOTAL: u64 = 64 << 30;
const MAX_FILE: u64 = 4 << 30; // zip32 per-entry limit
const RATIO: u64 = 1000;
const RATIO_FLOOR: u64 = 10 << 20;

/// What an export archived or an import replaced.
#[derive(Debug, Clone, Default)]
pub struct DataStats {
    pub files: u64,
    pub bytes: u64,
    /// Where the replaced data of the import were kept (`None` on export).
    pub backup: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    id: String,
    created: String,
    files: Vec<ManifestFile>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    path: String,
    size: u64,
    sha256: String,
}

/// UTC RFC3339 for a Unix timestamp in seconds: `1970-01-01T00:00:00Z`.
fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn now() -> u64 {
    UNIX_EPOCH
        .elapsed()
        .map(|t| t.as_secs())
        .unwrap_or_default()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Whether a relative path is excluded from the archive: the Servo profile, file locks and
/// anything that is not a plain file.
fn excluded(relative: &Path, is_dir: bool, metadata: &fs::Metadata) -> bool {
    if relative
        .components()
        .next()
        .is_some_and(|part| part.as_os_str() == "servo")
    {
        return true;
    }
    if relative
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with(".lock"))
    {
        return true;
    }
    if is_dir {
        return false;
    }
    if metadata.is_symlink() {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let kind = metadata.file_type();
        if kind.is_socket() || kind.is_fifo() {
            return true;
        }
    }
    false
}

/// Collects the archived files of `data_dir` in a deterministic order.
fn collect(data_dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut stack = vec![PathBuf::new()];
    let mut files = Vec::new();
    while let Some(relative) = stack.pop() {
        let full = data_dir.join(&relative);
        let read = fs::read_dir(&full)
            .map_err(|error| format!("cannot read {}: {error}", full.display()))?;
        let mut children: Vec<_> = read.filter_map(|entry| entry.ok()).collect();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let child_relative = relative.join(child.file_name());
            let Ok(metadata) = fs::symlink_metadata(child.path()) else {
                continue;
            };
            if excluded(&child_relative, metadata.is_dir(), &metadata) {
                continue;
            }
            if metadata.is_dir() {
                stack.push(child_relative);
            } else if metadata.is_file() {
                files.push((
                    child_relative
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                    child.path(),
                ));
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Serializes a manifest the way the one Ktav codec of the runtime writes its state files.
fn manifest_bytes(manifest: &Manifest) -> Result<Vec<u8>, String> {
    alef_core::security::permissions::store::to_bytes(manifest)
        .map_err(|error| format!("cannot write the manifest: {error}"))
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, String> {
    let manifest: Manifest = alef_core::security::permissions::store::from_bytes(bytes)
        .map_err(|error| format!("the manifest does not parse: {error}"))?;
    if manifest.format != FORMAT {
        return Err(format!(
            "unsupported archive format {} (this build reads {FORMAT})",
            manifest.format
        ));
    }
    Ok(manifest)
}

/// One validation pass over an archive for the confirmation dialog and before every import:
/// manifest present and sane, every path safe, the limits kept and every checksum right.
/// The archive is read exactly once.
pub fn inspect_archive(archive: &Path, app_id: &str) -> Result<DataStats, String> {
    let bytes = fs::read(archive).map_err(|error| format!("cannot read the archive: {error}"))?;
    inspect_bytes(&bytes, app_id)
}

fn inspect_bytes(bytes: &[u8], app_id: &str) -> Result<DataStats, String> {
    let parsed = ZipArchive::parse(bytes)?;
    let mut paths = HashSet::new();
    let mut data_files = 0u64;
    let mut total = 0u64;
    let mut manifest: Option<Manifest> = None;
    for entry in parsed.entries() {
        if entry.is_dir() {
            continue;
        }
        if entry.name == MANIFEST_NAME {
            let bytes = parsed.read(bytes, entry.name, MAX_FILE)?;
            let parsed_manifest = parse_manifest(&bytes)?;
            if parsed_manifest.id != app_id {
                return Err(format!(
                    "the archive belongs to app {}, not {app_id}",
                    parsed_manifest.id
                ));
            }
            manifest = Some(parsed_manifest);
            continue;
        }
        check_path(entry.name)?;
        if !paths.insert(entry.name.to_lowercase()) {
            return Err(format!("duplicate archive entry {}", entry.name));
        }
        data_files += 1;
        if data_files > MAX_FILES as u64 {
            return Err(format!("the archive holds more than {MAX_FILES} files"));
        }
        let uncompressed = u64::from(entry.uncompressed_size);
        let compressed = u64::from(entry.compressed_size);
        if uncompressed > MAX_FILE {
            return Err(format!("{} exceeds the 4 GiB entry limit", entry.name));
        }
        let expands = compressed == 0 || uncompressed / compressed.max(1) > RATIO;
        if expands && uncompressed > RATIO_FLOOR {
            return Err(format!("{} expands too much (archive bomb?)", entry.name));
        }
        total += uncompressed;
        if total > MAX_TOTAL {
            return Err("the archive expands above 64 GiB (archive bomb?)".into());
        }
    }
    let manifest =
        manifest.ok_or_else(|| format!("the archive has no {MANIFEST_NAME} manifest"))?;
    if manifest.files.len() > MAX_FILES {
        return Err(format!("the archive holds more than {MAX_FILES} files"));
    }
    let mut listed_total = 0u64;
    for file in &manifest.files {
        check_path(&file.path)?;
        if file.path == MANIFEST_NAME {
            return Err("the manifest lists itself".into());
        }
        if !paths.insert(file.path.to_lowercase()) {
            return Err(format!("duplicate archive entry {}", file.path));
        }
        listed_total += file.size;
    }
    if listed_total > MAX_TOTAL {
        return Err("the archive expands above 64 GiB (archive bomb?)".into());
    }
    // Verify every checksum before anything touches the disk.
    let mut files = 0u64;
    let mut bytes_total = 0u64;
    for file in &manifest.files {
        let contents = parsed.read(bytes, &file.path, MAX_FILE)?;
        if contents.len() as u64 != file.size || sha256_hex(&contents) != file.sha256 {
            return Err(format!("archive integrity check failed: {}", file.path));
        }
        files += 1;
        bytes_total += contents.len() as u64;
    }
    Ok(DataStats {
        files,
        bytes: bytes_total,
        backup: None,
    })
}

/// Every reason a relative archive path must never reach the disk.
fn check_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty entry path".into());
    }
    if path.contains('\\') {
        return Err(format!("backslash in entry path {path}"));
    }
    if path.starts_with('/') {
        return Err(format!("absolute entry path {path}"));
    }
    if path.len() >= 2 && path.as_bytes()[1] == b':' && path.as_bytes()[0].is_ascii_alphabetic() {
        return Err(format!("drive letter in entry path {path}"));
    }
    if path.chars().any(|c| c.is_control() || c == '\0') {
        return Err(format!("control character in entry path {path}"));
    }
    for component in path.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return Err(format!("bad path component in {path}"));
        }
        let stem = component.split('.').next().unwrap_or(component);
        if matches!(
            stem.to_ascii_uppercase().as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        ) {
            return Err(format!("reserved name in entry path {path}"));
        }
    }
    Ok(())
}

/// Archives the data directory of an application. The Servo profile, file locks, symlinks and
/// Unix sockets or pipes are left out; an empty directory archives as just the manifest.
/// The archive appears at `dest` only once it is complete.
pub fn export_app_data(data_dir: &Path, app_id: &str, dest: &Path) -> Result<DataStats, String> {
    if !data_dir.is_dir() {
        return Err(format!("no data directory for app {app_id}"));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let files = collect(data_dir)?;
    let stamp = now();
    let mut manifest_files = Vec::with_capacity(files.len());
    let mut stats = DataStats::default();
    let payload = |writer: &mut ZipWriter<Vec<u8>>| -> Result<(), String> {
        for (relative, full) in &files {
            let contents = fs::read(full)
                .map_err(|error| format!("cannot read {}: {error}", full.display()))?;
            manifest_files.push(ManifestFile {
                path: relative.clone(),
                size: contents.len() as u64,
                sha256: sha256_hex(&contents),
            });
            writer
                .add(relative, &contents)
                .map_err(|error| error.to_string())?;
            stats.files += 1;
            stats.bytes += contents.len() as u64;
        }
        let manifest = Manifest {
            format: FORMAT,
            id: app_id.to_owned(),
            created: rfc3339(stamp),
            files: manifest_files,
        };
        let bytes = manifest_bytes(&manifest)?;
        writer
            .add(MANIFEST_NAME, &bytes)
            .map_err(|error| error.to_string())
    };
    let mut writer = ZipWriter::new(Vec::new(), stamp);
    payload(&mut writer)?;
    let archive = writer.finish().map_err(|error| error.to_string())?;
    // Replace `dest` only once the complete archive is in hand: a temporary sibling, then rename.
    let temp = dest.with_file_name(format!(
        "{}.export-{}-{}",
        dest.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id(),
        now()
    ));
    fs::write(&temp, &archive).map_err(|error| format!("cannot write the archive: {error}"))?;
    place_file(&temp, dest)?;
    Ok(stats)
}

fn place_file(temp: &Path, dest: &Path) -> Result<(), String> {
    #[cfg(windows)]
    if dest.is_file() {
        fs::remove_file(dest).map_err(|error| format!("cannot replace the archive: {error}"))?;
    }
    #[cfg(unix)]
    let _ = dest;
    fs::rename(temp, dest).map_err(|error| format!("cannot place the archive: {error}"))
}

/// Whether the application holds its single-instance endpoint right now. Overridable in tests.
fn app_is_running(app_id: &str) -> bool {
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let (_, _, cache) = crate::application::plan::application_base_dirs(&home);
    alef_modules::desktop::app::instance::is_running(app_id, &home, &cache.join(app_id))
}

/// Replaces the data directory of an application with the contents of an archive. The archive is
/// fully inspected and checksum-verified first; the extraction goes into a sibling staging
/// directory and the old data are kept at `<data_dir>.before-import`.
pub fn import_app_data(data_dir: &Path, app_id: &str, archive: &Path) -> Result<DataStats, String> {
    import_with(data_dir, app_id, archive, || app_is_running(app_id))
}

fn import_with(
    data_dir: &Path,
    app_id: &str,
    archive: &Path,
    is_running: impl Fn() -> bool,
) -> Result<DataStats, String> {
    if is_running() {
        return Err(format!(
            "app {app_id} appears to be running; close it before importing data"
        ));
    }
    let bytes = fs::read(archive).map_err(|error| format!("cannot read the archive: {error}"))?;
    let stats = inspect_bytes(&bytes, app_id)?;
    let staging = data_dir.with_file_name(format!(
        "{}.import-{}-{}",
        data_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|t| t.subsec_nanos() | (t.as_secs() << 32) as u32)
            .unwrap_or_default()
    ));
    let _ = remove_path(&staging);
    fs::create_dir(&staging).map_err(|error| format!("cannot stage the import: {error}"))?;
    let placed = extract(&bytes, &staging);
    if let Err(error) = placed {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    // The staging directory is complete: swap it in, keeping one backup of the old data.
    let backup = data_dir.with_file_name(format!(
        "{}.before-import",
        data_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    if data_dir.exists() {
        remove_path(&backup)?;
        fs::rename(data_dir, &backup)
            .map_err(|error| format!("cannot keep the old data aside: {error}"))?;
        if let Err(error) = swap_in(&staging, data_dir, &backup) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        return Ok(DataStats {
            files: stats.files,
            bytes: stats.bytes,
            backup: Some(backup),
        });
    }
    match fs::rename(&staging, data_dir) {
        Ok(()) => Ok(DataStats {
            files: stats.files,
            bytes: stats.bytes,
            backup: None,
        }),
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            Err(format!("cannot place the data: {error}"))
        }
    }
}

/// Renames the staging directory into place; on failure the backup goes back.
fn swap_in(staging: &Path, data_dir: &Path, backup: &Path) -> Result<(), String> {
    match fs::rename(staging, data_dir) {
        Ok(()) => Ok(()),
        Err(error) => {
            let original = format!("cannot place the data: {error}");
            match fs::rename(backup, data_dir) {
                Ok(()) => Err(format!("{original}; the old data were restored")),
                Err(rollback) => Err(format!(
                    "{original}; and the rollback failed ({rollback}): the old data are at {}",
                    backup.display()
                )),
            }
        }
    }
}

/// Extracts the archive into `staging`; nothing outside it is ever touched.
fn extract(bytes: &[u8], staging: &Path) -> Result<(), String> {
    let parsed = ZipArchive::parse(bytes)?;
    let mut made: HashSet<String> = HashSet::new();
    let manifest: Option<Manifest> = parsed
        .entries()
        .find(|entry| entry.name == MANIFEST_NAME)
        .map(|entry| parsed.read(bytes, entry.name, MAX_FILE))
        .transpose()?
        .map(|bytes| parse_manifest(&bytes))
        .transpose()?;
    let manifest =
        manifest.ok_or_else(|| format!("the archive has no {MANIFEST_NAME} manifest"))?;
    // Directories first, so empty folders survive the trip.
    for dir in dirs_of(&manifest) {
        let target = staging.join(&dir);
        if !target.starts_with(staging) {
            return Err(format!("entry escapes the staging directory: {dir}"));
        }
        fs::create_dir_all(&target).map_err(|error| format!("cannot extract {dir}: {error}"))?;
        made.insert(dir);
    }
    for file in &manifest.files {
        let parent = parent_of(&file.path);
        if !parent.is_empty() && !made.contains(&parent) {
            return Err(format!(
                "entry escapes the staging directory: {}",
                file.path
            ));
        }
        let target = staging.join(&file.path);
        if !target.starts_with(staging) {
            return Err(format!(
                "entry escapes the staging directory: {}",
                file.path
            ));
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot extract {}: {error}", file.path))?;
        }
        let contents = parsed
            .read(bytes, &file.path, MAX_FILE)
            .map_err(|error| format!("cannot extract {}: {error}", file.path))?;
        fs::write(&target, &contents)
            .map_err(|error| format!("cannot extract {}: {error}", file.path))?;
    }
    Ok(())
}

/// The directories the archived files live in, deepest-parent order not needed: `create_dir_all`
/// handles nesting.
fn dirs_of(manifest: &Manifest) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    for file in &manifest.files {
        let mut parent = parent_of(&file.path);
        while !parent.is_empty() {
            if seen.insert(parent.clone()) {
                dirs.push(parent.clone());
            }
            parent = parent_of(&parent);
        }
    }
    dirs
}

fn parent_of(path: &str) -> String {
    match path.rfind('/') {
        Some(at) => path[..at].to_owned(),
        None => String::new(),
    }
}

fn remove_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)
            .map_err(|error| format!("cannot remove {}: {error}", path.display())),
        Ok(_) => fs::remove_file(path)
            .map_err(|error| format!("cannot remove {}: {error}", path.display())),
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
#[path = "data_tests.rs"]
mod data_tests;
