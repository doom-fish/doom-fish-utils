use std::sync::OnceLock;

pub const MAX_CHAR: u32 = 0x10_FFFF;

fn ranges_from(predicate: impl Fn(char) -> bool) -> Vec<(u32, u32)> {
    let mut ranges: Vec<(u32, u32)> = Vec::new();
    for value in 0..=MAX_CHAR {
        let Some(c) = char::from_u32(value) else {
            continue;
        };
        if !predicate(c) {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last.1 + 1 == value => last.1 = value,
            _ => ranges.push((value, value)),
        }
    }
    ranges
}

const EXTRA_WORD: [char; 12] = [
    '\u{203F}', '\u{2040}', '\u{2054}', '\u{FE33}', '\u{FE34}', '\u{FE4D}', '\u{FE4E}',
    '\u{FE4F}', '\u{FF3F}', '\u{200C}', '\u{200D}', '_',
];

pub fn is_word_char(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    c.is_alphanumeric() || EXTRA_WORD.contains(&c)
}

pub fn word() -> &'static [(u32, u32)] {
    static TABLE: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    TABLE.get_or_init(|| ranges_from(is_word_char))
}

pub fn digit() -> &'static [(u32, u32)] {
    static TABLE: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    TABLE.get_or_init(|| ranges_from(char::is_numeric))
}

pub fn space() -> &'static [(u32, u32)] {
    static TABLE: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    TABLE.get_or_init(|| ranges_from(char::is_whitespace))
}

#[derive(Clone, Copy)]
enum Property {
    Alphabetic,
    Lowercase,
    Uppercase,
    WhiteSpace,
    Numeric,
    Control,
    Letter,
    Any,
    Ascii,
}

fn normalize(name: &str) -> String {
    let mut out: String = name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect();
    if out.starts_with("is") && out.len() > 2 && !matches!(out.as_str(), "isc") {
        out.drain(..2);
    }
    out
}

fn lookup(name: &str) -> Option<Property> {
    Some(match normalize(name).as_str() {
        "alphabetic" | "alpha" => Property::Alphabetic,
        "lowercase" | "lower" | "ll" | "lowercaseletter" => Property::Lowercase,
        "uppercase" | "upper" | "lu" | "uppercaseletter" => Property::Uppercase,
        "whitespace" | "wspace" | "space" => Property::WhiteSpace,
        "n" | "number" | "nd" | "decimalnumber" | "digit" => Property::Numeric,
        "cc" | "control" | "cntrl" => Property::Control,
        "l" | "letter" => Property::Letter,
        "any" => Property::Any,
        "ascii" => Property::Ascii,
        _ => return None,
    })
}

pub fn property(name: &str) -> Option<&'static [(u32, u32)]> {
    static TABLES: [OnceLock<Vec<(u32, u32)>>; 9] = [
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
        OnceLock::new(),
    ];
    let property = lookup(name)?;
    Some(match property {
        Property::WhiteSpace => space(),
        Property::Numeric => digit(),
        _ => TABLES[property as usize].get_or_init(|| match property {
            Property::Alphabetic => ranges_from(char::is_alphabetic),
            Property::Lowercase => ranges_from(char::is_lowercase),
            Property::Uppercase => ranges_from(char::is_uppercase),
            Property::Control => ranges_from(char::is_control),
            Property::Letter => ranges_from(|c| c.is_alphabetic() && !c.is_numeric()),
            Property::Any => vec![(0, MAX_CHAR)],
            Property::Ascii => vec![(0, 0x7F)],
            Property::WhiteSpace | Property::Numeric => Vec::new(),
        }),
    })
}

fn single(mut iter: impl Iterator<Item = char>) -> Option<char> {
    let first = iter.next()?;
    iter.next().is_none().then_some(first)
}

fn find(parent: &mut [usize], mut index: usize) -> usize {
    while parent[index] != index {
        let next = parent[index];
        parent[index] = parent[next];
        index = next;
    }
    index
}

fn case_pairs() -> &'static [(u32, u32)] {
    static TABLE: OnceLock<Vec<(u32, u32)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for value in 0..=MAX_CHAR {
            let Some(c) = char::from_u32(value) else {
                continue;
            };
            if !c.is_alphabetic() {
                continue;
            }
            if let Some(lower) = single(c.to_lowercase()) {
                if lower != c {
                    edges.push((value, u32::from(lower)));
                }
            }
            if value != 0x131 {
                if let Some(upper) = single(c.to_uppercase()) {
                    if upper != c {
                        edges.push((value, u32::from(upper)));
                    }
                }
            }
        }
        edges.extend_from_slice(&[(0x390, 0x1FD3), (0x3B0, 0x1FE3), (0xFB05, 0xFB06)]);
        let mut members: Vec<u32> = edges.iter().flat_map(|&pair| <[u32; 2]>::from(pair)).collect();
        members.sort_unstable();
        members.dedup();
        let index_of = |value: u32| members.binary_search(&value).unwrap_or(0);
        let mut parent: Vec<usize> = (0..members.len()).collect();
        for &(a, b) in &edges {
            let root_a = find(&mut parent, index_of(a));
            let root_b = find(&mut parent, index_of(b));
            if root_a != root_b {
                parent[root_a] = root_b;
            }
        }
        let mut groups: Vec<(usize, u32)> = (0..members.len())
            .map(|index| (find(&mut parent, index), members[index]))
            .collect();
        groups.sort_unstable();
        let mut pairs = Vec::new();
        let mut start = 0;
        while start < groups.len() {
            let mut end = start;
            while end < groups.len() && groups[end].0 == groups[start].0 {
                end += 1;
            }
            for i in start..end {
                for j in start..end {
                    if i != j {
                        pairs.push((groups[i].1, groups[j].1));
                    }
                }
            }
            start = end;
        }
        pairs.sort_unstable();
        pairs
    })
}

pub fn fold_into(lo: u32, hi: u32, out: &mut Vec<(u32, u32)>) {
    let pairs = case_pairs();
    let first = pairs.partition_point(|&(value, _)| value < lo);
    for &(value, other) in &pairs[first..] {
        if value > hi {
            break;
        }
        out.push((other, other));
    }
}

pub fn has_fold(c: char) -> bool {
    let value = u32::from(c);
    let pairs = case_pairs();
    let first = pairs.partition_point(|&(item, _)| item < value);
    pairs.get(first).is_some_and(|&(item, _)| item == value)
}
