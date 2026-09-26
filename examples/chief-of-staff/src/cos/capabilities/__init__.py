"""Capability bindings: what the entry program's `needs` are bound to.

- ``fleet`` (``tool``): every effect on the fleet, with idempotency keys;
- ``me`` (``person``): notifications, and the questions the runtime pauses on;
- ``writer`` (``llm``): the built-in template writer, or an external one;
- ``crew`` (``agent``): always an external adapter process speaking the JSONL
  adapter protocol, wrapped so its effects carry the same idempotency keys.
"""
