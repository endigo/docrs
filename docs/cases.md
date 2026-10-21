# Cross-language case format (schema_version 1)

Frozen behavioral cases live in `tests/cases/*.json` and run with

```bash
cargo run -p docrs-compat -- cases            # Rust + pinned oracle
cargo run -p docrs-compat -- cases --no-oracle # Rust only
```

Python mock/proxy tests are never translated; only their public, observable
outcome is. Each case runs on a **fresh** copy of the fixture in both
implementations. An excerpt of `tests/cases/m3-plan-example.json`:

```json
{
  "schema_version": 1,
  "id": "m3.plan-example.fragmented-runs",
  "upstream": ["tests/paper/test_search_replace.py (split-run exact replacement)"],
  "fixture": "generated/feature-isolated/fragmented-runs.docx",
  "cases": [
    {"needle": "payment terms", "with": "payment schedule",
     "expect": {"outcome": "ok", "changed_parts": ["word/document.xml"],
                "reopen_contains": ["payment schedule"],
                "reopen_absent": ["payment terms"]}},
    {"needle": "hr", "with": "hour",
     "expect": {"outcome": "error", "category": "ambiguous_target"}}
  ]
}
```

Fields:

* `fixture` — path under `tests/fixtures/upstream/`, or `synthetic:<name>`
  for a generated document from `crates/docrs-compat/src/hostile.rs`.
* Case operation — `needle`, `with`, optional `match` (`exact` |
  `normalized`), `view`, `nth`, `all` (use `replace_all`), `tracked`,
  `author`, `date`, `preserve_revision` (replacement options), `again` (a
  second replacement text applied with the same span; reports the outcome),
  `acknowledge` (acknowledge protection first).
* `expect` — checked against the Rust result: `outcome` (`ok` | `error`),
  `category` (stable `Error::code()` values: the `RefusalKind` codes plus
  `invalid_argument`, `resource_limit`, …; `crates/docrs/src/error.rs`),
  `changed_parts` (exact patch-save budget), `reopen_contains` /
  `reopen_absent` (searched in the reopened output — contract checks 1–3).
  For `error`, the runner also asserts that a pre-existing output file is
  byte-identical (check 5).
* Oracle comparison — outcome/category must agree; for successes the
  replacement report must be identical and every changed part semantically
  equal between the Rust and Python outputs, with identical reopened
  outlines.
* `deviation` — documents an intentional difference; an oracle
  disagreement then does not fail the run. `note` is free text.

Offsets are Unicode scalar positions (Python string indices). Check 4
(independent loader) runs under `loader-smoke` (and `check`) when a working
LibreOffice is found and is skipped otherwise; `--require-libreoffice` makes
it mandatory, as the CI `loader` job does.

Large generated suites complement the frozen cases: `kernel-parity` (M1),
`m2-parity` (outlines and searches), `m3-parity` (generated replacement
cases over every fixture) and `m6-parity` (difflib, text diffs, pending
changes and compare). Multi-step operation cases in `tests/ops/*.json` run
with `ops`.
