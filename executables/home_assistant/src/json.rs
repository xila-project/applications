//! Streaming JSON reader and writer helpers.
//!
//! Home Assistant responses are read straight from the network device, one byte at a
//! time through a small buffer, and only the values the caller asks for are
//! kept. A long attributes object therefore never sits in memory as a whole.

use crate::error::{Error, Result};
use std::string::String;
use std::vec::Vec;

/// A byte stream, such as a response body or a slice.
pub trait Source {
    /// Next byte, or `None` at the end of the stream.
    fn next_byte(&mut self) -> Result<Option<u8>>;
}

/// Reads a JSON document from a byte slice.
pub struct SliceSource<'a> {
    bytes: &'a [u8],
}

impl<'a> SliceSource<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl Source for SliceSource<'_> {
    fn next_byte(&mut self) -> Result<Option<u8>> {
        let Some((&first, rest)) = self.bytes.split_first() else {
            return Ok(None);
        };
        self.bytes = rest;
        Ok(Some(first))
    }
}

/// Kind of the next value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Object,
    Array,
    String,
    Number,
    Boolean,
    Null,
}

/// Longest object key that is compared; longer keys never match.
const KEY_CAPACITY: usize = 40;
/// Longest number literal accepted.
const NUMBER_CAPACITY: usize = 40;

/// Pull reader over a [`Source`]. Callers walk objects and arrays with
/// closures and must consume every value they are given, if only with
/// [`Reader::skip`].
pub struct Reader<S> {
    source: S,
    peeked: Option<u8>,
}

impl<S: Source> Reader<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            peeked: None,
        }
    }

    fn next(&mut self) -> Result<Option<u8>> {
        match self.peeked.take() {
            Some(byte) => Ok(Some(byte)),
            None => self.source.next_byte(),
        }
    }

    fn required(&mut self) -> Result<u8> {
        self.next()?.ok_or(Error::InvalidResponse)
    }

    /// Next byte that is not white space, left in place.
    fn peek_token(&mut self) -> Result<Option<u8>> {
        loop {
            let byte = match self.peeked {
                Some(byte) => byte,
                None => match self.source.next_byte()? {
                    Some(byte) => {
                        self.peeked = Some(byte);
                        byte
                    }
                    None => return Ok(None),
                },
            };
            if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
                self.peeked = None;
            } else {
                return Ok(Some(byte));
            }
        }
    }

    fn token(&mut self) -> Result<u8> {
        self.peek_token()?.ok_or(Error::InvalidResponse)
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        if self.token()? != expected {
            return Err(Error::InvalidResponse);
        }
        self.peeked = None;
        Ok(())
    }

    /// Kind of the next value, without consuming it.
    pub fn kind(&mut self) -> Result<Kind> {
        Ok(match self.token()? {
            b'{' => Kind::Object,
            b'[' => Kind::Array,
            b'"' => Kind::String,
            b't' | b'f' => Kind::Boolean,
            b'n' => Kind::Null,
            b'-' | b'0'..=b'9' => Kind::Number,
            _ => return Err(Error::InvalidResponse),
        })
    }

    /// Walks an object. `each` gets every key and must consume its value.
    /// Keys longer than an internal limit are passed as an empty string.
    pub fn object(&mut self, mut each: impl FnMut(&mut Self, &str) -> Result<()>) -> Result<()> {
        self.expect(b'{')?;
        if self.token()? == b'}' {
            self.peeked = None;
            return Ok(());
        }
        loop {
            self.expect(b'"')?;
            let mut key = [0u8; KEY_CAPACITY];
            let mut length = 0;
            let mut overflow = false;
            self.read_string(&mut |bytes| {
                if length + bytes.len() > KEY_CAPACITY {
                    overflow = true;
                } else {
                    key[length..length + bytes.len()].copy_from_slice(bytes);
                    length += bytes.len();
                }
                Ok(())
            })?;
            self.expect(b':')?;
            let key = if overflow {
                ""
            } else {
                core::str::from_utf8(&key[..length]).unwrap_or("")
            };
            each(self, key)?;
            match self.token()? {
                b',' => self.peeked = None,
                b'}' => {
                    self.peeked = None;
                    return Ok(());
                }
                _ => return Err(Error::InvalidResponse),
            }
        }
    }

    /// Walks an array. `each` must consume one element per call.
    pub fn array(&mut self, mut each: impl FnMut(&mut Self) -> Result<()>) -> Result<()> {
        self.expect(b'[')?;
        if self.token()? == b']' {
            self.peeked = None;
            return Ok(());
        }
        loop {
            each(self)?;
            match self.token()? {
                b',' => self.peeked = None,
                b']' => {
                    self.peeked = None;
                    return Ok(());
                }
                _ => return Err(Error::InvalidResponse),
            }
        }
    }

    /// Reads a string of at most `limit` bytes; longer ones are
    /// [`Error::TooLarge`].
    pub fn string(&mut self, limit: usize) -> Result<String> {
        let mut text = Vec::new();
        let mut too_large = false;
        self.expect(b'"')?;
        self.read_string(&mut |bytes| {
            if text.len() + bytes.len() > limit {
                too_large = true;
            } else {
                text.extend_from_slice(bytes);
            }
            Ok(())
        })?;
        if too_large {
            return Err(Error::TooLarge);
        }
        String::from_utf8(text).map_err(|_| Error::InvalidResponse)
    }

    /// Reads a string and cuts it at `limit` bytes, on a character boundary.
    pub fn truncated_string(&mut self, limit: usize) -> Result<String> {
        let mut text = Vec::new();
        self.expect(b'"')?;
        self.read_string(&mut |bytes| {
            if text.len() < limit + 4 {
                text.extend_from_slice(bytes);
            }
            Ok(())
        })?;
        let mut text = String::from_utf8(text).map_err(|_| Error::InvalidResponse)?;
        if text.len() > limit {
            let mut end = limit;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        Ok(text)
    }

    /// A string, or `None` for any other kind of value, which is skipped.
    pub fn optional_string(&mut self, limit: usize) -> Result<Option<String>> {
        if self.kind()? == Kind::String {
            self.string(limit).map(Some)
        } else {
            self.skip().map(|()| None)
        }
    }

    /// Body of a string whose opening quote was consumed.
    fn read_string(&mut self, output: &mut dyn FnMut(&[u8]) -> Result<()>) -> Result<()> {
        loop {
            match self.required()? {
                b'"' => return Ok(()),
                b'\\' => {
                    let character = match self.required()? {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => self.unicode_escape()?,
                        _ => return Err(Error::InvalidResponse),
                    };
                    let mut buffer = [0; 4];
                    output(character.encode_utf8(&mut buffer).as_bytes())?;
                }
                0..=0x1f => return Err(Error::InvalidResponse),
                byte => output(&[byte])?,
            }
        }
    }

    fn hexadecimal(&mut self) -> Result<u32> {
        let mut value = 0;
        for _ in 0..4 {
            let digit = char::from(self.required()?)
                .to_digit(16)
                .ok_or(Error::InvalidResponse)?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn unicode_escape(&mut self) -> Result<char> {
        let first = self.hexadecimal()?;
        let code = if (0xD800..0xDC00).contains(&first) {
            if self.required()? != b'\\' || self.required()? != b'u' {
                return Err(Error::InvalidResponse);
            }
            let second = self.hexadecimal()?;
            if !(0xDC00..0xE000).contains(&second) {
                return Err(Error::InvalidResponse);
            }
            0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
        } else {
            first
        };
        Ok(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER))
    }

    /// Reads a number.
    pub fn number(&mut self) -> Result<f64> {
        if self.kind()? != Kind::Number {
            return Err(Error::InvalidResponse);
        }
        let mut buffer = [0u8; NUMBER_CAPACITY];
        let mut length = 0;
        while let Some(byte) = self.next()? {
            if !matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                self.peeked = Some(byte);
                break;
            }
            if length == buffer.len() {
                return Err(Error::InvalidResponse);
            }
            buffer[length] = byte;
            length += 1;
        }
        core::str::from_utf8(&buffer[..length])
            .ok()
            .and_then(|text| text.parse().ok())
            .ok_or(Error::InvalidResponse)
    }

    /// Consumes a number of any length, which [`Reader::number`] would refuse.
    fn skip_number(&mut self) -> Result<()> {
        let mut digits = 0usize;
        while let Some(byte) = self.next()? {
            if !matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                self.peeked = Some(byte);
                break;
            }
            digits += 1;
        }
        if digits == 0 {
            return Err(Error::InvalidResponse);
        }
        Ok(())
    }

    /// A number, or `None` for `null` and any other kind of value, which is
    /// skipped.
    pub fn optional_number(&mut self) -> Result<Option<f64>> {
        if self.kind()? == Kind::Number {
            self.number().map(Some)
        } else {
            self.skip().map(|()| None)
        }
    }

    /// A boolean, or `None` for any other kind of value, which is skipped.
    pub fn optional_boolean(&mut self) -> Result<Option<bool>> {
        if self.kind()? == Kind::Boolean {
            let value = self.token()? == b't';
            self.literal()?;
            Ok(Some(value))
        } else {
            self.skip().map(|()| None)
        }
    }

    fn literal(&mut self) -> Result<()> {
        let expected: &[u8] = match self.token()? {
            b't' => b"true",
            b'f' => b"false",
            b'n' => b"null",
            _ => return Err(Error::InvalidResponse),
        };
        self.peeked = None;
        for &byte in &expected[1..] {
            if self.required()? != byte {
                return Err(Error::InvalidResponse);
            }
        }
        Ok(())
    }

    /// Skips one value of any kind, without recursion.
    pub fn skip(&mut self) -> Result<()> {
        let mut depth = 0usize;
        loop {
            match self.token()? {
                b'{' | b'[' => {
                    self.peeked = None;
                    depth += 1;
                }
                b'}' | b']' if depth > 0 => {
                    self.peeked = None;
                    depth -= 1;
                }
                b',' | b':' if depth > 0 => self.peeked = None,
                b'"' => {
                    self.peeked = None;
                    self.read_string(&mut |_| Ok(()))?;
                }
                b't' | b'f' | b'n' => self.literal()?,
                b'-' | b'0'..=b'9' => self.skip_number()?,
                _ => return Err(Error::InvalidResponse),
            }
            if depth == 0 {
                return Ok(());
            }
        }
    }

    /// Checks that nothing but white space follows the document.
    pub fn finish(&mut self) -> Result<()> {
        match self.peek_token()? {
            None => Ok(()),
            Some(_) => Err(Error::InvalidResponse),
        }
    }
}

/// Appends `text` as a JSON string literal.
pub fn push_string(output: &mut String, text: &str) {
    output.push('"');
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if u32::from(character) < 0x20 => {
                output.push_str(&std::format!("\\u{:04x}", u32::from(character)));
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(text: &str) -> Reader<SliceSource<'_>> {
        Reader::new(SliceSource::new(text.as_bytes()))
    }

    #[test]
    fn reads_selected_fields_and_skips_the_rest() {
        let mut json = reader(
            r#" {"skip": {"deep": [1, {"a": "}"}, null, true]}, "name": "A\"é😀", "value": -1.5e2, "flag": false} "#,
        );
        let (mut name, mut value, mut flag) = (String::new(), 0.0, None);
        json.object(|json, key| {
            match key {
                "name" => name = json.string(64)?,
                "value" => value = json.number()?,
                "flag" => flag = json.optional_boolean()?,
                _ => json.skip()?,
            }
            Ok(())
        })
        .unwrap();
        json.finish().unwrap();
        assert_eq!(name, "A\"é😀");
        assert_eq!(value, -150.0);
        assert_eq!(flag, Some(false));
    }

    #[test]
    fn arrays_and_optional_values() {
        let mut json = reader("[1, null, \"x\", [], {}]");
        let mut values = Vec::new();
        json.array(|json| {
            values.push(json.optional_number()?);
            Ok(())
        })
        .unwrap();
        assert_eq!(values, [Some(1.0), None, None, None, None]);
    }

    #[test]
    fn limits_and_errors() {
        assert_eq!(reader("\"abcdef\"").string(3), Err(Error::TooLarge));
        assert_eq!(reader("\"aé\"").truncated_string(2).unwrap(), "a");
        assert_eq!(reader("{\"a\" 1}").skip(), Ok(()), "skip is lenient");
        assert_eq!(
            reader("{\"a\" 1}").object(|json, _| json.skip()),
            Err(Error::InvalidResponse)
        );
        assert_eq!(reader("[1,").skip(), Err(Error::InvalidResponse));
        assert_eq!(reader("tru").skip(), Err(Error::InvalidResponse));
        // A number literal too long to read is still skipped.
        let long = std::format!("[{}, 1]", "7".repeat(100));
        assert_eq!(reader(&long).skip(), Ok(()));
        assert_eq!(
            reader(&"7".repeat(100)).number(),
            Err(Error::InvalidResponse)
        );
        assert_eq!(reader("\"\u{1}\"").skip(), Err(Error::InvalidResponse));
        let mut long_key = reader(r#"{"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": 1}"#);
        long_key
            .object(|json, key| {
                assert_eq!(key, "");
                json.skip()
            })
            .unwrap();
    }

    #[test]
    fn writes_escaped_strings() {
        let mut output = String::new();
        push_string(&mut output, "a\"b\\c\nd\u{1}é");
        assert_eq!(output, r#""a\"b\\c\nd\u0001é""#);
        let mut json = reader(&output);
        assert_eq!(json.string(64).unwrap(), "a\"b\\c\nd\u{1}é");
    }
}
