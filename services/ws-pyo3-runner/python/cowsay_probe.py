"""Test fixture for `et-ws-pyo3-runner`: proves a mise-preinstalled pipx package
is importable from the embedded interpreter.

`cowsay` is declared as `pipx:cowsay` in the always-loaded mise config, and the
runner puts every mise `pipx:` package's site-packages on `sys.path` via
`edge_toolkit::config::mise_python_site_packages`. So the top-level `import
cowsay` below succeeds WITHOUT the operator adding it to PYO3_PYTHONPATH -- if
the runner didn't wire that path in, this import would fail the whole module
load. Exercised by `tests/cowsay.rs`.
"""

from __future__ import annotations

import cowsay


def on_text_frame(text: str) -> str:
    """Render the inbound text through cowsay and return it, so the round-trip
    proves cowsay both imported and actually runs."""
    return cowsay.get_output_string("cow", text)
