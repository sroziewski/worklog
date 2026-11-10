use worklog::keys::KeyMatcher;

fn matcher(keys: &[&str]) -> KeyMatcher {
    KeyMatcher::new(&keys.iter().map(|k| k.to_string()).collect::<Vec<_>>())
}

mod find_all {
    use super::*;

    #[test]
    fn extracts_configured_keys_per_contract() {
        let m = matcher(&["ABC", "XYZ"]);
        // (case, input, expected keys)
        let cases: &[(&str, &str, &[&str])] = &[
            ("plain key in subject", "ABC-1439 fix login", &["ABC-1439"]),
            ("lower case in branch", "feat/abc-1439-x", &["ABC-1439"]),
            ("underscore separator", "feat/abc_1439", &["ABC-1439"]),
            ("project key with digits", "XYZ-12 upload", &["XYZ-12"]),
            ("mixed case project", "Xyz_8 fix", &["XYZ-8"]),
            ("leading zeros dropped", "ABC-007 x", &["ABC-7"]),
            ("all-zero number is no key", "ABC-000 x", &[]),
            ("single zero is no key", "ABC-0", &[]),
            ("part of a longer word", "XABC-1 x", &[]),
            ("digit before the key", "1ABC-5", &[]),
            ("underscore before the key is a boundary", "feat_ABC-9", &["ABC-9"]),
            ("no separator", "ABC1439", &[]),
            ("space separator", "ABC 1439", &[]),
            ("double separator", "ABC--5", &[]),
            ("dangling separator", "ABC-", &[]),
            ("unconfigured project", "AAA-5 and XY-26", &[]),
            ("empty text", "", &[]),
            ("order kept, duplicates dropped", "abc-2, ABC-1, ABC-2, abc_1", &["ABC-2", "ABC-1"]),
            ("adjacent keys with comma", "ABC-1,ABC-2", &["ABC-1", "ABC-2"]),
            ("adjacent keys with dash", "ABC-1-ABC-2", &["ABC-1", "ABC-2"]),
            ("two projects", "XYZ-3 after ABC-4", &["XYZ-3", "ABC-4"]),
            ("zero-padded duplicate", "ABC-07 ABC-7", &["ABC-7"]),
            ("key at end of text", "fixes ABC-31", &["ABC-31"]),
        ];
        for (case, input, expected) in cases {
            assert_eq!(m.find_all(input), expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(), "case: {case} ({input:?})");
        }
    }

    // open question: the contract leaves `ABC-12a` to the code; the regex only requires a non-digit
    // after the number, so a trailing letter still yields the key. Pinned as plausibly intended
    // (`ABC-12a` in prose usually means "ABC-12, part a").
    #[test]
    fn trailing_letter_after_number_still_matches() {
        assert_eq!(matcher(&["ABC"]).find_all("ABC-12a done, ABC-5x"), vec!["ABC-12".to_string(), "ABC-5".to_string()]);
    }

    #[test]
    fn only_configured_projects_count() {
        assert_eq!(matcher(&["ABC"]).find_all("XYZ-12 and ABC-3"), vec!["ABC-3".to_string()]);
    }

    #[test]
    fn lower_case_configuration_matches_and_uppercases() {
        assert_eq!(matcher(&["abc"]).find_all("feat/abc-3"), vec!["ABC-3".to_string()]);
    }
}

mod first {
    use super::*;

    #[test]
    fn returns_first_key_in_text_order() {
        assert_eq!(matcher(&["ABC", "XYZ"]).first("x XYZ-40 then ABC-2"), Some("XYZ-40".to_string()));
    }

    #[test]
    fn returns_none_without_key() {
        assert_eq!(matcher(&["ABC", "XYZ"]).first("refactor XABC-1 ABC-000"), None);
    }
}
