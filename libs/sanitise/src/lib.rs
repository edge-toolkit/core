//! Checks that a name someone chose can be used as written wherever the workspace puts it.
//!
//! A name is plain when it is ASCII and two libraries each accept it unchanged: sanitize-filename, with its Windows
//! rules on whatever the host, so a name holds on every filesystem the workspace supports; and shlex, so a generated
//! shell command can name it as a bare word, with nothing in it that a shell would expand, split or run. Checking
//! rather than quoting keeps every generated file readable, and leaves no generator to forget the quoting.
//!
//! A path nobody here chose -- the directory a scenario happens to sit in, such as a Windows temporary directory under
//! `RUNNER~1` -- is held to a narrower rule instead, which no library states: that it reads back unchanged inside a
//! double-quoted shell string, where every generated command puts it, and inside a comma-separated list.

use std::borrow::Cow;
use std::path::{Component, Path};

use sanitize_filename::OptionsForCheck;

/// Whether `name`, one file or directory name, is used as written on every filesystem and in every shell command.
///
/// It has to be ASCII as well, which neither library asks: a non-ASCII name can be spelled in more than one Unicode
/// normal form, and filesystems disagree on whether those are the same name.
#[must_use]
pub fn is_plain_name(name: &str) -> bool {
    let portable = OptionsForCheck {
        windows: true,
        truncate: true,
    };
    name.is_ascii()
        && sanitize_filename::is_sanitized_with_options(name, portable)
        && matches!(shlex::try_quote(name), Ok(Cow::Borrowed(_)))
}

/// Whether every name `path` is made of is plain, and valid UTF-8.
///
/// The parts a platform supplies rather than a person -- a root, a Windows drive prefix, `.` and `..` -- are not names,
/// and are not checked.
#[must_use]
pub fn is_plain_path(path: &Path) -> bool {
    path.components().all(|component| match component {
        Component::Normal(name) => name.to_str().is_some_and(is_plain_name),
        Component::Prefix(_) | Component::RootDir | Component::CurDir | Component::ParentDir => true,
    })
}

/// Whether `path`, valid UTF-8, reads back unchanged inside a double-quoted shell string and a comma-separated list.
///
/// Inside double quotes a shell still expands `$` and a backtick, and a `"` ends the string, so all three are refused;
/// with them gone, a `\` -- the Windows separator -- escapes nothing and stays. A `,` would split the path in two in a
/// list such as `MODULES_PATHS`, and a control character, a newline among them, ends the command.
#[must_use]
pub fn is_quotable_path(path: &Path) -> bool {
    path.to_str().is_some_and(|text| {
        !text
            .chars()
            .any(|character| matches!(character, '"' | '$' | '`' | ',') || character.is_control())
    })
}
