# Bundled profile provenance

Checked 2026-09-21 against TypeSafe's [models documentation](https://docs.typesafe.ai/models)
and [HTTP API reference](https://docs.typesafe.ai/api) (spec section 10.6).

The documented concrete model is `jev-1.13.0`; `jev-latest` resolves to that
version in this bundle. Provider documentation specifies 64,000 total request
tokens, 32,000 tokens for state plus the longest question, 255 Choice criteria,
and $0.042 per million input tokens with free output tokens.

The provider does not publish a questions-per-request maximum. The bundle's
64-question cap is conservative runtime policy, not an observed service limit.
The `chars4` tokenizer estimates size; it does not reproduce the provider's
tokenizer. Override either through a profile file when a more suitable policy
or estimator is available. All runtime checks and prices read the selected
profile, including these policy values.
