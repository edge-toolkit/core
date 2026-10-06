//! Render each binary crate's `HELP.md`: its clap command tree, and the environment variables its config reads.
//!
//! The crates are found from the workspace manifests rather than listed by hand. A crate with a binary target that
//! depends on `clap` gets the clap section, which clap-markdown writes from the command tree linked in
//! [`clap_commands`]. One that loads its config with serde-env -- directly, or through `et-otlp`'s
//! `load_telemetered` -- gets the environment section, rendered here from the config's JSON Schema linked in
//! [`env_schemas`], with every nested field named the way serde-env reads it: `ws.max_frame_size` is
//! `WS_MAX_FRAME_SIZE`. A crate this finds but has no tree or schema for fails the run, naming what is missing, so a
//! new binary cannot quietly go without a `HELP.md`; so does an entry left behind for a crate that no longer needs one.
//!
//! The result goes through the repo's markdown formatter, so each file is byte-identical to what that formatter would
//! leave on disk.
#![expect(
    clippy::single_call_fn,
    reason = "each of render()'s steps is a named function for readability; most are called from one place"
)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::CommandFactory as _;
use command_error::{ChildExt as _, CommandExt as _};
use serde_json::Value;

use crate::Error;

/// Dependencies through which a binary loads its configuration from the environment with serde-env.
///
/// `et-otlp` counts because its `load_telemetered` is how the runners do it.
const ENV_LOADERS: [&str; 2] = ["serde-env", "et-otlp"];

/// Width the environment section's prose is wrapped to, matching the repo's markdown line limit.
const LINE_WIDTH: usize = 120;

/// What an environment-only binary's `HELP.md` says after naming it.
const ENV_ONLY: &str = "takes no command-line arguments; it reads its configuration from the environment.";

/// A workspace crate with a binary target, as far as `HELP.md` generation needs to know it.
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct BinaryCrate {
    /// The package name.
    pub name: String,
    /// The crate directory, relative to the workspace root, where its `HELP.md` lives.
    pub dir: PathBuf,
    /// The binary target the `HELP.md` is headed with.
    pub bin: String,
    /// Whether the crate depends on `clap`.
    pub uses_clap: bool,
    /// Whether the crate reads configuration from the environment through serde-env.
    pub reads_env: bool,
}

/// The clap command trees et-int-gen links, keyed by package name.
#[must_use]
pub fn clap_commands() -> BTreeMap<&'static str, clap::Command> {
    BTreeMap::from([
        ("et-cli", et_cli::cli::Cli::command()),
        ("et-int-gen", crate::cli::Cli::command()),
        ("et-onnx", et_onnx::Args::command()),
        ("et-ws-server", et_ws_server::cli::Args::command()),
    ])
}

/// The environment configs' JSON Schemas et-int-gen links, keyed by package name.
pub fn env_schemas() -> Result<BTreeMap<&'static str, Value>, Error> {
    use et_ws_runner_common::{pyo3_config, wasi_config, web_config};
    use schemars::schema_for;
    Ok(BTreeMap::from([
        (
            "et-ws-pyo3-runner",
            serde_json::to_value(schema_for!(pyo3_config::Config))?,
        ),
        (
            "et-ws-server",
            serde_json::to_value(schema_for!(et_ws_server::config::Config))?,
        ),
        (
            "et-ws-wasi-runner",
            serde_json::to_value(schema_for!(wasi_config::Config))?,
        ),
        (
            "et-ws-web-runner",
            serde_json::to_value(schema_for!(web_config::Config))?,
        ),
    ]))
}

/// Every workspace member with a binary target, read from the manifests and sorted by name.
///
/// The manifests are read directly rather than through `cargo metadata`, which takes the workspace lock. Only normal
/// dependencies count, target-specific ones included, so a crate using clap or serde-env in its tests alone is left
/// out.
pub fn binary_crates(root: &Path) -> Result<Vec<BinaryCrate>, Error> {
    let workspace = read_manifest(&root.join("Cargo.toml"))?;
    let members = workspace
        .get("workspace")
        .and_then(|table| table.get("members"))
        .and_then(toml::Value::as_array)
        .ok_or(Error::ManifestMalformed(
            "the root manifest has no `[workspace].members`",
        ))?;
    let mut crates = Vec::new();
    for member in members {
        let dir = PathBuf::from(
            member
                .as_str()
                .ok_or(Error::ManifestMalformed("a non-string workspace member"))?,
        );
        let manifest = read_manifest(&root.join(&dir).join("Cargo.toml"))?;
        let name = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .ok_or(Error::ManifestMalformed("a member manifest has no `[package].name`"))?
            .to_owned();
        let bins = binary_names(&manifest, &root.join(&dir), &name)?;
        let Some(bin) = bins.into_iter().next() else {
            continue;
        };
        let deps = normal_dependencies(&manifest);
        crates.push(BinaryCrate {
            uses_clap: deps.contains(&"clap"),
            reads_env: deps.iter().any(|dep| ENV_LOADERS.contains(dep)),
            name,
            dir,
            bin,
        });
    }
    crates.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
    Ok(crates)
}

/// Parse a `Cargo.toml`.
fn read_manifest(path: &Path) -> Result<toml::Table, Error> {
    Ok(toml::from_str(&fs_err::read_to_string(path)?)?)
}

/// The names of a crate's binary targets, in the order cargo lists them.
///
/// Explicit `[[bin]]` tables come first; otherwise cargo's auto-discovery applies, which names `src/main.rs` after the
/// package and each `src/bin/*.rs` after its file stem.
fn binary_names(manifest: &toml::Table, dir: &Path, package: &str) -> Result<Vec<String>, Error> {
    let explicit: Vec<String> = manifest
        .get("bin")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|bin| bin.get("name").and_then(toml::Value::as_str))
        .map(str::to_owned)
        .collect();
    if !explicit.is_empty() {
        return Ok(explicit);
    }
    let mut discovered = Vec::new();
    if dir.join("src/main.rs").is_file() {
        discovered.push(package.to_owned());
    }
    let bin_dir = dir.join("src/bin");
    if bin_dir.is_dir() {
        let mut stems: Vec<String> = fs_err::read_dir(&bin_dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
            .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()).map(str::to_owned))
            .collect();
        stems.sort();
        discovered.extend(stems);
    }
    Ok(discovered)
}

/// The crate's normal dependency names: `[dependencies]` and every `[target.*.dependencies]`.
fn normal_dependencies(manifest: &toml::Table) -> Vec<&str> {
    let target_tables = manifest
        .get("target")
        .and_then(toml::Value::as_table)
        .into_iter()
        .flat_map(|targets| targets.values())
        .filter_map(|target| target.get("dependencies"));
    manifest
        .get("dependencies")
        .into_iter()
        .chain(target_tables)
        .filter_map(toml::Value::as_table)
        .flat_map(|table| table.keys().map(String::as_str))
        .collect()
}

/// Every documented crate's `HELP.md`, as its path relative to the repo root and its rendered content.
pub fn render(root: &Path) -> Result<Vec<(PathBuf, String)>, Error> {
    let mut commands = clap_commands();
    let mut schemas = env_schemas()?;
    let mut rendered = Vec::new();
    let mut problems = Vec::new();
    for krate in binary_crates(root)? {
        let command = commands.remove(krate.name.as_str());
        let schema = schemas.remove(krate.name.as_str());
        let name = &krate.name;
        match (&command, krate.uses_clap) {
            (None, true) => problems.push(format!(
                "{name} depends on clap but help.rs links no command tree for it"
            )),
            (Some(_), false) => problems.push(format!("{name} has a command tree in help.rs but no clap dependency")),
            (None, false) | (Some(_), true) => {}
        }
        match (&schema, krate.reads_env) {
            (None, true) => problems.push(format!(
                "{name} reads serde-env config but help.rs links no schema for it"
            )),
            (Some(_), false) => problems.push(format!("{name} has a schema in help.rs but reads no serde-env config")),
            (None, false) | (Some(_), true) => {}
        }
        if !(krate.uses_clap || krate.reads_env) || !problems.is_empty() {
            continue;
        }
        let markdown = describe(&krate, command, schema.as_ref())?;
        rendered.push((krate.dir.join("HELP.md"), format_markdown(root, &markdown)?));
    }
    for name in commands.keys().chain(schemas.keys()) {
        problems.push(format!("{name} has an entry in help.rs but no binary in the workspace"));
    }
    if problems.is_empty() {
        Ok(rendered)
    } else {
        Err(Error::HelpUndocumentable(problems.join("; ")))
    }
}

/// The unformatted `HELP.md` for `krate`: its clap tree when it has one, then its environment section.
fn describe(krate: &BinaryCrate, command: Option<clap::Command>, schema: Option<&Value>) -> Result<String, Error> {
    let bin = &krate.bin;
    let mut markdown = command.map_or_else(
        || format!("# Help for `{bin}`\n\n`{bin}` {ENV_ONLY}\n\n"),
        |command| clap_tree(&command),
    );
    if let Some(schema) = schema {
        markdown.push('\n');
        markdown.push_str(&render_env_section(schema)?);
    }
    Ok(markdown)
}

/// The clap-markdown rendering of `command`, reshaped into markdown that needs no inline HTML.
///
/// The footer crediting clap-markdown is dropped, since it is the document's only HTML. Each `###### **Options:**`
/// style label becomes a `### Options` heading one level under its command's `##`, where clap-markdown otherwise
/// jumps straight from the second level to the sixth.
#[must_use]
pub fn clap_tree(command: &clap::Command) -> String {
    let options = clap_markdown::MarkdownOptions::new().show_footer(false);
    let markdown = clap_markdown::help_markdown_command_custom(command, &options);
    let mut out = String::with_capacity(markdown.len());
    for line in markdown.split_inclusive('\n') {
        let label = line
            .strip_prefix("###### **")
            .and_then(|rest| rest.trim_end().strip_suffix(":**"));
        match label {
            Some(label) => {
                out.push_str("### ");
                out.push_str(label);
                out.push('\n');
            }
            None => out.push_str(line),
        }
    }
    out
}

/// One environment variable, flattened out of the config's schema.
struct EnvVar {
    name: String,
    kind: String,
    default: Option<String>,
    required: bool,
    description: String,
}

/// Render the `## Environment variables` section from a config's JSON Schema, one entry per variable by name.
pub fn render_env_section(schema: &Value) -> Result<String, Error> {
    let mut vars = Vec::new();
    collect_vars(schema, schema, &[], true, &mut vars);
    vars.sort_by(|left, right| left.name.cmp(&right.name));
    let mut out = String::from("## Environment variables\n\n");
    for var in vars {
        write!(out, "### `{}`\n\n{}", var.name, wrap(&var.summary()))?;
        for paragraph in var
            .description
            .split("\n\n")
            .filter(|paragraph| !paragraph.trim().is_empty())
        {
            out.push('\n');
            out.push_str(&wrap(&code_links_to_spans(paragraph)?));
        }
        out.push('\n');
    }
    Ok(out)
}

impl EnvVar {
    /// The variable's type, default and whether it must be set, as one sentence-style line.
    fn summary(&self) -> String {
        let tail = match (&self.default, self.required) {
            (Some(default), _) if default.is_empty() => " Default: empty.".to_owned(),
            (Some(default), _) => format!(" Default: `{default}`."),
            (None, true) => " Required.".to_owned(),
            (None, false) => String::default(),
        };
        format!("Type: {}.{tail}", self.kind)
    }
}

/// Walk `node`'s properties, recursing into nested structs and recording each leaf as an [`EnvVar`].
///
/// A nested struct's fields are prefixed with the struct field's name, which is how serde-env names them. A field is
/// required only when every struct above it is required too, since an unset optional group needs none of its fields.
fn collect_vars(root: &Value, node: &Value, prefix: &[&str], required: bool, out: &mut Vec<EnvVar>) {
    let required_fields: Vec<&str> = node["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let Some(properties) = node["properties"].as_object() else {
        return;
    };
    for (field, property) in properties {
        let (target, nullable) = resolve(root, property);
        let mut path = prefix.to_vec();
        path.push(field);
        let field_required = required && required_fields.contains(&field.as_str()) && !nullable;
        if target["properties"].is_object() {
            collect_vars(root, target, &path, field_required, out);
            continue;
        }
        let default = property
            .get("default")
            .filter(|value| !value.is_null())
            .map(render_value);
        out.push(EnvVar {
            name: path.join("_").to_uppercase(),
            kind: kind_of(target),
            required: field_required && default.is_none(),
            default,
            description: description_of(property, target),
        });
    }
}

/// The schema a property stands for, and whether the property may be left unset.
///
/// A `$ref` is followed whether it is bare or the non-null arm of an `anyOf`.
fn resolve<'schema>(root: &'schema Value, property: &'schema Value) -> (&'schema Value, bool) {
    let nullable_type = property["type"]
        .as_array()
        .is_some_and(|types| types.iter().any(|name| name == "null"));
    if let Some(arms) = property["anyOf"].as_array() {
        let nullable = arms.iter().any(|arm| arm["type"] == "null");
        let target = arms.iter().find(|arm| arm["type"] != "null").unwrap_or(property);
        return (follow_ref(root, target), nullable || nullable_type);
    }
    (follow_ref(root, property), nullable_type)
}

/// `node` itself, or the `$defs` entry its `$ref` points at.
fn follow_ref<'schema>(root: &'schema Value, node: &'schema Value) -> &'schema Value {
    let defs = &root["$defs"];
    node["$ref"]
        .as_str()
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
        .and_then(|name| defs.get(name))
        .unwrap_or(node)
}

/// A short description of a leaf schema's accepted values.
fn kind_of(schema: &Value) -> String {
    if let Some(variants) = schema["oneOf"].as_array().or_else(|| schema["enum"].as_array()) {
        let values: Vec<String> = variants
            .iter()
            .filter_map(|variant| variant["const"].as_str().or_else(|| variant.as_str()))
            .map(|value| format!("`{value}`"))
            .collect();
        return format!("one of {}", values.join(", "));
    }
    let types: Vec<&str> = match &schema["type"] {
        Value::String(single) => vec![single.as_str()],
        Value::Array(many) => many
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| *name != "null")
            .collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => Vec::new(),
    };
    match types.as_slice() {
        ["array"] => format!("comma-separated list of {}", kind_of(&schema["items"])),
        [] => "string".to_owned(),
        _ => types.join(" or "),
    }
}

/// The property's own description, or the description of the type it points at.
fn description_of(property: &Value, target: &Value) -> String {
    property["description"]
        .as_str()
        .or_else(|| target["description"].as_str())
        .unwrap_or_default()
        .to_owned()
}

/// A default value as the environment variable would spell it.
fn render_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(render_value).collect::<Vec<_>>().join(","),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => value.to_string(),
    }
}

/// Rustdoc's intra-doc links, ``[`path`]``, as plain code spans: nothing in a `HELP.md` resolves them.
///
/// A bracketed span followed by `(` is an ordinary markdown link and is kept whole.
fn code_links_to_spans(text: &str) -> Result<String, Error> {
    let intra_doc = regex::Regex::new(r"\[(`[^`\]]+`)\](\(?)")?;
    Ok(intra_doc
        .replace_all(text, |captures: &regex::Captures<'_>| {
            let whole = captures.get(0).map_or("", |found| found.as_str());
            let span = captures.get(1).map_or("", |found| found.as_str());
            let opens_link = captures.get(2).is_some_and(|found| !found.as_str().is_empty());
            if opens_link { whole.to_owned() } else { span.to_owned() }
        })
        .into_owned())
}

/// One paragraph re-flowed to [`LINE_WIDTH`], ending in a newline.
fn wrap(paragraph: &str) -> String {
    let mut out = String::default();
    let mut line = String::default();
    for word in paragraph.split_whitespace() {
        if !line.is_empty() && line.len().saturating_add(1).saturating_add(word.len()) > LINE_WIDTH {
            out.push_str(&line);
            out.push('\n');
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    out.push_str(&line);
    out.push('\n');
    out
}

/// Format `markdown` as the repo's markdown formatter would, run from `root` so it finds the repo's config.
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
