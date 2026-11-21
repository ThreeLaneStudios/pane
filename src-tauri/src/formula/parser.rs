// Pane — Formula Engine Parser (#2339 PR 6, §33.5.1–§33.5.2)
//
// Hand-rolled tokenizer + recursive-descent parser for the §33.5.1 v1
// grammar. No parser-combinator crate, no Pratt-parser machinery — the
// grammar's precedence structure (unary minus, `*`/`/`, `+`/`-`,
// comparisons, in that binding order) is shallow enough that a textbook
// `parse_comparison -> parse_add_sub -> parse_mul_div -> parse_unary ->
// parse_primary` descent covers it completely, matching the spec's own
// framing ("Hand-rolled parser/evaluator. No crate with expression-
// language scope creep.").
//
// Reference forms (§33.5.2): the tokenizer recognizes both A1-style
// display references (`A1`, `B12`) and canonical stable-ID references
// (`{column_uuid:row_uuid}`, or `{column_uuid:from_row_uuid:to_row_uuid}`
// for an ID-form range). Both produce the SAME `AstNode::CellRef`/
// `AstNode::Range` shape — carrying whatever `(column_id, row_id)` (or
// `(start_col, start_row, end_col, end_row)`) strings the source syntax
// yielded. For A1-form input this means `column_id`/`row_id` are the
// literal column-letter/row-number text (e.g. `("A", "1")`), NOT a
// resolved database `column_id`/`row_id` pair — translating A1 text
// against a live grid is PR 7's `ReferenceMapper` concern (see
// `mod.rs`'s module comment). PR 6's evaluator (`eval.rs`) is agnostic to
// which form produced a given `AstNode` — it just calls `CellResolver`
// with whatever strings are on the node.

use crate::formula::eval::FormulaError;

// ---------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum AstNode {
    Number(f64),
    Text(String),
    Boolean(bool),
    /// A single cell reference. `column_id`/`row_id` are either A1-style
    /// literal text (`"A"`/`"1"`) or resolved stable-ID text, depending on
    /// which syntax the source formula used — see the module doc comment.
    CellRef { column_id: String, row_id: String },
    /// A range reference. Mirrors §33.5.2's storage shape
    /// `(column_id, from_row_id, to_row_id)` generalized to
    /// `start_col`/`end_col` so the same node type covers both A1-form
    /// (`A1:A5`, where `start_col == end_col` in practice) and any future
    /// cross-column range syntax without a second node type.
    Range {
        start_col: String,
        start_row: String,
        end_col: String,
        end_row: String,
    },
    UnaryMinus(Box<AstNode>),
    BinaryOp(Box<AstNode>, BinOp, Box<AstNode>),
    Compare(Box<AstNode>, CmpOp, Box<AstNode>),
    FnCall(FnName, Vec<AstNode>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FnName {
    Sum,
    Average,
    If,
    /// Any identifier followed by `(...)` that isn't a known function.
    /// Deliberately NOT a parse error — §33.5.3's `#NAME?` is an
    /// evaluation-time error, so the parser accepts the call shape and
    /// lets `eval.rs` produce `#NAME?` when it walks this node.
    Unknown(String),
}

// ---------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Text(String),
    Ident(String),
    IdRef {
        column_id: String,
        row_id: String,
    },
    IdRangeRef {
        column_id: String,
        start_row: String,
        end_row: String,
    },
    Plus,
    Minus,
    Star,
    Slash,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    LParen,
    RParen,
    Comma,
    Colon,
    Eof,
}

fn tokenize(input: &str) -> Result<Vec<Token>, FormulaError> {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut tokens = Vec::new();

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        match c {
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
            }
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            ':' => {
                tokens.push(Token::Colon);
                i += 1;
            }
            '=' => {
                tokens.push(Token::Eq);
                i += 1;
            }
            '!' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    tokens.push(Token::Ne);
                    i += 2;
                } else {
                    return Err(FormulaError::Parse(format!(
                        "unexpected character '!' at position {i} (did you mean '!='?)"
                    )));
                }
            }
            '<' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    tokens.push(Token::Le);
                    i += 2;
                } else {
                    tokens.push(Token::Lt);
                    i += 1;
                }
            }
            '>' => {
                if i + 1 < chars.len() && chars[i + 1] == '=' {
                    tokens.push(Token::Ge);
                    i += 2;
                } else {
                    tokens.push(Token::Gt);
                    i += 1;
                }
            }
            '"' => {
                let mut s = String::new();
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    s.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    return Err(FormulaError::Parse("unterminated string literal".to_string()));
                }
                tokens.push(Token::Text(s));
            }
            '{' => {
                let mut content = String::new();
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '}' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    content.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    return Err(FormulaError::Parse(
                        "unterminated stable-id reference (missing '}')".to_string(),
                    ));
                }
                let parts: Vec<&str> = content.split(':').map(|p| p.trim()).collect();
                match parts.as_slice() {
                    [col, row] if !col.is_empty() && !row.is_empty() => {
                        tokens.push(Token::IdRef {
                            column_id: col.to_string(),
                            row_id: row.to_string(),
                        });
                    }
                    [col, row1, row2] if !col.is_empty() && !row1.is_empty() && !row2.is_empty() => {
                        tokens.push(Token::IdRangeRef {
                            column_id: col.to_string(),
                            start_row: row1.to_string(),
                            end_row: row2.to_string(),
                        });
                    }
                    _ => {
                        return Err(FormulaError::Parse(format!(
                            "malformed stable-id reference '{{{content}}}'"
                        )));
                    }
                }
            }
            c if c.is_ascii_digit() => {
                let start = i;
                let mut seen_dot = false;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                    if chars[i] == '.' {
                        if seen_dot {
                            break;
                        }
                        seen_dot = true;
                    }
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                let n: f64 = text
                    .parse()
                    .map_err(|_| FormulaError::Parse(format!("invalid number literal '{text}'")))?;
                tokens.push(Token::Number(n));
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                tokens.push(Token::Ident(text));
            }
            other => {
                return Err(FormulaError::Parse(format!(
                    "unexpected character '{other}' at position {i}"
                )));
            }
        }
    }
    tokens.push(Token::Eof);
    Ok(tokens)
}

/// Splits an identifier into `(column_letters, row_digits)` if it matches
/// the A1-style shape (one-or-more letters followed by one-or-more
/// digits, nothing else). Returns `None` for anything else (function
/// names, `TRUE`/`FALSE`, malformed idents).
fn split_a1_ref(text: &str) -> Option<(String, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut idx = 0;
    while idx < chars.len() && chars[idx].is_ascii_alphabetic() {
        idx += 1;
    }
    if idx == 0 || idx == chars.len() {
        return None;
    }
    let rest = &chars[idx..];
    if !rest.iter().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let col: String = chars[..idx].iter().collect::<String>().to_uppercase();
    let row: String = rest.iter().collect();
    Some((col, row))
}

fn resolve_fn_name(name: &str) -> FnName {
    let upper = name.to_uppercase();
    match upper.as_str() {
        "SUM" => FnName::Sum,
        "AVERAGE" => FnName::Average,
        "IF" => FnName::If,
        _ => FnName::Unknown(upper),
    }
}

// ---------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------

/// Maximum recursive-descent nesting depth for parenthesized expressions,
/// function-call arguments, and unary-minus chains (#2339 PR 7, PR 6 audit
/// FAIL-tier finding, `docs/audits/2026-09-03-sheets-pr6-formula-audit.md`
/// §6). Pre-guard, ~150-200 levels of nested `(` caused a real stack-
/// overflow process abort (`SIGABRT`), not a graceful `#PARSE!` — a
/// pathological formula (bulk paste, malicious sync payload) could crash
/// the whole Tauri process. 128 is comfortably below that empirical crash
/// threshold while allowing any realistic hand-written or generated
/// formula.
const MAX_PARSE_DEPTH: usize = 128;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0, depth: 0 }
    }

    /// Increment the recursion-depth counter, failing with a descriptive
    /// `FormulaError::Parse` (never a stack overflow) if the ceiling is
    /// exceeded. Called at every point the recursive descent can re-enter
    /// itself: `parse_expr` (parens, function-call args) and `parse_unary`
    /// (chained unary-minus, e.g. `--------5`).
    fn enter_depth(&mut self) -> Result<(), FormulaError> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            return Err(FormulaError::Parse(format!(
                "Formula exceeds maximum nesting depth ({MAX_PARSE_DEPTH})"
            )));
        }
        Ok(())
    }

    fn exit_depth(&mut self) {
        self.depth -= 1;
    }

    fn peek(&self) -> Token {
        self.tokens[self.pos].clone()
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, expected: Token) -> Result<(), FormulaError> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(FormulaError::Parse(format!(
                "expected {expected:?}, found {:?}",
                self.peek()
            )))
        }
    }

    fn expect_eof(&self) -> Result<(), FormulaError> {
        if self.peek() == Token::Eof {
            Ok(())
        } else {
            Err(FormulaError::Parse(format!(
                "unexpected trailing input: {:?}",
                self.peek()
            )))
        }
    }

    fn expect_a1_ref(&mut self) -> Result<(String, String), FormulaError> {
        match self.peek() {
            Token::Ident(name) => {
                if let Some(pair) = split_a1_ref(&name) {
                    self.advance();
                    Ok(pair)
                } else {
                    Err(FormulaError::Parse(format!(
                        "expected a cell reference after ':', found '{name}'"
                    )))
                }
            }
            other => Err(FormulaError::Parse(format!(
                "expected a cell reference after ':', found {other:?}"
            ))),
        }
    }

    /// Mirrors `expect_a1_ref` but for the ID-form `{col_id:row_id}`
    /// cell-ref token — used by the new two-cell-ref range syntax
    /// (`{col1:row1}:{col2:row2}`, see parser.rs module doc / item 37
    /// scope) to consume the second endpoint after a `Colon`.
    fn expect_id_ref(&mut self) -> Result<(String, String), FormulaError> {
        match self.peek() {
            Token::IdRef { column_id, row_id } => {
                self.advance();
                Ok((column_id, row_id))
            }
            other => Err(FormulaError::Parse(format!(
                "expected a {{col:row}} cell reference after ':', found {other:?}"
            ))),
        }
    }

    /// Entry point: comparisons bind loosest, matching the spec's
    /// operator-precedence ordering (unary minus highest, then `*`/`/`,
    /// then `+`/`-`, then comparisons lowest).
    fn parse_expr(&mut self) -> Result<AstNode, FormulaError> {
        self.enter_depth()?;
        let result = self.parse_comparison();
        self.exit_depth();
        result
    }

    fn parse_comparison(&mut self) -> Result<AstNode, FormulaError> {
        let mut lhs = self.parse_add_sub()?;
        loop {
            let op = match self.peek() {
                Token::Eq => CmpOp::Eq,
                Token::Ne => CmpOp::Ne,
                Token::Lt => CmpOp::Lt,
                Token::Gt => CmpOp::Gt,
                Token::Le => CmpOp::Le,
                Token::Ge => CmpOp::Ge,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_add_sub()?;
            lhs = AstNode::Compare(Box::new(lhs), op, Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_add_sub(&mut self) -> Result<AstNode, FormulaError> {
        let mut lhs = self.parse_mul_div()?;
        loop {
            match self.peek() {
                Token::Plus => {
                    self.advance();
                    let rhs = self.parse_mul_div()?;
                    lhs = AstNode::BinaryOp(Box::new(lhs), BinOp::Add, Box::new(rhs));
                }
                Token::Minus => {
                    self.advance();
                    let rhs = self.parse_mul_div()?;
                    lhs = AstNode::BinaryOp(Box::new(lhs), BinOp::Sub, Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn parse_mul_div(&mut self) -> Result<AstNode, FormulaError> {
        let mut lhs = self.parse_unary()?;
        loop {
            match self.peek() {
                Token::Star => {
                    self.advance();
                    let rhs = self.parse_unary()?;
                    lhs = AstNode::BinaryOp(Box::new(lhs), BinOp::Mul, Box::new(rhs));
                }
                Token::Slash => {
                    self.advance();
                    let rhs = self.parse_unary()?;
                    lhs = AstNode::BinaryOp(Box::new(lhs), BinOp::Div, Box::new(rhs));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    /// Unary minus: highest-precedence prefix operator (Matt ruling
    /// 2026-09-03). Recurses on itself so `--5` (double negative) parses
    /// as nested `UnaryMinus(UnaryMinus(Number(5)))`.
    fn parse_unary(&mut self) -> Result<AstNode, FormulaError> {
        if self.peek() == Token::Minus {
            self.advance();
            self.enter_depth()?;
            let inner = self.parse_unary();
            self.exit_depth();
            return Ok(AstNode::UnaryMinus(Box::new(inner?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<AstNode, FormulaError> {
        match self.peek() {
            Token::Number(n) => {
                self.advance();
                Ok(AstNode::Number(n))
            }
            Token::Text(s) => {
                self.advance();
                Ok(AstNode::Text(s))
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(expr)
            }
            Token::IdRef { column_id, row_id } => {
                self.advance();
                if self.peek() == Token::Colon {
                    self.advance();
                    let (end_col, end_row) = self.expect_id_ref()?;
                    return Ok(AstNode::Range {
                        start_col: column_id,
                        start_row: row_id,
                        end_col,
                        end_row,
                    });
                }
                Ok(AstNode::CellRef { column_id, row_id })
            }
            Token::IdRangeRef {
                column_id,
                start_row,
                end_row,
            } => {
                self.advance();
                Ok(AstNode::Range {
                    start_col: column_id.clone(),
                    start_row,
                    end_col: column_id,
                    end_row,
                })
            }
            Token::Ident(name) => {
                self.advance();
                if name.eq_ignore_ascii_case("TRUE") {
                    return Ok(AstNode::Boolean(true));
                }
                if name.eq_ignore_ascii_case("FALSE") {
                    return Ok(AstNode::Boolean(false));
                }
                if self.peek() == Token::LParen {
                    self.advance();
                    let args = self.parse_args()?;
                    self.expect(Token::RParen)?;
                    return Ok(AstNode::FnCall(resolve_fn_name(&name), args));
                }
                if let Some((col, row)) = split_a1_ref(&name) {
                    if self.peek() == Token::Colon {
                        self.advance();
                        let (end_col, end_row) = self.expect_a1_ref()?;
                        return Ok(AstNode::Range {
                            start_col: col,
                            start_row: row,
                            end_col,
                            end_row,
                        });
                    }
                    return Ok(AstNode::CellRef {
                        column_id: col,
                        row_id: row,
                    });
                }
                Err(FormulaError::Parse(format!("unknown identifier '{name}'")))
            }
            other => Err(FormulaError::Parse(format!("unexpected token {other:?}"))),
        }
    }

    fn parse_args(&mut self) -> Result<Vec<AstNode>, FormulaError> {
        let mut args = Vec::new();
        if self.peek() == Token::RParen {
            return Ok(args);
        }
        args.push(self.parse_expr()?);
        while self.peek() == Token::Comma {
            self.advance();
            args.push(self.parse_expr()?);
        }
        Ok(args)
    }
}

/// Parse a formula string into an `AstNode`. Accepts an optional leading
/// `=` (§33.5.1's formula prefix marker) — callers may pass either the
/// full `=`-prefixed cell value or a bare expression (convenient for
/// tests and any future non-cell use of the parser).
///
/// Never panics on malformed input — every failure path returns
/// `FormulaError::Parse` with a descriptive message.
pub fn parse_formula(input: &str) -> Result<AstNode, FormulaError> {
    let trimmed = input.trim();
    let body = trimmed.strip_prefix('=').unwrap_or(trimmed);
    if body.trim().is_empty() {
        return Err(FormulaError::Parse("empty formula".to_string()));
    }
    let tokens = tokenize(body)?;
    let mut parser = Parser::new(tokens);
    let ast = parser.parse_expr()?;
    parser.expect_eof()?;
    Ok(ast)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &str) -> AstNode {
        parse_formula(input).unwrap_or_else(|e| panic!("expected '{input}' to parse, got {e:?}"))
    }

    fn parse_err(input: &str) -> FormulaError {
        match parse_formula(input) {
            Ok(ast) => panic!("expected '{input}' to fail to parse, got {ast:?}"),
            Err(e) => e,
        }
    }

    // ---- literals ----

    #[test]
    fn test_number_literal_integer() {
        assert_eq!(parse("42"), AstNode::Number(42.0));
    }

    #[test]
    fn test_number_literal_decimal() {
        assert_eq!(parse("3.14"), AstNode::Number(3.14));
    }

    #[test]
    fn test_string_literal() {
        assert_eq!(parse("\"hello world\""), AstNode::Text("hello world".to_string()));
    }

    #[test]
    fn test_boolean_literal_true() {
        assert_eq!(parse("TRUE"), AstNode::Boolean(true));
    }

    #[test]
    fn test_boolean_literal_false() {
        assert_eq!(parse("FALSE"), AstNode::Boolean(false));
    }

    #[test]
    fn test_boolean_literal_case_insensitive() {
        assert_eq!(parse("true"), AstNode::Boolean(true));
        assert_eq!(parse("False"), AstNode::Boolean(false));
    }

    // ---- formula prefix ----

    #[test]
    fn test_leading_equals_prefix_stripped() {
        assert_eq!(parse("=1+2"), parse("1+2"));
    }

    // ---- operator precedence ----

    #[test]
    fn test_precedence_mul_before_add() {
        // 2+3*4 -> Add(2, Mul(3,4))
        let ast = parse("2+3*4");
        assert_eq!(
            ast,
            AstNode::BinaryOp(
                Box::new(AstNode::Number(2.0)),
                BinOp::Add,
                Box::new(AstNode::BinaryOp(
                    Box::new(AstNode::Number(3.0)),
                    BinOp::Mul,
                    Box::new(AstNode::Number(4.0))
                ))
            )
        );
    }

    #[test]
    fn test_precedence_div_before_sub() {
        let ast = parse("10-8/2");
        assert_eq!(
            ast,
            AstNode::BinaryOp(
                Box::new(AstNode::Number(10.0)),
                BinOp::Sub,
                Box::new(AstNode::BinaryOp(
                    Box::new(AstNode::Number(8.0)),
                    BinOp::Div,
                    Box::new(AstNode::Number(2.0))
                ))
            )
        );
    }

    #[test]
    fn test_precedence_parens_override() {
        let ast = parse("(2+3)*4");
        assert_eq!(
            ast,
            AstNode::BinaryOp(
                Box::new(AstNode::BinaryOp(
                    Box::new(AstNode::Number(2.0)),
                    BinOp::Add,
                    Box::new(AstNode::Number(3.0))
                )),
                BinOp::Mul,
                Box::new(AstNode::Number(4.0))
            )
        );
    }

    #[test]
    fn test_precedence_left_associative_sub() {
        // 10-3-2 should be (10-3)-2, not 10-(3-2)
        let ast = parse("10-3-2");
        assert_eq!(
            ast,
            AstNode::BinaryOp(
                Box::new(AstNode::BinaryOp(
                    Box::new(AstNode::Number(10.0)),
                    BinOp::Sub,
                    Box::new(AstNode::Number(3.0))
                )),
                BinOp::Sub,
                Box::new(AstNode::Number(2.0))
            )
        );
    }

    #[test]
    fn test_precedence_comparisons_lowest() {
        // 1+2>3 -> Compare(Add(1,2), Gt, 3)
        let ast = parse("1+2>3");
        assert_eq!(
            ast,
            AstNode::Compare(
                Box::new(AstNode::BinaryOp(
                    Box::new(AstNode::Number(1.0)),
                    BinOp::Add,
                    Box::new(AstNode::Number(2.0))
                )),
                CmpOp::Gt,
                Box::new(AstNode::Number(3.0))
            )
        );
    }

    #[test]
    fn test_all_comparison_operators_parse() {
        assert!(matches!(parse("1=2"), AstNode::Compare(_, CmpOp::Eq, _)));
        assert!(matches!(parse("1!=2"), AstNode::Compare(_, CmpOp::Ne, _)));
        assert!(matches!(parse("1<2"), AstNode::Compare(_, CmpOp::Lt, _)));
        assert!(matches!(parse("1>2"), AstNode::Compare(_, CmpOp::Gt, _)));
        assert!(matches!(parse("1<=2"), AstNode::Compare(_, CmpOp::Le, _)));
        assert!(matches!(parse("1>=2"), AstNode::Compare(_, CmpOp::Ge, _)));
    }

    // ---- unary minus ----

    #[test]
    fn test_unary_minus_number() {
        assert_eq!(parse("-5"), AstNode::UnaryMinus(Box::new(AstNode::Number(5.0))));
    }

    #[test]
    fn test_unary_minus_cell_ref() {
        assert_eq!(
            parse("-A1"),
            AstNode::UnaryMinus(Box::new(AstNode::CellRef {
                column_id: "A".to_string(),
                row_id: "1".to_string()
            }))
        );
    }

    #[test]
    fn test_unary_minus_parenthesized() {
        assert_eq!(
            parse("-(1+2)"),
            AstNode::UnaryMinus(Box::new(AstNode::BinaryOp(
                Box::new(AstNode::Number(1.0)),
                BinOp::Add,
                Box::new(AstNode::Number(2.0))
            )))
        );
    }

    #[test]
    fn test_double_negative() {
        assert_eq!(
            parse("--5"),
            AstNode::UnaryMinus(Box::new(AstNode::UnaryMinus(Box::new(AstNode::Number(5.0)))))
        );
    }

    #[test]
    fn test_unary_minus_binds_tighter_than_mul() {
        // -2*3 should be Mul(UnaryMinus(2), 3) = -6, not UnaryMinus(2*3)
        let ast = parse("-2*3");
        assert_eq!(
            ast,
            AstNode::BinaryOp(
                Box::new(AstNode::UnaryMinus(Box::new(AstNode::Number(2.0)))),
                BinOp::Mul,
                Box::new(AstNode::Number(3.0))
            )
        );
    }

    // ---- cell references (A1-style) ----

    #[test]
    fn test_a1_style_cell_ref() {
        assert_eq!(
            parse("A1"),
            AstNode::CellRef {
                column_id: "A".to_string(),
                row_id: "1".to_string()
            }
        );
    }

    #[test]
    fn test_a1_style_multi_letter_column() {
        assert_eq!(
            parse("AB123"),
            AstNode::CellRef {
                column_id: "AB".to_string(),
                row_id: "123".to_string()
            }
        );
    }

    #[test]
    fn test_a1_style_lowercase_normalized_uppercase() {
        assert_eq!(
            parse("b3"),
            AstNode::CellRef {
                column_id: "B".to_string(),
                row_id: "3".to_string()
            }
        );
    }

    // ---- cell references (stable-ID form, §33.5.2) ----

    #[test]
    fn test_stable_id_cell_ref() {
        assert_eq!(
            parse("{col-uuid-123:row-uuid-456}"),
            AstNode::CellRef {
                column_id: "col-uuid-123".to_string(),
                row_id: "row-uuid-456".to_string()
            }
        );
    }

    #[test]
    fn test_stable_id_range_ref() {
        assert_eq!(
            parse("{col-uuid-123:row-uuid-1:row-uuid-9}"),
            AstNode::Range {
                start_col: "col-uuid-123".to_string(),
                start_row: "row-uuid-1".to_string(),
                end_col: "col-uuid-123".to_string(),
                end_row: "row-uuid-9".to_string(),
            }
        );
    }

    #[test]
    fn test_stable_id_ref_malformed_too_many_parts_errors() {
        let err = parse_err("{a:b:c:d}");
        assert!(matches!(err, FormulaError::Parse(_)));
    }

    #[test]
    fn test_stable_id_ref_unterminated_brace_errors() {
        let err = parse_err("{col:row");
        assert!(matches!(err, FormulaError::Parse(_)));
    }

    // ---- ranges (A1-style) ----

    #[test]
    fn test_a1_range() {
        assert_eq!(
            parse("A1:A5"),
            AstNode::Range {
                start_col: "A".to_string(),
                start_row: "1".to_string(),
                end_col: "A".to_string(),
                end_row: "5".to_string(),
            }
        );
    }

    #[test]
    fn test_malformed_range_missing_second_endpoint() {
        let err = parse_err("A1:");
        assert!(matches!(err, FormulaError::Parse(_)));
    }

    // ---- item 37 (#2874): single-row ranges, `{col1:row1}:{col2:row2}` ----

    #[test]
    fn test_parse_single_row_range() {
        assert_eq!(
            parse("A1:C1"),
            AstNode::Range {
                start_col: "A".to_string(),
                start_row: "1".to_string(),
                end_col: "C".to_string(),
                end_row: "1".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_column_range_regression() {
        // Same assertion as `test_a1_range` above, named per item 37's
        // scope doc to explicitly flag it as the column-range regression
        // check alongside the new row-range test.
        assert_eq!(
            parse("A1:A5"),
            AstNode::Range {
                start_col: "A".to_string(),
                start_row: "1".to_string(),
                end_col: "A".to_string(),
                end_row: "5".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_legacy_column_range_form() {
        // Legacy 3-part brace form `{col:row1:row2}` stays read-only:
        // the tokenizer/parser arms that handle `Token::IdRangeRef` are
        // untouched by item 37, and must keep parsing this shape
        // correctly forever (existing test sheets may still contain it).
        assert_eq!(
            parse("{col-uuid-123:row-uuid-1:row-uuid-9}"),
            AstNode::Range {
                start_col: "col-uuid-123".to_string(),
                start_row: "row-uuid-1".to_string(),
                end_col: "col-uuid-123".to_string(),
                end_row: "row-uuid-9".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_new_two_ref_range_form() {
        // New ID-form range: two complete `{col:row}` cell refs joined
        // by a colon, produced by `parse_primary`'s `Token::IdRef` arm
        // peek-ahead (no new `Token` variant needed).
        assert_eq!(
            parse("{col-a:row-1}:{col-c:row-1}"),
            AstNode::Range {
                start_col: "col-a".to_string(),
                start_row: "row-1".to_string(),
                end_col: "col-c".to_string(),
                end_row: "row-1".to_string(),
            }
        );
    }

    // ---- function calls ----

    #[test]
    fn test_sum_function_call() {
        assert_eq!(
            parse("SUM(A1:A5)"),
            AstNode::FnCall(
                FnName::Sum,
                vec![AstNode::Range {
                    start_col: "A".to_string(),
                    start_row: "1".to_string(),
                    end_col: "A".to_string(),
                    end_row: "5".to_string(),
                }]
            )
        );
    }

    #[test]
    fn test_average_function_call() {
        assert_eq!(
            parse("AVERAGE(B1:B10)"),
            AstNode::FnCall(
                FnName::Average,
                vec![AstNode::Range {
                    start_col: "B".to_string(),
                    start_row: "1".to_string(),
                    end_col: "B".to_string(),
                    end_row: "10".to_string(),
                }]
            )
        );
    }

    #[test]
    fn test_if_function_call() {
        let ast = parse("IF(A1>0, A1, 0)");
        match ast {
            AstNode::FnCall(FnName::If, args) => assert_eq!(args.len(), 3),
            other => panic!("expected IF call, got {other:?}"),
        }
    }

    #[test]
    fn test_function_name_case_insensitive() {
        assert!(matches!(parse("sum(A1:A2)"), AstNode::FnCall(FnName::Sum, _)));
        assert!(matches!(
            parse("Average(A1:A2)"),
            AstNode::FnCall(FnName::Average, _)
        ));
    }

    #[test]
    fn test_unknown_function_name_parses_as_unknown_not_a_parse_error() {
        // #NAME? is an eval-time error (§33.5.3), not a parse error.
        match parse("FOO(1,2)") {
            AstNode::FnCall(FnName::Unknown(name), args) => {
                assert_eq!(name, "FOO");
                assert_eq!(args.len(), 2);
            }
            other => panic!("expected FnCall(Unknown), got {other:?}"),
        }
    }

    #[test]
    fn test_nested_function_calls() {
        let ast = parse("SUM(A1:A3) + AVERAGE(B1:B3)");
        assert!(matches!(
            ast,
            AstNode::BinaryOp(_, BinOp::Add, _)
        ));
    }

    // ---- malformed inputs ----

    #[test]
    fn test_unbalanced_parens_missing_close() {
        assert!(matches!(parse_err("(1+2"), FormulaError::Parse(_)));
    }

    #[test]
    fn test_unbalanced_parens_extra_close() {
        assert!(matches!(parse_err("1+2)"), FormulaError::Parse(_)));
    }

    #[test]
    fn test_trailing_garbage() {
        assert!(matches!(parse_err("1+2 3"), FormulaError::Parse(_)));
    }

    #[test]
    fn test_empty_input() {
        assert!(matches!(parse_err(""), FormulaError::Parse(_)));
    }

    #[test]
    fn test_empty_formula_after_equals_prefix() {
        assert!(matches!(parse_err("="), FormulaError::Parse(_)));
    }

    #[test]
    fn test_whitespace_only_input() {
        assert!(matches!(parse_err("   "), FormulaError::Parse(_)));
    }

    #[test]
    fn test_dangling_operator() {
        assert!(matches!(parse_err("1+"), FormulaError::Parse(_)));
    }

    #[test]
    fn test_unterminated_string_literal() {
        assert!(matches!(parse_err("\"unterminated"), FormulaError::Parse(_)));
    }

    #[test]
    fn test_invalid_character() {
        assert!(matches!(parse_err("1 & 2"), FormulaError::Parse(_)));
    }

    #[test]
    fn test_bare_unknown_identifier_not_a_ref_or_function() {
        // "HELLO123X" doesn't match A1 shape (letters, then digits, then
        // more letters) and isn't a function call (no trailing paren).
        assert!(matches!(parse_err("HELLO123X"), FormulaError::Parse(_)));
    }

    // ---- whitespace handling ----

    #[test]
    fn test_whitespace_tolerant() {
        assert_eq!(parse("  1   +   2  "), parse("1+2"));
    }

    #[test]
    fn test_whitespace_inside_function_call() {
        assert_eq!(parse("SUM( A1 : A5 )"), parse("SUM(A1:A5)"));
    }

    // ---- tokenizer-level cases ----

    #[test]
    fn test_tokenize_rejects_bang_without_equals() {
        let err = tokenize("1!2").unwrap_err();
        assert!(matches!(err, FormulaError::Parse(_)));
    }

    #[test]
    fn test_tokenize_number_with_multiple_dots_stops_at_second() {
        // "1.2.3" tokenizes as Number(1.2) followed by trailing "." — the
        // parser then rejects the leftover trailing input.
        assert!(matches!(parse_err("1.2.3"), FormulaError::Parse(_)));
    }

    // ---- recursion-depth guard (#2339 PR 7, PR 6 audit FAIL-tier fix) ----

    fn nested_parens(depth: usize) -> String {
        let mut s = String::with_capacity(depth * 2 + 1);
        for _ in 0..depth {
            s.push('(');
        }
        s.push('1');
        for _ in 0..depth {
            s.push(')');
        }
        s
    }

    #[test]
    fn test_deeply_nested_parens_returns_parse_error_not_crash() {
        // 200 levels of nesting is well past the PR 6 audit's empirically
        // observed 150-200-level stack-overflow crash threshold. Must
        // return a graceful #PARSE!, never abort the process.
        let input = nested_parens(200);
        let err = parse_err(&input);
        assert!(matches!(err, FormulaError::Parse(_)));
        assert!(err.to_string().contains("#PARSE!"));
    }

    #[test]
    fn test_formula_at_exactly_max_depth_still_parses() {
        // `enter_depth` counts every `parse_expr` entry, including the
        // implicit top-level call `parse_formula` makes before any paren
        // is even seen — so `MAX_PARSE_DEPTH - 1` levels of nested `(`
        // pushes the counter to exactly `MAX_PARSE_DEPTH` (the ceiling),
        // which must still succeed. The guard must not be off-by-one
        // against legitimate formulas sitting exactly at the ceiling.
        let input = nested_parens(MAX_PARSE_DEPTH - 1);
        let ast = parse(&input);
        assert_eq!(ast, AstNode::Number(1.0));
    }

    #[test]
    fn test_formula_one_level_over_max_depth_is_parse_error() {
        // MAX_PARSE_DEPTH levels of nested `(` pushes the counter to
        // MAX_PARSE_DEPTH + 1 (one past the boundary case above) — must
        // fail.
        let input = nested_parens(MAX_PARSE_DEPTH);
        let err = parse_err(&input);
        assert!(matches!(err, FormulaError::Parse(_)));
    }

    #[test]
    fn test_deeply_chained_unary_minus_returns_parse_error_not_crash() {
        // Chained unary minus re-enters parse_unary recursively too —
        // exercise the same guard via a different recursive path.
        let mut input = "-".repeat(300);
        input.push('1');
        let err = parse_err(&input);
        assert!(matches!(err, FormulaError::Parse(_)));
    }
}
