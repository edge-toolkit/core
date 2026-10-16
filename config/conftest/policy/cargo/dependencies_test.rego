# Unit tests for the dependency rules, run by `conftest verify`.
#
# The real manifests hold none of the shapes these rules exist to reject -- a dependency on an internal crate, an
# `et-` dependency of the standalone crate, or either one reached through a `package` rename -- so a run over them
# proves only that the rules stay quiet. The cases are fed in synthetically here instead.
package cargo_test

import data.cargo

depends_on_internal(msgs, name) if {
	some msg in msgs
	contains(msg, $"depends on internal crate \"{name}\"")
}

standalone_flagged(msgs, name) if {
	some msg in msgs
	contains(msg, $"must not depend on this repo's \"{name}\"")
}

test_a_dependency_on_an_internal_crate_is_rejected_in_any_table if {
	contents := {
		"dependencies": {"int-wasm-cov-wrapper": {"workspace": true}},
		"dev-dependencies": {"et-int-gen": {"workspace": true}},
	}
	msgs := cargo.deny with input as member("services/a/Cargo.toml", contents)
	depends_on_internal(msgs, "int-wasm-cov-wrapper")
	depends_on_internal(msgs, "et-int-gen")
}

# The marker test is a prefix, so a third-party name that merely contains `int-` passes.
test_a_name_merely_containing_int_is_not_internal if {
	contents := {"dependencies": {"color-print-proc-macro": {"workspace": true}}}
	msgs := cargo.deny with input as member("services/a/Cargo.toml", contents)
	not depends_on_internal(msgs, "color-print-proc-macro")
}

test_the_standalone_crate_may_not_depend_on_an_et_crate if {
	contents := {
		"package": {"name": "et-actix-web-config"},
		"dependencies": {"et-org": {"workspace": true}, "serde": {"workspace": true}},
	}
	msgs := cargo.deny with input as member("libs/actix-web-config/Cargo.toml", contents)
	standalone_flagged(msgs, "et-org")
	not standalone_flagged(msgs, "serde")
}

test_any_other_crate_may_depend_on_an_et_crate if {
	contents := {"package": {"name": "et-ws-server"}, "dependencies": {"et-actix-web-config": {"workspace": true}}}
	msgs := cargo.deny with input as member("services/ws-server/Cargo.toml", contents)
	not standalone_flagged(msgs, "et-actix-web-config")
}

# A key that renames its package is judged by the package it names, not by the key.
test_a_rename_on_the_member_entry_is_resolved if {
	contents := {
		"package": {"name": "et-actix-web-config"},
		"dependencies": {"org": {"package": "et-org", "version": "0.2"}},
		"dev-dependencies": {"wrapper": {"package": "int-wasm-cov-wrapper", "version": "0.1"}},
	}
	msgs := cargo.deny with input as member("libs/actix-web-config/Cargo.toml", contents)
	standalone_flagged(msgs, "et-org")
	depends_on_internal(msgs, "int-wasm-cov-wrapper")
}

test_a_rename_on_the_inherited_workspace_entry_is_resolved if {
	root := {"path": "Cargo.toml", "contents": {"workspace": {"dependencies": {
		"org": {"package": "et-org", "path": "libs/org", "version": "0.2.0"},
		"gen": {"package": "et-int-gen", "path": "utilities/int-gen", "version": "0.1.0"},
	}}}}
	crate := {"path": "libs/actix-web-config/Cargo.toml", "contents": {
		"package": {"name": "et-actix-web-config"},
		"dependencies": {"org": {"workspace": true}, "gen": {"workspace": true}},
	}}
	msgs := cargo.deny with input as [root, crate]
	standalone_flagged(msgs, "et-org")
	depends_on_internal(msgs, "et-int-gen")
}
