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
                // A conditional the writer decides is not a placeholder the intake
                // report should call a typo.
                if !deferred(token) && !unknown.iter().any(|seen: &String| seen == token) {
                    unknown.push(token.to_owned());
                }
            }
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    (out, unknown)
}

/// Whether `pattern` matches anywhere in `text`.
///
/// The dialect spells a match `=~/(FLEX|TPU|TPE)/`, so what this has to understand
/// is literal characters, `.` for any one, `*` for any number of the atom before
/// it, `|` between alternatives and `(` `)` to group them -- and nothing more. A
/// pattern using anything else matches nothing rather than being guessed at.
fn matches_pattern(pattern: &str, text: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let alternatives = split_alternatives(pattern);
    alternatives
        .iter()
        .filter_map(|alternative| pieces(alternative))
        .any(|pieces| (0..=text.len()).any(|start| matches_at(&pieces, &text[start..])))
}

/// The alternatives of a pattern, split at every `|` outside a group.
fn split_alternatives(pattern: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for c in pattern.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            '|' if depth == 0 => out.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

enum Atom {
    Char(char),
    Any,
    Group(Vec<String>),
}

struct Piece {
    atom: Atom,
    star: bool,
}

/// One alternative as a sequence of atoms, or `None` if it uses a construct this
/// does not understand.
fn pieces(alternative: &str) -> Option<Vec<Piece>> {
    let chars: Vec<char> = alternative.chars().collect();
    let mut out: Vec<Piece> = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let atom = match chars[at] {
            '.' => Atom::Any,
            '(' => {
                let close = chars[at + 1..].iter().position(|c| *c == ')')? + at + 1;
                let inner: String = chars[at + 1..close].iter().collect();
                at = close;
                Atom::Group(split_alternatives(&inner))
            }
            '\\' => {
                at += 1;
                Atom::Char(*chars.get(at)?)
            }
            '*' | ')' | ']' | '[' | '+' | '?' | '{' | '}' | '^' | '$' => return None,
            c => Atom::Char(c),
        };
        at += 1;
        let star = chars.get(at) == Some(&'*');
        if star {
            at += 1;
        }
        out.push(Piece { atom, star });
    }
    Some(out)
}

fn matches_at(pieces: &[Piece], text: &[char]) -> bool {
    let Some((piece, rest)) = pieces.split_first() else {
        return true;
    };
    for taken in lengths(&piece.atom, text) {
        if piece.star {
            if (0..=taken).any(|used| matches_at(&pieces[1..], &text[used..]))
                || matches_at(rest, &text[taken..])
            {
                return true;
            }
        } else if matches_at(rest, &text[taken..]) {
            return true;
        }
    }
    piece.star && matches_at(rest, text)
}

/// How many characters the atom at the front of `text` can consume, longest first.
fn lengths(atom: &Atom, text: &[char]) -> Vec<usize> {
    match atom {
        Atom::Char(c) => match text.first() {
            Some(found) if found == c => vec![1],
            _ => Vec::new(),
        },
        Atom::Any => match text.first() {
            Some(_) => vec![1],
            None => Vec::new(),
        },
        Atom::Group(alternatives) => alternatives
            .iter()
            .filter_map(|alternative| pieces(alternative))
            .filter_map(|pieces| consumed(&pieces, text))
            .collect(),
    }
}

/// The length a group's alternative takes when it matches the front of `text`.
fn consumed(pieces: &[Piece], text: &[char]) -> Option<usize> {
    match pieces.split_first() {
        None => Some(0),
        Some((piece, rest)) => {
            for taken in lengths(&piece.atom, text) {
                if let Some(more) = consumed(rest, &text[taken..]) {
                    return Some(taken + more);
                }
                if piece.star && matches_at(rest, text) {
                    return Some(0);
                }
            }
            if piece.star {
                consumed(rest, text)
            } else {
                None
            }
        }
    }
}

/// Whether a token is decided by the writer rather than here: a conditional, or a
/// function of the layer height, both of which the writer knows and this does not.
pub(crate) fn deferred(token: &str) -> bool {
    let token = token.trim();
    token.starts_with("if ")
        || token == "else"
        || token == "endif"
        || token.starts_with("interpolate_table(")
}

/// The straight line through \p points, read at \p x, held flat past either end.
fn interpolate(points: &[(f64, f64)], x: f64) -> f64 {
    let first = points[0];
    let last = points[points.len() - 1];
    if x <= first.0 {
        return first.1;
    }
    if x >= last.0 {
        return last.1;
    }
    for window in points.windows(2) {
        let (left, right) = (window[0], window[1]);
        if x >= left.0 && x <= right.0 {
            if right.0 == left.0 {
                return right.1;
            }
            let t = (x - left.0) / (right.0 - left.0);
            return left.1 + t * (right.1 - left.1);
        }
    }
    last.1
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
    let unknown: Vec<String> = Vec::new();
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
            // A condition this cannot judge reads a per-layer value such as
            // `layer_z`, which the writer knows and this does not. The block goes
            // through whole, markers and all, so the writer decides it one layer at
            // a time instead of both branches reaching the printer.
            let taken = match evaluate(condition, lookup) {
                Some(value) => {
                    if Value::of(&value).truthy() {
                        Branch::On
                    } else {
                        Branch::Off
                    }
                }
                None => Branch::Both,
            };
            emitting.push(if live { taken } else { Branch::Off });
            if live && taken == Branch::Both {
                out.push_str(&rest[at..at + end + 1]);
            }
        } else if token.trim() == "else" {
            let both = matches!(emitting.last(), Some(Branch::Both));
            if let Some(last) = emitting.last_mut() {
                *last = match last {
                    Branch::On => Branch::Off,
                    Branch::Off => Branch::On,
                    Branch::Both => Branch::Both,
                };
            }
            if both && live {
                out.push_str(&rest[at..at + end + 1]);
            }
        } else if token.trim() == "endif" {
            let both = matches!(emitting.last(), Some(Branch::Both));
            emitting.pop();
            if both && live {
                out.push_str(&rest[at..at + end + 1]);
            }
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
    /// `[initial_tool]`: an index the dialect computes rather than writes.
    NamedIndex(String),
    /// `/.*MBL160.*/`: the pattern of a match operator.
    Pattern(String),
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
            let end = bytes[start..].iter().position(|c| *c == ']')? + start;
            let inner: String = bytes[start..end].iter().collect();
            at = end + 1;
            match inner.trim().parse::<usize>() {
                Ok(index) => tokens.push(Token::Index(index)),
                Err(_) => tokens.push(Token::NamedIndex(inner.trim().to_owned())),
            }
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
        if matches!(two.as_str(), "=~" | "!~") {
            let opened = at + 2;
            if bytes.get(opened) != Some(&'/') {
                return None;
            }
            let close = bytes[opened + 1..].iter().position(|c| *c == '/')? + opened + 1;
            tokens.push(Token::Symbol(two));
            tokens.push(Token::Pattern(bytes[opened + 1..close].iter().collect()));
            at = close + 1;
            continue;
        }
        if ["==", "!=", "<=", ">=", "&&", "||"].contains(&two.as_str()) {
            tokens.push(Token::Symbol(two));
            at += 2;
            continue;
        }
        if "?:<>+-*/%!,".contains(c) {
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

    /// Consumes a word operator such as `or`, which the dialect spells out where
    /// its sibling writes `||`.
    fn word(&mut self, keyword: &str) -> bool {
        if let Some(Token::Name(name)) = self.peek() {
            if name == keyword {
                self.at += 1;
                return true;
            }
        }
        false
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
        while self.eat("||") || self.word("or") {
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
        while self.eat("&&") || self.word("and") {
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
        for symbol in ["=~", "!~"] {
            if self.eat(symbol) {
                let pattern = match self.peek() {
                    Some(Token::Pattern(pattern)) => pattern.clone(),
                    _ => return None,
                };
                self.at += 1;
                let found = matches_pattern(&pattern, &left.print());
                let truth = if symbol == "=~" { found } else { !found };
                return Some(Value::Number(if truth { 1.0 } else { 0.0 }));
            }
        }
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

    /// A function the dialects carry. Only the one every profile uses is
    /// implemented: a call this does not know fails, which leaves the text as it
    /// was written for the caller to report rather than inventing a value.
    fn call(&mut self, name: &str, lookup: Lookup<'_>) -> Option<Value> {
        match name {
            "interpolate_table" => self.interpolate_table(lookup),
            "min" => self.aggregate(lookup, true),
            "is_nil" => self.is_nil(lookup),
            "max" => self.aggregate(lookup, false),
            _ => None,
        }
    }

    /// `is_nil(x)`: 1 when `x` names a setting the profile leaves unset, 0 when it
    /// has a value.
    ///
    /// The dialect writes `{if is_nil(idle_temperature[initial_tool])}100{else}…`:
    /// the point of the call is the argument that cannot be read, so a failed read
    /// is the answer, not an error to give up on.
    fn is_nil(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        if !matches!(self.peek(), Some(Token::Open)) {
            return None;
        }
        self.at += 1;
        let start = self.at;
        let unset = match self.ternary(lookup) {
            Some(value) => match self.peek() {
                Some(Token::Close) => value.print().trim().is_empty(),
                _ => true,
            },
            None => true,
        };
        self.at = start;
        let mut depth = 0usize;
        while let Some(token) = self.peek() {
            match token {
                Token::Open => depth += 1,
                Token::Close if depth == 0 => {
                    self.at += 1;
                    return Some(Value::Number(if unset { 1.0 } else { 0.0 }));
                }
                Token::Close => depth -= 1,
                _ => {}
            }
            self.at += 1;
        }
        None
    }

    /// `min(a, b, …)` / `max(a, b, …)`: whichever of the arguments is smallest or
    /// largest. Machine start G-code uses them to keep a bed-mesh probe inside the
    /// plate when the model sits against an edge.
    fn aggregate(&mut self, lookup: Lookup<'_>, smallest: bool) -> Option<Value> {
        if !matches!(self.peek(), Some(Token::Open)) {
            return None;
        }
        self.at += 1;
        let mut value = self.ternary(lookup)?.number()?;
        while self.eat(",") {
            let next = self.ternary(lookup)?.number()?;
            value = if smallest {
                value.min(next)
            } else {
                value.max(next)
            };
        }
        if !matches!(self.peek(), Some(Token::Close)) {
            return None;
        }
        self.at += 1;
        Some(Value::Number(value))
    }

    /// `interpolate_table(x, (a, b), (c, d), …)`: the value at `x` on the straight
    /// line through the points given, held flat past either end. Machine G-code
    /// uses it to taper acceleration and junction deviation against height.
    fn interpolate_table(&mut self, lookup: Lookup<'_>) -> Option<Value> {
        if !matches!(self.peek(), Some(Token::Open)) {
            return None;
        }
        self.at += 1;
        let x = self.ternary(lookup)?.number()?;
        let mut points: Vec<(f64, f64)> = Vec::new();
        while self.eat(",") {
            if !matches!(self.peek(), Some(Token::Open)) {
                return None;
            }
            self.at += 1;
            let at = self.ternary(lookup)?.number()?;
            if !self.eat(",") {
                return None;
            }
            let value = self.ternary(lookup)?.number()?;
            if !matches!(self.peek(), Some(Token::Close)) {
                return None;
            }
            self.at += 1;
            points.push((at, value));
        }
        if !matches!(self.peek(), Some(Token::Close)) || points.is_empty() {
            return None;
        }
        self.at += 1;
        Some(Value::Number(interpolate(&points, x)))
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
                    Some(Token::NamedIndex(inner)) => {
                        let inner = inner.clone();
                        self.at += 1;
                        // `[initial_tool]` names the element, so the element is
                        // worked out from the same settings before it is read.
                        let tokens = tokenize(&inner)?;
                        let mut cursor = Cursor { tokens, at: 0 };
                        let value = cursor.ternary(lookup)?;
                        if cursor.at != cursor.tokens.len() {
                            return None;
                        }
                        Some(value.number()? as usize)
                    }
                    _ => None,
                };
                if index.is_none() && matches!(self.peek(), Some(Token::Open)) {
                    return self.call(&name, lookup);
                }
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
    fn a_condition_reading_a_per_layer_value_is_left_for_the_writer() {
        // `layer_z` is not known here and is known to the writer, so the decision is
        // not made now and not guessed at: the block goes through whole.
        let (text, _) = rendered("{if layer_z > 150}M201 X6000{else}M201 X9000{endif}");
        assert_eq!(text, "{if layer_z > 150}M201 X6000{else}M201 X9000{endif}");
    }

    #[test]
    fn an_unknown_name_is_left_for_the_report_rather_than_guessed() {
        let (text, unknown) = rendered("M862.1 F{(missing[0] ? 1 : 0)}");
        assert_eq!(text, "M862.1 F{(missing[0] ? 1 : 0)}");
        assert_eq!(unknown.len(), 1);
    }
}

#[cfg(test)]
mod table_tests {
    use super::*;

    fn lookup(name: &str, _index: Option<usize>) -> Option<String> {
        match name {
            "layer_z" => Some("1.5".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn a_table_is_read_at_the_height_asked_for() {
        let (text, unknown) = expand(
            "M201 X{interpolate_table(layer_z, (0,6000), (1,6000), (2,2000))}",
            &lookup,
        );
        assert_eq!(text, "M201 X4000");
        assert!(unknown.is_empty(), "{unknown:?}");
    }

    #[test]
    fn a_table_holds_flat_past_either_end() {
        assert_eq!(interpolate(&[(0.0, 6000.0), (1.0, 1000.0)], -5.0), 6000.0);
        assert_eq!(interpolate(&[(0.0, 6000.0), (1.0, 1000.0)], 9.0), 1000.0);
    }
}
