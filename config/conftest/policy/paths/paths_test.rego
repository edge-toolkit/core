# Unit tests for the shared path normalisation, run by `conftest verify`.
#
# A normaliser that stopped normalising would leave every policy built on it checking nothing on Windows while
# every other lane stays green, so the Windows spelling is the input that matters here.
package paths_test

import data.paths

test_backslashes_become_forward_slashes if {
	paths.normalised(`.github\workflows\check.yaml`) == ".github/workflows/check.yaml"
}

test_a_forward_slash_path_is_unchanged if {
	paths.normalised(".mise/config.toml") == ".mise/config.toml"
}

test_files_normalise_the_path_and_keep_the_contents if {
	files := paths.files with input as [{"path": `.mise\config.toml`, "contents": {"tools": {}}}]
	files == [{"path": ".mise/config.toml", "contents": {"tools": {}}}]
}

test_files_keep_every_file_in_order if {
	files := paths.files with input as [{"path": "b", "contents": 1}, {"path": `a\b`, "contents": 2}]
	[f.path | some f in files] == ["b", "a/b"]
}
