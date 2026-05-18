//! Redaction helpers for diagnostic output.

/// Redact secret-like values before writing diagnostic text or JSON.
pub(crate) fn redact_diagnostic_value(value: &str) -> String {
    let without_query_secrets = redact_sensitive_query_values(value);
    let without_userinfo = redact_url_userinfo(&without_query_secrets);
    shipper_output_sanitizer::redact_sensitive(&without_userinfo)
}

fn redact_sensitive_query_values(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '?' || ch == '&' {
            out.push(ch);

            let mut key = String::new();
            while let Some(next) = chars.peek().copied() {
                if matches!(next, '=' | '&' | '#' | ' ' | '"' | '\'') {
                    break;
                }
                key.push(next);
                chars.next();
            }
            out.push_str(&key);

            if matches!(chars.peek(), Some('=')) {
                out.push('=');
                chars.next();

                let mut raw_value = String::new();
                while let Some(next) = chars.peek().copied() {
                    if matches!(next, '&' | '#' | ' ' | '"' | '\'' | ')' | ']') {
                        break;
                    }
                    raw_value.push(next);
                    chars.next();
                }
                if is_sensitive_query_key(&key) {
                    out.push_str("[REDACTED]");
                } else {
                    out.push_str(&raw_value);
                }
            }
        } else {
            out.push(ch);
        }
    }

    out
}

fn is_sensitive_query_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("passwd")
        || key == "key"
        || key.ends_with("_key")
        || key.contains("auth")
        || key.contains("credential")
}

fn redact_url_userinfo(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut remaining = value;

    while let Some((before_scheme, after_scheme)) = remaining.split_once("://") {
        out.push_str(before_scheme);
        out.push_str("://");

        let (authority, rest) = split_authority(after_scheme);
        if let Some((_userinfo, host)) = authority.rsplit_once('@') {
            out.push_str("[REDACTED]@");
            out.push_str(host);
        } else {
            out.push_str(authority);
        }

        remaining = rest;
    }

    out.push_str(remaining);
    out
}

fn split_authority(after_scheme: &str) -> (&str, &str) {
    for (idx, ch) in after_scheme.char_indices() {
        if matches!(ch, '/' | '?' | '#' | ' ' | '"' | '\'') {
            return after_scheme.split_at(idx);
        }
    }
    (after_scheme, "")
}

#[cfg(test)]
mod tests {
    use super::redact_diagnostic_value;

    #[test]
    fn redacts_token_like_url_query_values() {
        let input = "https://registry.example/api?token=abc&scope=all&api_key=def";
        let out = redact_diagnostic_value(input);

        assert!(!out.contains("abc"));
        assert!(!out.contains("def"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_token_like_values_inside_error_text() {
        let input = "request failed for https://registry.example/api?client_secret=s3cr3t: timeout";
        let out = redact_diagnostic_value(input);

        assert!(!out.contains("s3cr3t"));
        assert!(out.contains("[REDACTED]"));
        assert!(out.contains("timeout"));
    }

    #[test]
    fn redacts_url_userinfo() {
        let input = "https://user:password@registry.example/api";
        let out = redact_diagnostic_value(input);

        assert_eq!(out, "https://[REDACTED]@registry.example/api");
    }

    #[test]
    fn preserves_non_sensitive_urls() {
        let input = "https://registry.example/api?scope=all&crate=shipper";
        let out = redact_diagnostic_value(input);

        assert_eq!(out, input);
    }
}
