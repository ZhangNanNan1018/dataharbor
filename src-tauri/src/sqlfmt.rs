const KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "JOIN", "LEFT", "RIGHT", "INNER", "FULL", "CROSS",
    "ON", "GROUP", "ORDER", "BY", "HAVING", "LIMIT", "OFFSET", "UNION", "INSERT", "INTO",
    "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "ALTER", "DROP", "TABLE", "VIEW", "INDEX",
    "AS", "DISTINCT", "WITH", "CASE", "WHEN", "THEN", "ELSE", "END", "NOT", "NULL", "IS", "IN",
    "LIKE", "BETWEEN", "EXISTS", "PRIMARY", "KEY", "FOREIGN", "REFERENCES", "CONSTRAINT",
];

pub fn format_sql(sql: &str, minify: bool) -> String {
    let tokens = tokenize(sql);
    if minify {
        return tokens
            .into_iter()
            .map(|token| match token {
                Token::Word(word) if is_keyword(&word) => word.to_ascii_uppercase(),
                Token::Word(word) => word,
                Token::Other(text) => text,
            })
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for token in tokens {
        let word = match &token {
            Token::Word(word) => word.clone(),
            Token::Other(text) => text.clone(),
        };
        let upper = word.to_ascii_uppercase();
        let break_before = matches!(
            upper.as_str(),
            "SELECT" | "FROM" | "WHERE" | "GROUP" | "ORDER" | "HAVING" | "LIMIT" | "UNION"
                | "INSERT" | "UPDATE" | "DELETE" | "VALUES" | "SET" | "JOIN" | "LEFT" | "RIGHT"
                | "INNER" | "FULL" | "CROSS"
        );
        if break_before && !current.trim().is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        let rendered = if is_keyword(&word) { upper } else { word };
        if !current.is_empty() && !current.ends_with('(') {
            current.push(' ');
        }
        current.push_str(&rendered);
    }
    if !current.trim().is_empty() {
        lines.push(current);
    }
    lines.join("\n")
}

enum Token {
    Word(String),
    Other(String),
}

fn tokenize(sql: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if ch == '\'' || ch == '"' || ch == '`' {
            let mut text = String::new();
            text.push(ch);
            while let Some(next) = chars.next() {
                text.push(next);
                if next == ch {
                    if chars.peek() == Some(&ch) {
                        text.push(chars.next().unwrap());
                        continue;
                    }
                    break;
                }
            }
            tokens.push(Token::Other(text));
            continue;
        }
        if ch == '-' && chars.peek() == Some(&'-') {
            chars.next();
            while let Some(next) = chars.next() {
                if next == '\n' {
                    break;
                }
            }
            continue;
        }
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '@' || ch == ':' {
            let mut text = String::from(ch);
            while let Some(next) = chars.peek() {
                if next.is_ascii_alphanumeric() || *next == '_' {
                    text.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            tokens.push(Token::Word(text));
            continue;
        }
        tokens.push(Token::Other(ch.to_string()));
    }
    tokens
}

fn is_keyword(word: &str) -> bool {
    KEYWORDS.iter().any(|item| item.eq_ignore_ascii_case(word))
}

pub fn substitute_params(sql: &str, params: &[(String, String)]) -> String {
    let mut out = String::new();
    let chars: Vec<char> = sql.chars().collect();
    let mut index = 0;
    let mut quote: Option<char> = None;
    while index < chars.len() {
        let ch = chars[index];
        if let Some(mark) = quote {
            out.push(ch);
            if ch == mark {
                quote = None;
            }
            index += 1;
            continue;
        }
        if ch == '\'' || ch == '"' || ch == '`' {
            quote = Some(ch);
            out.push(ch);
            index += 1;
            continue;
        }
        if ch == ':' || ch == '@' {
            let start = index + 1;
            let mut end = start;
            if end < chars.len() && (chars[end].is_ascii_alphabetic() || chars[end] == '_') {
                end += 1;
                while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                    end += 1;
                }
                let name: String = chars[start..end].iter().collect();
                if let Some((_, value)) = params.iter().find(|(key, _)| key.eq_ignore_ascii_case(&name)) {
                    out.push_str(value);
                    index = end;
                    continue;
                }
            }
        }
        out.push(ch);
        index += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beautify_and_minify_round_trip_keywords() {
        let pretty = format_sql("select id, title from books where price > 10", false);
        assert!(pretty.contains("SELECT"));
        assert!(pretty.contains("\nFROM"));
        assert!(pretty.contains("\nWHERE"));
        let mini = format_sql(&pretty, true);
        assert!(!mini.contains('\n'));
        assert!(mini.contains("SELECT"));
    }

    #[test]
    fn params_skip_strings() {
        let sql = substitute_params(
            "SELECT * FROM books WHERE title = :title AND note = 'keep :title'",
            &[("title".into(), "'活着'".into())],
        );
        assert!(sql.contains("title = '活着'"));
        assert!(sql.contains("'keep :title'"));
    }
}
