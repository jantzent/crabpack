"""Python package for the `crabpack` virtual environment packer."""
from __future__ import annotations

from importlib import metadata as _metadata

from .progress import format_time, progressbar

try:  # pragma: no cover - depends on the Rust extension being built
    from .crabpack import pack
except ImportError:  # pragma: no cover - exercised indirectly in tests
    def pack(*_args, **_kwargs):
        raise ImportError(
            "The crabpack native module is not available. "
            "Build the project first (see README)."
        )


__all__ = ["pack", "progressbar", "format_time"]


def __getattr__(name: str):
    if name == "__version__":
        try:
            return _metadata.version("crabpack")
        except _metadata.PackageNotFoundError:  # pragma: no cover - during local dev
            return "0.0.0"
    raise AttributeError(name)
