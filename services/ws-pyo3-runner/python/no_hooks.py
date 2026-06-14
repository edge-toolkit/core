"""Test fixture for et-ws-pyo3-runner's load-time hook sanity check.

Intentionally defines none of the runner hooks (init / on_connect /
on_text_frame / on_binary_frame / on_shutdown), so importing it must fail fast
rather than register an agent that could never be driven. Used by
tests/no_hooks.rs.
"""
