use std::sync::Arc;

use blyte_cli::conversation_store::{maybe_digest_for_provider, ConversationStore};
use blyte_provider::{Message, UserPart};

#[test]
fn saving_twice_then_loading_returns_messages_goal_and_last_model_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s0123456789abcdef.conversation.jsonl");

    let mut store = ConversationStore::new_for_fresh(path.clone()).unwrap();
    let mut messages = vec![
        Message::User("first question".to_string()),
        Message::Assistant("first answer".to_string()),
    ];
    store
        .save("openai/gpt-test-1", Some("goal one"), &messages)
        .unwrap();

    messages.push(Message::UserParts(vec![
        UserPart::Text("second question".to_string()),
        UserPart::Image {
            mime_type: "image/png".to_string(),
            data: Arc::from("QUJD"),
        },
    ]));
    messages.push(Message::Assistant("second answer".to_string()));
    store
        .save("openai/gpt-test-2", Some("goal two"), &messages)
        .unwrap();

    let (_loaded_store, loaded) = ConversationStore::load(path).unwrap();
    let loaded = loaded.expect("expected loaded conversation");
    assert_eq!(loaded.model.as_deref(), Some("openai/gpt-test-2"));
    assert_eq!(loaded.goal.as_deref(), Some("goal two"));
    assert_eq!(loaded.messages, messages);
}

#[test]
fn second_save_leaves_earlier_bytes_identical_and_only_appends() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s1111111111111111.conversation.jsonl");

    let mut store = ConversationStore::new_for_fresh(path.clone()).unwrap();
    let mut messages = vec![
        Message::User("hello".to_string()),
        Message::Assistant("hi there".to_string()),
    ];
    store
        .save("openai/gpt-test", Some("ship it"), &messages)
        .unwrap();
    let bytes_after_first = std::fs::read(&path).unwrap();

    messages.push(Message::User("follow up".to_string()));
    messages.push(Message::Assistant("got it".to_string()));
    store
        .save("openai/gpt-test", Some("ship it"), &messages)
        .unwrap();
    let bytes_after_second = std::fs::read(&path).unwrap();

    assert!(bytes_after_second.len() > bytes_after_first.len());
    assert_eq!(
        &bytes_after_second[..bytes_after_first.len()],
        bytes_after_first.as_slice()
    );
    let appended = std::str::from_utf8(&bytes_after_second[bytes_after_first.len()..]).unwrap();
    assert!(!appended.contains("\"model\""));
    assert!(!appended.contains("\"goal\""));
    assert_eq!(appended.lines().count(), 2);
}

#[test]
fn file_ending_in_half_a_line_loads_without_it_and_truncates_to_last_full_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s2222222222222222.conversation.jsonl");

    let mut store = ConversationStore::new_for_fresh(path.clone()).unwrap();
    let messages = vec![
        Message::User("remember heron".to_string()),
        Message::Assistant("noted".to_string()),
    ];
    store.save("openai/gpt-test", None, &messages).unwrap();
    let complete_bytes = std::fs::read(&path).unwrap();

    let mut with_tail = complete_bytes.clone();
    with_tail.extend_from_slice(br#"{"message":{"User":"cut off mid"#);
    std::fs::write(&path, &with_tail).unwrap();

    let (_store, loaded) = ConversationStore::load(path.clone()).unwrap();
    let loaded = loaded.expect("expected loaded conversation");
    assert_eq!(loaded.messages, messages);
    assert_eq!(std::fs::read(&path).unwrap(), complete_bytes);
}

#[test]
fn corrupt_middle_line_wrong_version_and_unknown_key_fail_with_path_and_line_number() {
    let dir = tempfile::tempdir().unwrap();

    let bad_middle = dir.path().join("bad_middle.conversation.jsonl");
    std::fs::write(
        &bad_middle,
        "{\"version\":1}\n{\"model\":\"openai/gpt-test\"}\nnot json\n{\"message\":{\"User\":\"hi\"}}\n",
    )
    .unwrap();
    let err = match ConversationStore::load(bad_middle.clone()) {
        Ok(_) => panic!("expected error for bad third line"),
        Err(e) => e,
    };
    assert!(
        err.contains(&bad_middle.display().to_string()),
        "error should contain path: {err}"
    );
    assert!(err.contains("line 3"), "error should contain line 3: {err}");

    let wrong_version = dir.path().join("wrong_version.conversation.jsonl");
    std::fs::write(&wrong_version, "{\"version\":2}\n{\"model\":\"m\"}\n").unwrap();
    let err = match ConversationStore::load(wrong_version.clone()) {
        Ok(_) => panic!("expected error for wrong version"),
        Err(e) => e,
    };
    assert!(
        err.contains(&wrong_version.display().to_string()),
        "error should contain path: {err}"
    );
    assert!(err.contains("line 1"), "error should contain line 1: {err}");

    let unknown_key = dir.path().join("unknown_key.conversation.jsonl");
    std::fs::write(
        &unknown_key,
        "{\"version\":1}\n{\"model\":\"m\"}\n{\"mystery\":123}\n",
    )
    .unwrap();
    let err = match ConversationStore::load(unknown_key.clone()) {
        Ok(_) => panic!("expected error for unknown key"),
        Err(e) => e,
    };
    assert!(
        err.contains(&unknown_key.display().to_string()),
        "error should contain path: {err}"
    );
    assert!(err.contains("line 3"), "error should contain line 3: {err}");
}

#[test]
fn trailing_unmatched_tool_call_is_removed_on_load_and_from_file_while_completed_pair_stays() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s3333333333333333.conversation.jsonl");

    let mut store = ConversationStore::new_for_fresh(path.clone()).unwrap();
    let kept = vec![
        Message::User("check file".to_string()),
        Message::ToolCall {
            id: "c1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "a.txt"}),
            signature: None,
        },
        Message::ToolResult {
            id: "c1".to_string(),
            name: "read".to_string(),
            content: "contents".to_string(),
            is_error: false,
        },
    ];
    let mut with_dangling = kept.clone();
    with_dangling.push(Message::ToolCall {
        id: "c2".to_string(),
        name: "bash".to_string(),
        arguments: serde_json::json!({"command": "echo hi"}),
        signature: Some("sig-2".to_string()),
    });

    store
        .save("openai/gpt-test", Some("inspect"), &with_dangling)
        .unwrap();

    let (_loaded_store, loaded) = ConversationStore::load(path.clone()).unwrap();
    let loaded = loaded.expect("expected loaded conversation");
    assert_eq!(loaded.messages, kept);

    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        !on_disk.contains("\"c2\""),
        "dangling call stayed on disk: {on_disk}"
    );
    assert!(
        on_disk.contains("\"c1\""),
        "completed call missing on disk: {on_disk}"
    );

    let (_reloaded_store, reloaded) = ConversationStore::load(path).unwrap();
    assert_eq!(reloaded.unwrap().messages, kept);
}

#[cfg(unix)]
#[test]
fn file_mode_is_0600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s4444444444444444.conversation.jsonl");

    let mut store = ConversationStore::new_for_fresh(path.clone()).unwrap();
    store
        .save(
            "openai/gpt-test",
            None,
            &[Message::User("secret".to_string())],
        )
        .unwrap();

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);

    store.save("openai/gpt-test", None, &[]).unwrap();
    let mode_after_rewrite = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode_after_rewrite, 0o600);
}

#[test]
fn shorter_conversation_rewrites_file_and_reloads_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s5555555555555555.conversation.jsonl");

    let mut store = ConversationStore::new_for_fresh(path.clone()).unwrap();
    let long_msgs = vec![
        Message::User("one".to_string()),
        Message::Assistant("two".to_string()),
        Message::User("three".to_string()),
        Message::Assistant("four".to_string()),
    ];
    store
        .save("openai/gpt-test", Some("initial"), &long_msgs)
        .unwrap();

    let short_msgs = vec![
        Message::User("folded summary".to_string()),
        Message::Assistant("latest".to_string()),
    ];
    store.save("openai/gpt-test", None, &short_msgs).unwrap();

    let (_loaded_store, loaded) = ConversationStore::load(path).unwrap();
    let loaded = loaded.expect("expected loaded conversation");
    assert_eq!(loaded.messages, short_msgs);
    assert_eq!(loaded.goal, None);
    assert_eq!(loaded.model.as_deref(), Some("openai/gpt-test"));
}

#[test]
fn new_for_fresh_rejects_existing_file_naming_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s6666666666666666.conversation.jsonl");
    std::fs::write(&path, "anything").unwrap();

    let err = match ConversationStore::new_for_fresh(path.clone()) {
        Ok(_) => panic!("expected error when file already exists"),
        Err(e) => e,
    };
    assert!(
        err.contains(&path.display().to_string()),
        "error should name existing file: {err}"
    );
}

#[test]
fn cross_provider_digest_replaces_messages_and_rewrites_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s7777777777777777.conversation.jsonl");

    let mut store = ConversationStore::new_for_fresh(path.clone()).unwrap();
    let messages = vec![
        Message::User("the code word is heron".to_string()),
        Message::Assistant("I will remember heron".to_string()),
    ];
    store
        .save(
            "anthropic/claude-sonnet-4-5",
            Some("keep code word"),
            &messages,
        )
        .unwrap();

    let (mut store, loaded) = ConversationStore::load(path.clone()).unwrap();
    let mut loaded = loaded.unwrap();

    let same_changed =
        maybe_digest_for_provider(&mut store, &mut loaded, "anthropic/claude-opus-4-5").unwrap();
    assert!(!same_changed);
    assert_eq!(loaded.messages, messages);

    let diff_changed =
        maybe_digest_for_provider(&mut store, &mut loaded, "openai/gpt-test").unwrap();
    assert!(diff_changed);
    assert_eq!(loaded.messages.len(), 1);
    let summary_text = loaded.messages[0].user_text().expect("digest is user text");
    assert!(
        summary_text.contains("heron"),
        "summary was: {summary_text}"
    );

    let (_reloaded_store, reloaded) = ConversationStore::load(path).unwrap();
    let reloaded = reloaded.unwrap();
    assert_eq!(reloaded.model.as_deref(), Some("openai/gpt-test"));
    assert_eq!(reloaded.messages, loaded.messages);
}
