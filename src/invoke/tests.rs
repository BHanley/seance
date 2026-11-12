use super::*;
use serde_json::json;

/// A status like the real CLI's, with invented names.
fn status_doc() -> Value {
    json!({
        "BackendState": "Running",
        "Self": { "UserID": 7, "DNSName": "mac.tail0000.ts.net." },
        "User": { "7": { "LoginName": "alice@example.com" }, "9": { "LoginName": "bob@example.com" } },
        "Peer": {
            "a": { "UserID": 7, "OS": "iOS", "Online": true },
            "b": { "UserID": 7, "OS": "android", "Online": false },
            "c": { "UserID": 7, "OS": "linux", "Online": true },
            "d": { "UserID": 9, "OS": "iOS", "Online": true }
        }
    })
}

#[test]
fn only_the_standalone_mac_build_starts_invoke() {
    // (macos, personal, remote, has runtime)
    assert!(should_start(true, false, false, true));
    // Brian's own Seance runs his Invoke elsewhere.
    assert!(!should_start(true, true, false, true));
    assert!(!should_start(true, false, true, true));
    assert!(!should_start(false, false, false, true));
    // A build without the bundled runtime has nothing to start.
    assert!(!should_start(true, false, false, false));
}

#[test]
fn the_port_stays_put_unless_it_is_taken() {
    let free = |taken: &'static [u16]| move |p: u16| !taken.contains(&p);
    assert_eq!(pick_port(None, free(&[])), Some(DEFAULT_PORT));
    // The last port wins while it is free, so a phone link keeps working.
    assert_eq!(pick_port(Some(9700), free(&[])), Some(9700));
    assert_eq!(pick_port(Some(9700), free(&[9700])), Some(DEFAULT_PORT));
    assert_eq!(pick_port(None, free(&[9667, 9668])), Some(9669));
    assert_eq!(pick_port(None, |_| false), None);
}

#[test]
fn the_owner_and_phones_come_from_the_status() {
    let s = parse_status(&status_doc());
    assert!(s.running);
    assert_eq!(s.login.as_deref(), Some("alice@example.com"));
    assert_eq!(s.dns.as_deref(), Some("mac.tail0000.ts.net"));
    // Two of this account's phones (one asleep); another account's and a
    // laptop don't count.
    assert_eq!((s.phones, s.phones_online), (2, 1));
}

#[test]
fn signed_out_or_stopped_is_not_running() {
    for state in ["NeedsLogin", "Stopped", "NoState"] {
        let mut v = status_doc();
        v["BackendState"] = json!(state);
        assert!(!parse_status(&v).running, "{state}");
    }
    assert_eq!(parse_status(&json!({})), TsStatus::default());
}

#[test]
fn an_existing_serve_for_invoke_gives_the_phone_address() {
    let serve = json!({
        "TCP": { "8443": { "HTTPS": true } },
        "Web": { "mac.tail0000.ts.net:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:9667" } } } }
    });
    assert_eq!(
        serve_url(&serve, 9667).as_deref(),
        Some("https://mac.tail0000.ts.net:8443")
    );
    // Pointed at some other port: not ours.
    assert_eq!(serve_url(&serve, 9700), None);
    assert_eq!(serve_url(&json!({}), 9667), None);
}

#[test]
fn each_guide_step_checks_itself() {
    let status = parse_status(&status_doc());
    let serve = json!({
        "Web": { "mac.tail0000.ts.net:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:9667" } } } }
    });
    // (a) nothing installed: every later step is empty.
    let g = guide_from(true, 9667, false, None, None);
    assert!(!g.installed && !g.signed_in && !g.link_on && g.url.is_none());
    // (a) installed, (b) not signed in.
    let signed_out = TsStatus::default();
    let g = guide_from(true, 9667, true, Some(&signed_out), None);
    assert!(g.installed && !g.signed_in);
    // (b) signed in, (c) a phone is seen, (d) link still off.
    let g = guide_from(true, 9667, true, Some(&status), None);
    assert!(g.signed_in && g.login.as_deref() == Some("alice@example.com"));
    assert_eq!((g.phones, g.phones_online), (2, 1));
    assert!(!g.link_on && g.url.is_none());
    // (d) link on, (e) the address to open.
    let g = guide_from(true, 9667, true, Some(&status), Some(&serve));
    assert!(g.link_on);
    assert_eq!(g.url.as_deref(), Some("https://mac.tail0000.ts.net:8443"));
    // Invoke not running: there is nothing for a link to point at.
    let g = guide_from(true, 0, true, Some(&status), Some(&serve));
    assert!(!g.invoke_running && !g.link_on);
}

#[test]
fn serve_outcomes_are_told_apart() {
    let url = Some("https://mac.tail0000.ts.net:8443".to_string());
    assert_eq!(
        classify_serve(true, true, "", url.clone()),
        ServeOutcome::On(url.clone().unwrap())
    );
    assert_eq!(classify_serve(false, true, "", None), ServeOutcome::Off);
    // The one-time prompt: Serve is not enabled on this tailnet yet.
    let waiting = "\nServe is not enabled on your tailnet.\nTo enable, visit:\n\n         https://login.tailscale.com/f/serve?node=abc123\n";
    assert_eq!(
        classify_serve(true, false, waiting, None),
        ServeOutcome::NeedsEnable("https://login.tailscale.com/f/serve?node=abc123".into())
    );
    assert!(matches!(
        classify_serve(true, false, "something broke\nport in use", None),
        ServeOutcome::Failed(m) if m == "port in use"
    ));
}

#[test]
fn a_command_waiting_for_a_click_is_killed_with_what_it_said() {
    let dir = std::env::temp_dir().join(format!("seance-invoke-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cli = dir.join("tailscale");
    std::fs::write(
        &cli,
        "#!/bin/sh\necho 'Serve is not enabled on your tailnet.'\necho 'https://login.tailscale.com/f/serve?node=zz'\nsleep 30\n",
    )
    .unwrap();
    std::process::Command::new("chmod")
        .args(["755", &cli.to_string_lossy()])
        .status()
        .unwrap();
    let started = Instant::now();
    let (ok, out) = run_capture(&cli, &["serve"], Duration::from_millis(600));
    assert!(!ok);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        classify_serve(true, ok, &out, None),
        ServeOutcome::NeedsEnable("https://login.tailscale.com/f/serve?node=zz".into())
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_tailscale_override_that_is_missing_means_none_not_the_real_one() {
    let _g = crate::state::test_env_lock();
    let before = std::env::var_os("SEANCE_TAILSCALE");
    std::env::set_var("SEANCE_TAILSCALE", "/nonexistent/tailscale");
    assert_eq!(find_tailscale(), None);
    match before {
        Some(v) => std::env::set_var("SEANCE_TAILSCALE", v),
        None => std::env::remove_var("SEANCE_TAILSCALE"),
    }
}

#[test]
fn old_runtime_folders_are_pruned_to_the_current_and_the_newest_other() {
    let names: Vec<String> = ["0.27.9", "0.28.0", "0.28.1", "0.28.2", "junk"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut gone = prune_versions(&names, "0.28.2");
    gone.sort();
    assert_eq!(gone, ["0.27.9", "0.28.0"]);
}

/// Against a scratch daemon (`SEANCE_SOCKET`) with a fake `tailscale`: the
/// guide before and after the phone link, and the link turned off again.
/// Prints each answer for the proof.
#[test]
#[ignore]
fn e2e_phone_guide_against_a_scratch_daemon() {
    let (client, _events) = crate::gui_client::GuiClient::connect_empty().expect("connect");
    let show = |label: &str, v: serde_json::Value| eprintln!("{label}: {v}");
    show("guide", client.invoke_guide().unwrap());
    show("serve on", client.invoke_serve(true).unwrap());
    show("guide", client.invoke_guide().unwrap());
    show("serve off", client.invoke_serve(false).unwrap());
    show("guide", client.invoke_guide().unwrap());
}

#[test]
fn subscriptions_are_counted_from_invokes_file() {
    assert_eq!(count_subscriptions(""), 0);
    assert_eq!(count_subscriptions("not json"), 0);
    assert_eq!(count_subscriptions("{}"), 0);
    assert_eq!(count_subscriptions("[]"), 0);
    assert_eq!(
        count_subscriptions(r#"[{"endpoint":"https://a"},{"endpoint":"https://b"}]"#),
        2
    );
}

#[test]
fn only_our_invoke_is_restarted() {
    assert!(is_invoke_command(
        "/x/python/bin/python3 /x/seance-chat run"
    ));
    assert!(!is_invoke_command("/usr/bin/vim notes.txt"));
    assert!(!is_invoke_command(""));
}
