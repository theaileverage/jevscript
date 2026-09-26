# Jevscript specification boundaries

Status: documentation architecture for the 0.1 draft. This file is explanatory. Until the split described here is performed atomically, `spec/jevscript-language-specification.md` remains the sole normative authority.

## Boundary rule

The language specification keeps everything a program author needs to predict what a program means and what observable result it can produce. A separate contract may own transport fields, serialization, provider mappings, or a particular SDK spelling, but moving text must not make an existing guarantee optional.

The following therefore stay visible and normative from the language specification:

- syntax, names, types, values, expressions, statements, units, and module rules;
- judgment answer spaces, state construction, batching, selection, and sampling semantics;
- capability authority, effect ordering, task and machine control flow, budgets, gates, and verification semantics;
- the meanings of pause kinds and the determinism and replay guarantees that affect program behavior;
- compile and runtime error codes, the grammar, executable examples, and language conformance requirements.

## Proposed normative documents

| Document | Owns | Language specification links to |
| --- | --- | --- |
| Language specification | Source language and every observable semantic rule listed above. | The contracts below for exact boundary representations. |
| Runtime and recording contract | Run lifecycle, exact recording events and identities, serialization, redaction, retry/abort behavior, replay envelope, and IR support policy. | Observable ordering, pause meanings, and replay guarantees remain summarized normatively in the language specification. |
| Host and capability contract | Bindings and authority, exact pause payloads, host operations, JSON-RPC, and run ownership. | The distinction between input data and capabilities, plus the effect of every host-visible operation. |
| Judgment-provider and profile contract | Canonical question and answer protocol, response validation, confidence semantics, provider capabilities, token accounting, and profile schema. | State construction, answer-space meaning, selection, batching, and sampling remain language semantics. |
| TypeSafe adapter mapping | TypeSafe-specific HTTP mapping, credentials, error translation, and bundled-profile provenance. | Identified as the reference provider adapter, not the universal judgment definition. |
| IR and host wire schemas | Versioned JSON representations, declaration DTOs, tagged values, and negotiation or rejection rules. | Stable schema versions and compatibility guarantees. |
| SDK and CLI guides | Installation, language-specific method names, iterator conventions, command examples, and adapter setup. | Never redefine a pause, error, or replay rule. |
| Error reference | Plain-language explanations, likely causes, fixes, and documentation anchors for stable error codes. | Section 12 remains the normative list and payload contract. |
| Conformance and decision registry | Layered requirements, evidence links, and historical decisions. | Conformance remains release-gating; decisions link back to their normative home. |

## Material that can move without changing the language

The reference implementation's crate layout, a particular SDK's spelling, CLI usage prose, TypeSafe endpoint and credential details, generated schemas, profile-source notes, and extended troubleshooting do not belong in the core language narrative. They may move once their normative owner and version are explicit.

The current section 16 decision log does not move by itself. Repository policy and implementation doc comments point to this specification by section number, so the authority index, section migration map, cross-references, and ownership instructions must change in the same commit as any physical split.

## Migration sequence

1. Finish the current semantic review and decide the provider-neutral judgment and compatibility boundaries.
2. Create a versioned authority index naming every normative document and their compatible versions.
3. Assign each existing rule exactly one normative home and produce an old-to-new section map.
4. Move the text in one documentation-only change, preserving all 0.1 behavior and stable anchors where practical.
5. Update `AGENTS.md`, code comments, tests, schemas, and conformance evidence to cite the new homes.
6. Only after the split is verified, make semantic changes in their owning documents.

This sequence avoids mixing editorial relocation with language, runtime, or wire changes and prevents linked material from silently becoming non-normative.
