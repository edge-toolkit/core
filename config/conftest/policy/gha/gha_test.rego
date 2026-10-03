package gha_test

import data.gha

# A workflow as the YAML parser hands it over: a bare `on:` key arrives as the boolean `true`.
workflow(paths) := {
	"name": "probe",
	"true": {"pull_request": {"paths": paths}},
	"jobs": {"build": {"steps": [{"uses": "$/.github/actions/install-mise"}]}},
}

untriggered(msgs) if {
	some msg in msgs
	contains(msg, "but on.pull_request.paths doesn't include it")
}

test_local_action_outside_the_paths_filter_is_denied if {
	untriggered(gha.deny) with input as workflow(["src/**"])
}

test_local_action_inside_the_paths_filter_is_allowed if {
	not untriggered(gha.deny) with input as workflow(["src/**", ".github/actions/install-mise/**"])
}

test_quoted_on_key_is_read_too if {
	quoted := object.union(object.remove(workflow(["src/**"]), ["true"]), {"on": {"pull_request": {"paths": ["src/**"]}}})
	untriggered(gha.deny) with input as quoted
}
