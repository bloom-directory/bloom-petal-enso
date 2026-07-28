pub fn sanitize_message(input: &str) -> String {
    let normalized = input.replace(['\r', '\n'], " ");
    let words = normalized.split_whitespace().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut redact_next = false;
    for word in words {
        if redact_next {
            out.push("[redacted]");
            redact_next = false;
            continue;
        }
        if word.eq_ignore_ascii_case("bearer") {
            out.push("Bearer");
            redact_next = true;
        } else if word.eq_ignore_ascii_case("authorization:") {
            out.push("Authorization:");
        } else {
            out.push(word);
        }
    }
    out.join(" ").chars().take(512).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removes_bearer_values() {
        let out = sanitize_message("Authorization: Bearer secret-token");
        assert!(!out.contains("secret-token"));
    }
}
