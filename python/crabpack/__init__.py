"""Python package for the `crabpack` virtual environment packer."""
from __future__ import annotations

from importlib import metadata as _metadata

from .crabpack import pack

__all__ = ["pack"]


def __getattr__(name: str):
    if name == "__version__":
        try:
            return _metadata.version("crabpack")
        except _metadata.PackageNotFoundError:  # pragma: no cover - during local dev
            return "0.0.0"
    raise AttributeError(name)
