//! Redaction helpers for diagnostic output.

/// Redact secret-like values before writing diagnostic text or JSON.
pub(crate) fn redact_diagnostic_value(value: &str) -> String {
    let without_query_secrets = redact_sensitive_query_values(value);
    let without_userinfo = redact_url_userinfo(&without_query_secrets);
    shipper_output_sanitizer::redact_sensitive(&without_userinfo)
}

fn redact_sensitive_query_values(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let delimiter = bytes[i] as char;
        if delimiter == '?' || delimiter == '&' {
            out.push(delimiter);
            i += 1;

            let key_start = i;
            while i < bytes.len() && !matches!(bytes[i] as char, '=' | '&' | '#' | ' ' | '"' | '\'')
            {
                i += 1;
            }
            let key = &value[key_start..i];
            out.push_str(key);

            if i < bytes.len() && bytes[i] == b'=' {
                out.push('=');
                i += 1;
                let value_start = i;
                while i < bytes.len()
                    && !matches!(bytes[i] as char, '&' | '#' | ' ' | '"' | '\'' | ')' | ']')
                {
                    i += 1;
                }
                if is_sensitive_query_key(key) {
                    out.push_str("[REDACTED]");
                } else {
                    out.push_str(&value[value_start..i]);
                }
            }
        } else {
            let ch = value[i..].chars().next().unwrap_or('\0');
            out.push(ch);
            i += ch.len_utf8();
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

    while let Some(scheme_pos) = remaining.find("://") {
        let authority_start = scheme_pos + "://".len();
        out.push_str(&remaining[..authority_start]);
        let after_scheme = &remaining[authority_start..];
        let authority_end = after_scheme
            .find(['/', '?', '#', ' ', '"', '\''])
            .unwrap_or(after_scheme.len());
        let authority = &after_scheme[..authority_end];

        if let Some(at_pos) = authority.rfind('@') {
            out.push_str("[REDACTED]@");
            out.push_str(&authority[at_pos + 1..]);
        } else {
            out.push_str(authority);
        }

        remaining = &after_scheme[authority_end..];
    }

    out.push_str(remaining);
    out
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
