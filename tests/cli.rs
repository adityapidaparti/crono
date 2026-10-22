//! Command-line parsing that needs no session or network.

use std::process::Command;

fn crono(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_crono")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn day_and_export_take_the_day_with_or_without_d() {
    for cmd in ["day", "export"] {
        // A bad day is rejected the same way in both forms, before any login.
        for args in [vec![cmd, "notaday"], vec![cmd, "-d", "notaday"], vec![cmd, "--day", "notaday"]] {
            let (ok, err) = crono(&args);
            assert!(!ok && err.contains("notaday"), "{args:?}: {err}");
        }
        // Both forms at once is a usage error.
        let (ok, err) = crono(&[cmd, "yesterday", "-d", "today"]);
        assert!(!ok && err.contains("cannot be used with"), "{cmd}: {err}");
    }
}

#[test]
fn a_zeroth_search_result_is_a_usage_error() {
    let (ok, err) = crono(&["log", "hummus", "--pick", "0"]);
    assert!(!ok && err.contains("--pick"), "{err}");
}
