"""pywasm1 smoke module: cowsay a line under rustpython compiled to WASM.

Renders the line through the mise-managed `pipx:cowsay` package -- the same
pure-Python fixture the et-ws-pyo3-runner cowsay test uses -- to prove a
mise-installed package imports and runs under rustpython-on-WASM.
"""

import cowsay

cowsay.cow("hello from pywasm1 (rustpython compiled to WASM)")
