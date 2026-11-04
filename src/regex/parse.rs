use super::hir::{Class, Hir, Look};
use super::unicode;

#[derive(Clone, Copy, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct Flags {
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub dot_matches_new_line: bool,
    pub swap_greed: bool,
    pub ignore_whitespace: bool,
    pub unicode: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub flags: Flags,
    pub utf8: bool,
    pub nest_limit: u32,
}

pub struct Parsed {
    pub hir: Hir,
    pub names: Vec<Option<Box<str>>>,
}

enum Primitive {
    Literal(char, bool),
    Class(Class),
    Look(Look),
}

enum Item {
    Expr(Hir, u32),
    Flags,
}

#[derive(Clone, Copy)]
enum Operation {
    Intersection,
    Difference,
    SymmetricDifference,
}

struct Parser<'p> {
    pattern: &'p str,
    position: usize,
    flags: Flags,
    utf8: bool,
    nest_limit: u32,
    depth: u32,
    names: Vec<Option<Box<str>>>,
}

pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

type Result<T> = core::result::Result<T, ParseError>;

const fn is_meta(c: char) -> bool {
    matches!(
        c,
        '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '#'
            | '&' | '-' | '~'
    )
}

const fn is_escapeable(c: char) -> bool {
    is_meta(c) || (c.is_ascii() && !c.is_ascii_alphanumeric() && c != '<' && c != '>')
}

pub fn escape_into(text: &str, out: &mut String) {
    for c in text.chars() {
        if is_meta(c) {
            out.push('\\');
        }
        out.push(c);
    }
}

pub fn parse(pattern: &str, config: &Config) -> core::result::Result<Parsed, ParseError> {
    let mut parser = Parser {
        pattern,
        position: 0,
        flags: config.flags,
        utf8: config.utf8,
        nest_limit: config.nest_limit,
        depth: 0,
        names: vec![None],
    };
    let (hir, _) = parser.parse_alternation()?;
    if parser.position < pattern.len() {
        return Err(parser.error("unopened group"));
    }
    let mut present = Vec::new();
    hir.capture_indices(&mut present);
    let length = present.iter().max().map_or(1, |&index| index as usize + 1);
    let mut names = parser.names;
    names.truncate(length);
    let mut keep = vec![false; length];
    for index in present {
        keep[index as usize] = true;
    }
    for (name, keep) in names.iter_mut().zip(keep) {
        if !keep {
            *name = None;
        }
    }
    Ok(Parsed { hir, names })
}

fn ascii_class(name: &str) -> Option<&'static [(u32, u32)]> {
    Some(match name {
        "alnum" => &[(0x30, 0x39), (0x41, 0x5A), (0x61, 0x7A)],
        "alpha" => &[(0x41, 0x5A), (0x61, 0x7A)],
        "ascii" => &[(0x00, 0x7F)],
        "blank" => &[(0x09, 0x09), (0x20, 0x20)],
        "cntrl" => &[(0x00, 0x1F), (0x7F, 0x7F)],
        "digit" => &[(0x30, 0x39)],
        "graph" => &[(0x21, 0x7E)],
        "lower" => &[(0x61, 0x7A)],
        "print" => &[(0x20, 0x7E)],
        "punct" => &[(0x21, 0x2F), (0x3A, 0x40), (0x5B, 0x60), (0x7B, 0x7E)],
        "space" => &[(0x09, 0x0D), (0x20, 0x20)],
        "upper" => &[(0x41, 0x5A)],
        "word" => &[(0x30, 0x39), (0x41, 0x5A), (0x5F, 0x5F), (0x61, 0x7A)],
        "xdigit" => &[(0x30, 0x39), (0x41, 0x46), (0x61, 0x66)],
        _ => return None,
    })
}

impl Parser<'_> {
    fn error(&self, message: &str) -> ParseError {
        ParseError {
            message: message.to_owned(),
            offset: self.position,
        }
    }

    fn error_at(message: &str, offset: usize) -> ParseError {
        ParseError {
            message: message.to_owned(),
            offset,
        }
    }

    fn peek(&self) -> Option<char> {
        self.pattern[self.position..].chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        let mut chars = self.pattern[self.position..].chars();
        chars.next();
        chars.next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.position += c.len_utf8();
        Some(c)
    }

    fn bump_if(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.position += c.len_utf8();
            true
        } else {
            false
        }
    }

    fn bump_str(&mut self, prefix: &str) -> bool {
        if self.pattern[self.position..].starts_with(prefix) {
            self.position += prefix.len();
            true
        } else {
            false
        }
    }

    fn bump_space(&mut self) {
        if !self.flags.ignore_whitespace {
            return;
        }
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.bump();
            } else if c == '#' {
                while let Some(c) = self.bump() {
                    if c == '\n' {
                        break;
                    }
                }
            } else {
                break;
            }
        }
    }

    fn peek_space_after(&self) -> Option<char> {
        let mut chars = self.pattern[self.position..].chars();
        chars.next();
        if !self.flags.ignore_whitespace {
            return chars.next();
        }
        let mut in_comment = false;
        for c in chars {
            if in_comment {
                if c == '\n' {
                    in_comment = false;
                }
            } else if c == '#' {
                in_comment = true;
            } else if !c.is_whitespace() {
                return Some(c);
            }
        }
        None
    }

    fn check_depth(&self, depth: u32) -> Result<()> {
        if depth > self.nest_limit {
            return Err(self.error("exceed the maximum nesting depth"));
        }
        Ok(())
    }

    fn parse_alternation(&mut self) -> Result<(Hir, u32)> {
        let mut branches = Vec::new();
        let mut depth = 0;
        loop {
            let (branch, branch_depth) = self.parse_concat()?;
            branches.push(branch);
            depth = depth.max(branch_depth);
            if !self.bump_if('|') {
                break;
            }
        }
        if branches.len() > 1 {
            depth += 1;
            self.check_depth(depth)?;
        }
        Ok((Hir::alternation(branches), depth))
    }

    fn parse_concat(&mut self) -> Result<(Hir, u32)> {
        let mut items: Vec<Item> = Vec::new();
        loop {
            self.bump_space();
            let Some(c) = self.peek() else {
                break;
            };
            match c {
                '|' | ')' => break,
                '(' => match self.parse_group()? {
                    Some((hir, depth)) => items.push(Item::Expr(hir, depth)),
                    None => items.push(Item::Flags),
                },
                '[' => {
                    let (class, depth) = self.parse_class()?;
                    items.push(Item::Expr(Hir::class(class), depth));
                }
                '*' | '+' | '?' => {
                    let start = self.position;
                    self.bump();
                    let (min, max) = match c {
                        '*' => (0, None),
                        '+' => (1, None),
                        _ => (0, Some(1)),
                    };
                    let greedy = !self.bump_if('?');
                    self.repeat(&mut items, min, max, greedy, start)?;
                }
                '{' => {
                    let start = self.position;
                    if !matches!(items.last(), Some(Item::Expr(..))) {
                        return Err(self.error("repetition operator missing expression"));
                    }
                    let (min, max) = self.parse_counted()?;
                    let greedy = !self.bump_if('?');
                    self.repeat(&mut items, min, max, greedy, start)?;
                }
                _ => {
                    let primitive = self.parse_primitive()?;
                    let hir = self.primitive_hir(primitive)?;
                    items.push(Item::Expr(hir, 0));
                }
            }
        }
        let mut depth = 0;
        let mut exprs = Vec::with_capacity(items.len());
        for item in items {
            if let Item::Expr(hir, item_depth) = item {
                depth = depth.max(item_depth);
                exprs.push(hir);
            }
        }
        if exprs.len() > 1 {
            depth += 1;
            self.check_depth(depth)?;
        }
        Ok((Hir::concat(exprs), depth))
    }

    fn repeat(
        &self,
        items: &mut Vec<Item>,
        min: u32,
        max: Option<u32>,
        greedy: bool,
        start: usize,
    ) -> Result<()> {
        let Some(Item::Expr(sub, depth)) = items.pop() else {
            return Err(Self::error_at(
                "repetition operator missing expression",
                start,
            ));
        };
        let depth = depth + 1;
        self.check_depth(depth)?;
        let greedy = greedy != self.flags.swap_greed;
        items.push(Item::Expr(Hir::repeat(sub, min, max, greedy), depth));
        Ok(())
    }

    fn parse_decimal(&mut self) -> Result<u32> {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
        let start = self.position;
        let mut digits = String::new();
        while let Some(c) = self.peek().filter(char::is_ascii_digit) {
            digits.push(c);
            self.bump();
            self.bump_space();
        }
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
        if digits.is_empty() {
            return Err(Self::error_at("repetition quantifier expects a valid decimal", start));
        }
        digits
            .parse()
            .map_err(|_| Self::error_at("decimal literal invalid", start))
    }

    fn parse_counted(&mut self) -> Result<(u32, Option<u32>)> {
        let start = self.position;
        self.bump();
        self.bump_space();
        if self.peek().is_none() {
            return Err(Self::error_at("unclosed counted repetition", start));
        }
        let min = self.parse_decimal();
        if self.peek().is_none() {
            return Err(Self::error_at("unclosed counted repetition", start));
        }
        let (min, max) = if self.bump_if(',') {
            self.bump_space();
            if self.peek().is_none() {
                return Err(Self::error_at("unclosed counted repetition", start));
            }
            if self.peek() == Some('}') {
                (min?, None)
            } else {
                let min = min?;
                (min, Some(self.parse_decimal()?))
            }
        } else {
            let min = min?;
            (min, Some(min))
        };
        if !self.bump_if('}') {
            return Err(Self::error_at("unclosed counted repetition", start));
        }
        if let Some(max) = max {
            if min > max {
                return Err(Self::error_at("invalid repetition count range", start));
            }
        }
        Ok((min, max))
    }

    fn next_capture(&mut self, start: usize) -> Result<u32> {
        let index = u32::try_from(self.names.len())
            .map_err(|_| Self::error_at("too many capture groups", start))?;
        if index == u32::MAX {
            return Err(Self::error_at("too many capture groups", start));
        }
        self.names.push(None);
        Ok(index)
    }

    fn parse_capture_name(&mut self, index: u32) -> Result<()> {
        let start = self.position;
        loop {
            match self.peek() {
                None => return Err(self.error("unclosed capture group name")),
                Some('>') => break,
                Some(c) => {
                    let valid = if self.position == start {
                        c == '_' || c.is_alphabetic()
                    } else {
                        c == '_' || c == '.' || c == '[' || c == ']' || c.is_alphanumeric()
                    };
                    if !valid {
                        return Err(self.error("invalid capture group character"));
                    }
                    self.bump();
                }
            }
        }
        let name = &self.pattern[start..self.position];
        self.bump();
        if name.is_empty() {
            return Err(Self::error_at("empty capture group name", start));
        }
        if self.names.iter().flatten().any(|existing| &**existing == name) {
            return Err(Self::error_at("duplicate capture group name", start));
        }
        self.names[index as usize] = Some(name.into());
        Ok(())
    }

    fn parse_flags(&mut self) -> Result<Vec<(char, bool)>> {
        let mut items: Vec<(char, bool)> = Vec::new();
        let mut negated = false;
        let mut last_was_negation = false;
        loop {
            let Some(c) = self.peek() else {
                return Err(self.error("expected flag but got end of pattern"));
            };
            if c == ':' || c == ')' {
                break;
            }
            if c == '-' {
                if negated {
                    return Err(self.error("flag negation operator repeated"));
                }
                negated = true;
                last_was_negation = true;
            } else {
                if !matches!(c, 'i' | 'm' | 's' | 'U' | 'u' | 'x') {
                    return Err(self.error("unrecognized flag"));
                }
                if items.iter().any(|&(flag, _)| flag == c) {
                    return Err(self.error("duplicate flag"));
                }
                items.push((c, !negated));
                last_was_negation = false;
            }
            self.bump();
        }
        if last_was_negation {
            return Err(self.error("dangling flag negation operator"));
        }
        Ok(items)
    }

    fn apply_flags(&mut self, items: &[(char, bool)]) {
        for &(flag, value) in items {
            match flag {
                'i' => self.flags.case_insensitive = value,
                'm' => self.flags.multi_line = value,
                's' => self.flags.dot_matches_new_line = value,
                'U' => self.flags.swap_greed = value,
                'u' => self.flags.unicode = value,
                _ => self.flags.ignore_whitespace = value,
            }
        }
    }

    fn parse_group(&mut self) -> Result<Option<(Hir, u32)>> {
        let open = self.position;
        self.bump();
        self.bump_space();
        let rest = &self.pattern[self.position..];
        if rest.starts_with("?=")
            || rest.starts_with("?!")
            || rest.starts_with("?<=")
            || rest.starts_with("?<!")
        {
            return Err(Self::error_at(
                "look-around, including look-ahead and look-behind, is not supported",
                open,
            ));
        }
        let saved = self.flags;
        let capture = if self.bump_str("?P<") || self.bump_str("?<") {
            let index = self.next_capture(open)?;
            self.parse_capture_name(index)?;
            Some(index)
        } else if self.bump_if('?') {
            if self.peek().is_none() {
                return Err(Self::error_at("unclosed group", open));
            }
            let flags = self.parse_flags()?;
            if self.bump_if(')') {
                if flags.is_empty() {
                    return Err(Self::error_at("repetition operator missing expression", open));
                }
                self.apply_flags(&flags);
                return Ok(None);
            }
            self.bump();
            self.apply_flags(&flags);
            None
        } else {
            Some(self.next_capture(open)?)
        };
        self.depth += 1;
        self.check_depth(self.depth)?;
        let (inner, depth) = self.parse_alternation()?;
        self.depth -= 1;
        if !self.bump_if(')') {
            return Err(Self::error_at("unclosed group", open));
        }
        self.flags = saved;
        let depth = depth + 1;
        self.check_depth(depth)?;
        Ok(Some(match capture {
            Some(index) => (
                Hir::Capture {
                    index,
                    sub: Box::new(inner),
                },
                depth,
            ),
            None => (inner, depth),
        }))
    }

    fn parse_primitive(&mut self) -> Result<Primitive> {
        let start = self.position;
        match self.peek() {
            Some('\\') => self.parse_escape(),
            Some('.') => {
                self.bump();
                if self.utf8 && !self.flags.unicode {
                    return Err(Self::error_at("pattern can match invalid UTF-8", start));
                }
                let max = if self.flags.unicode { unicode::MAX_CHAR } else { 0xFF };
                let mut class = Class::new(self.flags.unicode, [(0, max)]);
                if !self.flags.dot_matches_new_line {
                    class.difference(&Class::new(self.flags.unicode, [(0x0A, 0x0A)]));
                }
                Ok(Primitive::Class(class))
            }
            Some('^') => {
                self.bump();
                Ok(Primitive::Look(if self.flags.multi_line {
                    Look::StartLine
                } else {
                    Look::Start
                }))
            }
            Some('$') => {
                self.bump();
                Ok(Primitive::Look(if self.flags.multi_line {
                    Look::EndLine
                } else {
                    Look::End
                }))
            }
            Some(c) => {
                self.bump();
                Ok(Primitive::Literal(c, false))
            }
            None => Err(self.error("unexpected end of pattern")),
        }
    }

    fn primitive_hir(&self, primitive: Primitive) -> Result<Hir> {
        match primitive {
            Primitive::Literal(c, byte) => self.literal(c, byte),
            Primitive::Class(class) => Ok(Hir::class(class)),
            Primitive::Look(look) => Ok(Hir::Look(look)),
        }
    }

    fn literal(&self, c: char, byte: bool) -> Result<Hir> {
        if !self.flags.unicode && byte && u32::from(c) >= 0x80 {
            if self.utf8 {
                return Err(self.error("pattern can match invalid UTF-8"));
            }
            return Ok(Hir::Literal(vec![u8::try_from(u32::from(c)).unwrap_or(0xFF)]));
        }
        if self.flags.case_insensitive {
            if self.flags.unicode {
                if unicode::has_fold(c) {
                    let mut class = Class::new(true, [(u32::from(c), u32::from(c))]);
                    class.case_fold();
                    return Ok(Hir::class(class));
                }
            } else if c.is_ascii_alphabetic() {
                let mut class = Class::new(false, [(u32::from(c), u32::from(c))]);
                class.case_fold();
                return Ok(Hir::class(class));
            }
        }
        let mut buffer = [0u8; 4];
        Ok(Hir::Literal(c.encode_utf8(&mut buffer).as_bytes().to_vec()))
    }

    fn parse_hex(&mut self, kind: char, start: usize) -> Result<Primitive> {
        self.bump_space();
        if self.peek().is_none() {
            return Err(Self::error_at("incomplete escape sequence", start));
        }
        if self.bump_if('{') {
            let mut digits = String::new();
            loop {
                self.bump_space();
                match self.peek() {
                    None => return Err(Self::error_at("unclosed hexadecimal literal", start)),
                    Some('}') => {
                        self.bump();
                        break;
                    }
                    Some(c) if c.is_ascii_hexdigit() => {
                        digits.push(c);
                        self.bump();
                    }
                    Some(_) => return Err(self.error("invalid hexadecimal digit")),
                }
            }
            if digits.is_empty() {
                return Err(Self::error_at("empty hexadecimal literal", start));
            }
            let value = u32::from_str_radix(&digits, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| Self::error_at("invalid Unicode scalar value", start))?;
            return Ok(Primitive::Literal(value, false));
        }
        let count = match kind {
            'x' => 2,
            'u' => 4,
            _ => 8,
        };
        let mut value = 0u32;
        for index in 0..count {
            if index > 0 {
                self.bump_space();
            }
            match self.peek() {
                None => return Err(Self::error_at("incomplete escape sequence", start)),
                Some(c) => {
                    let digit = c
                        .to_digit(16)
                        .ok_or_else(|| self.error("invalid hexadecimal digit"))?;
                    value = value * 16 + digit;
                    self.bump();
                }
            }
        }
        let c = char::from_u32(value)
            .ok_or_else(|| Self::error_at("invalid Unicode scalar value", start))?;
        Ok(Primitive::Literal(c, kind == 'x'))
    }

    fn perl_class(&self, kind: char, start: usize) -> Result<Class> {
        let negated = kind.is_ascii_uppercase();
        let mut class = if self.flags.unicode {
            let ranges = match kind.to_ascii_lowercase() {
                'd' => unicode::digit(),
                's' => unicode::space(),
                _ => unicode::word(),
            };
            Class::new(true, ranges.iter().copied())
        } else {
            let name = match kind.to_ascii_lowercase() {
                'd' => "digit",
                's' => "space",
                _ => "word",
            };
            Class::new(false, ascii_class(name).unwrap_or(&[]).iter().copied())
        };
        if negated {
            class.negate();
        }
        if !class.unicode && self.utf8 && !class.is_ascii() {
            return Err(Self::error_at("pattern can match invalid UTF-8", start));
        }
        Ok(class)
    }

    fn parse_unicode_class(&mut self, negated: bool, start: usize) -> Result<Class> {
        self.bump_space();
        let mut name = String::new();
        if self.bump_if('{') {
            loop {
                self.bump_space();
                match self.bump() {
                    None => return Err(Self::error_at("incomplete escape sequence", start)),
                    Some('}') => break,
                    Some(c) => name.push(c),
                }
            }
        } else {
            match self.bump() {
                None => return Err(Self::error_at("incomplete escape sequence", start)),
                Some(c) => name.push(c),
            }
        }
        if !self.flags.unicode {
            return Err(Self::error_at("Unicode not allowed here", start));
        }
        let mut negated = negated;
        let value = if let Some((key, value)) = name.split_once("!=") {
            negated = !negated;
            Self::property_value(key, value)
        } else if let Some((key, value)) = name.split_once(['=', ':']) {
            Self::property_value(key, value)
        } else {
            Some(name.as_str())
        };
        let ranges = value
            .and_then(unicode::property)
            .ok_or_else(|| Self::error_at("Unicode property not found", start))?;
        let mut class = Class::new(true, ranges.iter().copied());
        if self.flags.case_insensitive {
            class.case_fold();
        }
        if negated {
            class.negate();
        }
        Ok(class)
    }

    fn property_value<'a>(key: &str, value: &'a str) -> Option<&'a str> {
        let key: String = key
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
            .flat_map(char::to_lowercase)
            .collect();
        matches!(key.as_str(), "gc" | "generalcategory").then_some(value)
    }

    fn parse_escape(&mut self) -> Result<Primitive> {
        let start = self.position;
        self.bump();
        let Some(c) = self.bump() else {
            return Err(Self::error_at("incomplete escape sequence", start));
        };
        match c {
            '0' => {
                if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    return Err(Self::error_at("octal escapes are not supported", start));
                }
                Ok(Primitive::Literal('\0', false))
            }
            '1'..='9' => Err(Self::error_at("backreferences are not supported", start)),
            'x' | 'u' | 'U' => self.parse_hex(c, start),
            'p' | 'P' => Ok(Primitive::Class(self.parse_unicode_class(c == 'P', start)?)),
            'd' | 's' | 'w' | 'D' | 'S' | 'W' => Ok(Primitive::Class(self.perl_class(c, start)?)),
            'a' => Ok(Primitive::Literal('\x07', false)),
            'f' => Ok(Primitive::Literal('\x0C', false)),
            't' => Ok(Primitive::Literal('\t', false)),
            'n' => Ok(Primitive::Literal('\n', false)),
            'r' => Ok(Primitive::Literal('\r', false)),
            'v' => Ok(Primitive::Literal('\x0B', false)),
            'A' => Ok(Primitive::Look(Look::Start)),
            'z' => Ok(Primitive::Look(Look::End)),
            'b' => {
                let unicode = self.flags.unicode;
                if self.peek() == Some('{') {
                    if let Some(look) = self.parse_special_word_boundary(start)? {
                        return Ok(Primitive::Look(look));
                    }
                }
                Ok(Primitive::Look(if unicode {
                    Look::WordUnicode
                } else {
                    Look::WordAscii
                }))
            }
            'B' => Ok(Primitive::Look(if self.flags.unicode {
                Look::WordUnicodeNegate
            } else {
                Look::WordAsciiNegate
            })),
            '<' => Ok(Primitive::Look(if self.flags.unicode {
                Look::WordStartUnicode
            } else {
                Look::WordStartAscii
            })),
            '>' => Ok(Primitive::Look(if self.flags.unicode {
                Look::WordEndUnicode
            } else {
                Look::WordEndAscii
            })),
            c if is_escapeable(c) => Ok(Primitive::Literal(c, false)),
            _ => Err(Self::error_at("unrecognized escape sequence", start)),
        }
    }

    fn parse_special_word_boundary(&mut self, start: usize) -> Result<Option<Look>> {
        let open = self.position;
        self.bump();
        self.bump_space();
        let valid = |c: char| c.is_ascii_alphabetic() || c == '-';
        if !self.peek().is_some_and(valid) {
            if self.peek().is_none() {
                return Err(Self::error_at("unexpected end of pattern", start));
            }
            self.position = open;
            return Ok(None);
        }
        let mut name = String::new();
        while let Some(c) = self.peek().filter(|&c| valid(c)) {
            name.push(c);
            self.bump();
            self.bump_space();
        }
        if !self.bump_if('}') {
            return Err(Self::error_at("unclosed special word boundary", start));
        }
        let unicode = self.flags.unicode;
        let look = match (name.as_str(), unicode) {
            ("start", true) => Look::WordStartUnicode,
            ("start", false) => Look::WordStartAscii,
            ("end", true) => Look::WordEndUnicode,
            ("end", false) => Look::WordEndAscii,
            ("start-half", true) => Look::WordStartHalfUnicode,
            ("start-half", false) => Look::WordStartHalfAscii,
            ("end-half", true) => Look::WordEndHalfUnicode,
            ("end-half", false) => Look::WordEndHalfAscii,
            _ => return Err(Self::error_at("unrecognized special word boundary", start)),
        };
        Ok(Some(look))
    }

    fn class_value(&self, c: char, byte: bool, start: usize) -> Result<u32> {
        if self.flags.unicode {
            return Ok(u32::from(c));
        }
        if byte && u32::from(c) >= 0x80 {
            if self.utf8 {
                return Err(Self::error_at("pattern can match invalid UTF-8", start));
            }
            return Ok(u32::from(c));
        }
        if c.is_ascii() {
            Ok(u32::from(c))
        } else {
            Err(Self::error_at("Unicode not allowed here", start))
        }
    }

    fn parse_class_atom(&mut self) -> Result<(Option<u32>, Class)> {
        let start = self.position;
        if self.peek() == Some('\\') {
            match self.parse_escape()? {
                Primitive::Literal(c, byte) => {
                    let value = self.class_value(c, byte, start)?;
                    Ok((Some(value), Class::new(self.flags.unicode, [(value, value)])))
                }
                Primitive::Class(class) => Ok((None, class)),
                Primitive::Look(_) => Err(Self::error_at(
                    "assertions are not allowed in character classes",
                    start,
                )),
            }
        } else {
            let Some(c) = self.bump() else {
                return Err(Self::error_at("unclosed character class", start));
            };
            let value = self.class_value(c, false, start)?;
            Ok((Some(value), Class::new(self.flags.unicode, [(value, value)])))
        }
    }

    fn parse_class_range(&mut self) -> Result<Class> {
        let start = self.position;
        let (first, class) = self.parse_class_atom()?;
        self.bump_space();
        if self.peek().is_none() {
            return Err(Self::error_at("unclosed character class", start));
        }
        if self.peek() != Some('-') || matches!(self.peek_space_after(), Some(']' | '-')) {
            return Ok(class);
        }
        self.bump();
        self.bump_space();
        if self.peek().is_none() {
            return Err(Self::error_at("unclosed character class", start));
        }
        let (second, _) = self.parse_class_atom()?;
        let (Some(lo), Some(hi)) = (first, second) else {
            return Err(Self::error_at(
                "invalid range boundary, must be a literal",
                start,
            ));
        };
        if lo > hi {
            return Err(Self::error_at(
                "invalid character class range, the start must be <= the end",
                start,
            ));
        }
        Ok(Class::new(self.flags.unicode, [(lo, hi)]))
    }

    fn try_ascii_class(&mut self) -> Result<Option<Class>> {
        let rest = &self.pattern[self.position..];
        let Some(body) = rest.strip_prefix("[:") else {
            return Ok(None);
        };
        let (negated, body) = body
            .strip_prefix('^')
            .map_or((false, body), |body| (true, body));
        let Some(end) = body.find(':') else {
            return Ok(None);
        };
        if !body[end..].starts_with(":]") {
            return Ok(None);
        }
        let Some(ranges) = ascii_class(&body[..end]) else {
            return Ok(None);
        };
        let start = self.position;
        self.position += rest.len() - body.len() + end + 2;
        let mut class = Class::new(self.flags.unicode, ranges.iter().copied());
        self.fold_and_negate(&mut class, negated, start)?;
        Ok(Some(class))
    }

    fn fold_and_negate(&self, class: &mut Class, negated: bool, start: usize) -> Result<()> {
        if self.flags.case_insensitive {
            class.case_fold();
        }
        if negated {
            class.negate();
        }
        if !class.unicode && self.utf8 && !class.is_ascii() {
            return Err(Self::error_at("pattern can match invalid UTF-8", start));
        }
        Ok(())
    }

    fn apply_operation(&self, mut lhs: Class, operation: Operation, mut rhs: Class) -> Class {
        if self.flags.case_insensitive {
            lhs.case_fold();
            rhs.case_fold();
        }
        match operation {
            Operation::Intersection => lhs.intersect(&rhs),
            Operation::Difference => lhs.difference(&rhs),
            Operation::SymmetricDifference => lhs.symmetric_difference(&rhs),
        }
        lhs
    }

    fn parse_class(&mut self) -> Result<(Class, u32)> {
        let open = self.position;
        self.bump();
        self.bump_space();
        self.depth += 1;
        self.check_depth(self.depth)?;
        if self.peek().is_none() {
            return Err(Self::error_at("unclosed character class", open));
        }
        let negated = if self.bump_if('^') {
            self.bump_space();
            if self.peek().is_none() {
                return Err(Self::error_at("unclosed character class", open));
            }
            true
        } else {
            false
        };
        let unicode = self.flags.unicode;
        let mut union = Class::empty(unicode);
        let mut any = false;
        while self.peek() == Some('-') {
            union.push(u32::from(b'-'), u32::from(b'-'));
            any = true;
            self.bump();
            self.bump_space();
            if self.peek().is_none() {
                return Err(Self::error_at("unclosed character class", open));
            }
        }
        if !any && self.peek() == Some(']') {
            union.push(u32::from(b']'), u32::from(b']'));
            self.bump();
            self.bump_space();
        }
        let mut pending: Option<(Operation, Class)> = None;
        let mut depth = 0;
        loop {
            self.bump_space();
            let Some(c) = self.peek() else {
                return Err(Self::error_at("unclosed character class", open));
            };
            let operation = match (c, self.peek_second()) {
                ('[', _) => {
                    if let Some(class) = self.try_ascii_class()? {
                        union.union(&class);
                    } else {
                        let (nested, nested_depth) = self.parse_class()?;
                        depth = depth.max(nested_depth);
                        union.union(&nested);
                    }
                    continue;
                }
                (']', _) => {
                    self.bump();
                    break;
                }
                ('&', Some('&')) => Operation::Intersection,
                ('-', Some('-')) => Operation::Difference,
                ('~', Some('~')) => Operation::SymmetricDifference,
                _ => {
                    let item = self.parse_class_range()?;
                    union.union(&item);
                    continue;
                }
            };
            self.bump();
            self.bump();
            let rhs = core::mem::replace(&mut union, Class::empty(unicode));
            let lhs = match pending.take() {
                Some((previous, lhs)) => self.apply_operation(lhs, previous, rhs),
                None => rhs,
            };
            pending = Some((operation, lhs));
        }
        let mut class = match pending {
            Some((operation, lhs)) => self.apply_operation(lhs, operation, union),
            None => union,
        };
        self.fold_and_negate(&mut class, negated, open)?;
        self.depth -= 1;
        let depth = depth + 1;
        self.check_depth(depth)?;
        Ok((class, depth))
    }
}
