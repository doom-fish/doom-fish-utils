#![allow(clippy::missing_errors_doc)]

use std::collections::BTreeMap;
use std::fmt::{self, Write};

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

impl JsonValue {
    pub fn field<T: FromJson>(&self, key: &str) -> Result<T, JsonError> {
        let Self::Object(object) = self else {
            return Err(JsonError("expected an object".into()));
        };
        object.get(key).map_or_else(
            || T::from_json(&Self::Null).map_err(|_| JsonError(format!("missing field `{key}`"))),
            T::from_json,
        )
    }
}

impl fmt::Display for JsonValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(formatter, "{value}"),
            Self::Integer(value) => write!(formatter, "{value}"),
            Self::Float(value) if value.is_finite() => write!(formatter, "{value:?}"),
            Self::Null | Self::Float(_) => formatter.write_str("null"),
            Self::String(value) => write_string(value, formatter),
            Self::Array(values) => {
                formatter.write_str("[")?;
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(",")?;
                    }
                    write!(formatter, "{value}")?;
                }
                formatter.write_str("]")
            }
            Self::Object(values) => {
                formatter.write_str("{")?;
                for (index, (key, value)) in values.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(",")?;
                    }
                    write_string(key, formatter)?;
                    write!(formatter, ":{value}")?;
                }
                formatter.write_str("}")
            }
        }
    }
}

fn write_string(text: &str, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    formatter.write_str("\"")?;
    for character in text.chars() {
        match character {
            '"' => formatter.write_str("\\\"")?,
            '\\' => formatter.write_str("\\\\")?,
            '\u{08}' => formatter.write_str("\\b")?,
            '\u{0c}' => formatter.write_str("\\f")?,
            '\n' => formatter.write_str("\\n")?,
            '\r' => formatter.write_str("\\r")?,
            '\t' => formatter.write_str("\\t")?,
            '\u{00}'..='\u{1f}' => write!(formatter, "\\u{:04x}", u32::from(character))?,
            _ => formatter.write_char(character)?,
        }
    }
    formatter.write_str("\"")
}

#[derive(Debug)]
pub struct JsonError(String);

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for JsonError {}

pub trait FromJson: Sized {
    fn from_json(value: &JsonValue) -> Result<Self, JsonError>;
}

impl FromJson for JsonValue {
    fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
        Ok(value.clone())
    }
}

impl FromJson for bool {
    fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
        match value {
            JsonValue::Bool(value) => Ok(*value),
            _ => Err(JsonError("expected a boolean".into())),
        }
    }
}

macro_rules! integer_from_json {
    ($($type:ty),+ $(,)?) => {
        $(
            impl FromJson for $type {
                fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
                    match value {
                        JsonValue::Integer(value) => Self::try_from(*value).map_err(|_| {
                            JsonError(concat!("integer out of range for ", stringify!($type)).into())
                        }),
                        _ => Err(JsonError(concat!("expected an integer for ", stringify!($type)).into())),
                    }
                }
            }
        )+
    };
}

integer_from_json!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

impl FromJson for f64 {
    #[allow(clippy::cast_precision_loss)]
    fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
        match value {
            JsonValue::Integer(value) => Ok(*value as Self),
            JsonValue::Float(value) => Ok(*value),
            _ => Err(JsonError("expected a number".into())),
        }
    }
}

impl FromJson for f32 {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
        match value {
            JsonValue::Integer(value) => Ok(*value as Self),
            JsonValue::Float(value) => Ok(*value as Self),
            _ => Err(JsonError("expected a number".into())),
        }
    }
}

impl FromJson for String {
    fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
        match value {
            JsonValue::String(value) => Ok(value.clone()),
            _ => Err(JsonError("expected a string".into())),
        }
    }
}

impl<T: FromJson> FromJson for Option<T> {
    fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
        match value {
            JsonValue::Null => Ok(None),
            _ => T::from_json(value).map(Some),
        }
    }
}

impl<T: FromJson> FromJson for Vec<T> {
    fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
        match value {
            JsonValue::Array(values) => values.iter().map(T::from_json).collect(),
            _ => Err(JsonError("expected an array".into())),
        }
    }
}

impl<T: FromJson> FromJson for BTreeMap<String, T> {
    fn from_json(value: &JsonValue) -> Result<Self, JsonError> {
        match value {
            JsonValue::Object(values) => values
                .iter()
                .map(|(key, value)| T::from_json(value).map(|value| (key.clone(), value)))
                .collect(),
            _ => Err(JsonError("expected an object".into())),
        }
    }
}

pub fn parse(text: &str) -> Result<JsonValue, JsonError> {
    let mut reader = Reader { text, position: 0 };
    let value = reader.value(0)?;
    reader.whitespace();
    if reader.position != text.len() {
        return Err(reader.error("unexpected trailing data"));
    }
    Ok(value)
}

struct Reader<'a> {
    text: &'a str,
    position: usize,
}

impl Reader<'_> {
    fn error(&self, message: &str) -> JsonError {
        JsonError(format!("{message} at byte {}", self.position))
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.position).copied()
    }

    fn consume(&mut self, byte: u8) -> bool {
        if self.peek() == Some(byte) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.position += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        self.whitespace();
        match self.peek() {
            Some(b'{' | b'[') if depth >= 128 => Err(self.error("nesting limit exceeded")),
            Some(b'{') => self.object(depth + 1),
            Some(b'[') => self.array(depth + 1),
            Some(b'"') => self.string().map(JsonValue::String),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(b't') if self.text[self.position..].starts_with("true") => {
                self.position += 4;
                Ok(JsonValue::Bool(true))
            }
            Some(b'f') if self.text[self.position..].starts_with("false") => {
                self.position += 5;
                Ok(JsonValue::Bool(false))
            }
            Some(b'n') if self.text[self.position..].starts_with("null") => {
                self.position += 4;
                Ok(JsonValue::Null)
            }
            _ => Err(self.error("expected a JSON value")),
        }
    }

    fn object(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        self.position += 1;
        let mut values = BTreeMap::new();
        self.whitespace();
        if self.consume(b'}') {
            return Ok(JsonValue::Object(values));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.error("expected an object key"));
            }
            let key = self.string()?;
            self.whitespace();
            if !self.consume(b':') {
                return Err(self.error("expected `:`"));
            }
            let value = self.value(depth)?;
            values.insert(key, value);
            self.whitespace();
            if self.consume(b'}') {
                return Ok(JsonValue::Object(values));
            }
            if !self.consume(b',') {
                return Err(self.error("expected `,` or `}`"));
            }
            self.whitespace();
        }
    }

    fn array(&mut self, depth: usize) -> Result<JsonValue, JsonError> {
        self.position += 1;
        let mut values = Vec::new();
        self.whitespace();
        if self.consume(b']') {
            return Ok(JsonValue::Array(values));
        }
        loop {
            values.push(self.value(depth)?);
            self.whitespace();
            if self.consume(b']') {
                return Ok(JsonValue::Array(values));
            }
            if !self.consume(b',') {
                return Err(self.error("expected `,` or `]`"));
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.position += 1;
        let mut value = String::new();
        let mut start = self.position;
        loop {
            match self.peek() {
                Some(b'"') => {
                    value.push_str(&self.text[start..self.position]);
                    self.position += 1;
                    return Ok(value);
                }
                Some(b'\\') => {
                    value.push_str(&self.text[start..self.position]);
                    self.position += 1;
                    let escaped = match self.peek() {
                        Some(b'"') => '"',
                        Some(b'\\') => '\\',
                        Some(b'/') => '/',
                        Some(b'b') => '\u{08}',
                        Some(b'f') => '\u{0c}',
                        Some(b'n') => '\n',
                        Some(b'r') => '\r',
                        Some(b't') => '\t',
                        Some(b'u') => {
                            self.position += 1;
                            let first = self.hex_quad()?;
                            let code = if (0xd800..=0xdbff).contains(&first) {
                                if !self.consume(b'\\') || !self.consume(b'u') {
                                    return Err(self.error("expected a low surrogate"));
                                }
                                let second = self.hex_quad()?;
                                if !(0xdc00..=0xdfff).contains(&second) {
                                    return Err(self.error("invalid low surrogate"));
                                }
                                0x10000 + ((first - 0xd800) << 10) + second - 0xdc00
                            } else {
                                first
                            };
                            let character = char::from_u32(code)
                                .ok_or_else(|| self.error("invalid Unicode escape"))?;
                            value.push(character);
                            start = self.position;
                            continue;
                        }
                        _ => return Err(self.error("invalid string escape")),
                    };
                    value.push(escaped);
                    self.position += 1;
                    start = self.position;
                }
                Some(0..=0x1f) => return Err(self.error("unescaped control character")),
                Some(_) => self.position += 1,
                None => return Err(self.error("unterminated string")),
            }
        }
    }

    fn hex_quad(&mut self) -> Result<u32, JsonError> {
        let mut value = 0;
        for _ in 0..4 {
            let digit = match self.peek() {
                Some(byte @ b'0'..=b'9') => byte - b'0',
                Some(byte @ b'a'..=b'f') => byte - b'a' + 10,
                Some(byte @ b'A'..=b'F') => byte - b'A' + 10,
                _ => return Err(self.error("invalid Unicode escape")),
            };
            value = value * 16 + u32::from(digit);
            self.position += 1;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<JsonValue, JsonError> {
        let start = self.position;
        self.consume(b'-');
        match self.peek() {
            Some(b'0') => self.position += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.position += 1;
                }
            }
            _ => return Err(self.error("expected a digit")),
        }
        let mut float = false;
        if self.consume(b'.') {
            float = true;
            let fraction_start = self.position;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
            if self.position == fraction_start {
                return Err(self.error("expected a fraction digit"));
            }
        }
        if self.consume(b'e') || self.consume(b'E') {
            float = true;
            if !self.consume(b'+') {
                self.consume(b'-');
            }
            let exponent_start = self.position;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.position += 1;
            }
            if self.position == exponent_start {
                return Err(self.error("expected an exponent digit"));
            }
        }
        let token = &self.text[start..self.position];
        if !float && token != "-0" {
            if let Ok(value) = token.parse::<i64>() {
                return Ok(JsonValue::Integer(value));
            }
        }
        let value = token
            .parse::<f64>()
            .map_err(|_| self.error("invalid number"))?;
        if !value.is_finite() {
            return Err(self.error("number out of range"));
        }
        Ok(JsonValue::Float(value))
    }
}
