"""Progress reporting utilities used by the Python packaging interface.

The implementation is intentionally a direct Python port of the progress bar
used by ``venv-pack``.  Keeping this code in Python avoids introducing a Rust
extension module for what ultimately boils down to formatting text and writing
it to an ``io`` stream; the heavy work still happens in Python land during the
iteration that drives the pack operation.
"""

from __future__ import absolute_import, division

import sys
import threading
import time
from timeit import default_timer
from typing import Generic, Iterable, Iterator, Optional, TextIO, TypeVar

__all__ = ["progressbar", "format_time"]

T = TypeVar("T")


def format_time(t: float) -> str:
    """Format seconds into a human readable form.

    >>> format_time(10.4)
    '10.4s'
    >>> format_time(1000.4)
    '16min 40.4s'
    """

    m, s = divmod(t, 60)
    h, m = divmod(m, 60)
    if h:
        return "{0:2.0f}hr {1:2.0f}min {2:4.1f}s".format(h, m, s)
    if m:
        return "{0:2.0f}min {1:4.1f}s".format(m, s)
    return "{0:4.1f}s".format(s)


class progressbar(Generic[T]):
    """A simple progressbar for iterables.

    Displays a progress bar showing progress through an iterable.

    Parameters
    ----------
    iterable : iterable
        The object to iterate over.
    width : int, optional
        Width of the bar in characters.
    enabled : bool, optional
        Whether to log progress. Useful for turning off progress reports
        without changing your code. Default is True.
    file : file, optional
        Where to log progress. Default is ``sys.stdout``.

    Example
    -------
    >>> with progressbar(iterable) as itbl:  # doctest: +SKIP
    ...     for i in itbl:
    ...         do_stuff(i)
    [########################################] | 100% Completed | 5.2 s
    """

    def __init__(
        self,
        iterable: Iterable[T],
        width: int = 40,
        enabled: bool = True,
        file: Optional[TextIO] = None,
    ) -> None:
        self._iterable = iterable
        self._ndone = 0
        self._ntotal = len(iterable)
        self._width = width
        self._enabled = enabled
        self._file = sys.stdout if file is None else file
        self._start_time: float
        self._running: bool
        self._timer: threading.Thread

    def __enter__(self) -> "progressbar[T]":
        if self._enabled:
            self._start_time = default_timer()
            # Start background thread
            self._running = True
            self._timer = threading.Thread(target=self._timer_func)
            self._timer.daemon = True
            self._timer.start()
        return self

    def __exit__(self, type, value, traceback) -> None:
        if self._enabled:
            self._running = False
            self._timer.join()
            self._update_bar()
            self._file.write("\n")
            self._file.flush()

    def __iter__(self) -> Iterator[T]:
        for i in self._iterable:
            self._ndone += 1
            yield i

    def _timer_func(self) -> None:
        while self._running:
            self._update_bar()
            time.sleep(0.1)

    def _update_bar(self) -> None:
        elapsed = default_timer() - self._start_time
        frac = (self._ndone / self._ntotal) if self._ntotal else 1
        bar = "#" * int(self._width * frac)
        percent = int(100 * frac)
        elapsed_str = format_time(elapsed)
        msg = "\r[{0:<{1}}] | {2}% Completed | {3}".format(
            bar,
            self._width,
            percent,
            elapsed_str,
        )
        try:
            self._file.write(msg)
            self._file.flush()
        except ValueError:
            # Writing to closed file handles raises ValueError; mirror the
            # behaviour of the original implementation by silently ignoring it.
            pass
