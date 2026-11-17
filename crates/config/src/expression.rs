//! The small expression language the reference slicers' machine G-code is
//! written in.
//!
//! PrusaSlicer and OrcaSlicer do not stop at `[first_layer_temperature]`: a
//! machine's start G-code tests settings, picks between two commands and picks
//! an element out of a per-filament list.
//!
//! ```text
//! M862.1 P[nozzle_diameter] A{(filament_abrasive[0] ? 1 : 0)}
//! {if nozzle_diameter[0] == 0.4}M900 K0.05{else}M900 K0.08{endif}
//! ```
//!
//! This module evaluates that language. It is deliberately small: the values it
//! can see are the preset's settings, the operators are the ones those dialects
//! document, and anything it cannot evaluate is left in the text for the caller
//! to report rather than guessed at.

/// A setting's value as an expression sees it.
///
/// Everything the preset carries is text; a number is only a number when the
/// comparison or the arithmetic needs one.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Number(f64),
    Text(String),
}

impl Value {
    fn of(text: &str) -> Value {
        match text.trim().parse::<f64>() {
            Ok(number) => Value::Number(number),
            Err(_) => Value::Text(text.trim().to_owned()),
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Value::Number(number) => *number != 0.0,
            Value::Text(text) => !text.is_empty() && !text.eq_ignore_ascii_case("false"),
        }
    }

    fn print(&self) -> String {
        match self {
            Value::Number(number) => {
                if number.fract() == 0.0 && number.abs() < 1e15 {
                    format!("{}", *number as i64)
                } else {
                    format!("{number}")
                }
            }
            Value::Text(text) => text.clone(),
        }
    }

    fn number(&self) -> Option<f64> {
        match self {
            Value::Number(number) => Some(*number),
            Value::Text(text) => text.trim().parse::<f64>().ok(),
        }
    }
}

/// Looks a setting up by name, with the element index the dialect may append.
pub(crate) type Lookup<'a> = &'a dyn Fn(&str, Option<usize>) -> Option<String>;

/// Replaces every expression the text carries.
///
/// `{if …}`, `{else}` and `{endif}` are resolved first, so the text of a branch
/// that is not taken never reaches the printer. Whatever is left inside braces
/// is evaluated; a brace that does not evaluate is left exactly as written and
/// named in the returned list, which is what the intake report is built from.
pub(crate) fn expand(text: &str, lookup: Lookup<'_>) -> (String, Vec<String>) {
    let (unconditioned, mut unknown) = expand_conditions(text, lookup);
    let mut out = String::with_capacity(unconditioned.len());
    let mut rest = unconditioned.as_str();
    while let Some(at) = rest.find('{') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let Some(end) = after.find('}') else {
            break;
        };
        let token = &after[..end];
        match evaluate(token, lookup) {
            Some(value) => out.push_str(&value),
            None => {
                out.push('{');
                out.push_str(token);
                out.push('}');
                if !unknown.iter().any(|seen: &String| seen == token) {
                    unknown.push(token.to_owned());
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    (out, unknown)
}

/// Whether the text of one `{if}` block belongs in the output.
#[derive(Clone, Copy, PartialEq)]
enum Branch {
    On,
    Off,
    Both,
}

/// Resolves `{if …}` / `{else}` / `{endif}` blocks, dropping untaken branches.
fn expand_conditions(text: &str, lookup: Lookup<'_>) -> (String, Vec<String>) {
    let mut unknown: Vec<String> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut emitting: Vec<Branch> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        let Some(end) = rest[at..].find('}') else {
            break;
        };
        let token = &rest[at + 1..at + end];
        let live = emitting.iter().all(|branch| *branch != Branch::Off);
        if live {
            out.push_str(&rest[..at]);
        }
        if let Some(condition) = token.trim().strip_prefix("if ") {
            // A condition this cannot judge - anything reading a per-layer value
            // such as `layer_z` - keeps both branches rather than guessing at
            // one, and is named in the report so the profile's author can see
            // that the choice was not made.
            let taken = match evaluate(condition, lookup) {
                Some(value) => {
                    if Value::of(&value).truthy() {
                        Branch::On
                    } else {
                        Branch::Off
                    }
                }
                None => {
                    let condition = condition.trim().to_owned();
                    if !unknown.contains(&condition) {
                        unknown.push(condition);
                    }
                    Branch::Both
                }
            };
            emitting.push(if live { taken } else { Branch::Off });
        } else if token.trim() == "else" {
            if let Some(last) = emitting.last_mut() {
                *last = match last {
                    Branch::On => Branch::Off,
                    Branch::Off => Branch::On,
                    Branch::Both => Branch::Both,
                };
            }
        } else if token.trim() == "endif" {
            emitting.pop();
        } else if live {
            out.push_str(&rest[at..at + end + 1]);
        }
        rest = &rest[at + end + 1..];
    }
    if emitting.is_empty() {
        out.push_str(rest);
    }
    (out, unknown)
}

/// Evaluates one expression, or reports that it could not.
pub(crate) fn evaluate(expr: &str, lookup: Lookup<'_>) -> Option<String> {
    let tokens = tokenize(expr)?;
    let mut cursor = Cursor { tokens, at: 0 };
    let value = cursor.ternary(lookup)?;
    if cursor.at != cursor.tokens.len() {
        return None;
    }
    Some(value.print())
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Number(f64),
    Text(String),
    Name(String),
    Index(usize),
    Open,
    Close,
    Symbol(String),
}

fn tokenize(expr: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let bytes: Vec<char> = expr.chars().collect();
    let mut at = 0;
    while at < bytes.len() {
        let c = bytes[at];
        if c.is_whitespace() {
            at += 1;
            continue;
        }
        if c.is_ascii_digit() || (c == '.' && bytes.get(at + 1).is_some_and(char::is_ascii_digit)) {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == '.') {
                at += 1;
            }
            let text: String = bytes[start..at].iter().collect();
            tokens.push(Token::Number(text.parse::<f64>().ok()?));
            continue;
        }
        if c == '"' || c == '\'' {
            let quote = c;
            at += 1;
            let mut text = String::new();
            while at < bytes.len() && bytes[at] != quote {
                text.push(bytes[at]);
                at += 1;
            }
            if at >= bytes.len() {
                return None;
            }
            at += 1;
            tokens.push(Token::Text(text));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = at;
            while at < bytes.len() && (bytes[at].is_alphanumeric() || bytes[at] == '_') {
                at += 1;
            }
            tokens.push(Token::Name(bytes[start..at].iter().collect()));
            continue;
        }
        if c == '[' {
            let start = at + 1;
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_digit() {
                at += 1;
            }
            let index: usize = bytes[start..at].iter().collect::<String>().parse().ok()?;
            if bytes.get(at) != Some(&']') {
                return None;
            }
            at += 1;
            tokens.push(Token::Index(index));
            continue;
        }
        if c == '(' {
            tokens.push(Token::Open);
            at += 1;
            continue;
        }
        if c == ')' {
            tokens.push(Token::Close);
            at += 1;
            continue;
        }
        let two: String = bytes[at..(at + 2).min(bytes.len())].iter().collect();
        if ["==", "!=", "<=", ">=", "&&", "||"].contains(&two.as_str()) {
            tokens.push(Token::Symbol(two));
            at += 2;
            continue;
        }
        if "?:<>+-*/%!".contains(c) {
            tokens.push(Token::Symbol(c.to_string()));
            at += 1;
            continue;
        }
        return None;
    }
    Some(tokens)
}

struct Cursor {
    tokens: Vec<Token>,
    at: usize,
}

impl Cursor {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, symbol: &str) -> bool {
        if let Some(Token::Symbol(found)) = self.peek() {
            if found == symbol {
                self.at += 1;
                return true;
            }
        }
        false
    }

    fn ternary(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        let condition = self.or(lookup)?;
        if !self.eat("?") {
            return Some(condition);
        }
        let taken = self.ternary(lookup)?;
        if !self.eat(":") {
            return None;
        }
        let otherwise = self.ternary(lookup)?;
        Some(if condition.truthy() { taken } else { otherwise })
    }

    fn or(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        let mut value = self.and(lookup)?;
        while self.eat("||") {
            let right = self.and(lookup)?;
            value = Value::Number(if value.truthy() || right.truthy() {
                1.0
            } else {
                0.0
            });
        }
        Some(value)
    }

    fn and(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        let mut value = self.comparison(lookup)?;
        while self.eat("&&") {
            let right = self.comparison(lookup)?;
            value = Value::Number(if value.truthy() && right.truthy() {
                1.0
            } else {
                0.0
            });
        }
        Some(value)
    }

    fn comparison(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        let left = self.sum(lookup)?;
        for symbol in ["==", "!=", "<=", ">=", "<", ">"] {
            if self.eat(symbol) {
                let right = self.sum(lookup)?;
                let equal = match (left.number(), right.number()) {
                    (Some(a), Some(b)) => a == b,
                    _ => left.print() == right.print(),
                };
                let order = match (left.number(), right.number()) {
                    (Some(a), Some(b)) => Some(a.partial_cmp(&b)?),
                    _ => None,
                };
                let truth = match symbol {
                    "==" => equal,
                    "!=" => !equal,
                    "<" => order.is_some_and(|order| order.is_lt()),
                    ">" => order.is_some_and(|order| order.is_gt()),
                    "<=" => order.is_some_and(|order| order.is_le()),
                    ">=" => order.is_some_and(|order| order.is_ge()),
                    _ => false,
                };
                return Some(Value::Number(if truth { 1.0 } else { 0.0 }));
            }
        }
        Some(left)
    }

    fn sum(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        let mut value = self.product(lookup)?;
        loop {
            if self.eat("+") {
                let right = self.product(lookup)?;
                value = Value::Number(value.number()? + right.number()?);
            } else if self.eat("-") {
                let right = self.product(lookup)?;
                value = Value::Number(value.number()? - right.number()?);
            } else {
                return Some(value);
            }
        }
    }

    fn product(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        let mut value = self.unary(lookup)?;
        loop {
            if self.eat("*") {
                let right = self.unary(lookup)?;
                value = Value::Number(value.number()? * right.number()?);
            } else if self.eat("/") {
                let right = self.unary(lookup)?;
                let divisor = right.number()?;
                if divisor == 0.0 {
                    return None;
                }
                value = Value::Number(value.number()? / divisor);
            } else if self.eat("%") {
                let right = self.unary(lookup)?;
                let divisor = right.number()?;
                if divisor == 0.0 {
                    return None;
                }
                value = Value::Number(value.number()? % divisor);
            } else {
                return Some(value);
            }
        }
    }

    fn unary(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        if self.eat("!") {
            let value = self.unary(lookup)?;
            return Some(Value::Number(if value.truthy() { 0.0 } else { 1.0 }));
        }
        if self.eat("-") {
            let value = self.unary(lookup)?;
            return Some(Value::Number(-value.number()?));
        }
        self.primary(lookup)
    }

    fn primary(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        let token = self.peek()?.clone();
        match token {
            Token::Number(number) => {
                self.at += 1;
                Some(Value::Number(number))
            }
            Token::Text(text) => {
                self.at += 1;
                Some(Value::Text(text))
            }
            Token::Name(name) => {
                self.at += 1;
                let index = match self.peek() {
                    Some(Token::Index(index)) => {
                        let index = *index;
                        self.at += 1;
                        Some(index)
                    }
                    _ => None,
                };
                Some(Value::of(&lookup(&name, index)?))
            }
            Token::Open => {
                self.at += 1;
                let value = self.ternary(lookup)?;
                if !matches!(self.peek(), Some(Token::Close)) {
                    return None;
                }
                self.at += 1;
                Some(value)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(name: &str, index: Option<usize>) -> Option<String> {
        match (name, index) {
            ("nozzle_diameter", _) => Some("0.4".to_owned()),
            ("filament_type", Some(0)) => Some("PLA".to_owned()),
            ("filament_type", Some(1)) => Some("PETG".to_owned()),
            ("has_fan", _) => Some("0".to_owned()),
            ("brand", _) => Some("Prusament".to_owned()),
            _ => None,
        }
    }

    fn rendered(text: &str) -> (String, Vec<String>) {
        expand(text, &settings)
    }

    #[test]
    fn an_expression_picks_the_branch_its_condition_names() {
        let (text, unknown) = rendered("M862.1 P[nozzle_diameter] A{(has_fan ? 1 : 0)}");
        assert_eq!(text, "M862.1 P[nozzle_diameter] A0");
        assert!(unknown.is_empty(), "{unknown:?}");
    }

    #[test]
    fn a_conditional_block_emits_only_the_branch_taken() {
        let (text, _) = rendered("{if nozzle_diameter[0] == 0.4}M900 K0.05{else}M900 K0.08{endif}");
        assert_eq!(text, "M900 K0.05");
        let (text, _) = rendered("{if nozzle_diameter[0] > 0.4}high{else}low{endif}");
        assert_eq!(text, "low");
    }

    #[test]
    fn indexed_settings_read_the_element_asked_for() {
        let (text, _) = rendered("{filament_type[1]}");
        assert_eq!(text, "PETG");
        let (text, _) = rendered("{if filament_type[0] == \"PLA\"}pla{endif}");
        assert_eq!(text, "pla");
    }

    #[test]
    fn arithmetic_and_text_work_where_the_dialect_allows_them() {
        let (text, _) = rendered("{(nozzle_diameter[0] * 2 + 1)}");
        assert_eq!(text, "1.8");
        let (text, _) = rendered("{if brand == \"Prusament\"}{brand}{endif}");
        assert_eq!(text, "Prusament");
    }

    #[test]
    fn a_condition_reading_a_per_layer_value_keeps_both_branches_and_is_named() {
        let (text, unknown) = rendered("{if layer_z > 150}M201 X6000{else}M201 X9000{endif}");
        assert!(text.contains("M201 X6000"), "{text}");
        assert!(text.contains("M201 X9000"), "{text}");
        assert_eq!(unknown, vec!["layer_z > 150".to_owned()]);
    }

    #[test]
    fn an_unknown_name_is_left_for_the_report_rather_than_guessed() {
        let (text, unknown) = rendered("M862.1 F{(missing[0] ? 1 : 0)}");
        assert_eq!(text, "M862.1 F{(missing[0] ? 1 : 0)}");
        assert_eq!(unknown.len(), 1);
    }
}
