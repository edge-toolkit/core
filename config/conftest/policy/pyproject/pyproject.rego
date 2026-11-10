# pyproject.toml policy, selected with --namespace pyproject.
# Evaluated over conftest's --combine input ({path, contents}).
package pyproject

import data.paths

# A `path = ".."` uv source points at the parent directory, which isn't a package -- almost always a mistake.
# Sibling packages are referenced by their specific relative path (e.g. ../../../generated/python-rest), not the
# bare parent.
deny contains msg if {
	some file in paths.files
	endswith(file.path, "pyproject.toml")
	some name, src in file.contents.tool.uv.sources
	src.path == ".."
	msg := sprintf("%s: [tool.uv.sources] %q uses path = \"..\"; point at the package path", [file.path, name])
}

# uv_build must be pinned to exactly the mise uv version.
# This makes `uv build` use the matching backend with no out-of-range warning. Bump both together when upgrading uv.
uv_version := v if {
	some file in paths.files
	endswith(file.path, ".mise/config.toml")
	v := file.contents.tools.uv
}

# A pyproject's uv_build requirement must pin the uv version mise installs.
deny contains msg if {
	some file in paths.files
	endswith(file.path, "pyproject.toml")
	some req in file.contents["build-system"].requires
	startswith(req, "uv_build")
	req != $"uv_build=={uv_version}"
	msg := sprintf("%s: pin uv_build==%s to match mise's uv (found %q)", [file.path, uv_version, req])
}

# A pyproject may not link this repository's GitHub issues in [project.urls], under `Issues` or any other label.
# Issues are switched off here, so the link would lead a package index's visitors to a page that does not exist;
# Source and Homepage remain.
deny contains msg if {
	some file in paths.files
	endswith(file.path, "pyproject.toml")
	some label, url in file.contents.project.urls
	issue_tracker_link(label, url)
	msg := sprintf("%s: [project.urls] %s links to GitHub issues, which are switched off here", [file.path, label])
}

# A link is to the issue tracker when its label says so, or when its URL leads to this repository's issues.
issue_tracker_link("Issues", _)

issue_tracker_link(_, url) if contains(lower(url), "github.com/edge-toolkit/core/issues")
