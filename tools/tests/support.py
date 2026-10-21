"""Mark optional evidence checks separately from contributor checks."""
import unittest


def evidence(available, reason):
    """Select only for --fixtures, failing that suite if unavailable."""
    def decorate(value):
        value.__wellfed_evidence__ = True
        return unittest.skipUnless(available, reason)(value)
    return decorate
