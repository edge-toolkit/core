"""Who this project is, as the strings that carry its identity into registries, images and manifests.

The Python twin of the Rust `et-org` crate. The organisation name is written exactly once, in `ORG`, and every
longer string is assembled from it; a test in the Rust crate fails if `ORG` here stops matching the Rust one.
"""

ORG = "edge-toolkit"

CRATE_PREFIX = "et-"

NPM_SCOPE = f"@{ORG}/"

REPOSITORY_URL = f"https://github.com/{ORG}/core"

IMAGE_REGISTRY = f"ghcr.io/{ORG}/core"


def served_npm_module_path(distribution: str) -> str:
    """Return the path the hub serves the module published as `distribution` under, `/modules/<scope><name>`."""
    return f"/modules/{NPM_SCOPE}{distribution}"
