use serde::Serialize;

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
