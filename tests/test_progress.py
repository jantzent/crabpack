"""Tests for the Python progress bar utilities."""

from __future__ import annotations

import io
from typing import Iterable

import pytest

from crabpack.progress import format_time, progressbar


@pytest.mark.parametrize(
    "seconds, expected",
    [
        (0.4, " 0.4s"),
        (10.4, "10.4s"),
        (100.4, " 1min 40.4s"),
        (3661.2, " 1hr  1min  1.2s"),
    ],
)
def test_format_time(seconds: float, expected: str) -> None:
    assert format_time(seconds) == expected


def test_progressbar_writes_output(monkeypatch: pytest.MonkeyPatch) -> None:
    iterable: Iterable[int] = range(3)
    buffer = io.StringIO()

    # Speed up the background thread while keeping the behaviour identical.
    monkeypatch.setattr("crabpack.progress.time.sleep", lambda _t: None)

    with progressbar(iterable, file=buffer) as it:
        for _ in it:
            pass

    output = buffer.getvalue()
    assert "] | 100% Completed |" in output
    assert output.endswith("\n")
