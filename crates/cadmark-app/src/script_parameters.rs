// Script parameters — the module-level names a build123d script binds to
// numbers, and the surgical rewrite of one of their values.
//
// The panel this feeds is not a view of whatever the AI labelled as a
// parameter block: it is every top-level numeric name the script
// actually uses (C16). So the script is read the way Python reads it
// rather than the way the file is typed, and it is read once, as a
// grammar. `tokenize` turns the source into Python's tokens — comments
// and backslash continuations gone, a string literal taken whole
// (including an f-string's nested quotes), bracket depth carried on
// every token, and a statement ended by a newline outside brackets or
// by a semicolon. Everything above that walks tokens. Nothing matches
// on the shape of a physical line, which is what kept losing to the
// language one shape at a time.
//
// A statement counts when the first token of its logical line stands in
// column zero. That is a deliberate boundary rather than a claim about
// Python's scopes: a `def` or `class` body really is another scope, but
// a `with BuildPart()` or `if` body binds module names too. An indented
// binding is modelling code — a dimension chosen inside a builder
// block, or one that only runs on some path — and offering to edit it
// beside the script's parameters would be offering to edit the model's
// internals. So a name bound only inside a suite is not a row, and a
// name a suite rebinds keeps the value of its module-level binding.
//
// A statement's targets are parsed rather than pattern-matched: a
// target list becomes plain names, starred names, nested tuples, and
// targets this module cannot read (an attribute, a subscript), and that
// tree is walked against the value opposite it. Every shape Python
// spells a binding with therefore reaches the panel — `width = 80`,
// `width: float = 80`, `width = depth = 40`, `width = 80; depth = 40`,
// a statement continued across lines, `width, (depth, wall) = 80, (40,
// 2)`, and `width, *rest = 80, 40, 20`, where the star absorbs the
// values left over and the names around it keep their own. A target
// the module cannot read contributes no name and takes none of its
// neighbours with it. Only a shape that could not bind at run time —
// three names against two values, a tuple target against a plain
// number — yields nothing at all, because there the names' values
// would be a guess and a guessed row is worse than an absent one.
//
// A value is either a numeric literal, which the user can edit, or an
// arithmetic expression over numbers and other parameters, which is
// shown for what it is and not editable — C33 asks the AI to derive
// dimensions from each other, and a derived name is a name the script
// uses. Numeric constants the script imported from `math` count as
// numbers there: `angle = pi / 4` and `angle = math.tau / 8` are both
// names the script uses.
//
// Rewriting replaces the bytes of one literal and nothing else. The AI
// is the author of the file, so an edit must return a file that differs
// from the original in exactly that span: no reformatting, no
// reordering, no normalised whitespace, and the literal written back in
// the notation the author wrote it in.
//
// A parameter can be locked: a `# locked` comment at the end of its
// binding's line (`# locked: why`, optionally) marks it as a hard
// constraint the user has fixed — a fit, a clearance, a mounting
// position — which the AI may change only with the user's explicit
// permission. The marker lives in the script because the script is the
// design: it survives every rewrite the AI makes, shows in the code
// panel, and travels with the file. Locking and unlocking are the same
// kind of surgical edit as a value change: the marker's bytes and
// nothing else.

use std::collections::HashSet;
use std::ops::Range;

/// A module-level name bound to a number.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    pub name: String,
    /// One-based source line of the binding that decides the value.
    pub line: u32,
    pub binding: Binding,
    /// The `# locked` marker on the binding's line, when it has one.
    pub lock: Option<Lock>,
    /// Byte offset just past the binding statement's last token, where a
    /// marker is added.
    statement_end: usize,
}

/// A `# locked` marker: the declaration that a parameter is a hard
/// constraint, changed only with the user's explicit permission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lock {
    /// Why it is locked, when the marker says (`# locked: must clear the
    /// bolt head`).
    pub reason: Option<String>,
    /// The bytes the marker occupies, from the whitespace before its `#`
    /// to the end of the line, so unlocking removes exactly it.
    span: Range<usize>,
}

/// How a parameter gets its value.
#[derive(Debug, Clone, PartialEq)]
pub enum Binding {
    /// A numeric literal: its value, the byte range it occupies in the
    /// source, and the notation it was written in (so an edit keeps the
    /// author's).
    Literal {
        value: f64,
        span: Range<usize>,
        notation: Notation,
    },
    /// An expression over numbers and earlier parameters, as the
    /// script writes it on one line.
    Derived { expression: String },
}

/// How a literal is spelled, so a value written back is spelled the
/// same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notation {
    /// A plain integer: `80`.
    Integer,
    /// A decimal point or an exponent: `40.0`, `1e3`.
    Decimal,
    /// A radix literal — `0x1F`, `0o17`, `0b1010` — keeping the prefix
    /// and the letter case the author used.
    Radix {
        prefix: String,
        base: u32,
        uppercase: bool,
    },
}

impl Parameter {
    /// The literal value, when this parameter has one.
    pub fn value(&self) -> Option<f64> {
        match &self.binding {
            Binding::Literal { value, .. } => Some(*value),
            Binding::Derived { .. } => None,
        }
    }

    /// The expression a derived parameter is bound to, when it has one.
    pub fn expression(&self) -> Option<&str> {
        match &self.binding {
            Binding::Derived { expression } => Some(expression),
            Binding::Literal { .. } => None,
        }
    }

    /// The value or expression as the script states it, for telling one
    /// version of a parameter from another.
    pub fn stated(&self) -> String {
        match &self.binding {
            Binding::Literal { value, .. } => format!("{value}"),
            Binding::Derived { expression } => expression.clone(),
        }
    }
}

/// Why a value could not be written back.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RewriteError {
    #[error("{0} is not a parameter of this script")]
    NoSuchParameter(String),
    #[error("{0} is derived from other parameters, so it has no value of its own to set")]
    NotEditable(String),
    #[error("{0} is not a number that can be written into the script")]
    NotANumber(f64),
}

/// Every module-level name the script binds to a number, in source order.
pub fn extract(source: &str) -> Vec<Parameter> {
    let tokens = tokenize(source);
    let mut found: Vec<Parameter> = Vec::new();
    let mut known: HashSet<String> = HashSet::new();
    let mut imports = Imports::default();

    for statement in module_statements(&tokens) {
        note_import(statement, &mut imports);
        let Some((targets, value)) = split_assignment(statement) else {
            continue;
        };
        let value = value_tree(value);
        // A chained binding (`width = depth = 40`) binds every target to
        // the one value, each name reaching it by its own route.
        let mut bindings = Vec::new();
        for target in targets {
            let mut bound = Vec::new();
            if bind(&target_tree(target), &value, &mut bound).is_some() {
                bindings.extend(bound);
            }
        }
        let line = statement[0].line;
        let statement_end = statement.last().map_or(0, |token| token.span.end);
        let lock = trailing_lock(source, statement_end);
        for (name, bound) in bindings {
            let binding = bound.and_then(|tokens| {
                if let Some((value, span, notation)) = literal(tokens) {
                    Some(Binding::Literal {
                        value,
                        span,
                        notation,
                    })
                } else if is_number_expression(tokens, &known, &imports) {
                    Some(Binding::Derived {
                        expression: expression_text(source, tokens),
                    })
                } else {
                    None
                }
            });

            match binding {
                Some(binding) => {
                    let parameter = Parameter {
                        name: name.to_string(),
                        line,
                        binding,
                        lock: lock.clone(),
                        statement_end,
                    };
                    known.insert(name.to_string());
                    match found.iter_mut().find(|existing| existing.name == name) {
                        // A rebinding decides the value the script runs
                        // with, but the name keeps the place it first
                        // appeared.
                        Some(existing) => *existing = parameter,
                        None => found.push(parameter),
                    }
                }
                None => {
                    // The name is rebound to something that is not a
                    // number, so it is not a parameter of the script that
                    // runs.
                    known.remove(name);
                    found.retain(|existing| existing.name != name);
                }
            }
        }
    }
    found
}

/// Return `source` with one parameter's literal replaced by `value`,
/// every other byte untouched.
pub fn rewrite(source: &str, name: &str, value: f64) -> Result<String, RewriteError> {
    if !value.is_finite() {
        return Err(RewriteError::NotANumber(value));
    }
    let parameter = find(source, name)?;
    let Binding::Literal { span, notation, .. } = parameter.binding else {
        return Err(RewriteError::NotEditable(name.to_string()));
    };
    Ok(splice(source, span, &format_number(value, &notation)))
}

/// Return `source` with a `# locked` marker at the end of one parameter's
/// line — `# locked: reason` when a reason is given — replacing the
/// marker already there, if any, and leaving every other byte as it is.
pub fn lock(source: &str, name: &str, reason: Option<&str>) -> Result<String, RewriteError> {
    let parameter = find(source, name)?;
    // A reason is one line: a break in it would split the script.
    let reason = reason
        .map(|reason| reason.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|reason| !reason.is_empty());
    let marker = match reason {
        Some(reason) => format!("  # locked: {reason}"),
        None => "  # locked".to_string(),
    };
    let replaced = match parameter.lock {
        Some(existing) => existing.span,
        None => {
            let end = line_end(source, parameter.statement_end);
            end..end
        }
    };
    Ok(splice(source, replaced, &marker))
}

/// Return `source` with the `# locked` marker taken off one parameter's
/// line; a parameter that is not locked comes back unchanged.
pub fn unlock(source: &str, name: &str) -> Result<String, RewriteError> {
    let parameter = find(source, name)?;
    Ok(match parameter.lock {
        Some(existing) => splice(source, existing.span, ""),
        None => source.to_string(),
    })
}

fn find(source: &str, name: &str) -> Result<Parameter, RewriteError> {
    extract(source)
        .into_iter()
        .find(|parameter| parameter.name == name)
        .ok_or_else(|| RewriteError::NoSuchParameter(name.to_string()))
}

fn splice(source: &str, span: Range<usize>, replacement: &str) -> String {
    let mut out = String::with_capacity(source.len() + replacement.len());
    out.push_str(&source[..span.start]);
    out.push_str(replacement);
    out.push_str(&source[span.end..]);
    out
}

/// The byte offset of the newline ending the physical line `at` is on,
/// or the end of the source.
fn line_end(source: &str, at: usize) -> usize {
    source[at..]
        .find('\n')
        .map_or(source.len(), |offset| at + offset)
}

/// The `# locked` marker ending the physical line a statement ends on,
/// when there is one. The bytes between a statement's last token and the
/// newline hold only whitespace and comments — anything else would have
/// been a token — so the scan is direct. The marker is the last comment
/// segment on the line whose text begins `locked`, so a comment of the
/// author's before it (`# mm  # locked`) is left as theirs.
fn trailing_lock(source: &str, statement_end: usize) -> Option<Lock> {
    let end = line_end(source, statement_end);
    let tail = &source[statement_end..end];
    let mut found = None;
    for (hash, _) in tail.match_indices('#') {
        let body = tail[hash + 1..].trim_start();
        let Some(after) = body
            .get(.."locked".len())
            .filter(|head| head.eq_ignore_ascii_case("locked"))
            .map(|_| &body["locked".len()..])
        else {
            continue;
        };
        if !(after.is_empty() || after.starts_with(':') || after.starts_with(char::is_whitespace)) {
            continue;
        }
        let reason = after.trim_start_matches(':').trim();
        let start = statement_end + tail[..hash].trim_end().len();
        found = Some(Lock {
            reason: (!reason.is_empty()).then(|| reason.to_string()),
            span: start..end,
        });
    }
    found
}

/// A number as the script should carry it: a name the author wrote as an
/// integer stays an integer unless the new value needs a fraction, one
/// written with a decimal point keeps one, and one written in hex,
/// octal or binary is written back the same way.
fn format_number(value: f64, notation: &Notation) -> String {
    match notation {
        Notation::Decimal => {
            let text = format!("{value}");
            if text.contains(['.', 'e', 'E']) {
                text
            } else {
                format!("{text}.0")
            }
        }
        Notation::Integer => plain_number(value),
        Notation::Radix {
            prefix,
            base,
            uppercase,
        } => {
            if value.fract() != 0.0 || value.abs() >= 1e15 {
                // The new value has no radix spelling, so it takes the
                // plainest one that is true.
                return plain_number(value);
            }
            let magnitude = value.abs() as u64;
            let digits = match base {
                16 => format!("{magnitude:x}"),
                8 => format!("{magnitude:o}"),
                _ => format!("{magnitude:b}"),
            };
            let digits = if *uppercase {
                digits.to_uppercase()
            } else {
                digits
            };
            let sign = if value < 0.0 { "-" } else { "" };
            format!("{sign}{prefix}{digits}")
        }
    }
}

fn plain_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

// ---------------------------------------------------------------------
// The tokeniser
// ---------------------------------------------------------------------

/// What a token is. `End` is the boundary between simple statements: a
/// newline outside every bracket, or a semicolon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Name,
    Number,
    Text,
    Op,
    End,
}

/// One Python token, with the byte range it occupies in the source — so
/// a span reached through the grammar is a span in the file, and a
/// rewrite splices where the author's bytes actually are.
#[derive(Debug, Clone)]
struct Token<'a> {
    kind: Kind,
    text: &'a str,
    span: Range<usize>,
    /// One-based source line the token starts on.
    line: u32,
    /// Bracket depth around the token: a bracket reports the depth
    /// outside it, so an opening bracket and its partner agree.
    depth: u32,
    /// Whether the logical line this token belongs to begins in column
    /// zero — module scope, as opposed to the body of a suite.
    module_scope: bool,
}

impl Token<'_> {
    fn is_op(&self, text: &str) -> bool {
        self.kind == Kind::Op && self.text == text
    }

    fn is_name(&self, text: &str) -> bool {
        self.kind == Kind::Name && self.text == text
    }
}

/// Operators, longest first so `**=` is not read as `**` then `=`.
const OPERATORS: [&str; 33] = [
    "**=", "//=", ">>=", "<<=", "...", "!=", "==", "<=", ">=", "->", ":=", "+=", "-=", "*=", "/=",
    "%=", "&=", "|=", "^=", "@=", "**", "//", "<<", ">>", "+", "-", "*", "/", "%", "@", "&", "|",
    "^",
];

/// The keywords that cannot begin an assignment, so a statement opening
/// with one is not a binding however it goes on. The soft keywords
/// (`match`, `case`, `type`) are absent deliberately: `match = 5` binds
/// a parameter.
const KEYWORDS: [&str; 33] = [
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
];

/// The source as Python's tokens. Comments and backslash continuations
/// are dropped rather than blanked, because a token carries its own
/// span and no downstream stage reads the text between tokens for
/// structure.
fn tokenize(source: &str) -> Vec<Token<'_>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    let mut line = 1;
    let mut depth: u32 = 0;
    // Whether the next token opens a logical line, and the scope of the
    // one currently open. A semicolon does not open a new logical line,
    // so statements joined by one share their line's scope.
    let mut opening = true;
    let mut module_scope = true;

    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'\n' {
            if depth == 0 && !opening {
                tokens.push(Token {
                    kind: Kind::End,
                    text: "\n",
                    span: at..at + 1,
                    line,
                    depth,
                    module_scope,
                });
                opening = true;
            }
            line += 1;
            at += 1;
            continue;
        }
        if byte.is_ascii_whitespace() {
            at += 1;
            continue;
        }
        if byte == b'#' {
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
            continue;
        }
        if let Some(length) = continuation_length(&bytes[at..]) {
            at += length;
            line += 1;
            continue;
        }
        if opening {
            module_scope = at == 0 || bytes[at - 1] == b'\n';
            opening = false;
        }
        let start = at;
        let kind = if let Some(end) = end_of_string(bytes, at) {
            at = end;
            Kind::Text
        } else if byte.is_ascii_digit()
            || (byte == b'.' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit))
        {
            at = end_of_number(bytes, at);
            Kind::Number
        } else if byte == b'_' || !byte.is_ascii() || byte.is_ascii_alphabetic() {
            while at < bytes.len()
                && (bytes[at] == b'_' || !bytes[at].is_ascii() || bytes[at].is_ascii_alphanumeric())
            {
                at += 1;
            }
            Kind::Name
        } else if byte == b';' && depth == 0 {
            at += 1;
            Kind::End
        } else {
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            let operator = OPERATORS
                .iter()
                .find(|operator| source[at..].starts_with(**operator))
                .copied();
            at += operator.map_or(1, str::len);
            Kind::Op
        };
        // A bracket reports the depth outside it, so its partner agrees.
        let token_depth = match byte {
            b'(' | b'[' | b'{' => depth - 1,
            _ => depth,
        };
        let text = &source[start..at];
        tokens.push(Token {
            kind,
            text,
            span: start..at,
            line,
            depth: token_depth,
            module_scope,
        });
        if kind == Kind::End {
            // A semicolon ends the statement but not the line.
            continue;
        }
        line += text.matches('\n').count() as u32;
    }
    if !opening {
        tokens.push(Token {
            kind: Kind::End,
            text: "",
            span: bytes.len()..bytes.len(),
            line,
            depth,
            module_scope,
        });
    }
    tokens
}

/// The length of the backslash line continuation starting here, if that
/// is what this is: the backslash, the newline, and a carriage return
/// between them.
fn continuation_length(bytes: &[u8]) -> Option<usize> {
    if bytes.first() != Some(&b'\\') {
        return None;
    }
    match bytes.get(1) {
        Some(b'\n') => Some(2),
        Some(b'\r') if bytes.get(2) == Some(&b'\n') => Some(3),
        _ => None,
    }
}

/// The byte just past the number that starts at `at`. A radix literal's
/// letters and an underscore separator belong to it, and so does the
/// sign of a decimal exponent — but not of a hex digit `e`, which is a
/// digit rather than an exponent marker.
fn end_of_number(bytes: &[u8], at: usize) -> usize {
    let radix = matches!(bytes.get(at), Some(b'0'))
        && matches!(
            bytes.get(at + 1),
            Some(b'x' | b'X' | b'o' | b'O' | b'b' | b'B')
        );
    let mut end = at;
    while end < bytes.len() {
        let byte = bytes[end];
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' {
            end += 1;
            continue;
        }
        if !radix
            && matches!(byte, b'+' | b'-')
            && matches!(bytes.get(end - 1), Some(b'e' | b'E'))
            && bytes.get(end + 1).is_some_and(u8::is_ascii_digit)
        {
            end += 1;
            continue;
        }
        break;
    }
    end
}

/// The letters a string prefix may be made of (`r`, `b`, `u`, `f`, and
/// their pairs).
fn is_prefix_letter(byte: u8) -> bool {
    matches!(byte, b'r' | b'R' | b'b' | b'B' | b'u' | b'U' | b'f' | b'F')
}

/// The byte just past the string literal starting at `at`, or `None`
/// when no string starts there. A prefix belongs to the literal, and an
/// f-string's replacement fields are read as code — so a quote nested
/// inside braces does not end the string, which is legal from Python
/// 3.12 and is the one input a text scanner reads as structure it is
/// not. An unterminated literal runs to the end of the source, as
/// Python's tokeniser would have it.
fn end_of_string(bytes: &[u8], at: usize) -> Option<usize> {
    let mut prefix = 0;
    while prefix < 2
        && bytes
            .get(at + prefix)
            .copied()
            .is_some_and(is_prefix_letter)
    {
        prefix += 1;
    }
    // The prefix only exists if a quote follows it.
    let open = loop {
        let start = at + prefix;
        if matches!(bytes.get(start), Some(b'"' | b'\'')) {
            break start;
        }
        if prefix == 0 {
            return None;
        }
        prefix -= 1;
    };
    let format = bytes[at..open]
        .iter()
        .any(|byte| matches!(byte, b'f' | b'F'));
    let quote: &[u8] = if bytes[open..].starts_with(b"\"\"\"") {
        b"\"\"\""
    } else if bytes[open..].starts_with(b"'''") {
        b"'''"
    } else if bytes[open] == b'"' {
        b"\""
    } else {
        b"'"
    };
    let mut i = open + quote.len();
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i..].starts_with(quote) {
            return Some(i + quote.len());
        }
        if format && bytes[i] == b'{' {
            if bytes.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            i = end_of_replacement_field(bytes, i + 1);
            continue;
        }
        i += 1;
    }
    Some(bytes.len())
}

/// The byte just past an f-string's replacement field, which is code:
/// its own strings are skipped whole, so `f"{d["k"]}"` reads as one
/// string rather than as two with a name between them.
fn end_of_replacement_field(bytes: &[u8], from: usize) -> usize {
    let mut i = from;
    let mut depth = 1;
    while i < bytes.len() {
        if let Some(end) = end_of_string(bytes, i) {
            i = end;
            continue;
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    i
}

// ---------------------------------------------------------------------
// The grammar
// ---------------------------------------------------------------------

/// The simple statements at module scope, in source order.
fn module_statements<'t, 'a>(tokens: &'t [Token<'a>]) -> Vec<&'t [Token<'a>]> {
    tokens
        .split(|token| token.kind == Kind::End)
        .filter(|statement| statement.first().is_some_and(|token| token.module_scope))
        .collect()
}

/// A statement split into the targets it binds and the expression they
/// are bound to, at every `=` that stands at the statement's own
/// bracket depth. A chain (`width = depth = 40`) is several targets
/// against one value. An augmented assignment is a single `+=` token
/// and not a cut, a comparison is `==` and not a cut, and a keyword
/// statement is not a binding at all. Everything after a `lambda`
/// belongs to the lambda, default arguments included.
fn split_assignment<'t, 'a>(
    statement: &'t [Token<'a>],
) -> Option<(Vec<&'t [Token<'a>]>, &'t [Token<'a>])> {
    let first = statement.first()?;
    if first.kind == Kind::Name && KEYWORDS.contains(&first.text) {
        return None;
    }
    let base = first.depth;
    let mut cuts = Vec::new();
    for (index, token) in statement.iter().enumerate() {
        if token.depth == base && token.is_name("lambda") {
            break;
        }
        if token.is_op("=") && token.depth == base {
            cuts.push(index);
        }
    }
    if cuts.is_empty() {
        return None;
    }
    let mut targets = Vec::with_capacity(cuts.len());
    let mut start = 0;
    for cut in cuts {
        targets.push(&statement[start..cut]);
        start = cut + 1;
    }
    let value = &statement[start..];
    if value.is_empty() || targets.iter().any(|target| target.is_empty()) {
        return None;
    }
    Some((targets, value))
}

/// What an assignment binds to.
#[derive(Debug)]
enum Target<'a> {
    /// A plain name, the only thing that becomes a parameter.
    Name(&'a str),
    /// A starred name, which takes the values left over — a list, never
    /// a number.
    Star(Box<Target<'a>>),
    /// A tuple or list of targets, matched element by element.
    Tuple(Vec<Target<'a>>),
    /// A target this module does not read: an attribute, a subscript,
    /// anything else. It binds no parameter and costs its neighbours
    /// nothing, because its position in the tuple is still known.
    Opaque,
}

/// The value side, in the same shape.
#[derive(Debug)]
enum Value<'t, 'a> {
    Tuple(Vec<Value<'t, 'a>>),
    Expr(&'t [Token<'a>]),
}

/// Parse a target list: `width`, `width, depth`, `width, (depth, wall)`,
/// `width, *rest`, `part.width`.
fn target_tree<'a>(tokens: &[Token<'a>]) -> Target<'a> {
    let (pieces, comma) = split_commas(tokens);
    if comma || pieces.len() > 1 {
        return Target::Tuple(pieces.into_iter().map(one_target).collect());
    }
    match pieces.first() {
        Some(piece) => one_target(piece),
        None => Target::Opaque,
    }
}

/// Parse one element of a target list.
fn one_target<'a>(tokens: &[Token<'a>]) -> Target<'a> {
    let Some(first) = tokens.first() else {
        return Target::Opaque;
    };
    if first.is_op("*") {
        return Target::Star(Box::new(one_target(&tokens[1..])));
    }
    // An annotation is not part of the name: `width: float = 80`.
    let tokens = match tokens
        .iter()
        .position(|token| token.is_op(":") && token.depth == first.depth)
    {
        Some(colon) => &tokens[..colon],
        None => tokens,
    };
    if let [name] = tokens
        && name.kind == Kind::Name
        && !KEYWORDS.contains(&name.text)
    {
        return Target::Name(name.text);
    }
    match bracketed(tokens) {
        Some((_, inner)) => target_tree(inner),
        None => Target::Opaque,
    }
}

/// Parse the value side. A tuple — parenthesised or bare — and a list
/// are element lists; parentheses that merely wrap an expression are
/// not, so `width = (80)` is the literal it holds.
fn value_tree<'t, 'a>(tokens: &'t [Token<'a>]) -> Value<'t, 'a> {
    let (pieces, comma) = split_commas(tokens);
    if comma || pieces.len() > 1 {
        return Value::Tuple(pieces.into_iter().map(value_tree).collect());
    }
    match bracketed(tokens) {
        // A list is a sequence however many elements it has, so
        // `sizes = [80]` is a list and not the number inside it.
        Some((b'[', inner)) => {
            let (pieces, _) = split_commas(inner);
            Value::Tuple(pieces.into_iter().map(value_tree).collect())
        }
        Some((_, inner)) if inner.len() < tokens.len() => value_tree(inner),
        _ => Value::Expr(tokens),
    }
}

/// Walk a target against the value bound to it, collecting the names it
/// reaches and the tokens each is bound to — `None` where a name is
/// bound to a sequence rather than to an expression, which is a name
/// the panel cannot show but must still know about, because it may be
/// rebinding one it could.
///
/// Returns `None` when the shapes cannot bind at run time, in which case
/// the whole assignment yields nothing: the names' values would be a
/// guess, and a guessed row is worse than an absent one.
fn bind<'t, 'a>(
    target: &Target<'a>,
    value: &Value<'t, 'a>,
    bound: &mut Vec<(&'a str, Option<&'t [Token<'a>]>)>,
) -> Option<()> {
    match (target, value) {
        (Target::Name(name), Value::Expr(tokens)) => bound.push((name, Some(tokens))),
        (Target::Name(name), Value::Tuple(_)) => bound.push((name, None)),
        // A starred name takes a list of what is left, never a number.
        (Target::Star(inner), _) => star_names(inner, bound),
        (Target::Opaque, _) => {}
        (Target::Tuple(_), Value::Expr(_)) => return None,
        (Target::Tuple(targets), Value::Tuple(values)) => {
            let stars = targets
                .iter()
                .filter(|target| matches!(target, Target::Star(_)))
                .count();
            match stars {
                0 => {
                    if targets.len() != values.len() {
                        return None;
                    }
                    for (target, value) in targets.iter().zip(values) {
                        bind(target, value, bound)?;
                    }
                }
                1 => {
                    // The star absorbs whatever is left over; the names
                    // before and after it keep their own values.
                    let star = targets
                        .iter()
                        .position(|target| matches!(target, Target::Star(_)))
                        .unwrap_or_default();
                    let after = targets.len() - star - 1;
                    if values.len() + 1 < targets.len() {
                        return None;
                    }
                    for (target, value) in targets[..star].iter().zip(&values[..star]) {
                        bind(target, value, bound)?;
                    }
                    for (target, value) in targets[star + 1..]
                        .iter()
                        .zip(&values[values.len() - after..])
                    {
                        bind(target, value, bound)?;
                    }
                    if let Target::Star(inner) = &targets[star] {
                        star_names(inner, bound);
                    }
                }
                _ => return None,
            }
        }
    }
    Some(())
}

/// The name a starred target binds, which is bound to a list — recorded
/// so that a name it takes over from a numeric binding leaves the panel.
fn star_names<'a>(target: &Target<'a>, bound: &mut Vec<(&'a str, Option<&[Token<'a>]>)>) {
    if let Target::Name(name) = target {
        bound.push((name, None));
    }
}

/// Split a token slice at the commas that stand at its own bracket
/// depth, reporting whether there was one — a trailing comma makes a
/// one-element tuple, so it is structure rather than punctuation.
fn split_commas<'t, 'a>(tokens: &'t [Token<'a>]) -> (Vec<&'t [Token<'a>]>, bool) {
    let Some(first) = tokens.first() else {
        return (Vec::new(), false);
    };
    let base = first.depth;
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut comma = false;
    for (index, token) in tokens.iter().enumerate() {
        if token.is_op(",") && token.depth == base {
            comma = true;
            pieces.push(&tokens[start..index]);
            start = index + 1;
        }
    }
    let last = &tokens[start..];
    if !last.is_empty() {
        pieces.push(last);
    }
    (pieces, comma)
}

/// The bracket wrapping the whole of this slice, and what is inside it.
fn bracketed<'t, 'a>(tokens: &'t [Token<'a>]) -> Option<(u8, &'t [Token<'a>])> {
    let first = tokens.first()?;
    let last = tokens.last()?;
    let (open, close) = if first.is_op("(") {
        (b'(', ")")
    } else if first.is_op("[") {
        (b'[', "]")
    } else {
        return None;
    };
    if !last.is_op(close) || last.depth != first.depth {
        return None;
    }
    // The opening bracket must close at the end and not before it.
    let closes_early = tokens[1..tokens.len() - 1]
        .iter()
        .any(|token| token.depth == first.depth && matches!(token.text, ")" | "]" | "}"));
    if closes_early {
        return None;
    }
    Some((open, &tokens[1..tokens.len() - 1]))
}

/// Strip parentheses that only wrap the tokens inside them.
fn unwrap_parens<'t, 'a>(mut tokens: &'t [Token<'a>]) -> &'t [Token<'a>] {
    while let Some((b'(', inner)) = bracketed(tokens) {
        if inner.len() == tokens.len() {
            break;
        }
        tokens = inner;
    }
    tokens
}

// ---------------------------------------------------------------------
// What counts as a number
// ---------------------------------------------------------------------

/// The numeric literal a value is, when that is all it is: a number
/// token on its own or behind a sign, with the byte range an edit
/// replaces and the notation it keeps.
fn literal(tokens: &[Token<'_>]) -> Option<(f64, Range<usize>, Notation)> {
    match unwrap_parens(tokens) {
        [number] if number.kind == Kind::Number => {
            let (value, notation) = number_value(number.text)?;
            Some((value, number.span.clone(), notation))
        }
        [sign, number] if number.kind == Kind::Number && (sign.is_op("-") || sign.is_op("+")) => {
            let (value, notation) = number_value(number.text)?;
            let value = if sign.is_op("-") { -value } else { value };
            Some((value, sign.span.start..number.span.end, notation))
        }
        _ => None,
    }
}

/// The value of one Python number token, and how it was written.
/// Underscores are separators; an imaginary literal is not a real
/// number and is not read as one.
fn number_value(text: &str) -> Option<(f64, Notation)> {
    let cleaned: String = text.chars().filter(|c| *c != '_').collect();
    if cleaned.ends_with(['j', 'J']) {
        return None;
    }
    for (marker, base) in [("x", 16), ("o", 8), ("b", 2)] {
        let lower = cleaned.to_ascii_lowercase();
        let Some(digits) = lower.strip_prefix(&format!("0{marker}")) else {
            continue;
        };
        let value = u64::from_str_radix(digits, base).ok()?;
        return Some((
            value as f64,
            Notation::Radix {
                prefix: cleaned[..2].to_string(),
                base,
                uppercase: cleaned[2..].chars().any(|c| c.is_ascii_uppercase()),
            },
        ));
    }
    let value: f64 = cleaned.parse().ok()?;
    let notation = if cleaned.contains(['.', 'e', 'E']) {
        Notation::Decimal
    } else {
        Notation::Integer
    };
    Some((value, notation))
}

/// Whether these tokens are arithmetic over numbers and names already
/// known to be parameters. A call, a subscript, a string or an unknown
/// name means this is some other kind of value — a shape, a vector, a
/// filename — and not a number the panel can honestly report.
fn is_number_expression(tokens: &[Token<'_>], known: &HashSet<String>, imports: &Imports) -> bool {
    const ARITHMETIC: [&str; 8] = ["+", "-", "*", "/", "%", "**", "//", "("];
    let mut saw_value = false;
    let mut at = 0;
    while at < tokens.len() {
        let token = &tokens[at];
        match token.kind {
            Kind::Number => {
                if number_value(token.text).is_none() {
                    return false;
                }
                saw_value = true;
                at += 1;
            }
            Kind::Name => {
                // `math.pi`: a constant of the module the script imported.
                if let (Some(dot), Some(constant)) = (tokens.get(at + 1), tokens.get(at + 2))
                    && dot.is_op(".")
                {
                    if !imports.modules.contains(token.text)
                        || !MATH_CONSTANTS.contains(&constant.text)
                    {
                        return false;
                    }
                    saw_value = true;
                    at += 3;
                    continue;
                }
                // A call is not a number, whatever it is called on.
                if tokens.get(at + 1).is_some_and(|next| next.is_op("(")) {
                    return false;
                }
                if !known.contains(token.text) && !imports.constants.contains(token.text) {
                    return false;
                }
                saw_value = true;
                at += 1;
            }
            Kind::Op if ARITHMETIC.contains(&token.text) || token.text == ")" => at += 1,
            _ => return false,
        }
    }
    saw_value
}

/// An expression as the panel shows it: the script's own text, with the
/// gap between two tokens — whitespace, a comment, a continuation —
/// collapsed to the single space that separated them. So an expression
/// the author spread over several lines reads as one, and `m.tau / 8`
/// keeps its shape rather than being respaced.
fn expression_text(source: &str, tokens: &[Token<'_>]) -> String {
    let mut text = String::new();
    let mut previous: Option<&Range<usize>> = None;
    for token in tokens {
        if let Some(previous) = previous
            && previous.end != token.span.start
        {
            text.push(' ');
        }
        text.push_str(&source[token.span.clone()]);
        previous = Some(&token.span);
    }
    text
}

/// The numeric constants `math` offers. A script that imports one is
/// using a number, so an expression over it is a derived parameter.
const MATH_CONSTANTS: [&str; 4] = ["pi", "tau", "e", "inf"];

/// What the script imported that a derived expression may name: `math`
/// under whatever alias, and constants taken from it directly.
#[derive(Debug, Default)]
struct Imports {
    /// Names bound by `from math import pi` (or `... as p`).
    constants: HashSet<String>,
    /// Aliases bound by `import math` (or `... as m`).
    modules: HashSet<String>,
}

/// Record a `math` import, so later expressions can name what it bound.
/// Other modules bind values this module cannot judge to be numbers, so
/// they are ignored and names from them are not parameters.
fn note_import(statement: &[Token<'_>], imports: &mut Imports) {
    let Some(first) = statement.first() else {
        return;
    };
    if first.is_name("from") {
        let Some(module) = statement.get(1) else {
            return;
        };
        if !module.is_name("math")
            || !statement
                .get(2)
                .is_some_and(|token| token.is_name("import"))
        {
            return;
        }
        let (names, _) = split_commas(unwrap_parens(&statement[3..]));
        for name in names {
            match name {
                // `from math import *` brings every constant with it.
                [star] if star.is_op("*") => {
                    imports
                        .constants
                        .extend(MATH_CONSTANTS.iter().map(|c| c.to_string()));
                }
                [constant] if MATH_CONSTANTS.contains(&constant.text) => {
                    imports.constants.insert(constant.text.to_string());
                }
                [constant, as_keyword, alias]
                    if as_keyword.is_name("as")
                        && MATH_CONSTANTS.contains(&constant.text)
                        && alias.kind == Kind::Name =>
                {
                    imports.constants.insert(alias.text.to_string());
                }
                _ => {}
            }
        }
    } else if first.is_name("import") {
        let (modules, _) = split_commas(&statement[1..]);
        for module in modules {
            match module {
                [name] if name.is_name("math") => {
                    imports.modules.insert(name.text.to_string());
                }
                [name, as_keyword, alias]
                    if name.is_name("math")
                        && as_keyword.is_name("as")
                        && alias.kind == Kind::Name =>
                {
                    imports.modules.insert(alias.text.to_string());
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(source: &str) -> Vec<String> {
        extract(source)
            .into_iter()
            .map(|parameter| parameter.name)
            .collect()
    }

    fn value(source: &str, name: &str) -> Option<f64> {
        extract(source)
            .into_iter()
            .find(|parameter| parameter.name == name)
            .and_then(|parameter| parameter.value())
    }

    /// A script in the shape the AI writes when it is being careless:
    /// no parameter-block comment, parameters interleaved with modelling
    /// code, and locals that look exactly like parameters.
    const SLOPPY: &str = r#"from build123d import *

plate_width = 80
plate_depth = 40.0
hole_d = 6
margin = plate_width / 4

def bracket(thickness):
    plate_width = 999
    rib = 3
    return thickness

with BuildPart() as part:
    with BuildSketch() as profile:
        hole_d = 12
        Rectangle(plate_width, plate_depth)
    extrude(amount=8)
    fillet(part.edges().filter_by(Axis.Z), radius=2)

label = "plate_width = 5"
offsets = [1, 2, 3]
"#;

    #[test]
    fn indented_bindings_that_shadow_parameters_are_not_parameters() {
        let extracted = extract(SLOPPY);
        assert_eq!(
            names(SLOPPY),
            vec!["plate_width", "plate_depth", "hole_d", "margin"],
            "only names bound in column zero belong in the panel"
        );
        // `plate_width = 999` is a function local. `hole_d = 12` is not:
        // a `with` body binds module names, so Python's own answer for
        // `hole_d` is 12. The panel keeps 6 because a dimension chosen
        // inside a builder block is modelling code, not a parameter.
        assert_eq!(value(SLOPPY, "plate_width"), Some(80.0));
        assert_eq!(value(SLOPPY, "hole_d"), Some(6.0));
        assert!(
            extracted.iter().all(|p| p.name != "rib"),
            "a name bound inside a function is a local"
        );
    }

    #[test]
    fn a_derived_name_is_listed_but_not_editable() {
        let derived = extract(SLOPPY)
            .into_iter()
            .find(|parameter| parameter.name == "margin")
            .expect("a name bound to arithmetic over parameters is still a parameter");
        assert_eq!(derived.value(), None, "a derived name has no value to edit");
        assert_eq!(derived.expression(), Some("plate_width / 4"));
        assert_eq!(
            rewrite(SLOPPY, "margin", 7.0),
            Err(RewriteError::NotEditable("margin".into()))
        );
    }

    #[test]
    fn non_numeric_bindings_are_not_parameters() {
        let listed = names(SLOPPY);
        for name in ["part", "profile", "label", "offsets", "bracket"] {
            assert!(
                !listed.contains(&name.to_string()),
                "{name} is not a number the panel can show"
            );
        }
    }

    #[test]
    fn a_name_rebound_at_module_level_takes_its_last_value() {
        let source = "depth = 10\ndepth = 25\nwidth = 3\n";
        assert_eq!(names(source), vec!["depth", "width"]);
        assert_eq!(value(source, "depth"), Some(25.0));
        // The edit must land on the binding that decides the value.
        assert_eq!(
            rewrite(source, "depth", 4.0).unwrap(),
            "depth = 10\ndepth = 4\nwidth = 3\n"
        );
    }

    #[test]
    fn a_name_rebound_to_a_shape_leaves_the_panel() {
        let source = "size = 10\nsize = Box(1, 2, 3)\n";
        assert!(
            names(source).is_empty(),
            "after the rebinding the name is not a number at run time"
        );
    }

    #[test]
    fn an_edit_changes_one_literal_and_nothing_else() {
        let source = "\
# 8 mm plate, 8 holes
thickness = 8  # 8 by default
spacing = 8
note = \"thickness = 8\"
extrude(amount=8)
";
        let rewritten = rewrite(source, "thickness", 10.0).unwrap();
        assert_eq!(
            rewritten,
            "\
# 8 mm plate, 8 holes
thickness = 10  # 8 by default
spacing = 8
note = \"thickness = 8\"
extrude(amount=8)
"
        );
    }

    #[test]
    fn the_authors_formatting_survives_an_edit() {
        let source = "WIDTH   =   20.5   # wide\nHEIGHT\t=\t3\n";
        assert_eq!(
            rewrite(source, "WIDTH", 21.0).unwrap(),
            "WIDTH   =   21.0   # wide\nHEIGHT\t=\t3\n",
            "a decimal keeps its point and the spacing is the author's"
        );
        assert_eq!(
            rewrite(source, "HEIGHT", 4.0).unwrap(),
            "WIDTH   =   20.5   # wide\nHEIGHT\t=\t4\n",
            "an integer stays an integer"
        );
        assert_eq!(
            rewrite(source, "HEIGHT", 4.5).unwrap(),
            "WIDTH   =   20.5   # wide\nHEIGHT\t=\t4.5\n",
            "unless the new value needs a fraction"
        );
    }

    #[test]
    fn a_commented_out_or_quoted_binding_is_not_a_parameter() {
        let source = "\
# radius = 5
doc = '''
gap = 3
'''
real = 2
";
        assert_eq!(names(source), vec!["real"]);
    }

    #[test]
    fn a_bracketed_expression_does_not_split_into_statements() {
        let source = "\
points = [
(0, 0),
(1, 1),
]
height = 12
";
        assert_eq!(names(source), vec!["height"]);
    }

    #[test]
    fn negative_and_underscored_literals_read_and_write() {
        let source = "offset = -2.5\nbig = 1_000\n";
        assert_eq!(value(source, "offset"), Some(-2.5));
        assert_eq!(value(source, "big"), Some(1000.0));
        assert_eq!(
            rewrite(source, "offset", -3.0).unwrap(),
            "offset = -3.0\nbig = 1_000\n"
        );
    }

    #[test]
    fn comparisons_and_augmented_assignment_are_not_bindings() {
        let source = "total = 1\ntotal += 2\nflag = total == 1\n";
        assert_eq!(names(source), vec!["total"]);
        assert_eq!(value(source, "total"), Some(1.0));
    }

    #[test]
    fn unpacked_names_are_parameters_with_a_value_each() {
        let source = "\
width, depth = 80, 40.0
(inner, outer) = (2, 4)
";
        assert_eq!(names(source), vec!["width", "depth", "inner", "outer"]);
        assert_eq!(value(source, "depth"), Some(40.0));
        assert_eq!(value(source, "outer"), Some(4.0));
        // The edit lands on that name's own element, not the first
        // number of the tuple.
        assert_eq!(
            rewrite(source, "depth", 55.0).unwrap(),
            "width, depth = 80, 55.0\n(inner, outer) = (2, 4)\n"
        );
        assert_eq!(
            rewrite(source, "outer", 5.0).unwrap(),
            "width, depth = 80, 40.0\n(inner, outer) = (2, 5)\n"
        );
    }

    #[test]
    fn a_nested_unpacking_binds_every_name_it_reaches() {
        let source = "\
width, (depth, wall) = 80, (40, 2)
((rib, web), gap) = ((3, 4), 5)
pitch, [rise, run] = 6, [7, 8]
lost, (also, gone) = 1, 2
";
        assert_eq!(
            names(source),
            vec![
                "width", "depth", "wall", "rib", "web", "gap", "pitch", "rise", "run"
            ],
            "a name nested inside a tuple target is still a module-level number"
        );
        assert_eq!(value(source, "wall"), Some(2.0));
        assert_eq!(value(source, "web"), Some(4.0));
        assert_eq!(value(source, "run"), Some(8.0));
        // A nested target against a value that is not a tuple of the
        // same shape cannot be read, so the whole assignment yields
        // nothing rather than a guessed value for `lost`.
        assert_eq!(value(source, "lost"), None);
        // The edit lands on that name's own element, deep in the tuple.
        assert_eq!(
            rewrite(source, "wall", 3.0).unwrap(),
            source.replace("(40, 2)", "(40, 3)")
        );
        assert_eq!(
            rewrite(source, "rib", 9.0).unwrap(),
            source.replace("((3, 4), 5)", "((9, 4), 5)")
        );
    }

    #[test]
    fn an_unpacking_whose_shape_does_not_match_yields_nothing() {
        let source = "\
a, b = size
x, y = corner()
i, j, k = 1, 2
kept = 3
";
        assert_eq!(
            names(source),
            vec!["kept"],
            "a name whose value cannot be known is not shown with a guessed one"
        );
    }

    #[test]
    fn unpacked_locals_are_still_locals() {
        let source = "\
outer = 1

def make():
    inner_a, inner_b = 2, 3
    return inner_a

with BuildPart() as part:
    held_a, held_b = 4, 5
";
        assert_eq!(names(source), vec!["outer"]);
    }

    #[test]
    fn imported_math_constants_make_an_expression_a_parameter() {
        let source = "\
from math import pi, tau
import math as m
import math

sweep = pi / 4
turn = m.tau / 8
half = math.pi * 2
rows = os.cpu_count() / 2
label = other.thing / 2
";
        assert_eq!(names(source), vec!["sweep", "turn", "half"]);
        let extracted = extract(source);
        let sweep = extracted.iter().find(|p| p.name == "sweep").unwrap();
        assert_eq!(sweep.expression(), Some("pi / 4"));
        assert_eq!(sweep.value(), None, "a constant expression is not editable");
    }

    #[test]
    fn a_constant_from_another_module_is_not_assumed_numeric() {
        let source = "\
from settings import WIDTH
plate = WIDTH / 2
real = 4
";
        assert_eq!(
            names(source),
            vec!["real"],
            "an imported name this module cannot know is a number is not a parameter"
        );
    }

    fn lock_of(source: &str, name: &str) -> Option<Lock> {
        extract(source)
            .into_iter()
            .find(|parameter| parameter.name == name)
            .and_then(|parameter| parameter.lock)
    }

    #[test]
    fn a_locked_marker_is_read_with_its_reason() {
        let source = "wall = 2  # locked: must clear the M3 head\n\
                      depth = 40 # LOCKED\n\
                      width = 80  # locker\n\
                      height = (\n    10\n)  # locked\n\
                      gap = 0.4  # mm  # locked\n\
                      free = 1\n";
        assert_eq!(
            lock_of(source, "wall").unwrap().reason.as_deref(),
            Some("must clear the M3 head")
        );
        assert_eq!(lock_of(source, "depth").unwrap().reason, None);
        assert!(
            lock_of(source, "width").is_none(),
            "`locker` is not `locked`"
        );
        assert!(
            lock_of(source, "height").is_some(),
            "the marker ends the statement's last line"
        );
        assert!(
            lock_of(source, "gap").is_some(),
            "a marker after the author's comment"
        );
        assert!(lock_of(source, "free").is_none());
        // A rebinding decides the lock as it decides the value.
        assert!(lock_of("x = 1  # locked\nx = 2\n", "x").is_none());
        assert!(lock_of("x = 1\nx = 2  # locked\n", "x").is_some());
    }

    #[test]
    fn locking_and_unlocking_change_only_the_marker() {
        let source = "width = 80\ndepth = 40  # mm\nheight = width * 2\n";
        let locked = lock(source, "width", None).unwrap();
        assert_eq!(
            locked,
            "width = 80  # locked\ndepth = 40  # mm\nheight = width * 2\n"
        );
        // A reason rides on the marker, flattened to one line.
        let reasoned = lock(&locked, "width", Some(" fits the\nbracket ")).unwrap();
        assert_eq!(
            reasoned,
            "width = 80  # locked: fits the bracket\ndepth = 40  # mm\nheight = width * 2\n"
        );
        // The author's own comment stays; the marker goes after it.
        let depth = lock(&reasoned, "depth", None).unwrap();
        assert!(depth.contains("depth = 40  # mm  # locked\n"), "{depth}");
        // A derived parameter locks too: its expression is the constraint.
        let derived = lock(&depth, "height", None).unwrap();
        assert!(
            derived.contains("height = width * 2  # locked\n"),
            "{derived}"
        );
        // Unlocking removes exactly the marker, reason included.
        let unlocked = unlock(&derived, "width").unwrap();
        assert!(unlocked.starts_with("width = 80\n"), "{unlocked}");
        let unlocked = unlock(&unlocked, "depth").unwrap();
        assert!(unlocked.contains("depth = 40  # mm\n"), "{unlocked}");
        // Unlocking what is not locked changes nothing.
        assert_eq!(unlock(source, "width").unwrap(), source);
        assert_eq!(
            lock(source, "nothing", None),
            Err(RewriteError::NoSuchParameter("nothing".into()))
        );
        // A value edit leaves the lock where it is.
        let edited = rewrite(&reasoned, "width", 90.0).unwrap();
        assert!(
            edited.starts_with("width = 90  # locked: fits the bracket\n"),
            "{edited}"
        );
    }

    #[test]
    fn an_unknown_or_impossible_edit_is_refused() {
        let source = "width = 5\n";
        assert_eq!(
            rewrite(source, "depth", 1.0),
            Err(RewriteError::NoSuchParameter("depth".into()))
        );
        assert!(matches!(
            rewrite(source, "width", f64::NAN),
            Err(RewriteError::NotANumber(_))
        ));
    }

    #[test]
    fn semicolon_joined_statements_are_separate_bindings() {
        let source = "\
width = 80; depth = 40  # the plate's two sizes
note = \"a # b; c\"; gap = 3
tail = 7;
points = [(0, 0), (1, 1)]
";
        assert_eq!(
            names(source),
            vec!["width", "depth", "gap", "tail"],
            "a semicolon joins statements; each one binds its own name"
        );
        assert_eq!(value(source, "depth"), Some(40.0));
        assert_eq!(
            value(source, "gap"),
            Some(3.0),
            "the separator and the hash inside the string above are part of the string, \
             and the apostrophe in the comment above that is part of the comment"
        );
        assert_eq!(
            value(source, "tail"),
            Some(7.0),
            "a trailing semicolon ends a statement rather than starting one"
        );
        assert_eq!(
            rewrite(source, "depth", 45.0).unwrap(),
            "\
width = 80; depth = 45  # the plate's two sizes
note = \"a # b; c\"; gap = 3
tail = 7;
points = [(0, 0), (1, 1)]
",
            "the edit lands on the second statement of the line and nowhere else"
        );
    }

    #[test]
    fn a_continued_line_is_one_statement() {
        let source = "\
width = \\
    80
height = (
    40
)
margin = width \\
    / 4
depth = 5
";
        assert_eq!(
            names(source),
            vec!["width", "height", "margin", "depth"],
            "a statement spread over lines by a backslash or a bracket still binds its name"
        );
        assert_eq!(value(source, "width"), Some(80.0));
        assert_eq!(
            value(source, "height"),
            Some(40.0),
            "a value written inside brackets of its own is still the literal it holds"
        );
        assert_eq!(
            extract(source)
                .into_iter()
                .find(|parameter| parameter.name == "depth")
                .map(|parameter| parameter.line),
            Some(8),
            "the lines a continuation spans are still lines of the file"
        );
        assert_eq!(
            extract(source)
                .into_iter()
                .find(|parameter| parameter.name == "margin")
                .and_then(|parameter| parameter.expression().map(str::to_string)),
            Some("width / 4".to_string()),
            "a continued expression reads as the one line it is"
        );
        assert_eq!(
            rewrite(source, "height", 45.0).unwrap(),
            "\
width = \\
    80
height = (
    45
)
margin = width \\
    / 4
depth = 5
"
        );
    }

    #[test]
    fn annotated_and_chained_bindings_bind_every_name() {
        let source = "\
width: float = 80
depth = height = 40
thickness: int = width
spare: float
";
        assert_eq!(
            names(source),
            vec!["width", "depth", "height", "thickness"],
            "an annotation is not part of the name, a chain binds every target, \
             and a declaration with no value binds nothing"
        );
        assert_eq!(value(source, "width"), Some(80.0));
        assert_eq!(value(source, "depth"), Some(40.0));
        assert_eq!(value(source, "height"), Some(40.0));
        assert_eq!(
            extract(source)
                .into_iter()
                .find(|parameter| parameter.name == "thickness")
                .and_then(|parameter| parameter.expression().map(str::to_string)),
            Some("width".to_string()),
            "an annotated name bound to another parameter is derived from it"
        );
        assert_eq!(
            rewrite(source, "height", 50.0).unwrap(),
            "\
width: float = 80
depth = height = 50
thickness: int = width
spare: float
",
            "a chain shares one literal, so the edit lands on it once"
        );
    }

    #[test]
    fn a_starred_unpacking_binds_the_names_around_the_star() {
        let source = "\
width, *remaining = 80, 40, 20
*lead, tail = 1, 2, 30
first, *middle, last = 5, 6, 7, 8.5
short, *none_left = 9,
Box(width, tail, last)
";
        assert_eq!(
            names(source),
            vec!["width", "tail", "first", "last", "short"],
            "the star absorbs the values left over and the names around it keep their own"
        );
        assert_eq!(value(source, "width"), Some(80.0));
        assert_eq!(value(source, "tail"), Some(30.0));
        assert_eq!(value(source, "first"), Some(5.0));
        assert_eq!(value(source, "last"), Some(8.5));
        assert_eq!(value(source, "short"), Some(9.0));
        for absorbed in ["remaining", "lead", "middle", "none_left"] {
            assert!(
                !names(source).contains(&absorbed.to_string()),
                "{absorbed} is bound to a list of what is left, not to a number"
            );
        }
        // The edit lands on the element the name actually takes, which
        // is counted from the end when the star comes first.
        assert_eq!(
            rewrite(source, "tail", 35.0).unwrap(),
            source.replace("1, 2, 30", "1, 2, 35")
        );
        assert_eq!(
            rewrite(source, "last", 9.5).unwrap(),
            source.replace("6, 7, 8.5", "6, 7, 9.5")
        );
    }

    #[test]
    fn a_target_this_module_cannot_read_does_not_hide_its_neighbour() {
        let source = "\
config.width, keep = 1, 90
holes[0], gap = 2, 7
plain = 3
";
        assert_eq!(
            names(source),
            vec!["keep", "gap", "plain"],
            "an attribute or a subscript binds no parameter and costs its neighbour nothing"
        );
        assert_eq!(value(source, "keep"), Some(90.0));
        assert_eq!(
            rewrite(source, "gap", 8.0).unwrap(),
            "config.width, keep = 1, 90\nholes[0], gap = 2, 8\nplain = 3\n"
        );
    }

    #[test]
    fn a_parenthesised_target_binds_the_name_inside_it() {
        let source = "(width) = 80\n((depth)) = 40\n";
        assert_eq!(names(source), vec!["width", "depth"]);
        assert_eq!(
            rewrite(source, "width", 85.0).unwrap(),
            "(width) = 85\n((depth)) = 40\n"
        );
    }

    #[test]
    fn an_f_string_holds_its_own_quotes_and_braces() {
        let source = "\
label = f\"{sizes[\"plate\"]} mm\"
key = f\"{sizes[\"it's\"]}\"
note = f\"{{literal}} {width:>{pad}}\"
width = 40
";
        assert_eq!(
            names(source),
            vec!["width"],
            "an f-string is one string however it nests quotes and braces, \
             so the binding below it is still read"
        );
        assert_eq!(value(source, "width"), Some(40.0));
    }

    #[test]
    fn a_radix_literal_is_a_number_that_keeps_its_notation() {
        let source = "\
mask = 0xFF
flags = 0b1010
seats = 0o17
";
        assert_eq!(names(source), vec!["mask", "flags", "seats"]);
        assert_eq!(value(source, "mask"), Some(255.0));
        assert_eq!(value(source, "flags"), Some(10.0));
        assert_eq!(value(source, "seats"), Some(15.0));
        assert_eq!(
            rewrite(source, "mask", 190.0).unwrap(),
            source.replace("0xFF", "0xBE"),
            "hex is written back as hex, in the letter case the author used"
        );
        assert_eq!(
            rewrite(source, "flags", 6.0).unwrap(),
            source.replace("0b1010", "0b110")
        );
        assert_eq!(
            rewrite(source, "seats", 2.5).unwrap(),
            source.replace("0o17", "2.5"),
            "a value with no radix spelling takes the plainest one that is true"
        );
    }
}
