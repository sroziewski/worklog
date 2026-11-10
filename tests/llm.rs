//! LLM credentials, answer parsing and the chat client against a local tiny_http mock; no request
//! leaves 127.0.0.1 and no real opencode config is read.

mod common;

use std::collections::HashMap;
use std::path::PathBuf;

use common::MockJira;
use serde_json::json;
use worklog::config::TicketConfig;
use worklog::llm::{self, Llm, LlmCredentials};

const KEY: &str = "sk-llm-9Zq4-secret";

/// Environment of one table row, as (name, value) pairs.
type Env = &'static [(&'static str, &'static str)];

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |k: &str| m.get(k).cloned()
}

mod credentials {
    use super::*;

    /// opencode config in a temp dir; `opencode_config` always points there (or nowhere), never at
    /// the real file the default names. The defaults name no provider or model, so both are set.
    fn cfg(dir: &tempfile::TempDir, opencode: Option<serde_json::Value>, llm_url: Option<&str>) -> TicketConfig {
        let p = dir.path().join("opencode.json");
        if let Some(v) = opencode {
            std::fs::write(&p, v.to_string()).unwrap();
        }
        TicketConfig { opencode_config: Some(p), llm_url: llm_url.map(str::to_owned), ..named() }
    }

    fn named() -> TicketConfig {
        TicketConfig { llm_provider: "myllm".into(), llm_model: "my-model".into(), ..TicketConfig::default() }
    }

    fn provider(name: &str, base_url: &str, api_key: &str) -> serde_json::Value {
        json!({"providers": {name: {"settings": {"baseURL": base_url, "apiKey": api_key}}}})
    }

    fn resolve(cfg: &TicketConfig, env: &[(&str, &str)]) -> (String, String, String) {
        let c = llm::credentials(cfg, env_of(env)).unwrap();
        (c.url, c.api_key, c.model)
    }

    const FILE_URL: &str = "https://file.example.test/v1";
    const NO_URL: &str = "no LLM endpoint: set LLM_URL, tickets.llm_url, or provider 'myllm' in opencode's config";
    const NO_KEY: &str = "no LLM API key: set LLM_API_KEY or provider 'myllm' apiKey in opencode's config";

    #[test]
    fn opencode_file_alone_supplies_url_and_key_model_from_config() {
        let d = tempfile::tempdir().unwrap();
        let c = cfg(&d, Some(provider("myllm", FILE_URL, "file-key")), None);
        assert_eq!(resolve(&c, &[]), (FILE_URL.into(), "file-key".into(), "my-model".into()));
    }

    #[test]
    fn url_precedence_env_then_config_then_file_skipping_blanks() {
        // (name, env LLM_URL, cfg llm_url, expected url)
        let rows: [(&str, Option<&str>, Option<&str>, &str); 7] = [
            ("env beats config and file", Some("https://env.example.test"), Some("https://cfg.example.test"), "https://env.example.test"),
            ("config beats file", None, Some("https://cfg.example.test"), "https://cfg.example.test"),
            ("file when neither set", None, None, FILE_URL),
            ("empty env falls to config", Some(""), Some("https://cfg.example.test"), "https://cfg.example.test"),
            ("blank env falls to config", Some(" \t "), Some("https://cfg.example.test"), "https://cfg.example.test"),
            ("blank env and blank config fall to file", Some("  "), Some(" "), FILE_URL),
            ("empty config falls to file", None, Some(""), FILE_URL),
        ];
        let d = tempfile::tempdir().unwrap();
        for (name, env_url, cfg_url, expected) in rows {
            let c = cfg(&d, Some(provider("myllm", FILE_URL, "file-key")), cfg_url);
            let mut env = vec![];
            if let Some(u) = env_url {
                env.push(("LLM_URL", u));
            }
            assert_eq!(llm::credentials(&c, env_of(&env)).unwrap().url, expected, "{name}");
        }
    }

    #[test]
    fn key_and_model_precedence_env_first_blank_skipped() {
        let d = tempfile::tempdir().unwrap();
        let c = TicketConfig { llm_model: "cfg-model".into(), ..cfg(&d, Some(provider("myllm", FILE_URL, "file-key")), None) };
        // (name, env, expected (key, model))
        let rows: [(&str, Env, (&str, &str)); 3] = [
            ("env wins", &[("LLM_API_KEY", "env-key"), ("LLM_MODEL", "env-model")], ("env-key", "env-model")),
            ("blank env skipped", &[("LLM_API_KEY", "  "), ("LLM_MODEL", "\t")], ("file-key", "cfg-model")),
            ("unset env", &[], ("file-key", "cfg-model")),
        ];
        for (name, env, (key, model)) in rows {
            let r = resolve(&c, env);
            assert_eq!((r.1.as_str(), r.2.as_str()), (key, model), "{name}");
        }
    }

    #[test]
    fn env_alone_is_enough_without_opencode_file() {
        let d = tempfile::tempdir().unwrap();
        let env = [("LLM_URL", "https://env.example.test/"), ("LLM_API_KEY", "env-key"), ("LLM_MODEL", "env-model")];
        let expected = ("https://env.example.test".to_string(), "env-key".to_string(), "env-model".to_string());
        // Nonexistent file.
        assert_eq!(resolve(&cfg(&d, None, None), &env), expected);
        // No file configured.
        assert_eq!(resolve(&TicketConfig { opencode_config: None, ..TicketConfig::default() }, &env), expected);
    }

    #[test]
    fn trailing_slashes_are_trimmed_from_every_source() {
        let d = tempfile::tempdir().unwrap();
        let c = cfg(&d, Some(provider("myllm", "https://file.example.test/v1/", "k")), Some("https://cfg.example.test/v2//"));
        assert_eq!(resolve(&c, &[("LLM_URL", "https://env.example.test/v3/")]).0, "https://env.example.test/v3");
        assert_eq!(resolve(&c, &[]).0, "https://cfg.example.test/v2");
        let c = cfg(&d, Some(provider("myllm", "https://file.example.test/v1/", "k")), None);
        assert_eq!(resolve(&c, &[]).0, "https://file.example.test/v1");
    }

    #[test]
    fn env_placeholders_in_opencode_resolve_through_env_fn() {
        let d = tempfile::tempdir().unwrap();
        let c = cfg(&d, Some(provider("myllm", "{env:MYLLM_BASE}", "{env:MYLLM_KEY}")), None);
        assert_eq!(
            resolve(&c, &[("MYLLM_BASE", "https://placeholder.example.test/"), ("MYLLM_KEY", "placeholder-key")]),
            ("https://placeholder.example.test".into(), "placeholder-key".into(), "my-model".into())
        );
    }

    #[test]
    fn unset_or_blank_placeholder_counts_as_missing() {
        let d = tempfile::tempdir().unwrap();
        let c = cfg(&d, Some(provider("myllm", "{env:MYLLM_BASE}", "{env:MYLLM_KEY}")), None);
        // (name, env, expected error)
        let rows: [(&str, Env, &str); 4] = [
            ("url placeholder unset", &[("MYLLM_KEY", "k")], NO_URL),
            ("url placeholder blank", &[("MYLLM_BASE", " "), ("MYLLM_KEY", "k")], NO_URL),
            ("key placeholder unset", &[("MYLLM_BASE", "https://p.example.test")], NO_KEY),
            ("key placeholder blank", &[("MYLLM_BASE", "https://p.example.test"), ("MYLLM_KEY", "")], NO_KEY),
        ];
        for (name, env, expected) in rows {
            let e = llm::credentials(&c, env_of(env)).unwrap_err();
            assert_eq!(e.to_string(), expected, "{name}");
        }
    }

    #[test]
    fn configured_provider_is_the_one_read() {
        let d = tempfile::tempdir().unwrap();
        let file = json!({"providers": {
            "myllm": {"settings": {"baseURL": "https://myllm.example.test", "apiKey": "myllm-key"}},
            "zeta": {"settings": {"baseURL": "https://zeta.example.test", "apiKey": "zeta-key"}}
        }});
        let c = TicketConfig { llm_provider: "zeta".into(), ..cfg(&d, Some(file), None) };
        assert_eq!(resolve(&c, &[]), ("https://zeta.example.test".into(), "zeta-key".into(), "my-model".into()));
    }

    #[test]
    fn provider_missing_from_file_names_it_in_the_error() {
        let d = tempfile::tempdir().unwrap();
        let c = TicketConfig { llm_provider: "zeta".into(), ..cfg(&d, Some(provider("myllm", FILE_URL, "k")), None) };
        let e = llm::credentials(&c, env_of(&[])).unwrap_err();
        assert_eq!(e.to_string(), "no LLM endpoint: set LLM_URL, tickets.llm_url, or provider 'zeta' in opencode's config");
    }

    #[test]
    fn missing_url_errors_mention_llm_url() {
        let d = tempfile::tempdir().unwrap();
        // (name, opencode content, env)
        let rows: [(&str, Option<serde_json::Value>, Env); 4] = [
            ("no file, only key in env", None, &[("LLM_API_KEY", "k")]),
            ("file without providers", Some(json!({"theme": "dark"})), &[("LLM_API_KEY", "k")]),
            ("provider without baseURL", Some(json!({"providers": {"myllm": {"settings": {"apiKey": "k"}}}})), &[]),
            ("baseURL not a string", Some(json!({"providers": {"myllm": {"settings": {"baseURL": 8080, "apiKey": "k"}}}})), &[]),
        ];
        for (name, file, env) in rows {
            let e = llm::credentials(&cfg(&d, file, None), env_of(env)).unwrap_err();
            assert_eq!(e.to_string(), NO_URL, "{name}");
            let _ = std::fs::remove_file(d.path().join("opencode.json"));
        }
    }

    #[test]
    fn missing_key_errors_mention_llm_api_key_and_config_url_is_no_key_source() {
        let d = tempfile::tempdir().unwrap();
        let c = cfg(&d, Some(json!({"providers": {"myllm": {"settings": {"baseURL": FILE_URL}}}})), Some("https://cfg.example.test"));
        let e = llm::credentials(&c, env_of(&[("LLM_URL", "https://env.example.test")])).unwrap_err();
        assert_eq!(e.to_string(), NO_KEY);
    }

    /// Config whose opencode file holds text that is not JSON (opencode itself accepts comments).
    fn malformed(dir: &tempfile::TempDir, llm_url: Option<&str>) -> (TicketConfig, PathBuf) {
        let p = dir.path().join("opencode.json");
        std::fs::write(&p, "{ // comment\n \"providers\": { ").unwrap();
        (TicketConfig { opencode_config: Some(p.clone()), llm_url: llm_url.map(str::to_owned), ..named() }, p)
    }

    #[test]
    fn malformed_opencode_file_is_not_read_when_url_and_key_are_given() {
        let d = tempfile::tempdir().unwrap();
        // (name, cfg llm_url, env)
        let rows: [(&str, Option<&str>, Env, &str); 2] = [
            ("env url and key", None, &[("LLM_URL", "https://env.example.test"), ("LLM_API_KEY", "env-key")], "https://env.example.test"),
            ("config url and env key", Some("https://cfg.example.test"), &[("LLM_API_KEY", "env-key")], "https://cfg.example.test"),
        ];
        for (name, llm_url, env, url) in rows {
            let (c, _) = malformed(&d, llm_url);
            assert_eq!(resolve(&c, env), (url.into(), "env-key".into(), "my-model".into()), "{name}");
        }
    }

    #[test]
    fn malformed_opencode_file_is_an_error_naming_it_when_needed() {
        let d = tempfile::tempdir().unwrap();
        // (name, cfg llm_url, env): url or key still missing after env and config.
        let rows: [(&str, Option<&str>, Env); 4] = [
            ("key missing", None, &[("LLM_URL", "https://env.example.test")]),
            ("key missing, url from config", Some("https://cfg.example.test"), &[]),
            ("url missing", None, &[("LLM_API_KEY", "env-key")]),
            ("both missing", None, &[]),
        ];
        for (name, llm_url, env) in rows {
            let (c, p) = malformed(&d, llm_url);
            let e = llm::credentials(&c, env_of(env)).unwrap_err();
            assert_eq!(e.to_string(), format!("parsing {}", p.display()), "{name}");
        }
    }

    #[test]
    fn file_fills_only_what_env_left_unset() {
        let d = tempfile::tempdir().unwrap();
        let c = cfg(&d, Some(provider("myllm", FILE_URL, "file-key")), None);
        assert_eq!(resolve(&c, &[("LLM_URL", "https://env.example.test")]), ("https://env.example.test".into(), "file-key".into(), "my-model".into()));
        assert_eq!(resolve(&c, &[("LLM_API_KEY", "env-key")]), (FILE_URL.into(), "env-key".into(), "my-model".into()));
    }

    #[test]
    fn upstream_provider_options_schema_is_the_fallback() {
        let d = tempfile::tempdir().unwrap();
        let upstream = |name: &str, url: &str, key: &str| json!({"provider": {name: {"options": {"baseURL": url, "apiKey": key}}}});
        // (name, file, env, expected (url, key))
        let rows: [(&str, serde_json::Value, Env, (&str, &str)); 4] = [
            ("upstream alone", upstream("myllm", "https://up.example.test/", "up-key"), &[], ("https://up.example.test", "up-key")),
            (
                "upstream with env placeholders",
                upstream("myllm", "{env:UP_URL}", "{env:UP_KEY}"),
                &[("UP_URL", "https://up-env.example.test"), ("UP_KEY", "up-env-key")],
                ("https://up-env.example.test", "up-env-key"),
            ),
            (
                "providers.settings wins over provider.options",
                json!({
                    "providers": {"myllm": {"settings": {"baseURL": "https://settings.example.test", "apiKey": "settings-key"}}},
                    "provider": {"myllm": {"options": {"baseURL": "https://options.example.test", "apiKey": "options-key"}}}
                }),
                &[],
                ("https://settings.example.test", "settings-key"),
            ),
            (
                "settings of another provider do not block our options",
                json!({
                    "providers": {"zeta": {"settings": {"baseURL": "https://zeta.example.test", "apiKey": "zeta-key"}}},
                    "provider": {"myllm": {"options": {"baseURL": "https://options.example.test", "apiKey": "options-key"}}}
                }),
                &[],
                ("https://options.example.test", "options-key"),
            ),
        ];
        for (name, file, env, (url, key)) in rows {
            let r = resolve(&cfg(&d, Some(file), None), env);
            assert_eq!((r.0.as_str(), r.1.as_str()), (url, key), "{name}");
        }
    }

    #[test]
    fn upstream_entry_of_another_provider_is_not_used() {
        let d = tempfile::tempdir().unwrap();
        let file = json!({"provider": {"zeta": {"options": {"baseURL": "https://zeta.example.test", "apiKey": "zeta-key"}}}});
        let e = llm::credentials(&cfg(&d, Some(file), None), env_of(&[])).unwrap_err();
        assert_eq!(e.to_string(), NO_URL);
    }

    #[test]
    fn blank_provider_errors_say_where_to_name_one() {
        let d = tempfile::tempdir().unwrap();
        const BLANK_NO_URL: &str =
            "no LLM endpoint: set LLM_URL, tickets.llm_url, or a provider (WORKLOG_LLM_PROVIDER in .env or tickets.llm_provider) in opencode's config";
        const BLANK_NO_KEY: &str =
            "no LLM API key: set LLM_API_KEY or a provider (WORKLOG_LLM_PROVIDER in .env or tickets.llm_provider) apiKey in opencode's config";
        // (name, provider, opencode file, env, expected error)
        let rows: [(&str, &str, Option<serde_json::Value>, Env, &str); 6] = [
            ("empty provider, nothing anywhere", "", None, &[], BLANK_NO_URL),
            ("whitespace provider, file has other providers", " \t", Some(provider("myllm", FILE_URL, "k")), &[("LLM_API_KEY", "k")], BLANK_NO_URL),
            ("empty provider, url from env, no key", "", None, &[("LLM_URL", "https://env.example.test")], BLANK_NO_KEY),
            ("whitespace provider, url from env, no key", "  ", Some(provider("myllm", FILE_URL, "k")), &[("LLM_URL", "https://env.example.test")], BLANK_NO_KEY),
            ("named provider keeps its name, no url", "zeta", None, &[], "no LLM endpoint: set LLM_URL, tickets.llm_url, or provider 'zeta' in opencode's config"),
            ("named provider keeps its name, no key", "zeta", None, &[("LLM_URL", "https://env.example.test")], "no LLM API key: set LLM_API_KEY or provider 'zeta' apiKey in opencode's config"),
        ];
        for (name, provider_name, file, env, expected) in rows {
            let _ = std::fs::remove_file(d.path().join("opencode.json"));
            let c = TicketConfig { llm_provider: provider_name.into(), ..cfg(&d, file, None) };
            let e = llm::credentials(&c, env_of(env)).unwrap_err();
            assert_eq!(e.to_string(), expected, "{name}");
        }
    }

    #[test]
    fn model_from_env_else_config_else_an_error() {
        let d = tempfile::tempdir().unwrap();
        const NO_MODEL: &str = "no LLM model: set LLM_MODEL, WORKLOG_LLM_MODEL in .env, or tickets.llm_model";
        let base = [("LLM_URL", "https://env.example.test"), ("LLM_API_KEY", "env-key")];
        // (name, cfg llm_model, env LLM_MODEL (None: unset), expected model or error)
        type Row = (&'static str, &'static str, Option<&'static str>, Result<&'static str, &'static str>);
        let rows: [Row; 6] = [
            ("both unset", "", None, Err(NO_MODEL)),
            ("config whitespace, env blank", " \t", Some("  "), Err(NO_MODEL)),
            ("config empty, env empty", "", Some(""), Err(NO_MODEL)),
            ("config set, env blank: config used", "cfg-model", Some(" "), Ok("cfg-model")),
            ("config set, env unset: config used", "cfg-model", None, Ok("cfg-model")),
            ("env wins over config", "cfg-model", Some("env-model"), Ok("env-model")),
        ];
        for (name, model, env_model, expected) in rows {
            let c = TicketConfig { llm_model: model.into(), ..cfg(&d, None, None) };
            let mut env = base.to_vec();
            if let Some(m) = env_model {
                env.push(("LLM_MODEL", m));
            }
            let got = llm::credentials(&c, env_of(&env)).map(|c| c.model).map_err(|e| e.to_string());
            assert_eq!(got, expected.map(str::to_owned).map_err(str::to_owned), "{name}");
        }
    }

    #[test]
    fn debug_never_shows_the_key() {
        let c = LlmCredentials { url: "https://llm.example.test".into(), api_key: KEY.into(), model: "m-1".into() };
        assert_eq!(format!("{c:?}"), r#"LlmCredentials { url: "https://llm.example.test", api_key: "<redacted>", model: "m-1" }"#);
        assert!(!format!("{c:#?}").contains(KEY));
    }
}

mod json_answer {
    use super::*;

    const TWO: &str = r#"{"tickets": [{"title": "Fix login", "items": [1]}, {"title": "Add export", "items": [2, 3]}]}"#;

    fn two() -> serde_json::Value {
        json!({"tickets": [{"title": "Fix login", "items": [1]}, {"title": "Add export", "items": [2, 3]}]})
    }

    #[test]
    fn accepted_answer_shapes() {
        let think = format!("<think>Maybe {{\"tickets\": []}} or a [list]? {{ unclosed\n</think>\n{TWO}");
        let rows: Vec<(&str, String, serde_json::Value)> = vec![
            ("plain object", TWO.to_string(), two()),
            ("plain object with surrounding whitespace", format!("\n  {TWO}\n\n"), two()),
            ("bare array", r#"[{"title": "A"}, {"title": "B"}]"#.into(), json!([{"title": "A"}, {"title": "B"}])),
            ("bare scalar", "42".into(), json!(42)),
            ("json code fence", format!("```json\n{TWO}\n```"), two()),
            ("plain code fence", format!("```\n{TWO}\n```"), two()),
            ("prose around", format!("Here are the tickets:\n{TWO}\nHope this helps."), two()),
            ("prose around bare array", "Tickets: [{\"title\": \"A\"}, {\"title\": \"B\"}] done.".into(), json!([{"title": "A"}, {"title": "B"}])),
            ("think block with braces", think, two()),
            ("think block then fenced json", format!("<think>{{x}}</think>```json\n{TWO}\n```"), two()),
            ("only the last think block is dropped", "<think>a</think>{\"x\": 1}</think>{\"y\": 2}".into(), json!({"y": 2})),
        ];
        for (name, answer, expected) in rows {
            assert_eq!(llm::json_answer(&answer).unwrap(), expected, "{name}");
        }
    }

    /// The span starts at the first opener of either kind: a bare array whose first element is an
    /// object is the array, not that object. `draft_tickets` reads a lone object as "no tasks" /
    /// "no tickets", so a fenced one-ticket array would fail the whole proposal.
    #[test]
    fn fenced_or_prose_wrapped_bare_array_of_one_object_is_the_array() {
        // (name, answer, expected)
        let rows = [
            ("fenced", "```json\n[{\"title\": \"Only\"}]\n```", json!([{"title": "Only"}])),
            ("prose around", "Tickets: [{\"items\": [2]}] done.", json!([{"items": [2]}])),
            ("after thinking", "<think>x</think>\n[{\"cases\": [\"c\"]}]\n.", json!([{"cases": ["c"]}])),
        ];
        for (name, answer, expected) in rows {
            assert_eq!(llm::json_answer(answer).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn object_before_a_later_array_in_prose_is_the_object() {
        // (name, answer, expected)
        let rows = [
            ("object holding an array, array after", "Result: {\"tasks\": [{\"items\": [1]}]} and the list [2, 3] again.", json!({"tasks": [{"items": [1]}]})),
            ("fenced object then a bracketed remark", "```json\n{\"tickets\": []}\n```\n[note: none]", json!({"tickets": []})),
        ];
        for (name, answer, expected) in rows {
            assert_eq!(llm::json_answer(answer).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn first_bracket_that_does_not_parse_falls_back_to_the_other_kind() {
        // (name, answer, expected)
        let rows = [
            ("braces in prose before an array of objects", "Use {braces} sparingly: [{\"title\": \"A\"}] done", json!([{"title": "A"}])),
            ("bracketed prose before an object", "[1 of 2] {\"x\": 1}", json!({"x": 1})),
            ("brace prose then an array", "Note {not json} then [1, 2].", json!([1, 2])),
        ];
        for (name, answer, expected) in rows {
            assert_eq!(llm::json_answer(answer).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn answer_without_json_is_an_error_quoting_what_follows_the_thinking() {
        // (answer, quoted part)
        let rows = [
            ("Sorry, I cannot help with that.", "Sorry, I cannot help with that."),
            ("", ""),
            ("<think>{\"tickets\": [{\"title\": \"X\"}]}</think> no json after", " no json after"),
            ("} reversed {", "} reversed {"),
        ];
        for (answer, quoted) in rows {
            let e = llm::json_answer(answer).unwrap_err();
            assert_eq!(e.to_string(), format!("LLM answer holds no JSON: {quoted}"), "{answer:?}");
        }
    }

    #[test]
    fn quoted_answer_is_cut_to_200_chars() {
        let e = llm::json_answer(&"ż".repeat(250)).unwrap_err();
        assert_eq!(e.to_string(), format!("LLM answer holds no JSON: {}", "ż".repeat(200)));
    }
}

mod chat {
    use super::*;

    fn creds(base: &str) -> LlmCredentials {
        LlmCredentials { url: format!("{base}/v1"), api_key: KEY.into(), model: "glm-test-7".into() }
    }

    fn completion(content: &str) -> (u16, String) {
        (200, json!({"id": "c-1", "choices": [{"index": 0, "message": {"role": "assistant", "content": content}}]}).to_string())
    }

    #[test]
    fn posts_chat_completion_and_returns_content() {
        let mock = MockJira::start(|_, _, _| completion("Cześć"));
        let answer = Llm::new(creds(&mock.base)).chat("You are terse.", "Say hi ż").unwrap();
        assert_eq!(answer, "Cześć");
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].method.as_str(), r[0].url.as_str()), ("POST", "/v1/chat/completions"));
        assert_eq!(r[0].authorization.as_deref(), Some(format!("Bearer {KEY}").as_str()));
        assert!(r[0].content_type.as_deref().is_some_and(|c| c.starts_with("application/json")), "{:?}", r[0].content_type);
        let body: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "glm-test-7",
                "messages": [{"role": "system", "content": "You are terse."}, {"role": "user", "content": "Say hi ż"}],
                "temperature": 0.2,
                "stream": false
            })
        );
    }

    #[test]
    fn any_2xx_is_success() {
        let mock = MockJira::start(|_, _, _| (201, json!({"choices": [{"message": {"content": "ok-201"}}]}).to_string()));
        assert_eq!(Llm::new(creds(&mock.base)).chat("s", "u").unwrap(), "ok-201");
    }

    #[test]
    fn refused_key_names_the_api_key_and_hides_it() {
        for status in [401u16, 403] {
            let mock = MockJira::start(move |_, _, _| (status, format!("{{\"error\":\"invalid key {KEY}\"}}")));
            let e = Llm::new(creds(&mock.base)).chat("s", "u").unwrap_err();
            let full = format!("{e:#}");
            assert_eq!(full, format!("LLM refused the API key ({status}); check opencode's provider key or LLM_API_KEY"));
            assert!(!full.contains(KEY) && !format!("{e:?}").contains(KEY), "{full}");
        }
    }

    #[test]
    fn other_error_status_quotes_body_up_to_300_chars() {
        let long: String = "ż".repeat(350);
        let body = long.clone();
        let mock = MockJira::start(move |_, _, _| (500, body.clone()));
        let e = Llm::new(creds(&mock.base)).chat("s", "u").unwrap_err();
        assert_eq!(e.to_string(), format!("LLM 500: {}", "ż".repeat(300)));

        let mock = MockJira::start(|_, _, _| (404, "model not found".into()));
        let e = Llm::new(creds(&mock.base)).chat("s", "u").unwrap_err();
        assert_eq!(e.to_string(), "LLM 404: model not found");
    }

    #[test]
    fn success_body_that_is_not_json_is_an_error() {
        let mock = MockJira::start(|_, _, _| (200, "<html>proxy login</html>".into()));
        let e = Llm::new(creds(&mock.base)).chat("s", "u").unwrap_err();
        assert_eq!(e.to_string(), "LLM response is not JSON");
    }

    #[test]
    fn missing_or_blank_content_is_an_error() {
        let rows = [
            ("no choices key", json!({"id": "x"})),
            ("empty choices", json!({"choices": []})),
            ("no message", json!({"choices": [{"index": 0}]})),
            ("null content", json!({"choices": [{"message": {"content": null}}]})),
            ("blank content", json!({"choices": [{"message": {"content": " \n "}}]})),
            ("non-string content", json!({"choices": [{"message": {"content": 5}}]})),
        ];
        for (name, resp) in rows {
            let body = resp.to_string();
            let mock = MockJira::start(move |_, _, _| (200, body.clone()));
            let e = Llm::new(creds(&mock.base)).chat("s", "u").unwrap_err();
            assert_eq!(e.to_string(), "LLM response has no message content", "{name}");
        }
    }

    #[test]
    fn only_first_choice_is_read() {
        let mock = MockJira::start(|_, _, _| {
            (200, json!({"choices": [{"message": {"content": "first"}}, {"message": {"content": "second"}}]}).to_string())
        });
        assert_eq!(Llm::new(creds(&mock.base)).chat("s", "u").unwrap(), "first");
    }

    #[test]
    fn llm_is_the_drafting_chat_sending_system_and_user_as_given() {
        let mock = MockJira::start(|_, _, _| completion("{\"tasks\": []}"));
        let llm = Llm::new(creds(&mock.base));
        let chat: &dyn worklog::drafting::Chat = &llm;
        assert_eq!(chat.chat("system Ż", "user 7").unwrap(), "{\"tasks\": []}");
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        let body: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
        assert_eq!(body["messages"], json!([{"role": "system", "content": "system Ż"}, {"role": "user", "content": "user 7"}]));
    }
}

