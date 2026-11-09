# Unit tests for the pyproject.toml policy, run by `conftest verify`.
#
# The real task only sees pyprojects that already comply, which looks the same as a rule that never matches.
package pyproject_test

import data.pyproject

mise_uv := {"path": ".mise/config.toml", "contents": {"tools": {"uv": "0.13.3"}}}

project(contents) := [mise_uv, {"path": "modules/web/pymath1/pyproject.toml", "contents": contents}]

parent_source(msgs) if {
	some msg in msgs
	contains(msg, "uses path = \"..\"")
}

unpinned(msgs) if {
	some msg in msgs
	contains(msg, "to match mise's uv")
}

issues_link(msgs) if {
	some msg in msgs
	contains(msg, "links to GitHub issues")
}

test_a_source_at_the_parent_directory_is_denied if {
	contents := {"tool": {"uv": {"sources": {"et": {"path": ".."}}}}}
	parent_source(pyproject.deny) with input as project(contents)
}

test_a_source_at_a_package_path_is_accepted if {
	contents := {"tool": {"uv": {"sources": {"et": {"path": "../../../generated/python-rest"}}}}}
	not parent_source(pyproject.deny) with input as project(contents)
}

test_uv_build_pinned_to_the_mise_uv_is_accepted if {
	contents := {"build-system": {"requires": ["uv_build==0.13.3"]}}
	not unpinned(pyproject.deny) with input as project(contents)
}

test_uv_build_pinned_to_another_version_is_denied if {
	contents := {"build-system": {"requires": ["uv_build==0.13.2"]}}
	unpinned(pyproject.deny) with input as project(contents)
}

test_a_uv_build_range_is_denied if {
	contents := {"build-system": {"requires": ["uv_build>=0.13,<0.14"]}}
	unpinned(pyproject.deny) with input as project(contents)
}

test_other_build_requirements_are_not_checked if {
	contents := {"build-system": {"requires": ["hatchling>=1"]}}
	not unpinned(pyproject.deny) with input as project(contents)
}

test_an_issues_link_is_denied if {
	contents := {"project": {"urls": {"Issues": "https://github.com/edge-toolkit/core/issues"}}}
	issues_link(pyproject.deny) with input as project(contents)
}

test_the_issues_url_under_another_label_is_denied if {
	contents := {"project": {"urls": {"Bug Tracker": "https://GitHub.com/edge-toolkit/core/issues/new"}}}
	issues_link(pyproject.deny) with input as project(contents)
}

test_the_other_links_are_accepted if {
	contents := {"project": {"urls": {"Source": "https://github.com/edge-toolkit/core"}}}
	not issues_link(pyproject.deny) with input as project(contents)
}
