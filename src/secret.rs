//! A string that never shows up in `Debug` output and is wiped when dropped.

use std::fmt;

use zeroize::Zeroize;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// The secret value; call sites that need it are easy to grep for.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub(crate) fn push(&mut self, c: char) {
        self.0.push(c);
    }

    pub(crate) fn pop(&mut self) -> Option<char> {
        self.0.pop()
    }

    #[must_use]
    pub(crate) fn char_count(&self) -> usize {
        self.0.chars().count()
    }
}

impl From<&str> for Secret {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Only the length crosses the wire (as `*`s): enough to draw the mask,
/// never the secret itself.
impl serde::Serialize for Secret {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&"*".repeat(self.char_count()))
    }
}

impl<'de> serde::Deserialize<'de> for Secret {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(Secret)
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let s = Secret::from("hunter2");
        assert_eq!(format!("{s:?}"), "<redacted>");
        assert_eq!(s.expose(), "hunter2");
    }
}
