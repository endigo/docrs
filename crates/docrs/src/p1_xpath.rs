//! A self-contained XPath 1.0 evaluator over [`XmlTree`], used only to
//! resolve `w:dataBinding/@w:xpath` in custom XML stores (upstream evaluates
//! them with lxml).
//!
//! Supported: the full expression grammar (`or`, `and`, equality,
//! relational, additive, multiplicative, unary minus, union, filter and
//! location paths with abbreviations), the axes child, descendant,
//! descendant-or-self, self, parent, ancestor, ancestor-or-self,
//! following-sibling, preceding-sibling and attribute, node tests by name,
//! `prefix:*`, `*`, `node()`, `text()`, `comment()` and
//! `processing-instruction()`, and the core function library except `id`,
//! `lang` and the namespace axis. Constructs outside that subset are
//! reported as [`XError::Unsupported`]; syntax errors as
//! [`XError::Syntax`]; undefined prefixes and unknown functions as
//! [`XError::Eval`] (lxml raises `XPathEvalError` for those).

use std::collections::HashMap;

use crate::xml::chars::{is_name_char, is_name_start_char};
use crate::xml::{NodeData, NodeId, XmlTree};

/// One XPath node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum XNode {
    /// The document node.
    Root,
    /// An element, text, comment or processing-instruction node.
    Node(NodeId),
    /// Attribute `index` of an element.
    Attr(NodeId, usize),
}

/// An XPath value.
#[derive(Debug, Clone)]
pub(crate) enum XValue {
    Nodes(Vec<XNode>),
    Bool(bool),
    Num(f64),
    Str(String),
}

impl XValue {
    /// Python type name lxml returns for a non-node-set result.
    pub(crate) fn python_type(&self) -> &'static str {
        match self {
            XValue::Nodes(_) => "list",
            XValue::Bool(_) => "bool",
            XValue::Num(_) => "float",
            XValue::Str(_) => "str",
        }
    }
}

/// Evaluation failure.
#[derive(Debug, Clone)]
pub(crate) enum XError {
    Syntax(String),
    Eval(String),
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    LParen,
    RParen,
    LBracket,
    RBracket,
    Dot,
    DotDot,
    At,
    Comma,
    ColonColon,
    Slash,
    SlashSlash,
    Pipe,
    Plus,
    Minus,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Star,
    /// Operator name (`and`, `or`, `mod`, `div`).
    OpName(String),
    Literal(String),
    Number(f64),
    /// `NCName`, `prefix:local` or `prefix:*`.
    Name(Option<String>, String),
}

fn is_nc_start(c: char) -> bool {
    c != ':' && is_name_start_char(c)
}

fn is_nc_char(c: char) -> bool {
    c != ':' && is_name_char(c)
}

fn lex(expr: &str) -> Result<Vec<Tok>, XError> {
    let chars: Vec<char> = expr.chars().collect();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0;
    let syntax = |msg: &str| XError::Syntax(msg.to_string());
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, ' ' | '\t' | '\r' | '\n') {
            i += 1;
            continue;
        }
        // An operator is expected when a preceding token exists that is not
        // one of `@ :: ( [ ,` or an operator (XPath 1.0 §3.7).
        let operator_expected = match out.last() {
            None => false,
            Some(
                Tok::At
                | Tok::ColonColon
                | Tok::LParen
                | Tok::LBracket
                | Tok::Comma
                | Tok::Slash
                | Tok::SlashSlash
                | Tok::Pipe
                | Tok::Plus
                | Tok::Minus
                | Tok::Eq
                | Tok::Ne
                | Tok::Lt
                | Tok::Le
                | Tok::Gt
                | Tok::Ge
                | Tok::OpName(_),
            ) => false,
            Some(_) => true,
        };
        let next = chars.get(i + 1).copied();
        let tok = match c {
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            '@' => Tok::At,
            ',' => Tok::Comma,
            '|' => Tok::Pipe,
            '+' => Tok::Plus,
            '-' => Tok::Minus,
            '=' => Tok::Eq,
            '*' if operator_expected => Tok::OpName("*".into()),
            '*' => Tok::Star,
            '!' if next == Some('=') => {
                i += 1;
                Tok::Ne
            }
            '<' if next == Some('=') => {
                i += 1;
                Tok::Le
            }
            '<' => Tok::Lt,
            '>' if next == Some('=') => {
                i += 1;
                Tok::Ge
            }
            '>' => Tok::Gt,
            ':' if next == Some(':') => {
                i += 1;
                Tok::ColonColon
            }
            '/' if next == Some('/') => {
                i += 1;
                Tok::SlashSlash
            }
            '/' => Tok::Slash,
            '.' if next == Some('.') => {
                i += 1;
                Tok::DotDot
            }
            '.' if !next.is_some_and(|n| n.is_ascii_digit()) => Tok::Dot,
            '"' | '\'' => {
                let quote = c;
                let start = i + 1;
                let mut end = start;
                while end < chars.len() && chars[end] != quote {
                    end += 1;
                }
                if end >= chars.len() {
                    return Err(syntax("unterminated string literal"));
                }
                let value: String = chars[start..end].iter().collect();
                i = end + 1;
                out.push(Tok::Literal(value));
                continue;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if i < chars.len() && chars[i] == '.' {
                    i += 1;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                let value = text.parse::<f64>().map_err(|_| syntax("invalid number"))?;
                out.push(Tok::Number(value));
                continue;
            }
            c if is_nc_start(c) => {
                let start = i;
                while i < chars.len() && is_nc_char(chars[i]) {
                    i += 1;
                }
                let first: String = chars[start..i].iter().collect();
                if operator_expected {
                    if matches!(first.as_str(), "and" | "or" | "mod" | "div") {
                        out.push(Tok::OpName(first));
                        continue;
                    }
                    return Err(syntax("expected an operator"));
                }
                if i + 1 < chars.len() && chars[i] == ':' && chars[i + 1] != ':' {
                    if chars[i + 1] == '*' {
                        i += 2;
                        out.push(Tok::Name(Some(first), "*".into()));
                        continue;
                    }
                    if is_nc_start(chars[i + 1]) {
                        let local_start = i + 1;
                        i = local_start;
                        while i < chars.len() && is_nc_char(chars[i]) {
                            i += 1;
                        }
                        let local: String = chars[local_start..i].iter().collect();
                        out.push(Tok::Name(Some(first), local));
                        continue;
                    }
                    return Err(syntax("invalid qualified name"));
                }
                out.push(Tok::Name(None, first));
                continue;
            }
            _ => return Err(syntax("unexpected character")),
        };
        out.push(tok);
        i += 1;
    }
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Child,
    Descendant,
    DescendantOrSelf,
    Current,
    Parent,
    Ancestor,
    AncestorOrSelf,
    FollowingSibling,
    PrecedingSibling,
    Attribute,
}

#[derive(Debug, Clone)]
enum NodeTest {
    /// `(prefix, local)`; local `*` matches any name.
    Name(Option<String>, String),
    AnyNode,
    Text,
    Comment,
    Pi(Option<String>),
}

#[derive(Debug, Clone)]
struct Step {
    axis: Axis,
    test: NodeTest,
    predicates: Vec<Expr>,
}

#[derive(Debug, Clone)]
enum PathStart {
    Root,
    Context,
    Filter(Box<Expr>),
}

#[derive(Debug, Clone)]
enum Expr {
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Neg(Box<Expr>),
    Literal(String),
    Number(f64),
    Call(Option<String>, String, Vec<Expr>),
    Filter(Box<Expr>, Vec<Expr>),
    Path(PathStart, Vec<Step>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Union,
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_at(&self, offset: usize) -> Option<&Tok> {
        self.toks.get(self.pos + offset)
    }

    fn bump(&mut self) -> Option<Tok> {
        let tok = self.toks.get(self.pos).cloned();
        self.pos += 1;
        tok
    }

    fn expect(&mut self, tok: Tok) -> Result<(), XError> {
        if self.peek() == Some(&tok) {
            self.pos += 1;
            Ok(())
        } else {
            Err(XError::Syntax(format!("expected {tok:?}")))
        }
    }

    fn op_name(&self, name: &str) -> bool {
        matches!(self.peek(), Some(Tok::OpName(n)) if n == name)
    }

    fn expr(&mut self) -> Result<Expr, XError> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr, XError> {
        let mut left = self.and_expr()?;
        while self.op_name("or") {
            self.pos += 1;
            let right = self.and_expr()?;
            left = Expr::Binary(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Expr, XError> {
        let mut left = self.equality()?;
        while self.op_name("and") {
            self.pos += 1;
            let right = self.equality()?;
            left = Expr::Binary(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn equality(&mut self) -> Result<Expr, XError> {
        let mut left = self.relational()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Eq) => BinOp::Eq,
                Some(Tok::Ne) => BinOp::Ne,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.relational()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn relational(&mut self) -> Result<Expr, XError> {
        let mut left = self.additive()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Lt) => BinOp::Lt,
                Some(Tok::Le) => BinOp::Le,
                Some(Tok::Gt) => BinOp::Gt,
                Some(Tok::Ge) => BinOp::Ge,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.additive()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn additive(&mut self) -> Result<Expr, XError> {
        let mut left = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.multiplicative()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn multiplicative(&mut self) -> Result<Expr, XError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::OpName(n)) if n == "*" => BinOp::Mul,
                Some(Tok::OpName(n)) if n == "div" => BinOp::Div,
                Some(Tok::OpName(n)) if n == "mod" => BinOp::Mod,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn unary(&mut self) -> Result<Expr, XError> {
        if self.peek() == Some(&Tok::Minus) {
            self.pos += 1;
            return Ok(Expr::Neg(Box::new(self.unary()?)));
        }
        self.union()
    }

    fn union(&mut self) -> Result<Expr, XError> {
        let mut left = self.path_expr()?;
        while self.peek() == Some(&Tok::Pipe) {
            self.pos += 1;
            let right = self.path_expr()?;
            left = Expr::Binary(BinOp::Union, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn starts_primary(&self) -> bool {
        match self.peek() {
            Some(Tok::LParen | Tok::Literal(_) | Tok::Number(_)) => true,
            Some(Tok::Name(_, local)) if local != "*" => {
                self.peek_at(1) == Some(&Tok::LParen)
                    && !matches!(
                        local.as_str(),
                        "node" | "text" | "comment" | "processing-instruction"
                    )
            }
            _ => false,
        }
    }

    fn path_expr(&mut self) -> Result<Expr, XError> {
        if self.starts_primary() {
            let primary = self.primary()?;
            let mut predicates = Vec::new();
            while self.peek() == Some(&Tok::LBracket) {
                predicates.push(self.predicate()?);
            }
            let filtered = if predicates.is_empty() {
                primary
            } else {
                Expr::Filter(Box::new(primary), predicates)
            };
            match self.peek() {
                Some(Tok::Slash) => {
                    self.pos += 1;
                    let steps = self.relative_steps()?;
                    Ok(Expr::Path(PathStart::Filter(Box::new(filtered)), steps))
                }
                Some(Tok::SlashSlash) => {
                    self.pos += 1;
                    let mut steps = vec![descendant_or_self_step()];
                    steps.extend(self.relative_steps()?);
                    Ok(Expr::Path(PathStart::Filter(Box::new(filtered)), steps))
                }
                _ => Ok(filtered),
            }
        } else {
            self.location_path()
        }
    }

    fn primary(&mut self) -> Result<Expr, XError> {
        match self.bump() {
            Some(Tok::LParen) => {
                let inner = self.expr()?;
                self.expect(Tok::RParen)?;
                Ok(inner)
            }
            Some(Tok::Literal(value)) => Ok(Expr::Literal(value)),
            Some(Tok::Number(value)) => Ok(Expr::Number(value)),
            Some(Tok::Name(prefix, local)) => {
                self.expect(Tok::LParen)?;
                let mut args = Vec::new();
                if self.peek() != Some(&Tok::RParen) {
                    args.push(self.expr()?);
                    while self.peek() == Some(&Tok::Comma) {
                        self.pos += 1;
                        args.push(self.expr()?);
                    }
                }
                self.expect(Tok::RParen)?;
                Ok(Expr::Call(prefix, local, args))
            }
            _ => Err(XError::Syntax("expected a primary expression".into())),
        }
    }

    fn predicate(&mut self) -> Result<Expr, XError> {
        self.expect(Tok::LBracket)?;
        let inner = self.expr()?;
        self.expect(Tok::RBracket)?;
        Ok(inner)
    }

    fn location_path(&mut self) -> Result<Expr, XError> {
        match self.peek() {
            Some(Tok::Slash) => {
                self.pos += 1;
                if self.starts_step() {
                    Ok(Expr::Path(PathStart::Root, self.relative_steps()?))
                } else {
                    Ok(Expr::Path(PathStart::Root, Vec::new()))
                }
            }
            Some(Tok::SlashSlash) => {
                self.pos += 1;
                let mut steps = vec![descendant_or_self_step()];
                steps.extend(self.relative_steps()?);
                Ok(Expr::Path(PathStart::Root, steps))
            }
            _ => Ok(Expr::Path(PathStart::Context, self.relative_steps()?)),
        }
    }

    fn starts_step(&self) -> bool {
        matches!(
            self.peek(),
            Some(Tok::Dot | Tok::DotDot | Tok::At | Tok::Star | Tok::Name(_, _))
        )
    }

    fn relative_steps(&mut self) -> Result<Vec<Step>, XError> {
        let mut steps = vec![self.step()?];
        loop {
            match self.peek() {
                Some(Tok::Slash) => {
                    self.pos += 1;
                    steps.push(self.step()?);
                }
                Some(Tok::SlashSlash) => {
                    self.pos += 1;
                    steps.push(descendant_or_self_step());
                    steps.push(self.step()?);
                }
                _ => return Ok(steps),
            }
        }
    }

    fn step(&mut self) -> Result<Step, XError> {
        match self.peek() {
            Some(Tok::Dot) => {
                self.pos += 1;
                return Ok(Step {
                    axis: Axis::Current,
                    test: NodeTest::AnyNode,
                    predicates: Vec::new(),
                });
            }
            Some(Tok::DotDot) => {
                self.pos += 1;
                return Ok(Step {
                    axis: Axis::Parent,
                    test: NodeTest::AnyNode,
                    predicates: Vec::new(),
                });
            }
            _ => {}
        }
        let axis = if self.peek() == Some(&Tok::At) {
            self.pos += 1;
            Axis::Attribute
        } else if let (Some(Tok::Name(None, name)), Some(Tok::ColonColon)) =
            (self.peek(), self.peek_at(1))
        {
            let axis = match name.as_str() {
                "child" => Axis::Child,
                "descendant" => Axis::Descendant,
                "descendant-or-self" => Axis::DescendantOrSelf,
                "self" => Axis::Current,
                "parent" => Axis::Parent,
                "ancestor" => Axis::Ancestor,
                "ancestor-or-self" => Axis::AncestorOrSelf,
                "following-sibling" => Axis::FollowingSibling,
                "preceding-sibling" => Axis::PrecedingSibling,
                "attribute" => Axis::Attribute,
                "following" | "preceding" | "namespace" => {
                    return Err(XError::Unsupported(format!("the {name} axis")));
                }
                _ => return Err(XError::Syntax(format!("unknown axis {name}"))),
            };
            self.pos += 2;
            axis
        } else {
            Axis::Child
        };
        let test = match self.bump() {
            Some(Tok::Star) => NodeTest::Name(None, "*".into()),
            Some(Tok::Name(prefix, local)) => {
                if prefix.is_none() && self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let test = match local.as_str() {
                        "node" => NodeTest::AnyNode,
                        "text" => NodeTest::Text,
                        "comment" => NodeTest::Comment,
                        "processing-instruction" => {
                            if let Some(Tok::Literal(target)) = self.peek().cloned() {
                                self.pos += 1;
                                NodeTest::Pi(Some(target))
                            } else {
                                NodeTest::Pi(None)
                            }
                        }
                        _ => return Err(XError::Syntax("function call in a step".into())),
                    };
                    self.expect(Tok::RParen)?;
                    test
                } else {
                    NodeTest::Name(prefix, local)
                }
            }
            _ => return Err(XError::Syntax("expected a node test".into())),
        };
        let mut predicates = Vec::new();
        while self.peek() == Some(&Tok::LBracket) {
            predicates.push(self.predicate()?);
        }
        Ok(Step {
            axis,
            test,
            predicates,
        })
    }
}

fn descendant_or_self_step() -> Step {
    Step {
        axis: Axis::DescendantOrSelf,
        test: NodeTest::AnyNode,
        predicates: Vec::new(),
    }
}

struct Ctx<'a> {
    tree: &'a XmlTree,
    namespaces: &'a [(String, String)],
    order: HashMap<XNode, usize>,
}

#[derive(Clone, Copy)]
struct Focus {
    node: XNode,
    position: usize,
    size: usize,
}

fn number_to_string(value: f64) -> String {
    if value.is_nan() {
        "NaN".into()
    } else if value.is_infinite() {
        if value > 0.0 { "Infinity" } else { "-Infinity" }.into()
    } else if value == value.trunc() && value.abs() < 1e18 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn string_to_number(text: &str) -> f64 {
    let trimmed = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'));
    let body = trimmed.strip_prefix('-').unwrap_or(trimmed);
    let valid = !body.is_empty()
        && body.chars().all(|c| c.is_ascii_digit() || c == '.')
        && body.chars().filter(|c| *c == '.').count() <= 1
        && body != ".";
    if valid {
        trimmed.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

impl<'a> Ctx<'a> {
    fn new(tree: &'a XmlTree, namespaces: &'a [(String, String)]) -> Self {
        let mut order = HashMap::new();
        order.insert(XNode::Root, 0);
        let mut index = 1;
        for node in tree.descendants(tree.root()) {
            order.insert(XNode::Node(node), index);
            index += 1;
            if let Some(element) = tree.element(node) {
                for attr in 0..element.attrs.len() {
                    order.insert(XNode::Attr(node, attr), index);
                    index += 1;
                }
            }
        }
        Ctx {
            tree,
            namespaces,
            order,
        }
    }

    fn sort(&self, nodes: &mut Vec<XNode>) {
        nodes.sort_by_key(|node| self.order.get(node).copied().unwrap_or(usize::MAX));
        nodes.dedup();
    }

    fn resolve_prefix(&self, prefix: &str) -> Result<&str, XError> {
        self.namespaces
            .iter()
            .find(|(p, _)| p == prefix)
            .map(|(_, uri)| uri.as_str())
            .ok_or_else(|| XError::Eval(format!("Undefined namespace prefix {prefix}")))
    }

    fn string_value(&self, node: XNode) -> String {
        match node {
            XNode::Root => self.tree.text_content(self.tree.root()),
            XNode::Attr(element, index) => self
                .tree
                .element(element)
                .and_then(|e| e.attrs.get(index))
                .map(|a| a.value.clone())
                .unwrap_or_default(),
            XNode::Node(id) => match self.tree.data(id) {
                NodeData::Element(_) => self.tree.text_content(id),
                NodeData::Text(text) => text.clone(),
                NodeData::Comment(text) => text.clone(),
                NodeData::Pi { data, .. } => data.clone(),
            },
        }
    }

    fn children(&self, node: XNode) -> Vec<XNode> {
        match node {
            XNode::Root => vec![XNode::Node(self.tree.root())],
            XNode::Node(id) => self
                .tree
                .children(id)
                .iter()
                .map(|c| XNode::Node(*c))
                .collect(),
            XNode::Attr(..) => Vec::new(),
        }
    }

    fn parent(&self, node: XNode) -> Option<XNode> {
        match node {
            XNode::Root => None,
            XNode::Attr(element, _) => Some(XNode::Node(element)),
            XNode::Node(id) => match self.tree.parent(id) {
                Some(parent) => Some(XNode::Node(parent)),
                None if id == self.tree.root() => Some(XNode::Root),
                None => None,
            },
        }
    }

    fn descendants(&self, node: XNode, out: &mut Vec<XNode>) {
        for child in self.children(node) {
            out.push(child);
            self.descendants(child, out);
        }
    }

    fn axis_nodes(&self, node: XNode, axis: Axis) -> Vec<XNode> {
        match axis {
            Axis::Child => self.children(node),
            Axis::Descendant => {
                let mut out = Vec::new();
                self.descendants(node, &mut out);
                out
            }
            Axis::DescendantOrSelf => {
                let mut out = vec![node];
                self.descendants(node, &mut out);
                out
            }
            Axis::Current => vec![node],
            Axis::Parent => self.parent(node).into_iter().collect(),
            Axis::Ancestor | Axis::AncestorOrSelf => {
                let mut out = Vec::new();
                if axis == Axis::AncestorOrSelf {
                    out.push(node);
                }
                let mut current = self.parent(node);
                while let Some(parent) = current {
                    out.push(parent);
                    current = self.parent(parent);
                }
                out
            }
            Axis::FollowingSibling | Axis::PrecedingSibling => {
                let (XNode::Node(id), Some(XNode::Node(parent))) = (node, self.parent(node)) else {
                    return Vec::new();
                };
                let siblings = self.tree.children(parent);
                let index = siblings.iter().position(|s| *s == id).unwrap_or(0);
                if axis == Axis::FollowingSibling {
                    siblings[index + 1..]
                        .iter()
                        .map(|s| XNode::Node(*s))
                        .collect()
                } else {
                    siblings[..index]
                        .iter()
                        .rev()
                        .map(|s| XNode::Node(*s))
                        .collect()
                }
            }
            Axis::Attribute => match node {
                XNode::Node(id) => self
                    .tree
                    .element(id)
                    .map(|e| (0..e.attrs.len()).map(|i| XNode::Attr(id, i)).collect())
                    .unwrap_or_default(),
                _ => Vec::new(),
            },
        }
    }

    fn matches(&self, node: XNode, axis: Axis, test: &NodeTest) -> Result<bool, XError> {
        let data = match node {
            XNode::Node(id) => Some(self.tree.data(id)),
            _ => None,
        };
        Ok(match test {
            NodeTest::AnyNode => true,
            NodeTest::Text => matches!(data, Some(NodeData::Text(_))),
            NodeTest::Comment => matches!(data, Some(NodeData::Comment(_))),
            NodeTest::Pi(target) => match data {
                Some(NodeData::Pi { target: t, .. }) => target.as_ref().is_none_or(|w| w == t),
                _ => false,
            },
            NodeTest::Name(prefix, local) => {
                let wanted_ns = match prefix {
                    Some(prefix) => Some(self.resolve_prefix(prefix)?),
                    None => None,
                };
                let (ns, name): (Option<&str>, &str) = match (axis, node) {
                    (Axis::Attribute, XNode::Attr(element, index)) => {
                        let Some(attr) =
                            self.tree.element(element).and_then(|e| e.attrs.get(index))
                        else {
                            return Ok(false);
                        };
                        (attr.ns.map(|n| self.tree.uri(n)), &attr.name.local)
                    }
                    (Axis::Attribute, _) => return Ok(false),
                    (_, XNode::Node(id)) if matches!(data, Some(NodeData::Element(_))) => {
                        (self.tree.ns_uri(id), self.tree.local_name(id).unwrap_or(""))
                    }
                    _ => return Ok(false),
                };
                if local == "*" {
                    prefix.is_none() || ns == wanted_ns
                } else {
                    name == local && ns == wanted_ns
                }
            }
        })
    }

    fn step(&self, input: &[XNode], step: &Step) -> Result<Vec<XNode>, XError> {
        let mut out = Vec::new();
        for node in input {
            let mut candidates = Vec::new();
            for candidate in self.axis_nodes(*node, step.axis) {
                if self.matches(candidate, step.axis, &step.test)? {
                    candidates.push(candidate);
                }
            }
            for predicate in &step.predicates {
                candidates = self.filter(candidates, predicate)?;
            }
            out.extend(candidates);
        }
        self.sort(&mut out);
        Ok(out)
    }

    /// Keep the nodes (in proximity order) for which `predicate` holds.
    fn filter(&self, nodes: Vec<XNode>, predicate: &Expr) -> Result<Vec<XNode>, XError> {
        let size = nodes.len();
        let mut kept = Vec::new();
        for (index, node) in nodes.into_iter().enumerate() {
            let focus = Focus {
                node,
                position: index + 1,
                size,
            };
            let keep = match self.eval(predicate, focus)? {
                XValue::Num(n) => n == (index + 1) as f64,
                other => self.boolean(&other),
            };
            if keep {
                kept.push(node);
            }
        }
        Ok(kept)
    }

    fn boolean(&self, value: &XValue) -> bool {
        match value {
            XValue::Nodes(nodes) => !nodes.is_empty(),
            XValue::Bool(b) => *b,
            XValue::Num(n) => *n != 0.0 && !n.is_nan(),
            XValue::Str(s) => !s.is_empty(),
        }
    }

    fn string(&self, value: &XValue) -> String {
        match value {
            XValue::Nodes(nodes) => nodes
                .first()
                .map(|n| self.string_value(*n))
                .unwrap_or_default(),
            XValue::Bool(b) => if *b { "true" } else { "false" }.into(),
            XValue::Num(n) => number_to_string(*n),
            XValue::Str(s) => s.clone(),
        }
    }

    fn number(&self, value: &XValue) -> f64 {
        match value {
            XValue::Num(n) => *n,
            XValue::Bool(b) => {
                if *b {
                    1.0
                } else {
                    0.0
                }
            }
            other => string_to_number(&self.string(other)),
        }
    }

    fn compare(&self, op: BinOp, left: &XValue, right: &XValue) -> bool {
        let cmp_num = |a: f64, b: f64| match op {
            BinOp::Eq => a == b,
            BinOp::Ne => a != b,
            BinOp::Lt => a < b,
            BinOp::Le => a <= b,
            BinOp::Gt => a > b,
            BinOp::Ge => a >= b,
            _ => false,
        };
        let equality = matches!(op, BinOp::Eq | BinOp::Ne);
        match (left, right) {
            (XValue::Nodes(a), XValue::Nodes(b)) => a.iter().any(|x| {
                let sx = self.string_value(*x);
                b.iter().any(|y| {
                    let sy = self.string_value(*y);
                    if equality {
                        (sx == sy) == (op == BinOp::Eq)
                    } else {
                        cmp_num(string_to_number(&sx), string_to_number(&sy))
                    }
                })
            }),
            (XValue::Nodes(nodes), other) | (other, XValue::Nodes(nodes)) => {
                let nodes_left = matches!(left, XValue::Nodes(_));
                match other {
                    XValue::Bool(b) => {
                        let nb = !nodes.is_empty();
                        let (l, r) = if nodes_left { (nb, *b) } else { (*b, nb) };
                        cmp_num(l as u8 as f64, r as u8 as f64)
                    }
                    XValue::Num(n) => nodes.iter().any(|node| {
                        let v = string_to_number(&self.string_value(*node));
                        if nodes_left {
                            cmp_num(v, *n)
                        } else {
                            cmp_num(*n, v)
                        }
                    }),
                    XValue::Str(s) => nodes.iter().any(|node| {
                        let v = self.string_value(*node);
                        if equality {
                            (v == *s) == (op == BinOp::Eq)
                        } else {
                            let (a, b) = (string_to_number(&v), string_to_number(s));
                            if nodes_left {
                                cmp_num(a, b)
                            } else {
                                cmp_num(b, a)
                            }
                        }
                    }),
                    XValue::Nodes(_) => false,
                }
            }
            _ => {
                if equality {
                    if matches!(left, XValue::Bool(_)) || matches!(right, XValue::Bool(_)) {
                        (self.boolean(left) == self.boolean(right)) == (op == BinOp::Eq)
                    } else if matches!(left, XValue::Num(_)) || matches!(right, XValue::Num(_)) {
                        cmp_num(self.number(left), self.number(right))
                    } else {
                        (self.string(left) == self.string(right)) == (op == BinOp::Eq)
                    }
                } else {
                    cmp_num(self.number(left), self.number(right))
                }
            }
        }
    }

    fn eval(&self, expr: &Expr, focus: Focus) -> Result<XValue, XError> {
        Ok(match expr {
            Expr::Literal(value) => XValue::Str(value.clone()),
            Expr::Number(value) => XValue::Num(*value),
            Expr::Neg(inner) => XValue::Num(-self.number(&self.eval(inner, focus)?)),
            Expr::Binary(op, left, right) => match op {
                BinOp::Or => XValue::Bool(
                    self.boolean(&self.eval(left, focus)?)
                        || self.boolean(&self.eval(right, focus)?),
                ),
                BinOp::And => XValue::Bool(
                    self.boolean(&self.eval(left, focus)?)
                        && self.boolean(&self.eval(right, focus)?),
                ),
                BinOp::Union => {
                    let (XValue::Nodes(mut a), XValue::Nodes(b)) =
                        (self.eval(left, focus)?, self.eval(right, focus)?)
                    else {
                        return Err(XError::Eval("union of non-node-sets".into()));
                    };
                    a.extend(b);
                    self.sort(&mut a);
                    XValue::Nodes(a)
                }
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                    let a = self.number(&self.eval(left, focus)?);
                    let b = self.number(&self.eval(right, focus)?);
                    XValue::Num(match op {
                        BinOp::Add => a + b,
                        BinOp::Sub => a - b,
                        BinOp::Mul => a * b,
                        BinOp::Div => a / b,
                        _ => a % b,
                    })
                }
                _ => {
                    let a = self.eval(left, focus)?;
                    let b = self.eval(right, focus)?;
                    XValue::Bool(self.compare(*op, &a, &b))
                }
            },
            Expr::Filter(primary, predicates) => {
                let XValue::Nodes(mut nodes) = self.eval(primary, focus)? else {
                    return Err(XError::Eval("predicate on a non-node-set".into()));
                };
                for predicate in predicates {
                    nodes = self.filter(nodes, predicate)?;
                }
                XValue::Nodes(nodes)
            }
            Expr::Path(start, steps) => {
                let mut nodes = match start {
                    PathStart::Root => vec![XNode::Root],
                    PathStart::Context => vec![focus.node],
                    PathStart::Filter(inner) => match self.eval(inner, focus)? {
                        XValue::Nodes(nodes) => nodes,
                        _ => return Err(XError::Eval("path from a non-node-set".into())),
                    },
                };
                for step in steps {
                    nodes = self.step(&nodes, step)?;
                }
                XValue::Nodes(nodes)
            }
            Expr::Call(prefix, name, args) => self.call(prefix.as_deref(), name, args, focus)?,
        })
    }

    fn call(
        &self,
        prefix: Option<&str>,
        name: &str,
        args: &[Expr],
        focus: Focus,
    ) -> Result<XValue, XError> {
        if let Some(prefix) = prefix {
            self.resolve_prefix(prefix)?;
            return Err(XError::Eval(format!(
                "Unregistered function {prefix}:{name}"
            )));
        }
        let values: Vec<XValue> = args
            .iter()
            .map(|a| self.eval(a, focus))
            .collect::<Result<_, _>>()?;
        let arity = |min: usize, max: usize| {
            if values.len() < min || values.len() > max {
                Err(XError::Eval(format!(
                    "Invalid number of arguments for {name}"
                )))
            } else {
                Ok(())
            }
        };
        let context_string = |values: &[XValue]| match values.first() {
            Some(value) => self.string(value),
            None => self.string_value(focus.node),
        };
        let node_arg = |values: &[XValue]| -> Result<Option<XNode>, XError> {
            match values.first() {
                None => Ok(Some(focus.node)),
                Some(XValue::Nodes(nodes)) => Ok(nodes.first().copied()),
                Some(_) => Err(XError::Eval(format!("{name} needs a node-set"))),
            }
        };
        Ok(match name {
            "last" => {
                arity(0, 0)?;
                XValue::Num(focus.size as f64)
            }
            "position" => {
                arity(0, 0)?;
                XValue::Num(focus.position as f64)
            }
            "count" => {
                arity(1, 1)?;
                match &values[0] {
                    XValue::Nodes(nodes) => XValue::Num(nodes.len() as f64),
                    _ => return Err(XError::Eval("count needs a node-set".into())),
                }
            }
            "local-name" | "name" | "namespace-uri" => {
                arity(0, 1)?;
                let node = node_arg(&values)?;
                let (ns, prefix, local) = match node {
                    Some(XNode::Node(id)) => match self.tree.element(id) {
                        Some(element) => (
                            self.tree.ns_uri(id).map(str::to_string),
                            element.name.prefix.as_deref().map(str::to_string),
                            element.name.local.to_string(),
                        ),
                        None => match self.tree.data(id) {
                            NodeData::Pi { target, .. } => (None, None, target.clone()),
                            _ => (None, None, String::new()),
                        },
                    },
                    Some(XNode::Attr(element, index)) => {
                        match self.tree.element(element).and_then(|e| e.attrs.get(index)) {
                            Some(attr) => (
                                attr.ns.map(|n| self.tree.uri(n).to_string()),
                                attr.name.prefix.as_deref().map(str::to_string),
                                attr.name.local.to_string(),
                            ),
                            None => (None, None, String::new()),
                        }
                    }
                    _ => (None, None, String::new()),
                };
                XValue::Str(match name {
                    "local-name" => local,
                    "namespace-uri" => ns.unwrap_or_default(),
                    _ => match prefix {
                        Some(prefix) => format!("{prefix}:{local}"),
                        None => local,
                    },
                })
            }
            "string" => {
                arity(0, 1)?;
                XValue::Str(context_string(&values))
            }
            "concat" => {
                if values.len() < 2 {
                    return Err(XError::Eval("concat needs two arguments".into()));
                }
                XValue::Str(values.iter().map(|v| self.string(v)).collect())
            }
            "starts-with" | "contains" | "substring-before" | "substring-after" => {
                arity(2, 2)?;
                let a = self.string(&values[0]);
                let b = self.string(&values[1]);
                match name {
                    "starts-with" => XValue::Bool(a.starts_with(&b)),
                    "contains" => XValue::Bool(a.contains(&b)),
                    "substring-before" => {
                        XValue::Str(a.find(&b).map(|i| a[..i].to_string()).unwrap_or_default())
                    }
                    _ => XValue::Str(
                        a.find(&b)
                            .map(|i| a[i + b.len()..].to_string())
                            .unwrap_or_default(),
                    ),
                }
            }
            "substring" => {
                arity(2, 3)?;
                let text: Vec<char> = self.string(&values[0]).chars().collect();
                let start = self.number(&values[1]).round();
                let end = match values.get(2) {
                    Some(length) => start + self.number(length).round(),
                    None => f64::INFINITY,
                };
                XValue::Str(
                    text.iter()
                        .enumerate()
                        .filter(|(i, _)| {
                            let p = (*i + 1) as f64;
                            p >= start && p < end
                        })
                        .map(|(_, c)| *c)
                        .collect(),
                )
            }
            "string-length" => {
                arity(0, 1)?;
                XValue::Num(context_string(&values).chars().count() as f64)
            }
            "normalize-space" => {
                arity(0, 1)?;
                XValue::Str(
                    context_string(&values)
                        .split([' ', '\t', '\r', '\n'])
                        .filter(|p| !p.is_empty())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }
            "translate" => {
                arity(3, 3)?;
                let from: Vec<char> = self.string(&values[1]).chars().collect();
                let to: Vec<char> = self.string(&values[2]).chars().collect();
                XValue::Str(
                    self.string(&values[0])
                        .chars()
                        .filter_map(|c| match from.iter().position(|f| *f == c) {
                            Some(i) => to.get(i).copied(),
                            None => Some(c),
                        })
                        .collect(),
                )
            }
            "boolean" => {
                arity(1, 1)?;
                XValue::Bool(self.boolean(&values[0]))
            }
            "not" => {
                arity(1, 1)?;
                XValue::Bool(!self.boolean(&values[0]))
            }
            "true" | "false" => {
                arity(0, 0)?;
                XValue::Bool(name == "true")
            }
            "number" => {
                arity(0, 1)?;
                match values.first() {
                    Some(value) => XValue::Num(self.number(value)),
                    None => XValue::Num(string_to_number(&self.string_value(focus.node))),
                }
            }
            "sum" => {
                arity(1, 1)?;
                match &values[0] {
                    XValue::Nodes(nodes) => XValue::Num(
                        nodes
                            .iter()
                            .map(|n| string_to_number(&self.string_value(*n)))
                            .sum(),
                    ),
                    _ => return Err(XError::Eval("sum needs a node-set".into())),
                }
            }
            "floor" | "ceiling" | "round" => {
                arity(1, 1)?;
                let n = self.number(&values[0]);
                XValue::Num(match name {
                    "floor" => n.floor(),
                    "ceiling" => n.ceil(),
                    _ => (n + 0.5).floor(),
                })
            }
            "id" | "lang" => return Err(XError::Unsupported(format!("the {name}() function"))),
            _ => return Err(XError::Eval(format!("Unregistered function {name}"))),
        })
    }
}

/// Evaluate `expr` with the document element of `tree` as context node.
pub(crate) fn evaluate(
    tree: &XmlTree,
    expr: &str,
    namespaces: &[(String, String)],
) -> Result<XValue, XError> {
    let toks = lex(expr)?;
    if toks.is_empty() {
        return Err(XError::Syntax("empty expression".into()));
    }
    let mut parser = Parser { toks, pos: 0 };
    let ast = parser.expr()?;
    if parser.pos != parser.toks.len() {
        return Err(XError::Syntax("trailing tokens".into()));
    }
    let ctx = Ctx::new(tree, namespaces);
    ctx.eval(
        &ast,
        Focus {
            node: XNode::Node(tree.root()),
            position: 1,
            size: 1,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nodes(xml: &str, expr: &str, ns: &[(&str, &str)]) -> Result<XValue, XError> {
        let tree = crate::xml::parse_str(xml).unwrap();
        let ns: Vec<(String, String)> = ns
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        evaluate(&tree, expr, &ns)
    }

    #[test]
    fn paths_predicates_and_values() {
        let xml = r#"<f:root xmlns:f="urn:f"><f:name a="1">old</f:name><f:name a="2">two</f:name></f:root>"#;
        let ns = [("f", "urn:f")];
        let XValue::Nodes(found) = nodes(xml, "/f:root/f:name[2]", &ns).unwrap() else {
            panic!()
        };
        assert_eq!(found.len(), 1);
        let XValue::Nodes(found) = nodes(xml, "/f:root/f:name[@a='1']/@a", &ns).unwrap() else {
            panic!()
        };
        assert!(matches!(found[0], XNode::Attr(_, 0)));
        assert!(matches!(nodes(xml, "count(//f:name)", &ns).unwrap(), XValue::Num(n) if n == 2.0));
        assert!(matches!(
            nodes(xml, "/f:root/[[[", &ns),
            Err(XError::Syntax(_))
        ));
        assert!(matches!(
            nodes(xml, "bogus-fn(/f:root)", &ns),
            Err(XError::Eval(_))
        ));
        assert!(matches!(nodes(xml, "/g:root", &ns), Err(XError::Eval(_))));
        assert!(matches!(
            nodes(xml, "boolean(/f:root)", &ns).unwrap(),
            XValue::Bool(true)
        ));
        let XValue::Nodes(found) = nodes(xml, "/f:root/f:name[last()] | /f:root", &ns).unwrap()
        else {
            panic!()
        };
        assert_eq!(found.len(), 2);
    }
}
