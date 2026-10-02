//! Render `CHECKS.md`, the catalogue of every check this repo runs.
//!
//! The source of truth is the task tables of every `.mise/config*.toml`, read as plain TOML rather than through
//! `mise tasks`: mise loads only the configs its `MISE_ENV` and the host OS select, and the document has to come out
//! identical whichever lane renders it. A check is any task the `check` / `check:<lang>` aggregates reach through
//! `depends`, plus any task named as a check that nothing reaches, which the document marks as run by hand. Each
//! check is filed under the tool its `run` body invokes -- the first command that is not shell plumbing -- or under
//! the shell-script section when it invokes none. Where a check's configuration is a set of rules this repo writes,
//! every rule is listed too, with the name and description its rule file gives it.
#![expect(
    clippy::single_call_fn,
    reason = "each helper is one named step of the render, kept separate so the steps read one at a time"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use command_error::CommandExt as _;
use fs_err as fs;
use regex::Regex;
use serde::Deserialize as _;

use crate::Error;

/// The line width the rendered document wraps to, which is the repo-wide editorconfig limit.
const WIDTH: usize = 120;

/// The section heading for checks whose `run` body invokes no tool beyond shell plumbing.
const SHELL_SECTION: &str = "Shell-script checks";

/// Commands that only move data around a check rather than performing it.
///
/// A command word in this list ends the search through that command, so its arguments are never mistaken for the tool:
/// in `git ls-files | xargs <tool>` the tool is the one `xargs` runs, and a body built only from these is a shell
/// script.
const PLUMBING: &[&str] = &[
    ":",
    "cat",
    "cd",
    "command",
    "coreutils",
    "cp",
    "cut",
    "echo",
    "exit",
    "export",
    "false",
    "find",
    "gh",
    "git",
    "goawk",
    "jaq",
    "jq",
    "local",
    "ls",
    "mise",
    "mkdir",
    "mv",
    "printf",
    "read",
    "return",
    "rg",
    "rm",
    "set",
    "shift",
    "sort",
    "tr",
    "true",
    "yq",
];

/// Shell words that introduce a command rather than being one, so the search continues past them.
const PREFIXES: &[&str] = &[
    "!", "do", "done", "elif", "else", "esac", "fi", "if", "then", "until", "while", "{", "}",
];

/// Configs a tool finds by itself, from the repo root or an `[env]` variable, rather than through its command line.
const IMPLICIT_CONFIGS: &[(&str, &[&str])] = &[
    ("cargo clippy", &["Cargo.toml", "config/clippy.toml"]),
    ("cargo fmt", &[".rustfmt.toml"]),
    ("dprint", &[".dprint.jsonc", "config/dprint.jsonc"]),
    ("editorconfig-checker", &[".editorconfig"]),
    ("ruff", &["ruff.toml"]),
    ("semgrep", &[".semgrepignore"]),
    ("taplo", &["config/taplo.toml"]),
];

/// For each check, the aggregates that list it in `depends`.
type Parents = BTreeMap<String, BTreeSet<String>>;

/// One task as one config file declares it.
struct Task {
    name: String,
    env: String,
    description: String,
    body: String,
    depends: Vec<String>,
}

/// One rule this repo writes, with the description its rule file gives it.
struct Rule {
    name: String,
    description: String,
    /// The rule's documentation page, or empty for a rule whose description is all there is.
    link: String,
}

/// The rules one section's checks are configured with.
struct SectionRules {
    /// The deepest directory every rule file shares.
    location: String,
    rendered: String,
    count: usize,
}

/// What a single shell command contributed to the search for a check's tool.
enum Found {
    Tool(String),
    Nothing,
}

/// Render the whole document from the configs and rule files under `root`.
pub fn render(root: &Path) -> Result<String, Error> {
    let tasks = load_tasks(&root.join(".mise"))?;
    let (reached, parents) = reachable(&tasks)?;
    let check_name = Regex::new("(?:^|[-:])check(?:$|[-:])|crosscheck")?;

    let mut sections: BTreeMap<String, Vec<&Task>> = BTreeMap::new();
    for task in &tasks {
        let aggregate = task.name == "check" || task.name.starts_with("check:");
        let wrapper = task.body.contains("mise run ");
        let listed = reached.contains(&task.name) || check_name.is_match(&task.name);
        if listed && !aggregate && !wrapper && !task.body.trim().is_empty() {
            sections.entry(runner_label(&task.body)?).or_default().push(task);
        }
    }

    let shell = sections.remove(SHELL_SECTION);
    let mut body = String::default();
    let mut rule_count = 0_usize;
    for (label, entries) in &sections {
        writeln!(body, "\n## `{label}`\n")?;
        render_entries(&mut body, label, entries, &parents)?;
        let rules = section_rules(root, label, entries)?;
        rule_count = rule_count.saturating_add(rules.count);
        if !rules.rendered.is_empty() {
            writeln!(body, "\nRules, from `{}`:\n", rules.location)?;
            body.push_str(&rules.rendered);
        }
    }
    let check_count = sections.values().chain(&shell).map(Vec::len).sum::<usize>();
    let shell_count = shell.as_ref().map_or(0, Vec::len);
    if let Some(entries) = shell {
        writeln!(body, "\n## {SHELL_SECTION}\n")?;
        render_entries(&mut body, SHELL_SECTION, &entries, &parents)?;
    }

    let mut out = String::from("# Checks\n\n");
    writeln!(out, "- **Checks:** {check_count}")?;
    writeln!(out, "- **Tools:** {}", sections.len())?;
    writeln!(out, "- **Shell-script checks:** {shell_count}")?;
    writeln!(
        out,
        "- **Rules:** {rule_count} custom local rules, or non-default strict linter settings\n"
    )?;
    let intro = [
        "Every check is listed across every `MISE_ENV`, filed under the tool it runs. Each entry names the env",
        "whose config declares it, the aggregate that runs it as part of `mise run check`, and the config files",
        "it reads. Generated by `mise run gen:checks` from the task tables in `.mise/config*.toml`, the rule",
        "files under `config/` and the `[workspace.lints]` in `Cargo.toml`; `mise run checks-md-check` fails when",
        "this file drifts from them.",
    ];
    out.push_str(&wrap("", "", &intro.join(" ")));
    out.push_str(&body);
    Ok(out)
}

/// Name the tool a `run` body invokes, or the shell-script section when it invokes none.
///
/// The search walks the body's commands in order and returns the first command word that is not shell plumbing. `cargo
/// <sub>` is reported with its subcommand, since each one is a different checker, and `cargo run -p <pkg>` as the
/// package it runs. An upper-case `$VAR` in command position is a tool the env hands over (`$MVN`), and a `$(...)`
/// substitution is searched like any other command.
pub fn runner_label(body: &str) -> Result<String, Error> {
    let assignment = Regex::new("^[A-Za-z_][A-Za-z0-9_]*=")?;
    let command_word = Regex::new("^[A-Za-z][A-Za-z0-9_.+-]*$")?;
    for command in commands(body) {
        if let Found::Tool(tool) = command_tool(&command, &assignment, &command_word)? {
            return Ok(tool);
        }
    }
    Ok(SHELL_SECTION.to_owned())
}

/// Every config file path a `run` body or its task's env names: `config/...` and the `.mise/` helper scripts.
pub fn config_paths(text: &str) -> Result<BTreeSet<String>, Error> {
    let path = Regex::new(r"(?:^|[^\w.-])((?:config/[\w./-]*\w)|(?:\.mise/[\w-]+\.(?:awk|jq|sh)))")?;
    Ok(path
        .captures_iter(text)
        .filter_map(|caps| caps.get(1))
        .map(|found| found.as_str().to_owned())
        .collect())
}

/// The summary line of every `deny` / `warn` rule in one Rego policy, in source order.
///
/// A rule's summary is the first line of the last top-level comment block before it within the same stretch of the file
/// -- the comments between the previous rule's closing brace and this rule. Every rule must have one: a rule with no
/// comment of its own is an error naming `file` and the rule's line, since the catalogue lists every rule by its
/// description.
pub fn rego_summaries(file: &str, source: &str) -> Result<Vec<String>, Error> {
    let mut summaries = Vec::new();
    let mut summary = String::default();
    let mut in_block = false;
    for (index, line) in source.lines().enumerate() {
        if let Some(comment) = line.strip_prefix('#') {
            if !in_block {
                comment.trim().clone_into(&mut summary);
            }
            in_block = true;
            continue;
        }
        in_block = false;
        if line.starts_with("deny") || line.starts_with("warn") {
            if summary.is_empty() {
                let line_number = index.saturating_add(1);
                return Err(Error::UndescribedRule(format!(
                    "{file}:{line_number}: rule has no summary comment; describe it in a comment directly above it"
                )));
            }
            summaries.push(core::mem::take(&mut summary));
        } else if line == "}" || line.starts_with("package ") {
            summary.clear();
        } else {
            // A helper definition between a rule's comment and the rule keeps that comment as the summary.
        }
    }
    Ok(summaries)
}

/// Fail unless a rule's description field is present and non-empty.
fn described(file: &Path, rule: &str, field: &str, value: &str) -> Result<(), Error> {
    if value.trim().is_empty() {
        let file = file.display();
        Err(Error::UndescribedRule(format!(
            "{file}: rule `{rule}` has no `{field}` describing it"
        )))
    } else {
        Ok(())
    }
}

/// Wrap `text` to the document width, the first line behind `first` and every later line behind `rest`.
#[must_use]
pub fn wrap(first: &str, rest: &str, text: &str) -> String {
    let mut out = String::default();
    let mut line = first.to_owned();
    let mut prefix_len = first.len();
    let mut empty = true;
    for word in text.split_whitespace() {
        let needed = line.len().saturating_add(1).saturating_add(word.len());
        if !empty && needed > WIDTH {
            // A continuation line must not open with a Markdown block marker, or it starts a new list item, quote or
            // heading; the word before it moves down too so the marker is never first.
            let carried = match line.rsplit_once(' ') {
                Some((head, last)) if starts_block(word) && head.len() > prefix_len => {
                    let last = last.to_owned();
                    line.truncate(head.len());
                    last
                }
                Some(_) | None => String::default(),
            };
            out.push_str(line.trim_end());
            out.push('\n');
            rest.clone_into(&mut line);
            prefix_len = rest.len();
            empty = carried.is_empty();
            line.push_str(&carried);
        }
        if !empty {
            line.push(' ');
        }
        line.push_str(word);
        empty = false;
    }
    out.push_str(line.trim_end());
    out.push('\n');
    out
}

/// Whether a word would open a Markdown block -- a list item, quote or heading -- if it began a line.
fn starts_block(word: &str) -> bool {
    let ordinal = word
        .strip_suffix(['.', ')'])
        .is_some_and(|number| !number.is_empty() && number.chars().all(|digit| digit.is_ascii_digit()));
    matches!(word, "+" | "-" | "*" | ">") || word.starts_with('#') || ordinal
}

/// Read every task from every `config*.toml` in `mise_dir`, in file order.
fn load_tasks(mise_dir: &Path) -> Result<Vec<Task>, Error> {
    let mut files: Vec<_> = fs::read_dir(mise_dir)?
        .map(|entry| entry.map(|found| found.path()))
        .collect::<Result<_, _>>()?;
    files.sort();
    let mut tasks = Vec::new();
    for file in files {
        let Some(file_name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(env) = file_name
            .strip_prefix("config")
            .and_then(|rest| rest.strip_suffix(".toml"))
        else {
            continue;
        };
        // A mise local override is untracked and per-machine, so its tasks must never reach the committed catalogue.
        if env == ".local" || has_extension(Path::new(env), "local") {
            continue;
        }
        let env = env.strip_prefix('.').unwrap_or("default").to_owned();
        let table: toml::Table = toml::from_str(&fs::read_to_string(&file)?)?;
        let Some(defined) = table.get("tasks").and_then(toml::Value::as_table) else {
            continue;
        };
        for (name, value) in defined {
            tasks.push(task(name, &env, value));
        }
    }
    Ok(tasks)
}

/// Build one task from its TOML value: a full table, or the `name = "command"` shorthand.
fn task(name: &str, env: &str, value: &toml::Value) -> Task {
    let text = |key: &str| value.get(key).map(strings).unwrap_or_default().join("\n");
    let mut body = value.as_str().map_or_else(|| text("run"), str::to_owned);
    let windows = text("run_windows");
    if !windows.is_empty() {
        body.push('\n');
        body.push_str(&windows);
    }
    let env_values = value.get("env").and_then(toml::Value::as_table).map(|table| {
        table
            .values()
            .filter_map(toml::Value::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    });
    if let Some(env_values) = env_values {
        body.push('\n');
        body.push_str(&env_values);
    }
    Task {
        name: name.to_owned(),
        env: env.to_owned(),
        description: text("description"),
        body,
        depends: value.get("depends").map(strings).unwrap_or_default(),
    }
}

/// The strings a TOML value holds: itself, each string of an array, or each `task` of an array of tables.
fn strings(value: &toml::Value) -> Vec<String> {
    if let Some(single) = value.as_str() {
        return vec![single.to_owned()];
    }
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().or_else(|| item.get("task").and_then(toml::Value::as_str)))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// Every task the `check` aggregates run, and for each one the aggregates that list it in `depends`.
///
/// The search descends only through aggregates -- tasks with no `run` of their own, which only group other checks --
/// so a check's own prerequisites (the generator a drift check reruns first) are not mistaken for checks.
fn reachable(tasks: &[Task]) -> Result<(BTreeSet<String>, Parents), Error> {
    let names: BTreeSet<&str> = tasks.iter().map(|task| task.name.as_str()).collect();
    let mut depends: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for task in tasks.iter().filter(|task| task.body.trim().is_empty()) {
        depends
            .entry(&task.name)
            .or_default()
            .extend(task.depends.iter().map(String::as_str));
    }
    let mut queue: Vec<String> = names
        .iter()
        .filter(|name| **name == "check" || name.starts_with("check:"))
        .map(|name| (*name).to_owned())
        .collect();
    let mut reached: BTreeSet<String> = queue.iter().cloned().collect();
    let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    while let Some(parent) = queue.pop() {
        for pattern in depends.get(parent.as_str()).into_iter().flatten() {
            let glob = Regex::new(&format!("^{}$", regex::escape(pattern).replace(r"\*", ".*")))?;
            for child in names.iter().filter(|name| glob.is_match(name)) {
                let _first_parent: bool = parents.entry((*child).to_owned()).or_default().insert(parent.clone());
                if reached.insert((*child).to_owned()) {
                    queue.push((*child).to_owned());
                }
            }
        }
    }
    Ok((reached, parents))
}

/// Split a shell body into its commands, each a list of words with the shell's quoting removed.
///
/// Commands end at an unquoted newline, `;`, `|`, `||` or `&&`. A `$(...)` substitution stays one word, operators and
/// all, so it can be searched as a command of its own, and a `#` that starts a word comments out the rest of the line.
fn commands(body: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut words = Vec::new();
    let mut word = String::default();
    let mut quote = None;
    let mut inner_quote = None;
    let mut depth = 0_usize;
    let mut comment = false;
    let mut chars = body.chars().peekable();
    while let Some(current) = chars.next() {
        if comment {
            if current == '\n' {
                comment = false;
                end_command(&mut commands, &mut words, &mut word);
            }
            continue;
        }
        if let Some(open) = quote {
            if current == open {
                quote = None;
            } else {
                word.push(current);
            }
            continue;
        }
        if current == '$' && chars.peek() == Some(&'(') {
            depth = depth.saturating_add(1);
            word.push(current);
            continue;
        }
        if depth > 0 {
            // Quotes inside the substitution are kept, and shield a `)` they contain from closing it.
            if inner_quote == Some(current) {
                inner_quote = None;
            } else if inner_quote.is_none() && matches!(current, '\'' | '"') {
                inner_quote = Some(current);
            } else if inner_quote.is_none() && current == ')' {
                depth = depth.saturating_sub(1);
            } else {
                // Any other character is part of the substitution's text.
            }
            word.push(current);
            continue;
        }
        match current {
            '\'' | '"' => quote = Some(current),
            '#' if word.is_empty() => comment = true,
            '\n' | ';' | '|' | '&' => end_command(&mut commands, &mut words, &mut word),
            ' ' | '\t' => end_word(&mut words, &mut word),
            _ => word.push(current),
        }
    }
    end_command(&mut commands, &mut words, &mut word);
    commands
}

/// Close the word being built, if any.
fn end_word(words: &mut Vec<String>, word: &mut String) {
    if !word.is_empty() {
        words.push(core::mem::take(word));
    }
}

/// Close the word and the command being built, if any.
fn end_command(commands: &mut Vec<Vec<String>>, words: &mut Vec<String>, word: &mut String) {
    end_word(words, word);
    if !words.is_empty() {
        commands.push(core::mem::take(words));
    }
}

/// Search a `$(...)` substitution's command for the tool it runs.
fn substitution_tool(inner: &str) -> Result<Found, Error> {
    let tool = runner_label(inner)?;
    Ok(if tool == SHELL_SECTION {
        Found::Nothing
    } else {
        Found::Tool(tool)
    })
}

/// Search one command for the tool it runs.
fn command_tool(words: &[String], assignment: &Regex, command: &Regex) -> Result<Found, Error> {
    let mut rest = words.iter();
    let mut after_xargs = false;
    while let Some(word) = rest.next() {
        if let Some(inner) = word.strip_prefix("$(").and_then(|inner| inner.strip_suffix(')')) {
            return substitution_tool(inner);
        }
        if word.starts_with("{{") {
            if !word.ends_with("}}") {
                let _closing: Option<&String> = rest.by_ref().find(|later| later.ends_with("}}"));
            }
            continue;
        }
        if assignment.is_match(word) {
            let value = word.split_once('=').map(|(_, value)| value).unwrap_or_default();
            let inner = value.strip_prefix("$(").and_then(|inner| inner.strip_suffix(')'));
            if let Some(Found::Tool(tool)) = inner.map(substitution_tool).transpose()? {
                return Ok(Found::Tool(tool));
            }
            continue;
        }
        if after_xargs && word.starts_with('-') {
            continue;
        }
        let name = word.as_str();
        if name == "xargs" {
            after_xargs = true;
            continue;
        }
        if PREFIXES.contains(&name) {
            continue;
        }
        if PLUMBING.contains(&name) || matches!(name, "[" | "[[" | "test" | "for" | "case" | "eval") {
            return Ok(Found::Nothing);
        }
        if name == "cargo" {
            return Ok(Found::Tool(cargo_label(rest.as_slice())));
        }
        if name == "conda_exe" {
            let binary = rest.nth(1).map_or_else(|| "conda_exe".to_owned(), Clone::clone);
            return Ok(Found::Tool(binary));
        }
        if let Some(variable) = name.strip_prefix('$') {
            if !variable.is_empty()
                && variable
                    .chars()
                    .all(|letter| letter.is_ascii_uppercase() || letter == '_')
            {
                return Ok(Found::Tool(variable.to_ascii_lowercase()));
            }
            return Ok(Found::Nothing);
        }
        if command.is_match(name) {
            return Ok(Found::Tool(name.to_owned()));
        }
        return Ok(Found::Nothing);
    }
    Ok(Found::Nothing)
}

/// Label a `cargo` invocation by its subcommand, or by the package a `cargo run -p` runs.
///
/// A `+toolchain` selector is skipped, including one spelt as a `+{{ vars.... }}` template across several words.
fn cargo_label(args: &[String]) -> String {
    let mut in_template = false;
    let mut words = args.iter().filter(|word| {
        let skip = in_template || word.starts_with('+');
        if word.contains("{{") && !word.contains("}}") {
            in_template = true;
        } else if word.contains("}}") {
            in_template = false;
        } else {
            // A word outside any template leaves the state as it was.
        }
        !skip
    });
    let Some(subcommand) = words.next() else {
        return "cargo".to_owned();
    };
    if subcommand == "run" {
        let mut after = words.skip_while(|word| *word != "-p");
        if let Some(package) = after.nth(1).filter(|package| !package.starts_with('$')) {
            return package.clone();
        }
    }
    format!("cargo {subcommand}")
}

/// Write one section's checks, each with its env, the aggregate that runs it, its description and its configs.
fn render_entries(out: &mut String, label: &str, entries: &[&Task], parents: &Parents) -> Result<(), Error> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|left, right| (&left.name, &left.env).cmp(&(&right.name, &right.env)));
    for task in sorted {
        let run_by = parents.get(&task.name).map_or_else(
            || "not run by `mise run check`".to_owned(),
            |names| {
                format!(
                    "run by {}",
                    names
                        .iter()
                        .map(|name| format!("`{name}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            },
        );
        let mut configs = config_paths(&task.body)?;
        configs.extend(implicit_configs(label).iter().map(|path| (*path).to_owned()));
        let mut text = format!("`{}` (`{}` env; {run_by})", task.name, task.env);
        if !task.description.is_empty() {
            write!(text, " -- {}", task.description.trim_end_matches('.'))?;
        }
        text.push('.');
        if !configs.is_empty() {
            let listed = configs
                .iter()
                .map(|path| format!("`{path}`"))
                .collect::<Vec<_>>()
                .join(", ");
            write!(text, " Config: {listed}.")?;
        }
        out.push_str(&wrap("- ", "  ", &text));
    }
    Ok(())
}

/// The paths a `run` body passes as a tool's configuration: those following a config-style flag.
///
/// A path handed over as an input to check rather than as configuration -- a rule directory a policy validates the
/// layout of -- follows no such flag, so it is not mistaken for the rules of the check that reads it.
pub fn rule_sources(body: &str) -> Result<BTreeSet<String>, Error> {
    let flagged = Regex::new(
        r#"(?:^|\s)(?:--config-file|--config|--policy|--schema|--rule|-c|-p)(?:\s+|=)"?[^"\s]*?(config/[\w./-]*\w)"#,
    )?;
    Ok(flagged
        .captures_iter(body)
        .filter_map(|caps| caps.get(1))
        .map(|found| found.as_str().to_owned())
        .collect())
}

/// The rules a section's checks are configured with, and the deepest directory every rule file shares.
///
/// Rule files are recognised by format rather than by tool: a `.rego` policy (its tests excepted), a `.schema.json`
/// schema, and a YAML document that is a rule (`id` + `message`), lists rules (`rules:` as a sequence), or points at
/// directories of them (`ruleDirs:`), and a TOML manifest's `[workspace.lints]` tables. Sources are the paths the
/// checks pass as configuration plus the configs the section's tool finds by itself. A directory is searched
/// recursively. A section with none renders nothing.
fn section_rules(root: &Path, label: &str, entries: &[&Task]) -> Result<SectionRules, Error> {
    let mut sources = BTreeSet::new();
    for task in entries {
        sources.extend(rule_sources(&task.body)?);
    }
    sources.extend(implicit_configs(label).iter().map(|path| (*path).to_owned()));
    let mut files = BTreeSet::new();
    for source in sources {
        collect_rule_files(&root.join(source), &mut files)?;
    }
    let mut listed = Vec::new();
    let mut policies = String::default();
    let mut policy_rules = 0_usize;
    for file in &files {
        let name = file.file_name().and_then(|found| found.to_str()).unwrap_or_default();
        if let Some(stem) = name.strip_suffix(".rego") {
            let summaries = rego_summaries(&relative_path(root, file), &fs::read_to_string(file)?)?;
            policy_rules = policy_rules.saturating_add(summaries.len());
            policies.push_str(&render_policy(root, file, stem, &summaries));
        } else if let Some(stem) = name.strip_suffix(".schema.json") {
            listed.push(schema_rule(file, stem)?);
        } else if has_extension(file, "toml") {
            listed.extend(lint_rules(file)?);
        } else {
            listed.extend(yaml_rules(file)?);
        }
    }
    listed.sort_by(|left, right| left.name.cmp(&right.name));
    let mut rendered = render_rules(&listed)?;
    rendered.push_str(&policies);
    let location = match files.iter().collect::<Vec<_>>().as_slice() {
        [single] => relative_path(root, single),
        _ => common_dir(root, &files),
    };
    Ok(SectionRules {
        location,
        rendered,
        count: listed.len().saturating_add(policy_rules),
    })
}

/// Add every rule file at `path` to `files`, searching a directory recursively and following `ruleDirs`.
fn collect_rule_files(path: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), Error> {
    if path.is_dir() {
        let mut children: Vec<_> = fs::read_dir(path)?
            .map(|entry| entry.map(|found| found.path()))
            .collect::<Result<_, _>>()?;
        children.sort();
        for child in children {
            collect_rule_files(&child, files)?;
        }
        return Ok(());
    }
    let name = path.file_name().and_then(|found| found.to_str()).unwrap_or_default();
    if !path.is_file() || name.ends_with("_test.rego") {
        return Ok(());
    }
    if has_extension(path, "rego") || is_schema(path) || (has_extension(path, "toml") && declares_lints(path)?) {
        let _new_file: bool = files.insert(path.to_path_buf());
        return Ok(());
    }
    if !has_extension(path, "yaml") {
        return Ok(());
    }
    for document in yaml_documents(path)? {
        let rule_dirs = document.get("ruleDirs").and_then(serde_yaml::Value::as_sequence);
        for dir in rule_dirs.into_iter().flatten().filter_map(serde_yaml::Value::as_str) {
            collect_rule_files(&path.parent().unwrap_or(path).join(dir), files)?;
        }
        let single = document.get("id").is_some() && document.get("message").is_some();
        let listed = document.get("rules").and_then(serde_yaml::Value::as_sequence).is_some();
        if single || listed {
            let _new_file: bool = files.insert(path.to_path_buf());
        }
    }
    Ok(())
}

/// The configs a section's tool finds by itself, or none.
fn implicit_configs(label: &str) -> &'static [&'static str] {
    IMPLICIT_CONFIGS
        .iter()
        .find(|(tool, _)| *tool == label)
        .map_or(&[], |(_, paths)| paths)
}

/// The `[workspace.lints]` tables of a TOML manifest, keyed by lint tool (`rust`, `clippy`, `rustdoc`).
fn workspace_lints(path: &Path) -> Result<toml::Table, Error> {
    let manifest: toml::Table = toml::from_str(&fs::read_to_string(path)?)?;
    let lints = manifest.get("workspace").and_then(|workspace| workspace.get("lints"));
    Ok(lints.and_then(toml::Value::as_table).cloned().unwrap_or_default())
}

/// Whether a TOML manifest configures any lints in `[workspace.lints]`.
fn declares_lints(path: &Path) -> Result<bool, Error> {
    Ok(!workspace_lints(path)?.is_empty())
}

/// A `[workspace.lints.<tool>]` entry's lint, named as a `#[expect]` would name it.
///
/// A lint of the `rust` table is the compiler's own and goes unprefixed; every other table names its tool, as in
/// `clippy::unwrap_used`.
fn lint_name(tool: &str, lint: &str) -> String {
    if tool == "rust" {
        lint.to_owned()
    } else {
        format!("{tool}::{lint}")
    }
}

/// The lint each `[workspace.lints]` entry is marked as the opposite of, keyed by the entry's `lint_name`.
///
/// Some lints, most of them in clippy's `restriction` group, come in pairs where one demands the spelling the other
/// rejects, so the workspace can deny at most one of the two. The entry that settles such a pair carries an
/// `# Opposite of <lint>.` line in the comment block directly above it, naming the other lint the same way.
#[must_use]
pub fn lint_opposites(manifest: &str) -> BTreeMap<String, String> {
    let mut opposites = Vec::new();
    let mut tool = "";
    let mut opposite = "";
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            tool = header
                .strip_prefix("workspace.lints.")
                .and_then(|rest| rest.strip_suffix(']'))
                .unwrap_or_default();
            opposite = "";
        } else if let Some(comment) = line.strip_prefix('#') {
            if let Some(name) = comment
                .trim()
                .strip_prefix("Opposite of ")
                .and_then(|rest| rest.strip_suffix('.'))
            {
                opposite = name;
            }
        } else if let Some((lint, _)) = line.split_once('=')
            && !tool.is_empty()
            && !opposite.is_empty()
        {
            opposites.push((lint_name(tool, lint.trim()), opposite.to_owned()));
            opposite = "";
        } else {
            opposite = "";
        }
    }
    opposites.into_iter().collect()
}

/// Every lint a manifest's `[workspace.lints]` sets, named by [`lint_name`], with its level.
///
/// The description is the level, with the priority when one is set and the lint it is the opposite of when its entry
/// is marked as one by [`lint_opposites`].
fn lint_rules(path: &Path) -> Result<Vec<Rule>, Error> {
    let rustc_levels = rustc_lint_levels()?;
    let opposites = lint_opposites(&fs::read_to_string(path)?);
    let mut rules = Vec::new();
    for (tool, lints) in workspace_lints(path)? {
        for (lint, setting) in lints.as_table().into_iter().flatten() {
            let name = lint_name(&tool, lint);
            let level = setting
                .as_str()
                .or_else(|| setting.get("level").and_then(toml::Value::as_str))
                .unwrap_or_default();
            described(path, &name, "level", level)?;
            let priority = setting.get("priority").and_then(toml::Value::as_integer);
            let mut description = priority.map_or_else(|| level.to_owned(), |rank| format!("{level}, priority {rank}"));
            if let Some(opposite) = opposites.get(&name) {
                write!(description, ", opposite of `{opposite}`")?;
            }
            let link = lint_link(&tool, lint, &rustc_levels);
            rules.push(Rule {
                name,
                description,
                link,
            });
        }
    }
    Ok(rules)
}

/// Whether `path` carries `extension`, compared case-insensitively.
fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

/// Whether `path` is a JSON schema, named `<stem>.schema.json`.
fn is_schema(path: &Path) -> bool {
    has_extension(path, "json")
        && path
            .file_stem()
            .is_some_and(|stem| has_extension(Path::new(stem), "schema"))
}

/// Every YAML document in one file.
fn yaml_documents(path: &Path) -> Result<Vec<serde_yaml::Value>, Error> {
    let source = fs::read_to_string(path)?;
    let mut documents = Vec::new();
    for document in serde_yaml::Deserializer::from_str(&source) {
        documents.push(serde_yaml::Value::deserialize(document)?);
    }
    Ok(documents)
}

/// The rules one YAML rule file defines: each document that is a rule, or each entry of a `rules:` list.
fn yaml_rules(path: &Path) -> Result<Vec<Rule>, Error> {
    let mut rules = Vec::new();
    for document in yaml_documents(path)? {
        let entries = document
            .get("rules")
            .and_then(serde_yaml::Value::as_sequence)
            .map_or_else(|| vec![document.clone()], Clone::clone);
        for entry in entries.iter().filter(|entry| entry.get("id").is_some()) {
            let field = |key: &str| {
                entry
                    .get(key)
                    .and_then(serde_yaml::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let rule = Rule {
                name: field("id"),
                description: field("message"),
                link: String::default(),
            };
            described(path, &rule.name, "message", &rule.description)?;
            rules.push(rule);
        }
    }
    Ok(rules)
}

/// One schema rule: the schema's file stem, with its `title` and `description`.
fn schema_rule(path: &Path, stem: &str) -> Result<Rule, Error> {
    let schema: serde_json::Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let field = |key: &str| {
        schema
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    described(path, stem, "title", &field("title"))?;
    described(path, stem, "description", &field("description"))?;
    Ok(Rule {
        name: stem.to_owned(),
        description: format!("{}: {}", field("title"), field("description")),
        link: String::default(),
    })
}

/// One policy file, followed by the summary of each of its rules; nothing for a file with no rules.
fn render_policy(root: &Path, path: &Path, stem: &str, summaries: &[String]) -> String {
    if summaries.is_empty() {
        return String::default();
    }
    let relative = relative_path(root, path);
    let mut out = wrap("- ", "  ", &format!("`{stem}` (`{relative}`):"));
    for summary in summaries {
        out.push_str(&wrap("  - ", "    ", summary));
    }
    out
}

/// The deepest directory, relative to `root`, that contains every one of `files`.
fn common_dir(root: &Path, files: &BTreeSet<PathBuf>) -> String {
    let mut parents = files.iter().filter_map(|file| file.parent());
    let Some(first) = parents.next() else {
        return String::default();
    };
    let mut common = first.to_path_buf();
    for parent in parents {
        while !parent.starts_with(&common) {
            if !common.pop() {
                break;
            }
        }
    }
    relative_path(root, &common)
}

/// `path` relative to `root`, with `/` separators on every platform.
fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Render a rule list, one bullet per rule, its description flattened onto wrapped lines.
///
/// A rule with a documentation page links to it through a reference labelled by [`link_label`], its URL defined
/// after the list, so a long lint name and its URL never have to share one line.
fn render_rules(rules: &[Rule]) -> Result<String, Error> {
    let mut out = String::default();
    let mut references = String::default();
    let mut labels = BTreeSet::new();
    for rule in rules {
        let mut shown = format!("`{}`", rule.name);
        if !rule.link.is_empty() {
            let label = link_label(&rule.name, &rule.link);
            if !labels.insert(label.clone()) {
                return Err(Error::DuplicateLinkLabel(format!(
                    "`{}` would share the link label `{label}` with an earlier rule",
                    rule.name
                )));
            }
            writeln!(references, "[{label}]: {}", rule.link)?;
            shown = format!("[{shown}][{label}]");
        }
        out.push_str(&wrap("- ", "  ", &format!("{shown} -- {}", rule.description)));
    }
    if !references.is_empty() {
        out.push('\n');
        out.push_str(&references);
    }
    Ok(out)
}

/// The reference label a rule's documentation link is defined under, derived from the rule's name alone.
///
/// The label is the name without its tool prefix, so adding or removing a rule never relabels any other. Where
/// `[label]: <link>` would run past `WIDTH`, whole words are dropped from the end of the name until it fits --
/// `allow_attributes_without_reason` becomes `allow_attributes_without` -- which is just as stable, and leaves only
/// real words for the spell checker to read.
#[must_use]
pub fn link_label(name: &str, link: &str) -> String {
    let bare = name.rsplit("::").next().unwrap_or(name);
    let budget = WIDTH.saturating_sub(link.len()).saturating_sub("[]: ".len());
    let mut words: Vec<&str> = bare.split('_').collect();
    while words.len() > 1 && words.join("_").len() > budget {
        let _: Option<&str> = words.pop();
    }
    words.join("_")
}

/// The lint groups, which name a set of lints and have no entry of their own in the lint index.
const CLIPPY_GROUPS: &[&str] = &[
    "all",
    "cargo",
    "complexity",
    "correctness",
    "nursery",
    "pedantic",
    "perf",
    "restriction",
    "style",
    "suspicious",
];

/// The documentation page for one lint of a `[workspace.lints.<tool>]` table, or empty when it has none.
///
/// A clippy lint links to its entry in the stable lint index, and a clippy group to nothing, since groups have no
/// entry there. A rustdoc lint links to its heading on the rustdoc lints page. A compiler lint's page depends on its
/// default level -- the rustc book lists allow-, warn- and deny-by-default lints on separate pages, and groups on a
/// fourth -- so it is found in `rustc_levels`; a lint the installed rustc does not list stays unlinked.
fn lint_link(tool: &str, lint: &str, rustc_levels: &BTreeMap<String, String>) -> String {
    let anchor = lint.replace('_', "-");
    let rustc_page = |page: &str| format!("https://doc.rust-lang.org/rustc/lints/listing/{page}");
    match tool {
        "clippy" if !CLIPPY_GROUPS.contains(&lint) => {
            format!("https://rust-lang.github.io/rust-clippy/stable/index.html#{lint}")
        }
        "rustdoc" => format!("https://doc.rust-lang.org/rustdoc/lints.html#{lint}"),
        "rust" => match rustc_levels.get(lint).map(String::as_str) {
            Some("group") => "https://doc.rust-lang.org/rustc/lints/groups.html".to_owned(),
            Some("allow") => rustc_page(&format!("allowed-by-default.html#{anchor}")),
            Some("warn") => rustc_page(&format!("warn-by-default.html#{anchor}")),
            Some("deny" | "forbid") => rustc_page(&format!("deny-by-default.html#{anchor}")),
            Some(_) | None => String::default(),
        },
        _ => String::default(),
    }
}

/// The default level rustc gives each of its own lints, and `group` for each lint group, read from `rustc -W help`.
///
/// Names are returned in their `snake_case` spelling, as `[workspace.lints.rust]` writes them; rustc prints them
/// hyphenated.
fn rustc_lint_levels() -> Result<BTreeMap<String, String>, Error> {
    let help = std::process::Command::new("rustc")
        .args(["-W", "help"])
        .output_checked_utf8()?
        .stdout;
    let mut levels = BTreeMap::new();
    let mut in_groups = false;
    for line in help.lines() {
        if line.starts_with("Lint groups provided by rustc") {
            in_groups = true;
            continue;
        }
        let mut columns = line.split_whitespace();
        let (Some(name), Some(second)) = (columns.next(), columns.next()) else {
            continue;
        };
        if name == "name" || name.starts_with("----") {
            continue;
        }
        let level = if in_groups { "group" } else { second };
        if in_groups || matches!(level, "allow" | "warn" | "deny" | "forbid") {
            let _previous: Option<String> = levels.insert(name.replace('-', "_"), level.to_owned());
        }
    }
    Ok(levels)
}
