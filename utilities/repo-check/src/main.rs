//! Repository-wide checks that need to read git itself, which no file-at-a-time linter can do.
//!
//! The repo's linters each read one file and judge it alone. That covers almost everything this repo wants to
//! enforce, and a rule belongs there whenever it can be written there. What is left over is the small set of
//! questions whose answer lives in the object database or the commit graph rather than in any file, and this
//! binary is where those go.
//!
//! Every check runs rather than stopping at the first failure, so one run reports the whole picture instead of
//! revealing the next problem only after the previous one is fixed.

use std::process::Command;

use command_error::CommandExt as _;

mod commit_hashes;
mod error;

use self::error::Error;

/// Every path git tracks, which is the same set the external analyzers see.
#[expect(
    clippy::single_call_fn,
    reason = "shared input every check reads; kept separate from any one of them"
)]
fn tracked_files() -> Result<Vec<String>, Error> {
    let listing = Command::new("git").args(["ls-files", "-z"]).output_checked()?;
    let text = String::from_utf8(listing.stdout)?;
    Ok(text
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect())
}

fn main() -> Result<(), Error> {
    let files = tracked_files()?;
    let failures = commit_hashes::run(&files)?;
    if failures == 0 {
        return Ok(());
    }
    Err(Error::Failures(failures))
}
