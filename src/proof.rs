//! The proof check: a claim is only accepted if its quote is really in the
//! node's source.

/// Shortest quote worth accepting, counted without whitespace. Anything
/// shorter ("(", "if x") would match almost anywhere and prove nothing.
pub const MIN_QUOTE_CHARS: usize = 8;
pub const MAX_QUOTE_CHARS: usize = 600;

/// Collapses every run of whitespace so that indentation and line wrapping
/// do not decide whether a quote matches. Returns the text and, for each of
/// its bytes, the byte offset it came from.
fn normalize(text: &str) -> (String, Vec<usize>) {
    let mut out = String::with_capacity(text.len());
    let mut origin = Vec::with_capacity(text.len());
    let mut pending_space = false;
    for (offset, ch) in text.char_indices() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            origin.push(offset);
            pending_space = false;
        }
        let before = out.len();
        out.push(ch);
        for _ in before..out.len() {
            origin.push(offset);
        }
    }
    (out, origin)
}

#[derive(Debug, PartialEq, Eq)]
pub enum QuoteError {
    TooShort,
    TooLong,
    NotFound,
}

impl QuoteError {
    pub fn message(&self) -> String {
        match self {
            Self::TooShort => format!(
                "quote is too short to prove anything (needs at least {MIN_QUOTE_CHARS} non-space characters)"
            ),
            Self::TooLong => format!(
                "quote is too long (at most {MAX_QUOTE_CHARS} characters); quote the decisive lines only"
            ),
            Self::NotFound => {
                "quote was not found in this unit's source; copy it exactly from `seam show`"
                    .to_string()
            }
        }
    }
}

/// Finds `quote` in `text`. Returns the zero-based line offset inside `text`
/// where the quote starts.
pub fn locate(text: &str, quote: &str) -> Result<usize, QuoteError> {
    let (needle, _) = normalize(quote);
    let solid = needle.chars().filter(|c| !c.is_whitespace()).count();
    if solid < MIN_QUOTE_CHARS {
        return Err(QuoteError::TooShort);
    }
    if needle.chars().count() > MAX_QUOTE_CHARS {
        return Err(QuoteError::TooLong);
    }
    let (haystack, origin) = normalize(text);
    let at = haystack.find(&needle).ok_or(QuoteError::NotFound)?;
    let offset = origin[at];
    Ok(text[..offset].matches('\n').count())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "fn check(user: &User) -> bool {\n    if user.is_admin {\n        return true;\n    }\n    false\n}";

    #[test]
    fn finds_a_quote_and_its_line() {
        assert_eq!(locate(SRC, "if user.is_admin {"), Ok(1));
    }

    #[test]
    fn ignores_whitespace_differences() {
        assert_eq!(locate(SRC, "if   user.is_admin  {\n return true;"), Ok(1));
    }

    #[test]
    fn rejects_code_that_is_not_there() {
        assert_eq!(
            locate(SRC, "if user.is_superuser {"),
            Err(QuoteError::NotFound)
        );
    }

    #[test]
    fn rejects_trivial_quotes() {
        assert_eq!(locate(SRC, "false"), Err(QuoteError::TooShort));
    }

    #[test]
    fn does_not_match_across_removed_characters() {
        // "is_admin" must not match "isadmin": only whitespace is flexible.
        assert_eq!(locate(SRC, "if user.isadmin {"), Err(QuoteError::NotFound));
    }
}
