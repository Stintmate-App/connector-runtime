//! The reason codes of the connector kit's catalogs format (catalogs.md section 9).

use std::fmt;

/// Why an entry, a source message or a whole pack file is rejected. [`Reason::as_str`] is the
/// contract's code (the one the test vectors use).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, thiserror::Error)]
pub enum Reason {
    /// The file cannot be read.
    Unreadable,
    /// The file is not a JSON object.
    NotJson,
    /// `schema` is wrong, or `messages` is not an object.
    BadSchema,
    /// `catalog` is not the one asked for.
    CatalogMismatch,
    /// `language` does not match the folder.
    LanguageMismatch,
    /// The key is not in the built-in catalog.
    UnknownKey,
    /// `text` is not a non-empty string (or a plural object where allowed).
    BadText,
    /// A control character in a text.
    ControlCharacter,
    /// Unbalanced braces or an invalid placeholder name.
    BadPlaceholder,
    /// The placeholders do not match the source's params.
    PlaceholderMismatch,
    /// A plural object without `other`.
    PluralMissingOther,
    /// A plural category outside the language's rules.
    PluralBadCategory,
    /// `values` do not give exactly the source's value set.
    EnumValuesMismatch,
    /// `forms` name a param that is not `int`/`ordinal`, or a gender that is not `m`/`f`/`n`.
    BadForms,
    /// A `speech.` text longer than 200 characters.
    TooLong,
    /// `source_hash` is not 12 lower-case hexadecimal digits.
    BadSourceHash,
    /// A source key outside the key grammar.
    BadKey,
    /// A source message with a bad param name, kind or spec.
    BadParams,
    /// A source message whose `plural` is not an `int`/`laps` param, or a plural object
    /// without `plural`.
    BadPlural,
}

impl Reason {
    /// The contract's reason code.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Unreadable => "unreadable",
            Reason::NotJson => "not_json",
            Reason::BadSchema => "bad_schema",
            Reason::CatalogMismatch => "catalog_mismatch",
            Reason::LanguageMismatch => "language_mismatch",
            Reason::UnknownKey => "unknown_key",
            Reason::BadText => "bad_text",
            Reason::ControlCharacter => "control_character",
            Reason::BadPlaceholder => "bad_placeholder",
            Reason::PlaceholderMismatch => "placeholder_mismatch",
            Reason::PluralMissingOther => "plural_missing_other",
            Reason::PluralBadCategory => "plural_bad_category",
            Reason::EnumValuesMismatch => "enum_values_mismatch",
            Reason::BadForms => "bad_forms",
            Reason::TooLong => "too_long",
            Reason::BadSourceHash => "bad_source_hash",
            Reason::BadKey => "bad_key",
            Reason::BadParams => "bad_params",
            Reason::BadPlural => "bad_plural",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
