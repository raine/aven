#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedTaskSearchQuery {
    pub(crate) trimmed: String,
    pub(crate) fts_match: Option<String>,
    #[allow(dead_code)]
    pub(crate) phrases: Vec<String>,
    #[allow(dead_code)]
    pub(crate) tokens: Vec<String>,
    #[allow(dead_code)]
    pub(crate) active_prefix: Option<String>,
    pub(crate) ref_query: Option<ParsedRefSearchQuery>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRefSearchQuery {
    pub(crate) normalized_prefix: Option<String>,
    pub(crate) normalized_suffix: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TermKind {
    Phrase,
    Bare,
}

pub(crate) fn parse_task_search_query(input: &str) -> ParsedTaskSearchQuery {
    let trimmed = input.trim().to_string();
    let ends_in_whitespace = input.ends_with(char::is_whitespace);
    let mut phrases = Vec::new();
    let mut bare_terms = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut current_is_phrase = false;
    let mut final_term_kind = None;

    for ch in trimmed.chars() {
        if ch == '"' {
            final_term_kind = flush_term(
                &mut current,
                current_is_phrase,
                &mut phrases,
                &mut bare_terms,
            );
            in_quote = !in_quote;
            current_is_phrase = in_quote;
        } else if ch.is_whitespace() && !in_quote {
            final_term_kind = flush_term(
                &mut current,
                current_is_phrase,
                &mut phrases,
                &mut bare_terms,
            );
            current_is_phrase = false;
        } else {
            current.push(ch);
            current_is_phrase = in_quote;
        }
    }
    if let Some(kind) = flush_term(
        &mut current,
        current_is_phrase,
        &mut phrases,
        &mut bare_terms,
    ) {
        final_term_kind = Some(kind);
    }

    let active_prefix = if matches!(final_term_kind, Some(TermKind::Bare)) && !ends_in_whitespace {
        bare_terms.pop()
    } else {
        None
    };

    let tokens = bare_terms;
    let fts_match = build_fts_match(&phrases, &tokens, active_prefix.as_deref());
    let ref_query = parse_ref_query(&trimmed);

    ParsedTaskSearchQuery {
        trimmed,
        fts_match,
        phrases,
        tokens,
        active_prefix,
        ref_query,
    }
}

fn sanitize_term(term: &str) -> Option<String> {
    let sanitized = term
        .chars()
        .filter_map(|ch| match ch {
            '"' | '*' => None,
            ch if ch.is_control() => Some(' '),
            ch => Some(ch),
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized)
    }
}

fn flush_term(
    current: &mut String,
    is_phrase: bool,
    phrases: &mut Vec<String>,
    tokens: &mut Vec<String>,
) -> Option<TermKind> {
    if current.is_empty() {
        return None;
    }
    let sanitized = sanitize_term(current);
    current.clear();
    if let Some(term) = sanitized {
        if is_phrase {
            phrases.push(term);
            Some(TermKind::Phrase)
        } else {
            tokens.push(term);
            Some(TermKind::Bare)
        }
    } else {
        None
    }
}

fn build_fts_match(
    phrases: &[String],
    tokens: &[String],
    active_prefix: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    for phrase in phrases {
        parts.push(format!("\"{}\"", phrase));
    }
    for token in tokens {
        parts.push(format!("\"{}\"", token));
    }
    if let Some(prefix) = active_prefix {
        parts.push(format!("\"{}\"*", prefix));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn parse_ref_query(input: &str) -> Option<ParsedRefSearchQuery> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.contains(char::is_whitespace) {
        return None;
    }
    let raw = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let groups = raw
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|group| !group.is_empty())
        .collect::<Vec<_>>();
    if groups.is_empty() {
        return None;
    }
    if groups.len() >= 2 && groups[0].chars().all(|c| c.is_ascii_alphabetic()) {
        let suffix = groups[1..].join("");
        if suffix.len() >= 3 {
            return Some(ParsedRefSearchQuery {
                normalized_prefix: Some(normalize_ref_string(groups[0])),
                normalized_suffix: normalize_ref_string(&suffix),
            });
        }
    }
    let suffix = groups.join("");
    if suffix.len() >= 3 {
        return Some(ParsedRefSearchQuery {
            normalized_prefix: None,
            normalized_suffix: normalize_ref_string(&suffix),
        });
    }
    None
}

fn normalize_ref_string(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_search_parser_empty_input_has_no_match_expression() {
        let parsed = parse_task_search_query("   ");
        assert!(parsed.trimmed.is_empty());
        assert_eq!(parsed.fts_match, None);
        assert!(parsed.phrases.is_empty());
        assert!(parsed.tokens.is_empty());
        assert_eq!(parsed.active_prefix, None);
        assert_eq!(parsed.ref_query, None);
    }

    #[test]
    fn task_search_parser_builds_safe_match_for_special_characters() {
        let parsed = parse_task_search_query("\"a*b\" AND (c OR d):");
        let fts_match = parsed.fts_match.as_deref().unwrap();
        assert!(fts_match.contains("\"ab\""));
        assert!(fts_match.contains("\"AND\""));
        assert!(fts_match.contains("\"(c\""));
        assert!(fts_match.contains("\"OR\""));
        assert!(fts_match.contains("\"d):\"*"));
        assert!(!fts_match.contains("a*b"));
    }

    #[test]
    fn task_search_parser_keeps_quoted_phrase_atomic() {
        let parsed = parse_task_search_query("\"pager rotation\" security");
        assert_eq!(parsed.phrases, vec!["pager rotation".to_string()]);
        assert!(parsed.tokens.is_empty());
        assert_eq!(parsed.active_prefix.as_deref(), Some("security"));
        assert_eq!(
            parsed.fts_match.as_deref(),
            Some("\"pager rotation\" \"security\"*")
        );
    }

    #[test]
    fn task_search_parser_marks_active_final_token() {
        let parsed = parse_task_search_query("ios auth");
        assert_eq!(parsed.tokens, vec!["ios".to_string()]);
        assert_eq!(parsed.active_prefix.as_deref(), Some("auth"));
        assert_eq!(parsed.fts_match.as_deref(), Some("\"ios\" \"auth\"*"));

        let complete = parse_task_search_query("ios auth ");
        assert_eq!(complete.tokens, vec!["ios".to_string(), "auth".to_string()]);
        assert_eq!(complete.active_prefix, None);
        assert_eq!(complete.fts_match.as_deref(), Some("\"ios\" \"auth\""));
    }

    #[test]
    fn task_search_parser_identifies_ref_shapes() {
        let suffix = parse_task_search_query("7KQ9").ref_query.unwrap();
        assert_eq!(suffix.normalized_prefix, None);
        assert_eq!(suffix.normalized_suffix, "7KQ9");

        let qualified = parse_task_search_query("/APP-7OKI").ref_query.unwrap();
        assert_eq!(qualified.normalized_prefix.as_deref(), Some("APP"));
        assert_eq!(qualified.normalized_suffix, "70K1");

        assert_eq!(parse_task_search_query("release cleanup").ref_query, None);
    }

    #[test]
    fn task_search_parser_identifies_punctuation_insensitive_ref_shapes() {
        let qualified = parse_task_search_query("/APP.7OKI").ref_query.unwrap();
        assert_eq!(qualified.normalized_prefix.as_deref(), Some("APP"));
        assert_eq!(qualified.normalized_suffix, "70K1");

        let suffix = parse_task_search_query("7KQ-9").ref_query.unwrap();
        assert_eq!(suffix.normalized_prefix, None);
        assert_eq!(suffix.normalized_suffix, "7KQ9");

        let durable = parse_task_search_query("7KQ9A1X4MV2P8D6R")
            .ref_query
            .unwrap();
        assert_eq!(durable.normalized_suffix, "7KQ9A1X4MV2P8D6R");
    }

    #[tokio::test]
    async fn task_search_parser_match_expression_compiles_in_fts5() {
        use sqlx::{Connection, SqliteConnection};

        let mut conn = SqliteConnection::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE VIRTUAL TABLE docs USING fts5(body)")
            .execute(&mut conn)
            .await
            .unwrap();

        for input in [
            "\"",
            "\"\"",
            "(",
            ")",
            "a*b",
            "\"(",
            "AND OR NOT",
            ":",
            "/",
            "-",
            "a:b",
            "\"unfinished",
            "x OR y",
            "\"pager rotation\" security",
            "foo \"*\"",
        ] {
            let parsed = parse_task_search_query(input);
            if let Some(fts_match) = parsed.fts_match {
                sqlx::query("SELECT rowid FROM docs WHERE docs MATCH ?")
                    .bind(&fts_match)
                    .fetch_all(&mut conn)
                    .await
                    .unwrap_or_else(|err| {
                        panic!("input {input:?} produced invalid MATCH {fts_match:?}: {err}")
                    });
            }
        }
    }
}
