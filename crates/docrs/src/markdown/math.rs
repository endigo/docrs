//! LaTeX math → Office Math (OMML), for the subset Markdown documents use:
//! scripts, fractions, roots, large operators with limits, delimiters,
//! accents, matrices, cases and aligned equations, Greek letters, symbols
//! and font commands. Word lays the result out natively. Commands outside
//! the subset keep their source text and are reported.

use crate::Result;
use crate::wml::{M, W};
use crate::xml::{NodeId, XML_NS, XmlTree};

/// Character style of a math run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    /// Word's default (italic letters, upright digits and operators).
    Auto,
    /// Upright (function names, `\mathrm`).
    Upright,
    /// Ordinary text (`\text`).
    Text,
    Bold,
    Italic,
    DoubleStruck,
    Script,
    Fraktur,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Node {
    Run(String, Style),
    Sup(Vec<Node>, Vec<Node>),
    Sub(Vec<Node>, Vec<Node>),
    SubSup(Vec<Node>, Vec<Node>, Vec<Node>),
    Frac {
        num: Vec<Node>,
        den: Vec<Node>,
        bar: bool,
    },
    Sqrt {
        degree: Option<Vec<Node>>,
        body: Vec<Node>,
    },
    Nary {
        chr: char,
        sub: Option<Vec<Node>>,
        sup: Option<Vec<Node>>,
        body: Vec<Node>,
        integral: bool,
    },
    LimLow(Vec<Node>, Vec<Node>),
    /// A function name (with any scripts) applied to its argument.
    Func(Vec<Node>, Vec<Node>),
    Delim {
        open: String,
        close: String,
        body: Vec<Node>,
    },
    Accent(char, Vec<Node>),
    Bar(bool, Vec<Node>),
    Matrix(Vec<Vec<Vec<Node>>>),
    EqArray(Vec<Vec<Node>>),
}

/// Parse result: the nodes and the commands that were not understood.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub nodes: Vec<Node>,
    pub unknown: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Command(String),
    Char(char),
    Open,
    Close,
    Sup,
    Sub,
    Amp,
    NewRow,
    /// Whitespace: insignificant in math, kept by `\text{…}`.
    Space,
}

fn tokenize(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        index += 1;
        match c {
            '\\' => {
                let Some(&next) = chars.get(index) else {
                    break;
                };
                if next.is_ascii_alphabetic() {
                    let name: String = chars[index..]
                        .iter()
                        .take_while(|c| c.is_ascii_alphabetic())
                        .collect();
                    index += name.chars().count();
                    tokens.push(Token::Command(name));
                } else if next == '\\' {
                    index += 1;
                    tokens.push(Token::NewRow);
                } else {
                    index += 1;
                    tokens.push(Token::Command(next.to_string()));
                }
            }
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '^' => tokens.push(Token::Sup),
            '_' => tokens.push(Token::Sub),
            '&' => tokens.push(Token::Amp),
            c if c.is_whitespace() => {
                if tokens.last() != Some(&Token::Space) {
                    tokens.push(Token::Space);
                }
            }
            c => tokens.push(Token::Char(c)),
        }
    }
    tokens
}

fn symbol(name: &str) -> Option<&'static str> {
    Some(match name {
        "alpha" => "α",
        "beta" => "β",
        "gamma" => "γ",
        "delta" => "δ",
        "epsilon" => "ϵ",
        "varepsilon" => "ε",
        "zeta" => "ζ",
        "eta" => "η",
        "theta" => "θ",
        "vartheta" => "ϑ",
        "iota" => "ι",
        "kappa" => "κ",
        "lambda" => "λ",
        "mu" => "μ",
        "nu" => "ν",
        "xi" => "ξ",
        "pi" => "π",
        "varpi" => "ϖ",
        "rho" => "ρ",
        "varrho" => "ϱ",
        "sigma" => "σ",
        "varsigma" => "ς",
        "tau" => "τ",
        "upsilon" => "υ",
        "phi" => "ϕ",
        "varphi" => "φ",
        "chi" => "χ",
        "psi" => "ψ",
        "omega" => "ω",
        "Gamma" => "Γ",
        "Delta" => "Δ",
        "Theta" => "Θ",
        "Lambda" => "Λ",
        "Xi" => "Ξ",
        "Pi" => "Π",
        "Sigma" => "Σ",
        "Upsilon" => "Υ",
        "Phi" => "Φ",
        "Psi" => "Ψ",
        "Omega" => "Ω",
        "times" => "×",
        "cdot" => "⋅",
        "div" => "÷",
        "pm" => "±",
        "mp" => "∓",
        "le" | "leq" => "≤",
        "ge" | "geq" => "≥",
        "ne" | "neq" => "≠",
        "approx" => "≈",
        "equiv" => "≡",
        "sim" => "∼",
        "simeq" => "≃",
        "cong" => "≅",
        "propto" => "∝",
        "infty" => "∞",
        "partial" => "∂",
        "nabla" => "∇",
        "forall" => "∀",
        "exists" => "∃",
        "in" => "∈",
        "notin" => "∉",
        "ni" => "∋",
        "subset" => "⊂",
        "subseteq" => "⊆",
        "supset" => "⊃",
        "supseteq" => "⊇",
        "cup" => "∪",
        "cap" => "∩",
        "setminus" => "∖",
        "emptyset" | "varnothing" => "∅",
        "to" | "rightarrow" => "→",
        "leftarrow" | "gets" => "←",
        "Rightarrow" | "implies" => "⇒",
        "Leftarrow" => "⇐",
        "iff" | "Leftrightarrow" => "⇔",
        "leftrightarrow" => "↔",
        "mapsto" => "↦",
        "uparrow" => "↑",
        "downarrow" => "↓",
        "ldots" | "dots" => "…",
        "cdots" => "⋯",
        "vdots" => "⋮",
        "ddots" => "⋱",
        "prime" => "′",
        "circ" => "∘",
        "angle" => "∠",
        "degree" => "°",
        "neg" | "lnot" => "¬",
        "land" | "wedge" => "∧",
        "lor" | "vee" => "∨",
        "oplus" => "⊕",
        "otimes" => "⊗",
        "perp" => "⊥",
        "parallel" => "∥",
        "mid" => "∣",
        "ll" => "≪",
        "gg" => "≫",
        "langle" => "⟨",
        "rangle" => "⟩",
        "lfloor" => "⌊",
        "rfloor" => "⌋",
        "lceil" => "⌈",
        "rceil" => "⌉",
        "hbar" => "ℏ",
        "ell" => "ℓ",
        "Re" => "ℜ",
        "Im" => "ℑ",
        "aleph" => "ℵ",
        "star" => "⋆",
        "ast" => "∗",
        "bullet" => "∙",
        "therefore" => "∴",
        "because" => "∵",
        "{" | "lbrace" => "{",
        "}" | "rbrace" => "}",
        "|" | "Vert" => "‖",
        "vert" => "|",
        "%" => "%",
        "$" => "$",
        "#" => "#",
        "&" => "&",
        "_" => "_",
        "," => "\u{2009}",
        ":" | ";" | ">" => "\u{2005}",
        " " => " ",
        "quad" => "\u{2003}",
        "qquad" => "\u{2003}\u{2003}",
        "!" => "",
        _ => return None,
    })
}

const FUNCTIONS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh",
    "coth", "log", "ln", "lg", "exp", "det", "dim", "gcd", "deg", "arg", "ker", "Pr", "hom",
    "bmod",
];

/// Limit-style operators (`\lim_{x \to 0}` sets the limit below).
const LIMITS: &[&str] = &["lim", "limsup", "liminf", "max", "min", "sup", "inf"];

fn nary(name: &str) -> Option<(char, bool)> {
    Some(match name {
        "sum" => ('∑', false),
        "prod" => ('∏', false),
        "coprod" => ('∐', false),
        "int" => ('∫', true),
        "iint" => ('∬', true),
        "iiint" => ('∭', true),
        "oint" => ('∮', true),
        "bigcup" => ('⋃', false),
        "bigcap" => ('⋂', false),
        "bigoplus" => ('⨁', false),
        "bigotimes" => ('⨂', false),
        "bigvee" => ('⋁', false),
        "bigwedge" => ('⋀', false),
        _ => return None,
    })
}

fn accent(name: &str) -> Option<char> {
    Some(match name {
        "hat" | "widehat" => '\u{0302}',
        "bar" => '\u{0305}',
        "vec" => '\u{20D7}',
        "dot" => '\u{0307}',
        "ddot" => '\u{0308}',
        "tilde" | "widetilde" => '\u{0303}',
        "check" => '\u{030C}',
        "acute" => '\u{0301}',
        "grave" => '\u{0300}',
        "breve" => '\u{0306}',
        _ => return None,
    })
}

fn font_style(name: &str) -> Option<Style> {
    Some(match name {
        "text" | "textrm" | "mbox" | "textnormal" => Style::Text,
        "mathrm" | "operatorname" | "rm" => Style::Upright,
        "mathbf" | "boldsymbol" | "bm" | "textbf" => Style::Bold,
        "mathit" | "textit" => Style::Italic,
        "mathbb" => Style::DoubleStruck,
        "mathcal" | "mathscr" => Style::Script,
        "mathfrak" => Style::Fraktur,
        _ => return None,
    })
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    unknown: Vec<String>,
}

/// Where an expression stops.
#[derive(Clone, Copy, PartialEq)]
enum Stop {
    End,
    Close,
    Right,
    EndEnv,
}

impl Parser {
    fn skip_space(&mut self) {
        while self.tokens.get(self.index) == Some(&Token::Space) {
            self.index += 1;
        }
    }

    fn peek(&mut self) -> Option<&Token> {
        self.skip_space();
        self.tokens.get(self.index)
    }

    fn next(&mut self) -> Option<Token> {
        self.skip_space();
        let token = self.tokens.get(self.index).cloned();
        self.index += 1;
        token
    }

    /// Nodes up to `stop`; cells (`&`) and rows (`\\`) also stop it when
    /// `in_table`.
    fn expression(&mut self, stop: Stop, in_table: bool) -> Vec<Node> {
        let mut nodes = Vec::new();
        while let Some(token) = self.peek() {
            match token {
                Token::Close if stop == Stop::Close => {
                    self.index += 1;
                    return nodes;
                }
                Token::Close => {
                    // Unbalanced: ignore.
                    self.index += 1;
                }
                Token::Command(name) if name == "right" && stop == Stop::Right => return nodes,
                Token::Command(name) if name == "end" && stop == Stop::EndEnv => return nodes,
                Token::Amp | Token::NewRow if in_table => return nodes,
                Token::Amp | Token::NewRow => {
                    self.index += 1;
                    nodes.push(Node::Run(" ".into(), Style::Auto));
                }
                _ => {
                    let atom = self.atom();
                    let node = self.scripts(atom);
                    push_node(&mut nodes, node);
                }
            }
        }
        nodes
    }

    /// One `{group}` or single-token argument.
    fn argument(&mut self) -> Vec<Node> {
        match self.peek() {
            Some(Token::Open) => {
                self.index += 1;
                self.expression(Stop::Close, false)
            }
            Some(_) => {
                let atom = self.atom();
                atom.into_iter().collect()
            }
            None => Vec::new(),
        }
    }

    /// Raw text of a `{…}` argument (for `\text`, `\begin`).
    fn raw_argument(&mut self) -> String {
        if self.peek() != Some(&Token::Open) {
            return match self.next() {
                Some(Token::Char(c)) => c.to_string(),
                _ => String::new(),
            };
        }
        self.index += 1;
        let mut depth = 0;
        let mut text = String::new();
        while let Some(token) = self.tokens.get(self.index).cloned() {
            self.index += 1;
            match token {
                Token::Space => text.push(' '),
                Token::Open => {
                    depth += 1;
                    text.push('{');
                }
                Token::Close if depth == 0 => break,
                Token::Close => {
                    depth -= 1;
                    text.push('}');
                }
                Token::Char(c) => text.push(c),
                Token::Command(name) => match symbol(&name) {
                    Some(s) => text.push_str(s),
                    None => text.push_str(&name),
                },
                Token::Sup => text.push('^'),
                Token::Sub => text.push('_'),
                Token::Amp => text.push('&'),
                Token::NewRow => text.push(' '),
            }
        }
        text
    }

    fn optional_argument(&mut self) -> Option<Vec<Node>> {
        if self.peek() != Some(&Token::Char('[')) {
            return None;
        }
        self.index += 1;
        let mut nodes = Vec::new();
        while let Some(token) = self.peek() {
            if token == &Token::Char(']') {
                self.index += 1;
                break;
            }
            let atom = self.atom();
            let node = self.scripts(atom);
            push_node(&mut nodes, node);
        }
        Some(nodes)
    }

    /// A delimiter after `\left` / `\right`.
    fn delimiter(&mut self) -> String {
        match self.next() {
            Some(Token::Char('.')) => String::new(),
            Some(Token::Char(c)) => c.to_string(),
            Some(Token::Command(name)) => symbol(&name).unwrap_or("").to_string(),
            _ => String::new(),
        }
    }

    fn atom(&mut self) -> Option<Node> {
        let token = self.next()?;
        Some(match token {
            Token::Char(c) => Node::Run(c.to_string(), Style::Auto),
            Token::Open => {
                let group = self.expression(Stop::Close, false);
                return Some(group_node(group));
            }
            Token::Sup | Token::Sub => Node::Run(String::new(), Style::Auto),
            Token::Close | Token::Amp | Token::NewRow | Token::Space => return None,
            Token::Command(name) => self.command(&name),
        })
    }

    fn command(&mut self, name: &str) -> Node {
        if let Some(text) = symbol(name) {
            return Node::Run(text.to_string(), Style::Auto);
        }
        if FUNCTIONS.contains(&name) {
            // `\sin^2 x`: scripts belong to the name.
            let name_node = self.scripts(Some(Node::Run(name.to_string(), Style::Upright)));
            let starts_argument = matches!(
                self.peek(),
                Some(Token::Char(c)) if c.is_alphanumeric() || *c == '('
            ) || matches!(self.peek(), Some(Token::Open | Token::Command(_)));
            if !starts_argument {
                return name_node.expect("a function name");
            }
            let atom = self.atom();
            let argument: Vec<Node> = self.scripts(atom).into_iter().collect();
            return Node::Func(name_node.into_iter().collect(), argument);
        }
        if LIMITS.contains(&name) {
            let base = vec![Node::Run(name.to_string(), Style::Upright)];
            if self.peek() == Some(&Token::Sub) {
                self.index += 1;
                let lim = self.argument();
                return Node::LimLow(base, lim);
            }
            return group_node(base);
        }
        if let Some((chr, integral)) = nary(name) {
            let (mut sub, mut sup) = (None, None);
            while let Some(token) = self.peek() {
                match token {
                    Token::Sub if sub.is_none() => {
                        self.index += 1;
                        sub = Some(self.argument());
                    }
                    Token::Sup if sup.is_none() => {
                        self.index += 1;
                        sup = Some(self.argument());
                    }
                    Token::Command(c) if c == "limits" || c == "nolimits" => self.index += 1,
                    _ => break,
                }
            }
            let body = match self.atom() {
                Some(atom) => vec![self.scripts(Some(atom))]
                    .into_iter()
                    .flatten()
                    .collect(),
                None => Vec::new(),
            };
            return Node::Nary {
                chr,
                sub,
                sup,
                body,
                integral,
            };
        }
        if let Some(chr) = accent(name) {
            return Node::Accent(chr, self.argument());
        }
        if let Some(style) = font_style(name) {
            return match style {
                Style::Text => Node::Run(self.raw_argument(), Style::Text),
                style => {
                    let inner = self.argument();
                    restyle(inner, style)
                }
            };
        }
        match name {
            "frac" | "dfrac" | "tfrac" | "cfrac" => {
                let num = self.argument();
                let den = self.argument();
                Node::Frac {
                    num,
                    den,
                    bar: true,
                }
            }
            "binom" | "dbinom" | "tbinom" => {
                let num = self.argument();
                let den = self.argument();
                Node::Delim {
                    open: "(".into(),
                    close: ")".into(),
                    body: vec![Node::Frac {
                        num,
                        den,
                        bar: false,
                    }],
                }
            }
            "sqrt" => {
                let degree = self.optional_argument();
                let body = self.argument();
                Node::Sqrt { degree, body }
            }
            "overline" => Node::Bar(true, self.argument()),
            "underline" => Node::Bar(false, self.argument()),
            "left" => {
                let open = self.delimiter();
                let body = self.expression(Stop::Right, false);
                let close = if matches!(self.peek(), Some(Token::Command(n)) if n == "right") {
                    self.index += 1;
                    self.delimiter()
                } else {
                    String::new()
                };
                Node::Delim { open, close, body }
            }
            "big" | "Big" | "bigg" | "Bigg" | "bigl" | "bigr" | "Bigl" | "Bigr"
            | "displaystyle" | "textstyle" | "limits" | "nolimits" | "middle" => {
                Node::Run(String::new(), Style::Auto)
            }
            "begin" => self.environment(),
            other => {
                self.unknown.push(format!("\\{other}"));
                Node::Run(format!("\\{other}"), Style::Text)
            }
        }
    }

    /// `\begin{env} … \end{env}`.
    fn environment(&mut self) -> Node {
        let env = self.raw_argument();
        let mut rows: Vec<Vec<Vec<Node>>> = vec![vec![]];
        loop {
            let cell = self.expression(Stop::EndEnv, true);
            rows.last_mut().expect("a row").push(cell);
            match self.next() {
                Some(Token::Amp) => {}
                Some(Token::NewRow) => rows.push(Vec::new()),
                Some(Token::Command(name)) if name == "end" => {
                    self.raw_argument();
                    break;
                }
                _ => break,
            }
        }
        if rows.last().is_some_and(|row| row.iter().all(Vec::is_empty)) && rows.len() > 1 {
            rows.pop();
        }
        let matrix = |rows: Vec<Vec<Vec<Node>>>| Node::Matrix(rows);
        let delimited = |open: &str, close: &str, body: Node| Node::Delim {
            open: open.into(),
            close: close.into(),
            body: vec![body],
        };
        match env.as_str() {
            "matrix" | "smallmatrix" | "array" => matrix(rows),
            "pmatrix" => delimited("(", ")", matrix(rows)),
            "bmatrix" => delimited("[", "]", matrix(rows)),
            "Bmatrix" => delimited("{", "}", matrix(rows)),
            "vmatrix" => delimited("|", "|", matrix(rows)),
            "Vmatrix" => delimited("‖", "‖", matrix(rows)),
            "cases" => delimited("{", "", matrix(rows)),
            "aligned" | "align" | "align*" | "gather" | "gather*" | "split" | "eqnarray" => {
                Node::EqArray(rows.into_iter().map(|cells| cells.concat()).collect())
            }
            other => {
                self.unknown.push(format!("\\begin{{{other}}}"));
                matrix(rows)
            }
        }
    }

    /// Attach `^`/`_` scripts to `base`.
    fn scripts(&mut self, base: Option<Node>) -> Option<Node> {
        let mut sub = None;
        let mut sup = None;
        loop {
            match self.peek() {
                Some(Token::Sub) if sub.is_none() => {
                    self.index += 1;
                    sub = Some(self.argument());
                }
                Some(Token::Sup) if sup.is_none() => {
                    self.index += 1;
                    sup = Some(self.argument());
                }
                Some(Token::Char('\'')) if sup.is_none() => {
                    let mut primes = String::new();
                    while self.peek() == Some(&Token::Char('\'')) {
                        self.index += 1;
                        primes.push('′');
                    }
                    sup = Some(vec![Node::Run(primes, Style::Auto)]);
                }
                _ => break,
            }
        }
        let base: Vec<Node> = base.into_iter().collect();
        Some(match (sub, sup) {
            (None, None) => return base.into_iter().next(),
            (Some(sub), None) => Node::Sub(base, sub),
            (None, Some(sup)) => Node::Sup(base, sup),
            (Some(sub), Some(sup)) => Node::SubSup(base, sub, sup),
        })
    }
}

/// Append, merging adjacent runs of one style.
fn push_node(nodes: &mut Vec<Node>, node: Option<Node>) {
    let Some(node) = node else {
        return;
    };
    if let Node::Run(text, style) = &node {
        if text.is_empty() {
            return;
        }
        if let Some(Node::Run(last, last_style)) = nodes.last_mut()
            && last_style == style
            && *style != Style::Upright
        {
            last.push_str(text);
            return;
        }
    }
    nodes.push(node);
}

/// A group becomes one node when it is a single node, else a run-less
/// container (an invisible delimiter would add space, so a sole child or a
/// flattened sequence is used).
fn group_node(mut group: Vec<Node>) -> Node {
    if group.len() == 1 {
        return group.pop().expect("one node");
    }
    Node::Delim {
        open: String::new(),
        close: String::new(),
        body: group,
    }
}

fn restyle(nodes: Vec<Node>, style: Style) -> Node {
    let restyled: Vec<Node> = nodes
        .into_iter()
        .map(|node| match node {
            Node::Run(text, _) => Node::Run(text, style),
            other => other,
        })
        .collect();
    group_node(restyled)
}

/// Parse a LaTeX math source.
pub(crate) fn parse(source: &str) -> Parsed {
    let mut parser = Parser {
        tokens: tokenize(source),
        index: 0,
        unknown: Vec::new(),
    };
    let nodes = parser.expression(Stop::End, false);
    Parsed {
        nodes,
        unknown: parser.unknown,
    }
}

// ------------------------------------------------------------- writing

fn m_el(tree: &mut XmlTree, parent: NodeId, local: &str) -> Result<NodeId> {
    let node = tree.create_element(parent, M, local)?;
    crate::oxml::append(tree, parent, node)?;
    Ok(node)
}

fn m_val(tree: &mut XmlTree, parent: NodeId, local: &str, value: &str) -> Result<NodeId> {
    let node = m_el(tree, parent, local)?;
    tree.set_attr(node, Some(M), "val", value)?;
    Ok(node)
}

/// `m:oMath` (inline) for `nodes`, appended to `parent`.
pub(crate) fn write_math(tree: &mut XmlTree, parent: NodeId, nodes: &[Node]) -> Result<NodeId> {
    let math = m_el(tree, parent, "oMath")?;
    write_nodes(tree, math, nodes)?;
    Ok(math)
}

/// `m:oMathPara` (display) for `nodes`, appended to `parent`.
pub(crate) fn write_display(tree: &mut XmlTree, parent: NodeId, nodes: &[Node]) -> Result<NodeId> {
    let para = m_el(tree, parent, "oMathPara")?;
    write_math(tree, para, nodes)?;
    Ok(para)
}

fn write_nodes(tree: &mut XmlTree, parent: NodeId, nodes: &[Node]) -> Result<()> {
    for node in nodes {
        write_node(tree, parent, node)?;
    }
    Ok(())
}

fn container(tree: &mut XmlTree, parent: NodeId, local: &str, nodes: &[Node]) -> Result<()> {
    let element = m_el(tree, parent, local)?;
    write_nodes(tree, element, nodes)
}

fn write_run(tree: &mut XmlTree, parent: NodeId, text: &str, style: Style) -> Result<()> {
    let r = m_el(tree, parent, "r")?;
    let (scr, sty, normal) = match style {
        Style::Auto => (None, None, false),
        Style::Upright => (None, Some("p"), false),
        Style::Text => (None, Some("p"), true),
        Style::Bold => (None, Some("b"), false),
        Style::Italic => (None, Some("i"), false),
        Style::DoubleStruck => (Some("double-struck"), Some("p"), false),
        Style::Script => (Some("script"), Some("p"), false),
        Style::Fraktur => (Some("fraktur"), Some("p"), false),
    };
    if scr.is_some() || sty.is_some() || normal {
        let rpr = m_el(tree, r, "rPr")?;
        if normal {
            m_el(tree, rpr, "nor")?;
        }
        if let Some(scr) = scr {
            m_val(tree, rpr, "scr", scr)?;
        }
        if let Some(sty) = sty {
            m_val(tree, rpr, "sty", sty)?;
        }
    }
    // Word's math font for every run.
    let w_rpr = tree.create_element(r, W, "rPr")?;
    crate::oxml::append(tree, r, w_rpr)?;
    let fonts = tree.create_element(w_rpr, W, "rFonts")?;
    tree.set_attr(fonts, Some(W), "ascii", "Cambria Math")?;
    tree.set_attr(fonts, Some(W), "hAnsi", "Cambria Math")?;
    crate::oxml::append(tree, w_rpr, fonts)?;
    let t = m_el(tree, r, "t")?;
    if text.starts_with(' ') || text.ends_with(' ') {
        tree.set_attr(t, Some(XML_NS), "space", "preserve")?;
    }
    let content = tree.create_text(text);
    tree.insert(t, 0, content)
}

fn write_node(tree: &mut XmlTree, parent: NodeId, node: &Node) -> Result<()> {
    match node {
        Node::Run(text, style) => write_run(tree, parent, text, *style),
        Node::Sup(base, sup) => {
            let element = m_el(tree, parent, "sSup")?;
            container(tree, element, "e", base)?;
            container(tree, element, "sup", sup)
        }
        Node::Sub(base, sub) => {
            let element = m_el(tree, parent, "sSub")?;
            container(tree, element, "e", base)?;
            container(tree, element, "sub", sub)
        }
        Node::SubSup(base, sub, sup) => {
            let element = m_el(tree, parent, "sSubSup")?;
            container(tree, element, "e", base)?;
            container(tree, element, "sub", sub)?;
            container(tree, element, "sup", sup)
        }
        Node::Frac { num, den, bar } => {
            let element = m_el(tree, parent, "f")?;
            if !bar {
                let pr = m_el(tree, element, "fPr")?;
                m_val(tree, pr, "type", "noBar")?;
            }
            container(tree, element, "num", num)?;
            container(tree, element, "den", den)
        }
        Node::Sqrt { degree, body } => {
            let element = m_el(tree, parent, "rad")?;
            if degree.is_none() {
                let pr = m_el(tree, element, "radPr")?;
                m_val(tree, pr, "degHide", "1")?;
            }
            container(tree, element, "deg", degree.as_deref().unwrap_or(&[]))?;
            container(tree, element, "e", body)
        }
        Node::Nary {
            chr,
            sub,
            sup,
            body,
            integral,
        } => {
            let element = m_el(tree, parent, "nary")?;
            let pr = m_el(tree, element, "naryPr")?;
            m_val(tree, pr, "chr", &chr.to_string())?;
            m_val(
                tree,
                pr,
                "limLoc",
                if *integral { "subSup" } else { "undOvr" },
            )?;
            if sub.is_none() {
                m_val(tree, pr, "subHide", "1")?;
            }
            if sup.is_none() {
                m_val(tree, pr, "supHide", "1")?;
            }
            container(tree, element, "sub", sub.as_deref().unwrap_or(&[]))?;
            container(tree, element, "sup", sup.as_deref().unwrap_or(&[]))?;
            container(tree, element, "e", body)
        }
        Node::Func(name, argument) => {
            let element = m_el(tree, parent, "func")?;
            container(tree, element, "fName", name)?;
            container(tree, element, "e", argument)
        }
        Node::LimLow(base, lim) => {
            let element = m_el(tree, parent, "limLow")?;
            container(tree, element, "e", base)?;
            container(tree, element, "lim", lim)
        }
        Node::Delim { open, close, body } => {
            if open.is_empty() && close.is_empty() {
                // A plain group: its content inline.
                return write_nodes(tree, parent, body);
            }
            let element = m_el(tree, parent, "d")?;
            let pr = m_el(tree, element, "dPr")?;
            m_val(tree, pr, "begChr", open)?;
            m_val(tree, pr, "endChr", close)?;
            container(tree, element, "e", body)
        }
        Node::Accent(chr, body) => {
            let element = m_el(tree, parent, "acc")?;
            let pr = m_el(tree, element, "accPr")?;
            m_val(tree, pr, "chr", &chr.to_string())?;
            container(tree, element, "e", body)
        }
        Node::Bar(top, body) => {
            let element = m_el(tree, parent, "bar")?;
            let pr = m_el(tree, element, "barPr")?;
            m_val(tree, pr, "pos", if *top { "top" } else { "bot" })?;
            container(tree, element, "e", body)
        }
        Node::Matrix(rows) => {
            let element = m_el(tree, parent, "m")?;
            for row in rows {
                let mr = m_el(tree, element, "mr")?;
                for cell in row {
                    container(tree, mr, "e", cell)?;
                }
            }
            Ok(())
        }
        Node::EqArray(rows) => {
            let element = m_el(tree, parent, "eqArr")?;
            for row in rows {
                container(tree, element, "e", row)?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Node {
        Node::Run(text.into(), Style::Auto)
    }

    #[test]
    fn scripts_fractions_and_roots() {
        let parsed = parse(r"x^2 + \frac{a}{b} - \sqrt[3]{y_1}");
        assert!(parsed.unknown.is_empty());
        assert_eq!(parsed.nodes[0], Node::Sup(vec![run("x")], vec![run("2")]));
        assert!(matches!(&parsed.nodes[2], Node::Frac { bar: true, .. }));
        assert!(matches!(
            &parsed.nodes[4],
            Node::Sqrt {
                degree: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn large_operators_take_limits_and_a_body() {
        let parsed = parse(r"\sum_{i=1}^{n} i^2 = \int_0^1 f(x)\,dx");
        let Node::Nary {
            chr,
            sub,
            sup,
            body,
            integral,
        } = &parsed.nodes[0]
        else {
            panic!("nary: {:?}", parsed.nodes)
        };
        assert_eq!(*chr, '∑');
        assert!(sub.is_some() && sup.is_some() && !integral);
        assert_eq!(body[0], Node::Sup(vec![run("i")], vec![run("2")]));
        assert!(
            parsed
                .nodes
                .iter()
                .any(|n| matches!(n, Node::Nary { integral: true, .. }))
        );
    }

    #[test]
    fn delimiters_environments_and_unknowns() {
        let parsed = parse(
            r"\left( \frac{1}{2} \right] + \begin{pmatrix} a & b \\ c & d \end{pmatrix} + \foo",
        );
        assert!(
            matches!(&parsed.nodes[0], Node::Delim { open, close, .. } if open == "(" && close == "]")
        );
        let Node::Delim { body, .. } = &parsed.nodes[2] else {
            panic!("pmatrix: {:?}", parsed.nodes)
        };
        let Node::Matrix(rows) = &body[0] else {
            panic!("matrix")
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].len(), 2);
        assert_eq!(parsed.unknown, vec!["\\foo"]);
    }

    #[test]
    fn symbols_functions_and_text() {
        let parsed = parse(r"\alpha \le \sin\theta \text{ if } \mathbb{R}");
        assert_eq!(parsed.nodes[0], run("α≤"));
        assert_eq!(
            parsed.nodes[1],
            Node::Func(
                vec![Node::Run("sin".into(), Style::Upright)],
                vec![run("θ")]
            )
        );
        assert!(
            parsed
                .nodes
                .contains(&Node::Run(" if ".into(), Style::Text))
        );
        assert!(
            parsed
                .nodes
                .contains(&Node::Run("R".into(), Style::DoubleStruck))
        );
    }
}
