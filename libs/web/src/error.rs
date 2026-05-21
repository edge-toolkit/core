//! Conversion helpers that replace the two universal `map_err` patterns in
//! WASM modules:
//!
//! - `.dyn_into::<T>().map_err(|_| JsValue::from_str("..."))` — replace
//!   with `.dyn_into_msg::<T>("...")`.
//! - `<fallible JS call>.map_err(|e| JsValue::from_str(&format!("ctx: {e:?}")))`
//!   — replace with `.js_context("ctx")` (it formats the inner JsValue
//!   into the prefix string itself).
//!
//! This is the only file in the workspace where `map_err` is permitted —
//! the `no-map-err` ast-grep rule exempts it.

use wasm_bindgen::{JsCast, JsValue};

/// Replacement for `.dyn_into::<T>().map_err(|_| JsValue::from_str(msg))`.
/// Drops the original (irrelevant — it's just the same JsValue we tried to
/// cast) and surfaces a descriptive string instead.
pub trait JsCastExt {
    fn dyn_into_msg<T: JsCast>(self, msg: &str) -> Result<T, JsValue>;
}

impl<S: JsCast> JsCastExt for S {
    fn dyn_into_msg<T: JsCast>(self, msg: &str) -> Result<T, JsValue> {
        self.dyn_into::<T>().map_err(|_| JsValue::from_str(msg))
    }
}

/// Replacement for `.map_err(|e| JsValue::from_str(&format!("ctx: {e:?}")))`
/// on any `Result<T, E>` where `E: std::fmt::Debug`. Use `.js_context("ctx")`
/// to attach a prefix to a JS error you propagated from a fallible call.
pub trait JsResultExt<T> {
    fn js_context(self, context: &str) -> Result<T, JsValue>;
}

impl<T, E: std::fmt::Debug> JsResultExt<T> for Result<T, E> {
    fn js_context(self, context: &str) -> Result<T, JsValue> {
        self.map_err(|err| JsValue::from_str(&format!("{context}: {err:?}")))
    }
}
