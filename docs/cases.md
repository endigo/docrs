# Cross-language case format (schema_version 1)

Frozen behavioral cases live in `tests/cases/*.json` and run with

```bash
cargo run -p docrs-compat -- cases            # Rust + pinned oracle
cargo run -p docrs-compat -- cases --no-oracle # Rust only
```

Python mock/proxy tests are never translated; only their public, observable
outcome is. Each case runs on a **fresh** copy of the fixture in both
implementations.

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
  `normalized`), `view`, `nth`, `all` (use `replace_all`), `again` (re-use
  the same span afterwards; reports the outcome), `acknowledge` (acknowledge
  protection first).
* `expect` — checked against the Rust result: `outcome` (`ok` | `error`),
  `category` (stable codes, `RefusalKind` in `crates/docrs/src/error.rs`),
  `changed_parts` (exact patch-save budget), `reopen_contains` /
  `reopen_absent` (searched in the reopened output — contract checks 1–3).
  For `error`, the runner also asserts that a pre-existing output file is
  byte-identical (check 5).
* Oracle comparison — outcome/category must agree; for successes the
  replacement report must be identical and every changed part semantically
  equal between the Rust and Python outputs, with identical reopened
  outlines.
* `deviation` — documents an intentional difference; the oracle result is
  then recorded without failing. `note` is free text.

Offsets are Unicode scalar positions (Python string indices). Check 4
(independent loader) runs only under `loader-smoke --require-libreoffice`.

Large generated suites complement the frozen cases: `kernel-parity` (M1),
`m2-parity` (outlines and searches) and `m3-parity` (generated replacement
cases over every fixture).
