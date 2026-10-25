//! Read-only openCypher subset over the graph.
//!
//! Supported: MATCH / OPTIONAL MATCH / WHERE / WITH (+WHERE) / RETURN /
//! ORDER BY / SKIP / LIMIT / DISTINCT / UNWIND / UNION [ALL] / CASE, labelled
//! nodes and alternation, relationship types/direction, variable-length paths,
//! inline property maps, comparisons, IN, CONTAINS, STARTS/ENDS WITH, IS NULL,
//! regex, label tests, single-hop EXISTS, aggregates and the documented scalar
//! functions. Anything outside the subset fails explicitly.

use gry_core::types::{Edge, Node};
use gry_store::Store;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

// ── Tokenizer ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f64),
    Punct(char),
    Op(String),
}

fn tokenize(input: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != quote {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    s.push(match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    s.push(chars[i]);
                }
                i += 1;
            }
            if i >= chars.len() {
                return Err("unterminated string literal".to_string());
            }
            i += 1;
            out.push(Tok::Str(s));
            continue;
        }
        if c.is_ascii_digit() || (c == '-' && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()))
        {
            let start = i;
            if c == '-' {
                i += 1;
            }
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || (chars[i] == '.' && chars.get(i + 1) != Some(&'.')))
            {
                i += 1;
            }
            let raw: String = chars[start..i].iter().collect();
            out.push(Tok::Num(
                raw.parse().map_err(|_| format!("invalid number {raw}"))?,
            ));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        // Multi-character operators.
        let rest: String = chars[i..].iter().take(4).collect();
        let op = ["=~", "<>", "!=", "<=", ">=", "->", "<-", "..", "+="]
            .iter()
            .find(|op| rest.starts_with(**op))
            .copied();
        if let Some(op) = op {
            i += op.chars().count();
            out.push(Tok::Op(op.to_string()));
            continue;
        }
        out.push(Tok::Punct(c));
        i += 1;
    }
    Ok(out)
}

// ── AST ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Expr {
    Var(String),
    Property(Box<Expr>, String),
    Literal(Value),
    List(Vec<Expr>),
    Func(String, Vec<Expr>, bool), // name, args, distinct
    Binary(String, Box<Expr>, Box<Expr>),
    Unary(String, Box<Expr>),
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        otherwise: Option<Box<Expr>>,
    },
    Exists(Box<Pattern>),
    LabelTest(Box<Expr>, Vec<String>),
    Star,
}

#[derive(Debug, Clone)]
struct NodePattern {
    var: Option<String>,
    labels: Vec<String>,
    props: Vec<(String, Value)>,
}

#[derive(Debug, Clone)]
struct RelPattern {
    var: Option<String>,
    types: Vec<String>,
    direction: RelDirection,
    min: usize,
    max: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum RelDirection {
    Out,
    In,
    Both,
}

#[derive(Debug, Clone)]
struct Pattern {
    start: NodePattern,
    steps: Vec<(RelPattern, NodePattern)>,
}

#[derive(Debug, Clone)]
enum Clause {
    Match {
        optional: bool,
        patterns: Vec<Pattern>,
        where_: Option<Expr>,
    },
    With {
        items: Vec<ReturnItem>,
        distinct: bool,
        where_: Option<Expr>,
    },
    Return {
        items: Vec<ReturnItem>,
        distinct: bool,
        order: Vec<(Expr, bool)>,
        skip: Option<usize>,
        limit: Option<usize>,
    },
    Unwind {
        expr: Expr,
        alias: String,
    },
}

#[derive(Debug, Clone)]
struct ReturnItem {
    expr: Expr,
    alias: Option<String>,
}

#[derive(Debug, Clone)]
struct Query {
    clauses: Vec<Clause>,
}

// ── Parser ──────────────────────────────────────────────────────────

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn new(toks: Vec<Tok>) -> Self {
        Self { toks, pos: 0 }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn peek_ident(&self) -> Option<&str> {
        match self.peek() {
            Some(Tok::Ident(s)) => Some(s),
            _ => None,
        }
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn eat_keyword(&mut self, kw: &str) -> bool {
        if self
            .peek_ident()
            .map(|s| s.eq_ignore_ascii_case(kw))
            .unwrap_or(false)
        {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, kw: &str) -> Result<(), String> {
        if self.eat_keyword(kw) {
            Ok(())
        } else {
            Err(format!("expected {kw}"))
        }
    }

    fn expect_punct(&mut self, p: char) -> Result<(), String> {
        if self.peek() == Some(&Tok::Punct(p)) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected '{p}'"))
        }
    }

    fn eat_punct(&mut self, p: char) -> bool {
        if self.peek() == Some(&Tok::Punct(p)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.peek() == Some(&Tok::Op(op.to_string())) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_query(&mut self) -> Result<Query, String> {
        let mut clauses = Vec::new();
        loop {
            if self.eat_keyword("MATCH") {
                let patterns = self.parse_patterns()?;
                let where_ = if self.eat_keyword("WHERE") {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                clauses.push(Clause::Match {
                    optional: false,
                    patterns,
                    where_,
                });
            } else if self.eat_keyword("OPTIONAL") {
                self.expect_keyword("MATCH")?;
                let patterns = self.parse_patterns()?;
                let where_ = if self.eat_keyword("WHERE") {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                clauses.push(Clause::Match {
                    optional: true,
                    patterns,
                    where_,
                });
            } else if self.eat_keyword("WITH") {
                let distinct = self.eat_keyword("DISTINCT");
                let items = self.parse_return_items()?;
                let where_ = if self.eat_keyword("WHERE") {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                clauses.push(Clause::With {
                    items,
                    distinct,
                    where_,
                });
            } else if self.eat_keyword("UNWIND") {
                let expr = self.parse_expr()?;
                self.expect_keyword("AS")?;
                let alias = match self.next() {
                    Some(Tok::Ident(s)) => s,
                    _ => return Err("expected UNWIND alias".to_string()),
                };
                clauses.push(Clause::Unwind { expr, alias });
            } else if self.eat_keyword("RETURN") {
                let distinct = self.eat_keyword("DISTINCT");
                let items = self.parse_return_items()?;
                let mut order = Vec::new();
                if self.eat_keyword("ORDER") {
                    self.expect_keyword("BY")?;
                    loop {
                        let expr = self.parse_expr()?;
                        let asc = if self.eat_keyword("ASC") || self.eat_keyword("ASCENDING") {
                            true
                        } else if self.eat_keyword("DESC") || self.eat_keyword("DESCENDING") {
                            false
                        } else {
                            true
                        };
                        order.push((expr, asc));
                        if !self.eat_punct(',') {
                            break;
                        }
                    }
                }
                let skip = if self.eat_keyword("SKIP") {
                    Some(self.parse_usize()?)
                } else {
                    None
                };
                let limit = if self.eat_keyword("LIMIT") {
                    Some(self.parse_usize()?)
                } else {
                    None
                };
                clauses.push(Clause::Return {
                    items,
                    distinct,
                    order,
                    skip,
                    limit,
                });
                break;
            } else if self.pos >= self.toks.len() {
                break;
            } else {
                return Err(format!("unsupported clause near {:?}", self.peek()));
            }
        }
        if clauses.is_empty() {
            return Err("empty query".to_string());
        }
        Ok(Query { clauses })
    }

    fn parse_patterns(&mut self) -> Result<Vec<Pattern>, String> {
        let mut patterns = Vec::new();
        loop {
            patterns.push(self.parse_pattern()?);
            if !self.eat_punct(',') {
                break;
            }
        }
        Ok(patterns)
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        let start = self.parse_node_pattern()?;
        let mut steps = Vec::new();
        loop {
            let save = self.pos;
            let left_arrow = self.eat_op("<-");
            if !left_arrow && !self.eat_punct('-') {
                self.pos = save;
                break;
            }
            if !left_arrow {
                // Could be a bare `-` relationship without brackets.
            }
            let mut rel = RelPattern {
                var: None,
                types: Vec::new(),
                direction: if left_arrow {
                    RelDirection::In
                } else {
                    RelDirection::Out
                },
                min: 1,
                max: 1,
            };
            if self.eat_punct('[') {
                if let Some(Tok::Ident(v)) = self.peek().cloned() {
                    rel.var = Some(v);
                    self.pos += 1;
                }
                if self.eat_punct(':') {
                    loop {
                        match self.next() {
                            Some(Tok::Ident(t)) => rel.types.push(t),
                            _ => return Err("expected relationship type".to_string()),
                        }
                        if !self.eat_punct('|') {
                            break;
                        }
                        self.eat_punct(':');
                    }
                }
                if self.eat_op("..") || self.eat_punct('*') {
                    // *1..3 / *..3 / *2
                    if let Some(Tok::Num(n)) = self.peek().cloned() {
                        if n >= 1.0 {
                            rel.min = n as usize;
                            self.pos += 1;
                        }
                    }
                    if self.eat_op("..") {
                        if let Some(Tok::Num(n)) = self.peek().cloned() {
                            rel.max = n as usize;
                            self.pos += 1;
                        }
                    } else if rel.min > 1 {
                        rel.max = rel.min;
                    } else {
                        rel.max = 5;
                    }
                } else if self.eat_punct('*') {
                    rel.max = 5;
                }
                // Inline relationship property maps are not supported.
                if self.peek() == Some(&Tok::Punct('{')) {
                    return Err("unsupported relationship property map".to_string());
                }
                self.expect_punct(']')?;
            }
            let right_arrow = self.eat_op("->");
            if !right_arrow {
                self.eat_punct('-');
            }
            if left_arrow {
                rel.direction = RelDirection::In;
            } else if right_arrow {
                rel.direction = RelDirection::Out;
            } else {
                rel.direction = RelDirection::Both;
            }
            let node = self.parse_node_pattern()?;
            steps.push((rel, node));
            if steps.len() > 8 {
                return Err("unsupported pattern length (max 8 relationships)".to_string());
            }
        }
        Ok(Pattern { start, steps })
    }

    fn parse_node_pattern(&mut self) -> Result<NodePattern, String> {
        self.expect_punct('(')?;
        let mut var = None;
        if let Some(Tok::Ident(v)) = self.peek().cloned() {
            var = Some(v);
            self.pos += 1;
        }
        let mut labels = Vec::new();
        if self.eat_punct(':') {
            loop {
                match self.next() {
                    Some(Tok::Ident(l)) => labels.push(l),
                    _ => return Err("expected node label".to_string()),
                }
                if !self.eat_punct('|') {
                    break;
                }
                self.eat_punct(':');
            }
        }
        let mut props = Vec::new();
        if self.eat_punct('{') {
            loop {
                let key = match self.next() {
                    Some(Tok::Ident(k)) => k,
                    _ => return Err("expected property key".to_string()),
                };
                self.expect_punct(':')?;
                let value = self.parse_primary()?;
                props.push((key, eval_const(&value)?));
                if !self.eat_punct(',') {
                    break;
                }
            }
            self.expect_punct('}')?;
        }
        self.expect_punct(')')?;
        Ok(NodePattern { var, labels, props })
    }

    fn parse_return_items(&mut self) -> Result<Vec<ReturnItem>, String> {
        let mut items = Vec::new();
        loop {
            let expr = self.parse_expr()?;
            let alias = if self.eat_keyword("AS") {
                match self.next() {
                    Some(Tok::Ident(a)) => Some(a),
                    _ => return Err("expected alias after AS".to_string()),
                }
            } else {
                None
            };
            items.push(ReturnItem { expr, alias });
            if !self.eat_punct(',') {
                break;
            }
        }
        Ok(items)
    }

    fn parse_usize(&mut self) -> Result<usize, String> {
        match self.next() {
            Some(Tok::Num(n)) if n >= 0.0 => Ok(n as usize),
            _ => Err("expected non-negative integer".to_string()),
        }
    }

    // Expression precedence: OR, XOR, AND, NOT, comparison, additive, multiplicative, unary, primary.
    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_xor()?;
        while self.eat_keyword("OR") {
            let right = self.parse_xor()?;
            left = Expr::Binary("OR".into(), Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_xor(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_and()?;
        while self.eat_keyword("XOR") {
            let right = self.parse_and()?;
            left = Expr::Binary("XOR".into(), Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_not()?;
        while self.eat_keyword("AND") {
            let right = self.parse_not()?;
            left = Expr::Binary("AND".into(), Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, String> {
        if self.eat_keyword("NOT") {
            if self.eat_keyword("EXISTS") {
                let pattern = self.parse_exists_pattern()?;
                return Ok(Expr::Unary(
                    "NOT".into(),
                    Box::new(Expr::Exists(Box::new(pattern))),
                ));
            }
            return Ok(Expr::Unary("NOT".into(), Box::new(self.parse_not()?)));
        }
        if self.eat_keyword("EXISTS") {
            let pattern = self.parse_exists_pattern()?;
            return Ok(Expr::Exists(Box::new(pattern)));
        }
        self.parse_comparison()
    }

    fn parse_exists_pattern(&mut self) -> Result<Pattern, String> {
        self.expect_punct('{')?;
        let pattern = self.parse_pattern()?;
        self.expect_punct('}')?;
        Ok(pattern)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_additive()?;
        loop {
            if self.eat_keyword("IS") {
                let not = self.eat_keyword("NOT");
                self.expect_keyword("NULL")?;
                let expr = Expr::Unary(
                    if not {
                        "IS NOT NULL".into()
                    } else {
                        "IS NULL".into()
                    },
                    Box::new(left),
                );
                left = expr;
                continue;
            }
            let op = if self.eat_punct('=') || self.eat_op("=") {
                Some("=")
            } else if self.eat_op("<>") || self.eat_op("!=") {
                Some("<>")
            } else if self.eat_op("<=") {
                Some("<=")
            } else if self.eat_op(">=") {
                Some(">=")
            } else if self.eat_punct('<') {
                Some("<")
            } else if self.eat_punct('>') {
                Some(">")
            } else if self.eat_op("=~") {
                Some("=~")
            } else if self.eat_keyword("IN") {
                Some("IN")
            } else if self.eat_keyword("CONTAINS") {
                Some("CONTAINS")
            } else if self.eat_keyword("STARTS") {
                self.expect_keyword("WITH")?;
                Some("STARTS WITH")
            } else if self.eat_keyword("ENDS") {
                self.expect_keyword("WITH")?;
                Some("ENDS WITH")
            } else {
                None
            };
            if let Some(op) = op {
                let right = self.parse_additive()?;
                left = Expr::Binary(op.to_string(), Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        // Label test `n:Label`.
        if self.eat_punct(':') {
            let mut labels = Vec::new();
            loop {
                match self.next() {
                    Some(Tok::Ident(l)) => labels.push(l),
                    _ => return Err("expected label after ':'".to_string()),
                }
                if !self.eat_punct('|') {
                    break;
                }
                self.eat_punct(':');
            }
            left = Expr::LabelTest(Box::new(left), labels);
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_multiplicative()?;
        loop {
            if self.eat_punct('+') {
                let right = self.parse_multiplicative()?;
                left = Expr::Binary("+".into(), Box::new(left), Box::new(right));
            } else if self.eat_punct('-') {
                let right = self.parse_multiplicative()?;
                left = Expr::Binary("-".into(), Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        let mut left = self.parse_unary()?;
        loop {
            if self.eat_punct('*') {
                let right = self.parse_unary()?;
                left = Expr::Binary("*".into(), Box::new(left), Box::new(right));
            } else if self.eat_punct('/') {
                let right = self.parse_unary()?;
                left = Expr::Binary("/".into(), Box::new(left), Box::new(right));
            } else if self.eat_punct('%') {
                let right = self.parse_unary()?;
                left = Expr::Binary("%".into(), Box::new(left), Box::new(right));
            } else {
                break;
            }
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if self.eat_punct('-') {
            return Ok(Expr::Unary("-".into(), Box::new(self.parse_unary()?)));
        }
        if self.eat_punct('+') {
            return self.parse_unary();
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let tok = self
            .next()
            .ok_or_else(|| "unexpected end of query".to_string())?;
        let mut expr = match tok {
            Tok::Str(s) => Expr::Literal(Value::String(s)),
            Tok::Num(n) => Expr::Literal(if n.fract() == 0.0 {
                json!(n as i64)
            } else {
                json!(n)
            }),
            Tok::Punct('(') => {
                let e = self.parse_expr()?;
                self.expect_punct(')')?;
                e
            }
            Tok::Punct('[') => {
                let mut items = Vec::new();
                if !self.eat_punct(']') {
                    loop {
                        items.push(self.parse_expr()?);
                        if !self.eat_punct(',') {
                            break;
                        }
                    }
                    self.expect_punct(']')?;
                }
                Expr::List(items)
            }
            Tok::Punct('{') => return Err("unsupported map literal".to_string()),
            Tok::Punct('$') => return Err("unsupported query parameter".to_string()),
            Tok::Punct('*') => Expr::Star,
            Tok::Ident(id) => {
                if id.eq_ignore_ascii_case("NULL") {
                    Expr::Literal(Value::Null)
                } else if id.eq_ignore_ascii_case("TRUE") {
                    Expr::Literal(Value::Bool(true))
                } else if id.eq_ignore_ascii_case("FALSE") {
                    Expr::Literal(Value::Bool(false))
                } else if id.eq_ignore_ascii_case("CASE") {
                    return self.parse_case();
                } else if self.peek() == Some(&Tok::Punct('(')) {
                    self.pos += 1;
                    let distinct = self.eat_keyword("DISTINCT");
                    let mut args = Vec::new();
                    if !self.eat_punct(')') {
                        loop {
                            args.push(self.parse_expr()?);
                            if !self.eat_punct(',') {
                                break;
                            }
                        }
                        self.expect_punct(')')?;
                    }
                    Expr::Func(id.to_ascii_lowercase(), args, distinct)
                } else {
                    Expr::Var(id)
                }
            }
            Tok::Op(op) => return Err(format!("unsupported operator {op}")),
            other => return Err(format!("unexpected token {other:?}")),
        };
        loop {
            if self.eat_punct('.') {
                match self.next() {
                    Some(Tok::Ident(prop)) => {
                        expr = Expr::Property(Box::new(expr), prop);
                    }
                    _ => return Err("expected property name after '.'".to_string()),
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_case(&mut self) -> Result<Expr, String> {
        let operand = if !self
            .peek_ident()
            .map(|s| s.eq_ignore_ascii_case("WHEN"))
            .unwrap_or(false)
        {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        let mut whens = Vec::new();
        while self.eat_keyword("WHEN") {
            let when = self.parse_expr()?;
            self.expect_keyword("THEN")?;
            let then = self.parse_expr()?;
            whens.push((when, then));
        }
        let otherwise = if self.eat_keyword("ELSE") {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect_keyword("END")?;
        Ok(Expr::Case {
            operand,
            whens,
            otherwise,
        })
    }
}

fn eval_const(expr: &Expr) -> Result<Value, String> {
    match expr {
        Expr::Literal(v) => Ok(v.clone()),
        Expr::Unary(op, e) if op == "-" => match eval_const(e)? {
            Value::Number(n) => Ok(json!(-n.as_f64().unwrap_or(0.0))),
            _ => Err("invalid negative literal".to_string()),
        },
        _ => Err("property map values must be literals".to_string()),
    }
}

// ── Graph snapshot ─────────────────────────────────────────────────

struct Graph {
    nodes: Vec<Node>,
    by_id: HashMap<i64, Node>,
    edges: Vec<Edge>,
    out: HashMap<i64, Vec<usize>>,
    inc: HashMap<i64, Vec<usize>>,
}

impl Graph {
    fn load(store: &Store, project: &str) -> gry_core::GryResult<Self> {
        let nodes = store.all_nodes(project)?;
        let edges = store.all_edges(project)?;
        let mut by_id = HashMap::new();
        for n in &nodes {
            by_id.insert(n.id, n.clone());
        }
        let mut out: HashMap<i64, Vec<usize>> = HashMap::new();
        let mut inc: HashMap<i64, Vec<usize>> = HashMap::new();
        for (idx, e) in edges.iter().enumerate() {
            out.entry(e.source_id).or_default().push(idx);
            inc.entry(e.target_id).or_default().push(idx);
        }
        Ok(Self {
            nodes,
            by_id,
            edges,
            out,
            inc,
        })
    }
}

type Binding = HashMap<String, Value>;

fn node_value(node: &Node) -> Value {
    let mut map = Map::new();
    if let Value::Object(props) = &node.properties {
        for (k, v) in props {
            map.insert(k.clone(), v.clone());
        }
    }
    map.insert("name".into(), Value::String(node.name.clone()));
    map.insert("label".into(), Value::String(node.label.clone()));
    map.insert(
        "qualified_name".into(),
        Value::String(node.qualified_name.clone()),
    );
    map.insert("qn".into(), Value::String(node.qualified_name.clone()));
    map.insert("file_path".into(), Value::String(node.file_path.clone()));
    map.insert("file".into(), Value::String(node.file_path.clone()));
    map.insert("start_line".into(), json!(node.start_line));
    map.insert("end_line".into(), json!(node.end_line));
    map.insert("__node_id".into(), json!(node.id));
    Value::Object(map)
}

fn edge_value(edge: &Edge) -> Value {
    let mut map = Map::new();
    if let Value::Object(props) = &edge.properties {
        for (k, v) in props {
            map.insert(k.clone(), v.clone());
        }
    }
    map.insert("__edge_id".into(), json!(edge.id));
    map.insert("__type".into(), Value::String(edge.edge_type.clone()));
    map.insert("__source".into(), json!(edge.source_id));
    map.insert("__target".into(), json!(edge.target_id));
    Value::Object(map)
}

fn externalize(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            if let Some(id) = map.get("__node_id").and_then(Value::as_i64) {
                let mut out = Map::new();
                out.insert("id".into(), json!(id));
                for key in [
                    "name",
                    "label",
                    "qualified_name",
                    "qn",
                    "file_path",
                    "file",
                    "start_line",
                    "end_line",
                ] {
                    if let Some(v) = map.get(key) {
                        out.insert(key.into(), v.clone());
                    }
                }
                let mut props = Map::new();
                for (k, v) in map {
                    if !k.starts_with("__")
                        && !matches!(
                            k.as_str(),
                            "name"
                                | "label"
                                | "qualified_name"
                                | "qn"
                                | "file_path"
                                | "file"
                                | "start_line"
                                | "end_line"
                        )
                    {
                        props.insert(k.clone(), externalize(v));
                    }
                }
                out.insert("properties".into(), Value::Object(props));
                return Value::Object(out);
            }
            if let Some(id) = map.get("__edge_id").and_then(Value::as_i64) {
                let mut out = Map::new();
                out.insert("id".into(), json!(id));
                out.insert(
                    "type".into(),
                    map.get("__type").cloned().unwrap_or(Value::Null),
                );
                out.insert(
                    "source".into(),
                    map.get("__source").cloned().unwrap_or(Value::Null),
                );
                out.insert(
                    "target".into(),
                    map.get("__target").cloned().unwrap_or(Value::Null),
                );
                let mut props = Map::new();
                for (k, v) in map {
                    if !k.starts_with("__") {
                        props.insert(k.clone(), externalize(v));
                    }
                }
                out.insert("properties".into(), Value::Object(props));
                return Value::Object(out);
            }
            Value::Object(
                map.iter()
                    .filter(|(k, _)| !k.starts_with("__"))
                    .map(|(k, v)| (k.clone(), externalize(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(externalize).collect()),
        other => other.clone(),
    }
}

// ── Evaluation ─────────────────────────────────────────────────────

pub struct QueryRequest {
    pub project: String,
    pub query: String,
    pub max_rows: usize,
    pub offset: usize,
    pub cursor: Option<String>,
    pub graph: String,
}

pub fn query_graph(store: &Store, req: &QueryRequest) -> gry_core::GryResult<Value> {
    if req.graph == "missed" {
        return missed_graph(store, &req.project);
    }
    if req.graph != "code" && !req.graph.is_empty() {
        return Err(gry_core::GryError::invalid(format!(
            "invalid graph value: {} (use code or missed)",
            req.graph
        )));
    }
    let project = store.require_project(&req.project)?;
    let offset = if let Some(cursor) = &req.cursor {
        let (generation, query, offset) = decode_cursor(cursor)?;
        if generation != project.generation {
            return Err(gry_core::GryError::invalid(
                "stale cursor: the project was reindexed; re-run the query without a cursor",
            ));
        }
        if query != req.query {
            return Err(gry_core::GryError::invalid(
                "cursor belongs to a different query; re-run without a cursor",
            ));
        }
        offset
    } else {
        req.offset
    };
    let max_rows = req.max_rows.max(1).min(99_998);
    let graph = Graph::load(store, &req.project)?;
    let parts = split_union(&req.query)?;
    let mut all_rows: Vec<(Vec<Value>, Binding)> = Vec::new();
    let mut columns: Vec<String> = Vec::new();
    let mut union_all = false;
    for (idx, part) in parts.iter().enumerate() {
        if idx > 0 {
            union_all = part.all;
        }
        let toks = tokenize(&part.query).map_err(|e| gry_core::GryError::unsupported(e))?;
        let mut parser = Parser::new(toks);
        let query = parser
            .parse_query()
            .map_err(|e| gry_core::GryError::unsupported(e))?;
        if parser.pos < parser.toks.len() {
            return Err(gry_core::GryError::unsupported(format!(
                "unsupported trailing tokens near {:?}",
                parser.toks.get(parser.pos)
            )));
        }
        let (cols, rows) = execute(&graph, &query)?;
        if columns.is_empty() {
            columns = cols;
        } else if columns != cols {
            return Err(gry_core::GryError::unsupported(
                "UNION branches must return the same columns".to_string(),
            ));
        }
        all_rows.extend(rows);
    }
    if !union_all {
        let mut seen = HashSet::new();
        all_rows.retain(|(values, _)| {
            seen.insert(
                values
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join("\u{1f}"),
            )
        });
    }
    let total = all_rows.len();
    let page: Vec<Value> = all_rows
        .into_iter()
        .skip(offset)
        .take(max_rows)
        .map(|(values, _)| Value::Array(values.into_iter().map(|v| externalize(&v)).collect()))
        .collect();
    let next_offset = offset + page.len();
    let next_cursor = if next_offset < total {
        Some(encode_cursor(&project.generation, &req.query, next_offset))
    } else {
        None
    };
    Ok(json!({
        "project": req.project,
        "generation": project.generation,
        "columns": columns,
        "rows": page,
        "total": total,
        "returned": page.len(),
        "offset": offset,
        "has_more": next_offset < total,
        "next_cursor": next_cursor,
        "next_offset": if next_offset < total { Some(next_offset) } else { None },
        "query": req.query,
    }))
}

fn missed_graph(store: &Store, project: &str) -> gry_core::GryResult<Value> {
    store.require_project(project)?;
    let rows = store.coverage(project)?;
    let mut tree: BTreeMapLike = BTreeMapLike::default();
    for row in &rows {
        tree.insert(&row.rel_path, &row.kind, &row.detail);
    }
    Ok(json!({
        "project": project,
        "graph": "missed",
        "gaps": rows.len(),
        "files": tree.into_json(),
        "note": "Absence from this list is not proof of completeness.",
    }))
}

#[derive(Default)]
struct BTreeMapLike {
    children: std::collections::BTreeMap<String, BTreeMapLike>,
    gaps: Vec<Value>,
}

impl BTreeMapLike {
    fn insert(&mut self, path: &str, kind: &str, detail: &str) {
        let mut current = self;
        for part in path.split('/') {
            current = current.children.entry(part.to_string()).or_default();
        }
        current.gaps.push(json!({"kind": kind, "detail": detail}));
    }

    fn into_json(self) -> Value {
        let mut map = Map::new();
        for (name, child) in self.children {
            map.insert(name, child.into_json());
        }
        if !self.gaps.is_empty() {
            map.insert("_gaps".into(), Value::Array(self.gaps));
        }
        Value::Object(map)
    }
}

struct UnionPart {
    query: String,
    all: bool,
}

fn split_union(query: &str) -> gry_core::GryResult<Vec<UnionPart>> {
    let toks = tokenize(query).map_err(gry_core::GryError::unsupported)?;
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut all = false;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Punct('(') | Tok::Punct('[') | Tok::Punct('{') => depth += 1,
            Tok::Punct(')') | Tok::Punct(']') | Tok::Punct('}') => depth -= 1,
            Tok::Ident(id) if depth == 0 && id.eq_ignore_ascii_case("UNION") => {
                // Reconstruct the source slice for this part.
                let part_text = slice_tokens(query, &toks, start, i);
                parts.push(UnionPart {
                    query: part_text,
                    all,
                });
                i += 1;
                all = if i < toks.len()
                    && matches!(&toks[i], Tok::Ident(s) if s.eq_ignore_ascii_case("ALL"))
                {
                    i += 1;
                    true
                } else {
                    false
                };
                start = i;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    let part_text = slice_tokens(query, &toks, start, toks.len());
    if !part_text.trim().is_empty() {
        parts.push(UnionPart {
            query: part_text,
            all: false,
        });
    }
    if parts.is_empty() {
        return Err(gry_core::GryError::unsupported("empty query"));
    }
    Ok(parts)
}

fn slice_tokens(source: &str, toks: &[Tok], start: usize, end: usize) -> String {
    // Token positions are not retained; rebuild from token text. This loses
    // original whitespace but preserves semantics.
    let mut out = String::new();
    for tok in &toks[start.min(toks.len())..end.min(toks.len())] {
        let piece = match tok {
            Tok::Ident(s) => s.clone(),
            Tok::Str(s) => format!("'{}'", s.replace('\'', "\\'")),
            Tok::Num(n) => n.to_string(),
            Tok::Punct(c) => c.to_string(),
            Tok::Op(o) => o.clone(),
        };
        out.push_str(&piece);
        out.push(' ');
    }
    let _ = source;
    out
}

fn encode_cursor(generation: &str, query: &str, offset: usize) -> String {
    use base64::Engine;
    let payload = serde_json::json!({"v":1,"g":generation,"q":query,"o":offset});
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&payload).unwrap_or_default())
}

fn decode_cursor(cursor: &str) -> gry_core::GryResult<(String, String, usize)> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| gry_core::GryError::invalid("invalid cursor"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|_| gry_core::GryError::invalid("invalid cursor"))?;
    let generation = value
        .get("g")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| gry_core::GryError::invalid("invalid cursor"))?;
    let query = value
        .get("q")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| gry_core::GryError::invalid("invalid cursor"))?;
    let offset = value
        .get("o")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| gry_core::GryError::invalid("invalid cursor"))? as usize;
    Ok((generation.to_string(), query.to_string(), offset))
}

fn execute(
    graph: &Graph,
    query: &Query,
) -> gry_core::GryResult<(Vec<String>, Vec<(Vec<Value>, Binding)>)> {
    let mut rows: Vec<Binding> = vec![HashMap::new()];
    let mut columns = Vec::new();
    let mut final_rows: Vec<(Vec<Value>, Binding)> = Vec::new();
    for clause in &query.clauses {
        match clause {
            Clause::Match {
                optional,
                patterns,
                where_,
            } => {
                let mut next = Vec::new();
                for binding in &rows {
                    let mut matched = match_patterns(graph, patterns, binding);
                    if let Some(where_) = where_ {
                        matched.retain(|b| truthy(&eval(where_, b, graph).unwrap_or(Value::Null)));
                    }
                    if matched.is_empty() && *optional {
                        let mut b = binding.clone();
                        for pattern in patterns {
                            if let Some(var) = &pattern.start.var {
                                b.entry(var.clone()).or_insert(Value::Null);
                            }
                            for (rel, node) in &pattern.steps {
                                if let Some(var) = &rel.var {
                                    b.entry(var.clone()).or_insert(Value::Null);
                                }
                                if let Some(var) = &node.var {
                                    b.entry(var.clone()).or_insert(Value::Null);
                                }
                            }
                        }
                        next.push(b);
                    } else {
                        next.extend(matched);
                    }
                }
                rows = next;
            }
            Clause::With {
                items,
                distinct,
                where_,
            } => {
                let (cols, projected) = project_rows(&rows, items, *distinct, graph)?;
                columns = cols;
                rows = projected.into_iter().map(|(_, b)| b).collect();
                if let Some(where_) = where_ {
                    rows.retain(|b| truthy(&eval(where_, b, graph).unwrap_or(Value::Null)));
                }
            }
            Clause::Unwind { expr, alias } => {
                let mut next = Vec::new();
                for binding in &rows {
                    let value = eval(expr, binding, graph)?;
                    match value {
                        Value::Array(items) => {
                            for item in items {
                                let mut b = binding.clone();
                                b.insert(alias.clone(), item);
                                next.push(b);
                            }
                        }
                        Value::Null => {}
                        other => {
                            let mut b = binding.clone();
                            b.insert(alias.clone(), other);
                            next.push(b);
                        }
                    }
                }
                rows = next;
            }
            Clause::Return {
                items,
                distinct,
                order,
                skip,
                limit,
            } => {
                let (cols, mut projected) = project_rows(&rows, items, *distinct, graph)?;
                columns = cols;
                if !order.is_empty() {
                    projected.sort_by(|a, b| {
                        for (expr, asc) in order {
                            let va = eval(expr, &a.1, graph).unwrap_or(Value::Null);
                            let vb = eval(expr, &b.1, graph).unwrap_or(Value::Null);
                            let mut cmp = compare_values(&va, &vb);
                            if !asc {
                                cmp = cmp.reverse();
                            }
                            if cmp != std::cmp::Ordering::Equal {
                                return cmp;
                            }
                        }
                        std::cmp::Ordering::Equal
                    });
                }
                let start = skip.unwrap_or(0).min(projected.len());
                let end = limit
                    .map(|l| (start + l).min(projected.len()))
                    .unwrap_or(projected.len());
                final_rows = projected.drain(start..end).collect();
            }
        }
    }
    Ok((columns, final_rows))
}

fn project_rows(
    rows: &[Binding],
    items: &[ReturnItem],
    distinct: bool,
    graph: &Graph,
) -> gry_core::GryResult<(Vec<String>, Vec<(Vec<Value>, Binding)>)> {
    let star = items.len() == 1 && matches!(items[0].expr, Expr::Star);
    if star {
        let mut columns: Vec<String> = Vec::new();
        for b in rows {
            for k in b.keys() {
                if !columns.contains(k) {
                    columns.push(k.clone());
                }
            }
        }
        columns.sort();
        let out = rows
            .iter()
            .map(|b| {
                let values = columns
                    .iter()
                    .map(|c| b.get(c).cloned().unwrap_or(Value::Null))
                    .collect();
                (values, b.clone())
            })
            .collect();
        return Ok((columns, out));
    }
    let has_aggregate = items.iter().any(|i| contains_aggregate(&i.expr));
    let columns: Vec<String> = items.iter().map(item_name).collect();
    let mut out: Vec<(Vec<Value>, Binding)> = Vec::new();
    if has_aggregate {
        let mut groups: HashMap<String, Vec<Binding>> = HashMap::new();
        let mut order = Vec::new();
        for b in rows {
            let mut key_parts = Vec::new();
            for item in items {
                if !contains_aggregate(&item.expr) {
                    key_parts.push(eval(&item.expr, b, graph)?.to_string());
                }
            }
            let key = key_parts.join("\u{1f}");
            groups
                .entry(key.clone())
                .or_insert_with(|| {
                    order.push(key);
                    Vec::new()
                })
                .push(b.clone());
        }
        for key in order {
            let group = groups.remove(&key).unwrap_or_default();
            let representative = group.first().cloned().unwrap_or_default();
            let mut values = Vec::new();
            let mut scope = representative.clone();
            for (item, name) in items.iter().zip(columns.iter()) {
                let value = eval_aggregate(&item.expr, &group, graph)?;
                values.push(value.clone());
                scope.insert(name.clone(), value);
            }
            out.push((values, scope));
        }
    } else {
        for b in rows {
            let mut values = Vec::new();
            let mut scope = b.clone();
            for (item, name) in items.iter().zip(columns.iter()) {
                let value = eval(&item.expr, b, graph)?;
                values.push(value.clone());
                scope.insert(name.clone(), value);
            }
            out.push((values, scope));
        }
    }
    if distinct {
        let mut seen = HashSet::new();
        out.retain(|(values, _)| {
            seen.insert(
                values
                    .iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join("\u{1f}"),
            )
        });
    }
    Ok((columns, out))
}

fn item_name(item: &ReturnItem) -> String {
    item.alias.clone().unwrap_or_else(|| match &item.expr {
        Expr::Var(v) => v.clone(),
        Expr::Property(base, prop) => format!("{}.{}", expr_text(base), prop),
        Expr::Func(name, _, _) => name.clone(),
        Expr::Star => "*".to_string(),
        other => expr_text(other),
    })
}

fn expr_text(expr: &Expr) -> String {
    match expr {
        Expr::Var(v) => v.clone(),
        Expr::Property(b, p) => format!("{}.{}", expr_text(b), p),
        Expr::Literal(v) => v.to_string(),
        Expr::Func(n, _, _) => format!("{n}(...)"),
        Expr::Binary(op, a, b) => format!("{} {} {}", expr_text(a), op, expr_text(b)),
        Expr::Unary(op, e) => format!("{op} {}", expr_text(e)),
        Expr::List(_) => "[...]".to_string(),
        Expr::Case { .. } => "CASE".to_string(),
        Expr::Exists(_) => "EXISTS".to_string(),
        Expr::LabelTest(e, l) => format!("{}:{}", expr_text(e), l.join("|")),
        Expr::Star => "*".to_string(),
    }
}

fn contains_aggregate(expr: &Expr) -> bool {
    match expr {
        Expr::Func(name, args, _) => {
            matches!(
                name.as_str(),
                "count" | "sum" | "avg" | "min" | "max" | "collect"
            ) || args.iter().any(contains_aggregate)
        }
        Expr::Binary(_, a, b) => contains_aggregate(a) || contains_aggregate(b),
        Expr::Unary(_, e) => contains_aggregate(e),
        Expr::Property(e, _) => contains_aggregate(e),
        _ => false,
    }
}

fn eval_aggregate(expr: &Expr, group: &[Binding], graph: &Graph) -> gry_core::GryResult<Value> {
    match expr {
        Expr::Func(name, args, distinct) => match name.as_str() {
            "count" => {
                if args.is_empty() || matches!(args[0], Expr::Star) {
                    return Ok(json!(group.len()));
                }
                let mut values = Vec::new();
                for b in group {
                    let v = eval(&args[0], b, graph)?;
                    if !v.is_null() {
                        values.push(v);
                    }
                }
                if *distinct {
                    let set: HashSet<String> = values.iter().map(|v| v.to_string()).collect();
                    Ok(json!(set.len()))
                } else {
                    Ok(json!(values.len()))
                }
            }
            "sum" | "avg" => {
                let mut sum = 0.0;
                let mut count = 0;
                let mut seen = HashSet::new();
                for b in group {
                    let v = eval(&args[0], b, graph)?;
                    if *distinct && !seen.insert(v.to_string()) {
                        continue;
                    }
                    if let Some(n) = v.as_f64() {
                        sum += n;
                        count += 1;
                    }
                }
                if name == "avg" {
                    Ok(if count == 0 {
                        Value::Null
                    } else {
                        json!(sum / count as f64)
                    })
                } else {
                    Ok(json!(sum))
                }
            }
            "min" | "max" => {
                let mut values: Vec<Value> = group
                    .iter()
                    .filter_map(|b| eval(&args[0], b, graph).ok())
                    .filter(|v| !v.is_null())
                    .collect();
                values.sort_by(compare_values);
                if name == "min" {
                    Ok(values.into_iter().next().unwrap_or(Value::Null))
                } else {
                    Ok(values.into_iter().last().unwrap_or(Value::Null))
                }
            }
            "collect" => {
                let mut values = Vec::new();
                let mut seen = HashSet::new();
                for b in group {
                    let v = eval(&args[0], b, graph)?;
                    if *distinct && !seen.insert(v.to_string()) {
                        continue;
                    }
                    values.push(v);
                }
                Ok(Value::Array(values))
            }
            _ => eval(expr, group.first().unwrap_or(&HashMap::new()), graph),
        },
        Expr::Binary(op, a, b) => {
            let left = eval_aggregate(a, group, graph)?;
            let right = eval_aggregate(b, group, graph)?;
            Ok(binary_value(op, left, right))
        }
        Expr::Unary(op, e) => {
            let v = eval_aggregate(e, group, graph)?;
            Ok(unary_value(op, v))
        }
        other => eval(other, group.first().unwrap_or(&HashMap::new()), graph),
    }
}

fn match_patterns(graph: &Graph, patterns: &[Pattern], binding: &Binding) -> Vec<Binding> {
    let mut results = vec![binding.clone()];
    for pattern in patterns {
        let mut next = Vec::new();
        for b in &results {
            next.extend(match_pattern(graph, pattern, b));
        }
        results = next;
    }
    results
}

fn match_pattern(graph: &Graph, pattern: &Pattern, binding: &Binding) -> Vec<Binding> {
    let mut results = Vec::new();
    let start_nodes = match_node(graph, &pattern.start, binding);
    for (node, b) in start_nodes {
        expand_steps(graph, &pattern.steps, 0, node, b, &mut results);
    }
    results
}

fn match_node(graph: &Graph, pat: &NodePattern, binding: &Binding) -> Vec<(Node, Binding)> {
    if let Some(var) = &pat.var {
        if let Some(bound) = binding.get(var) {
            if let Some(id) = bound.get("__node_id").and_then(Value::as_i64) {
                if let Some(node) = graph.by_id.get(&id) {
                    if node_matches(node, pat) {
                        return vec![(node.clone(), binding.clone())];
                    }
                }
            }
            return Vec::new();
        }
    }
    let mut out = Vec::new();
    for node in &graph.nodes {
        if !node_matches(node, pat) {
            continue;
        }
        let mut b = binding.clone();
        if let Some(var) = &pat.var {
            b.insert(var.clone(), node_value(node));
        }
        out.push((node.clone(), b));
    }
    out
}

fn node_matches(node: &Node, pat: &NodePattern) -> bool {
    if !pat.labels.is_empty() && !pat.labels.iter().any(|l| l == &node.label) {
        return false;
    }
    for (key, expected) in &pat.props {
        let actual = node
            .properties
            .get(key)
            .cloned()
            .unwrap_or_else(|| match key.as_str() {
                "name" => Value::String(node.name.clone()),
                "label" => Value::String(node.label.clone()),
                "qualified_name" | "qn" => Value::String(node.qualified_name.clone()),
                "file_path" | "file" => Value::String(node.file_path.clone()),
                _ => Value::Null,
            });
        if &actual != expected {
            return false;
        }
    }
    true
}

fn expand_steps(
    graph: &Graph,
    steps: &[(RelPattern, NodePattern)],
    index: usize,
    current: Node,
    binding: Binding,
    results: &mut Vec<Binding>,
) {
    if index >= steps.len() {
        results.push(binding);
        return;
    }
    let (rel, node_pat) = &steps[index];
    let paths = expand_rel(graph, rel, current.id, &binding);
    for (end_id, edges, rel_binding) in paths {
        let Some(end_node) = graph.by_id.get(&end_id) else {
            continue;
        };
        if !node_matches(end_node, node_pat) {
            continue;
        }
        if let Some(var) = &node_pat.var {
            if let Some(bound) = binding.get(var) {
                if bound.get("__node_id").and_then(Value::as_i64) != Some(end_id) {
                    continue;
                }
            }
        }
        let mut b = binding.clone();
        if let Some(var) = &rel.var {
            let value = if edges.len() == 1 {
                edge_value(&edges[0])
            } else {
                Value::Array(edges.iter().map(edge_value).collect())
            };
            b.insert(var.clone(), value);
        }
        if let Some(var) = &node_pat.var {
            b.insert(var.clone(), node_value(end_node));
        }
        let _ = &rel_binding;
        expand_steps(graph, steps, index + 1, end_node.clone(), b, results);
    }
}

fn expand_rel(
    graph: &Graph,
    rel: &RelPattern,
    start: i64,
    _binding: &Binding,
) -> Vec<(i64, Vec<Edge>, ())> {
    let mut out = Vec::new();
    if rel.max <= 1 {
        let candidates: Vec<usize> = match rel.direction {
            RelDirection::Out => graph.out.get(&start).cloned().unwrap_or_default(),
            RelDirection::In => graph.inc.get(&start).cloned().unwrap_or_default(),
            RelDirection::Both => {
                let mut v = graph.out.get(&start).cloned().unwrap_or_default();
                v.extend(graph.inc.get(&start).cloned().unwrap_or_default());
                v
            }
        };
        for idx in candidates {
            let edge = &graph.edges[idx];
            if !rel.types.is_empty() && !rel.types.iter().any(|t| t == &edge.edge_type) {
                continue;
            }
            let other = match rel.direction {
                RelDirection::Out => edge.target_id,
                RelDirection::In => edge.source_id,
                RelDirection::Both => {
                    if edge.source_id == start {
                        edge.target_id
                    } else {
                        edge.source_id
                    }
                }
            };
            out.push((other, vec![edge.clone()], ()));
        }
        return out;
    }
    // Variable-length paths: BFS with simple-path semantics.
    let mut queue: std::collections::VecDeque<(i64, Vec<Edge>, HashSet<i64>)> =
        std::collections::VecDeque::new();
    let mut seen = HashSet::new();
    seen.insert(start);
    queue.push_back((start, Vec::new(), seen));
    while let Some((current, path, visited)) = queue.pop_front() {
        if path.len() >= rel.max {
            continue;
        }
        let candidates: Vec<usize> = match rel.direction {
            RelDirection::Out => graph.out.get(&current).cloned().unwrap_or_default(),
            RelDirection::In => graph.inc.get(&current).cloned().unwrap_or_default(),
            RelDirection::Both => {
                let mut v = graph.out.get(&current).cloned().unwrap_or_default();
                v.extend(graph.inc.get(&current).cloned().unwrap_or_default());
                v
            }
        };
        for idx in candidates {
            let edge = &graph.edges[idx];
            if !rel.types.is_empty() && !rel.types.iter().any(|t| t == &edge.edge_type) {
                continue;
            }
            let other = match rel.direction {
                RelDirection::Out => edge.target_id,
                RelDirection::In => edge.source_id,
                RelDirection::Both => {
                    if edge.source_id == current {
                        edge.target_id
                    } else {
                        edge.source_id
                    }
                }
            };
            if visited.contains(&other) {
                continue;
            }
            let mut new_path = path.clone();
            new_path.push(edge.clone());
            if new_path.len() >= rel.min {
                out.push((other, new_path.clone(), ()));
            }
            let mut new_visited = visited.clone();
            new_visited.insert(other);
            queue.push_back((other, new_path, new_visited));
        }
    }
    out
}

fn eval(expr: &Expr, binding: &Binding, graph: &Graph) -> gry_core::GryResult<Value> {
    Ok(match expr {
        Expr::Var(v) => binding.get(v).cloned().unwrap_or(Value::Null),
        Expr::Literal(v) => v.clone(),
        Expr::List(items) => Value::Array(
            items
                .iter()
                .map(|e| eval(e, binding, graph).unwrap_or(Value::Null))
                .collect(),
        ),
        Expr::Property(base, prop) => {
            let value = eval(base, binding, graph)?;
            match value {
                Value::Object(map) => map.get(prop).cloned().unwrap_or(Value::Null),
                _ => Value::Null,
            }
        }
        Expr::Binary(op, a, b) => {
            if op == "AND" {
                return Ok(Value::Bool(
                    truthy(&eval(a, binding, graph)?) && truthy(&eval(b, binding, graph)?),
                ));
            }
            if op == "OR" {
                return Ok(Value::Bool(
                    truthy(&eval(a, binding, graph)?) || truthy(&eval(b, binding, graph)?),
                ));
            }
            if op == "XOR" {
                return Ok(Value::Bool(
                    truthy(&eval(a, binding, graph)?) ^ truthy(&eval(b, binding, graph)?),
                ));
            }
            let left = eval(a, binding, graph)?;
            let right = eval(b, binding, graph)?;
            binary_value(op, left, right)
        }
        Expr::Unary(op, e) => {
            let value = eval(e, binding, graph)?;
            unary_value(op, value)
        }
        Expr::Func(name, args, distinct) => eval_func(name, args, *distinct, binding, graph)?,
        Expr::Case {
            operand,
            whens,
            otherwise,
        } => {
            let operand_value = operand
                .as_ref()
                .map(|e| eval(e, binding, graph))
                .transpose()?;
            for (when, then) in whens {
                let cond = eval(when, binding, graph)?;
                let matched = if let Some(operand_value) = &operand_value {
                    compare_values(operand_value, &cond) == std::cmp::Ordering::Equal
                } else {
                    truthy(&cond)
                };
                if matched {
                    return eval(then, binding, graph);
                }
            }
            match otherwise {
                Some(e) => eval(e, binding, graph)?,
                None => Value::Null,
            }
        }
        Expr::Exists(pattern) => {
            let matched = match_pattern(graph, pattern, binding);
            Value::Bool(!matched.is_empty())
        }
        Expr::LabelTest(e, labels) => {
            let value = eval(e, binding, graph)?;
            let label = value.get("label").and_then(Value::as_str).unwrap_or("");
            Value::Bool(labels.iter().any(|l| l == label))
        }
        Expr::Star => Value::Null,
    })
}

fn eval_func(
    name: &str,
    args: &[Expr],
    distinct: bool,
    binding: &Binding,
    graph: &Graph,
) -> gry_core::GryResult<Value> {
    if matches!(name, "count" | "sum" | "avg" | "min" | "max" | "collect") {
        // Aggregates outside a grouping context operate on one row.
        return eval_aggregate(
            &Expr::Func(name.to_string(), args.to_vec(), distinct),
            &[binding.clone()],
            graph,
        );
    }
    let values: Vec<Value> = args
        .iter()
        .map(|a| eval(a, binding, graph).unwrap_or(Value::Null))
        .collect();
    Ok(match name {
        "tolower" => Value::String(
            values
                .first()
                .map(as_string)
                .unwrap_or_default()
                .to_lowercase(),
        ),
        "toupper" => Value::String(
            values
                .first()
                .map(as_string)
                .unwrap_or_default()
                .to_uppercase(),
        ),
        "tostring" => Value::String(values.first().map(as_string).unwrap_or_default()),
        "tointeger" => values
            .first()
            .and_then(|v| v.as_f64().or_else(|| as_string(v).parse().ok()))
            .map(|n| json!(n as i64))
            .unwrap_or(Value::Null),
        "tofloat" => values
            .first()
            .and_then(|v| v.as_f64().or_else(|| as_string(v).parse().ok()))
            .map(|n| json!(n))
            .unwrap_or(Value::Null),
        "toboolean" => values
            .first()
            .map(|v| Value::Bool(truthy(v)))
            .unwrap_or(Value::Null),
        "labels" => {
            let label = values
                .first()
                .and_then(|v| v.get("label"))
                .and_then(Value::as_str);
            label
                .map(|l| json!([l]))
                .unwrap_or(Value::Array(Vec::new()))
        }
        "type" => values
            .first()
            .and_then(|v| v.get("__type"))
            .cloned()
            .unwrap_or(Value::Null),
        "id" => values
            .first()
            .and_then(|v| v.get("__node_id").or_else(|| v.get("__edge_id")))
            .cloned()
            .unwrap_or(Value::Null),
        "properties" => {
            let mut map = Map::new();
            if let Some(Value::Object(obj)) = values.first() {
                for (k, v) in obj {
                    if !k.starts_with("__") {
                        map.insert(k.clone(), v.clone());
                    }
                }
            }
            Value::Object(map)
        }
        "keys" => {
            let mut keys: Vec<Value> = values
                .first()
                .and_then(Value::as_object)
                .map(|obj| {
                    obj.keys()
                        .filter(|k| !k.starts_with("__"))
                        .map(|k| Value::String(k.clone()))
                        .collect()
                })
                .unwrap_or_default();
            keys.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            Value::Array(keys)
        }
        "size" | "length" => {
            let n = match values.first() {
                Some(Value::Array(items)) => items.len(),
                Some(Value::String(s)) => s.chars().count(),
                _ => 0,
            };
            json!(n)
        }
        "trim" => Value::String(
            as_string(values.first().unwrap_or(&Value::Null))
                .trim()
                .to_string(),
        ),
        "ltrim" => Value::String(
            as_string(values.first().unwrap_or(&Value::Null))
                .trim_start()
                .to_string(),
        ),
        "rtrim" => Value::String(
            as_string(values.first().unwrap_or(&Value::Null))
                .trim_end()
                .to_string(),
        ),
        "reverse" => {
            let s = as_string(values.first().unwrap_or(&Value::Null));
            Value::String(s.chars().rev().collect())
        }
        "coalesce" => values
            .into_iter()
            .find(|v| !v.is_null())
            .unwrap_or(Value::Null),
        "substring" => {
            let s = as_string(values.first().unwrap_or(&Value::Null));
            let start = values.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
            let take = values.get(2).and_then(Value::as_u64).map(|n| n as usize);
            let chars: Vec<char> = s.chars().collect();
            let end = take
                .map(|n| (start + n).min(chars.len()))
                .unwrap_or(chars.len());
            Value::String(
                chars
                    .get(start.min(chars.len())..end)
                    .unwrap_or(&[])
                    .iter()
                    .collect(),
            )
        }
        "replace" => {
            let s = as_string(values.first().unwrap_or(&Value::Null));
            let from = as_string(values.get(1).unwrap_or(&Value::Null));
            let to = as_string(values.get(2).unwrap_or(&Value::Null));
            Value::String(s.replace(&from, &to))
        }
        "left" => {
            let s = as_string(values.first().unwrap_or(&Value::Null));
            let n = values.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
            Value::String(s.chars().take(n).collect())
        }
        "right" => {
            let s = as_string(values.first().unwrap_or(&Value::Null));
            let n = values.get(1).and_then(Value::as_u64).unwrap_or(0) as usize;
            let chars: Vec<char> = s.chars().collect();
            let start = chars.len().saturating_sub(n);
            Value::String(chars[start..].iter().collect())
        }
        other => {
            return Err(gry_core::GryError::unsupported(format!(
                "unsupported function {other}"
            )))
        }
    })
}

fn as_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(_) => true,
    }
}

fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x
            .as_f64()
            .partial_cmp(&y.as_f64())
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(x), Value::String(y)) => x.cmp(y),
        (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
        _ => a.to_string().cmp(&b.to_string()),
    }
}

fn binary_value(op: &str, left: Value, right: Value) -> Value {
    match op {
        "=" => Value::Bool(compare_values(&left, &right) == std::cmp::Ordering::Equal),
        "<>" => Value::Bool(compare_values(&left, &right) != std::cmp::Ordering::Equal),
        "<" => Value::Bool(compare_values(&left, &right) == std::cmp::Ordering::Less),
        "<=" => Value::Bool(compare_values(&left, &right) != std::cmp::Ordering::Greater),
        ">" => Value::Bool(compare_values(&left, &right) == std::cmp::Ordering::Greater),
        ">=" => Value::Bool(compare_values(&left, &right) != std::cmp::Ordering::Less),
        "IN" => match right {
            Value::Array(items) => Value::Bool(
                items
                    .iter()
                    .any(|v| compare_values(v, &left) == std::cmp::Ordering::Equal),
            ),
            _ => Value::Bool(false),
        },
        "CONTAINS" => Value::Bool(as_string(&left).contains(&as_string(&right))),
        "STARTS WITH" => Value::Bool(as_string(&left).starts_with(&as_string(&right))),
        "ENDS WITH" => Value::Bool(as_string(&left).ends_with(&as_string(&right))),
        "=~" => {
            let pattern = as_string(&right);
            match Regex::new(&pattern) {
                Ok(re) => Value::Bool(re.is_match(&as_string(&left))),
                Err(_) => Value::Bool(false),
            }
        }
        "+" => match (&left, &right) {
            (Value::Number(a), Value::Number(b)) => {
                json!(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0))
            }
            (Value::String(a), Value::String(b)) => Value::String(format!("{a}{b}")),
            _ => Value::Null,
        },
        "-" => match (&left, &right) {
            (Value::Number(a), Value::Number(b)) => {
                json!(a.as_f64().unwrap_or(0.0) - b.as_f64().unwrap_or(0.0))
            }
            _ => Value::Null,
        },
        "*" => match (&left, &right) {
            (Value::Number(a), Value::Number(b)) => {
                json!(a.as_f64().unwrap_or(0.0) * b.as_f64().unwrap_or(0.0))
            }
            _ => Value::Null,
        },
        "/" => match (&left, &right) {
            (Value::Number(a), Value::Number(b)) if b.as_f64().unwrap_or(0.0) != 0.0 => {
                json!(a.as_f64().unwrap_or(0.0) / b.as_f64().unwrap_or(1.0))
            }
            _ => Value::Null,
        },
        "%" => match (&left, &right) {
            (Value::Number(a), Value::Number(b)) if b.as_f64().unwrap_or(0.0) != 0.0 => {
                json!(a.as_f64().unwrap_or(0.0) % b.as_f64().unwrap_or(1.0))
            }
            _ => Value::Null,
        },
        _ => Value::Null,
    }
}

fn unary_value(op: &str, value: Value) -> Value {
    match op {
        "NOT" => Value::Bool(!truthy(&value)),
        "-" => value.as_f64().map(|n| json!(-n)).unwrap_or(Value::Null),
        "IS NULL" => Value::Bool(value.is_null()),
        "IS NOT NULL" => Value::Bool(!value.is_null()),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gry_store::test_support::test_store;

    fn run(store: &Store, query: &str) -> Value {
        query_graph(
            store,
            &QueryRequest {
                project: "p".into(),
                query: query.into(),
                max_rows: 200,
                offset: 0,
                cursor: None,
                graph: "code".into(),
            },
        )
        .unwrap()
    }

    #[test]
    fn basic_match_return() {
        let store = test_store();
        let out = run(
            &store,
            "MATCH (f:Function) RETURN f.name ORDER BY f.name LIMIT 2",
        );
        assert_eq!(out["rows"].as_array().unwrap().len(), 2);
        assert_eq!(out["columns"][0], "f.name");
    }

    #[test]
    fn call_traversal_and_where() {
        let store = test_store();
        let out = run(
            &store,
            "MATCH (f:Function)-[:CALLS]->(g) WHERE f.name = 'main' RETURN g.name ORDER BY g.name",
        );
        let rows = out["rows"].as_array().unwrap();
        assert!(rows.iter().any(|r| r[0] == "parseRequest"));
        assert!(rows.iter().any(|r| r[0] == "saveOrder"));
    }

    #[test]
    fn aggregates_and_exists() {
        let store = test_store();
        let out = run(&store, "MATCH (f:Function) RETURN count(f)");
        assert_eq!(out["rows"][0][0], 5);
        let out = run(
            &store,
            "MATCH (f:Function) WHERE NOT EXISTS { (f)<-[:CALLS]-() } RETURN f.name ORDER BY f.name",
        );
        let rows = out["rows"].as_array().unwrap();
        assert!(rows.iter().any(|r| r[0] == "entryHandler"));
    }

    #[test]
    fn variable_length_and_optional() {
        let store = test_store();
        let out = run(
            &store,
            "MATCH (f:Function {name:'test_main'})-[:CALLS*1..3]->(g) RETURN g.name ORDER BY g.name",
        );
        assert!(out["rows"].as_array().unwrap().len() >= 3);

        let out = run(
            &store,
            "MATCH (f:Function {name:'parseRequest'}) OPTIONAL MATCH (f)-[:CALLS]->(g) RETURN f.name, g.name",
        );
        assert_eq!(out["rows"].as_array().unwrap().len(), 1);
        assert_eq!(out["rows"][0][1], Value::Null);
    }

    #[test]
    fn unsupported_fails_explicitly() {
        let store = test_store();
        let err = query_graph(
            &store,
            &QueryRequest {
                project: "p".into(),
                query: "MERGE (f:Function) RETURN f".into(),
                max_rows: 10,
                offset: 0,
                cursor: None,
                graph: "code".into(),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }
}
