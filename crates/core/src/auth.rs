//! The bearer token `sbtd` mints when it starts and `sbtctl` reads from the
//! state directory. Loopback binding and the Host/Origin check keep
//! browsers and other machines out; the token keeps other local users out,
//! because the state directory is readable by its owner alone.

use std::fmt;

use thiserror::Error;

const TOKEN_BYTES: usize = 32;
const TOKEN_HEX_LEN: usize = TOKEN_BYTES * 2;

/// 32 random bytes as 64 lowercase hex digits. Never printed: `Debug` is
/// redacted, and comparison takes the same time whatever differs.
#[derive(Clone, PartialEq, Eq)]
pub struct BearerToken(String);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TokenError {
    #[error("a token is {TOKEN_HEX_LEN} hex digits, this one has {0} characters")]
    Length(usize),
    #[error("a token is lowercase hex digits only")]
    NotHex,
    #[error("cannot draw random bytes: {0}")]
    Entropy(String),
}

impl BearerToken {
    /// A fresh token from the operating system's random source.
    ///
    /// # Errors
    ///
    /// `Entropy` when the random source is unavailable.
    pub fn generate() -> Result<Self, TokenError> {
        let mut bytes = [0u8; TOKEN_BYTES];
        getrandom::fill(&mut bytes).map_err(|error| TokenError::Entropy(error.to_string()))?;
        Ok(Self(hex(&bytes)))
    }

    /// The contents of `sbtd.token`, surrounding whitespace ignored.
    ///
    /// # Errors
    ///
    /// `Length` or `NotHex` when the text is not a token this crate wrote.
    pub fn parse(text: &str) -> Result<Self, TokenError> {
        let text = text.trim();
        if text.len() != TOKEN_HEX_LEN {
            return Err(TokenError::Length(text.len()));
        }
        if !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TokenError::NotHex);
        }
        Ok(Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The value of the `Authorization` header a client sends.
    #[must_use]
    pub fn authorization_value(&self) -> String {
        format!("Bearer {}", self.0)
    }

    /// Whether `presented` is this token. Every byte is compared whatever
    /// the first difference, so timing reveals only the length, which is
    /// public.
    #[must_use]
    pub fn matches(&self, presented: &str) -> bool {
        let ours = self.0.as_bytes();
        let theirs = presented.as_bytes();
        if ours.len() != theirs.len() {
            return false;
        }
        ours.iter()
            .zip(theirs)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BearerToken(<redacted>)")
    }
}

/// The credentials in an `Authorization` header value, when its scheme is
/// `Bearer` (case-insensitive); `None` for any other scheme or shape.
#[must_use]
pub fn bearer_credentials(header: &str) -> Option<&str> {
    let (scheme, credentials) = header.trim().split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| credentials.trim())
        .filter(|credentials| !credentials.is_empty())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 0x0f)],
            ]
        })
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_64_hex_digits_and_distinct() {
        let a = BearerToken::generate().unwrap();
        let b = BearerToken::generate().unwrap();
        assert_eq!(a.as_str().len(), TOKEN_HEX_LEN);
        assert!(a.as_str().bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn parse_round_trips_the_file_contents_and_ignores_whitespace() {
        let token = BearerToken::generate().unwrap();
        let text = format!("{}\n", token.as_str());
        assert_eq!(BearerToken::parse(&text).unwrap(), token);
    }

    #[test]
    fn parse_refuses_anything_this_crate_would_not_write() {
        assert_eq!(BearerToken::parse("abc"), Err(TokenError::Length(3)));
        assert_eq!(BearerToken::parse(&"g".repeat(64)), Err(TokenError::NotHex));
        assert_eq!(BearerToken::parse(&"A".repeat(64)), Err(TokenError::NotHex));
    }

    #[test]
    fn matches_only_the_same_token() {
        let token = BearerToken::generate().unwrap();
        let other = BearerToken::generate().unwrap();
        assert!(token.matches(token.as_str()));
        assert!(!token.matches(other.as_str()));
        assert!(!token.matches(&token.as_str()[..63]));
        assert!(!token.matches(""));
    }

    #[test]
    fn debug_never_shows_the_token() {
        let token = BearerToken::generate().unwrap();
        let text = format!("{token:?}");
        assert_eq!(text, "BearerToken(<redacted>)");
        assert!(!text.contains(token.as_str()));
    }

    #[test]
    fn bearer_credentials_accepts_the_scheme_case_insensitively_and_nothing_else() {
        assert_eq!(bearer_credentials("Bearer abc"), Some("abc"));
        assert_eq!(bearer_credentials("bearer  abc "), Some("abc"));
        assert_eq!(bearer_credentials("Basic abc"), None);
        assert_eq!(bearer_credentials("Bearer"), None);
        assert_eq!(bearer_credentials("Bearer "), None);
        assert_eq!(bearer_credentials(""), None);
    }

    #[test]
    fn authorization_value_is_the_bearer_scheme() {
        let token = BearerToken::parse(&"0".repeat(64)).unwrap();
        assert_eq!(
            token.authorization_value(),
            format!("Bearer {}", "0".repeat(64))
        );
    }
}
