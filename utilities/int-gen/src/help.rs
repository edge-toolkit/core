//! Render each utility's `HELP.md` from its clap command tree.
//!
//! clap-markdown writes the whole tree -- every subcommand, argument and default -- and the result goes through the
//! repo's markdown formatter, so the file is byte-identical to what that formatter would leave on disk.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::CommandFactory as _;
use command_error::{ChildExt as _, CommandExt as _};

use crate::Error;

/// Every utility's `HELP.md`, as its path relative to the repo root and its rendered content.
pub fn render(root: &Path) -> Result<Vec<(PathBuf, String)>, Error> {
    [
        ("utilities/cli/HELP.md", et_cli::cli::Cli::command()),
        ("utilities/int-gen/HELP.md", crate::cli::Cli::command()),
        ("utilities/onnx/HELP.md", et_onnx::Args::command()),
    ]
    .into_iter()
    .map(|(path, command)| {
        let markdown = clap_markdown::help_markdown_command(&command);
        Ok((PathBuf::from(path), format_markdown(root, &markdown)?))
    })
    .collect()
}

/// Format `markdown` as the repo's markdown formatter would, run from `root` so it finds the repo's config.
#[expect(
    clippy::single_call_fn,
    reason = "the formatter subprocess is a distinct step of render(); kept separate for its stdin handling"
)]
fn format_markdown(root: &Path, markdown: &str) -> Result<String, Error> {
    let mut child = Command::new("dprint")
        .args(["fmt", "--stdin", "md"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn_checked()?;
    // Scoped so stdin closes before the wait, which is what tells the formatter its input has ended.
    {
        let mut stdin = child
            .child_mut()
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("the formatter's stdin was not piped"))?;
        stdin.write_all(markdown.as_bytes())?;
    }
    Ok(child.output_checked_utf8()?.stdout)
}
