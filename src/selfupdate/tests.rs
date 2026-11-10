use super::*;

fn feed_json(version: &str, sha: &str) -> String {
    format!(
        r#"{{"version":"{version}","url":"file:///x/Seance.zip","sha256":"{sha}","notes":"Faster"}}"#
    )
}

const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("seance-selfupdate-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn newer_older_and_same_versions() {
    assert_eq!(compare("0.28.2", "0.28.3"), Verdict::Newer);
    assert_eq!(compare("0.28.2", "0.29.0"), Verdict::Newer);
    assert_eq!(compare("0.28.2", "0.28.2"), Verdict::Same);
    // An older feed (the app was dragged in newer than the feed) is no update.
    assert_eq!(compare("0.28.2", "0.28.1"), Verdict::Older);
    assert_eq!(compare("0.28.10", "0.28.9"), Verdict::Older);
}

#[test]
fn the_feed_must_be_readable() {
    let f = parse_feed(&feed_json("0.28.3", SHA)).unwrap();
    assert_eq!((f.version.as_str(), f.notes.as_str()), ("0.28.3", "Faster"));
    assert!(parse_feed("not json").is_err());
    assert!(parse_feed(&feed_json("banana", SHA)).is_err());
    assert!(parse_feed(&feed_json("0.28.3", "abc")).is_err());
}

#[test]
fn a_checksum_mismatch_is_caught_and_case_does_not_matter() {
    assert!(sha_matches(SHA, SHA));
    assert!(sha_matches(&SHA.to_uppercase(), &format!("{SHA}\n")));
    assert!(!sha_matches(SHA, &SHA.replace('0', "1")));
}

#[test]
fn only_the_standalone_mac_app_in_a_bundle_updates() {
    // (macos, personal, remote, in a bundle)
    assert!(enabled(true, false, false, true));
    assert!(!enabled(true, true, false, true)); // Brian's own Seance
    assert!(!enabled(true, false, true, true)); // a thin client
    assert!(!enabled(false, false, false, true));
    assert!(!enabled(true, false, false, false)); // a dev build
}

#[test]
fn file_and_https_feeds_are_supported_and_nothing_else() {
    assert_eq!(
        source_of("file:///Users/Shared/Seance-test/latest.json").unwrap(),
        Source::File(PathBuf::from("/Users/Shared/Seance-test/latest.json"))
    );
    assert!(matches!(
        source_of("https://example.com/latest.json"),
        Ok(Source::Https(_))
    ));
    assert!(source_of("http://example.com/latest.json").is_err());
    assert!(source_of("ftp://example.com/x").is_err());
}

#[test]
fn a_feed_is_read_from_a_file_address() {
    let d = tmp("feed");
    let p = d.join("latest.json");
    std::fs::write(&p, feed_json("0.28.3", SHA)).unwrap();
    let url = format!("file://{}", p.display());
    assert_eq!(check("0.28.2", &url).unwrap().unwrap().version, "0.28.3");
    assert!(check("0.28.3", &url).unwrap().is_none());
    assert!(check("0.29.0", &url).unwrap().is_none());
    assert!(check("0.28.2", "file:///no/such/latest.json").is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_read_only_or_image_location_is_refused_with_advice() {
    let ok = Path::new("/Applications/Seance.app");
    assert_eq!(classify_location(ok, true), Location::Ok);
    assert_eq!(classify_location(ok, false), Location::NotWritable);
    assert_eq!(
        classify_location(Path::new("/Volumes/Seance 0.28.2/Seance.app"), true),
        Location::FromImage
    );
    assert_eq!(
        classify_location(
            Path::new("/private/var/folders/x/AppTranslocation/ABC/d/Seance.app"),
            true
        ),
        Location::FromImage
    );
    assert!(location_message(&Location::FromImage)
        .unwrap()
        .contains("Applications"));
    assert!(location_message(&Location::NotWritable)
        .unwrap()
        .contains("Move Seance"));
    assert!(location_message(&Location::Ok).is_none());
}

#[test]
fn the_swap_keeps_the_old_app_until_cleanup_and_puts_it_back_on_failure() {
    let d = tmp("swap");
    let app = d.join("Seance.app");
    let new_app = d.join("new/Seance.app");
    std::fs::create_dir_all(app.join("Contents")).unwrap();
    std::fs::write(app.join("Contents/v"), "old").unwrap();
    std::fs::create_dir_all(new_app.join("Contents")).unwrap();
    std::fs::write(new_app.join("Contents/v"), "new").unwrap();
    let backup = backup_path(&app);
    assert_eq!(backup.file_name().unwrap(), ".Seance.app.previous");

    swap(&app, &new_app, &backup).unwrap();
    assert_eq!(
        std::fs::read_to_string(app.join("Contents/v")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::read_to_string(backup.join("Contents/v")).unwrap(),
        "old"
    );

    // A swap whose new app is missing leaves the (new) app where it was.
    let r = swap(&app, &d.join("missing/Seance.app"), &backup);
    assert!(r.is_err());
    assert_eq!(
        std::fs::read_to_string(app.join("Contents/v")).unwrap(),
        "new"
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_bad_checksum_leaves_the_app_untouched() {
    let d = tmp("badsha");
    let app = d.join("Seance.app");
    std::fs::create_dir_all(app.join("Contents")).unwrap();
    std::fs::write(app.join("Contents/v"), "old").unwrap();
    let zip = d.join("Seance.zip");
    std::fs::write(&zip, b"not the build the feed named").unwrap();
    let feed = Feed {
        version: "0.28.3".into(),
        url: format!("file://{}", zip.display()),
        sha256: SHA.into(),
        notes: String::new(),
    };
    let err = apply(&feed, &app).unwrap_err().to_string();
    assert!(err.contains("checksum"), "{err}");
    assert_eq!(
        std::fs::read_to_string(app.join("Contents/v")).unwrap(),
        "old"
    );
    assert!(!backup_path(&app).exists());
    // No scratch folder left beside it.
    assert!(std::fs::read_dir(&d).unwrap().flatten().all(|e| !e
        .file_name()
        .to_string_lossy()
        .starts_with(".seance-update-")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_click_is_the_consent_for_a_short_while_and_only_once() {
    assert!(consent_is_fresh("1000", 1000));
    assert!(consent_is_fresh("1000", 1000 + 14 * 60));
    assert!(!consent_is_fresh("1000", 1000 + 16 * 60)); // stale: ask as usual
    assert!(!consent_is_fresh("1000", 999)); // from the future
    assert!(!consent_is_fresh("", 1000));
    assert!(!consent_is_fresh("soon", 1000));
}

fn a_feed() -> Feed {
    parse_feed(&feed_json("0.28.5", SHA)).unwrap()
}

#[test]
fn every_check_comes_to_one_of_four_outcomes_and_is_logged() {
    // Off here: personal, remote, or not an app. Never reads the feed.
    let o = outcome(false, "0.28.2", Ok(Some(a_feed())));
    assert!(matches!(o, Outcome::Off(_)));
    assert!(log_line(false, "file:///f.json", &o).contains("enabled=false"));
    assert!(log_line(false, "file:///f.json", &o).ends_with("result=off"));
    // Newer, same, and a feed that could not be read.
    let o = outcome(true, "0.28.2", Ok(Some(a_feed())));
    assert_eq!(o, Outcome::Ready(a_feed()));
    assert_eq!(
        log_line(true, "file:///f.json", &o),
        "[seance] update check: enabled=true, feed=file:///f.json, result=newer 0.28.5"
    );
    let o = outcome(true, "0.28.5", Ok(None));
    assert_eq!(o, Outcome::UpToDate("Seance 0.28.5 is up to date.".into()));
    assert!(log_line(true, "u", &o).ends_with("result=up to date"));
    let o = outcome(true, "0.28.2", Err(anyhow!("read /x: No such file")));
    assert!(matches!(&o, Outcome::Failed(m) if m.contains("No such file")));
    assert!(log_line(true, "u", &o).contains("result=failed: read /x"));
}

const LEAF: &str = "9F5FF690007AA1E110278DA31A4FC5B662556375";

fn dr(hash: &str) -> String {
    format!("designated => identifier \"xyz.ham.seance\" and certificate leaf = H\"{hash}\"")
}

#[test]
fn the_signing_identity_is_read_from_the_designated_requirement() {
    assert_eq!(signing_id(&dr(LEAF)), SigningId::Leaf(LEAF.to_lowercase()));
    assert_eq!(
        signing_id("# designated => cdhash H\"37509ce355b280d7a50d9847483dad76c6dd7939\""),
        SigningId::AdHoc
    );
    assert_eq!(signing_id(""), SigningId::Other);
    assert_eq!(signing_id("designated => anchor apple"), SigningId::Other);
}

#[test]
fn a_swap_must_keep_the_signing_identity_once_there_is_one() {
    let leaf = SigningId::Leaf(LEAF.to_lowercase());
    let other = SigningId::Leaf("0000000000000000000000000000000000000000".into());
    // The one-time move from ad-hoc to the certificate is allowed.
    assert!(identity_allows_swap(&SigningId::AdHoc, &leaf).is_ok());
    assert!(identity_allows_swap(&SigningId::AdHoc, &SigningId::AdHoc).is_ok());
    // Signed with the certificate: only the same certificate may replace it.
    assert!(identity_allows_swap(&leaf, &SigningId::Leaf(LEAF.to_lowercase())).is_ok());
    let e = identity_allows_swap(&leaf, &SigningId::AdHoc).unwrap_err();
    assert!(e.contains("not signed with Seance's certificate"), "{e}");
    let e = identity_allows_swap(&leaf, &other).unwrap_err();
    assert!(e.contains("different certificate"), "{e}");
    assert!(identity_allows_swap(&leaf, &SigningId::Other).is_err());
}
