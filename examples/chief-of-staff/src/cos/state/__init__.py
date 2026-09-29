"""The state store: every durable record a home keeps, one module each.

``home`` (layout, atomic writes, the session lock, modes), ``registry``
(projects, delivery posture, ministers, dispatch profiles), ``backlog``,
``inbox`` (steering inbox and worker status logs), ``ledger``, ``memory``,
``workers`` and ``decisions``.
"""
