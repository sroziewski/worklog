//! A draft's ticket kind: issue types, epic, parent and required fields read from Jira's create
//! metadata (against a local tiny_http fake, never the real Jira), and how the New ticket window
//! uses them to block, count and send drafts.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{MockJira, query_param};
use serde_json::{Map, Value, json};
use worklog::config::TicketConfig;
use worklog::jira::{Issue, Jira, NewIssue};
use worklog::llm::TicketDraft;
use worklog::ticket_kind::{self, FieldKind, FieldValue, IssueType, Kind, ProjectMeta, RequiredField};
use worklog::ui_tickets::{Creator, Draft, MetaState};

const TOKEN: &str = "tok-kind-9Zp";
const EPIC_LINK: &str = "com.pyxis.greenhopper.jira:gh-epic-link";

fn it(id: &str, name: &str, subtask: bool) -> IssueType {
    IssueType { id: id.into(), name: name.into(), subtask }
}

fn issue(key: &str, summary: &str) -> Issue {
    Issue { key: key.into(), summary: summary.into(), status: "Open".into() }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().expect("a JSON object").clone()
}

fn set(v: &[&str]) -> BTreeSet<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn rf(id: &str, name: &str, kind: FieldKind) -> RequiredField {
    RequiredField { id: id.into(), name: name.into(), kind }
}

fn environment() -> RequiredField {
    rf("customfield_11659", "Environment", FieldKind::MultiSelect(strings(&["PROD", "TEST"])))
}

fn element() -> RequiredField {
    rf("customfield_13520", "Element", FieldKind::Select(strings(&["Backend", "Frontend"])))
}

fn steps() -> RequiredField {
    rf("customfield_12000", "Steps", FieldKind::Text)
}

fn epic_required() -> RequiredField {
    rf("customfield_10001", "Epic Link", FieldKind::Epic)
}

fn kind(issue_type: &str, subtask: bool) -> Kind {
    Kind { issue_type: issue_type.into(), subtask, ..Kind::default() }
}

fn with_values(mut k: Kind, values: Vec<(&str, FieldValue)>) -> Kind {
    k.values = values.into_iter().map(|(id, v)| (id.to_owned(), v)).collect();
    k
}

mod parse_issue_types {
    use super::*;

    #[test]
    fn id_text_or_number_name_and_subtask_flag_entries_without_id_or_name_skipped() {
        let v = json!({"values": [
            {"id": "3", "name": "Task", "subtask": false},
            {"id": 5, "name": "Sub-task", "subtask": true},
            {"name": "No id", "subtask": false},
            {"id": "1", "name": "Bug"},
            {"id": "9", "subtask": true},
            {"id": "6", "name": "Epic", "subtask": false},
        ]});
        assert_eq!(
            ticket_kind::parse_issue_types(&v),
            vec![it("3", "Task", false), it("5", "Sub-task", true), it("1", "Bug", false), it("6", "Epic", false)]
        );
    }

    #[test]
    fn no_values_gives_no_types() {
        for v in [json!({}), json!({"values": []}), json!({"values": "x"}), Value::Null] {
            assert_eq!(ticket_kind::parse_issue_types(&v), vec![], "{v}");
        }
    }
}

mod parse_required_fields {
    use super::*;

    fn field(id: &str, name: &str, schema: Value) -> Value {
        json!({"fieldId": id, "name": name, "required": true, "hasDefaultValue": false, "schema": schema})
    }

    #[test]
    fn each_schema_kind_with_allowed_values_from_value_or_name() {
        let v = json!({"values": [
            {"fieldId": "summary", "name": "Summary", "required": true, "hasDefaultValue": false, "schema": {"type": "string"}},
            {"fieldId": "customfield_11659", "name": "Environment", "required": true, "hasDefaultValue": false,
             "schema": {"type": "array", "items": "option", "custom": "com.atlassian.jira.plugin.system.customfieldtypes:multicheckboxes"},
             "allowedValues": [{"value": "PROD", "id": "21"}, {"value": "TEST", "id": "22"}]},
            {"fieldId": "customfield_13520", "name": "Element", "required": true,
             "schema": {"type": "option"},
             "allowedValues": [{"value": "Backend", "id": "31"}, {"id": "32"}, {"name": "Frontend", "id": "33"}]},
            {"fieldId": "customfield_12000", "name": "Steps", "required": true, "schema": {"type": "string"}},
            {"fieldId": "customfield_12001", "required": true, "schema": {"type": "user"}},
            {"fieldId": "customfield_12002", "name": "Tags", "required": true, "schema": {"type": "array", "items": "string"}},
            {"fieldId": "customfield_12003", "name": "Mystery", "required": true},
        ]});
        assert_eq!(
            ticket_kind::parse_required_fields(&v, &Map::new()),
            vec![
                environment(),
                rf("customfield_13520", "Element", FieldKind::Select(strings(&["Backend", "Frontend"]))),
                steps(),
                rf("customfield_12001", "customfield_12001", FieldKind::Unsupported("user".into())),
                rf("customfield_12002", "Tags", FieldKind::Unsupported("array of string".into())),
                rf("customfield_12003", "Mystery", FieldKind::Unsupported("unknown".into())),
            ]
        );
    }

    #[test]
    fn epic_link_by_schema_custom_is_an_epic_field_whatever_its_type_or_name() {
        let v = json!({"values": [
            field("customfield_10001", "Epic Link", json!({"type": "any", "custom": EPIC_LINK, "customId": 10001})),
            field("customfield_10777", "Epos", json!({"type": "option", "custom": EPIC_LINK})),
            field("customfield_10778", "Epic Name", json!({"type": "string", "custom": "com.pyxis.greenhopper.jira:gh-epic-label"})),
        ]});
        assert_eq!(ticket_kind::parse_required_fields(&v, &Map::new()), vec![
            epic_required(),
            rf("customfield_10777", "Epos", FieldKind::Epic),
            rf("customfield_10778", "Epic Name", FieldKind::Text),
        ]);
    }

    #[test]
    fn configured_epic_link_field_is_left_out() {
        let v = json!({"values": [field("customfield_10001", "Epic Link", json!({"type": "any", "custom": EPIC_LINK}))]});
        assert_eq!(ticket_kind::parse_required_fields(&v, &obj(json!({"customfield_10001": "HD-1"}))), vec![]);
    }

    #[test]
    fn select_without_allowed_values_offers_none() {
        let v = json!({"values": [field("customfield_14000", "Team", json!({"type": "option"}))]});
        assert_eq!(ticket_kind::parse_required_fields(&v, &Map::new()), vec![rf("customfield_14000", "Team", FieldKind::Select(vec![]))]);
    }

    #[test]
    fn optional_defaulted_or_unflagged_fields_are_left_out() {
        let v = json!({"values": [
            {"fieldId": "customfield_15001", "name": "Optional", "required": false, "schema": {"type": "string"}},
            {"fieldId": "customfield_15002", "name": "Defaulted", "required": true, "hasDefaultValue": true, "schema": {"type": "string"}},
            {"fieldId": "customfield_15003", "name": "Unflagged", "schema": {"type": "string"}},
            {"name": "No field id", "required": true, "schema": {"type": "string"}},
            {"fieldId": "customfield_15004", "name": "Kept", "required": true, "schema": {"type": "string"}},
        ]});
        assert_eq!(ticket_kind::parse_required_fields(&v, &Map::new()), vec![rf("customfield_15004", "Kept", FieldKind::Text)]);
    }

    #[test]
    fn fields_the_window_sets_or_the_config_provides_are_left_out() {
        for id in ["project", "issuetype", "summary", "description", "assignee", "reporter", "parent"] {
            let v = json!({"values": [field(id, "Handled", json!({"type": "string"})), field("customfield_16000", "Kept", json!({"type": "string"}))]});
            assert_eq!(ticket_kind::parse_required_fields(&v, &Map::new()), vec![rf("customfield_16000", "Kept", FieldKind::Text)], "{id}");
        }
        let v = json!({"values": [
            field("customfield_10000", "Configured", json!({"type": "option"})),
            field("customfield_16000", "Kept", json!({"type": "string"})),
        ]});
        let provided = obj(json!({"customfield_10000": {"value": "maintenance"}, "labels": ["wl"]}));
        assert_eq!(ticket_kind::parse_required_fields(&v, &provided), vec![rf("customfield_16000", "Kept", FieldKind::Text)]);
    }

    #[test]
    fn no_values_gives_no_fields() {
        for v in [json!({}), json!({"values": []}), Value::Null] {
            assert_eq!(ticket_kind::parse_required_fields(&v, &Map::new()), vec![], "{v}");
        }
    }
}

mod epic_link_field {
    use super::*;

    #[test]
    fn found_by_schema_type_not_by_name() {
        let v = json!([
            {"id": "summary", "name": "Summary", "schema": {"type": "string", "system": "summary"}},
            {"id": "customfield_10002", "name": "Epic Link", "schema": {"type": "string", "custom": "com.atlassian.jira.plugin.system.customfieldtypes:textfield"}},
            {"id": "customfield_10001", "name": "Epos", "schema": {"type": "any", "custom": EPIC_LINK}},
        ]);
        assert_eq!(ticket_kind::epic_link_field(&v), Some("customfield_10001".into()));
    }

    #[test]
    fn none_without_such_field_or_list() {
        let rows = [
            ("only other fields", json!([{"id": "customfield_10002", "name": "Epic Link", "schema": {"type": "string"}}])),
            ("empty list", json!([])),
            ("not a list", json!({"id": "customfield_10001", "schema": {"custom": EPIC_LINK}})),
        ];
        for (name, v) in rows {
            assert_eq!(ticket_kind::epic_link_field(&v), None, "{name}");
        }
    }
}

mod offered_types {
    use super::*;

    fn all() -> Vec<IssueType> {
        vec![it("3", "Task", false), it("5", "Sub-task", true), it("1", "Bug", false), it("6", "Epic", false), it("7", "Story", false), it("8", "Improvement", false)]
    }

    #[test]
    fn wanted_types_the_project_has_default_first_each_once_never_epic() {
        // (name, wanted, default, expected type ids)
        let rows: [(&str, &[&str], &str, &[&str]); 8] = [
            ("contract default list", &["Task", "Story", "Bug", "Sub-task"], "Task", &["3", "7", "1", "5"]),
            ("default moves first", &["Task", "Story", "Bug", "Sub-task"], "Bug", &["1", "3", "7", "5"]),
            ("case-insensitive names", &["story", "SUB-TASK"], "task", &["3", "7", "5"]),
            ("duplicate names and default repeated", &["Story", "STORY", "Bug", "story"], "Story", &["7", "1"]),
            ("epic named is skipped", &["Epic", "Task"], "Epic", &["3"]),
            ("names with spaces are trimmed", &[" Bug "], "Task", &["3", "1"]),
            ("names the project lacks are skipped", &["Spike", "Improvement"], "Chore", &["8"]),
            ("nothing wanted offers the default alone", &[], "Story", &["7"]),
        ];
        for (name, wanted, default, expected) in rows {
            let ids: Vec<String> = ticket_kind::offered_types(&all(), &strings(wanted), default).into_iter().map(|t| t.id).collect();
            assert_eq!(ids, strings(expected), "{name}");
        }
    }

    #[test]
    fn epic_named_with_spaces_is_never_offered() {
        for (wanted, default) in [(&["  epic ", "Task"][..], "Task"), (&["Task"][..], " Epic")] {
            let ids: Vec<String> = ticket_kind::offered_types(&all(), &strings(wanted), default).into_iter().map(|t| t.id).collect();
            assert_eq!(ids, strings(&["3"]), "{wanted:?} / {default:?}");
        }
    }

    #[test]
    fn offered_types_keep_jiras_name_and_subtask_flag() {
        assert_eq!(ticket_kind::offered_types(&all(), &strings(&["sub-task", "bug"]), "TASK"), vec![
            it("3", "Task", false),
            it("5", "Sub-task", true),
            it("1", "Bug", false)
        ]);
    }

    #[test]
    fn project_without_types_offers_nothing() {
        assert_eq!(ticket_kind::offered_types(&[], &strings(&["Task", "Bug"]), "Task"), vec![]);
    }
}

mod fallback_types {
    use super::*;

    #[test]
    fn configured_names_default_first_sub_names_are_subtasks() {
        assert_eq!(ticket_kind::fallback_types(&strings(&["Task", "Story", "Bug", "Sub-task"]), "Task"), vec![
            it("", "Task", false),
            it("", "Story", false),
            it("", "Bug", false),
            it("", "Sub-task", true)
        ]);
    }

    #[test]
    fn blanks_duplicates_and_epic_skipped_names_trimmed() {
        let wanted = strings(&["  ", "", "Epic", " EPIC ", "bug", " Story ", "Subtask", "SUB-TASK", "story"]);
        assert_eq!(ticket_kind::fallback_types(&wanted, "Bug"), vec![
            it("", "Bug", false),
            it("", "Story", false),
            it("", "Subtask", true),
            it("", "SUB-TASK", true)
        ]);
    }

    #[test]
    fn blank_default_and_nothing_wanted_gives_nothing() {
        assert_eq!(ticket_kind::fallback_types(&[], " "), vec![]);
    }
}

mod key_in_project {
    use super::*;

    #[test]
    fn project_part_case_insensitive_digits_after_the_last_dash() {
        // (key, project, expected)
        let rows = [
            ("HD-12", "HD", true),
            ("hd-12", "HD", true),
            ("HD-12", "hd", true),
            (" HD-7 ", "HD", true),
            ("HD-0", "HD", true),
            ("AB-CD-3", "AB-CD", true),
            ("HDX-12", "HD", false),
            ("XHD-12", "HD", false),
            ("ABC-12", "HD", false),
            ("HD-", "HD", false),
            ("HD-1a", "HD", false),
            ("HD-12 3", "HD", false),
            ("HD12", "HD", false),
            ("-12", "HD", false),
            ("HD-12-", "HD", false),
            ("HD-١٢", "HD", false),
            ("", "HD", false),
        ];
        for (key, project, expected) in rows {
            assert_eq!(ticket_kind::key_in_project(key, project), expected, "{key:?} in {project:?}");
        }
    }
}

mod blockers {
    use super::*;

    fn required() -> Vec<RequiredField> {
        vec![environment(), element(), steps()]
    }

    fn filled(mut k: Kind) -> Kind {
        k.values = BTreeMap::from([
            ("customfield_11659".to_owned(), FieldValue::Many(set(&["PROD"]))),
            ("customfield_13520".to_owned(), FieldValue::One("Frontend".into())),
            ("customfield_12000".to_owned(), FieldValue::Text("repro".into())),
        ]);
        k
    }

    fn subtask(parent: &str) -> Kind {
        Kind { parent: parent.into(), ..filled(kind("Sub-task", true)) }
    }

    fn set_value(mut k: Kind, id: &str, v: Option<FieldValue>) -> Kind {
        match v {
            Some(v) => k.values.insert(id.to_owned(), v),
            None => k.values.remove(id),
        };
        k
    }

    #[test]
    fn each_reason_alone() {
        let task = || filled(kind("Bug", false));
        // (name, kind, expected)
        let rows: Vec<(&str, Kind, Vec<&str>)> = vec![
            ("everything filled", task(), vec![]),
            ("a parent typed on a non-sub-task is ignored", Kind { parent: "x".into(), ..task() }, vec![]),
            ("sub-task with a parent of the project", subtask("HD-4"), vec![]),
            ("sub-task parent in lower case, padded", subtask(" hd-4 "), vec![]),
            ("sub-task without parent", subtask(""), vec!["pick a parent"]),
            ("sub-task with blank parent", subtask("   "), vec!["pick a parent"]),
            ("sub-task parent of another project", subtask("ABC-3"), vec!["the parent must be an issue of HD"]),
            ("sub-task parent not a key", subtask("HD"), vec!["the parent must be an issue of HD"]),
            ("multi-select missing", set_value(task(), "customfield_11659", None), vec!["Environment"]),
            ("multi-select empty", set_value(task(), "customfield_11659", Some(FieldValue::Many(set(&[])))), vec!["Environment"]),
            ("multi-select with a value not allowed", set_value(task(), "customfield_11659", Some(FieldValue::Many(set(&["PROD", "STAGE"])))), vec![
                "Environment",
            ]),
            ("multi-select given one value", set_value(task(), "customfield_11659", Some(FieldValue::One("PROD".into()))), vec!["Environment"]),
            ("select missing", set_value(task(), "customfield_13520", None), vec!["Element"]),
            ("select with a value not allowed", set_value(task(), "customfield_13520", Some(FieldValue::One("Mobile".into()))), vec!["Element"]),
            ("select given text", set_value(task(), "customfield_13520", Some(FieldValue::Text("Frontend".into()))), vec!["Element"]),
            ("text missing", set_value(task(), "customfield_12000", None), vec!["Steps"]),
            ("text blank", set_value(task(), "customfield_12000", Some(FieldValue::Text(" \n\t".into()))), vec!["Steps"]),
            ("text given a select value", set_value(task(), "customfield_12000", Some(FieldValue::One("repro".into()))), vec!["Steps"]),
        ];
        for (name, k, expected) in rows {
            assert_eq!(ticket_kind::blockers(&k, "HD", &required()), strings(&expected), "{name}");
        }
    }

    #[test]
    fn all_reasons_together_parent_first_then_fields_in_order() {
        let k = kind("Sub-task", true);
        assert_eq!(ticket_kind::blockers(&k, "HD", &required()), strings(&["pick a parent", "Environment", "Element", "Steps"]));
        let k = Kind { parent: "ABC-3".into(), ..set_value(filled(kind("Sub-task", true)), "customfield_13520", None) };
        assert_eq!(ticket_kind::blockers(&k, "HD", &required()), strings(&["the parent must be an issue of HD", "Element"]));
    }

    #[test]
    fn unsupported_field_always_blocks_naming_it_and_the_setting() {
        let reviewer = rf("customfield_12001", "Reviewer", FieldKind::Unsupported("user".into()));
        let k = set_value(filled(kind("Bug", false)), "customfield_12001", Some(FieldValue::Text("jdoe".into())));
        assert_eq!(
            ticket_kind::blockers(&k, "HD", &[element(), reviewer.clone(), steps()]),
            strings(&["Reviewer (user) cannot be set here; add it to WORKLOG_TICKET_FIELDS"])
        );
        assert_eq!(
            ticket_kind::blockers(&kind("Bug", false), "HD", &[element(), reviewer, steps()]),
            strings(&["Element", "Reviewer (user) cannot be set here; add it to WORKLOG_TICKET_FIELDS", "Steps"])
        );
    }

    #[test]
    fn required_epic_blocks_anything_but_a_sub_task_until_an_epic_is_picked() {
        let req = [element(), epic_required(), steps()];
        let with_epic = |k: Kind| Kind { epic: Some("HD-7".into()), ..k };
        // (name, kind, expected)
        let rows: Vec<(&str, Kind, Vec<&str>)> = vec![
            ("no epic, nothing filled: in field order", kind("Story", false), vec!["Element", "pick an epic", "Steps"]),
            ("no epic, rest filled", filled(kind("Story", false)), vec!["pick an epic"]),
            (
                "a value under the epic field id is not an epic",
                set_value(filled(kind("Story", false)), "customfield_10001", Some(FieldValue::One("HD-7".into()))),
                vec!["pick an epic"],
            ),
            ("epic picked", with_epic(filled(kind("Story", false))), vec![]),
            ("sub-task without epic", subtask("HD-4"), vec![]),
            ("sub-task without epic or parent", subtask(""), vec!["pick a parent"]),
        ];
        for (name, k, expected) in rows {
            assert_eq!(ticket_kind::blockers(&k, "HD", &req), strings(&expected), "{name}");
        }
    }

    #[test]
    fn nothing_required_nothing_blocks() {
        assert_eq!(ticket_kind::blockers(&Kind::default(), "HD", &[]), Vec::<String>::new());
    }
}

mod kind_fields {
    use super::*;

    const EPIC_FIELD: &str = "customfield_10001";

    #[test]
    fn sub_task_sends_its_trimmed_parent_and_never_the_epic() {
        let k = Kind { parent: " HD-12 ".into(), epic: Some("HD-7".into()), ..kind("Sub-task", true) };
        assert_eq!(ticket_kind::kind_fields(&k, Some(EPIC_FIELD), &[]), obj(json!({"parent": {"key": "HD-12"}})));
    }

    #[test]
    fn other_types_send_the_epic_in_the_epic_link_field_and_no_parent() {
        let k = Kind { parent: "HD-12".into(), epic: Some("HD-7".into()), ..kind("Story", false) };
        assert_eq!(ticket_kind::kind_fields(&k, Some(EPIC_FIELD), &[]), obj(json!({"customfield_10001": "HD-7"})));
    }

    #[test]
    fn no_epic_or_no_epic_link_field_sends_nothing() {
        let with_epic = Kind { epic: Some("HD-7".into()), ..kind("Story", false) };
        assert_eq!(ticket_kind::kind_fields(&with_epic, None, &[]), Map::new());
        assert_eq!(ticket_kind::kind_fields(&kind("Story", false), Some(EPIC_FIELD), &[]), Map::new());
    }

    #[test]
    fn required_values_in_their_jira_shape_others_left_out() {
        let required = [environment(), element(), steps(), rf("customfield_12001", "Reviewer", FieldKind::Unsupported("user".into())), rf(
            "customfield_12005",
            "Unfilled",
            FieldKind::Text,
        )];
        let k = with_values(kind("Bug", false), vec![
            ("customfield_11659", FieldValue::Many(set(&["TEST", "PROD"]))),
            ("customfield_13520", FieldValue::One("Backend".into())),
            ("customfield_12000", FieldValue::Text("  repro steps \n".into())),
            ("customfield_99999", FieldValue::One("left from another type".into())),
        ]);
        // A set has no order Jira cares about; BTreeSet sends it sorted.
        assert_eq!(
            ticket_kind::kind_fields(&k, None, &required),
            obj(json!({
                "customfield_11659": [{"value": "PROD"}, {"value": "TEST"}],
                "customfield_13520": {"value": "Backend"},
                "customfield_12000": "repro steps",
            }))
        );
    }

    #[test]
    fn required_epic_field_takes_the_epic_key_never_a_value_shape() {
        let req = [rf("customfield_10101", "Epic Link", FieldKind::Epic), element()];
        let stale = vec![("customfield_10101", FieldValue::One("stale".into())), ("customfield_13520", FieldValue::One("Backend".into()))];
        // (name, kind, optional epic field of the project, expected)
        let rows: Vec<(&str, Kind, Option<&str>, Value)> = vec![
            (
                "epic set, no optional field",
                Kind { epic: Some("HD-7".into()), ..with_values(kind("Story", false), stale.clone()) },
                None,
                json!({"customfield_10101": "HD-7", "customfield_13520": {"value": "Backend"}}),
            ),
            (
                "epic set, optional field of another id",
                Kind { epic: Some("HD-7".into()), ..with_values(kind("Story", false), stale.clone()) },
                Some(EPIC_FIELD),
                json!({"customfield_10001": "HD-7", "customfield_10101": "HD-7", "customfield_13520": {"value": "Backend"}}),
            ),
            ("no epic", with_values(kind("Story", false), stale.clone()), Some(EPIC_FIELD), json!({"customfield_13520": {"value": "Backend"}})),
            (
                "sub-task with an epic",
                Kind { epic: Some("HD-7".into()), parent: "HD-12".into(), ..with_values(kind("Sub-task", true), stale.clone()) },
                Some(EPIC_FIELD),
                json!({"parent": {"key": "HD-12"}, "customfield_13520": {"value": "Backend"}}),
            ),
        ];
        for (name, k, epic_field, expected) in rows {
            assert_eq!(ticket_kind::kind_fields(&k, epic_field, &req), obj(expected), "{name}");
        }
    }

    #[test]
    fn required_and_optional_epic_on_the_same_field_send_it_once() {
        let k = Kind { epic: Some("HD-9".into()), ..kind("Story", false) };
        assert_eq!(ticket_kind::kind_fields(&k, Some(EPIC_FIELD), &[epic_required()]), obj(json!({"customfield_10001": "HD-9"})));
    }

    #[test]
    fn epic_parent_and_values_together() {
        let k = Kind { epic: Some("HD-7".into()), ..with_values(kind("Bug", false), vec![("customfield_13520", FieldValue::One("Frontend".into()))]) };
        assert_eq!(
            ticket_kind::kind_fields(&k, Some(EPIC_FIELD), &[element()]),
            obj(json!({"customfield_10001": "HD-7", "customfield_13520": {"value": "Frontend"}}))
        );
        let k = Kind { parent: "HD-12".into(), ..with_values(kind("Sub-task", true), vec![("customfield_12000", FieldValue::Text("why".into()))]) };
        assert_eq!(
            ticket_kind::kind_fields(&k, Some(EPIC_FIELD), &[steps()]),
            obj(json!({"parent": {"key": "HD-12"}, "customfield_12000": "why"}))
        );
    }
}

mod type_name {
    use super::*;

    #[test]
    fn chosen_type_or_the_default_when_none_or_blank() {
        for (chosen, expected) in [("Bug", "Bug"), ("", "Task"), ("  ", "Task")] {
            assert_eq!(kind(chosen, false).type_name("Task"), expected, "{chosen:?}");
        }
    }
}

mod filter_issues {
    use super::*;

    fn issues() -> Vec<Issue> {
        vec![issue("HD-1", "Login page"), issue("HD-12", "Export CSV"), issue("HD-120", "export pdf")]
    }

    #[test]
    fn key_or_summary_case_insensitive_blank_gives_all() {
        // (query, expected keys)
        let rows: [(&str, &[&str]); 7] = [
            ("", &["HD-1", "HD-12", "HD-120"]),
            ("  ", &["HD-1", "HD-12", "HD-120"]),
            ("export", &["HD-12", "HD-120"]),
            (" EXPORT ", &["HD-12", "HD-120"]),
            ("hd-12", &["HD-12", "HD-120"]),
            ("PAGE", &["HD-1"]),
            ("zzz", &[]),
        ];
        let all = issues();
        for (query, expected) in rows {
            let keys: Vec<&str> = ticket_kind::filter_issues(&all, query).into_iter().map(|i| i.key.as_str()).collect();
            assert_eq!(keys, expected.to_vec(), "{query:?}");
        }
    }
}

mod parent_choices {
    use super::*;

    #[test]
    fn created_first_then_jiras_each_key_once() {
        let created = [issue("HD-30", "Created A"), issue("HD-29", "Created B")];
        let jira = [issue("HD-5", "Old"), issue("HD-30", "Same key from Jira"), issue("HD-6", "Older"), issue("HD-5", "Twice")];
        assert_eq!(ticket_kind::parent_choices(&created, &jira), vec![
            issue("HD-30", "Created A"),
            issue("HD-29", "Created B"),
            issue("HD-5", "Old"),
            issue("HD-6", "Older")
        ]);
    }

    #[test]
    fn either_side_empty() {
        assert_eq!(ticket_kind::parent_choices(&[], &[issue("HD-5", "Old")]), vec![issue("HD-5", "Old")]);
        assert_eq!(ticket_kind::parent_choices(&[issue("HD-30", "New")], &[]), vec![issue("HD-30", "New")]);
        assert_eq!(ticket_kind::parent_choices(&[], &[]), vec![]);
    }
}

mod type_counts {
    use super::*;

    #[test]
    fn counts_in_order_of_first_appearance() {
        let rows: [(&[&str], &str); 3] =
            [(&[], ""), (&["Story"], "1 Story"), (&["Task", "Bug", "Task", "Sub-task", "Bug", "Task"], "3 Task, 2 Bug, 1 Sub-task")];
        for (types, expected) in rows {
            assert_eq!(ticket_kind::type_counts(types.iter().copied()), expected, "{types:?}");
        }
    }
}

mod load {
    use super::*;

    const EPICS_JQL: &str = "project = \"HD\" AND issuetype = Epic AND resolution = Unresolved ORDER BY updated DESC";
    const PARENTS_JQL: &str =
        "project = \"HD\" AND resolution = Unresolved AND issuetype not in subTaskIssueTypes() AND issuetype != Epic ORDER BY updated DESC";

    fn types() -> Value {
        json!({"values": [
            {"id": "3", "name": "Task", "subtask": false},
            {"id": "5", "name": "Sub-task", "subtask": true},
            {"id": "1", "name": "Bug", "subtask": false},
            {"id": "6", "name": "Epic", "subtask": false},
            {"id": "4", "name": "Improvement", "subtask": false},
        ]})
    }

    fn create_fields(type_id: &str) -> Value {
        let summary = json!({"fieldId": "summary", "name": "Summary", "required": true, "hasDefaultValue": false, "schema": {"type": "string"}});
        match type_id {
            "3" => json!({"values": [summary, {"fieldId": "customfield_13520", "name": "Element", "required": true, "hasDefaultValue": false,
                "schema": {"type": "option"}, "allowedValues": [{"value": "Backend", "id": "31"}, {"value": "Frontend", "id": "33"}]}]}),
            "1" => json!({"values": [summary,
                {"fieldId": "customfield_11659", "name": "Environment", "required": true, "hasDefaultValue": false,
                 "schema": {"type": "array", "items": "option", "custom": "com.atlassian.jira.plugin.system.customfieldtypes:multicheckboxes"},
                 "allowedValues": [{"value": "PROD", "id": "21"}, {"value": "TEST", "id": "22"}]},
                {"fieldId": "customfield_10000", "name": "Configured", "required": true, "schema": {"type": "option"}, "allowedValues": [{"value": "maintenance"}]}]}),
            "5" => json!({"values": [summary, {"fieldId": "parent", "name": "Parent", "required": true, "schema": {"type": "issuelink"}}]}),
            "4" => json!({"values": [summary, {"fieldId": "customfield_10001", "name": "Epic Link", "required": true, "hasDefaultValue": false,
                "schema": {"type": "any", "custom": EPIC_LINK, "customId": 10001}}]}),
            _ => json!({"values": [summary, {"fieldId": "customfield_19999", "name": "Not offered", "required": true, "schema": {"type": "string"}}]}),
        }
    }

    fn field_list(with_epic_link: bool) -> Value {
        let mut v = vec![
            json!({"id": "summary", "name": "Summary", "schema": {"type": "string", "system": "summary"}}),
            json!({"id": "customfield_11659", "name": "Environment", "schema": {"type": "array", "items": "option"}}),
        ];
        if with_epic_link {
            v.push(json!({"id": "customfield_10001", "name": "Epic Link", "schema": {"type": "any", "custom": EPIC_LINK}}));
        }
        Value::Array(v)
    }

    fn search(issues: &[(&str, &str)]) -> Value {
        json!({"issues": issues.iter().map(|(k, s)| json!({"key": k, "fields": {"summary": s, "status": {"name": "Open"}}})).collect::<Vec<_>>()})
    }

    /// The part of the fake Jira a request asks for: "types", "fields-<id>", "field", "epics",
    /// "parents", or "unknown".
    pub(crate) fn part(url: &str) -> String {
        let path = url.split('?').next().unwrap_or_default();
        if path == "/rest/api/2/issue/createmeta/HD/issuetypes" {
            return "types".into();
        }
        if let Some(id) = path.strip_prefix("/rest/api/2/issue/createmeta/HD/issuetypes/") {
            return if query_param(url, "maxResults").as_deref() == Some("500") { format!("fields-{id}") } else { "unknown".into() };
        }
        if path == "/rest/api/2/field" {
            return "field".into();
        }
        if path == "/rest/api/2/search" {
            match query_param(url, "jql").as_deref() {
                Some(EPICS_JQL) => return "epics".into(),
                Some(PARENTS_JQL) => return "parents".into(),
                _ => {}
            }
        }
        "unknown".into()
    }

    /// A Jira answering every part with the fixtures above, except the parts in `fail` (500 with
    /// "boom <part>").
    pub(crate) fn fake_jira(fail: &'static [&'static str], with_epic_link: bool) -> MockJira {
        MockJira::start(move |method, url, _| {
            let p = part(url);
            if method != "GET" || p == "unknown" {
                return (404, json!({"errorMessages": [format!("unexpected {method} {url}")]}).to_string());
            }
            if fail.contains(&p.as_str()) {
                return (500, json!({"errorMessages": [format!("boom {p}")]}).to_string());
            }
            let body = match p.as_str() {
                "types" => types(),
                "field" => field_list(with_epic_link),
                "epics" => search(&[("HD-7", "Payments epic"), ("HD-3", "Reports epic")]),
                "parents" => search(&[("HD-12", "Login"), ("HD-11", "Export")]),
                fields => create_fields(fields.trim_start_matches("fields-")),
            };
            (200, body.to_string())
        })
    }

    pub(crate) fn provided() -> Map<String, Value> {
        obj(json!({"customfield_10000": {"value": "maintenance"}}))
    }

    fn wanted() -> Vec<String> {
        strings(&["Task", "Story", "Bug", "Sub-task"])
    }

    fn run(mock: &MockJira, wanted: &[String]) -> anyhow::Result<ProjectMeta> {
        ticket_kind::load(&Jira::new(&mock.base, TOKEN), "HD", wanted, "Task", &provided())
    }

    fn parts(mock: &MockJira) -> Vec<String> {
        mock.requests().iter().map(|r| part(&r.url)).collect()
    }

    fn url_of(mock: &MockJira, wanted_part: &str) -> String {
        mock.requests().into_iter().find(|r| part(&r.url) == wanted_part).map(|r| r.url).expect("requested")
    }

    pub(crate) fn complete() -> ProjectMeta {
        ProjectMeta {
            types: vec![it("3", "Task", false), it("1", "Bug", false), it("5", "Sub-task", true)],
            required: BTreeMap::from([("Task".to_owned(), vec![element()]), ("Bug".to_owned(), vec![environment()]), ("Sub-task".to_owned(), vec![])]),
            epics: vec![issue("HD-7", "Payments epic"), issue("HD-3", "Reports epic")],
            parents: vec![issue("HD-12", "Login"), issue("HD-11", "Export")],
            epic_field: Some("customfield_10001".into()),
            warnings: vec![],
        }
    }

    #[test]
    fn every_part_read() {
        let mock = fake_jira(&[], true);
        assert_eq!(run(&mock, &wanted()).unwrap(), complete());
        assert_eq!(parts(&mock), strings(&["types", "fields-3", "fields-1", "fields-5", "field", "epics", "parents"]));
        let auth: Vec<Option<String>> = mock.requests().into_iter().map(|r| r.authorization).collect();
        assert_eq!(auth, vec![Some(format!("Bearer {TOKEN}")); 7]);
    }

    #[test]
    fn types_failing_is_the_error_and_nothing_else_is_read() {
        let mock = fake_jira(&["types"], true);
        let e = run(&mock, &wanted()).unwrap_err();
        assert_eq!(format!("{e:#}"), "Jira 500 on /rest/api/2/issue/createmeta/HD/issuetypes: boom types");
        assert_eq!(parts(&mock), strings(&["types"]));
    }

    #[test]
    fn a_types_fields_failing_is_a_warning_and_that_type_has_no_entry() {
        let mock = fake_jira(&["fields-1"], true);
        let meta = run(&mock, &wanted()).unwrap();
        let mut expected = complete();
        expected.required.remove("Bug");
        expected.warnings = vec!["fields of Bug: Jira 500 on /rest/api/2/issue/createmeta/HD/issuetypes/1?maxResults=500: boom fields-1".into()];
        assert_eq!(meta, expected);
    }

    #[test]
    fn field_list_failing_is_a_warning_and_no_epics_are_read() {
        let mock = fake_jira(&["field"], true);
        let meta = run(&mock, &wanted()).unwrap();
        let expected = ProjectMeta {
            epic_field: None,
            epics: vec![],
            warnings: vec!["the Epic Link field: Jira 500 on /rest/api/2/field: boom field".into()],
            ..complete()
        };
        assert_eq!(meta, expected);
        assert_eq!(parts(&mock), strings(&["types", "fields-3", "fields-1", "fields-5", "field", "parents"]));
    }

    #[test]
    fn epics_failing_is_a_warning_and_the_epic_field_stays() {
        let mock = fake_jira(&["epics"], true);
        let meta = run(&mock, &wanted()).unwrap();
        let expected = ProjectMeta { epics: vec![], warnings: vec![format!("epics: Jira 500 on {}: boom epics", url_of(&mock, "epics"))], ..complete() };
        assert_eq!(meta, expected);
    }

    #[test]
    fn parents_failing_is_a_warning() {
        let mock = fake_jira(&["parents"], true);
        let meta = run(&mock, &wanted()).unwrap();
        let expected =
            ProjectMeta { parents: vec![], warnings: vec![format!("parent issues: Jira 500 on {}: boom parents", url_of(&mock, "parents"))], ..complete() };
        assert_eq!(meta, expected);
    }

    #[test]
    fn several_parts_failing_give_a_warning_each_in_reading_order() {
        let mock = fake_jira(&["fields-3", "fields-5", "parents"], true);
        let meta = run(&mock, &wanted()).unwrap();
        let mut expected = complete();
        expected.required = BTreeMap::from([("Bug".to_owned(), vec![environment()])]);
        expected.parents = vec![];
        expected.warnings = vec![
            "fields of Task: Jira 500 on /rest/api/2/issue/createmeta/HD/issuetypes/3?maxResults=500: boom fields-3".into(),
            "fields of Sub-task: Jira 500 on /rest/api/2/issue/createmeta/HD/issuetypes/5?maxResults=500: boom fields-5".into(),
            format!("parent issues: Jira 500 on {}: boom parents", url_of(&mock, "parents")),
        ];
        assert_eq!(meta, expected);
    }

    #[test]
    fn no_epic_link_field_reads_no_epics() {
        let mock = fake_jira(&[], false);
        let meta = run(&mock, &wanted()).unwrap();
        assert_eq!(meta, ProjectMeta { epic_field: None, epics: vec![], ..complete() });
        assert_eq!(parts(&mock), strings(&["types", "fields-3", "fields-1", "fields-5", "field", "parents"]));
    }

    #[test]
    fn a_required_epic_link_field_is_an_epic_field() {
        let mock = fake_jira(&[], true);
        let meta = run(&mock, &strings(&["Improvement"])).unwrap();
        let expected = ProjectMeta {
            types: vec![it("3", "Task", false), it("4", "Improvement", false)],
            required: BTreeMap::from([("Task".to_owned(), vec![element()]), ("Improvement".to_owned(), vec![epic_required()])]),
            parents: vec![],
            ..complete()
        };
        assert_eq!(meta, expected);
        assert_eq!(parts(&mock), strings(&["types", "fields-3", "fields-4", "field", "epics"]));
    }

    #[test]
    fn no_sub_task_type_offered_reads_no_parents() {
        let mock = fake_jira(&[], true);
        let meta = run(&mock, &strings(&["Bug", "Epic"])).unwrap();
        let expected = ProjectMeta {
            types: vec![it("3", "Task", false), it("1", "Bug", false)],
            required: BTreeMap::from([("Task".to_owned(), vec![element()]), ("Bug".to_owned(), vec![environment()])]),
            parents: vec![],
            ..complete()
        };
        assert_eq!(meta, expected);
        assert_eq!(parts(&mock), strings(&["types", "fields-3", "fields-1", "field", "epics"]));
    }
}

mod creator {
    use super::*;

    const EPIC_FIELD: &str = "customfield_10001";

    fn hd_meta() -> ProjectMeta {
        ProjectMeta {
            types: vec![it("3", "Task", false), it("1", "Bug", false), it("5", "Sub-task", true)],
            required: BTreeMap::from([
                ("Task".to_owned(), vec![element()]),
                ("Bug".to_owned(), vec![environment()]),
                ("Sub-task".to_owned(), vec![steps()]),
            ]),
            epics: vec![issue("HD-7", "Payments epic")],
            parents: vec![issue("HD-12", "Login")],
            epic_field: Some(EPIC_FIELD.into()),
            warnings: vec![],
        }
    }

    fn cfg(default_type: &str, fields: Value) -> TicketConfig {
        TicketConfig { issue_type: default_type.into(), fields: obj(fields), opencode_config: None, ..TicketConfig::default() }
    }

    fn draft(project: &str, title: &str, k: Kind) -> Draft {
        let mut d = Draft::new(project, title, &format!("Body of {title}"), vec![]);
        d.kind = k;
        d
    }

    fn created(mut d: Draft, result: Result<&str, &str>) -> Draft {
        d.created = Some(result.map(str::to_owned).map_err(str::to_owned));
        d
    }

    fn unselected(mut d: Draft) -> Draft {
        d.selected = false;
        d
    }

    fn task() -> Kind {
        with_values(kind("Task", false), vec![("customfield_13520", FieldValue::One("Backend".into()))])
    }

    fn bug() -> Kind {
        with_values(kind("Bug", false), vec![("customfield_11659", FieldValue::Many(set(&["PROD"])))])
    }

    fn sub_task(parent: &str) -> Kind {
        Kind { parent: parent.into(), ..with_values(kind("Sub-task", true), vec![("customfield_12000", FieldValue::Text("why".into()))]) }
    }

    fn creator_with(drafts: Vec<Draft>) -> Creator {
        let mut c = Creator::new("HD".into(), None);
        c.set_meta("HD", hd_meta());
        c.drafts = drafts;
        c
    }

    mod create_jobs {
        use super::*;

        fn job(project: &str, issue_type: &str, summary: &str, extra_fields: Value) -> NewIssue {
            NewIssue {
                project: project.into(),
                issue_type: issue_type.into(),
                summary: summary.into(),
                description: format!("Body of {summary}"),
                assignee: Some("jdoe".into()),
                extra_fields: obj(extra_fields),
            }
        }

        #[test]
        fn each_draft_with_its_type_and_kind_fields_over_the_configured_ones() {
            let c = creator_with(vec![
                draft("HD", "  Payment retry  ", Kind { epic: Some("HD-7".into()), ..task() }),
                draft(
                    "HD",
                    "Crash on save",
                    Kind { values: bug().values.into_iter().chain([("customfield_13520".to_owned(), FieldValue::One("Backend".into()))]).collect(), ..bug() },
                ),
                draft("HD", "Sub piece", Kind { epic: Some("HD-7".into()), ..sub_task(" HD-12 ") }),
                draft("HD", "Default kind", with_values(Kind::default(), vec![("customfield_12000", FieldValue::Text("not required".into()))])),
                draft("ABC", "Other project", Kind { epic: Some("ABC-2".into()), ..kind("Story", false) }),
            ]);
            let cfg = cfg("Improvement", json!({"labels": ["wl"]}));
            assert_eq!(c.create_jobs(&cfg, "jdoe"), vec![
                (0, NewIssue {
                    description: "Body of   Payment retry".into(),
                    ..job("HD", "Task", "Payment retry", json!({"labels": ["wl"], "customfield_10001": "HD-7", "customfield_13520": {"value": "Backend"}}))
                }),
                (1, job("HD", "Bug", "Crash on save", json!({"labels": ["wl"], "customfield_11659": [{"value": "PROD"}]}))),
                (2, job("HD", "Sub-task", "Sub piece", json!({"labels": ["wl"], "parent": {"key": "HD-12"}, "customfield_12000": "why"}))),
                (3, job("HD", "Improvement", "Default kind", json!({"labels": ["wl"]}))),
                (4, job("ABC", "Story", "Other project", json!({"labels": ["wl"]}))),
            ]);
        }

        #[test]
        fn summary_and_description_are_trimmed_and_assignee_is_me() {
            let mut d = draft("HD", " \tTrim me\n", task());
            d.description = "\n  Line one\nLine two  \n".into();
            let c = creator_with(vec![d]);
            assert_eq!(c.create_jobs(&cfg("Task", json!({})), "anowak"), vec![(0, NewIssue {
                project: "HD".into(),
                issue_type: "Task".into(),
                summary: "Trim me".into(),
                description: "Line one\nLine two".into(),
                assignee: Some("anowak".into()),
                extra_fields: obj(json!({"customfield_13520": {"value": "Backend"}})),
            })]);
        }

        #[test]
        fn a_kind_field_wins_over_the_configured_one_on_the_same_key() {
            let c = creator_with(vec![draft("HD", "Overlay", Kind { epic: Some("HD-7".into()), ..bug() })]);
            let cfg = cfg(
                "Task",
                json!({"customfield_10001": "HD-1", "customfield_11659": [{"value": "TEST"}], "priority": {"name": "Minor"}}),
            );
            assert_eq!(c.create_jobs(&cfg, "jdoe"), vec![(
                0,
                job("HD", "Bug", "Overlay", json!({"customfield_10001": "HD-7", "customfield_11659": [{"value": "PROD"}], "priority": {"name": "Minor"}}))
            )]);
        }

        #[test]
        fn blocked_drafts_are_skipped() {
            let c = creator_with(vec![
                draft("HD", "Bug without environment", kind("Bug", false)),
                draft("HD", "Ready", bug()),
                draft("HD", "Sub-task of another project", sub_task("ABC-3")),
            ]);
            assert_eq!(c.create_jobs(&cfg("Task", json!({})), "jdoe"), vec![(1, job("HD", "Bug", "Ready", json!({"customfield_11659": [{"value": "PROD"}]})))]);
        }
    }

    mod blockers_and_sendable {
        use super::*;

        #[test]
        fn each_reason_alone_and_together_title_first() {
            // (name, draft at index 1, expected blockers); index 0 holds a sendable draft.
            let rows: Vec<(&str, Draft, Vec<&str>)> = vec![
                ("ready task", draft("HD", "T", task()), vec![]),
                ("ready sub-task", draft("HD", "T", sub_task("HD-12")), vec![]),
                ("blank title", draft("HD", " \t", task()), vec!["a title"]),
                ("no parent", draft("HD", "T", sub_task("")), vec!["pick a parent"]),
                ("parent of another project", draft("HD", "T", sub_task("ABC-3")), vec!["the parent must be an issue of HD"]),
                ("required field unfilled", draft("HD", "T", kind("Bug", false)), vec!["Environment"]),
                ("the default type's required field", draft("HD", "T", Kind::default()), vec!["Element"]),
                ("all at once", draft("HD", "", kind("Sub-task", true)), vec!["a title", "pick a parent", "Steps"]),
                ("project not read yet: nothing required", draft("ABC", "T", kind("Bug", false)), vec![]),
            ];
            for (name, d, expected) in rows {
                let c = creator_with(vec![draft("HD", "Ready", bug()), d]);
                assert_eq!(c.draft_blockers(1, "Task"), strings(&expected), "{name}");
                assert_eq!(c.sendable(1, "Task"), expected.is_empty(), "{name}");
                assert!(c.sendable(0, "Task"), "{name}: the other draft");
            }
        }

        #[test]
        fn configured_default_type_matches_jiras_name_case_insensitively() {
            let c = creator_with(vec![draft("HD", "T", Kind::default())]);
            assert_eq!(c.draft_blockers(0, "task"), strings(&["Element"]));
            assert!(!c.sendable(0, "task"));
        }

        #[test]
        fn unselected_or_created_is_not_sendable_a_failed_one_is() {
            let rows = [
                ("unselected", unselected(draft("HD", "T", task())), false),
                ("created", created(draft("HD", "T", task()), Ok("HD-40")), false),
                ("failed", created(draft("HD", "T", task()), Err("Jira 400")), true),
            ];
            for (name, d, expected) in rows {
                let c = creator_with(vec![d]);
                assert_eq!(c.draft_blockers(0, "Task"), Vec::<String>::new(), "{name}");
                assert_eq!(c.sendable(0, "Task"), expected, "{name}");
            }
        }

        #[test]
        fn unknown_index_has_no_blockers_and_is_not_sendable() {
            let c = creator_with(vec![draft("HD", "T", task())]);
            assert_eq!(c.draft_blockers(1, "Task"), Vec::<String>::new());
            assert!(!c.sendable(1, "Task"));
            assert_eq!(c.required_for(1, "Task"), vec![]);
        }

        #[test]
        fn required_for_follows_the_drafts_type_and_project() {
            let c = creator_with(vec![draft("HD", "T", kind("Bug", false)), draft("HD", "T", Kind::default()), draft("ABC", "T", kind("Bug", false))]);
            assert_eq!(c.required_for(0, "Task"), vec![environment()]);
            assert_eq!(c.required_for(1, "Task"), vec![element()]);
            assert_eq!(c.required_for(1, "Story"), vec![]);
            assert_eq!(c.required_for(2, "Task"), vec![]);
        }
    }

    mod create_label {
        use super::*;

        #[test]
        fn counts_sendable_drafts_by_type_and_names_their_projects() {
            let cfg = cfg("Task", json!({}));
            // (name, drafts, expected)
            let rows: Vec<(&str, Vec<Draft>, &str)> = vec![
                (
                    "contract example; blocked, unselected and created left out",
                    vec![
                        draft("HD", "A", task()),
                        draft("HD", "B", kind("Bug", false)),
                        draft("HD", "C", with_values(Kind::default(), vec![("customfield_13520", FieldValue::One("Frontend".into()))])),
                        unselected(draft("HD", "D", bug())),
                        created(draft("HD", "E", bug()), Ok("HD-41")),
                        draft("HD", "F", bug()),
                    ],
                    "Create 3 (2 Task, 1 Bug) in HD as jdoe",
                ),
                ("order of first appearance", vec![draft("HD", "A", bug()), draft("HD", "B", task()), draft("HD", "C", task())], "Create 3 (1 Bug, 2 Task) in HD as jdoe"),
                ("nothing sendable", vec![draft("HD", "", task()), draft("HD", "B", sub_task(""))], "Create 0 in Jira as jdoe"),
                ("no drafts", vec![], "Create 0 in Jira as jdoe"),
                (
                    "several projects in order of first appearance",
                    vec![draft("HD", "A", task()), draft("ABC", "B", kind("Story", false)), draft("HD", "C", bug())],
                    "Create 3 (1 Task, 1 Story, 1 Bug) in HD + ABC as jdoe",
                ),
                ("a blocked draft's project is not named", vec![draft("ABC", "A", kind("Sub-task", true)), draft("HD", "B", sub_task("HD-12"))], "Create 1 (1 Sub-task) in HD as jdoe"),
            ];
            for (name, drafts, expected) in rows {
                let mut c = creator_with(drafts);
                c.set_meta("ABC", ProjectMeta::default());
                assert_eq!(c.create_label(&cfg, "jdoe"), expected, "{name}");
            }
        }
    }

    mod defaults {
        use super::*;

        fn proposed(titles: &[&str]) -> Vec<TicketDraft> {
            titles.iter().map(|t| TicketDraft { title: t.to_string(), description: String::new(), items: vec![] }).collect()
        }

        fn kinds(c: &Creator) -> Vec<(String, Kind)> {
            c.drafts.iter().map(|d| (d.project.clone(), d.kind.clone())).collect()
        }

        #[test]
        fn set_kind_changes_only_that_draft_and_becomes_its_projects_default() {
            let mut c = creator_with(vec![draft("HD", "A", Kind::default()), draft("HD", "B", Kind::default()), draft("ABC", "C", Kind::default())]);
            c.set_kind(1, bug());
            assert_eq!(kinds(&c), vec![("HD".into(), Kind::default()), ("HD".into(), bug()), ("ABC".into(), Kind::default())]);
            assert_eq!(c.defaults, BTreeMap::from([("HD".to_owned(), bug())]));

            c.set_kind(2, kind("Story", false));
            c.set_kind(0, task());
            assert_eq!(c.defaults, BTreeMap::from([("HD".to_owned(), task()), ("ABC".to_owned(), kind("Story", false))]));
        }

        #[test]
        fn set_kind_on_an_unknown_index_changes_nothing() {
            let mut c = creator_with(vec![draft("HD", "A", Kind::default())]);
            c.set_kind(1, bug());
            assert_eq!(kinds(&c), vec![("HD".into(), Kind::default())]);
            assert_eq!(c.defaults, BTreeMap::new());
        }

        #[test]
        fn new_empty_drafts_start_with_their_projects_default() {
            let mut c = creator_with(vec![draft("HD", "A", Kind::default()), draft("ABC", "B", Kind::default())]);
            c.set_kind(0, bug());
            c.add_empty_draft(&[]);
            c.project = "XY".into();
            c.add_empty_draft(&[]);
            c.set_kind(1, sub_task("ABC-2"));
            c.project = "ABC".into();
            c.add_empty_draft(&[]);
            assert_eq!(kinds(&c), vec![
                ("HD".into(), bug()),
                ("ABC".into(), sub_task("ABC-2")),
                ("HD".into(), bug()),
                ("XY".into(), Kind::default()),
                ("ABC".into(), sub_task("ABC-2")),
            ]);
        }

        #[test]
        fn llm_drafts_start_with_their_projects_default() {
            let mut c = creator_with(vec![draft("HD", "A", Kind::default()), draft("ABC", "B", Kind::default())]);
            c.set_kind(0, bug());
            c.set_kind(1, kind("Story", false));
            c.apply_drafts(Ok(proposed(&["One", "Two"])), &[]);
            assert_eq!(kinds(&c), vec![("HD".into(), bug()), ("HD".into(), bug())]);
            c.project = "ABC".into();
            c.apply_drafts(Ok(proposed(&["Three"])), &[]);
            assert_eq!(kinds(&c), vec![("ABC".into(), kind("Story", false))]);
            c.project = "XY".into();
            c.apply_drafts(Ok(proposed(&["Four"])), &[]);
            assert_eq!(kinds(&c), vec![("XY".into(), Kind::default())]);
        }
    }

    mod same_for_all {
        use super::*;

        #[test]
        fn copies_the_kind_to_the_other_uncreated_drafts_of_its_project() {
            let source = Kind { epic: Some("HD-7".into()), ..bug() };
            let mut c = creator_with(vec![
                draft("HD", "A", task()),
                draft("HD", "Source", source.clone()),
                created(draft("HD", "Done", kind("Story", false)), Ok("HD-42")),
                draft("ABC", "Other project", kind("Story", false)),
                created(draft("HD", "Failed", Kind::default()), Err("Jira 400")),
                unselected(draft("HD", "Unticked", sub_task("HD-12"))),
            ]);
            c.same_for_all(1);
            let kinds: Vec<Kind> = c.drafts.iter().map(|d| d.kind.clone()).collect();
            assert_eq!(kinds, vec![source.clone(), source.clone(), kind("Story", false), kind("Story", false), source.clone(), source]);
        }

        #[test]
        fn unknown_index_changes_nothing() {
            let mut c = creator_with(vec![draft("HD", "A", task()), draft("HD", "B", bug())]);
            c.same_for_all(2);
            let kinds: Vec<Kind> = c.drafts.iter().map(|d| d.kind.clone()).collect();
            assert_eq!(kinds, vec![task(), bug()]);
        }
    }

    mod created_parents {
        use super::*;

        fn keys(v: Vec<Issue>) -> Vec<(String, String)> {
            v.into_iter().map(|i| (i.key, i.summary)).collect()
        }

        #[test]
        fn created_non_sub_tasks_of_the_project_newest_first() {
            let c = creator_with(vec![
                created(draft("HD", "  First  ", task()), Ok("HD-20")),
                created(draft("HD", "A sub-task", sub_task("HD-20")), Ok("HD-21")),
                created(draft("ABC", "Elsewhere", task()), Ok("ABC-5")),
                created(draft("HD", "Failed", task()), Err("Jira 400")),
                draft("HD", "Not sent", task()),
                created(draft("HD", "Second", bug()), Ok("HD-22")),
            ]);
            let pair = |k: &str, s: &str| (k.to_owned(), s.to_owned());
            assert_eq!(keys(c.created_parents("HD", "Task")), vec![pair("HD-22", "Second"), pair("HD-20", "First")]);
            assert_eq!(keys(c.created_parents("ABC", "Task")), vec![pair("ABC-5", "Elsewhere")]);
            assert_eq!(keys(c.created_parents("XY", "Task")), vec![]);
        }

        #[test]
        fn an_untouched_draft_is_a_sub_task_when_the_default_type_is_one() {
            let c = creator_with(vec![
                created(draft("HD", "Default kind", Kind::default()), Ok("HD-30")),
                created(draft("HD", "Chosen task", kind("task", true)), Ok("HD-31")),
                created(draft("ABC", "Unread default", Kind::default()), Ok("ABC-8")),
                created(draft("ABC", "Unread chosen", kind("Story", false)), Ok("ABC-9")),
            ]);
            let pair = |k: &str, s: &str| (k.to_owned(), s.to_owned());
            // (default type, HD parents, ABC parents)
            type Row<'a> = (&'a str, Vec<(String, String)>, Vec<(String, String)>);
            let rows: [Row; 3] = [
                ("Task", vec![pair("HD-31", "Chosen task"), pair("HD-30", "Default kind")], vec![pair("ABC-9", "Unread chosen"), pair("ABC-8", "Unread default")]),
                ("sub-task", vec![pair("HD-31", "Chosen task")], vec![pair("ABC-9", "Unread chosen")]),
                ("Sub-task", vec![pair("HD-31", "Chosen task")], vec![pair("ABC-9", "Unread chosen")]),
            ];
            for (default_type, hd, abc) in rows {
                assert_eq!(keys(c.created_parents("HD", default_type)), hd, "HD, {default_type}");
                assert_eq!(keys(c.created_parents("ABC", default_type)), abc, "ABC, {default_type}");
            }
        }
    }

    #[test]
    fn meta_of_is_the_read_metadata_only() {
        let c = creator_with(vec![]);
        assert_eq!(c.meta_of("HD").as_deref(), Some(&hd_meta()));
        assert_eq!(c.meta_of("ABC"), None);
    }

    mod jira_spelling {
        use super::*;

        fn effective(d: Draft, default_type: &str) -> Kind {
            let mut c = creator_with(vec![draft("HD", "Ready", task()), d]);
            c.meta.insert("XY".into(), MetaState::Failed("Jira 500".into()));
            c.effective_kind(1, default_type)
        }

        #[test]
        fn effective_kind_takes_the_spelling_and_subtask_flag_of_the_projects_jira_type() {
            let rich = |t: &str, subtask: bool| Kind {
                epic: Some("HD-7".into()),
                parent: "HD-12".into(),
                ..with_values(kind(t, subtask), vec![("customfield_11659", FieldValue::Many(set(&["TEST"])))])
            };
            // (name, draft, configured default, expected kind)
            let rows: Vec<(&str, Draft, &str, Kind)> = vec![
                ("chosen in lower case", draft("HD", "T", rich("bug", false)), "Task", rich("Bug", false)),
                ("chosen padded upper case", draft("HD", "T", kind(" SUB-TASK ", false)), "Task", kind("Sub-task", true)),
                ("chosen sub-task flag corrected", draft("HD", "T", kind("Sub-task", false)), "Task", kind("Sub-task", true)),
                ("chosen task flag corrected", draft("HD", "T", kind("task", true)), "Bug", kind("Task", false)),
                ("default in another case, padded", draft("HD", "T", Kind::default()), " tASK ", kind("Task", false)),
                ("default Jira flags as sub-task", draft("HD", "T", Kind::default()), "sub-task", kind("Sub-task", true)),
                ("default Jira lacks", draft("HD", "T", Kind::default()), "Story", Kind::default()),
                ("chosen name Jira lacks", draft("HD", "T", rich("Spike", true)), "Task", rich("Spike", true)),
                ("project not read", draft("ABC", "T", kind("bug", true)), "Task", kind("bug", true)),
                ("project failed to read", draft("XY", "T", kind("bug", false)), "Task", kind("bug", false)),
                ("project read, default Jira lacks, sub in its name", draft("HD", "T", Kind::default()), "Subtask", Kind::default()),
                ("project not read, default a sub-task name", draft("ABC", "T", Kind::default()), "Sub-task", kind("", true)),
                ("project not read, default padded upper case", draft("ABC", "T", Kind::default()), " SUBTASK ", kind("", true)),
                ("project failed to read, default a sub-task name", draft("XY", "T", Kind::default()), "sub-task", kind("", true)),
                ("project not read, default not a sub-task", draft("ABC", "T", kind("", true)), "Task", kind("", false)),
                ("project not read, chosen sub-task keeps its flag", draft("ABC", "T", kind("Sub-task", false)), "Sub-task", kind("Sub-task", false)),
            ];
            for (name, d, default_type, expected) in rows {
                assert_eq!(effective(d, default_type), expected, "{name}");
            }
        }

        #[test]
        fn effective_kind_of_an_unknown_index_is_the_default_kind() {
            let c = creator_with(vec![draft("HD", "T", bug())]);
            assert_eq!(c.effective_kind(1, "Task"), Kind::default());
        }

        #[test]
        fn create_jobs_sends_jiras_spelling_for_a_lower_case_default_or_chosen_type() {
            let c = creator_with(vec![
                draft("HD", "Default", with_values(Kind::default(), vec![("customfield_13520", FieldValue::One("Backend".into()))])),
                draft("HD", "Chosen", with_values(kind("bug", false), vec![("customfield_11659", FieldValue::Many(set(&["PROD"])))])),
                draft("ABC", "Unread project", kind("bug", false)),
            ]);
            let jobs: Vec<(usize, String, Map<String, Value>)> =
                c.create_jobs(&cfg("task", json!({})), "jdoe").into_iter().map(|(i, n)| (i, n.issue_type, n.extra_fields)).collect();
            assert_eq!(jobs, vec![
                (0, "Task".to_owned(), obj(json!({"customfield_13520": {"value": "Backend"}}))),
                (1, "Bug".to_owned(), obj(json!({"customfield_11659": [{"value": "PROD"}]}))),
                (2, "bug".to_owned(), Map::new()),
            ]);
        }

        #[test]
        fn chosen_lower_case_type_needs_the_fields_of_jiras_type() {
            let c = creator_with(vec![draft("HD", "T", kind("bug", false))]);
            assert_eq!(c.draft_blockers(0, "Task"), strings(&["Environment"]));
            assert_eq!(c.create_jobs(&cfg("Task", json!({})), "jdoe"), vec![]);
        }

        #[test]
        fn create_label_counts_case_variants_as_jiras_type() {
            let c = creator_with(vec![
                draft("HD", "A", with_values(Kind::default(), vec![("customfield_13520", FieldValue::One("Backend".into()))])),
                draft("HD", "B", task()),
                draft("HD", "C", Kind { issue_type: "TASK".into(), ..task() }),
                draft("HD", "D", with_values(kind("bug", false), vec![("customfield_11659", FieldValue::Many(set(&["PROD"])))])),
            ]);
            assert_eq!(c.create_label(&cfg("task", json!({})), "jdoe"), "Create 4 (3 Task, 1 Bug) in HD as jdoe");
        }

        #[test]
        fn default_type_jira_flags_as_sub_task_needs_a_parent_and_sends_it() {
            let cfg = cfg("sub-task", json!({}));
            let c = creator_with(vec![draft("HD", "No parent", Kind::default())]);
            assert_eq!(c.draft_blockers(0, &cfg.issue_type), strings(&["pick a parent", "Steps"]));
            assert_eq!(c.create_jobs(&cfg, "jdoe"), vec![]);

            let filled = Kind { parent: " HD-12 ".into(), ..with_values(Kind::default(), vec![("customfield_12000", FieldValue::Text("why".into()))]) };
            let c = creator_with(vec![draft("HD", "With parent", filled)]);
            assert_eq!(c.draft_blockers(0, &cfg.issue_type), Vec::<String>::new());
            assert_eq!(c.create_jobs(&cfg, "jdoe"), vec![(0, NewIssue {
                project: "HD".into(),
                issue_type: "Sub-task".into(),
                summary: "With parent".into(),
                description: "Body of With parent".into(),
                assignee: Some("jdoe".into()),
                extra_fields: obj(json!({"parent": {"key": "HD-12"}, "customfield_12000": "why"})),
            })]);
        }

        #[test]
        fn unread_project_default_sub_task_needs_a_parent_and_sends_it() {
            let cfg = cfg("Sub-task", json!({"labels": ["wl"]}));
            let c = creator_with(vec![draft("ABC", "No parent", Kind::default())]);
            assert_eq!(c.draft_blockers(0, &cfg.issue_type), strings(&["pick a parent"]));
            assert_eq!(c.create_jobs(&cfg, "jdoe"), vec![]);

            let c = creator_with(vec![draft("ABC", "With parent", Kind { parent: "ABC-3".into(), ..Kind::default() })]);
            assert_eq!(c.create_jobs(&cfg, "jdoe"), vec![(0, NewIssue {
                project: "ABC".into(),
                issue_type: "Sub-task".into(),
                summary: "With parent".into(),
                description: "Body of With parent".into(),
                assignee: Some("jdoe".into()),
                extra_fields: obj(json!({"labels": ["wl"], "parent": {"key": "ABC-3"}})),
            })]);
        }
    }

    mod meta_loading {
        use std::sync::mpsc::channel;
        use std::time::{Duration, Instant};

        use super::*;
        use crate::load::{complete, fake_jira, part, provided};
        use worklog::config::JiraCredentials;

        /// "loading", "ready" (with the metadata compared separately) or "failed: <why>".
        fn state(c: &Creator, project: &str) -> String {
            match c.meta.get(project) {
                Some(MetaState::Loading(_)) => "loading".into(),
                Some(MetaState::Ready(_)) => "ready".into(),
                Some(MetaState::Failed(e)) => format!("failed: {e}"),
                None => "none".into(),
            }
        }

        fn wait_until_read(c: &mut Creator, project: &str) {
            let until = Instant::now() + Duration::from_secs(20);
            while state(c, project) == "loading" {
                assert!(Instant::now() < until, "{project} still loading after 20 s");
                std::thread::sleep(Duration::from_millis(10));
                c.poll_meta();
            }
        }

        fn types_requests(mock: &MockJira) -> usize {
            mock.requests().iter().filter(|r| part(&r.url) == "types").count()
        }

        fn ticket_cfg() -> TicketConfig {
            TicketConfig { issue_type: "Task".into(), fields: provided(), opencode_config: None, ..TicketConfig::default() }
        }

        #[test]
        fn poll_meta_settles_each_loading_project_by_what_its_reader_sent() {
            let mut c = Creator::new("HD".into(), None);
            let (ok_tx, ok_rx) = channel();
            let (err_tx, err_rx) = channel();
            let (late_tx, late_rx) = channel();
            let (gone_tx, gone_rx) = channel::<Result<ProjectMeta, String>>();
            ok_tx.send(Ok(hd_meta())).unwrap();
            err_tx.send(Err("Jira 500 on /rest/api/2/issue/createmeta/AB/issuetypes: boom".to_owned())).unwrap();
            drop(gone_tx);
            c.meta.insert("HD".into(), MetaState::Loading(ok_rx));
            c.meta.insert("AB".into(), MetaState::Loading(err_rx));
            c.meta.insert("LATE".into(), MetaState::Loading(late_rx));
            c.meta.insert("GONE".into(), MetaState::Loading(gone_rx));
            c.meta.insert("OLD".into(), MetaState::Failed("earlier".into()));

            c.poll_meta();
            let states: Vec<(&str, String)> = ["HD", "AB", "LATE", "GONE", "OLD"].into_iter().map(|p| (p, state(&c, p))).collect();
            assert_eq!(states, vec![
                ("HD", "ready".to_owned()),
                ("AB", "failed: Jira 500 on /rest/api/2/issue/createmeta/AB/issuetypes: boom".to_owned()),
                ("LATE", "loading".to_owned()),
                ("GONE", "failed: reading Jira stopped unexpectedly".to_owned()),
                ("OLD", "failed: earlier".to_owned()),
            ]);
            assert_eq!(c.meta_of("HD").as_deref(), Some(&hd_meta()));

            let late = ProjectMeta { epic_field: Some("customfield_10999".into()), ..ProjectMeta::default() };
            late_tx.send(Ok(late.clone())).unwrap();
            c.poll_meta();
            assert_eq!(state(&c, "LATE"), "ready");
            assert_eq!(c.meta_of("LATE").as_deref(), Some(&late));
            assert_eq!(c.meta_of("HD").as_deref(), Some(&hd_meta()));
        }

        #[test]
        fn ensure_meta_reads_a_project_once() {
            let mock = fake_jira(&[], true);
            let creds = JiraCredentials { url: mock.base.clone(), token: TOKEN.into() };
            let mut c = Creator::new("HD".into(), None);
            c.ensure_meta(&ticket_cfg(), &creds, "HD");
            assert_eq!(state(&c, "HD"), "loading");
            c.ensure_meta(&ticket_cfg(), &creds, "HD");
            wait_until_read(&mut c, "HD");
            assert_eq!(c.meta_of("HD").as_deref(), Some(&complete()));

            c.ensure_meta(&ticket_cfg(), &creds, "HD");
            assert_eq!(state(&c, "HD"), "ready");
            assert_eq!(types_requests(&mock), 1);
        }

        #[test]
        fn a_failed_read_is_kept_until_its_entry_is_removed() {
            let mock = fake_jira(&["types"], true);
            let creds = JiraCredentials { url: mock.base.clone(), token: TOKEN.into() };
            let failed = "failed: Jira 500 on /rest/api/2/issue/createmeta/HD/issuetypes: boom types";
            let mut c = Creator::new("HD".into(), None);
            c.ensure_meta(&ticket_cfg(), &creds, "HD");
            wait_until_read(&mut c, "HD");
            assert_eq!(state(&c, "HD"), failed);

            c.ensure_meta(&ticket_cfg(), &creds, "HD");
            assert_eq!(state(&c, "HD"), failed);
            assert_eq!(types_requests(&mock), 1);

            c.meta.remove("HD");
            c.ensure_meta(&ticket_cfg(), &creds, "HD");
            assert_eq!(state(&c, "HD"), "loading");
            wait_until_read(&mut c, "HD");
            assert_eq!(state(&c, "HD"), failed);
            assert_eq!(types_requests(&mock), 2);
        }
    }
}
