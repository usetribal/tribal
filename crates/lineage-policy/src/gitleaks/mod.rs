mod config;
mod detect;
mod entropy;

#[cfg(test)]
mod conformance;

pub(crate) const REDACTED: &str = "[REDACTED]";

/// Redact secret spans in `text` using the embedded gitleaks rule set.
pub fn redact_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    apply_span_redactions(text, &detect::find_secret_spans(text))
}

pub(crate) fn apply_span_redactions(text: &str, spans: &[config::Span]) -> String {
    if spans.is_empty() {
        return text.to_string();
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for span in spans {
        if span.start < cursor {
            continue;
        }
        if !text.is_char_boundary(span.start) || !text.is_char_boundary(span.end) {
            continue;
        }
        out.push_str(&text[cursor..span.start]);
        out.push_str(REDACTED);
        cursor = span.end;
    }
    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitleaks::config::Span;

    #[test]
    fn preserves_surrounding_context() {
        let text = "Use token sk_test_abcdefghijklmnopqrstuvwxyz in staging only.";
        let redacted = redact_text(text);
        assert!(redacted.contains("Use token"));
        assert!(redacted.contains(REDACTED));
        assert!(!redacted.contains("sk_test_"));
    }

    #[test]
    fn leaves_clean_text_unchanged() {
        let text = "I'll explore the Pulumi infrastructure code to map the architecture.";
        assert_eq!(redact_text(text), text);
    }

    #[test]
    fn empty_input_returns_empty_string() {
        assert_eq!(redact_text(""), "");
    }

    #[test]
    fn apply_span_redactions_skips_out_of_order_spans() {
        let text = "abcdef";
        let out = apply_span_redactions(
            text,
            &[Span { start: 0, end: 2 }, Span { start: 1, end: 3 }],
        );
        assert_eq!(out, "[REDACTED]cdef");
    }

    #[test]
    fn apply_span_redactions_skips_non_char_boundaries() {
        let text = "a☃b";
        let out = apply_span_redactions(text, &[Span { start: 1, end: 2 }]);
        assert_eq!(out, "a☃b");
    }
}
