//! Browser-side helpers around the client: its localStorage records, the page's textareas, and JS reflection.

use wasm_bindgen::prelude::*;

const STORED_AGENT_ID_KEY: &str = "ws_wasm_agent.agent_id";
const STORED_LAST_OFFLINE_AT_KEY: &str = "ws_wasm_agent.last_offline_at";

#[must_use]
pub fn js_number_field(value: &JsValue, field: &str) -> Option<f64> {
    let field_value = js_sys::Reflect::get(value, &JsValue::from_str(field)).ok()?;
    field_value.as_f64()
}

#[must_use]
pub fn js_bool_field(value: &JsValue, field: &str) -> Option<bool> {
    let field_value = js_sys::Reflect::get(value, &JsValue::from_str(field)).ok()?;
    field_value.as_bool()
}

#[must_use]
pub fn js_nested_object(value: &JsValue, field: &str) -> Option<JsValue> {
    js_sys::Reflect::get(value, &JsValue::from_str(field))
        .ok()
        .filter(|nested| !nested.is_null() && !nested.is_undefined())
}

pub(crate) fn load_stored_agent_id() -> Option<String> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok()??;
    storage.get_item(STORED_AGENT_ID_KEY).ok()?
}

pub(crate) fn store_agent_id(agent_id: &str) -> Result<(), JsValue> {
    local_storage()?.set_item(STORED_AGENT_ID_KEY, agent_id)
}

pub(crate) fn store_last_offline_at(timestamp: &str) -> Result<(), JsValue> {
    local_storage()?.set_item(STORED_LAST_OFFLINE_AT_KEY, timestamp)
}

/// The page's localStorage, or the reason there is none to write to.
fn local_storage() -> Result<web_sys::Storage, JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    window
        .local_storage()?
        .ok_or_else(|| JsValue::from_str("No localStorage available"))
}

#[wasm_bindgen(js_name = set_textarea_value)]
pub fn set_textarea_value(element_id: &str, message: &str) -> Result<(), JsValue> {
    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(output) = document.get_element_by_id(element_id)
    {
        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("value"),
            &JsValue::from_str(message),
        )?;
    }

    Ok(())
}

#[wasm_bindgen(js_name = append_to_textarea)]
pub fn append_to_textarea(element_id: &str, message: &str) -> Result<(), JsValue> {
    if let Some(window) = web_sys::window()
        && let Some(document) = window.document()
        && let Some(output) = document.get_element_by_id(element_id)
    {
        let current_value = js_sys::Reflect::get(output.as_ref(), &JsValue::from_str("value"))?
            .as_string()
            .unwrap_or_default();
        let next_value = if current_value.is_empty() || current_value.starts_with("Workflow module") {
            message.to_string()
        } else {
            format!("{current_value}\n{message}")
        };

        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("value"),
            &JsValue::from_str(&next_value),
        )?;

        // Auto-scroll to bottom
        js_sys::Reflect::set(
            output.as_ref(),
            &JsValue::from_str("scrollTop"),
            &js_sys::Reflect::get(output.as_ref(), &JsValue::from_str("scrollHeight"))?,
        )?;
    }

    Ok(())
}
