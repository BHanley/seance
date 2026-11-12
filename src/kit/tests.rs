use super::*;

fn env(tag: &str) -> Env {
    let root = std::env::temp_dir().join(format!("seance-kit-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    Env {
        home: root.join("home"),
        config: root.join("home/.config/seance"),
        launchctl: false,
    }
}

fn put(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn cleanup(e: &Env) {
    let _ = std::fs::remove_dir_all(e.home.parent().unwrap());
}

#[test]
fn on_writes_the_starter_and_records_it() {
    let e = env("on");
    assert_eq!(status(Item::BrainNotes, &e), Status::Off);
    assert_eq!(on(Item::BrainNotes, &e), Ok(Status::On));
    assert!(read(&e.brain().join("INDEX.md")).starts_with("# Brain index"));
    assert_eq!(read(&e.claude_md()), "@~/Claude/Brain/INDEX.md\n");
    assert!(read(&e.record_path()).contains("brain-notes"));
    assert_eq!(status(Item::BrainNotes, &e), Status::On);
    cleanup(&e);
}

#[test]
fn off_removes_only_what_was_recorded() {
    let e = env("off");
    put(&e.claude_md(), "# Mine\nkeep me\n");
    on(Item::BrainNotes, &e).unwrap();
    on(Item::Rules, &e).unwrap();
    off(Item::BrainNotes, &e).unwrap();
    // Brain notes' file and import are gone; rules' are not; the user's line stays.
    assert!(!e.brain().join("INDEX.md").exists());
    assert!(e.brain().join("rules.md").exists());
    assert_eq!(
        read(&e.claude_md()),
        "# Mine\nkeep me\n@~/Claude/Brain/rules.md\n"
    );
    off(Item::Rules, &e).unwrap();
    assert_eq!(read(&e.claude_md()), "# Mine\nkeep me\n");
    assert!(!e.brain().exists(), "folders the kit made go too");
    assert_eq!(status(Item::Rules, &e), Status::Off);
    cleanup(&e);
}

#[test]
fn a_file_or_key_the_user_already_has_is_never_touched() {
    let e = env("existing");
    put(&e.brain().join("INDEX.md"), "my own index");
    put(&e.brain().join("rules.md"), "my own rules");
    let settings = "{\"autoMemoryDirectory\":\"/somewhere\",\"theme\":\"dark\"}";
    put(&e.settings(), settings);
    for item in [Item::BrainNotes, Item::Rules, Item::Memory] {
        assert_eq!(status(item, &e), Status::AlreadySetUp);
        assert_eq!(on(item, &e), Ok(Status::AlreadySetUp));
        // And turning it off does not remove what the user made.
        assert_eq!(off(item, &e), Ok(Status::AlreadySetUp));
    }
    assert_eq!(read(&e.brain().join("INDEX.md")), "my own index");
    assert_eq!(read(&e.brain().join("rules.md")), "my own rules");
    assert_eq!(read(&e.settings()), settings);
    assert!(!e.claude_md().exists(), "no import line for a skipped item");
    assert!(!e.record_path().exists());
    cleanup(&e);
}

#[test]
fn off_leaves_a_file_the_user_edited() {
    let e = env("edited");
    on(Item::Rules, &e).unwrap();
    let rules = e.brain().join("rules.md");
    put(&rules, "# Rules\nmy edit\n");
    off(Item::Rules, &e).unwrap();
    assert_eq!(read(&rules), "# Rules\nmy edit\n");
    assert!(!read(&e.claude_md()).contains("rules.md"));
    cleanup(&e);
}

#[test]
fn settings_keep_everything_else_through_on_and_off() {
    let e = env("settings");
    let before = json!({
        "theme": "dark",
        "hooks": { "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "/mine.sh" }] }],
                   "Stop": [] }
    });
    write_json(&e.settings(), &before).unwrap();
    on(Item::Memory, &e).unwrap();
    on(Item::ContextWarning, &e).unwrap();
    let mid = read_json(&e.settings()).unwrap();
    assert_eq!(mid["theme"], "dark");
    assert!(mid["autoMemoryDirectory"].is_string());
    assert_eq!(
        mid["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
        2
    );
    off(Item::ContextWarning, &e).unwrap();
    off(Item::Memory, &e).unwrap();
    assert_eq!(read_json(&e.settings()).unwrap(), before);
    assert!(!e.hook_script().exists());
    cleanup(&e);
}

#[test]
fn a_settings_file_the_kit_made_is_removed_when_empty() {
    let e = env("made");
    on(Item::Memory, &e).unwrap();
    assert!(e.settings().exists());
    off(Item::Memory, &e).unwrap();
    assert!(!e.settings().exists());
    assert!(!e.brain().exists());
    cleanup(&e);
}

#[test]
fn an_unreadable_settings_file_is_left_alone_and_nothing_stays_written() {
    let e = env("badjson");
    put(&e.settings(), "not json {");
    assert!(on(Item::Memory, &e).is_err());
    assert_eq!(read(&e.settings()), "not json {");
    assert!(!e.brain().join("memory/MEMORY.md").exists(), "rolled back");
    assert!(!e.record_path().exists());
    cleanup(&e);
}

#[test]
fn weekly_review_writes_an_agent_and_off_deletes_it() {
    let e = env("weekly");
    on(Item::WeeklyReview, &e).unwrap();
    assert!(e.plist().exists() && e.review_script().exists());
    assert!(read(&e.plist()).contains(LABEL));
    off(Item::WeeklyReview, &e).unwrap();
    assert!(!e.plist().exists() && !e.review_script().exists());
    cleanup(&e);
}

#[test]
fn design_sheets_is_coming_soon_and_writes_nothing() {
    let e = env("sheets");
    assert_eq!(status(Item::DesignSheets, &e), Status::ComingSoon);
    assert_eq!(on(Item::DesignSheets, &e), Ok(Status::ComingSoon));
    assert!(!e.record_path().exists());
    cleanup(&e);
}

#[test]
fn the_tiles_show_once_and_only_on_a_fresh_install() {
    let fresh = env("first");
    assert!(first_run(&fresh, false));
    assert!(!first_run(&fresh, false), "marker stops a second showing");
    cleanup(&fresh);
    // An install that already made its Seance Custom circle is an upgrade.
    let upgraded = env("upgrade");
    assert!(!first_run(&upgraded, true));
    assert!(!first_run(&upgraded, false), "and stays quiet later");
    cleanup(&upgraded);
}

#[test]
fn the_context_hook_warns_at_300k_and_asks_for_compact_at_500k() {
    let e = env("hook");
    on(Item::ContextWarning, &e).unwrap();
    let run = |tokens: u64| {
        let transcript = e.home.join("t.jsonl");
        put(
            &transcript,
            &format!(
                "{{\"usage\":{{\"input_tokens\":3,\"cache_creation_input_tokens\":1000,\"cache_read_input_tokens\":{tokens}}}}}\n"
            ),
        );
        let mut child = std::process::Command::new("sh")
            .arg(e.hook_script())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        let input = format!("{{\"transcript_path\":\"{}\"}}", transcript.display());
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    assert_eq!(run(100_000), "");
    assert!(run(350_000).contains("Mention it"));
    assert!(run(600_000).contains("/compact"));
    cleanup(&e);
}
