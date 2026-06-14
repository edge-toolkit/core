# pyproject.toml policy, evaluated over conftest's --combine input ({path,
# contents}); selected with --namespace pyproject.
package pyproject

# A `path = ".."` uv source points at the parent directory, which isn't a package
# -- almost always a mistake. Sibling packages are referenced by their specific
# relative path (e.g. ../../../generated/python-rest), not the bare parent.
deny contains msg if {
	some file in input
	endswith(file.path, "pyproject.toml")
	some name, src in file.contents.tool.uv.sources
	src.path == ".."
	msg := sprintf("%s: [tool.uv.sources] %q uses path = \"..\"; point at the package path", [file.path, name])
}
