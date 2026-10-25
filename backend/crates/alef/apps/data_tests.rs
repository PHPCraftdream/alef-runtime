// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pure unit tests for the application data export/import: no processes, no network; every
//! temporary directory lives under the system temporary folder with a unique name and is
//! removed at the end of its test.
use super::{export_app_data, inspect_archive, DataStats, Manifest, ManifestFile, MANIFEST_NAME};
use crate::apps::zip::ZipWriter;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique, always-removed scratch directory for one test.
struct Scratch(PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "alef-data-tests-{}-{}-{}",
            std::process::id(),
            label,
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn entry(path: &str, size: u64, sha256: &str) -> ManifestFile {
    ManifestFile {
        path: path.to_owned(),
        size,
        sha256: sha256.to_owned(),
    }
}

fn manifest_bytes(app_id: &str, files: Vec<ManifestFile>) -> Vec<u8> {
    alef_core::security::permissions::store::to_bytes(&Manifest {
        format: 1,
        id: app_id.to_owned(),
        created: "2026-01-01T00:00:00Z".to_owned(),
        files,
    })
    .unwrap()
}

fn never_running() -> bool {
    false
}

fn always_running() -> bool {
    true
}

/// Exports `data` and returns the archive bytes through `inspect_archive`'s path.
fn export(data: &Path, id: &str, scratch: &Scratch, label: &str) -> PathBuf {
    let dest = scratch.path().join(format!("{label}.zip"));
    let stats = export_app_data(data, id, &dest).unwrap();
    assert!(dest.is_file());
    assert!(stats.backup.is_none());
    dest
}

const ALL_BYTES: [u8; 256] = {
    let mut bytes = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        bytes[i] = i as u8;
        i += 1;
    }
    bytes
};

#[test]
fn manifest_round_trips_through_the_ktav_codec() {
    let manifest = Manifest {
        format: 1,
        id: "org.example.app".to_owned(),
        created: "2026-10-24T12:34:56Z".to_owned(),
        files: vec![entry("store/db.sqlite", 256, "ab".repeat(32).as_str())],
    };
    let bytes = alef_core::security::permissions::store::to_bytes(&manifest).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("format: 1"), "unexpected text: {text}");
    assert!(text.contains("id: org.example.app"));
    assert!(
        text.lines()
            .any(|line| line.starts_with("created") && line.contains("2026-10-24T12:34:56Z")),
        "unexpected text: {text}"
    );
    assert!(text.contains("path: store/db.sqlite"));
    let back: Manifest =
        alef_core::security::permissions::store::from_bytes(text.as_bytes()).unwrap();
    assert_eq!(back.id, "org.example.app");
    assert_eq!(back.files.len(), 1);
    assert_eq!(back.files[0].path, "store/db.sqlite");
    assert_eq!(back.files[0].size, 256);
    // A tampered manifest is rejected, not silently trimmed.
    let broken = text.replace("format: 1", "format: 2\nextra: 1");
    assert!(
        alef_core::security::permissions::store::from_bytes::<Manifest>(broken.as_bytes()).is_err()
    );
}

#[test]
fn export_excludes_the_servo_profile_and_file_locks() {
    let scratch = Scratch::new("exclusions");
    let data = scratch.path().join("data");
    write_file(&data.join("store/db.sqlite"), b"user data");
    write_file(&data.join("logs/app.log"), b"log line");
    write_file(&data.join("servo/cache.dat"), b"browser cache");
    write_file(&data.join("servo/nested/inner.bin"), b"more cache");
    write_file(&data.join("db.lock"), b"lock");
    write_file(&data.join("deep/other.lock"), b"lock");
    let dest = scratch.path().join("out.zip");
    export_app_data(&data, "org.example.app", &dest).unwrap();
    let archive = fs::read(&dest).unwrap();
    let parsed = crate::apps::zip::ZipArchive::parse(&archive).unwrap();
    let names: Vec<_> = parsed.entries().map(|e| e.name.to_owned()).collect();
    assert!(names.iter().any(|n| n == "store/db.sqlite"));
    assert!(names.iter().any(|n| n == "logs/app.log"));
    assert!(!names.iter().any(|n| n.starts_with("servo")));
    assert!(!names.iter().any(|n| n.ends_with(".lock")));
}

#[test]
fn export_refuses_a_missing_directory_but_accepts_an_empty_one() {
    let scratch = Scratch::new("empty");
    let missing = scratch.path().join("missing");
    let error =
        export_app_data(&missing, "org.example.app", &scratch.path().join("a.zip")).unwrap_err();
    assert!(error.contains("no data directory"), "error: {error}");
    let empty = scratch.path().join("empty-data");
    fs::create_dir_all(&empty).unwrap();
    let dest = scratch.path().join("b.zip");
    let stats = export_app_data(&empty, "org.example.app", &dest).unwrap();
    assert_eq!(stats.files, 0);
    assert!(inspect_archive(&dest, "org.example.app").is_ok());
}

#[test]
fn export_replaces_an_existing_archive() {
    let scratch = Scratch::new("replace");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"first");
    let dest = scratch.path().join("out.zip");
    export_app_data(&data, "org.example.app", &dest).unwrap();
    fs::remove_dir_all(&data).unwrap();
    fs::create_dir_all(&data).unwrap();
    write_file(&data.join("b.txt"), b"second");
    export_app_data(&data, "org.example.app", &dest).unwrap();
    let parsed = crate::apps::zip::ZipArchive::parse(&fs::read(&dest).unwrap()).unwrap();
    let names: Vec<_> = parsed.entries().map(|e| e.name.to_owned()).collect();
    assert!(!names.iter().any(|n| n == "a.txt"));
    assert!(names.iter().any(|n| n == "b.txt"));
}

#[test]
fn export_import_round_trips_every_byte() {
    let scratch = Scratch::new("roundtrip");
    let data = scratch.path().join("data");
    let mut binary = ALL_BYTES.to_vec();
    binary.extend_from_slice(b"\xff\x00tail");
    write_file(&data.join("store/db.sqlite"), &binary);
    write_file(&data.join("logs/deep/nested/app.log"), b"line\n");
    write_file(&data.join("window-state.ktav"), b"label: main\n");
    let dest = export(&data, "org.example.app", &scratch, "rt");
    let inspected = inspect_archive(&dest, "org.example.app").unwrap();
    assert_eq!(inspected.files, 3);
    assert_eq!(inspected.bytes, binary.len() as u64 + 5 + 12);
    let target = scratch.path().join("restored");
    import_never_running(&target, &dest).unwrap();
    assert_eq!(fs::read(target.join("store/db.sqlite")).unwrap(), binary);
    assert_eq!(
        fs::read(target.join("logs/deep/nested/app.log")).unwrap(),
        b"line\n"
    );
    assert_eq!(
        fs::read(target.join("window-state.ktav")).unwrap(),
        b"label: main\n"
    );
    // The target did not exist, so there was nothing to keep.
    let backup = target.with_file_name("restored.before-import");
    assert!(!backup.exists());
}

/// Imports with the running check overridden, because tests spawn no applications.
fn import_never_running(target: &Path, archive: &Path) -> Result<DataStats, String> {
    super::import_with(target, "org.example.app", archive, never_running)
}

#[test]
fn import_refuses_a_running_application_without_touching_data() {
    let scratch = Scratch::new("running");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"original");
    let dest = export(&data, "org.example.app", &scratch, "run");
    let error = super::import_with(&data, "org.example.app", &dest, always_running).unwrap_err();
    assert!(error.contains("appears to be running"), "error: {error}");
    assert_eq!(fs::read(data.join("a.txt")).unwrap(), b"original");
}

#[test]
fn import_rejects_a_foreign_or_future_archive() {
    let scratch = Scratch::new("foreign");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"x");
    let dest = export(&data, "org.other.app", &scratch, "foreign");
    let target = scratch.path().join("target");
    fs::create_dir_all(&target).unwrap();
    let error = import_never_running(&target, &dest).unwrap_err();
    assert!(
        error.contains("belongs to app org.other.app"),
        "error: {error}"
    );
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
    // A manifest with a wrong format is refused the same way.
    let tampered = scratch.path().join("format.zip");
    let manifest = manifest_bytes("org.example.app", vec![entry("a.txt", 1, "aa")]);
    let text = String::from_utf8(manifest)
        .unwrap()
        .replace("format: 1", "format: 9");
    build_archive(&tampered, "a.txt", b"x", text.into_bytes());
    let error = import_never_running(&target, &tampered).unwrap_err();
    assert!(
        error.contains("unsupported archive format"),
        "error: {error}"
    );
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
}

/// Builds a one-file archive with the given manifest bytes as the last entry.
fn build_archive(dest: &Path, name: &str, contents: &[u8], manifest: Vec<u8>) {
    let mut writer = ZipWriter::new(Vec::new(), 1_800_000_000);
    writer.add(name, contents).unwrap();
    writer.add(MANIFEST_NAME, &manifest).unwrap();
    let bytes = writer.finish().unwrap();
    fs::write(dest, bytes).unwrap();
}

#[test]
fn import_rejects_every_unsafe_entry_path() {
    let bad = [
        "/etc/passwd",
        "..\\..\\win.ini",
        "a\\b.txt",
        "C:evil.txt",
        "c:\\windows",
        r"\\server\share",
        "../up.txt",
        "a/../../up.txt",
        "./here.txt",
        "a//b.txt",
        "con.txt",
        "CON",
        "aux.log",
        "nul.txt",
        "com1.dat",
        "LPT9.prn",
        "dir/nul",
        "bad\u{0}name.txt",
        "bad\u{1f}name.txt",
    ];
    for path in bad {
        let scratch = Scratch::new("unsafe");
        let dest = scratch.path().join("bad.zip");
        let manifest = manifest_bytes("org.example.app", vec![entry(path, 1, "aa")]);
        build_archive(&dest, path, b"x", manifest);
        let target = scratch.path().join("target");
        fs::create_dir_all(&target).unwrap();
        let error = import_never_running(&target, &dest);
        assert!(error.is_err(), "accepted unsafe path {path:?}");
        assert!(
            fs::read_dir(&target).unwrap().count() == 0,
            "path {path:?} reached the disk"
        );
    }
    // Duplicates that differ only in case are refused too.
    let scratch = Scratch::new("dupcase");
    let dest = scratch.path().join("dup.zip");
    let manifest = manifest_bytes(
        "org.example.app",
        vec![entry("a/file.txt", 1, "aa"), entry("A/FILE.txt", 1, "aa")],
    );
    build_archive(&dest, "a/file.txt", b"x", manifest);
    let target = scratch.path().join("target");
    fs::create_dir_all(&target).unwrap();
    let error = import_never_running(&target, &dest).unwrap_err();
    assert!(error.contains("duplicate"), "error: {error}");
    // A second entry named like the manifest collides.
    let dest = scratch.path().join("self.zip");
    let manifest = manifest_bytes("org.example.app", vec![entry(MANIFEST_NAME, 1, "aa")]);
    build_archive(&dest, "x.txt", b"x", manifest);
    let error = import_never_running(&target, &dest).unwrap_err();
    assert!(error.contains("lists itself"), "error: {error}");
    // Two entries named like the manifest are refused before either is read.
    let dest = scratch.path().join("twin.zip");
    let manifest = manifest_bytes("org.example.app", vec![entry("x.txt", 1, "aa")]);
    build_archive(&dest, MANIFEST_NAME, b"x", manifest);
    let error = import_never_running(&target, &dest).unwrap_err();
    assert!(error.contains("duplicate"), "error: {error}");
}

#[test]
fn import_rejects_entry_count_and_expansion_limits() {
    let scratch = Scratch::new("limits");
    let target = scratch.path().join("target");
    fs::create_dir_all(&target).unwrap();
    // More files than the limit: one synthetic manifest with too many entries.
    let mut files = Vec::new();
    for i in 0..(super::MAX_FILES + 1) {
        files.push(entry(&format!("f/{i}.txt"), 1, "aa"));
    }
    let dest = scratch.path().join("count.zip");
    build_archive(
        &dest,
        "f/0.txt",
        b"x",
        manifest_bytes("org.example.app", files),
    );
    let error = import_never_running(&target, &dest).unwrap_err();
    assert!(error.contains("more than"), "error: {error}");
    // A highly compressible entry is an application's preallocated file, not a bomb: it imports.
    let zeros_zip = scratch.path().join("zeros.zip");
    let mut writer = ZipWriter::new(Vec::new(), 1_800_000_000);
    let zeros = vec![0u8; 24 << 20];
    writer.add("store/0.jnl", &zeros).unwrap();
    writer
        .add(
            MANIFEST_NAME,
            &manifest_bytes(
                "org.example.app",
                vec![entry(
                    "store/0.jnl",
                    zeros.len() as u64,
                    &super::sha256_hex(&zeros),
                )],
            ),
        )
        .unwrap();
    fs::write(&zeros_zip, writer.finish().unwrap()).unwrap();
    let fresh = scratch.path().join("fresh");
    import_never_running(&fresh, &zeros_zip).unwrap();
    assert_eq!(
        fs::metadata(fresh.join("store/0.jnl")).unwrap().len(),
        zeros.len() as u64
    );
}

#[test]
fn a_flipped_byte_refuses_the_whole_import_and_leaves_the_data_alone() {
    let scratch = Scratch::new("corrupt");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"original");
    let dest = export(&data, "org.example.app", &scratch, "corrupt");
    let mut bytes = fs::read(&dest).unwrap();
    let at = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 1, 2])
        .unwrap();
    bytes[at + 16] ^= 0xff; // the CRC32 field of the first central directory entry
    fs::write(&dest, &bytes).unwrap();
    let error = super::import_with(&data, "org.example.app", &dest, never_running).unwrap_err();
    assert!(
        error.contains("integrity") || error.contains("checksum"),
        "error: {error}"
    );
    assert_eq!(fs::read(data.join("a.txt")).unwrap(), b"original");
    assert!(!data.with_file_name("data.before-import").exists());
}

#[test]
fn a_failed_backup_is_replaced_and_a_failed_swap_restores_the_old_data() {
    let scratch = Scratch::new("rollback");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"original");
    // A stale file where the backup goes: the export succeeds and the stale file is replaced.
    fs::write(data.with_file_name("data.before-import"), b"stale").unwrap();
    let dest = export(&data, "org.example.app", &scratch, "rollback");
    let stats = import_never_running(&data, &dest).unwrap();
    assert_eq!(stats.files, 1);
    assert!(stats.backup.as_ref().unwrap().is_dir());
    assert_eq!(
        fs::read(data.with_file_name("data.before-import").join("a.txt")).unwrap(),
        b"original"
    );
    // A rename that cannot place the staging directory rolls the backup back.
    let old = scratch.path().join("swap-data");
    let backup = scratch.path().join("swap-data.before-import");
    write_file(&backup.join("old.txt"), b"old");
    let missing_staging = scratch.path().join("swap-data.import-missing");
    let error = super::swap_in(&missing_staging, &old, &backup).unwrap_err();
    assert!(error.contains("restored"), "error: {error}");
    assert_eq!(fs::read(old.join("old.txt")).unwrap(), b"old");
    assert!(!backup.exists());
    // The staging directory of a full failed import is cleaned up.
    let scratch = Scratch::new("rollback2");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"original");
    let dest = export(&data, "org.example.app", &scratch, "rollback2");
    let mut bytes = fs::read(&dest).unwrap();
    let at = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4b, 1, 2])
        .unwrap();
    bytes[at + 16] ^= 0xff; // the CRC32 field of the first central directory entry
    fs::write(&dest, &bytes).unwrap();
    assert!(super::import_with(&data, "org.example.app", &dest, never_running).is_err());
    let leftovers: Vec<_> = fs::read_dir(scratch.path())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().contains(".import-"))
        .collect();
    assert!(leftovers.is_empty(), "staging left behind: {leftovers:?}");
}

#[test]
fn zip_round_trips_and_rejects_truncation() {
    let scratch = Scratch::new("zip");
    let dest = scratch.path().join("rt.zip");
    let mut writer = ZipWriter::new(Vec::new(), 1_800_000_000);
    writer.add("empty.txt", b"").unwrap();
    writer.add("text.txt", b"hello hello hello").unwrap();
    let big = vec![7u8; 300_000];
    writer.add("big.bin", &big).unwrap();
    writer.add("folder/", b"").unwrap();
    let bytes = writer.finish().unwrap();
    fs::write(&dest, &bytes).unwrap();
    let parsed = crate::apps::zip::ZipArchive::parse(&bytes).unwrap();
    let names: Vec<_> = parsed
        .entries()
        .map(|e| (e.name.to_owned(), e.is_dir()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("empty.txt".to_owned(), false),
            ("text.txt".to_owned(), false),
            ("big.bin".to_owned(), false),
            ("folder/".to_owned(), true),
        ]
    );
    assert_eq!(parsed.read(&bytes, "empty.txt", 1024).unwrap(), b"");
    assert_eq!(
        parsed.read(&bytes, "text.txt", 1024).unwrap(),
        b"hello hello hello"
    );
    assert_eq!(parsed.read(&bytes, "big.bin", 1 << 20).unwrap(), big);
    assert!(
        parsed.read(&bytes, "big.bin", 1000).is_err(),
        "the cap must hold"
    );
    assert!(parsed.read(&bytes, "missing.txt", 1024).is_err());
    for cut in [0, 5, 20, bytes.len() / 2, bytes.len() - 25] {
        assert!(
            crate::apps::zip::ZipArchive::parse(&bytes[..cut]).is_err(),
            "truncation to {cut} was accepted"
        );
    }
    // One flipped byte in the stored CRC of a payload breaks the checksum.
    let mut flipped = bytes.clone();
    // The central directory comes last: its entry names the file for the second time.
    let at = flipped.windows(8).rposition(|w| w == b"text.txt").unwrap();
    flipped[at - 30] ^= 0xff; // the CRC32 field of that central entry
    let parsed = crate::apps::zip::ZipArchive::parse(&flipped).unwrap();
    assert!(parsed.read(&flipped, "text.txt", 1024).is_err());
    // A truncated EOCD (bad comment length) is rejected.
    let mut broken = bytes.clone();
    let end = broken.len() - 22;
    broken[end + 20] = 0xff;
    broken[end + 21] = 0xff;
    assert!(crate::apps::zip::ZipArchive::parse(&broken).is_err());
}

#[test]
fn inspect_reports_archived_totals_and_rejects_a_missing_manifest() {
    let scratch = Scratch::new("inspect");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"12345");
    let dest = export(&data, "org.example.app", &scratch, "inspect");
    let stats = inspect_archive(&dest, "org.example.app").unwrap();
    assert_eq!(stats.files, 1);
    assert_eq!(stats.bytes, 5);
    // An archive without a manifest is refused by the inspection itself.
    let bare = scratch.path().join("bare.zip");
    let mut writer = ZipWriter::new(Vec::new(), 1_800_000_000);
    writer.add("a.txt", b"12345").unwrap();
    fs::write(&bare, writer.finish().unwrap()).unwrap();
    let error = inspect_archive(&bare, "org.example.app").unwrap_err();
    assert!(error.contains(MANIFEST_NAME), "error: {error}");
}

#[test]
fn import_into_a_fresh_directory_needs_no_backup() {
    let scratch = Scratch::new("fresh");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"payload");
    let dest = export(&data, "org.example.app", &scratch, "fresh");
    let target = scratch.path().join("brand-new");
    let stats = import_never_running(&target, &dest).unwrap();
    assert_eq!(stats.files, 1);
    assert!(stats.backup.is_none());
    assert_eq!(fs::read(target.join("a.txt")).unwrap(), b"payload");
}

#[test]
fn dos_times_and_rfc3339_come_out_sane() {
    assert_eq!(crate::apps::zip::dos_datetime(0), (33, 0)); // before 1980 clamps to 1980-01-01
    assert_eq!(
        crate::apps::zip::dos_datetime(1_800_000_000),
        (0x5E2F, 0x4000)
    ); // 2027-01-15 08:00:00 UTC
    assert_eq!(super::rfc3339(0), "1970-01-01T00:00:00Z");
    assert_eq!(super::rfc3339(1_792_800_000), "2026-10-24T00:00:00Z");
}

#[test]
fn data_stats_defaults_are_empty() {
    let stats = DataStats::default();
    assert_eq!(stats.files, 0);
    assert_eq!(stats.bytes, 0);
    assert!(stats.backup.is_none());
}

#[test]
fn a_held_shared_lock_makes_the_import_see_a_running_application() {
    let scratch = Scratch::new("lock");
    let data = scratch.path().join("data");
    fs::create_dir_all(&data).unwrap();
    assert!(!super::data_lock_held(&data));
    let held = super::hold_running(&data).unwrap();
    assert!(super::data_lock_held(&data));
    drop(held);
    assert!(!super::data_lock_held(&data));
}

#[test]
fn the_web_profile_stays_with_the_application_through_an_import() {
    let scratch = Scratch::new("profile");
    let data = scratch.path().join("data");
    write_file(&data.join("a.txt"), b"original");
    let dest = export(&data, "org.example.app", &scratch, "profile");
    write_file(&data.join("servo/cookies"), b"kept");
    write_file(&data.join("a.txt"), b"changed");
    import_never_running(&data, &dest).unwrap();
    assert_eq!(fs::read(data.join("a.txt")).unwrap(), b"original");
    assert_eq!(fs::read(data.join("servo/cookies")).unwrap(), b"kept");
}
