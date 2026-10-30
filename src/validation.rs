//! Validation helpers from droneengage_server_common (hlp_validation.js /
//! hlp_strings.js) that the comm server uses.

/// `/^[_a-z0-9]+$/i`
pub fn is_alphanumeric(p_text: &str) -> bool {
    !p_text.is_empty() && p_text.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphanumeric() {
        assert!(is_alphanumeric("abc_123XYZ"));
        assert!(!is_alphanumeric(""));
        assert!(!is_alphanumeric("a-b"));
        assert!(!is_alphanumeric("é"));
    }
}
