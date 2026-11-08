//! The Jinja templates every hand-written text file of a generated deployment is rendered from.
//!
//! What a file says lives in its template under `templates/`, and what varies by scenario is worked out in Rust and
//! handed over as a serialisable context. Files with a schema of their own -- `mise.toml`, `compose.yaml`, the
//! Kubernetes manifests -- are still built as values, since a template would have to reimplement their quoting.

use std::fmt::Write as _;

use minijinja::syntax::SyntaxConfig;
use minijinja::value::{Serde, Value};
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::Serialize;

use crate::error::CliError;

/// Each template under its name, which is its path below `templates/`, with its source compiled into the binary.
///
/// Compiled in rather than read at run time, so an installed `et-cli` carries everything it renders from.
macro_rules! templates {
    ($($name:literal),+ $(,)?) => {
        [$(($name, include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/", $name)))),+]
    };
}

/// Every template, including the ones only ever included by another.
const TEMPLATES: [(&str, &str); 22] = templates![
    "gitignore.j2",
    "npmrc.j2",
    "readme.md.j2",
    "readme/addresses.md.j2",
    "readme/board-hub.md.j2",
    "readme/commands.j2",
    "readme/dekit.md.j2",
    "readme/docker-compose.md.j2",
    "readme/firmware.md.j2",
    "readme/install.md.j2",
    "readme/k3s-machines.md.j2",
    "readme/k3s.md.j2",
    "readme/load-board.md.j2",
    "readme/loopback.md.j2",
    "readme/machines.md.j2",
    "readme/mise.md.j2",
    "readme/process-compose.md.j2",
    "readme/run.md.j2",
    "readme/runner-board.md.j2",
    "scenario-image.dockerignore.j2",
    "scenario-image.j2",
    "secrets.env.j2",
];

/// Render the template `name` with `context`.
pub(crate) fn render<C>(name: &str, context: &C) -> Result<String, CliError>
where
    C: Serialize,
{
    let environment = environment()?;
    Ok(environment.get_template(name)?.render(Serde(context))?)
}

/// The environment every template renders in.
///
/// A block tag takes its own line with it, so a template reads as the file it renders rather than as a run of
/// `{%-` markers, and a file keeps the newline it ends with. A name the context does not carry fails the render instead
/// of printing nothing, which is how a template that drifted from its context is caught.
///
/// Nothing is escaped, whatever a template's name ends in. Every file here is read as itself -- Markdown, shell, a
/// Dockerfile -- and minijinja's own default picks escaping by extension and changes with the features the workspace
/// happens to unify on.
fn environment() -> Result<Environment<'static>, minijinja::Error> {
    let mut environment = Environment::new();
    environment.set_syntax(
        SyntaxConfig::builder()
            .trim_blocks(true)
            .lstrip_blocks(true)
            .keep_trailing_newline(true)
            .build()?,
    );
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    environment.set_auto_escape_callback(|_name| AutoEscape::None);
    environment.add_filter("code", code);
    environment.add_filter("fill", fill);
    environment.add_filter("flags", flags);
    environment.add_filter("quote", quote);
    for (name, source) in TEMPLATES {
        environment.add_template(name, source)?;
    }
    Ok(environment)
}

/// `text` as Markdown inline code.
fn code(text: &str) -> String {
    format!("`{text}`")
}

/// `text` in shell double quotes.
fn quote(text: &str) -> String {
    format!("\"{text}\"")
}

/// Each of `values` as the argument of a `flag` of its own, each led by the space that separates it from the command.
///
/// An empty list adds nothing, so a command naming the flag for each of a list writes it once whatever the list holds.
fn flags(values: &Value, flag: &str) -> Result<String, minijinja::Error> {
    Ok(values.try_iter()?.fold(String::default(), |mut flags, value| {
        let _write_result = write!(flags, " {flag} {value}");
        flags
    }))
}

/// `text` wrapped to the editorconfig line length every generated file is held to, broken only at spaces.
fn fill(text: &str) -> String {
    let options = textwrap::Options::new(120)
        .break_words(false)
        .word_separator(textwrap::WordSeparator::AsciiSpace)
        .word_splitter(textwrap::WordSplitter::NoHyphenation);
    textwrap::fill(text, options)
}
