# docrs-cli

The `docrs` command: find, replace, review, compare and compose Word (`.docx`) files without disturbing what you didn't touch, and convert Markdown or editor HTML to Word styled by a template. Built on the [docrs](https://crates.io/crates/docrs) library.

```bash
cargo install docrs-cli --locked
```

```bash
docrs diagnose input.docx
docrs find input.docx "payment terms" --normalized
docrs replace input.docx --find 'payment terms' --with 'payment schedule' --out output.docx
docrs revisions input.docx
docrs resolve input.docx --accept --out output.docx
docrs compare original.docx revised.docx --author Reviewer --out redline.docx
docrs append base.docx addition.docx --out combined.docx
docrs from-markdown notes.md --reference template.docx --toc --out notes.docx
docrs from-markdown page.html --reference template.docx --out page.docx   # editor HTML
docrs markdown-styles template.docx
docrs markdown-inspect template.docx          # what a conversion takes from the template
docrs layout input.docx                       # estimated page of each paragraph
docrs roundtrip input.docx --out copy.docx    # byte-identical when unchanged
```

`docrs --help` lists every command and `docrs <command> --help` its options.

**Output and exit codes:**

- `--json` prints machine-readable reports (`schema_version: 1`). Errors are printed as JSON on stdout too, `{"schema_version": 1, "error": {"code", "reason", "message", "refusal", "limit"}}`, including argument errors (`--json` anywhere on the command line). `--help` and `--version` print text and exit 0.
- Exit codes:

  | Code | Meaning |
  | -- | -- |
  | 0 | success |
  | 2 | invalid argument |
  | 3 | refusal (ambiguous, not found, protected, unsupported structure, boundary, relationship policy, malformed package) |
  | 4 | resource limit |
  | 5 | I/O |
  | 70 | internal error (a bug; a panic is reported as code `internal`, reason `internal_panic`) |

- `find` exits 3 when nothing matches. `replace` exits 3 when it replaces nothing, `--all` included (no match: `target_not_found`; every match refused: the first refusal), and writes no output. Its report gives `replaced_count` and `refused_count`.
- Outputs are written atomically, and `--out` may be the input file. A refusal leaves every file unchanged.

## License

MIT. Upstream attribution is in `NOTICE.md`.
