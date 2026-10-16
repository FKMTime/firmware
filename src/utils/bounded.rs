//! Fixed-capacity string/vec that serialize like `String`/`Vec` but never grow the
//! heap: over-long input is truncated (string at a char boundary, extra list items
//! skipped) instead of failing the whole packet.

use core::{fmt, hash::Hash, marker::PhantomData};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{IgnoredAny, SeqAccess, Visitor},
};

#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct BoundedString<const N: usize>(heapless::String<N>);

impl<const N: usize> BoundedString<N> {
    /// `s` truncated to at most `N` bytes (at a char boundary).
    pub fn truncated(s: &str) -> Self {
        let mut end = s.len().min(N);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut out = heapless::String::new();
        // cannot fail: end <= N
        _ = out.push_str(&s[..end]);
        Self(out)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<const N: usize> core::ops::Deref for BoundedString<N> {
    type Target = str;
    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl<const N: usize> From<&str> for BoundedString<N> {
    fn from(s: &str) -> Self {
        Self::truncated(s)
    }
}

impl<const N: usize> From<alloc::string::String> for BoundedString<N> {
    fn from(s: alloc::string::String) -> Self {
        Self::truncated(&s)
    }
}

impl<const N: usize> PartialEq<str> for BoundedString<N> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl<const N: usize> PartialEq<&str> for BoundedString<N> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl<const N: usize> fmt::Display for BoundedString<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl<const N: usize> fmt::Debug for BoundedString<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl<const N: usize> fmt::Write for BoundedString<N> {
    /// Appends what fits (truncating) and never fails.
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let room = N - self.0.len();
        let mut end = s.len().min(room);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        _ = self.0.push_str(&s[..end]);
        Ok(())
    }
}

impl<const N: usize> Serialize for BoundedString<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de, const N: usize> Deserialize<'de> for BoundedString<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V<const N: usize>;
        impl<const N: usize> Visitor<'_> for V<N> {
            type Value = BoundedString<N>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
                Ok(BoundedString::truncated(v))
            }
        }
        deserializer.deserialize_str(V::<N>)
    }
}

#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct BoundedVec<T, const N: usize>(heapless::Vec<T, N>);

impl<T, const N: usize> BoundedVec<T, N> {
    pub const fn new() -> Self {
        Self(heapless::Vec::new())
    }

    /// Pushes `item`, dropping it if full (returns whether it was stored).
    pub fn push(&mut self, item: T) -> bool {
        self.0.push(item).is_ok()
    }
}

impl<T, const N: usize> core::ops::Deref for BoundedVec<T, N> {
    type Target = heapless::Vec<T, N>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T, const N: usize> core::ops::DerefMut for BoundedVec<T, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<T: Serialize, const N: usize> Serialize for BoundedVec<T, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter())
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for BoundedVec<T, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V<T, const N: usize>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for V<T, N> {
            type Value = BoundedVec<T, N>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a list")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut out = BoundedVec::new();
                while out.len() < N {
                    match seq.next_element()? {
                        Some(item) => _ = out.push(item),
                        None => return Ok(out),
                    }
                }
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(out)
            }
        }
        deserializer.deserialize_seq(V::<T, N>(PhantomData))
    }
}

/// Heap-backed list with the same truncating `Deserialize` as [`BoundedVec`]: for lists
/// inside packets, so a packet stays small wherever it is queued but can't grow past `N`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct CappedVec<T, const N: usize>(alloc::vec::Vec<T>);

impl<T, const N: usize> Default for CappedVec<T, N> {
    fn default() -> Self {
        Self(alloc::vec::Vec::new())
    }
}

impl<T, const N: usize> core::ops::Deref for CappedVec<T, N> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        &self.0
    }
}

impl<T, const N: usize> CappedVec<T, N> {
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

impl<T, const N: usize> IntoIterator for CappedVec<T, N> {
    type Item = T;
    type IntoIter = alloc::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T: Serialize, const N: usize> Serialize for CappedVec<T, N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter())
    }
}

impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for CappedVec<T, N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let items = BoundedVec::<T, N>::deserialize(deserializer)?;
        Ok(Self(items.0.into_iter().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_at_char_boundary() {
        assert_eq!(BoundedString::<4>::truncated("abcdef").as_str(), "abcd");
        assert_eq!(BoundedString::<4>::truncated("abcó").as_str(), "abc");
        assert_eq!(BoundedString::<4>::truncated("").as_str(), "");
        let s: BoundedString<3> = serde_json::from_str("\"a\\\"bcd\"").unwrap();
        assert_eq!(s.as_str(), "a\"b");
        assert_eq!(serde_json::to_string(&s).unwrap(), "\"a\\\"b\"");
    }

    #[test]
    fn vec_skips_extra_items() {
        let v: BoundedVec<BoundedString<2>, 2> =
            serde_json::from_str(r#"["aaa", "b", "c", {"x": [1]}]"#).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].as_str(), "aa");
        let v: BoundedVec<u8, 2> = serde_json::from_str("[1, 2, 3, 4]").unwrap();
        assert_eq!(v.as_slice(), &[1, 2]);
        assert_eq!(serde_json::to_string(&v).unwrap(), "[1,2]");
    }
}
