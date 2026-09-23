//! Minimal `FourCharCode` implementation for pixel formats and color conversions
//!
//! A `FourCharCode` is a 4-byte code used in Core Video and Core Media to identify
//! pixel formats, codecs, and other media types.

use std::fmt;
use std::str::FromStr;

/// `FourCharCode` represents a 4-character code (used in Core Video/Media)
///
/// # Examples
///
/// ```
/// use doom_fish_utils::FourCharCode;
///
/// // Create from string
/// let code: FourCharCode = "BGRA".parse().unwrap();
/// assert_eq!(code.display(), "BGRA");
///
/// // Create from bytes
/// let code = FourCharCode::from_bytes(*b"420v");
/// assert_eq!(code.as_u32(), 0x34323076);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FourCharCode(u32);

impl FourCharCode {
    /// Create a `FourCharCode` from exactly 4 bytes (infallible)
    ///
    /// # Examples
    ///
    /// ```
    /// use doom_fish_utils::FourCharCode;
    ///
    /// let code = FourCharCode::from_bytes(*b"BGRA");
    /// assert_eq!(code.display(), "BGRA");
    /// ```
    #[inline]
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 4]) -> Self {
        Self(u32::from_be_bytes(bytes))
    }

    /// Create a `FourCharCode` from a byte slice
    #[must_use]
    pub const fn from_slice(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 4 {
            return None;
        }

        let code = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        Some(Self(code))
    }

    /// Get the u32 representation
    ///
    /// # Examples
    ///
    /// ```
    /// use doom_fish_utils::FourCharCode;
    ///
    /// let code = FourCharCode::from_bytes(*b"BGRA");
    /// let value: u32 = code.as_u32();
    /// assert_eq!(value, 0x42475241);
    /// ```
    #[inline]
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Get the bytes as an array
    #[inline]
    #[must_use]
    pub const fn as_bytes(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// Create from a u32 value (const version of From trait)
    #[inline]
    #[must_use]
    pub const fn from_u32(value: u32) -> Self {
        Self(value)
    }

    /// Compare with another `FourCharCode` at compile time
    #[inline]
    #[must_use]
    pub const fn equals(self, other: Self) -> bool {
        self.0 == other.0
    }

    /// Display the code as a string
    #[must_use]
    pub fn display(self) -> String {
        let bytes = self.0.to_be_bytes();
        String::from_utf8_lossy(&bytes).to_string()
    }
}

impl FromStr for FourCharCode {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 4 {
            return Err("FourCharCode must be exactly 4 characters");
        }
        if !s.is_ascii() {
            return Err("FourCharCode must contain only ASCII characters");
        }

        let bytes = s.as_bytes();
        let code = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        Ok(Self(code))
    }
}

impl From<u32> for FourCharCode {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl From<FourCharCode> for u32 {
    fn from(code: FourCharCode) -> Self {
        code.0
    }
}

impl fmt::Display for FourCharCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}

#[cfg(test)]
mod tests {
    use super::FourCharCode;

    #[test]
    fn bytes_and_integers_round_trip() {
        let code = FourCharCode::from_bytes(*b"avc1");

        assert_eq!(code.as_u32(), 0x6176_6331);
        assert_eq!(code.as_bytes(), *b"avc1");
        assert_eq!(FourCharCode::from_u32(0x6176_6331), code);
        assert_eq!(FourCharCode::from(0x6176_6331), code);
        assert_eq!(u32::from(code), 0x6176_6331);
        assert!(code.equals(FourCharCode::from_bytes(*b"avc1")));
        assert!(!code.equals(FourCharCode::from_bytes(*b"hvc1")));
    }

    #[test]
    fn from_slice_requires_exactly_four_bytes() {
        assert_eq!(
            FourCharCode::from_slice(b"BGRA"),
            Some(FourCharCode::from_bytes(*b"BGRA"))
        );
        assert_eq!(FourCharCode::from_slice(b"BGR"), None);
        assert_eq!(FourCharCode::from_slice(b"BGRA8"), None);
        assert_eq!(FourCharCode::from_slice(&[]), None);
    }

    #[test]
    fn parsing_rejects_wrong_lengths_and_non_ascii() {
        assert_eq!(
            "420v".parse::<FourCharCode>(),
            Ok(FourCharCode::from_bytes(*b"420v"))
        );
        assert_eq!(
            "BGR".parse::<FourCharCode>(),
            Err("FourCharCode must be exactly 4 characters")
        );
        assert_eq!(
            "BGRA8".parse::<FourCharCode>(),
            Err("FourCharCode must be exactly 4 characters")
        );
        assert_eq!(
            "ab\u{e9}".parse::<FourCharCode>(),
            Err("FourCharCode must contain only ASCII characters")
        );
    }

    #[test]
    fn display_shows_the_code_characters() {
        let code = FourCharCode::from_bytes(*b"lpcm");

        assert_eq!(code.display(), "lpcm");
        assert_eq!(code.to_string(), "lpcm");
        assert_eq!(
            FourCharCode::from_bytes([b'a', 0xff, b'b', b'c']).display(),
            "a\u{fffd}bc"
        );
    }
}
