# Unit tests for the patch-pin rule, run by `conftest verify`.
#
# They exist because that rule turns entirely on the form of a version requirement, and most of the forms that
# decide it are ones the manifest does not hold: a bare `1.2.3`, a comparator in front of one, a prerelease tail
# whose own dots must not be counted, a path dep's mandatory full triple. Running the real task over the real
# manifest reaches at most one of those at a time, and only for as long as that dep survives a bump, so they are
# fed in synthetically here instead.
package cargo_test

import data.cargo

# A root manifest carrying nothing but the dependency table under test.
manifest(deps) := [{"path": "Cargo.toml", "contents": {"workspace": {"dependencies": deps}}}]

flagged(msgs, name) if {
	some msg in msgs
	contains(msg, $"\"{name}\"")
	contains(msg, "to a patch version")
}

stale(msgs, name) if {
	some msg in msgs
	contains(msg, $"patch_pin_exception entry \"{name}\" is stale")
}

# The two shapes the rule exists to require, and the one it exists to reject.
test_bare_major_is_accepted if {
	msgs := cargo.deny with input as manifest({"crate-a": "1"})
	not flagged(msgs, "crate-a")
}

test_major_minor_is_accepted if {
	msgs := cargo.deny with input as manifest({"crate-a": "1.2"})
	not flagged(msgs, "crate-a")
}

test_major_minor_patch_is_rejected if {
	msgs := cargo.deny with input as manifest({"crate-a": "1.2.3"})
	flagged(msgs, "crate-a")
}

# The table form carries the requirement under `version`, and must be read the same way as the string form.
test_table_form_is_rejected if {
	msgs := cargo.deny with input as manifest({"crate-a": {"version": "1.2.3", "features": ["derive"]}})
	flagged(msgs, "crate-a")
}

# A comparator must not launder a patch pin past the rule, nor shift the components it counts.
test_comparator_does_not_launder_a_patch_pin if {
	msgs := cargo.deny with input as manifest({"crate-a": "=1.2.3"})
	flagged(msgs, "crate-a")
}

test_comparator_on_major_minor_is_accepted if {
	msgs := cargo.deny with input as manifest({"crate-a": ">=1.2"})
	not flagged(msgs, "crate-a")
}

# A prerelease tail contributes dots of its own, which belong to the tail rather than to the version core.
test_prerelease_counts_only_the_version_core if {
	msgs := cargo.deny with input as manifest({"crate-a": "=2.0.0-rc.10"})
	flagged(msgs, "crate-a")
}

# A path dep has no two-part form available to it, so it is exempt without needing an exception entry.
test_path_dep_keeps_its_full_triple if {
	msgs := cargo.deny with input as manifest({"et-a": {"path": "libs/a", "version": "0.1.0"}})
	not flagged(msgs, "et-a")
}

# An excepted dep keeps its pin, and the entry that permits it is what stops the denial.
test_excepted_dep_keeps_its_patch_pin if {
	msgs := cargo.deny with input as manifest({"deno_error": "=0.7.1", "ort": "=2.0.0-rc.10"})
	not flagged(msgs, "deno_error")
}

test_exception_only_covers_the_dep_it_names if {
	deps := {"deno_error": "=0.7.1", "ort": "=2.0.0-rc.10", "other": "1.2.3"}
	msgs := cargo.deny with input as manifest(deps)
	flagged(msgs, "other")
}

# The map is only honest while every entry still describes the manifest, so both ways of going stale report.
test_trimmed_dep_reports_its_stale_exception if {
	msgs := cargo.deny with input as manifest({"deno_error": "=0.7.1", "ort": "2.0"})
	stale(msgs, "ort")
}

test_dropped_dep_reports_its_stale_exception if {
	msgs := cargo.deny with input as manifest({"deno_error": "=0.7.1"})
	stale(msgs, "ort")
}

test_live_exceptions_report_nothing if {
	msgs := cargo.deny with input as manifest({"deno_error": "=0.7.1", "ort": "=2.0.0-rc.10"})
	not stale(msgs, "deno_error")
	not stale(msgs, "ort")
}

tracing_log_drift(msgs) if {
	some msg in msgs
	contains(msg, "\"tracing\" must enable its \"log\" feature")
}

test_tracing_without_features_is_rejected if {
	msgs := cargo.deny with input as manifest({"tracing": "0.1"})
	tracing_log_drift(msgs)
}

test_tracing_with_other_features_is_rejected if {
	msgs := cargo.deny with input as manifest({"tracing": {"version": "0.1", "features": ["attributes"]}})
	tracing_log_drift(msgs)
}

test_tracing_with_log_is_accepted if {
	msgs := cargo.deny with input as manifest({"tracing": {"version": "0.1", "features": ["log"]}})
	not tracing_log_drift(msgs)
}

test_workspace_without_tracing_is_accepted if {
	msgs := cargo.deny with input as manifest({"serde": "1.0"})
	not tracing_log_drift(msgs)
}

cargo_crate(msgs) if {
	some msg in msgs
	contains(msg, "is a cargo-driving crate")
}

test_cargo_underscore_crate_is_rejected if {
	msgs := cargo.deny with input as manifest({"cargo_metadata": "0.19"})
	cargo_crate(msgs)
}

test_cargo_hyphen_crate_is_rejected if {
	deps := {"cargo-toml": {"workspace": true}}
	msgs := cargo.deny with input as [{"path": "libs/a/Cargo.toml", "contents": {"dependencies": deps}}]
	cargo_crate(msgs)
}

test_crate_merely_containing_cargo_is_accepted if {
	msgs := cargo.deny with input as manifest({"serde-cargo-ish": "1.0"})
	not cargo_crate(msgs)
}

log_dep(msgs) if {
	some msg in msgs
	contains(msg, "dependency \"log\" is banned")
}

test_member_log_dependency_is_rejected if {
	deps := {"log": {"workspace": true}}
	msgs := cargo.deny with input as [{"path": "libs/a/Cargo.toml", "contents": {"dependencies": deps}}]
	log_dep(msgs)
}

test_exempt_member_log_dependency_is_accepted if {
	deps := {"log": {"workspace": true}}
	msgs := cargo.deny with input as [{"path": "services/storage/Cargo.toml", "contents": {"dependencies": deps}}]
	not log_dep(msgs)
}

test_workspace_log_entry_is_accepted if {
	msgs := cargo.deny with input as manifest({"log": "0.4"})
	not log_dep(msgs)
}

tokio_full(msgs) if {
	some msg in msgs
	contains(msg, "tokio's \"full\" feature is banned")
}

test_member_tokio_full_is_rejected if {
	deps := {"tokio": {"workspace": true, "features": ["full"]}}
	msgs := cargo.deny with input as [{"path": "libs/a/Cargo.toml", "contents": {"dependencies": deps}}]
	tokio_full(msgs)
}

test_workspace_tokio_full_is_rejected if {
	msgs := cargo.deny with input as manifest({"tokio": {"version": "1", "features": ["full"]}})
	tokio_full(msgs)
}

test_target_scoped_tokio_full_is_rejected if {
	deps := {"tokio": {"workspace": true, "features": ["macros", "full"]}}
	unix := {"cfg(unix)": {"dev-dependencies": deps}}
	msgs := cargo.deny with input as [{"path": "libs/a/Cargo.toml", "contents": {"target": unix}}]
	tokio_full(msgs)
}

test_named_tokio_features_are_accepted if {
	deps := {"tokio": {"workspace": true, "features": ["rt", "signal"]}}
	msgs := cargo.deny with input as [{"path": "libs/a/Cargo.toml", "contents": {"dependencies": deps}}]
	not tokio_full(msgs)
}

# A member manifest carrying only the tables the document-features rule reads.
member(path, contents) := [{"path": path, "contents": contents}]

feature_docs_flagged(msgs, problem) if {
	some msg in msgs
	contains(msg, "to publish their docs")
	contains(msg, problem)
}

feature_docs_clean(msgs) if not any_feature_docs(msgs)

any_feature_docs(msgs) if {
	some msg in msgs
	contains(msg, "to publish their docs")
}

optional_dep := {"document-features": {"workspace": true, "optional": true}}

docs_feature := {"extra": [], "docs": ["dep:document-features"]}

docs_rs_docs := {"metadata": {"docs": {"rs": {"features": ["docs"]}}}}

# A manifest meeting the whole contract, which the cases below each break in one place.
compliant := {"features": docs_feature, "dependencies": optional_dep, "package": docs_rs_docs}

test_compliant_crate_is_accepted if {
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", compliant)
	feature_docs_clean(msgs)
}

test_docs_rs_all_features_is_accepted if {
	all_features := {"metadata": {"docs": {"rs": {"all-features": true}}}}
	contents := {"features": docs_feature, "dependencies": optional_dep, "package": all_features}
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", contents)
	feature_docs_clean(msgs)
}

test_target_scoped_document_features_is_accepted if {
	wasi := {"cfg(target_os = \"wasi\")": {"dependencies": optional_dep}}
	contents := {"features": docs_feature, "target": wasi, "package": docs_rs_docs}
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", contents)
	feature_docs_clean(msgs)
}

test_missing_dependency_is_rejected if {
	contents := {"features": docs_feature, "package": docs_rs_docs}
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", contents)
	feature_docs_flagged(msgs, "must depend on document-features")
}

# A dev-dependency renders nothing on docs.rs, so it does not satisfy the rule.
test_dev_dependency_does_not_count if {
	contents := {"features": docs_feature, "dev-dependencies": optional_dep, "package": docs_rs_docs}
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", contents)
	feature_docs_flagged(msgs, "must depend on document-features")
}

test_non_optional_dependency_is_rejected if {
	required := {"document-features": {"workspace": true}}
	contents := {"features": docs_feature, "dependencies": required, "package": docs_rs_docs}
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", contents)
	feature_docs_flagged(msgs, "must be optional")
}

test_missing_docs_feature_is_rejected if {
	contents := {"features": {"extra": [], "docs": []}, "dependencies": optional_dep, "package": docs_rs_docs}
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", contents)
	feature_docs_flagged(msgs, "must declare `docs")
}

test_docs_rs_without_docs_feature_is_rejected if {
	no_docs := {"metadata": {"docs": {"rs": {"features": []}}}}
	contents := {"features": docs_feature, "dependencies": optional_dep, "package": no_docs}
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", contents)
	feature_docs_flagged(msgs, "must enable its `docs` feature")
}

test_default_and_docs_alone_need_nothing if {
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", {"features": {"default": [], "docs": []}})
	feature_docs_clean(msgs)
}

test_crate_without_features_needs_nothing if {
	msgs := cargo.deny with input as member("libs/a/Cargo.toml", {"package": {"name": "et-a"}})
	feature_docs_clean(msgs)
}

test_generated_rest_client_is_exempt if {
	msgs := cargo.deny with input as member("generated/rust-rest/Cargo.toml", {"features": {"tracing": []}})
	feature_docs_clean(msgs)
}
