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

# A workflow whose one job is `job`.
timed(job) := {"name": "probe", "jobs": {"build": job}}

over_cap(msgs) if {
	some msg in msgs
	contains(msg, "timeout-minutes can be")
}

unbounded(msgs) if {
	some msg in msgs
	contains(msg, "sets no timeout-minutes")
}

test_job_at_the_cap_is_allowed if {
	not over_cap(gha.deny) with input as timed({"timeout-minutes": 150})
}

test_job_over_the_cap_is_denied if {
	over_cap(gha.deny) with input as timed({"timeout-minutes": 151})
}

test_job_without_a_timeout_is_denied if {
	unbounded(gha.deny) with input as timed({"steps": []})
}

test_reusable_workflow_job_needs_no_timeout if {
	not unbounded(gha.deny) with input as timed({"uses": "./.github/workflows/other.yaml"})
}

test_number_in_an_expression_is_read if {
	over_cap(gha.deny) with input as timed({"timeout-minutes": "${{ matrix.language == 'rust' && 160 || 15 }}"})
}

test_expression_within_the_cap_is_allowed if {
	not over_cap(gha.deny) with input as timed({"timeout-minutes": "${{ matrix.language == 'rust' && 60 || 15 }}"})
}

test_matrix_include_timeout_over_the_cap_is_denied if {
	job := {
		"timeout-minutes": "${{ matrix.timeout }}",
		"strategy": {"matrix": {"include": [{"os": "a", "timeout": 40}, {"os": "b", "timeout": 180}]}},
	}
	over_cap(gha.deny) with input as timed(job)
}

test_matrix_list_timeout_over_the_cap_is_denied if {
	job := {"timeout-minutes": "${{ matrix.timeout }}", "strategy": {"matrix": {"timeout": [40, 200]}}}
	over_cap(gha.deny) with input as timed(job)
}

test_matrix_json_string_timeout_over_the_cap_is_denied if {
	include := `${{ fromJSON('[{"os":"a","timeout":225}]') }}`
	job := {"timeout-minutes": "${{ matrix.timeout }}", "strategy": {"matrix": {"include": include}}}
	over_cap(gha.deny) with input as timed(job)
}

test_other_matrix_timeout_keys_are_not_the_job_timeout if {
	include := `${{ fromJSON('[{"os":"a","timeout":150,"install_timeout":200}]') }}`
	job := {"timeout-minutes": "${{ matrix.timeout }}", "strategy": {"matrix": {"include": include}}}
	not over_cap(gha.deny) with input as timed(job)
}

test_digits_in_a_quoted_runner_label_are_not_a_timeout if {
	not over_cap(gha.deny) with input as timed({"timeout-minutes": "${{ matrix.os == 'windows-2022' && 90 || 60 }}"})
}

test_digits_in_a_hyphenated_name_are_not_a_timeout if {
	not over_cap(gha.deny) with input as timed({"timeout-minutes": "${{ matrix.node-180 && 90 || 60 }}"})
}

test_nested_matrix_timeout_is_not_the_job_timeout if {
	entry := {"os": "a", "timeout": 30, "config": {"timeout": 200}}
	job := {"timeout-minutes": "${{ matrix.timeout }}", "strategy": {"matrix": {"include": [entry]}}}
	not over_cap(gha.deny) with input as timed(job)
}

test_bracket_matrix_timeout_over_the_cap_is_denied if {
	job := {"timeout-minutes": "${{ matrix['timeout'] }}", "strategy": {"matrix": {"timeout": [200]}}}
	over_cap(gha.deny) with input as timed(job)
}

test_matrix_timeouts_are_ignored_when_the_job_does_not_read_them if {
	job := {"timeout-minutes": 30, "strategy": {"matrix": {"include": [{"os": "a", "timeout": 500}]}}}
	not over_cap(gha.deny) with input as timed(job)
}
