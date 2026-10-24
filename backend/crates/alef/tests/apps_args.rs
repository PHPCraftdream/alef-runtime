// SPDX-License-Identifier: MIT OR Apache-2.0
//! Application manager command line parsing.
use std::ffi::OsString;

use alef_launch::args::{parse_args, Command, USAGE};

fn parse(args: &[&str]) -> Result<Command, String> {
    parse_args(args.iter().map(OsString::from))
}

#[test]
fn apps_ui_is_a_distinct_mode_exclusive_of_json() {
    assert_eq!(parse(&["apps", "--ui"]).unwrap(), Command::AppsUi);
    assert_eq!(
        parse(&["apps", "--json"]).unwrap(),
        Command::Apps { json: true }
    );
    assert_eq!(parse(&["apps"]).unwrap(), Command::Apps { json: false });
    for args in [
        &["apps", "--ui", "--json"][..],
        &["apps", "--json", "--ui"][..],
        &["apps", "--ui", "x"][..],
    ] {
        assert!(parse(args).is_err());
    }
    assert!(USAGE.contains("alef apps [--json|--ui]"));
}

#[test]
fn apps_export_takes_two_positionals() {
    assert_eq!(
        parse(&["apps", "export", "org.example.app", "data.zip"]).unwrap(),
        Command::AppsExport {
            app: "org.example.app".into(),
            archive: "data.zip".into()
        }
    );
    for args in [
        &["apps", "export"][..],
        &["apps", "export", "org.example.app"][..],
        &["apps", "export", "a", "b", "c"][..],
        &["apps", "export", "a", "--json"][..],
    ] {
        assert!(parse(args).is_err());
    }
}

#[test]
fn apps_import_takes_two_positionals_then_flags_in_any_order() {
    assert_eq!(
        parse(&[
            "apps",
            "import",
            "org.example.app",
            "data.zip",
            "--confirm-red"
        ])
        .unwrap(),
        Command::AppsImport {
            app: "org.example.app".into(),
            archive: "data.zip".into(),
            confirm_red: true,
            json: false
        }
    );
    assert_eq!(
        parse(&["apps", "import", "a", "b", "--json", "--confirm-red"]).unwrap(),
        Command::AppsImport {
            app: "a".into(),
            archive: "b".into(),
            confirm_red: true,
            json: true
        }
    );
    assert_eq!(
        parse(&["apps", "import", "a", "b"]).unwrap(),
        Command::AppsImport {
            app: "a".into(),
            archive: "b".into(),
            confirm_red: false,
            json: false
        }
    );
}

#[test]
fn apps_import_rejects_repeated_and_unknown_flags() {
    for args in [
        &["apps", "import", "a", "b", "--confirm-red", "--confirm-red"][..],
        &["apps", "import", "a", "b", "--json", "--json"][..],
        &["apps", "import", "a", "b", "--frobnicate"][..],
        &["apps", "import"][..],
        &["apps", "import", "a"][..],
    ] {
        assert!(parse(args).is_err());
    }
}

#[test]
fn apps_subcommand_usage_lists_export_and_import() {
    assert!(USAGE.contains("alef apps export <PATH|ID> <FILE.zip>"));
    assert!(USAGE.contains("alef apps import <PATH|ID> <FILE.zip> [--confirm-red] [--json]"));
}
