"""Basic smoke tests for the Python package."""

def test_crabpack_imports():
    import importlib

    module = importlib.import_module("crabpack")
    assert hasattr(module, "pack")
