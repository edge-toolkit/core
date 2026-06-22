# pyproject.toml policy, selected with --namespace pyproject.
# Evaluated over conftest's --combine input ({path, contents}).
package pyproject

# A `path = ".."` uv source points at the parent directory, which isn't a package -- almost always a mistake.
# Sibling packages are referenced by their specific relative path (e.g. ../../../generated/python-rest), not the
# bare parent.
deny contains msg if {
	some file in input
	endswith(file.path, "pyproject.toml")
	some name, src in file.contents.tool.uv.sources
	src.path == ".."
	msg := sprintf("%s: [tool.uv.sources] %q uses path = \"..\"; point at the package path", [file.path, name])
}

# uv_build must be pinned to exactly the mise uv version.
# This makes `uv build` use the matching backend with no out-of-range warning. Bump both together when upgrading uv.
uv_version := v if {
	some file in input
	endswith(file.path, ".mise/config.toml")
	v := file.contents.tools.uv
}

deny contains msg if {
	some file in input
	endswith(file.path, "pyproject.toml")
	some req in file.contents["build-system"].requires
	startswith(req, "uv_build")
	req != sprintf("uv_build==%s", [uv_version])
	msg := sprintf("%s: pin uv_build==%s to match mise's uv (found %q)", [file.path, uv_version, req])
}
