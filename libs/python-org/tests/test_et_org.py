"""The assembled identity strings, pinned to the exact text registries and manifests are matched against."""

from et_org import CRATE_PREFIX, IMAGE_REGISTRY, NPM_SCOPE, ORG, REPOSITORY_URL, served_npm_module_path


def test_the_identity_strings_are_what_registries_are_matched_against() -> None:
    assert ORG == "edge-toolkit"
    assert CRATE_PREFIX == "et-"
    assert NPM_SCOPE == "@edge-toolkit/"
    assert REPOSITORY_URL == "https://github.com/edge-toolkit/core"
    assert IMAGE_REGISTRY == "ghcr.io/edge-toolkit/core"


def test_a_served_npm_module_path_names_the_distribution_under_the_scope() -> None:
    assert served_npm_module_path("et-ws-pyface1") == "/modules/@edge-toolkit/et-ws-pyface1"
