//! Covers what this crate adds to its two libraries: that a name has to pass both, which parts of a path are names,
//! and the rule of its own that a path nobody here chose is held to.
//!
//! What each library rejects is its own tests' to prove; one name per library is enough to show it is consulted.
#![cfg(test)]

use std::path::Path;

use et_sanitise::{is_plain_name, is_plain_path, is_quotable_path};

#[test]
fn a_name_passes_only_when_both_libraries_accept_it() {
    assert!(is_plain_name("math1-sender_v2.0"));
    // sanitize-filename's Windows rules, applied whatever the host.
    assert!(!is_plain_name("con"));
    // shlex's bare words, which sanitize-filename alone would allow.
    assert!(!is_plain_name("mod$(id)"));
    // ASCII, which both libraries would allow.
    assert!(!is_plain_name("r\u{e9}sum\u{e9}"));
}

#[test]
fn a_path_is_checked_name_by_name() {
    assert!(is_plain_path(Path::new("/modules/./mine/../math1")));
    assert!(!is_plain_path(Path::new("modules/my mod")));
}

#[test]
fn a_name_that_is_not_utf8_is_not_plain() {
    // A lone byte that starts no UTF-8 sequence, or a lone surrogate, as each platform spells a name.
    #[cfg(unix)]
    let name: std::ffi::OsString = std::os::unix::ffi::OsStringExt::from_vec(vec![0xff]);
    #[cfg(windows)]
    let name: std::ffi::OsString = std::os::windows::ffi::OsStringExt::from_wide(&[0xD800]);
    assert!(!is_plain_path(Path::new(&name)));
    assert!(!is_quotable_path(Path::new(&name)));
}

#[test]
fn a_quotable_path_keeps_what_double_quotes_leave_alone() {
    // What a home or temporary directory holds, which a bare word could not: a space, a `~`, a Windows `\`.
    for path in [
        r"C:\Users\RUNNER~1\AppData\Local\Temp",
        "/home/Jo Smith/my mods (old)",
        "/srv/~app/a;b'c",
    ] {
        assert!(is_quotable_path(Path::new(path)));
    }
}

#[test]
fn a_quotable_path_refuses_what_double_quotes_and_lists_do_not() {
    for path in ["/case\";id;echo \"", "/srv/$HOME", "/srv/`id`", "/srv/a,b", "/srv/a\nb"] {
        assert!(!is_quotable_path(Path::new(path)));
    }
}
