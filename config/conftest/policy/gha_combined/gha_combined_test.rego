# Unit tests for the path-aware workflow rules, run by `conftest verify`.
#
# Every real workflow already triggers correctly, so the real task cannot tell a compliant workflow from a rule that
# stopped matching.
package gha_combined_test

import data.gha_combined

path := ".github/workflows/probe.yaml"

# A workflow whose `on:` block is `on`, under the quoted key the repo's workflows use.
workflow(on) := [{"path": path, "contents": {"on": on}}]

reports(msgs, fragment) if {
	some msg in msgs
	contains(msg, fragment)
}

test_a_workflow_with_both_triggers_is_accepted if {
	msgs := gha_combined.deny with input as workflow({"pull_request": {}, "workflow_dispatch": null})
	count(msgs) == 0
}

test_a_workflow_without_pull_request_is_denied if {
	msgs := gha_combined.deny with input as workflow({"push": {}, "workflow_dispatch": null})
	reports(msgs, "must trigger on `pull_request`")
}

test_a_workflow_without_workflow_dispatch_is_denied if {
	msgs := gha_combined.deny with input as workflow({"pull_request": {}})
	reports(msgs, "must include `workflow_dispatch:`")
}

test_a_paths_filter_naming_the_workflow_is_accepted if {
	on := {"pull_request": {"paths": ["src/**", path]}, "workflow_dispatch": null}
	not reports(gha_combined.deny, "must include the workflow itself") with input as workflow(on)
}

test_a_paths_filter_missing_the_workflow_is_denied if {
	on := {"pull_request": {"paths": ["src/**"]}, "workflow_dispatch": null}
	reports(gha_combined.deny, "must include the workflow itself") with input as workflow(on)
}

test_a_windows_path_is_still_checked if {
	windows := [{"path": `.github\workflows\probe.yaml`, "contents": {"on": {"pull_request": {}}}}]
	reports(gha_combined.deny, "must include `workflow_dispatch:`") with input as windows
}

test_a_windows_path_still_finds_itself_in_the_paths_filter if {
	on := {"pull_request": {"paths": [path]}, "workflow_dispatch": null}
	windows := [{"path": `.github\workflows\probe.yaml`, "contents": {"on": on}}]
	not reports(gha_combined.deny, "must include the workflow itself") with input as windows
}

test_a_file_outside_the_workflows_dir_is_not_checked if {
	msgs := gha_combined.deny with input as [{"path": ".github/actions/x/action.yaml", "contents": {"runs": {}}}]
	count(msgs) == 0
}
