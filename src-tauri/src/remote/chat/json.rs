//! A JSON tree that borrows from the line it was parsed from.
//!
//! A transcript line that carries a picture holds the picture as one huge
//! base64 string. `serde_json::Value` would copy every string of the line; this
//! tree keeps a string that has no escapes as a `&str` into the line itself, so
//! a 20 MiB picture line costs a few small allocations, not another 20 MiB.
//! (A string with escapes is copied: that is the only way to unescape it.)
//!
//! Hostile or odd lines stay bounded: serde_json refuses nesting deeper than
//! 128, and [`NODE_MAX`] caps the number of values, so `[1,1,1,...]` over
//! 24 MiB cannot make millions of nodes. A line that breaks either limit is
//! simply "not parseable", like any other bad JSON.

use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

/// Most values (of any kind) one line may hold.
pub const NODE_MAX: usize = 1_500_000;

#[derive(Debug)]
pub enum J<'a> {
    Null,
    Bool(bool),
    /// A number: nothing in a transcript reader needs its value.
    Num,
    Str(Cow<'a, str>),
    Arr(Vec<J<'a>>),
    /// Keys in file order; a repeated key is resolved to the last one, as
    /// `serde_json::Value` does.
    Obj(Vec<(Cow<'a, str>, J<'a>)>),
}

impl<'a> J<'a> {
    pub fn get(&self, key: &str) -> Option<&J<'a>> {
        match self {
            J::Obj(m) => m.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[J<'a>]> {
        match self {
            J::Arr(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            J::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

/// Most values a line over `MAX_LINE` may hold. A line with pictures has tens
/// of values (the picture is one string); a big line with hundreds of
/// thousands is not one a conversation needs, and its tree would cost ~200 MB.
pub const BIG_NODE_MAX: usize = 200_000;

/// Parse one whole JSON document (nothing but whitespace may follow it).
pub fn parse(bytes: &[u8]) -> Option<J<'_>> {
    parse_with(bytes, NODE_MAX)
}

/// [`parse`] with at most `max_nodes` values; over that is "not parseable".
pub fn parse_with(bytes: &[u8], max_nodes: usize) -> Option<J<'_>> {
    let count = Cell::new(0usize);
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let v = Seed(&count, max_nodes).deserialize(&mut de).ok()?;
    de.end().ok()?;
    Some(v)
}

/// Parse a JSON document held in a string (a tool call's `arguments`).
pub fn parse_str(s: &str) -> Option<J<'_>> {
    parse(s.as_bytes())
}

#[derive(Clone, Copy)]
struct Seed<'c>(&'c Cell<usize>, usize);

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = J<'de>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<J<'de>, D::Error> {
        let n = self.0.get() + 1;
        if n > self.1 {
            return Err(de::Error::custom("too many values"));
        }
        self.0.set(n);
        d.deserialize_any(Tree(self.0, self.1))
    }
}

struct Tree<'c>(&'c Cell<usize>, usize);

impl<'de> Visitor<'de> for Tree<'_> {
    type Value = J<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<J<'de>, E> {
        Ok(J::Bool(v))
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<J<'de>, E> {
        Ok(J::Num)
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<J<'de>, E> {
        Ok(J::Num)
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<J<'de>, E> {
        Ok(J::Num)
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<J<'de>, E> {
        Ok(J::Str(Cow::Borrowed(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<J<'de>, E> {
        Ok(J::Str(Cow::Owned(v.to_string())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<J<'de>, E> {
        Ok(J::Str(Cow::Owned(v)))
    }

    fn visit_unit<E: de::Error>(self) -> Result<J<'de>, E> {
        Ok(J::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<J<'de>, E> {
        Ok(J::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<J<'de>, A::Error> {
        let mut out = Vec::new();
        while let Some(v) = a.next_element_seed(Seed(self.0, self.1))? {
            out.push(v);
        }
        Ok(J::Arr(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<J<'de>, A::Error> {
        let mut out = Vec::new();
        while let Some(Key(k)) = a.next_key::<Key<'de>>()? {
            out.push((k, a.next_value_seed(Seed(self.0, self.1))?));
        }
        Ok(J::Obj(out))
    }
}

/// An object key, borrowed when it has no escapes.
struct Key<'de>(Cow<'de, str>);

impl<'de> de::Deserialize<'de> for Key<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct K;
        impl<'de> Visitor<'de> for K {
            type Value = Key<'de>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string key")
            }

            fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Borrowed(v)))
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Owned(v.to_string())))
            }
        }
        d.deserialize_str(K)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_strings_are_borrowed_escaped_ones_are_not() {
        let line = br#"{"a":"plain","b":"es\ncaped","c":[1,2.5,null,true],"a":"last"}"#;
        let v = parse(line).unwrap();
        // The last of a repeated key wins.
        assert_eq!(v.get("a").and_then(J::as_str), Some("last"));
        assert_eq!(v.get("b").and_then(J::as_str), Some("es\ncaped"));
        match v.get("b") {
            Some(J::Str(Cow::Owned(_))) => {}
            other => panic!("{other:?}"),
        }
        let J::Obj(m) = &v else { panic!() };
        assert!(matches!(&m[0].0, Cow::Borrowed("a")));
        assert!(matches!(&m[0].1, J::Str(Cow::Borrowed("plain"))));
        let c = v.get("c").and_then(J::as_array).unwrap();
        assert_eq!(c.len(), 4);
        assert_eq!(c[3].as_bool(), Some(true));
    }

    #[test]
    fn a_big_string_is_not_copied() {
        let big = "A".repeat(5_000_000);
        let line = format!(r#"{{"data":"{big}"}}"#);
        let v = parse(line.as_bytes()).unwrap();
        let s = v.get("data").and_then(J::as_str).unwrap();
        let (lo, hi) = (line.as_ptr() as usize, line.as_ptr() as usize + line.len());
        assert!(
            (lo..hi).contains(&(s.as_ptr() as usize)),
            "points into the line"
        );
    }

    #[test]
    fn bad_json_trailing_text_depth_and_node_count_are_refused() {
        assert!(parse(b"").is_none());
        assert!(parse(b"{").is_none());
        assert!(parse(br#"{"a":1} x"#).is_none());
        assert!(parse(b"{\"a\":1}  \r\n").is_some());
        let deep = format!("{}{}", "[".repeat(300), "]".repeat(300));
        assert!(parse(deep.as_bytes()).is_none(), "nesting limit");
        let wide = format!("[{}0]", "0,".repeat(NODE_MAX));
        assert!(parse(wide.as_bytes()).is_none(), "node limit");
        let ok = format!("[{}0]", "0,".repeat(1000));
        assert!(parse(ok.as_bytes()).is_some());
    }
}
