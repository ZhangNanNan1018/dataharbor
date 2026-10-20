//! Shared lexical boundaries for statement splitting and query classification.
pub(super) struct Word {
    pub text: String,
    pub depth: usize,
}

pub(super) struct Scan {
    pub words: Vec<Word>,
    pub statements: Vec<(usize, usize)>,
    pub complete: bool,
}

pub(super) fn scan(sql: &str) -> Scan {
    let bytes = sql.as_bytes();
    let mut out = Scan {
        words: Vec::new(),
        statements: Vec::new(),
        complete: true,
    };
    let mut index = 0;
    let mut start = 0;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                }
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                index += 2;
                let mut nesting = 1;
                while index < bytes.len() && nesting > 0 {
                    if bytes.get(index..index + 2) == Some(b"/*") {
                        nesting += 1;
                        index += 2;
                    } else if bytes.get(index..index + 2) == Some(b"*/") {
                        nesting -= 1;
                        index += 2;
                    } else {
                        index += 1;
                    }
                }
                out.complete &= nesting == 0;
            }
            quote @ (b'\'' | b'"' | b'`' | b'[') => {
                let close = if quote == b'[' { b']' } else { quote };
                index += 1;
                let mut closed = false;
                while index < bytes.len() {
                    if bytes[index] == b'\\' && quote == b'\'' {
                        index = (index + 2).min(bytes.len());
                    } else if bytes[index] == close {
                        index += 1;
                        if bytes.get(index) == Some(&close) {
                            index += 1;
                        } else {
                            closed = true;
                            break;
                        }
                    } else {
                        index += 1;
                    }
                }
                out.complete &= closed;
            }
            b'$' => {
                let tag_start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
                {
                    index += 1;
                }
                if bytes.get(index) == Some(&b'$') {
                    index += 1;
                    let delimiter = &sql[tag_start..index];
                    if let Some(end) = sql[index..].find(delimiter) {
                        index += end + delimiter.len();
                    } else {
                        out.complete = false;
                        index = bytes.len();
                    }
                }
            }
            b'(' => {
                depth += 1;
                index += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b';' => {
                out.statements.push((start, index));
                index += 1;
                start = index;
            }
            value if value.is_ascii_alphabetic() || value == b'_' => {
                let word_start = index;
                index += 1;
                while index < bytes.len()
                    && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'$'))
                {
                    index += 1;
                }
                out.words.push(Word {
                    text: sql[word_start..index].to_ascii_uppercase(),
                    depth,
                });
            }
            _ => {
                index += 1;
            }
        }
    }
    out.statements.push((start, bytes.len()));
    out
}

pub(super) fn split(sql: &str) -> Vec<String> {
    scan(sql)
        .statements
        .into_iter()
        .filter_map(|(start, end)| {
            let statement = sql[start..end].trim();
            (!super::strip_leading_comments(statement).is_empty()).then(|| statement.to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitting_and_classification_share_quoted_boundaries() {
        let sql = "SELECT $q$a;b$q$, [semi;colon], 'c;d'; /* outer /* inner ; */ still ; */ SELECT 2; -- tail";
        let result = split(sql);
        assert_eq!(result.len(), 2);
        assert!(result[0].contains("$q$a;b$q$"));
        assert!(result[1].ends_with("SELECT 2"));
        assert_eq!(scan("SELECT $q$DELETE$q$, [UPDATE]").words.len(), 1);
        assert!(!scan("SELECT $q$unterminated").complete);
        assert!(!scan("SELECT 'unterminated").complete);
    }
}
