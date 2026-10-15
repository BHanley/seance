//! Lifetime identity regressions. All state and PTYs here belong to the tests.
use super::tests::with_test_state_dir;
use super::*;
use crate::control::ControlRequest;
use serde_json::{json, Value};

fn engine() -> Engine {
    Engine::bare_for_test(crate::state::state_dir().unwrap().join("scratch")).0
}

fn file_spec(name: &str, workspace: &str) -> SpawnSpec {
    SpawnSpec {
        name: name.into(),
        workspace: Some(workspace.into()),
        cwd: None,
        command: None,
        tiled: true,
        resume: false,
        file: Some("/tmp/identity-test.md".into()),
    }
}

fn assert_uuid(id: &str) {
    assert!(
        regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")
            .unwrap()
            .is_match(id),
        "{id}"
    );
}

fn identities(eng: &Engine) -> Vec<(String, String)> {
    eng.panes
        .iter()
        .map(|p| (p.pane_id.clone(), eng.workspace_ids[&p.workspace].clone()))
        .collect()
}

#[test]
fn mint_rename_sleep_restore_and_reuse() {
    with_test_state_dir("lifetime-ids", || {
        let mut eng = engine();
        let ws = eng.create_workspace("Research Circle");
        let empty = eng.create_workspace("Empty Circle");
        let slug = eng.spawn(file_spec("worker", &ws)).unwrap();
        eng.spawn(file_spec("worker", &ws)).unwrap();
        let ids = identities(&eng);
        let empty_id = eng.workspace_ids[&empty].clone();
        for (pane, workspace) in &ids {
            assert_uuid(pane);
            assert_uuid(workspace);
        }
        assert_uuid(&empty_id);
        assert_ne!(ids[0].0, ids[1].0);
        assert_eq!(ids[0].1, ids[1].1);
        assert_ne!(ids[0].1, empty_id);

        eng.handle_gui(
            GuiRequest::RenamePane {
                pane: slug.clone(),
                name: "Renamed worker".into(),
            },
            "test",
        );
        eng.rename_workspace(&ws, "Renamed circle").unwrap();
        assert_eq!(identities(&eng), ids);
        eng.sleep_workspace(&ws).unwrap();
        assert_eq!(identities(&eng), ids);
        eng.persist();
        drop(eng);

        let (mut eng, _) = Engine::new().unwrap();
        assert!(eng.panes.iter().all(|p| p.asleep));
        assert_eq!(identities(&eng), ids);
        assert_eq!(eng.workspace_ids[&empty], empty_id);
        assert_eq!(eng.workspace_label(&ws), "Renamed circle");
        assert_eq!(eng.panes[0].name, "Renamed worker");
        eng.wake_workspace(&ws).unwrap();
        assert_eq!(identities(&eng), ids);

        eng.kill_pane(&slug);
        // Same slot/slug, including a successor spawned by ctl handoff: spawn
        // always mints, regardless of the predecessor's conversation or cwd.
        let replacement = eng.spawn(file_spec("worker", &ws)).unwrap();
        assert_eq!(replacement, slug);
        assert!(eng.panes.iter().all(|p| p.pane_id != ids[0].0));
        eng.persist();
        assert!(AppState::load().panes.iter().all(|p| p.pane_id != ids[0].0));

        // Explicitly deleted circles can reuse their slug, never their UUID.
        eng.extra_workspaces.retain(|w| w != &empty);
        eng.forget_workspace(&empty);
        assert_eq!(eng.create_workspace("Empty Circle"), empty);
        assert_ne!(eng.workspace_ids[&empty], empty_id);
        // Implicit circles disappear with their last pane too.
        let transient = eng.spawn(file_spec("ephemeral", "transient")).unwrap();
        let transient_id = eng.workspace_ids["transient"].clone();
        eng.kill_pane(&transient);
        eng.spawn(file_spec("ephemeral", "transient")).unwrap();
        assert_ne!(eng.workspace_ids["transient"], transient_id);
    });
}

#[test]
fn migration_is_saved_before_restore_and_is_idempotent() {
    with_test_state_dir("identity-migration", || {
        let mut eng = engine();
        eng.create_workspace("empty");
        eng.spawn(file_spec("one", "lab")).unwrap();
        eng.spawn(file_spec("two", "lab")).unwrap();
        eng.sleep_workspace("lab").unwrap();
        eng.persist();
        let path = crate::state::state_dir().unwrap().join("state.json");
        let mut old: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        old.as_object_mut().unwrap().remove("workspace_ids");
        for pane in old["sessions"].as_array_mut().unwrap() {
            pane.as_object_mut().unwrap().remove("pane_id");
        }
        std::fs::write(&path, serde_json::to_vec(&old).unwrap()).unwrap();
        let migrated = AppState::load_with_ids().unwrap();
        let saved = AppState::load(); // No migration or engine persistence on this read.
        assert_eq!(saved.workspace_ids, migrated.workspace_ids);
        assert_uuid(&saved.workspace_ids["empty"]);
        assert_uuid(&saved.workspace_ids["lab"]);
        assert_eq!(saved.panes[0].pane_id, migrated.panes[0].pane_id);
        assert_uuid(&saved.panes[0].pane_id);
        assert_ne!(saved.panes[0].pane_id, saved.panes[1].pane_id);
        let bytes = std::fs::read(&path).unwrap();
        AppState::load_with_ids().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let (restored, _) = Engine::new().unwrap();
        assert_eq!(restored.panes[0].pane_id, saved.panes[0].pane_id);
        assert_eq!(restored.workspace_ids, saved.workspace_ids);
    });
}

#[test]
fn ctl_exposes_ids_and_id_targets_obey_scope_without_slug_fallback() {
    with_test_state_dir("identity-ctl", || {
        let mut eng = engine();
        let slug = eng.spawn(file_spec("worker", "lab")).unwrap();
        let id = eng.panes[0].pane_id.clone();
        let ws_id = eng.workspace_ids["lab"].clone();
        let key = format!("id:{id}");
        // Deliberately ambiguous slug: explicit UUID addressing still wins.
        eng.spawn(file_spec("lead", "worker")).unwrap();
        for req in [
            ControlRequest::List {
                scope: None,
                from: None,
            },
            ControlRequest::Roster {
                scope: None,
                from: None,
            },
            ControlRequest::Brief {
                scope: None,
                from: None,
            },
        ] {
            let row = eng.handle_control(req).data.unwrap()["panes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["slug"] == slug)
                .unwrap()
                .clone();
            assert_eq!(row["pane_id"], id);
            assert_eq!(row["workspace_id"], ws_id);
        }
        let me = eng
            .handle_control(ControlRequest::Whoami {
                scope: None,
                from: Some(slug.clone()),
            })
            .data
            .unwrap();
        assert_eq!(me["pane_id"], id);
        assert_eq!(me["workspace_id"], ws_id);
        let status = |scope| ControlRequest::Status {
            pane: key.clone(),
            scope,
            from: None,
        };
        assert_eq!(eng.handle_control(status(None)).data.unwrap()["slug"], slug);
        assert!(!eng.handle_control(status(Some("worker".into()))).ok);
        assert_eq!(eng.resolve_target(&key).unwrap().pane, slug);
        assert!(
            eng.handle_control(ControlRequest::Lead {
                workspace: Some("lab".into()),
                pane: Some(key.clone()),
                scope: None,
                from: None
            })
            .ok
        );
        assert_eq!(eng.pane_infos()[0].pane_id, id);
        assert_eq!(eng.pane_infos()[0].workspace_id, ws_id);
        assert!(
            eng.handle_control(ControlRequest::Kill {
                pane: key.clone(),
                scope: Some("lab".into()),
                from: None
            })
            .ok
        );
        eng.spawn(file_spec("worker", "lab")).unwrap();
        assert!(!eng.handle_control(status(None)).ok);
        assert!(eng.resolve_target(&key).is_err());
        assert!(!crate::ctl::pane_row_matches(
            &json!({"slug": key, "pane_id": "different"}),
            &key
        ));
    });
}

fn assert_process_env(eng: &Engine) {
    let p = &eng.panes[0];
    let expected = format!("IDS:{}:{}", p.pane_id, eng.workspace_ids[&p.workspace]);
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let text = p.session.as_ref().unwrap().screen_text(None);
        if text.contains(&expected) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected:?}, got {text:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn daemon_upgrade_transfers_ids_and_migrates_legacy_bundles() {
    with_test_state_dir("identity-upgrade", || {
        let mut eng = engine();
        eng.spawn(file_spec("worker", "lab")).unwrap();
        eng.create_workspace("empty");
        eng.sleep_workspace("lab").unwrap();
        let ids = identities(&eng);
        let workspaces = eng.workspace_ids.clone();
        let (bundle, fds) = eng.prepare_upgrade().unwrap();
        crate::runtime::set_upgrade_in_progress(false);
        assert!(fds.is_empty());
        let mut wire = serde_json::to_value(&bundle).unwrap();
        let (restored, _) = Engine::from_handoff(bundle, vec![]).unwrap();
        assert_eq!(identities(&restored), ids);
        assert_eq!(restored.workspace_ids, workspaces);
        assert!(restored.panes[0].asleep);
        assert_eq!(AppState::load().panes[0].pane_id, ids[0].0);

        wire.as_object_mut().unwrap().remove("workspace_ids");
        wire["panes"][0].as_object_mut().unwrap().remove("pane_id");
        let legacy = serde_json::from_value(wire).unwrap();
        let (migrated, _) = Engine::from_handoff(legacy, vec![]).unwrap();
        let saved = AppState::load();
        assert_eq!(saved.panes[0].pane_id, migrated.panes[0].pane_id);
        assert_eq!(saved.workspace_ids, migrated.workspace_ids);
        assert_uuid(&saved.panes[0].pane_id);
        assert_uuid(&saved.workspace_ids["empty"]);
        let (cold, _) = Engine::new().unwrap();
        assert_eq!(identities(&cold), identities(&migrated));
    });
}

#[test]
fn terminal_environment_keeps_ids_on_wake_and_cold_restore() {
    with_test_state_dir("identity-env", || {
        let mut eng = engine();
        let slug = eng.spawn(SpawnSpec {
            file: None,
            command: Some("/bin/sh -c 'printf \"IDS:%s:%s\\n\" \"$SEANCE_PANE_ID\" \"$SEANCE_WORKSPACE_ID\"; read line'".into()),
            ..file_spec("env-check", "lab")
        }).unwrap();
        let ids = identities(&eng);
        assert_process_env(&eng);
        // Model a saved sleeping terminal without starting an actual agent.
        eng.panes[0].asleep = true;
        eng.panes[0].session.take().unwrap().shutdown();
        eng.persist();
        drop(eng);
        let (mut eng, _) = Engine::new().unwrap();
        assert!(eng.panes[0].asleep);
        assert_eq!(identities(&eng), ids);
        eng.wake_pane(&slug).unwrap();
        assert_process_env(&eng);
        assert_eq!(identities(&eng), ids);
        eng.persist();
        drop(eng);
        let (eng, _) = Engine::new().unwrap();
        assert_process_env(&eng);
        assert_eq!(identities(&eng), ids);
    });
}

/// A fresh pane that is handed a dead pane's slug inherits nothing: not its
/// task, its pad, its sidecars, its messages. The dead pane's pad is kept
/// under its UUID in scratch/archive/.
#[test]
fn reused_slug_inherits_no_task_pad_or_inbox() {
    with_test_state_dir("slug-reuse", || {
        let mut eng = engine();
        let slug = eng.spawn(file_spec("claude", "lab")).unwrap();
        let old_id = eng.panes[0].pane_id.clone();
        let tid = eng.begin_task(&slug, "build the Wander workspace");
        let pad = eng.panes[0].scratch_path.clone();
        std::fs::write(&pad, "Kevin Weekly agenda — finish note\n").unwrap();
        eng.post_message(crate::runtime::protocol::MessageRecord {
            kind: "tell".into(),
            to_pane: slug.clone(),
            text: "for the old pane".into(),
            status: "delivered".into(),
            ..Default::default()
        });
        // Records were made in an earlier millisecond than the successor's birth.
        for t in eng.tasks.values_mut() {
            t.created_ms -= 1_000;
        }
        for m in eng.comms.messages.iter_mut() {
            m.created_ms -= 1_000;
        }

        eng.kill_pane(&slug);
        let reused = eng.spawn(file_spec("claude", "lab")).unwrap();
        assert_eq!(reused, slug);
        let idx = eng.panes.iter().position(|p| p.slug == slug).unwrap();
        assert_ne!(eng.panes[idx].pane_id, old_id);
        let fresh_pad = eng.panes[idx].scratch_path.clone();

        let task = eng.handle_control(ControlRequest::Task {
            pane: Some(slug.clone()),
            id: None,
            scope: None,
            from: None,
        });
        assert!(!task.ok, "inherited a task: {:?}", task.data);
        let me = eng.handle_control(ControlRequest::Whoami {
            scope: None,
            from: Some(slug.clone()),
        });
        assert_eq!(me.data.as_ref().unwrap()["task_id"], Value::Null);
        let row = eng.pane_summary_json(&eng.panes[idx]);
        assert_eq!(row["task_id"], Value::Null);
        let inbox = eng.handle_control(ControlRequest::MsgList {
            pane: Some(slug.clone()),
            circle: None,
            limit: None,
            scope: None,
            from: None,
        });
        assert_eq!(inbox.data.unwrap()["messages"], json!([]));

        let body = std::fs::read_to_string(&fresh_pad).unwrap();
        assert!(!body.contains("Kevin"), "inherited the pad: {body}");
        assert!(!pad.with_extension("taskid").exists());
        assert!(!pad.with_extension("task.json").exists());
        let archive = pad.parent().unwrap().join("archive");
        let kept = std::fs::read_to_string(archive.join(format!("{old_id}.md"))).unwrap();
        assert!(kept.contains("Kevin Weekly agenda"));
        let kept_id = std::fs::read_to_string(archive.join(format!("{old_id}.taskid"))).unwrap();
        assert_eq!(kept_id.trim(), tid);
        assert_eq!(eng.tasks[&tid].status, "orphaned");
        // The task itself is still addressable by id.
        let by_id = eng.handle_control(ControlRequest::Task {
            pane: None,
            id: Some(tid.clone()),
            scope: None,
            from: None,
        });
        assert!(by_id.ok);
    });
}

/// State written before close retired slug state: a pad and an active task
/// left behind under a dead slug. A new pane taking the slug sweeps the pad
/// aside; a restart drops active tasks that no live pane owns, while a live
/// pane keeps its own.
#[test]
fn leftovers_from_before_the_fix_are_cleared() {
    with_test_state_dir("slug-reuse-legacy", || {
        let mut eng = engine();
        let keeper = eng.spawn(file_spec("keeper", "lab")).unwrap();
        let own = eng.begin_task(&keeper, "keeper's own task");
        let gone = eng.spawn(file_spec("gone", "lab")).unwrap();
        let leaked = eng.begin_task(&gone, "assignment for a dead pane");
        let pad = eng.panes[1].scratch_path.clone();
        std::fs::write(&pad, "old notes\n").unwrap();
        eng.tasks.get_mut(&leaked).unwrap().created_ms -= 1_000;
        // Old close path: pane removed, slug state left in place.
        eng.panes.retain(|p| p.slug != gone);
        eng.comms.born.remove(&gone);
        eng.persist();
        drop(eng);

        let (mut eng, _) = Engine::new().unwrap();
        assert_eq!(eng.active_tasks.get(&keeper), Some(&own));
        assert!(!eng.active_tasks.contains_key(&gone));
        assert_eq!(eng.tasks[&leaked].status, "orphaned");
        assert!(pad.exists(), "load must not move pad files");

        let reused = eng.spawn(file_spec("gone", "lab")).unwrap();
        assert_eq!(reused, gone);
        assert!(!std::fs::read_to_string(&pad).unwrap().contains("old notes"));
        let archived: Vec<String> = std::fs::read_dir(pad.parent().unwrap().join("archive"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            archived
                .iter()
                .any(|n| n.starts_with(&format!("orphan-{gone}-")) && n.ends_with(".md")),
            "{archived:?}"
        );
    });
}
