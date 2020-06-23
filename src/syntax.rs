use std::ffi::OsStr;
use std::path::Path;
use std::str::FromStr;

use serde::{de, Deserialize, Deserializer};
use strum::{EnumString, IntoStaticStr};

/// Programming language or file format being edited in a buffer.
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq, EnumString, IntoStaticStr)]
pub enum Syntax {
    #[strum(serialize = "javascript")]
    JavaScript,

    #[strum(serialize = "rust")]
    Rust,
}

impl Syntax {
    /// Attempts to identify the syntax for a given file.
    ///
    /// If the syntax is unknown or unsupported, `None` is returned.
    pub fn identify(path: impl AsRef<Path>) -> Option<Self> {
        let path = path.as_ref();

        if let Some(ext) = path.extension().and_then(OsStr::to_str) {
            match ext {
                "js" => return Some(Syntax::JavaScript),
                "rs" => return Some(Syntax::Rust),
                _ => (),
            }
        }

        None
    }

    /// Converts returns a syntax to a [LSP-compatible language identifier][language id].
    ///
    /// [language id]: https://microsoft.github.io/language-server-protocol/specifications/specification-current/#textDocumentItem
    pub fn into_language_id(self) -> &'static str {
        self.into()
    }
}

/// Used for deserializing [`crate::config::Config`].
impl<'de> Deserialize<'de> for Syntax {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        FromStr::from_str(&s).map_err(de::Error::custom)
    }
}
