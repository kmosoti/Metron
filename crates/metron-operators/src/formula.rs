//! A small Boolean formula language, for proposals that come in as text.
//!
//! Grammar (lowest precedence first): `or := xor ('|' xor)*`,
//! `xor := and ('^' and)*`, `and := unary ('&' unary)*`,
//! `unary := ('~' | '!') unary | atom`, `atom := 'x' digits | '0' | '1' |
//! '(' or ')'`. The words `and`, `or`, `xor`, `not` and the symbols `∧`,
//! `∨`, `⊕`, `¬` are accepted too. Whitespace is ignored.

use metron_core::bits::BitVector;
use std::fmt;

/// A parsed formula.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Formula {
    /// A constant.
    Const(bool),
    /// Input variable `x_i`.
    Var(usize),
    /// Negation.
    Not(Box<Formula>),
    /// Conjunction.
    And(Box<Formula>, Box<Formula>),
    /// Disjunction.
    Or(Box<Formula>, Box<Formula>),
    /// Exclusive or.
    Xor(Box<Formula>, Box<Formula>),
}

/// Parse errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormulaError {
    /// Unexpected character or end of input.
    #[error("unexpected `{found}` at position {at}")]
    Unexpected {
        /// What was found (`end` at the end of input).
        found: String,
        /// Byte position.
        at: usize,
    },
    /// A variable index beyond the arity.
    #[error("variable x{index} exceeds arity {arity}")]
    VariableOutOfRange {
        /// The index.
        index: usize,
        /// The arity.
        arity: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Var(usize),
    Const(bool),
    Not,
    And,
    Or,
    Xor,
    LParen,
    RParen,
}

fn tokenize(text: &str) -> Result<Vec<(Token, usize)>, FormulaError> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        match c {
            c if c.is_whitespace() => {
                i += 1;
            }
            '(' => {
                tokens.push((Token::LParen, at));
                i += 1;
            }
            ')' => {
                tokens.push((Token::RParen, at));
                i += 1;
            }
            '~' | '!' | '¬' => {
                tokens.push((Token::Not, at));
                i += 1;
            }
            '&' | '∧' | '·' | '*' => {
                tokens.push((Token::And, at));
                i += 1;
                if c == '&' && chars.get(i).is_some_and(|&(_, d)| d == '&') {
                    i += 1;
                }
            }
            '|' | '∨' | '+' => {
                tokens.push((Token::Or, at));
                i += 1;
                if c == '|' && chars.get(i).is_some_and(|&(_, d)| d == '|') {
                    i += 1;
                }
            }
            '^' | '⊕' => {
                tokens.push((Token::Xor, at));
                i += 1;
            }
            '0' => {
                tokens.push((Token::Const(false), at));
                i += 1;
            }
            '1' => {
                tokens.push((Token::Const(true), at));
                i += 1;
            }
            // A variable: `x` followed by digits. Juxtaposed variables such
            // as `x0x1` tokenize one at a time.
            'x' | 'X' if chars.get(i + 1).is_some_and(|&(_, d)| d.is_ascii_digit()) => {
                let start = i + 1;
                i += 1;
                while i < chars.len() && chars[i].1.is_ascii_digit() {
                    i += 1;
                }
                let digits: String = chars[start..i].iter().map(|&(_, c)| c).collect();
                let index = digits.parse().map_err(|_| FormulaError::Unexpected {
                    found: format!("x{digits}"),
                    at,
                })?;
                tokens.push((Token::Var(index), at));
            }
            c if c.is_ascii_alphabetic() => {
                let start = i;
                while i < chars.len() && chars[i].1.is_ascii_alphabetic() {
                    i += 1;
                }
                let word: String = chars[start..i].iter().map(|&(_, c)| c).collect();
                let token = match word.to_ascii_lowercase().as_str() {
                    "and" => Token::And,
                    "or" => Token::Or,
                    "xor" => Token::Xor,
                    "not" => Token::Not,
                    "true" => Token::Const(true),
                    "false" => Token::Const(false),
                    _ => {
                        return Err(FormulaError::Unexpected { found: word, at });
                    }
                };
                tokens.push((token, at));
            }
            other => {
                return Err(FormulaError::Unexpected {
                    found: other.to_string(),
                    at,
                });
            }
        }
    }
    Ok(tokens)
}

struct Parser {
    tokens: Vec<(Token, usize)>,
    pos: usize,
    end: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(t, _)| t)
    }

    fn unexpected(&self) -> FormulaError {
        match self.tokens.get(self.pos) {
            Some((t, at)) => FormulaError::Unexpected {
                found: format!("{t:?}"),
                at: *at,
            },
            None => FormulaError::Unexpected {
                found: "end".into(),
                at: self.end,
            },
        }
    }

    fn or(&mut self) -> Result<Formula, FormulaError> {
        let mut left = self.xor()?;
        while self.peek() == Some(&Token::Or) {
            self.pos += 1;
            let right = self.xor()?;
            left = Formula::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn xor(&mut self) -> Result<Formula, FormulaError> {
        let mut left = self.and()?;
        while self.peek() == Some(&Token::Xor) {
            self.pos += 1;
            let right = self.and()?;
            left = Formula::Xor(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Formula, FormulaError> {
        let mut left = self.unary()?;
        loop {
            match self.peek() {
                Some(Token::And) => {
                    self.pos += 1;
                }
                // Juxtaposition (`x0x1`, `x0 (x1 | x2)`) is conjunction.
                Some(Token::Var(_) | Token::LParen | Token::Not) => {}
                _ => break,
            }
            let right = self.unary()?;
            left = Formula::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Formula, FormulaError> {
        match self.peek() {
            Some(Token::Not) => {
                self.pos += 1;
                Ok(Formula::Not(Box::new(self.unary()?)))
            }
            Some(Token::Var(i)) => {
                let i = *i;
                self.pos += 1;
                Ok(Formula::Var(i))
            }
            Some(Token::Const(b)) => {
                let b = *b;
                self.pos += 1;
                Ok(Formula::Const(b))
            }
            Some(Token::LParen) => {
                self.pos += 1;
                let inner = self.or()?;
                if self.peek() != Some(&Token::RParen) {
                    return Err(self.unexpected());
                }
                self.pos += 1;
                Ok(inner)
            }
            _ => Err(self.unexpected()),
        }
    }
}

impl Formula {
    /// Parses a formula.
    pub fn parse(text: &str) -> Result<Formula, FormulaError> {
        let tokens = tokenize(text)?;
        let mut parser = Parser {
            tokens,
            pos: 0,
            end: text.len(),
        };
        let formula = parser.or()?;
        if parser.pos != parser.tokens.len() {
            return Err(parser.unexpected());
        }
        Ok(formula)
    }

    /// Evaluates at a row index (`x_j` is bit `j`).
    #[must_use]
    pub fn eval(&self, row: usize) -> bool {
        match self {
            Formula::Const(b) => *b,
            Formula::Var(i) => (row >> i) & 1 == 1,
            Formula::Not(f) => !f.eval(row),
            Formula::And(a, b) => a.eval(row) && b.eval(row),
            Formula::Or(a, b) => a.eval(row) || b.eval(row),
            Formula::Xor(a, b) => a.eval(row) ^ b.eval(row),
        }
    }

    /// The largest variable index used, if any.
    #[must_use]
    pub fn max_var(&self) -> Option<usize> {
        match self {
            Formula::Const(_) => None,
            Formula::Var(i) => Some(*i),
            Formula::Not(f) => f.max_var(),
            Formula::And(a, b) | Formula::Or(a, b) | Formula::Xor(a, b) => {
                a.max_var().max(b.max_var())
            }
        }
    }

    /// The complete truth table on `arity` inputs.
    pub fn to_table(&self, arity: usize) -> Result<BitVector, FormulaError> {
        if let Some(i) = self.max_var()
            && i >= arity
        {
            return Err(FormulaError::VariableOutOfRange { index: i, arity });
        }
        Ok(BitVector::from_fn(1usize << arity, |row| self.eval(row)))
    }
}

impl fmt::Display for Formula {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Formula::Const(b) => write!(f, "{}", u8::from(*b)),
            Formula::Var(i) => write!(f, "x{i}"),
            Formula::Not(x) => write!(f, "~{x}"),
            Formula::And(a, b) => write!(f, "({a} & {b})"),
            Formula::Or(a, b) => write!(f, "({a} | {b})"),
            Formula::Xor(a, b) => write!(f, "({a} ^ {b})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_evaluates() {
        let f = Formula::parse("(x0 & x1) | (x0 & x2) | (x1 & x2)").unwrap();
        let t = f.to_table(3).unwrap();
        let rows: String = t.iter().map(|b| if b { '1' } else { '0' }).collect();
        assert_eq!(rows, "00010111");
        let g = Formula::parse("x0 xor x1 XOR x2").unwrap();
        assert_eq!(g.to_table(3).unwrap().count_ones(), 4);
        let h = Formula::parse("¬x0 ∧ (x1 ∨ x2)").unwrap();
        assert!(h.eval(0b110));
        assert!(!h.eval(0b111));
        assert_eq!(
            Formula::parse("x0x1").unwrap(),
            Formula::parse("x0 & x1").unwrap()
        );
        assert_eq!(
            Formula::parse("x0 && x1 || x2").unwrap().to_string(),
            "((x0 & x1) | x2)"
        );
    }

    #[test]
    fn precedence_and_errors() {
        let f = Formula::parse("x0 | x1 & x2 ^ x3").unwrap();
        assert_eq!(f.to_string(), "(x0 | ((x1 & x2) ^ x3))");
        assert!(matches!(
            Formula::parse("x0 &"),
            Err(FormulaError::Unexpected { .. })
        ));
        assert!(matches!(
            Formula::parse("y1"),
            Err(FormulaError::Unexpected { .. })
        ));
        assert!(matches!(
            Formula::parse("(x0"),
            Err(FormulaError::Unexpected { .. })
        ));
        assert!(matches!(
            Formula::parse("x5").unwrap().to_table(3),
            Err(FormulaError::VariableOutOfRange { index: 5, arity: 3 })
        ));
    }
}
