use serde::Serialize;
use serde_yaml::{Mapping, Value};

use crate::error::CliError;

/// Serialise one object, in the YAML style the repo's formatters hold a committed file to.
#[expect(
    clippy::unwrap_in_result,
    clippy::unwrap_used,
    reason = "pretty_yaml only fails on malformed YAML and serde output is always well-formed"
)]
pub(crate) fn document<T>(object: &T) -> Result<String, CliError>
where
    T: Serialize,
{
    let yaml = serde_yaml::to_string(object)?;
    Ok(pretty_yaml::format_text(&yaml, &pretty_yaml::config::FormatOptions::default()).unwrap())
}

/// Set `key` in `mapping`, which never holds it already: each key is written once, in the order the file shows them.
pub(crate) fn insert(mapping: &mut Mapping, key: &str, value: Value) {
    let _previous: Option<Value> = mapping.insert(Value::from(key), value);
}

/// A YAML sequence of `items`, the shape of every command and dependency list these files write.
pub(crate) fn strings(items: &[&str]) -> Value {
    Value::Sequence(items.iter().map(|item| Value::from(*item)).collect())
}
