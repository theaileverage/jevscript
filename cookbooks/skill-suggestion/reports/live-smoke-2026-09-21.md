# Live skill-suggestion smoke benchmark

Date: 2026-09-21

## Classification

This is a real local smoke benchmark over the four focused requests checked
into `data/sample_requests.json`. It is not the TypeSafe cookbook's 488-row
benchmark: that dataset is named but not downloadable from the documentation.
The 4-row result is useful execution evidence and is far too small for a model
quality conclusion.

This dated run used the former 182-skill roster. The current cookbook contains
a 157-skill licensed subset, so these live results do not measure its accuracy
or prove parity with its current prompt. No new provider run accompanied the
license cleanup.

## Configuration

| Item | Measured value |
| --- | --- |
| Roster | exact pinned Hermes snapshot: 182 skills, 33 categories |
| Jev profile | `jev-latest`, resolved by the runtime's dated profile bundle |
| Agent | OpenRouter `deepseek/deepseek-v4.1-flash` |
| Requests | 4: 2 covered, 2 uncovered |
| Agent arms | baseline, Jevscript suggestion, oracle |
| Live TypeSafe usage | 7 calls, 67,255 tokens, $0.00282471 |
| Live OpenRouter usage | 12 turns, 66,006 prompt tokens, 4,118 completion tokens, $0.00554366 |
| Sum of OpenRouter response latency | 258.210 seconds |

Costs are the values returned by each provider. OpenRouter reported the exact
requested model on all 12 rows. Raw provider response IDs, per-turn usage,
latency, Jev usage, suggestions, and loaded-skill lists are in
`live-smoke-2026-09-21.jsonl`; credentials and response prose are not stored.

## Scored result

| Arm | Wrong first load | Needless load | Covered | Uncovered |
| --- | ---: | ---: | ---: | ---: |
| Baseline | 0.0% | 0.0% | 2 | 2 |
| Jevscript suggestion | 0.0% | 0.0% | 2 | 2 |
| Oracle | 0.0% | 0.0% | 2 | 2 |

The metric counts only whether the first loaded skill matches gold on covered
rows and whether any skill is loaded on uncovered rows. It does not penalize
extra loads after a correct first load.

## Per-request behavior

| Request | Gold | Jevscript suggestion | Baseline loads | Jevscript-hint loads | Oracle loads |
| --- | --- | --- | --- | --- | --- |
| Apple Notes | `apple-notes` | `apple-notes` | `apple-notes` | `apple-notes` | `apple-notes` |
| PowerPoint authoring | `pptx-author` | `pptx-author` | `pptx-author`, `comps-analysis`, `dcf-model`, `lbo-model` | `pptx-author`, `powerpoint` | `pptx-author`, `excel-author` |
| Mastodon | none | none | none | none | none |
| Monad explanation | none | none | none | none | none |

The Jevscript policy therefore matched all four focused gold labels. The three
agent arms tie under the cookbook metric, but differ in extra PowerPoint skill
loads. The tie must not be extrapolated to the full benchmark.

## Published benchmark, kept separate

| Arm in TypeSafe's published run | Wrong first load | Needless load | Classification |
| --- | ---: | ---: | --- |
| Baseline | 16.8% | 9.8% | upstream published/static |
| TypeSafe Python suggestion | 7.3% | 4.0% | upstream published/static |
| Oracle | 2.5% | 1.2% | upstream published/static |

Those figures used 488 requests, `jev-1.12`, and
`claude-haiku-4-5-20251001`. They were not reproduced by this repository and
must not be combined with the 4-case DeepSeek smoke result.
