//! SNBT (stringified NBT) parser.
//!
//! This implements the subset of Minecraft's SNBT (`TagParser` / `SnbtGrammar`)
//! used by Folia:
//!
//! * compounds (`{key: value, ...}`) with quoted or unquoted keys,
//! * homogeneous lists (`[value, ...]`),
//! * typed arrays (`[B; ...]`, `[I; ...]`, `[L; ...]`),
//! * quoted strings with Java-style escapes and unquoted strings,
//! * `true` / `false` booleans (stored as [`Tag::Byte`]),
//! * byte/short/int/long/float/double numeric literals with their usual
//!   (`b`, `s`, `l`, `f`, `d`) suffixes.
//!
//! Deliberately unsupported syntax: hexadecimal (`0xFF`) and binary
//! (`0b1010`) integer literals, unsigned (`u`) suffixes, `_` digit
//! separators, Mojangson builtin operations (`uuid(...)`, `bool(...)`, ...),
//! and the extended `\s`, `\x`, `\U`, `\N{...}` string escapes. These are
//! accepted by newer vanilla parsers but are not used by the server code being
//! ported.

use crate::nbt::Tag;
use std::fmt;

/// Maximum number of nested compounds, lists and typed arrays accepted by
/// [`parse`]. A value nested 513 containers deep is rejected.
pub const MAX_DEPTH: usize = 512;

/// Error returned when SNBT input cannot be parsed.
///
/// The error carries the byte offset into the input at which parsing failed
/// and a small escaped snippet of the surrounding text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnbtError {
    position: usize,
    message: String,
    context: String,
}

impl SnbtError {
    fn new(input: &str, position: usize, message: impl Into<String>) -> Self {
        let mut position = position.min(input.len());
        while position > 0 && !input.is_char_boundary(position) {
            position -= 1;
        }
        Self {
            position,
            message: message.into(),
            context: context_snippet(input, position),
        }
    }

    /// Byte offset in the original input where parsing failed.
    pub fn position(&self) -> usize {
        self.position
    }

    /// Human-readable description of the failure.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Escaped snippet of the input surrounding [`position`](Self::position).
    pub fn context(&self) -> &str {
        &self.context
    }
}

impl fmt::Display for SnbtError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "SNBT parse error at byte {}: {}",
            self.position, self.message
        )?;
        if !self.context.is_empty() {
            write!(formatter, " (near `{}`)", self.context)?;
        }
        Ok(())
    }
}

impl std::error::Error for SnbtError {}

/// Parses a complete SNBT document into an NBT [`Tag`].
///
/// The whole input must be consumed; trailing non-whitespace data is an error.
/// Both compounds and standalone values (lists, numbers, strings, ...) are
/// accepted, matching Minecraft's `TagParser::parseFully`.
pub fn parse(input: &str) -> Result<Tag, SnbtError> {
    let mut parser = Parser::new(input);
    let value = parser.parse_value(0)?;
    parser.skip_whitespace();
    if parser.peek().is_some() {
        let character = parser.peek().unwrap_or('\0');
        return Err(parser.error(
            parser.position(),
            format!("unexpected trailing input starting with `{character}`"),
        ));
    }
    Ok(value)
}

fn context_snippet(input: &str, position: usize) -> String {
    const BEFORE: usize = 16;
    const AFTER: usize = 32;

    let before_bytes: usize = input[..position]
        .chars()
        .rev()
        .take(BEFORE)
        .map(char::len_utf8)
        .sum();
    let start = position - before_bytes;
    let after_bytes: usize = input[position..]
        .chars()
        .take(AFTER)
        .map(char::len_utf8)
        .sum();
    let end = position + after_bytes;

    let mut snippet = String::new();
    if start > 0 {
        snippet.push('\u{2026}');
    }
    snippet.push_str(&input[start..end]);
    if end < input.len() {
        snippet.push('\u{2026}');
    }

    let mut escaped = String::with_capacity(snippet.len());
    for character in snippet.chars() {
        match character {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\\' => escaped.push_str("\\\\"),
            character if character.is_control() => {
                escaped.push_str(&format!("\\u{{{:x}}}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// One of the three typed-array flavours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayKind {
    Byte,
    Int,
    Long,
}

impl ArrayKind {
    fn name(self) -> &'static str {
        match self {
            Self::Byte => "byte",
            Self::Int => "int",
            Self::Long => "long",
        }
    }

    fn default_integer_type(self) -> IntegerType {
        match self {
            Self::Byte => IntegerType::Byte,
            Self::Int => IntegerType::Int,
            Self::Long => IntegerType::Long,
        }
    }

    /// Whether an explicitly suffixed literal of this type may appear in the
    /// array. Minecraft's grammar restricts the narrower suffixes per array.
    fn allows(self, integer_type: IntegerType) -> bool {
        match self {
            Self::Byte => integer_type == IntegerType::Byte,
            Self::Int => matches!(
                integer_type,
                IntegerType::Byte | IntegerType::Short | IntegerType::Int
            ),
            Self::Long => matches!(
                integer_type,
                IntegerType::Byte | IntegerType::Short | IntegerType::Int | IntegerType::Long
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntegerType {
    Byte,
    Short,
    Int,
    Long,
}

impl IntegerType {
    fn name(self) -> &'static str {
        match self {
            Self::Byte => "byte",
            Self::Short => "short",
            Self::Int => "int",
            Self::Long => "long",
        }
    }

    fn range(self) -> (u128, u128) {
        match self {
            Self::Byte => (128, 127),
            Self::Short => (32_768, 32_767),
            Self::Int => (2_147_483_648, 2_147_483_647),
            Self::Long => (9_223_372_036_854_775_808, 9_223_372_036_854_775_807),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumberSuffix {
    Byte,
    Short,
    Int,
    Long,
    Float,
    Double,
}

impl NumberSuffix {
    fn integer_type(self) -> Option<IntegerType> {
        match self {
            Self::Byte => Some(IntegerType::Byte),
            Self::Short => Some(IntegerType::Short),
            Self::Int => Some(IntegerType::Int),
            Self::Long => Some(IntegerType::Long),
            Self::Float | Self::Double => None,
        }
    }
}

/// A lexed numeric token, split into sign, digits and optional type suffix.
struct NumberParts<'a> {
    token: &'a str,
    negative: bool,
    body: &'a str,
    suffix: Option<NumberSuffix>,
    is_float: bool,
}

impl<'a> NumberParts<'a> {
    fn parse(token: &'a str) -> Result<Self, String> {
        let (negative, rest) = if let Some(rest) = token.strip_prefix('-') {
            (true, rest)
        } else if let Some(rest) = token.strip_prefix('+') {
            (false, rest)
        } else {
            (false, token)
        };

        let (body, suffix) = match rest.chars().next_back() {
            Some('b' | 'B') => (&rest[..rest.len() - 1], Some(NumberSuffix::Byte)),
            Some('s' | 'S') => (&rest[..rest.len() - 1], Some(NumberSuffix::Short)),
            Some('i' | 'I') => (&rest[..rest.len() - 1], Some(NumberSuffix::Int)),
            Some('l' | 'L') => (&rest[..rest.len() - 1], Some(NumberSuffix::Long)),
            Some('f' | 'F') => (&rest[..rest.len() - 1], Some(NumberSuffix::Float)),
            Some('d' | 'D') => (&rest[..rest.len() - 1], Some(NumberSuffix::Double)),
            _ => (rest, None),
        };

        if body.is_empty() {
            return Err(format!("invalid number `{token}`: missing digits"));
        }

        let is_float = matches!(suffix, Some(NumberSuffix::Float | NumberSuffix::Double))
            || body.contains('.')
            || body.contains('e')
            || body.contains('E');

        if is_float
            && matches!(
                suffix,
                Some(
                    NumberSuffix::Byte
                        | NumberSuffix::Short
                        | NumberSuffix::Int
                        | NumberSuffix::Long
                )
            )
        {
            return Err(format!(
                "invalid number `{token}`: an integer suffix cannot be used on a floating-point literal"
            ));
        }

        Ok(Self {
            token,
            negative,
            body,
            suffix,
            is_float,
        })
    }

    /// Parses the token as a signed integer of the requested type, returning
    /// the value widened to `i128` (range-checked against `target`).
    fn parse_integer(&self, target: IntegerType) -> Result<i128, String> {
        if self.is_float {
            return Err(format!(
                "invalid number `{}`: expected an integer",
                self.token
            ));
        }
        if !self.body.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "invalid number `{}`: expected decimal digits",
                self.token
            ));
        }
        if self.body.len() > 1 && self.body.starts_with('0') {
            return Err(format!(
                "invalid number `{}`: leading zeros are not allowed",
                self.token
            ));
        }

        let magnitude: u128 = self.body.parse().map_err(|_| {
            format!(
                "number `{}` is out of range for {}",
                self.token,
                target.name()
            )
        })?;
        let (minimum_magnitude, maximum_magnitude) = target.range();

        let value = if self.negative {
            if magnitude > minimum_magnitude {
                return Err(format!(
                    "number `{}` is out of range for {}",
                    self.token,
                    target.name()
                ));
            }
            -(magnitude as i128)
        } else {
            if magnitude > maximum_magnitude {
                return Err(format!(
                    "number `{}` is out of range for {}",
                    self.token,
                    target.name()
                ));
            }
            magnitude as i128
        };
        Ok(value)
    }

    fn parse_f32(&self) -> Result<f32, String> {
        let canonical = self.canonical_float()?;
        let value: f32 = canonical
            .parse()
            .map_err(|_| format!("invalid floating-point literal `{}`", self.token))?;
        if !value.is_finite() {
            return Err(format!(
                "floating-point literal `{}` is not finite",
                self.token
            ));
        }
        Ok(value)
    }

    fn parse_f64(&self) -> Result<f64, String> {
        let canonical = self.canonical_float()?;
        let value: f64 = canonical
            .parse()
            .map_err(|_| format!("invalid floating-point literal `{}`", self.token))?;
        if !value.is_finite() {
            return Err(format!(
                "floating-point literal `{}` is not finite",
                self.token
            ));
        }
        Ok(value)
    }

    /// Validates the Java-style float grammar (`1.`, `.5`, `1e-3`, `1.e5`, ...)
    /// and returns a canonical string that Rust's float parser accepts.
    fn canonical_float(&self) -> Result<String, String> {
        let invalid = || format!("invalid floating-point literal `{}`", self.token);

        let (mantissa, exponent) = match self.body.find(['e', 'E']) {
            Some(index) => {
                let exponent = &self.body[index + 1..];
                let digits = if let Some(rest) = exponent.strip_prefix('+') {
                    rest
                } else if let Some(rest) = exponent.strip_prefix('-') {
                    rest
                } else {
                    exponent
                };
                if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(invalid());
                }
                (&self.body[..index], Some(exponent))
            }
            None => (self.body, None),
        };

        let (whole, fraction) = match mantissa.find('.') {
            Some(index) => {
                let fraction = &mantissa[index + 1..];
                if fraction.contains('.') {
                    return Err(invalid());
                }
                (&mantissa[..index], Some(fraction))
            }
            None => (mantissa, None),
        };

        if whole.is_empty() && fraction.is_none_or(|fraction| fraction.is_empty()) {
            return Err(invalid());
        }
        if !whole.is_empty() && !whole.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        if fraction.is_some_and(|fraction| !fraction.bytes().all(|byte| byte.is_ascii_digit())) {
            return Err(invalid());
        }

        let whole = if whole.is_empty() { "0" } else { whole };
        let fraction = match fraction {
            Some(fraction) if !fraction.is_empty() => fraction,
            _ => "0",
        };
        let exponent = exponent.unwrap_or("0");
        let sign = if self.negative { "-" } else { "" };
        Ok(format!("{sign}{whole}.{fraction}e{exponent}"))
    }
}

fn is_number_start(character: char) -> bool {
    matches!(character, '+' | '-' | '.' | '0'..='9')
}

/// Characters accepted in a Brigadier-style unquoted string.
fn is_unquoted_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '+')
}

fn tag_type_name(tag: &Tag) -> &'static str {
    match tag {
        Tag::End => "End",
        Tag::Byte(_) => "Byte",
        Tag::Short(_) => "Short",
        Tag::Int(_) => "Int",
        Tag::Long(_) => "Long",
        Tag::Float(_) => "Float",
        Tag::Double(_) => "Double",
        Tag::ByteArray(_) => "ByteArray",
        Tag::String(_) => "String",
        Tag::List(_) => "List",
        Tag::Compound(_) => "Compound",
        Tag::IntArray(_) => "IntArray",
        Tag::LongArray(_) => "LongArray",
    }
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn position(&self) -> usize {
        self.pos
    }

    fn error(&self, position: usize, message: impl Into<String>) -> SnbtError {
        SnbtError::new(self.input, position, message)
    }

    fn peek(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let character = self.peek()?;
        self.pos += character.len_utf8();
        Some(character)
    }

    fn skip_whitespace(&mut self) {
        while let Some(character) = self.peek() {
            if character.is_whitespace() {
                self.pos += character.len_utf8();
            } else {
                break;
            }
        }
    }

    fn read_unquoted(&mut self) -> &'a str {
        let start = self.pos;
        while let Some(character) = self.peek() {
            if is_unquoted_char(character) {
                self.pos += character.len_utf8();
            } else {
                break;
            }
        }
        &self.input[start..self.pos]
    }

    fn check_depth(&self, depth: usize) -> Result<(), SnbtError> {
        if depth >= MAX_DEPTH {
            return Err(self.error(
                self.pos,
                format!("maximum nesting depth of {MAX_DEPTH} exceeded"),
            ));
        }
        Ok(())
    }

    fn parse_value(&mut self, depth: usize) -> Result<Tag, SnbtError> {
        self.skip_whitespace();
        let Some(character) = self.peek() else {
            return Err(self.error(self.pos, "expected a value, found end of input"));
        };

        match character {
            '{' => {
                self.check_depth(depth)?;
                self.parse_compound(depth)
            }
            '[' => {
                self.check_depth(depth)?;
                self.parse_list_or_array(depth)
            }
            '"' | '\'' => self.parse_quoted().map(Tag::String),
            character if is_number_start(character) => self.parse_number(),
            character if is_unquoted_char(character) => self.parse_unquoted_value(),
            other => Err(self.error(
                self.pos,
                format!("expected a value, found unexpected character `{other}`"),
            )),
        }
    }

    fn parse_unquoted_value(&mut self) -> Result<Tag, SnbtError> {
        let value = self.read_unquoted();
        if value.is_empty() {
            let character = self.peek().unwrap_or('\0');
            return Err(self.error(
                self.pos,
                format!("expected a value, found unexpected character `{character}`"),
            ));
        }

        // Vanilla treats booleans case-insensitively and as byte tags.
        if value.eq_ignore_ascii_case("true") {
            Ok(Tag::Byte(1))
        } else if value.eq_ignore_ascii_case("false") {
            Ok(Tag::Byte(0))
        } else {
            Ok(Tag::String(value.to_owned()))
        }
    }

    fn parse_number(&mut self) -> Result<Tag, SnbtError> {
        let start = self.pos;
        let token = self.read_unquoted();
        let parts = NumberParts::parse(token).map_err(|message| self.error(start, message))?;

        if parts.is_float {
            if matches!(parts.suffix, Some(NumberSuffix::Float)) {
                let value = parts
                    .parse_f32()
                    .map_err(|message| self.error(start, message))?;
                Ok(Tag::Float(value))
            } else {
                let value = parts
                    .parse_f64()
                    .map_err(|message| self.error(start, message))?;
                Ok(Tag::Double(value))
            }
        } else {
            let target = parts
                .suffix
                .and_then(NumberSuffix::integer_type)
                .unwrap_or(IntegerType::Int);
            let value = parts
                .parse_integer(target)
                .map_err(|message| self.error(start, message))?;

            Ok(match target {
                IntegerType::Byte => Tag::Byte(value as i8),
                IntegerType::Short => Tag::Short(value as i16),
                IntegerType::Int => Tag::Int(value as i32),
                IntegerType::Long => Tag::Long(value as i64),
            })
        }
    }

    fn parse_key(&mut self) -> Result<String, SnbtError> {
        self.skip_whitespace();
        let start = self.pos;
        match self.peek() {
            Some('"' | '\'') => {
                let key = self.parse_quoted()?;
                if key.is_empty() {
                    return Err(self.error(start, "compound keys must not be empty"));
                }
                Ok(key)
            }
            _ => {
                let key = self.read_unquoted();
                if key.is_empty() {
                    return Err(self.error(self.pos, "expected a compound key"));
                }
                Ok(key.to_owned())
            }
        }
    }

    fn parse_compound(&mut self, depth: usize) -> Result<Tag, SnbtError> {
        self.bump(); // '{'
        let mut entries: Vec<(String, Tag)> = Vec::new();

        self.skip_whitespace();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(Tag::Compound(entries));
        }

        loop {
            let key = self.parse_key()?;
            self.skip_whitespace();
            match self.peek() {
                Some(':') => {
                    self.bump();
                }
                Some(character) => {
                    return Err(self.error(
                        self.pos,
                        format!("expected `:` after compound key, found `{character}`"),
                    ));
                }
                None => {
                    return Err(self.error(
                        self.pos,
                        "expected `:` after compound key, found end of input",
                    ));
                }
            }

            let value = self.parse_value(depth + 1)?;

            // Vanilla keeps the last value for duplicate keys while preserving
            // the position of the first occurrence.
            if let Some((_, slot)) = entries.iter_mut().find(|(existing, _)| existing == &key) {
                *slot = value;
            } else {
                entries.push((key, value));
            }

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    self.skip_whitespace();
                    match self.peek() {
                        Some('}') => {
                            self.bump();
                            break;
                        }
                        None => {
                            return Err(self.error(
                                self.pos,
                                "unclosed compound: expected `}` before end of input",
                            ));
                        }
                        _ => {}
                    }
                }
                Some('}') => {
                    self.bump();
                    break;
                }
                Some(character) => {
                    return Err(self.error(
                        self.pos,
                        format!("expected `,` or `}}` in compound, found `{character}`"),
                    ));
                }
                None => {
                    return Err(self.error(
                        self.pos,
                        "unclosed compound: expected `}` before end of input",
                    ));
                }
            }
        }

        Ok(Tag::Compound(entries))
    }

    /// Returns the typed-array kind if the cursor (just after `[`) is followed
    /// by `B;`, `I;` or `L;` (whitespace allowed before the semicolon).
    fn typed_array_kind(&self) -> Option<ArrayKind> {
        let mut characters = self.input[self.pos..].char_indices();
        let (_, first) = characters.next()?;
        let kind = match first {
            'B' => ArrayKind::Byte,
            'I' => ArrayKind::Int,
            'L' => ArrayKind::Long,
            _ => return None,
        };

        for (_, character) in characters {
            if character.is_whitespace() {
                continue;
            }
            return if character == ';' { Some(kind) } else { None };
        }
        None
    }

    fn unclosed_list_error(&self) -> SnbtError {
        self.error(self.pos, "unclosed list: expected `]` before end of input")
    }

    fn parse_list_or_array(&mut self, depth: usize) -> Result<Tag, SnbtError> {
        self.bump(); // '['
        self.skip_whitespace();

        if let Some(kind) = self.typed_array_kind() {
            return self.parse_typed_array(kind);
        }

        if self.peek() == Some(']') {
            self.bump();
            return Ok(Tag::List(Vec::new()));
        }

        let mut items: Vec<Tag> = Vec::new();
        let mut element_type: Option<(u8, &'static str)> = None;

        loop {
            let element_position = self.pos;
            let value = self.parse_value(depth + 1)?;

            match element_type {
                Some((id, name)) if id != value.id() => {
                    return Err(self.error(
                        element_position,
                        format!(
                            "mixed-type list: expected {name} (tag id {id}), found {} (tag id {})",
                            tag_type_name(&value),
                            value.id()
                        ),
                    ));
                }
                None => element_type = Some((value.id(), tag_type_name(&value))),
                _ => {}
            }
            items.push(value);

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    self.skip_whitespace();
                    match self.peek() {
                        Some(']') => {
                            self.bump();
                            break;
                        }
                        None => return Err(self.unclosed_list_error()),
                        _ => {}
                    }
                }
                Some(']') => {
                    self.bump();
                    break;
                }
                Some(character) => {
                    return Err(self.error(
                        self.pos,
                        format!("expected `,` or `]` in list, found `{character}`"),
                    ));
                }
                None => return Err(self.unclosed_list_error()),
            }
        }

        Ok(Tag::List(items))
    }

    fn parse_typed_array(&mut self, kind: ArrayKind) -> Result<Tag, SnbtError> {
        self.bump(); // 'B' | 'I' | 'L'
        self.skip_whitespace();
        if self.peek() != Some(';') {
            return Err(self.error(self.pos, "expected `;` after typed array prefix"));
        }
        self.bump(); // ';'

        let mut values: Vec<i128> = Vec::new();

        loop {
            self.skip_whitespace();
            match self.peek() {
                Some(']') => {
                    self.bump();
                    break;
                }
                None => {
                    return Err(self.error(
                        self.pos,
                        format!(
                            "unclosed {} array: expected `]` before end of input",
                            kind.name()
                        ),
                    ));
                }
                _ => {}
            }

            let element_position = self.pos;
            let character = self.peek().unwrap_or('\0');
            if !is_number_start(character) {
                return Err(self.error(
                    self.pos,
                    format!(
                        "expected an integer literal in {} array, found `{character}`",
                        kind.name()
                    ),
                ));
            }

            let token = self.read_unquoted();
            let parts = NumberParts::parse(token)
                .map_err(|message| self.error(element_position, message))?;

            if parts.is_float {
                return Err(self.error(
                    element_position,
                    format!(
                        "invalid {} array element `{token}`: expected an integer",
                        kind.name()
                    ),
                ));
            }

            let target = match parts.suffix {
                None => kind.default_integer_type(),
                Some(suffix) => match suffix.integer_type() {
                    Some(integer_type) if kind.allows(integer_type) => integer_type,
                    _ => {
                        return Err(self.error(
                            element_position,
                            format!(
                                "invalid element type in `{token}` for {} array",
                                kind.name()
                            ),
                        ));
                    }
                },
            };

            let value = parts
                .parse_integer(target)
                .map_err(|message| self.error(element_position, message))?;
            values.push(value);

            self.skip_whitespace();
            match self.peek() {
                Some(',') => {
                    self.bump();
                    self.skip_whitespace();
                    match self.peek() {
                        Some(']') => {
                            self.bump();
                            break;
                        }
                        None => {
                            return Err(self.error(
                                self.pos,
                                format!(
                                    "unclosed {} array: expected `]` before end of input",
                                    kind.name()
                                ),
                            ));
                        }
                        _ => {}
                    }
                }
                Some(']') => {
                    self.bump();
                    break;
                }
                Some(character) => {
                    return Err(self.error(
                        self.pos,
                        format!(
                            "expected `,` or `]` in {} array, found `{character}`",
                            kind.name()
                        ),
                    ));
                }
                None => {
                    return Err(self.error(
                        self.pos,
                        format!(
                            "unclosed {} array: expected `]` before end of input",
                            kind.name()
                        ),
                    ));
                }
            }
        }

        Ok(match kind {
            ArrayKind::Byte => {
                Tag::ByteArray(values.into_iter().map(|value| value as i8 as u8).collect())
            }
            ArrayKind::Int => Tag::IntArray(values.into_iter().map(|value| value as i32).collect()),
            ArrayKind::Long => {
                Tag::LongArray(values.into_iter().map(|value| value as i64).collect())
            }
        })
    }

    fn parse_quoted(&mut self) -> Result<String, SnbtError> {
        let open = self.pos;
        let quote = self.bump().unwrap_or('"');
        let mut value = String::new();

        loop {
            match self.peek() {
                None => {
                    return Err(self.error(
                        self.pos,
                        format!(
                            "unclosed string: missing closing `{quote}` for string opened at byte {open}"
                        ),
                    ));
                }
                Some(character) if character == quote => {
                    self.bump();
                    return Ok(value);
                }
                Some('\\') => {
                    let escape_position = self.pos;
                    self.bump();
                    self.parse_escape(&mut value, escape_position)?;
                }
                Some(character) => {
                    value.push(character);
                    self.bump();
                }
            }
        }
    }

    fn parse_escape(
        &mut self,
        value: &mut String,
        escape_position: usize,
    ) -> Result<(), SnbtError> {
        let Some(escaped) = self.bump() else {
            return Err(self.error(self.pos, "unterminated escape sequence at end of input"));
        };

        match escaped {
            'n' => value.push('\n'),
            't' => value.push('\t'),
            'r' => value.push('\r'),
            'b' => value.push('\u{8}'),
            'f' => value.push('\u{c}'),
            '\\' => value.push('\\'),
            '"' => value.push('"'),
            '\'' => value.push('\''),
            'u' => self.parse_unicode_escape(value, escape_position)?,
            other => {
                return Err(self.error(
                    escape_position,
                    format!("invalid escape sequence `\\{other}`"),
                ));
            }
        }
        Ok(())
    }

    fn read_hex_quad(&mut self, escape_position: usize) -> Result<u32, SnbtError> {
        let start = self.pos;
        for _ in 0..4 {
            match self.peek() {
                Some(character) if character.is_ascii_hexdigit() => {
                    self.bump();
                }
                Some(character) => {
                    return Err(self.error(
                        self.pos,
                        format!(
                            "invalid unicode escape: expected 4 hexadecimal digits, found `{character}`"
                        ),
                    ));
                }
                None => {
                    return Err(self.error(
                        self.pos,
                        "unterminated unicode escape: expected 4 hexadecimal digits",
                    ));
                }
            }
        }

        u32::from_str_radix(&self.input[start..self.pos], 16)
            .map_err(|_| self.error(escape_position, "invalid unicode escape"))
    }

    fn parse_unicode_escape(
        &mut self,
        value: &mut String,
        escape_position: usize,
    ) -> Result<(), SnbtError> {
        let first = self.read_hex_quad(escape_position)?;

        if (0xD800..=0xDBFF).contains(&first) {
            // Rust strings cannot hold lone UTF-16 surrogates; combine a valid
            // pair (`\uD83D\uDE00`) into its Unicode scalar value instead.
            if !self.input[self.pos..].starts_with("\\u") {
                return Err(self.error(
                    escape_position,
                    "unicode escape contains an unpaired high surrogate",
                ));
            }
            self.pos += 2; // skip `\u`
            let second = self.read_hex_quad(escape_position)?;
            if !(0xDC00..=0xDFFF).contains(&second) {
                return Err(self.error(
                    escape_position,
                    "unicode escape contains an invalid surrogate pair",
                ));
            }
            let code_point = 0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00);
            value.push(
                char::from_u32(code_point)
                    .expect("a valid surrogate pair always yields a Unicode scalar value"),
            );
        } else if (0xDC00..=0xDFFF).contains(&first) {
            return Err(self.error(
                escape_position,
                "unicode escape contains an unpaired low surrogate",
            ));
        } else {
            value.push(char::from_u32(first).expect("a non-surrogate u16 is always a valid char"));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::{read_root, write_root_to_vec};

    #[test]
    fn round_trips_all_tag_types() {
        let input = r#"{
            byte: -7b,
            short: 1234s,
            int: -123456,
            long: 9000000000L,
            float: 1.25f,
            double: -3.5d,
            byte_array: [B; 1b, 2b, 3b, -1b],
            string: "hello SNBT",
            list: [1, 2, 3],
            nested: {name: "Folia"},
            int_array: [I; 1, -2, 3],
            long_array: [L; 1L, -2l, 9223372036854775807L]
        }"#;

        let expected = Tag::Compound(vec![
            ("byte".into(), Tag::Byte(-7)),
            ("short".into(), Tag::Short(1234)),
            ("int".into(), Tag::Int(-123456)),
            ("long".into(), Tag::Long(9_000_000_000)),
            ("float".into(), Tag::Float(1.25)),
            ("double".into(), Tag::Double(-3.5)),
            ("byte_array".into(), Tag::ByteArray(vec![1, 2, 3, 255])),
            ("string".into(), Tag::String("hello SNBT".into())),
            (
                "list".into(),
                Tag::List(vec![Tag::Int(1), Tag::Int(2), Tag::Int(3)]),
            ),
            (
                "nested".into(),
                Tag::Compound(vec![("name".into(), Tag::String("Folia".into()))]),
            ),
            ("int_array".into(), Tag::IntArray(vec![1, -2, 3])),
            ("long_array".into(), Tag::LongArray(vec![1, -2, i64::MAX])),
        ]);

        let parsed = parse(input).expect("SNBT should parse");
        assert_eq!(parsed, expected);

        let encoded = write_root_to_vec(&parsed).expect("NBT encoding should succeed");
        let decoded = read_root(&mut encoded.as_slice()).expect("NBT decoding should succeed");
        assert_eq!(decoded, parsed);
        assert_eq!(decoded, expected);
    }

    #[test]
    fn quoted_keys_escapes_and_unicode() {
        let input = r#"{
            "quoted key": "line\n\t\r\b\f\\\"end",
            'single key': 'it\'s fine',
            "uni\u0063ode": "\u0041\u00e9\ud83d\ude00"
        }"#;

        let expected = Tag::Compound(vec![
            (
                "quoted key".into(),
                Tag::String("line\n\t\r\u{8}\u{c}\\\"end".into()),
            ),
            ("single key".into(), Tag::String("it's fine".into())),
            ("unicode".into(), Tag::String("A\u{e9}\u{1f600}".into())),
        ]);

        assert_eq!(parse(input).unwrap(), expected);
    }

    #[test]
    fn typed_arrays() {
        assert_eq!(
            parse("[B; 1b, -2B, 3, -1b]").unwrap(),
            Tag::ByteArray(vec![1, 254, 3, 255])
        );
        assert_eq!(
            parse("[I; 1b, 2s, -3, 4]").unwrap(),
            Tag::IntArray(vec![1, 2, -3, 4])
        );
        assert_eq!(
            parse("[L; 1b, 2s, 3, 4L, -9223372036854775808L]").unwrap(),
            Tag::LongArray(vec![1, 2, 3, 4, i64::MIN])
        );
        assert_eq!(parse("[B;]").unwrap(), Tag::ByteArray(Vec::new()));
        assert_eq!(parse("[I; ]").unwrap(), Tag::IntArray(Vec::new()));
        assert_eq!(parse("[ L ; ]").unwrap(), Tag::LongArray(Vec::new()));
        assert_eq!(parse("[]").unwrap(), Tag::List(Vec::new()));

        // Suffix compatibility and element ranges follow the vanilla grammar.
        assert!(parse("[B; 1s]").is_err());
        assert!(parse("[I; 1L]").is_err());
        assert!(parse("[L; 1.0]").is_err());
        assert!(parse("[B; 128]").is_err());
        assert!(parse("[B; 1, 2b]").is_ok());
        assert!(parse("[I; 1, 2f]").is_err());
    }

    #[test]
    fn booleans_and_numeric_suffixes() {
        let tag = parse(
            "{t: true, f: false, upper: TRUE, b: 1b, B: -2B, s: 3s, S: -4S, i: 5, plus: +6, l: 7L, L: -8l, fl: 1.5f, dbl: 2.5D, sci: 1e3, dot: .5, trailing_dot: 5.}",
        )
        .unwrap();

        let expected = Tag::Compound(vec![
            ("t".into(), Tag::Byte(1)),
            ("f".into(), Tag::Byte(0)),
            ("upper".into(), Tag::Byte(1)),
            ("b".into(), Tag::Byte(1)),
            ("B".into(), Tag::Byte(-2)),
            ("s".into(), Tag::Short(3)),
            ("S".into(), Tag::Short(-4)),
            ("i".into(), Tag::Int(5)),
            ("plus".into(), Tag::Int(6)),
            ("l".into(), Tag::Long(7)),
            ("L".into(), Tag::Long(-8)),
            ("fl".into(), Tag::Float(1.5)),
            ("dbl".into(), Tag::Double(2.5)),
            ("sci".into(), Tag::Double(1000.0)),
            ("dot".into(), Tag::Double(0.5)),
            ("trailing_dot".into(), Tag::Double(5.0)),
        ]);

        assert_eq!(tag, expected);
        assert_eq!(parse("true").unwrap(), Tag::Byte(1));
        assert_eq!(parse("false").unwrap(), Tag::Byte(0));
    }

    #[test]
    fn integer_limits_are_enforced() {
        assert_eq!(parse("127b").unwrap(), Tag::Byte(127));
        assert_eq!(parse("-128b").unwrap(), Tag::Byte(-128));
        assert_eq!(parse("32767s").unwrap(), Tag::Short(32767));
        assert_eq!(parse("-32768s").unwrap(), Tag::Short(-32768));
        assert_eq!(parse("2147483647").unwrap(), Tag::Int(i32::MAX));
        assert_eq!(parse("-2147483648").unwrap(), Tag::Int(i32::MIN));
        assert_eq!(parse("9223372036854775807L").unwrap(), Tag::Long(i64::MAX));
        assert_eq!(parse("-9223372036854775808L").unwrap(), Tag::Long(i64::MIN));

        for invalid in [
            "128b",
            "-129b",
            "32768s",
            "-32769s",
            "2147483648",
            "-2147483649",
            "9223372036854775808L",
            "-9223372036854775809L",
            "1e400",
            "1e39f",
            "1e",
            "0x10",
            "01",
            "--1",
            ".",
            "+",
        ] {
            assert!(parse(invalid).is_err(), "`{invalid}` should be rejected");
        }
    }

    #[test]
    fn rejects_mixed_type_lists() {
        let error = parse("[1, 2b]").unwrap_err();
        assert!(error.to_string().contains("mixed-type list"), "{error}");

        assert!(parse("[{a: 1}, [1, 2]]").is_err());
        assert!(parse("[1, \"two\"]").is_err());
        assert!(parse("[true, false]").is_ok()); // both are Tag::Byte
    }

    #[test]
    fn rejects_trailing_input() {
        for input in ["{a: 1} extra", "1 2", "{} {}", "\"a\" \"b\""] {
            assert!(parse(input).is_err(), "`{input}` should be rejected");
        }
    }

    #[test]
    fn rejects_unclosed_constructs() {
        for input in [
            "{a: 1", "[1, 2", "\"abc", "{a: [1}", "{a: \"x}", "[B; 1, 2", "{a: 1,", "[1,",
        ] {
            assert!(parse(input).is_err(), "`{input}` should be rejected");
        }
    }

    #[test]
    fn rejects_invalid_escapes() {
        for input in [
            r#""\q""#,
            r#""\x41""#,
            r#""\u12""#,
            r#""\uZZZZ""#,
            r#""\uD800""#,
            r#""\uDC00""#,
            r#""\uD800\u0041""#,
            r#""abc\"#,
        ] {
            assert!(parse(input).is_err(), "`{input}` should be rejected");
        }
    }

    #[test]
    fn enforces_depth_limit() {
        fn nested(depth: usize) -> String {
            let mut input = String::with_capacity(depth * 4 + 1);
            for _ in 0..depth {
                input.push_str("{a:");
            }
            input.push('1');
            for _ in 0..depth {
                input.push('}');
            }
            input
        }

        assert!(parse(&nested(MAX_DEPTH)).is_ok());
        let error = parse(&nested(MAX_DEPTH + 1)).unwrap_err();
        assert!(error.to_string().contains("nesting depth"), "{error}");
        assert!(error.position() < nested(MAX_DEPTH + 1).len());
    }

    #[test]
    fn compound_keys_and_duplicates() {
        assert_eq!(
            parse("{1: 2, foo-bar_2.0: 3, \"quoted:key\": 4}").unwrap(),
            Tag::Compound(vec![
                ("1".into(), Tag::Int(2)),
                ("foo-bar_2.0".into(), Tag::Int(3)),
                ("quoted:key".into(), Tag::Int(4)),
            ])
        );

        // Vanilla keeps the last value for a duplicate key.
        assert_eq!(
            parse("{a: 1, b: 2, a: 3}").unwrap(),
            Tag::Compound(vec![("a".into(), Tag::Int(3)), ("b".into(), Tag::Int(2)),])
        );

        assert!(parse("{: 1}").is_err());
        assert!(parse("{\"\": 1}").is_err());
    }

    #[test]
    fn errors_report_position_and_context() {
        let error = parse("{a: 1, b @ 2}").unwrap_err();
        assert!(error.position() > 0);
        assert!(error.position() <= "{a: 1, b @ 2}".len());
        assert!(error.to_string().contains("byte"));
        assert!(!error.context().is_empty());

        // Positions are byte offsets, not character offsets.
        let unicode = "{\"\u{e9}\": 1} x";
        let unicode_error = parse(unicode).unwrap_err();
        assert_eq!(unicode_error.position(), unicode.find('x').unwrap());

        fn assert_error<E: std::error::Error>() {}
        assert_error::<SnbtError>();
    }
}
