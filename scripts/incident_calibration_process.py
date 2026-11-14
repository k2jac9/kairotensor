"""Linux-only lifetime containment for owned offline fixture processes."""
import ctypes
import os
import signal
import sys


def arm_parent_death(expected_parent):
    if not sys.platform.startswith('linux') or type(expected_parent) is not int or expected_parent <= 0:
        raise RuntimeError('Linux parent-death containment required')
    if os.getppid() != expected_parent:
        raise RuntimeError('parent changed before containment')
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(1, signal.SIGKILL, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'parent-death containment unavailable')
    if os.getppid() != expected_parent:
        raise RuntimeError('parent changed during containment')
