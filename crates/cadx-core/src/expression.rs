//! Restricted, deterministic parameter expressions.
//!
//! Expressions are stored as editable source text, but are parsed and evaluated
//! locally every time they are admitted to a document. The grammar intentionally
//! excludes functions, units, assignment, and arbitrary identifiers beyond a
//! conservative ASCII parameter-name syntax:
//!
//! ```text
//! expression = sum
//! sum        = product (("+" | "-") product)*
//! product    = unary (("*" | "/") unary)*
//! unary      = ("+" | "-") unary | primary
//! primary    = number | identifier | "(" expression ")"
//! ```

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

const MAX_EXPRESSION_BYTES: usize = 4 * 1024;
const MAX_EXPRESSION_NODES: usize = 256;
const MAX_EXPRESSION_DEPTH: usize = 64;

/// Editable source for a scalar parameter formula.
///
/// Formula values are measured in the owning document's units. Parameter names
/// resolve case-sensitively and use [`is_valid_parameter_name`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ParameterExpression(String);

impl ParameterExpression {
    pub fn new(source: impl Into<String>) -> Result<Self, ExpressionError> {
        let source = source.into();
        let source = source.trim();
        let expression = Self(source.into());
        expression.parse()?;
        Ok(expression)
    }

    pub fn source(&self) -> &str {
        &self.0
    }

    /// Returns every referenced parameter name in deterministic order.
    pub fn dependencies(&self) -> Result<BTreeSet<String>, ExpressionError> {
        let expression = self.parse()?;
        let mut dependencies = BTreeSet::new();
        expression.collect_dependencies(&mut dependencies);
        Ok(dependencies)
    }

    /// Evaluates the formula with a local, caller-supplied parameter resolver.
    pub fn evaluate<F>(&self, mut resolve: F) -> Result<f64, ExpressionError>
    where
        F: FnMut(&str) -> Result<f64, ExpressionError>,
    {
        self.parse()?.evaluate(&mut resolve)
    }

    pub(crate) fn parse(&self) -> Result<Expression, ExpressionError> {
        if self.0.is_empty() {
            return Err(ExpressionError::Empty);
        }
        if self.0.len() > MAX_EXPRESSION_BYTES {
            return Err(ExpressionError::TooLong {
                limit: MAX_EXPRESSION_BYTES,
            });
        }
        Parser::new(&self.0).parse()
    }
}

/// Returns whether a string can be used as a stable parameter symbol.
pub fn is_valid_parameter_name(name: &str) -> bool {
    let mut characters = name.bytes();
    matches!(characters.next(), Some(byte) if byte.is_ascii_alphabetic() || byte == b'_')
        && characters.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExpressionError {
    Empty,
    TooLong { limit: usize },
    InvalidCharacter { position: usize },
    InvalidNumber { position: usize },
    UnexpectedToken { position: usize },
    UnexpectedEnd,
    NestingLimit { limit: usize },
    NodeLimit { limit: usize },
    UnknownParameter(String),
    DuplicateParameterName(String),
    DependencyCycle(Vec<String>),
    DivisionByZero,
    NonFiniteResult,
}

impl fmt::Display for ExpressionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("parameter expression cannot be empty"),
            Self::TooLong { limit } => {
                write!(
                    formatter,
                    "parameter expression exceeds the {limit}-byte limit"
                )
            }
            Self::InvalidCharacter { position } => {
                write!(formatter, "invalid expression character at byte {position}")
            }
            Self::InvalidNumber { position } => {
                write!(formatter, "invalid number at byte {position}")
            }
            Self::UnexpectedToken { position } => {
                write!(formatter, "unexpected expression token at byte {position}")
            }
            Self::UnexpectedEnd => formatter.write_str("expression ended unexpectedly"),
            Self::NestingLimit { limit } => {
                write!(
                    formatter,
                    "expression nesting exceeds the {limit}-level limit"
                )
            }
            Self::NodeLimit { limit } => {
                write!(formatter, "expression has more than {limit} nodes")
            }
            Self::UnknownParameter(name) => write!(formatter, "unknown parameter {name}"),
            Self::DuplicateParameterName(name) => {
                write!(formatter, "parameter name {name} is not unique")
            }
            Self::DependencyCycle(names) => {
                write!(
                    formatter,
                    "parameter dependency cycle: {}",
                    names.join(" -> ")
                )
            }
            Self::DivisionByZero => formatter.write_str("parameter expression divides by zero"),
            Self::NonFiniteResult => {
                formatter.write_str("parameter expression produced a non-finite result")
            }
        }
    }
}

impl std::error::Error for ExpressionError {}

#[derive(Clone, Debug)]
pub(crate) enum Expression {
    Number(f64),
    Parameter(String),
    Negate(Box<Expression>),
    Add(Box<Expression>, Box<Expression>),
    Subtract(Box<Expression>, Box<Expression>),
    Multiply(Box<Expression>, Box<Expression>),
    Divide(Box<Expression>, Box<Expression>),
}

impl Expression {
    fn collect_dependencies(&self, dependencies: &mut BTreeSet<String>) {
        match self {
            Self::Parameter(name) => {
                dependencies.insert(name.clone());
            }
            Self::Negate(value) => value.collect_dependencies(dependencies),
            Self::Add(left, right)
            | Self::Subtract(left, right)
            | Self::Multiply(left, right)
            | Self::Divide(left, right) => {
                left.collect_dependencies(dependencies);
                right.collect_dependencies(dependencies);
            }
            Self::Number(_) => {}
        }
    }

    fn evaluate<F>(&self, resolve: &mut F) -> Result<f64, ExpressionError>
    where
        F: FnMut(&str) -> Result<f64, ExpressionError>,
    {
        let value = match self {
            Self::Number(value) => *value,
            Self::Parameter(name) => resolve(name)?,
            Self::Negate(value) => -value.evaluate(resolve)?,
            Self::Add(left, right) => left.evaluate(resolve)? + right.evaluate(resolve)?,
            Self::Subtract(left, right) => left.evaluate(resolve)? - right.evaluate(resolve)?,
            Self::Multiply(left, right) => left.evaluate(resolve)? * right.evaluate(resolve)?,
            Self::Divide(left, right) => {
                let divisor = right.evaluate(resolve)?;
                if divisor == 0.0 {
                    return Err(ExpressionError::DivisionByZero);
                }
                left.evaluate(resolve)? / divisor
            }
        };
        if value.is_finite() {
            Ok(value)
        } else {
            Err(ExpressionError::NonFiniteResult)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Token<'a> {
    Number(f64),
    Identifier(&'a str),
    Plus,
    Minus,
    Star,
    Slash,
    LeftParenthesis,
    RightParenthesis,
    End,
}

struct Parser<'a> {
    source: &'a str,
    position: usize,
    token_start: usize,
    token: Token<'a>,
    nodes: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            position: 0,
            token_start: 0,
            token: Token::End,
            nodes: 0,
        }
    }

    fn parse(mut self) -> Result<Expression, ExpressionError> {
        self.advance()?;
        let expression = self.parse_sum(0)?;
        match self.token {
            Token::End => Ok(expression),
            _ => Err(ExpressionError::UnexpectedToken {
                position: self.token_start,
            }),
        }
    }

    fn parse_sum(&mut self, depth: usize) -> Result<Expression, ExpressionError> {
        let mut expression = self.parse_product(depth)?;
        loop {
            expression = match self.token {
                Token::Plus => {
                    self.advance()?;
                    let right = self.parse_product(depth)?;
                    self.node(Expression::Add(Box::new(expression), Box::new(right)))?
                }
                Token::Minus => {
                    self.advance()?;
                    let right = self.parse_product(depth)?;
                    self.node(Expression::Subtract(Box::new(expression), Box::new(right)))?
                }
                _ => return Ok(expression),
            };
        }
    }

    fn parse_product(&mut self, depth: usize) -> Result<Expression, ExpressionError> {
        let mut expression = self.parse_unary(depth)?;
        loop {
            expression = match self.token {
                Token::Star => {
                    self.advance()?;
                    let right = self.parse_unary(depth)?;
                    self.node(Expression::Multiply(Box::new(expression), Box::new(right)))?
                }
                Token::Slash => {
                    self.advance()?;
                    let right = self.parse_unary(depth)?;
                    self.node(Expression::Divide(Box::new(expression), Box::new(right)))?
                }
                _ => return Ok(expression),
            };
        }
    }

    fn parse_unary(&mut self, depth: usize) -> Result<Expression, ExpressionError> {
        match self.token {
            Token::Plus => {
                self.advance()?;
                self.parse_unary(depth + 1)
            }
            Token::Minus => {
                self.advance()?;
                let value = self.parse_unary(depth + 1)?;
                self.node(Expression::Negate(Box::new(value)))
            }
            _ => self.parse_primary(depth),
        }
    }

    fn parse_primary(&mut self, depth: usize) -> Result<Expression, ExpressionError> {
        if depth > MAX_EXPRESSION_DEPTH {
            return Err(ExpressionError::NestingLimit {
                limit: MAX_EXPRESSION_DEPTH,
            });
        }
        match self.token {
            Token::Number(value) => {
                self.advance()?;
                self.node(Expression::Number(value))
            }
            Token::Identifier(name) => {
                self.advance()?;
                self.node(Expression::Parameter(name.into()))
            }
            Token::LeftParenthesis => {
                self.advance()?;
                let expression = self.parse_sum(depth + 1)?;
                if self.token != Token::RightParenthesis {
                    return Err(match self.token {
                        Token::End => ExpressionError::UnexpectedEnd,
                        _ => ExpressionError::UnexpectedToken {
                            position: self.token_start,
                        },
                    });
                }
                self.advance()?;
                Ok(expression)
            }
            Token::End => Err(ExpressionError::UnexpectedEnd),
            _ => Err(ExpressionError::UnexpectedToken {
                position: self.token_start,
            }),
        }
    }

    fn node(&mut self, expression: Expression) -> Result<Expression, ExpressionError> {
        self.nodes += 1;
        if self.nodes > MAX_EXPRESSION_NODES {
            return Err(ExpressionError::NodeLimit {
                limit: MAX_EXPRESSION_NODES,
            });
        }
        Ok(expression)
    }

    fn advance(&mut self) -> Result<(), ExpressionError> {
        self.skip_whitespace();
        self.token_start = self.position;
        let Some(byte) = self.byte_at(self.position) else {
            self.token = Token::End;
            return Ok(());
        };
        self.token = match byte {
            b'+' => {
                self.position += 1;
                Token::Plus
            }
            b'-' => {
                self.position += 1;
                Token::Minus
            }
            b'*' => {
                self.position += 1;
                Token::Star
            }
            b'/' => {
                self.position += 1;
                Token::Slash
            }
            b'(' => {
                self.position += 1;
                Token::LeftParenthesis
            }
            b')' => {
                self.position += 1;
                Token::RightParenthesis
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => self.identifier(),
            byte if byte.is_ascii_digit() || byte == b'.' => self.number()?,
            _ => {
                return Err(ExpressionError::InvalidCharacter {
                    position: self.position,
                });
            }
        };
        Ok(())
    }

    fn skip_whitespace(&mut self) {
        while self
            .byte_at(self.position)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            self.position += 1;
        }
    }

    fn identifier(&mut self) -> Token<'a> {
        let start = self.position;
        self.position += 1;
        while self
            .byte_at(self.position)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            self.position += 1;
        }
        Token::Identifier(&self.source[start..self.position])
    }

    fn number(&mut self) -> Result<Token<'a>, ExpressionError> {
        let start = self.position;
        let mut digits_before_decimal = 0;
        while self
            .byte_at(self.position)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            self.position += 1;
            digits_before_decimal += 1;
        }
        let mut digits_after_decimal = 0;
        if self.byte_at(self.position) == Some(b'.') {
            self.position += 1;
            while self
                .byte_at(self.position)
                .is_some_and(|byte| byte.is_ascii_digit())
            {
                self.position += 1;
                digits_after_decimal += 1;
            }
        }
        if digits_before_decimal == 0 && digits_after_decimal == 0 {
            return Err(ExpressionError::InvalidNumber { position: start });
        }
        if matches!(self.byte_at(self.position), Some(b'e' | b'E')) {
            self.position += 1;
            if matches!(self.byte_at(self.position), Some(b'+' | b'-')) {
                self.position += 1;
            }
            let exponent_start = self.position;
            while self
                .byte_at(self.position)
                .is_some_and(|byte| byte.is_ascii_digit())
            {
                self.position += 1;
            }
            if self.position == exponent_start {
                return Err(ExpressionError::InvalidNumber { position: start });
            }
        }
        let value = self.source[start..self.position]
            .parse::<f64>()
            .map_err(|_| ExpressionError::InvalidNumber { position: start })?;
        if !value.is_finite() {
            return Err(ExpressionError::InvalidNumber { position: start });
        }
        Ok(Token::Number(value))
    }

    fn byte_at(&self, position: usize) -> Option<u8> {
        self.source.as_bytes().get(position).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_precedence_parentheses_and_dependencies() {
        let expression = ParameterExpression::new("(base + 5) * scale - offset / 2").unwrap();
        let dependencies = expression.dependencies().unwrap();

        assert_eq!(
            dependencies.into_iter().collect::<Vec<_>>(),
            vec!["base", "offset", "scale"]
        );
        assert_eq!(
            expression
                .evaluate(|name| match name {
                    "base" => Ok(10.0),
                    "scale" => Ok(3.0),
                    "offset" => Ok(8.0),
                    _ => Err(ExpressionError::UnknownParameter(name.into())),
                })
                .unwrap(),
            41.0
        );
    }

    #[test]
    fn rejects_invalid_formulae_and_non_finite_results() {
        assert!(matches!(
            ParameterExpression::new("unknown@value"),
            Err(ExpressionError::InvalidCharacter { .. })
        ));
        assert!(matches!(
            ParameterExpression::new("(1 + 2"),
            Err(ExpressionError::UnexpectedEnd)
        ));
        let division = ParameterExpression::new("10 / (2 - 2)").unwrap();
        assert_eq!(
            division.evaluate(|name| Err(ExpressionError::UnknownParameter(name.into()))),
            Err(ExpressionError::DivisionByZero)
        );
    }

    #[test]
    fn parameter_symbols_are_conservative_and_stable() {
        assert!(is_valid_parameter_name("base_width"));
        assert!(is_valid_parameter_name("A2"));
        assert!(!is_valid_parameter_name("2base"));
        assert!(!is_valid_parameter_name("base-width"));
        assert!(!is_valid_parameter_name(""));
    }
}
