use blyte_cli::skill::{remove_status_line, set_status_line};

#[test]
fn test_1_empty_settings_give_blyte_statusline() {
    let res_empty = set_status_line("")
        .unwrap()
        .expect("should produce new settings");
    let val_empty: serde_json::Value = serde_json::from_str(&res_empty).unwrap();
    assert_eq!(val_empty["statusLine"]["command"], "blyte statusline");
    assert_eq!(val_empty["statusLine"]["type"], "command");
    assert_eq!(
        res_empty,
        "{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"blyte statusline\"\n  }\n}\n"
    );

    let res_obj = set_status_line("{}")
        .unwrap()
        .expect("should produce new settings");
    let val_obj: serde_json::Value = serde_json::from_str(&res_obj).unwrap();
    assert_eq!(val_obj["statusLine"]["command"], "blyte statusline");
    assert_eq!(val_obj["statusLine"]["type"], "command");
    assert_eq!(
        res_obj,
        "{\n  \"statusLine\": {\n    \"type\": \"command\",\n    \"command\": \"blyte statusline\"\n  }\n}\n"
    );
}

#[test]
fn test_2_existing_command_is_wrapped_and_key_order_preserved() {
    let input = r#"{"model":"opus","statusLine":{"type":"command","command":"/u/it's.py"},"z":1}"#;
    let res = set_status_line(input)
        .unwrap()
        .expect("should produce new settings");

    let val: serde_json::Value = serde_json::from_str(&res).unwrap();
    assert_eq!(
        val["statusLine"]["command"],
        r"blyte statusline --after '/u/it'\''s.py'"
    );

    let keys: Vec<_> = val
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys, vec!["model", "statusLine", "z"]);

    let expected = concat!(
        "{\n",
        "  \"model\": \"opus\",\n",
        "  \"statusLine\": {\n",
        "    \"type\": \"command\",\n",
        "    \"command\": \"blyte statusline --after '/u/it'\\\\''s.py'\"\n",
        "  },\n",
        "  \"z\": 1\n",
        "}\n"
    );
    assert_eq!(res, expected);
}

#[test]
fn test_3_running_again_on_own_output_is_none() {
    let out1 = set_status_line("").unwrap().unwrap();
    assert_eq!(set_status_line(&out1), Ok(None));

    let out1_obj = set_status_line("{}").unwrap().unwrap();
    assert_eq!(set_status_line(&out1_obj), Ok(None));

    let input = r#"{"model":"opus","statusLine":{"type":"command","command":"/u/it's.py"},"z":1}"#;
    let out2 = set_status_line(input).unwrap().unwrap();
    assert_eq!(set_status_line(&out2), Ok(None));
}

#[test]
fn test_4_remove_status_line() {
    // remove_status_line of (1)'s output has no statusLine
    let out1 = set_status_line("").unwrap().unwrap();
    let rem1 = remove_status_line(&out1)
        .unwrap()
        .expect("should remove statusLine");
    let val1: serde_json::Value = serde_json::from_str(&rem1).unwrap();
    assert!(val1.get("statusLine").is_none());
    assert_eq!(rem1, "{}\n");

    let out1_obj = set_status_line("{}").unwrap().unwrap();
    let rem1_obj = remove_status_line(&out1_obj)
        .unwrap()
        .expect("should remove statusLine");
    let val1_obj: serde_json::Value = serde_json::from_str(&rem1_obj).unwrap();
    assert!(val1_obj.get("statusLine").is_none());
    assert_eq!(rem1_obj, "{}\n");

    // remove_status_line of (2)'s output gives back command /u/it's.py
    let input = r#"{"model":"opus","statusLine":{"type":"command","command":"/u/it's.py"},"z":1}"#;
    let out2 = set_status_line(input).unwrap().unwrap();
    let rem2 = remove_status_line(&out2)
        .unwrap()
        .expect("should restore statusLine command");
    let val2: serde_json::Value = serde_json::from_str(&rem2).unwrap();
    assert_eq!(val2["statusLine"]["command"], "/u/it's.py");
    let keys2: Vec<_> = val2
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(keys2, vec!["model", "statusLine", "z"]);

    // remove_status_line of {"statusLine":{"command":"mine"}} is Ok(None)
    assert_eq!(
        remove_status_line(r#"{"statusLine":{"command":"mine"}}"#),
        Ok(None)
    );
}

#[test]
fn test_5_invalid_json_is_err() {
    assert!(set_status_line("{not json").is_err());
    assert!(remove_status_line("{not json").is_err());
}
