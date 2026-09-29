"""A 1,000-Skill catalog and labelled dispatches for Skill selection (S6).

Forty hand-written Skills cover common software work. The other 960 are
near misses built from the same activities for languages none of the
labelled tasks use, so they compete for the same words ("dependency",
"ci", "changelog") and the filter has to tell them apart.

Each labelled dispatch names the Skills a full-catalog judgment should
select. The labels are hand-written ground truth, not Jev answers.
"""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any

HAND: dict[str, str] = {
    "rust-deps": "Use when bumping Rust crate versions in Cargo.toml, refreshing Cargo.lock and fixing the compile errors an upgrade causes.",
    "rust-testing": "Use when cargo test fails: reproduce the failing Rust test, read the panic, fix the code or the test and rerun the suite.",
    "rust-unsafe-review": "Use when reviewing unsafe Rust blocks for soundness, aliasing and undefined behaviour.",
    "ci-repair": "Use when a CI pipeline such as GitHub Actions is red: read the failing job log, reproduce it locally and fix the build.",
    "github-actions-setup": "Use when creating a new GitHub Actions workflow file from scratch for a repository that has none.",
    "release-notes": "Use when writing release notes for a new version: summarise user-facing changes, breaking changes and upgrade steps.",
    "changelog-writer": "Use when adding entries to CHANGELOG.md in Keep a Changelog format, grouped as Added, Changed and Fixed.",
    "docs-writing": "Use when writing or restructuring documentation pages, guides and how-to articles in Markdown.",
    "readme-polish": "Use when tightening a project README: the pitch, install steps, a quick example and badges.",
    "python-testing": "Use when pytest tests fail or need writing: fixtures, parametrize, reproducing failures and fixing them.",
    "python-packaging": "Use when building or publishing a Python package with pyproject.toml, wheels and PyPI uploads.",
    "figma-export": "Use when exporting icons, images or design tokens from Figma files into an application's assets.",
    "react-components": "Use when building or refactoring React components, props, hooks and state in a web frontend.",
    "css-layout": "Use when fixing CSS layout problems with flexbox or grid, spacing and responsive breakpoints.",
    "sql-migrations": "Use when writing database schema migrations for Postgres or MySQL: add tables, columns or indexes safely.",
    "query-tuning": "Use when a SQL query is slow: read the EXPLAIN plan, add indexes and rewrite joins.",
    "k8s-debug": "Use when a Kubernetes deployment fails or pods crash-loop: inspect events, logs, probes and resource limits.",
    "helm-charts": "Use when authoring or upgrading Helm charts and their values files.",
    "docker-images": "Use when writing Dockerfiles, shrinking container images and fixing image build failures.",
    "terraform": "Use when writing Terraform modules and plans for cloud infrastructure such as AWS S3 buckets, IAM and networking.",
    "security-review": "Use when auditing code for security bugs: injection, authentication and authorization flaws, secrets and unsafe input handling.",
    "dependency-audit": "Use when checking dependencies for known vulnerabilities and license problems with audit tools.",
    "web-perf": "Use when a web page loads slowly: profile Core Web Vitals, bundle size, render-blocking requests and caching.",
    "backend-profiling": "Use when a backend service is slow: CPU and memory profiling, flame graphs and hot paths.",
    "i18n": "Use when translating a user interface into other languages: extract strings, locale files and plural rules.",
    "accessibility": "Use when making user interfaces accessible: screen reader labels, ARIA roles, keyboard navigation and contrast.",
    "refactoring": "Use when restructuring code into smaller modules or functions without changing its behaviour, guarded by tests.",
    "api-design": "Use when designing a REST or HTTP API: resources, status codes, pagination and versioning.",
    "graphql-schema": "Use when changing a GraphQL schema, resolvers and their types.",
    "oauth-login": "Use when adding sign-in with OAuth or OpenID Connect providers to an application.",
    "payments-stripe": "Use when integrating Stripe payments, checkout sessions and webhooks.",
    "email-templates": "Use when designing transactional email templates and sending them through a provider.",
    "data-pipeline": "Use when building batch data pipelines that extract, transform and load records on a schedule.",
    "ml-eval": "Use when evaluating a machine learning model against a labelled dataset and reporting metrics.",
    "mobile-release": "Use when shipping an iOS or Android app build to the App Store or Play Store.",
    "git-history": "Use when cleaning git history: interactive rebase, squashing commits and resolving merge conflicts.",
    "incident-postmortem": "Use when writing a blameless postmortem for a production incident with a timeline and follow-ups.",
    "logging-observability": "Use when adding structured logging, metrics and traces to a service.",
    "feature-flags": "Use when gating a feature behind a runtime flag and rolling it out gradually.",
    "code-review": "Use when reviewing a pull request for correctness, readability and missing tests.",
}

LANGUAGES = ["go", "java", "kotlin", "swift", "ruby", "php", "elixir", "scala", "haskell", "dart",
             "lua", "perl", "clojure", "ocaml", "zig", "nim", "crystal", "fortran", "cobol", "erlang"]
ACTIVITIES = ["dependency upgrades", "failing unit tests", "ci pipeline fixes", "release packaging", "documentation updates",
              "lint configuration", "code formatting", "benchmark profiling", "api client generation", "database migrations",
              "logging setup", "error handling review", "security scanning", "build caching", "container images",
              "localization files", "accessibility checks", "feature flags", "code review checklists", "onboarding guides",
              "changelog writing", "license audits", "monorepo tooling", "debugging sessions"]


def distractors() -> dict[str, str]:
    out: dict[str, str] = {}
    for language in LANGUAGES:
        for activity in ACTIVITIES:
            slug = f"{language}-{activity.replace(' ', '-')}"
            out[slug] = f"Use when handling {activity} in {language} projects, following {language} community conventions."
            out[f"{slug}-guide"] = f"A {language} guide to {activity}: tools, commands and common mistakes for {language} codebases."
    return out


def catalog_texts() -> dict[str, str]:
    return {**HAND, **distractors()}


#: A dispatch with five known-relevant Skills, and the three lines its scout writes.
LARGE_TASK = "Upgrade tokio to 1.40 in the Rust runtime crate, fix the failing cargo tests and the broken GitHub Actions CI, then write release notes and a changelog entry"
LARGE_RELEVANT = ["rust-deps", "rust-testing", "ci-repair", "release-notes", "changelog-writer"]
LARGE_SCOUT = """kind: rust dependency upgrade and release
terms: tokio, cargo, cargo.toml, cargo.lock, rust crate, version bump, upgrade, failing tests, cargo test, github actions, ci failure, build, release notes, changelog, changelog.md
ideal_skill: Use when upgrading a Rust crate dependency, repairing the tests and CI it breaks, and documenting the release."""

#: Labelled dispatches for the recall comparison: request and relevant ids.
DISPATCHES: list[dict[str, Any]] = [
    {"request": "Bump tokio to 1.40 in jevscript-runtime and fix whatever breaks in CI", "relevant": ["rust-deps", "ci-repair"]},
    {"request": "Add a docs page explaining how to install the CLI", "relevant": ["docs-writing"]},
    {"request": "The pytest suite fails on Python 3.13; fix the failing tests", "relevant": ["python-testing"]},
    {"request": "Write release notes for version 0.2 and update the changelog", "relevant": ["release-notes", "changelog-writer"]},
    {"request": "Export the new icons from Figma into the web app", "relevant": ["figma-export"]},
    {"request": "Add a Postgres migration that adds an index on users.email", "relevant": ["sql-migrations"]},
    {"request": "Our Kubernetes deployment keeps crash-looping after the last rollout", "relevant": ["k8s-debug"]},
    {"request": "Audit the API handlers for injection and auth bugs", "relevant": ["security-review"]},
    {"request": "The dashboard page is slow to load; profile it and speed it up", "relevant": ["web-perf"]},
    {"request": "Translate the settings screen into Spanish and German", "relevant": ["i18n"]},
    {"request": "Make the signup form usable with a screen reader", "relevant": ["accessibility"]},
    {"request": "Add a Terraform module for the new S3 bucket", "relevant": ["terraform"]},
    {"request": "Split the payment service into smaller modules without changing what it does", "relevant": ["refactoring"]},
    # Held out: written after the ranking was revised against the dispatches above, and not tuned on.
    {"request": "Our GitHub Actions build is red after the Node upgrade; get CI green again", "relevant": ["ci-repair"], "held_out": True},
    {"request": "Add Sign in with Google to the web app", "relevant": ["oauth-login"], "held_out": True},
    {"request": "Write a blameless postmortem for last night's outage", "relevant": ["incident-postmortem"], "held_out": True},
    {"request": "Stripe webhooks are failing to verify signatures in checkout", "relevant": ["payments-stripe"], "held_out": True},
    {"request": "Squash the last five commits and resolve the merge conflict with main", "relevant": ["git-history"], "held_out": True},
    {"request": "Add a Helm chart for the worker service", "relevant": ["helm-charts"], "held_out": True},
    {"request": "The orders query takes 9 seconds; make it fast", "relevant": ["query-tuning"], "held_out": True},
    {"request": "Publish the Python package to PyPI", "relevant": ["python-packaging"], "held_out": True},
    # Paraphrased: the request avoids the words of the Skill it needs, which is where scout terms should matter.
    {"request": "On phones the pricing table spills past the edge of the page", "relevant": ["css-layout"], "held_out": True, "paraphrase": True},
    {"request": "Customers are billed twice when they buy a plan", "relevant": ["payments-stripe"], "held_out": True, "paraphrase": True},
    {"request": "Nobody can tell what the service is doing in production; give us visibility", "relevant": ["logging-observability"], "held_out": True, "paraphrase": True},
    {"request": "Let only 5% of users try the new editor first", "relevant": ["feature-flags"], "held_out": True, "paraphrase": True},
    {"request": "Tidy up the messy commits on this branch before we merge", "relevant": ["git-history"], "held_out": True, "paraphrase": True},
    {"request": "Our container weighs 2 GB; make it lighter", "relevant": ["docker-images"], "held_out": True, "paraphrase": True},
    {"request": "Find out whether any npm package we ship has a published CVE", "relevant": ["dependency-audit"], "held_out": True, "paraphrase": True},
    {"request": "The API server pegs a core at 100% under load; find the hot spot", "relevant": ["backend-profiling"], "held_out": True, "paraphrase": True},
]


def write_skill(root: Path, skill_id: str, description: str, **extra: Any) -> dict[str, Any]:
    source = root / skill_id / "SKILL.md"
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_text(f"---\nname: {skill_id}\ndescription: {description}\n---\n\n# {skill_id}\n\n{description}\n")
    return {"id": skill_id, "path": str(source), "sha256": hashlib.sha256(source.read_bytes()).hexdigest(), "dependencies": [], **extra}


def write_catalog(root: Path, texts: dict[str, str]) -> list[dict[str, Any]]:
    return [write_skill(root, skill_id, description) for skill_id, description in texts.items()]
