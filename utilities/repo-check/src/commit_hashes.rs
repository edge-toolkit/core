//! Rejects a commit hash written into a tracked file when this repository's history has no such commit.
//!
//! The repo's workaround rules ask every papered-over failure to record the commit it was observed on, so hashes
//! accumulate in comments across configs, Dockerfiles, workflows and prose. A hash is only evidence while it resolves:
//! once it does not, the reader cannot tell whether the note describes something real, and the pointer costs more than
//! it gives.
//!
//! A hash that fails here is usually not wrong. Pull requests merge by squash, so a commit written down while a branch
//! was in flight stops existing the moment it lands, even though GitHub still serves it forever. The fix is therefore
//! to rewrite the reference as the web URL this prints, which survives the squash, rather than to hunt for a
//! replacement hash.
//!
//! Membership is decided by `git rev-list --all`, which is reachability from a ref, and deliberately not by asking
//! whether the object exists. The two differ exactly where it matters: a workstation that once fetched a pull request
//! keeps those objects long after the branch is deleted, so `git cat-file` still answers `commit` for a hash that a
//! fresh clone cannot resolve at any depth. Judging by object presence therefore passes locally and fails on CI, which
//! is the wrong way round for a check whose whole job is to be trustworthy about what a reader will find.
//!
//! Two shapes are deliberately not checked. A hash already written as a `/commit/<sha>` URL is a reference to whatever
//! repository the URL names, which may not be this one. A hash written as a quoted string value is a pin some code
//! consumes -- an upstream revision, a gist revision -- and naming a foreign object is the whole point of it. Both
//! would otherwise fail here for being exactly what they are meant to be.

use std::collections::{BTreeMap, HashSet};
use std::process::Command;

use command_error::CommandExt as _;
use fs_err as fs;
use regex::Regex;

use crate::error::Error;

/// Marks a hash already written as a web reference, which may name a repository other than this one.
const URL_MARKER: &str = "/commit/";

/// Where one hash was written, so a failure can name the line a reader has to edit.
struct Site {
    path: String,
    line: usize,
}

/// Whether this hash is one of the two shapes that name something outside this repository on purpose.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the scan; kept separate for readability"
)]
fn is_foreign(line: &str, start: usize, end: usize) -> bool {
    let before = line.get(..start).unwrap_or_default();
    let after = line.get(end..).unwrap_or_default();
    before.ends_with(URL_MARKER) || (before.ends_with('"') && after.starts_with('"'))
}

/// Every bare hash in the tracked set, mapped to the places it is written.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the scan; kept separate for readability"
)]
fn collect(files: &[String]) -> Result<BTreeMap<String, Vec<Site>>, Error> {
    let pattern = Regex::new(r"\b[0-9a-f]{40}\b")?;
    let mut found: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    for path in files {
        // A file that does not read as UTF-8 is binary, and carries no prose to check.
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (offset, line) in text.lines().enumerate() {
            for hit in pattern.find_iter(line) {
                if is_foreign(line, hit.start(), hit.end()) {
                    continue;
                }
                let site = Site {
                    path: path.clone(),
                    line: offset.saturating_add(1),
                };
                found.entry(hit.as_str().to_owned()).or_default().push(site);
            }
        }
    }
    Ok(found)
}

/// Every commit on the permanent history, which is what "in the history" has to mean for this to be reproducible.
///
/// `origin/main` rather than `--all`, because `--all` answers differently in each place it runs: a workstation
/// keeps remote-tracking refs for branches the remote deleted long ago, so it resolves hashes a fresh clone
/// cannot, and the check then passes locally and fails on CI. `origin/main` is the one ref every checkout agrees
/// on. It is also the honest bar: a hash that is only reachable from the branch in flight stops resolving the
/// moment that branch squash-merges, so accepting it now would just defer the failure.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the scan; kept separate for readability"
)]
/// `None` where the ref is absent, which is a checkout this check cannot speak about rather than a failure.
/// The docker image builds copy the tree in and run the battery inside the container, where there is no remote
/// and so no `origin/main`; a hard error there would fail a lane over the shape of its checkout instead of over
/// anything in the repo.
fn permanent_commits() -> Result<Option<HashSet<String>>, Error> {
    if Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "origin/main"])
        .output_checked()
        .is_err()
    {
        return Ok(None);
    }
    let listing = Command::new("git").args(["rev-list", "origin/main"]).output_checked()?;
    let text = String::from_utf8(listing.stdout)?;
    Ok(Some(text.lines().map(str::to_owned).collect()))
}

/// The `owner/repo` this checkout pushes to, so the suggested replacement URL points at the right place.
#[expect(
    clippy::single_call_fn,
    reason = "distinct step of the report; kept separate for readability"
)]
fn origin_slug() -> Option<String> {
    let remote = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output_checked()
        .ok()?;
    let url = String::from_utf8(remote.stdout).ok()?;
    let trimmed = url.trim().trim_end_matches(".git");
    let tail = trimmed
        .rsplit_once("github.com")
        .map(|(_, rest)| rest.trim_start_matches([':', '/']))?;
    Some(tail.to_owned())
}

/// Reports every unreachable hash and returns how many there were.
pub(crate) fn run(files: &[String]) -> Result<usize, Error> {
    let hashes = collect(files)?;
    let Some(known) = permanent_commits()? else {
        println!("repo-check commit-hashes: no origin/main to judge against in this checkout; skipping");
        return Ok(0);
    };
    let missing: Vec<&String> = hashes.keys().filter(|hash| !known.contains(*hash)).collect();
    if missing.is_empty() {
        println!("repo-check commit-hashes: {} hashes, all reachable", hashes.len());
        return Ok(0);
    }
    let slug = origin_slug().unwrap_or_else(|| "<owner>/<repo>".to_owned());
    for hash in &missing {
        for site in hashes.get(*hash).map(Vec::as_slice).unwrap_or_default() {
            println!("{}:{}: no commit {hash} in this repository", site.path, site.line);
        }
        println!("    write it as https://github.com/{slug}/commit/{hash} if the commit is on a merged branch");
    }
    println!(
        "repo-check commit-hashes: {} of {} hashes are unreachable",
        missing.len(),
        hashes.len()
    );
    Ok(missing.len())
}
