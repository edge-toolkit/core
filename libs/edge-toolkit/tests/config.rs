//! The generic optional/sentinel deserialize helpers in `edge_toolkit::config`.
//!
//! Exercised directly over a `StrDeserializer` (the string form every value
//! arrives as under `serde-env`), so the genericity over the inner type is
//! visible: `deserialize_optional::<String>` and the `Duration` humantime
//! variant share one sentinel (`none` / `off` / `disabled`).
#![cfg(test)]

use std::time::Duration;

use edge_toolkit::config::{deserialize_optional, deserialize_optional_humantime, is_disabled_sentinel};
use serde::de::IntoDeserializer as _;
use serde::de::value::{Error as ValueError, StrDeserializer};

fn optional_string(value: &str) -> Option<String> {
    let deser: StrDeserializer<'_, ValueError> = value.into_deserializer();
    deserialize_optional::<_, String>(deser).unwrap()
}

fn optional_duration(value: &str) -> Option<Duration> {
    let deser: StrDeserializer<'_, ValueError> = value.into_deserializer();
    deserialize_optional_humantime(deser).unwrap()
}

#[test]
fn recognises_disable_sentinels() {
    for sentinel in ["none", "off", "disabled", "NONE", "Off", " disabled "] {
        assert!(is_disabled_sentinel(sentinel), "{sentinel:?} should disable");
    }
    for value in ["", "30s", "64MiB", "never-mind"] {
        assert!(!is_disabled_sentinel(value), "{value:?} should not disable");
    }
}

#[test]
fn generic_optional_works_for_a_non_duration_inner() {
    assert_eq!(optional_string("none"), None);
    assert_eq!(optional_string("disabled"), None);
    assert_eq!(optional_string("hello"), Some("hello".to_owned()));
}

#[test]
fn humantime_optional_parses_or_disables() {
    assert_eq!(optional_duration("off"), None);
    assert_eq!(optional_duration("1m30s"), Some(Duration::from_secs(90)));
}
