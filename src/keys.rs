use regex::Regex;

/// Finds Jira issue keys of the given projects in free text (commit messages, branch names).
pub struct KeyMatcher {
    re: Regex,
}

impl KeyMatcher {
    pub fn new(project_keys: &[String]) -> Self {
        let alts: Vec<String> = project_keys.iter().map(|k| regex::escape(&k.to_uppercase())).collect();
        // No keys would leave an empty group matching a bare "-12"; this never matches.
        if alts.is_empty() {
            return Self { re: Regex::new(r"[^\s\S]").expect("valid regex") };
        }
        // Branch names spell keys in lower case and sometimes with '_' (feat/hd_1439); the
        // lookarounds regex lacks are replaced by explicit non-alphanumeric boundaries.
        let pattern = format!(r"(?i)(?:^|[^A-Za-z0-9])({})[-_](\d+)(?:$|[^0-9])", alts.join("|"));
        Self { re: Regex::new(&pattern).expect("escaped project keys form a valid regex") }
    }

    /// Keys in order of first appearance, upper-cased, without duplicates; leading zeros dropped.
    pub fn find_all(&self, text: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut pos = 0;
        while let Some(c) = self.re.captures_at(text, pos) {
            let num = c[2].trim_start_matches('0');
            if !num.is_empty() {
                let key = format!("{}-{}", c[1].to_uppercase(), num);
                if !out.contains(&key) {
                    out.push(key);
                }
            }
            // Resume at the number's end so the boundary char consumed after it can start the
            // next match ("ABC-1,ABC-2").
            pos = c.get(2).unwrap().end();
        }
        out
    }

    pub fn first(&self, text: &str) -> Option<String> {
        self.find_all(text).into_iter().next()
    }
}
