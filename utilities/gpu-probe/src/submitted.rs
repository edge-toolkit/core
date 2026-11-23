//! A file of submissions in the registry's `submitted/` directory, as the hosted probe page proposes one on GitHub.
//!
//! The page has no server to send to, so it puts the file in the link to GitHub's new-file page, which opens only a
//! link short enough. The file is therefore plain text holding little but the results' values, laid out so that a visit
//! with two GPUs fits in one link.
//!
//! A file is one block per GPU, the blocks separated by a blank line. A block's lines are, in order: `label`, then the
//! label each new entry is given; each field of the row the GPU was seen at, its name then its value; then one line
//! per interface, its name then its results' values in the order [`result_names`] gives their names, or its name, then
//! `blocked:` and the reason the browser withheld it. A name is separated from what follows by one space. A block with
//! no interface line is refused, so that one can never be filed as nothing.
//!
//! A line longer than the 120 characters a tracked file allows is broken. A line that starts with three spaces
//! continues the one before it with nothing between them, which breaks a word too long to fit; failing that, a line
//! that starts with two spaces continues it with a space between them. A tracked file holds ASCII alone, so a backslash
//! is written `\\`, and a character past ASCII as `\u{...}`, its code point in hex.
#![expect(
    clippy::single_call_fn,
    reason = "each step of parse is a named function for readability, called from one place"
)]

use std::collections::BTreeMap;

use et_gpu_probe::result_names;

use crate::registry::{ROW_ORDER, RegisterError, Submission};

/// What opens a line that continues the one before it, after a space.
const CONTINUED: &str = "  ";

/// What opens a line that continues the one before it with nothing between them.
const JOINED: &str = "   ";

/// What follows an interface's name on the line of an interface the browser withheld, before the reason.
const BLOCKED: &str = "blocked: ";

/// The name of the line holding a block's label.
const LABEL: &str = "label";

/// A line of a file once its continuations are joined to it, numbered as the line it starts on.
struct Line {
    /// The number of the line it starts on, counting from 1.
    number: usize,
    /// Its text, continuations joined on.
    text: String,
}

/// Every submission the file `text` holds, in file order.
///
/// # Errors
///
/// [`RegisterError::Submitted`] naming the line at fault where the file is not laid out as the module says. Whether
/// each submission is one the registry would take is left to [`crate::registry::RegistryTree::check`].
pub fn parse(text: &str) -> Result<Vec<Submission>, RegisterError> {
    let mut submissions = Vec::new();
    for block in blocks(text)? {
        submissions.extend(parse_block(&block)?);
    }
    Ok(submissions)
}

/// The blocks of `text`, each its lines with their continuations joined on.
fn blocks(text: &str) -> Result<Vec<Vec<Line>>, RegisterError> {
    let mut blocks = Vec::new();
    let mut block: Vec<Line> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index.saturating_add(1);
        if line.is_empty() {
            if !block.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
        } else if let Some(rest) = line.strip_prefix(JOINED) {
            continue_last(&mut block, number, "", rest)?;
        } else if let Some(rest) = line.strip_prefix(CONTINUED) {
            continue_last(&mut block, number, " ", rest)?;
        } else {
            block.push(Line {
                number,
                text: line.to_owned(),
            });
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    Ok(blocks)
}

/// Add `rest`, from the line numbered `number`, to the last line of `block`, with `between` between them.
fn continue_last(block: &mut [Line], number: usize, between: &str, rest: &str) -> Result<(), RegisterError> {
    let continued = block
        .last_mut()
        .ok_or_else(|| fault(number, "a continuation with no line to continue"))?;
    continued.text.push_str(between);
    continued.text.push_str(rest);
    Ok(())
}

/// The submissions of one block: one for each of its interface lines, each with the block's label and row.
fn parse_block(block: &[Line]) -> Result<Vec<Submission>, RegisterError> {
    let mut label = String::default();
    let mut seen = BTreeMap::new();
    let mut submissions = Vec::new();
    for line in block {
        let (name, rest) = line.text.split_once(' ').unwrap_or((&line.text, ""));
        if name != LABEL && !ROW_ORDER.contains(&name) {
            submissions.push(interface_line(name, rest, &label, &seen, line.number)?);
        } else if !submissions.is_empty() {
            return Err(fault(
                line.number,
                "a label or row field after the block's first interface",
            ));
        } else if name == LABEL {
            label = label_line(&label, rest, line.number)?;
        } else {
            row_field(&mut seen, name, rest, line.number)?;
        }
    }
    if submissions.is_empty() {
        let number = block.first().map_or(0, |line| line.number);
        return Err(fault(number, "a block with no interface line"));
    }
    Ok(submissions)
}

/// The label `rest` gives on the line numbered `number`, the block's first, where `label` is the one read so far.
fn label_line(label: &str, rest: &str, number: usize) -> Result<String, RegisterError> {
    if !label.is_empty() {
        return Err(fault(number, "a second label"));
    }
    unescape(rest, number)
}

/// Add the field `name`, of the row, with the value `rest` on the line numbered `number`, to `seen`.
fn row_field(seen: &mut BTreeMap<String, String>, name: &str, rest: &str, number: usize) -> Result<(), RegisterError> {
    if seen.insert(name.to_owned(), unescape(rest, number)?).is_some() {
        return Err(fault(number, "a row field given twice"));
    }
    Ok(())
}

/// The submission the interface line numbered `number` gives.
///
/// The line is the interface `name`, then `rest`, its values or the reason it was withheld; the submission is filed as
/// `label` and seen at `seen`.
fn interface_line(
    name: &str,
    rest: &str,
    label: &str,
    seen: &BTreeMap<String, String>,
    number: usize,
) -> Result<Submission, RegisterError> {
    if let Some(reason) = rest.strip_prefix(BLOCKED) {
        let reason = unescape(reason, number)?;
        return Ok(Submission::withheld(name, label, &reason, seen.clone()));
    }
    let values: Vec<&str> = rest.split(' ').collect();
    let names = result_names(name);
    if values.len() != names.len() {
        return Err(fault(number, "not as many values as the interface has results"));
    }
    let results = names
        .into_iter()
        .map(str::to_owned)
        .zip(values.into_iter().map(str::to_owned));
    Ok(Submission::signature(name, label, results.collect(), seen.clone()))
}

/// `text` with each of its escapes, `\\` and `\u{...}`, read as the character it stands for.
fn unescape(text: &str, number: usize) -> Result<String, RegisterError> {
    let mut read = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            read.push(character);
            continue;
        }
        match characters.next() {
            Some('\\') => read.push('\\'),
            Some('u') => {
                let rest = characters.as_str();
                let hex = rest
                    .strip_prefix('{')
                    .and_then(|braced| braced.split_once('}'))
                    .ok_or_else(|| fault(number, "a \\u escape with no braced code point"))?;
                let code = u32::from_str_radix(hex.0, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| fault(number, "a \\u escape naming no character"))?;
                read.push(code);
                characters = hex.1.chars();
            }
            _ => return Err(fault(number, "a backslash that starts no escape")),
        }
    }
    Ok(read)
}

/// The fault `problem` on the line numbered `number`.
const fn fault(number: usize, problem: &'static str) -> RegisterError {
    RegisterError::Submitted { line: number, problem }
}
