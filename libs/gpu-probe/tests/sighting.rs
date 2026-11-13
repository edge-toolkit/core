//! Covers the rows of the registry: the values a row's fields take, what makes a row unreadable, and that every row the
//! registry holds can be read.
#![cfg(test)]

use et_gpu_probe::{
    Answer, Backend, Browser, GpuVendor, HOST, Named, OS, RowError, Sighting, Signature, WEBGPU, Window,
    every_sighting, reference,
};
use serde_json::{Value, json};

/// The fields a row names from fixed sets, in the order a row has them.
const ENUMERATED: [&str; 6] = ["browser", "os", "cpu_arch", "gpu", "backend", "window"];

/// A row as the registry holds one, seen in Brave on an Android phone.
fn row() -> Value {
    json!({
        "browser": "brave",
        "browser_version": "1.99",
        "os": "android",
        "cpu_arch": "aarch64",
        "gpu": "arm",
        "gpu_version": "Mali-G76",
        "backend": "vulkan",
        "window": "private",
    })
}

/// `changed` with `field` given as `value`.
fn set(mut changed: Value, field: &str, value: &str) -> Value {
    let _replaced: Option<Value> = changed
        .as_object_mut()
        .unwrap()
        .insert(field.to_owned(), Value::String(value.to_owned()));
    changed
}

/// [`row`] with `field` given as `value`.
fn row_with(field: &str, value: &str) -> Value {
    set(row(), field, value)
}

/// What reading `row` fails with, as the error words it.
fn refusal(row: &Value) -> String {
    Sighting::read(row).unwrap_err().to_string()
}

#[test]
fn a_row_reads_as_what_it_names() {
    let sighting = Sighting::read(&row()).unwrap();
    assert_eq!(sighting.browser, Browser::Brave);
    assert_eq!(sighting.system, Named::Known(OS::Android));
    assert_eq!(sighting.cpu_arch.to_string(), "aarch64");
    assert_eq!(sighting.gpu, GpuVendor::Arm);
    assert_eq!(sighting.backend, Backend::Vulkan);
    assert_eq!(sighting.window, Window::Private);
}

#[test]
fn a_row_seen_in_both_kinds_of_window_reads_as_both() {
    assert_eq!(
        Sighting::read(&row_with("window", "both")).unwrap().window,
        Window::Both
    );
}

#[test]
fn a_browser_the_webbrowser_crate_names_is_one_it_lists() {
    assert!(matches!("chrome".parse::<Browser>(), Ok(Browser::Listed(_))));
    assert_eq!("tor-browser".parse::<Browser>().unwrap(), Browser::Tor);
    assert_eq!("deno".parse::<Browser>().unwrap(), Browser::Deno);
    assert_eq!("gnome-web".parse::<Browser>().unwrap(), Browser::GnomeWeb);
    assert_eq!("mullvad-browser".parse::<Browser>().unwrap(), Browser::Mullvad);
    assert_eq!("none".parse::<Browser>().unwrap(), Browser::Absent);
}

#[test]
fn a_gpu_vendor_is_known_by_the_id_its_adapters_report() {
    assert_eq!(GpuVendor::from_id(0x10DE), GpuVendor::Nvidia);
    assert_eq!(GpuVendor::from_id(0x1414), GpuVendor::Microsoft);
    // Metal reports no vendor ID at all.
    assert_eq!(GpuVendor::from_id(0), GpuVendor::Unknown);
}

#[test]
fn a_gpu_vendor_is_known_by_the_name_metal_gives_its_gpu() {
    assert_eq!(GpuVendor::from_device_name("Apple M5 Pro"), GpuVendor::Apple);
    assert_eq!(GpuVendor::from_device_name("AMD Radeon Pro 5500M"), GpuVendor::Amd);
    assert_eq!(
        GpuVendor::from_device_name("Intel(R) UHD Graphics 630"),
        GpuVendor::Intel
    );
    assert_eq!(GpuVendor::from_device_name("Radeon"), GpuVendor::Unknown);
    assert_eq!(GpuVendor::from_device_name(""), GpuVendor::Unknown);
}

#[test]
fn every_gpu_vendor_has_a_name_that_reads_back_as_it() {
    for name in [
        "amd",
        "apple",
        "intel",
        "mesa",
        "microsoft",
        "nvidia",
        "unknown",
        "none",
    ] {
        let vendor: GpuVendor = name.parse().unwrap();
        assert_eq!(vendor.name(), name);
    }
}

#[test]
fn a_signature_gives_its_results_by_probe_name() {
    let signature = Signature::new(HOST, reference().into_iter().map(Answer::Bits).collect());
    let results = signature.results();
    assert_eq!(results.len(), et_gpu_probe::PROBE_COUNT);
    assert_eq!(results.first().unwrap(), &("multiply_add", "00000000".to_owned()));
}

#[test]
fn a_value_the_registry_has_no_name_for_is_kept_as_its_text() {
    let unnamed = ENUMERATED
        .iter()
        .fold(row(), |unnamed, field| set(unnamed, field, &format!("new-{field}")));
    let sighting = Sighting::read(&unnamed).unwrap();
    assert_eq!(sighting.browser, Browser::Other("new-browser".to_owned()));
    assert_eq!(sighting.system, Named::Other("new-os".to_owned()));
    assert_eq!(sighting.cpu_arch.to_string(), "new-cpu_arch");
    assert_eq!(sighting.gpu, GpuVendor::Other("new-gpu".to_owned()));
    assert_eq!(sighting.gpu.name(), "new-gpu");
    assert_eq!(sighting.backend, Backend::Other("new-backend".to_owned()));
    assert_eq!(sighting.window, Window::Other("new-window".to_owned()));
    assert_eq!(sighting.others(), ENUMERATED);
}

#[test]
fn a_row_naming_only_values_the_registry_knows_has_no_others() {
    assert_eq!(Sighting::read(&row()).unwrap().others(), Vec::<&str>::new());
}

#[test]
fn a_field_given_as_empty_text_is_missing() {
    for field in ENUMERATED {
        assert!(matches!(Sighting::read(&row_with(field, "")), Err(RowError::Missing(missing)) if missing == field));
    }
}

#[test]
fn a_sighting_on_an_os_or_cpu_the_registry_has_no_name_for_is_not_here() {
    let elsewhere = Sighting::read(&row_with("os", "nonesuch")).unwrap();
    assert!(!elsewhere.is_here());
    let here_but_unnamed_cpu = set(row_with("os", std::env::consts::OS), "cpu_arch", "nonesuch");
    assert!(!Sighting::read(&here_but_unnamed_cpu).unwrap().is_here());
}

#[test]
fn a_field_the_registry_does_not_know_is_refused() {
    assert_eq!(
        refusal(&row_with("colour", "blue")),
        "the registry gives `field` as `colour`, which is not one it knows"
    );
}

#[test]
fn a_row_lacking_a_field_every_row_carries_is_refused() {
    let mut lacking = row();
    let _removed: Option<Value> = lacking.as_object_mut().unwrap().remove("window");
    assert!(matches!(Sighting::read(&lacking), Err(RowError::Missing("window"))));
    assert_eq!(refusal(&lacking), "the registry has no `window` where one is required");
}

#[test]
fn an_entry_reads_as_each_row_it_lists() {
    let entry = json!({ "seen": [row(), row_with("browser", "deno")] });
    let browsers: Vec<Browser> = Sighting::read_entry(&entry)
        .unwrap()
        .iter()
        .map(|sighting| sighting.browser.clone())
        .collect();
    assert_eq!(browsers, [Browser::Brave, Browser::Deno]);
}

#[test]
fn an_entry_listing_no_rows_is_refused() {
    let entry = json!({ "label": "seen nowhere" });
    assert!(matches!(Sighting::read_entry(&entry), Err(RowError::Missing("seen"))));
}

#[test]
fn an_entry_listing_a_row_that_cannot_be_read_is_refused() {
    let entry = json!({ "seen": [row(), row_with("colour", "blue")] });
    assert_eq!(
        Sighting::read_entry(&entry).unwrap_err().to_string(),
        "the registry gives `field` as `colour`, which is not one it knows"
    );
}

#[test]
fn every_row_the_registry_holds_can_be_read() {
    let sightings = every_sighting().unwrap();
    assert!(sightings.len() > 100);
    assert!(sightings.iter().any(|sighting| sighting.gpu == GpuVendor::Apple));
    // Rows on a platform like this one, on its operating system with another architecture, and on neither.
    assert!(sightings.iter().any(Sighting::is_here));
    assert!(!sightings.iter().all(Sighting::is_here));
}

#[test]
fn a_signature_lists_each_place_it_was_seen() {
    let correct = Signature::new(HOST, reference().into_iter().map(Answer::Bits).collect());
    let seen = correct.seen().unwrap();
    assert!(seen.len() > 1);
    for sighting in seen {
        assert_eq!(sighting.browser, Browser::Absent);
        assert_eq!(sighting.gpu, GpuVendor::Absent);
        assert_eq!(sighting.backend, Backend::Absent);
        assert_eq!(sighting.window, Window::Absent);
    }
}

#[test]
fn a_signature_the_registry_has_not_seen_was_seen_nowhere() {
    let unseen = Signature::new(WEBGPU, reference().into_iter().map(Answer::Bits).collect());
    assert_eq!(unseen.seen().unwrap().len(), 0);
}
