---
title: Quarterly Platform Review
subtitle: Markdown to DOCX feature tour
author:
  - Ada Lovelace
  - Grace Hopper
date: 19 October 2026
abstract: |
  This document exercises the Markdown features the converter supports.
  Each element takes its look from the reference document's styles.
keywords: [markdown, docx, review]
toc: true
---

# Overview {#overview}

The first paragraph after a heading takes the *First Paragraph* style. Later
paragraphs take *Body Text*. Inline formatting covers **bold**, *italic*,
***both***, ~~strikethrough~~, `inline code`, H~2~O subscripts, x^2^
superscripts, <u>underline</u>, <mark>highlight</mark> and <kbd>raw</kbd>
HTML spans.\
That line ended with a hard break.

Links can be external ([example.com](https://example.com "Example site")),
automatic (<https://www.rust-lang.org>), e-mail (<team@example.com>) or
internal, like this jump to [the results section](#results) or to
[a custom anchor](#overview).

Footnotes work too.[^first] A second reference uses another note.[^long]

## Lists

Tight bullet list:

- Alpha
- Beta
  - Nested beta one
  - Nested beta two
    - Third level
- Gamma

Ordered list starting at 3:

3. Third
4. Fourth
5. Fifth

A fresh ordered list restarts at one:

1. One
2. Two
   1. Two point one
   2. Two point two

Task list:

- [x] Draft the review
- [ ] Circulate for comments

Loose list with continuation paragraphs:

1. First item, which has a second paragraph.

   This paragraph continues the first item and is indented with it.

2. Second item with code:

   ```sh
   cargo run -p docrs-cli -- from-markdown showcase.md --out out.docx
   ```

## Quotes and alerts

> Simplicity is prerequisite for reliability.
>
> > Nested quotes indent further.

> [!NOTE]
> Alerts become a labelled quote.

> [!WARNING]
> Check the reference document before publishing.

## Code

```rust
fn main() {
    let greeting = "hello";
	println!("{greeting}, world"); // a tab-indented line
}
```

    Indented code blocks work as well.

## Tables {#results}

Table: Quarterly results by region.

| Region        | Q1     | Q2     | Change |
|:--------------|-------:|-------:|:------:|
| North America | 1,200  | 1,420  | **+18%** |
| Europe        | 980    | 1,010  | +3%    |
| Asia Pacific  | 640    | 810    | *+27%* |

| Feature | Status |
|---------|--------|
| `tables` | done |
| [links](https://example.com) in cells | done |

## Images

A stand-alone image becomes a figure with its alt text as the caption:

![Revenue by product line](images/chart.png)

Wide images shrink to the page width:

![Banner photograph](images/banner.jpg "Banner")

An inline image ![tiny](images/chart.png) sits in the text line.

## Definitions

Reference document
: A .docx whose styles, page setup, headers and footers the converter uses.

Role
: The job a style does, such as *code block* or *table caption*.
: A role tries pandoc's style name first, then Word's.

## Breaks

Text above a horizontal rule.

---

Text below it, then a page break.

\newpage

# Appendix

LaTeX math becomes native Word equations: $E = mc^2$, $\alpha_i \le \sqrt{x^2 + y^2}$
and $\mathbb{R}^n$. Display math is centred:

$$
\int_0^1 x^2 \, dx = \frac{1}{3}, \qquad \sum_{i=1}^{n} i = \frac{n(n+1)}{2}
$$

$$
A = \begin{pmatrix} a & b \\ c & d \end{pmatrix}, \quad
|x| = \begin{cases} x & x \ge 0 \\ -x & x < 0 \end{cases}, \quad
\lim_{x \to 0} \frac{\sin x}{x} = 1
$$

[^first]: A short footnote with **formatting** and a [link](https://example.com).

[^long]: A footnote with two paragraphs.

    Its second paragraph is indented in the Markdown source.
