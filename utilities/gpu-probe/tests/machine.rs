//! Covers what a row says of the machine a measurement ran on: the system's release and CPU, the device or the GitHub
//! Actions runner image named in its place, the day, the run's URL, and how those join a row and a label.
#![cfg(test)]

use std::collections::BTreeMap;

use et_gpu_probe_server::machine::{Machine, environment};
use pretty_assertions::assert_eq;
use temp_env::with_vars;

/// The variables the machine reads, all unset.
const UNSET: [(&str, Option<&str>); 6] = [
    ("ImageOS", None),
    ("ImageVersion", None),
    ("GITHUB_SERVER_URL", None),
    ("GITHUB_REPOSITORY", None),
    ("GITHUB_RUN_ID", None),
    ("GPU_PROBE_DEVICE", None),
];

/// The variables a run on a `macos-latest` runner sets that the machine reads, a device named beside them.
const RUNNER: [(&str, Option<&str>); 6] = [
    ("ImageOS", Some("macos15")),
    ("ImageVersion", Some("20261110.1")),
    ("GITHUB_SERVER_URL", Some("https://github.com")),
    ("GITHUB_REPOSITORY", Some("edge-toolkit/core")),
    ("GITHUB_RUN_ID", Some("45112698960")),
    ("GPU_PROBE_DEVICE", Some("ignored on a runner")),
];

/// The variables the machine reads off a runner, `GPU_PROBE_DEVICE` naming `device` and the rest unset.
fn named(device: &'static str) -> [(&'static str, Option<&'static str>); 6] {
    let mut vars = UNSET;
    vars[5].1 = Some(device);
    vars
}

#[test]
fn a_machine_off_a_runner_is_named_as_given_and_gives_its_release_cpu_and_day() {
    let machine = with_vars(UNSET, || Machine::here("M5 Pro MacBook Pro")).unwrap();
    let fields = machine.fields();
    assert_eq!(fields.get("device").map(String::as_str), Some("M5 Pro MacBook Pro"));
    assert!(fields.get("os_version").is_some_and(|release| !release.is_empty()));
    assert!(fields.get("cpu_version").is_some_and(|cpu| !cpu.is_empty()));
    let date = fields.get("date").unwrap();
    assert_eq!(
        jiff::civil::Date::strptime("%Y-%m-%d", date).unwrap().to_string(),
        *date,
    );
    assert_eq!(fields.get("source"), None);
}

#[test]
fn a_machine_off_a_runner_named_nothing_gives_no_device() {
    let machine = with_vars(UNSET, || Machine::here("")).unwrap();
    assert_eq!(machine.fields().get("device"), None);
}

#[test]
fn a_machine_named_nothing_is_named_by_the_environment() {
    let machine = with_vars(named("MacBook Pro (16-inch, 2019)"), || Machine::here("")).unwrap();
    assert_eq!(
        machine.fields().get("device").map(String::as_str),
        Some("MacBook Pro (16-inch, 2019)"),
    );
}

#[test]
fn a_device_given_outranks_the_environments() {
    let machine = with_vars(named("a desktop"), || Machine::here("M5 Pro MacBook Pro")).unwrap();
    assert_eq!(
        machine.fields().get("device").map(String::as_str),
        Some("M5 Pro MacBook Pro"),
    );
}

#[test]
fn a_runner_is_named_by_its_image_and_the_run_it_is_part_of() {
    let machine = with_vars(RUNNER, || Machine::here("")).unwrap();
    let fields = machine.fields();
    assert_eq!(
        fields.get("device").map(String::as_str),
        Some("GitHub Actions runner image macos15 20261110.1"),
    );
    assert_eq!(
        fields.get("source").map(String::as_str),
        Some("https://github.com/edge-toolkit/core/actions/runs/45112698960"),
    );
}

#[test]
fn a_row_keeps_its_own_fields_and_gains_the_machines() {
    let machine = Machine::with(&[("os_version", "15.7.2"), ("device", "a runner")]);
    let row = BTreeMap::from([
        ("os".to_owned(), "macos".to_owned()),
        ("device".to_owned(), "the row's own".to_owned()),
    ]);
    let joined = machine.row(row);
    assert_eq!(joined.get("os").map(String::as_str), Some("macos"));
    assert_eq!(joined.get("os_version").map(String::as_str), Some("15.7.2"));
    assert_eq!(joined.get("device").map(String::as_str), Some("the row's own"));
}

#[test]
fn a_label_names_the_release_then_the_device_where_each_is_known() {
    let both = Machine::with(&[("os_version", "15.7.2"), ("device", "a runner")]);
    assert_eq!(both.named("macos", "aarch64"), "macos 15.7.2 aarch64, a runner");
    let device = Machine::with(&[("device", "a runner")]);
    assert_eq!(device.named("macos", "aarch64"), "macos aarch64, a runner");
    assert_eq!(Machine::with(&[]).named("macos", ""), "macos");
    assert_eq!(Machine::default().named("", ""), "");
}

/// `pairs` as owned environment variables.
fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn only_driver_settings_are_kept_in_name_order() {
    let settings = environment(vars(&[
        ("PATH", "/usr/bin"),
        ("VK_ICD_FILENAMES", "/usr/share/vulkan/icd.d/intel_icd.x86_64.json"),
        ("INTEL_DEBUG", "nofc"),
        ("HOME", "/home/someone"),
        ("__GL_SYNC_TO_VBLANK", "0"),
    ]));
    assert_eq!(
        settings,
        "INTEL_DEBUG=nofc VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/intel_icd.x86_64.json __GL_SYNC_TO_VBLANK=0",
    );
    assert_eq!(environment(vars(&[("PATH", "/usr/bin")])), "");
}

#[test]
fn the_vendor_glvnd_loads_is_a_driver_setting() {
    let settings = environment(vars(&[
        ("__GLX_VENDOR_LIBRARY_NAME", "mesa"),
        (
            "__EGL_VENDOR_LIBRARY_FILENAMES",
            "/usr/share/glvnd/egl_vendor.d/50_mesa.json",
        ),
    ]));
    assert_eq!(
        settings,
        "__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json __GLX_VENDOR_LIBRARY_NAME=mesa",
    );
}

#[test]
fn this_machine_gives_the_driver_settings_in_force() {
    let mut set = UNSET.to_vec();
    set.push(("INTEL_DEBUG", Some("nofc")));
    let machine = with_vars(set, || Machine::here("a desktop")).unwrap();
    let settings = machine.fields().get("environment").unwrap();
    assert!(
        settings.split(' ').any(|setting| setting == "INTEL_DEBUG=nofc"),
        "{settings}",
    );
}
