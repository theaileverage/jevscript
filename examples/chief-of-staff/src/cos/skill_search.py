"""The host's text filter for Skill selection: scout terms, then Okapi BM25.

A cheaper `llm` scout writes three lines (``kind``, ``terms``, ``ideal_skill``)
that expand the request, in the query2doc sense rather than HyDE: nothing is
embedded. The host parses that shape, folding the terms to lowercase, and on
any other shape falls back to the request's own words. Ranking is lexical and deterministic, so the result is a pure function
of the catalog, the scout text and the request; the ``fleet.skill_search``
call that returns it is recorded, which is what makes replay exact.
"""

from __future__ import annotations

import math
import re
from collections import Counter
from dataclasses import dataclass
from typing import Any

K1 = 1.2
B = 0.75
MAX_TERM_CHARS = 40
MAX_KIND_CHARS = 80
MAX_IDEAL_CHARS = 300
LOOKALIKES = 3

STOP = frozenset(
    "a an and any are as at be by can do does for from has have how i in into is it its of on or our should so "
    "some that the their them then there these this those to up use used using was we were what when where which "
    "while who why will with within without you your".split()
)


def stem(word: str) -> str:
    """Fold common English endings so "tests", "testing" and "test" meet, and "writer" meets "write"."""
    if len(word) > 4 and word.endswith("es") and word[-3] in "sxz" or word.endswith(("ches", "shes")):
        word = word[:-2]
    elif len(word) > 3 and word.endswith("s") and not word.endswith("ss"):
        word = word[:-1]
    for suffix in ("ing", "ed", "er"):
        if len(word) > len(suffix) + 3 and word.endswith(suffix):
            word = word[: -len(suffix)]
            break
    return word[:-1] if len(word) > 4 and word.endswith("e") else word


def tokens(text: str) -> list[str]:
    return [stem(word) for word in re.findall(r"[a-z0-9]+", text.lower()) if word not in STOP and len(word) > 1]


@dataclass(frozen=True)
class Scout:
    kind: str
    terms: tuple[str, ...]
    ideal_skill: str

    def text(self) -> str:
        return " ".join([self.kind, *self.terms, self.ideal_skill])


def parse_scout(text: str, max_terms: int) -> Scout:
    """Accept exactly the three-line schema or raise ``ValueError`` naming why."""
    lines = [line.strip() for line in text.splitlines()]
    if len(lines) != 3:
        raise ValueError(f"expected 3 lines, got {len(lines)}")
    fields: dict[str, str] = {}
    for line, key in zip(lines, ("kind", "terms", "ideal_skill")):
        prefix = f"{key}:"
        if not line.startswith(prefix):
            raise ValueError(f"expected a `{prefix}` line")
        fields[key] = line[len(prefix):].strip()
    terms = tuple(term.strip().lower() for term in fields["terms"].split(",") if term.strip())
    if not terms or len(terms) > max_terms:
        raise ValueError(f"expected 1 to {max_terms} terms, got {len(terms)}")
    for term in terms:
        if len(term) > MAX_TERM_CHARS:
            raise ValueError(f"term {term[:MAX_TERM_CHARS]!r}... exceeds {MAX_TERM_CHARS} characters")
    if not fields["kind"] or len(fields["kind"]) > MAX_KIND_CHARS:
        raise ValueError(f"kind must be 1 to {MAX_KIND_CHARS} characters")
    if not fields["ideal_skill"] or len(fields["ideal_skill"]) > MAX_IDEAL_CHARS:
        raise ValueError(f"ideal_skill must be 1 to {MAX_IDEAL_CHARS} characters")
    return Scout(fields["kind"], terms, fields["ideal_skill"])


def document(entry: dict[str, Any]) -> list[str]:
    return tokens(" ".join([entry["id"].replace("-", " ").replace("_", " "), entry["description"], *entry.get("keywords", [])]))


class Index:
    """Okapi BM25 (standard k1 and b) over a fixed list of catalog entries."""

    def __init__(self, entries: list[dict[str, Any]]) -> None:
        self.ids = [entry["id"] for entry in entries]
        docs = [document(entry) for entry in entries]
        self.tfs = dict(zip(self.ids, (Counter(doc) for doc in docs)))
        self.lengths = dict(zip(self.ids, (len(doc) for doc in docs)))
        self.average = sum(self.lengths.values()) / len(docs) if docs and sum(self.lengths.values()) else 1.0
        self.df = Counter(token for doc in docs for token in set(doc))

    def ranked(self, text: str) -> list[str]:
        """Ids with a positive score, best first, ties by id; each distinct query token counts once."""
        count, scores = len(self.ids), []
        weights = {token: math.log(1 + (count - self.df[token] + 0.5) / (self.df[token] + 0.5)) for token in set(tokens(text)) if self.df[token]}
        for skill_id in self.ids:
            tf, length = self.tfs[skill_id], self.lengths[skill_id]
            score = sum(idf * tf[token] * (K1 + 1) / (tf[token] + K1 * (1 - B + B * length / self.average)) for token, idf in weights.items() if tf[token])
            if score > 0:
                scores.append((-score, skill_id))
        return [skill_id for _, skill_id in sorted(scores)]

    def evidence(self, skill_id: str, query: set[str]) -> tuple[tuple[str, int], ...]:
        """What the query sees of one Skill: its matched tokens and their counts."""
        tf = self.tfs[skill_id]
        return tuple(sorted((token, tf[token]) for token in query if tf[token]))


def rank(catalog: list[dict[str, Any]], always: list[str], query: str, terms: list[str], k: int) -> list[str]:
    """Always-included ids in full, then up to ``k`` with BM25 and lexical diversification.

    Skills the query matches with identical evidence (say, the same procedure
    written for twenty languages the task never names) cannot be told apart
    by it. The first ``LOOKALIKES`` of such a group keep their places; the rest
    wait until every other match has had one. They are deferred, never
    dropped, and distinct Skills that merely share evidence with a few others
    stay together. Half of the open places go to the best overall matches;
    the other half go, in the scout's order, to the best match of each term,
    so one facet of the task is not crowded out. Leftover places go back to
    the overall ranking.
    """
    index = Index([entry for entry in catalog if entry["id"] not in set(always)])
    words = set(tokens(query))
    groups: Counter[tuple[tuple[str, int], ...]] = Counter()
    early, deferred = [], []
    for skill_id in index.ranked(query):
        evidence = index.evidence(skill_id, words)
        (early if groups[evidence] < LOOKALIKES else deferred).append(skill_id)
        groups[evidence] += 1
    overall = early + deferred
    room = max(0, k - len(always))
    chosen = overall[: room - room // 2]
    placed = Counter(index.evidence(skill_id, words) for skill_id in chosen)
    for term in terms:
        if len(chosen) >= room:
            break
        best = next((i for i in index.ranked(term) if i not in chosen and placed[index.evidence(i, words)] < LOOKALIKES), None)
        if best is not None:
            chosen.append(best)
            placed[index.evidence(best, words)] += 1
    chosen += [skill_id for skill_id in overall if skill_id not in chosen][: room - len(chosen)]
    return list(always) + chosen
