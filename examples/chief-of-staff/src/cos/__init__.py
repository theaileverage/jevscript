"""A chief of staff (the CoS) with control logic in Jevscript and a thin Python host.

The decisions live in ``jev/`` (intake, routing, supervision, escalation,
learning, playbooks); this package stores state, binds capabilities, detects
wakes and runs each wake as one bounded, recorded Jevscript episode.
"""

__version__ = "0.1.0"
