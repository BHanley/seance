//! Seance updates itself with one click.
//!
//! The app asks a small JSON feed (`{version, url, sha256, notes}`) whether a
//! newer build exists, when it opens and every hour. A newer one shows an
//! "Update ready" bar; the button downloads the zip with `curl` (so nothing is
//! quarantined), checks its sha256 against the feed, unpacks it beside the
//! app, swaps the new app into the same path (the old one kept as a hidden
//! backup until the new one has started), and relaunches. The relaunched app
//! finds its sessions on an older daemon and restarts them through the usual
//! path; the click was the consent, so it does not ask again (`consent`).
//!
//! Standalone Mac build only: Brian's own Seance (`seance-personal`) and thin
//! clients never check or update. Every failure leaves the old app as it was.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context as _, Result};
use serde::Deserialize;

/// Where the feed is unless `seance-update-feed` says otherwise.
pub const DEFAULT_FEED: &str = "file:///Users/Shared/Seance-test/latest.json";
/// How often the app looks again.
pub const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// A click's consent for the sessions restart is good for this long.
const CONSENT_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Feed {
    pub version: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub notes: String,
}

pub fn parse_feed(text: &str) -> Result<Feed> {
    let feed: Feed = serde_json::from_str(text).context("the update feed is not valid")?;
    if crate::update::parse_version(&feed.version).is_none() {
        bail!(
            "the update feed has a version I can't read: {}",
            feed.version
        );
    }
    if feed.sha256.trim().len() != 64 {
        bail!("the update feed has no usable sha256");
    }
    Ok(feed)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Newer,
    Same,
    Older,
}

/// The feed's version against the running one.
pub fn compare(current: &str, feed: &str) -> Verdict {
    if crate::update::is_older(current, feed) {
        Verdict::Newer
    } else if crate::update::is_older(feed, current) {
        Verdict::Older
    } else {
        Verdict::Same
    }
}

/// Does this app check and update at all? Only the standalone Mac build, from
/// an app bundle.
pub fn enabled(macos: bool, personal: bool, remote: bool, in_bundle: bool) -> bool {
    macos && !personal && !remote && in_bundle
}

/// [`enabled`] for this process.
pub fn enabled_here() -> bool {
    enabled(
        cfg!(target_os = "macos"),
        crate::term_config::personal(),
        crate::ACTIVE_TUNNEL.get().is_some(),
        app_path().is_some(),
    )
}

/// The feed to ask: the user's setting, else the build default.
pub fn feed_url() -> String {
    crate::term_config::get()
        .seance_update_feed
        .clone()
        .unwrap_or_else(|| DEFAULT_FEED.to_string())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Https(String),
}

/// `file://` and `https://` are supported; anything else is refused (a plain
/// http feed could be swapped on the way).
pub fn source_of(url: &str) -> Result<Source> {
    if let Some(p) = url.strip_prefix("file://") {
        Ok(Source::File(PathBuf::from(p)))
    } else if url.starts_with("https://") {
        Ok(Source::Https(url.to_string()))
    } else {
        Err(anyhow!(
            "update address must start with https:// or file://"
        ))
    }
}

pub fn sha_matches(expected: &str, actual: &str) -> bool {
    expected.trim().eq_ignore_ascii_case(actual.trim())
}

/// The `.app` this process runs from, or none (a dev build).
pub fn app_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension()? == "app" && crate::daemon::local_copy::in_bundle(&exe))
        .then(|| app.to_path_buf())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Location {
    Ok,
    /// Running from the disk image or a translocated copy: read-only.
    FromImage,
    /// The folder holding the app can't be written.
    NotWritable,
}

/// Where the app lives, and whether it can be replaced there.
pub fn classify_location(app: &Path, parent_writable: bool) -> Location {
    let p = app.to_string_lossy();
    if p.starts_with("/Volumes/") || p.contains("/AppTranslocation/") {
        Location::FromImage
    } else if !parent_writable {
        Location::NotWritable
    } else {
        Location::Ok
    }
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".seance-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// What to tell the person when the app can't be replaced where it is.
pub fn location_message(loc: &Location) -> Option<&'static str> {
    match loc {
        Location::Ok => None,
        Location::FromImage => {
            Some("Drag Seance to your Applications folder first, then update from there.")
        }
        Location::NotWritable => Some(
            "Seance can't replace itself here. Move Seance to Applications and open it from there.",
        ),
    }
}

// ---- fetching ---------------------------------------------------------------

fn curl(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "900"])
        .args(args)
        .output()
        .context("could not run curl")?;
    if !out.status.success() {
        bail!(
            "download failed ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// The feed's text.
pub fn fetch_feed(url: &str) -> Result<String> {
    let bytes = match source_of(url)? {
        Source::File(p) => std::fs::read(&p).with_context(|| format!("read {}", p.display()))?,
        Source::Https(u) => curl(&["--max-time", "30", &u])?,
    };
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Download the zip to `dest`. `curl` writes it, so no quarantine mark.
pub fn fetch_zip(url: &str, dest: &Path) -> Result<()> {
    match source_of(url)? {
        Source::File(p) => {
            std::fs::copy(&p, dest).with_context(|| format!("copy {}", p.display()))?;
        }
        Source::Https(u) => {
            curl(&["-o", &dest.to_string_lossy(), &u])?;
        }
    }
    Ok(())
}

pub fn sha256_of(path: &Path) -> Result<String> {
    let out = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .context("could not run shasum")?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("could not read the download's checksum"))
}

// ---- swapping -----------------------------------------------------------------

/// The hidden sibling that holds the old app until the new one has started.
pub fn backup_path(app: &Path) -> PathBuf {
    let name = app
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Seance.app".into());
    app.with_file_name(format!(".{name}.previous"))
}

/// Put `new_app` where `app` is, keeping `app` as `backup`. If the second move
/// fails the old app goes back, so there is always a working app at the path.
pub fn swap(app: &Path, new_app: &Path, backup: &Path) -> Result<()> {
    if backup.exists() {
        std::fs::remove_dir_all(backup).context("clear the old backup")?;
    }
    std::fs::rename(app, backup).context("move the current app aside")?;
    if let Err(e) = std::fs::rename(new_app, app) {
        let _ = std::fs::rename(backup, app);
        return Err(anyhow!(e)).context("put the new app in place (the old one is back)");
    }
    Ok(())
}

/// Once the new app is up, the backup is not needed.
pub fn cleanup_backup() {
    let Some(app) = app_path() else { return };
    let backup = backup_path(&app);
    if backup.is_dir() {
        let _ = std::fs::remove_dir_all(backup);
    }
}

/// Who signed a build, as far as the update cares.
#[derive(Debug, PartialEq, Eq)]
pub enum SigningId {
    /// Ad-hoc (or unsigned): its identity is its own hash, new every build.
    AdHoc,
    /// The release certificate, by the SHA-1 of its leaf certificate.
    Leaf(String),
    /// Signed some other way.
    Other,
}

/// The identity in a designated requirement (`codesign -d -r-`). The release
/// build writes `designated => identifier "…" and certificate leaf = H"<sha1>"`.
pub fn signing_id(requirement: &str) -> SigningId {
    if let Some(i) = requirement.find("certificate leaf = H\"") {
        let rest = &requirement[i + "certificate leaf = H\"".len()..];
        if let Some(hash) = rest.split('"').next().filter(|h| !h.is_empty()) {
            return SigningId::Leaf(hash.to_ascii_lowercase());
        }
    }
    if requirement.contains("designated => cdhash H\"") {
        return SigningId::AdHoc;
    }
    SigningId::Other
}

/// May `new` replace `running`? While the running app is only ad-hoc signed any
/// build may (the one-time move to the release certificate). Once it carries the
/// certificate, only a build with the same certificate may: a swap to anything
/// else would lose every permission grant, or be someone else's app.
pub fn identity_allows_swap(running: &SigningId, new: &SigningId) -> Result<(), String> {
    match (running, new) {
        (SigningId::AdHoc | SigningId::Other, _) => Ok(()),
        (SigningId::Leaf(a), SigningId::Leaf(b)) if a == b => Ok(()),
        (SigningId::Leaf(_), SigningId::AdHoc) => Err(
            "the new build is not signed with Seance's certificate, so it was not installed".into(),
        ),
        (SigningId::Leaf(_), _) => Err(
            "the new build is signed by a different certificate, so it was not installed".into(),
        ),
    }
}

fn requirement_of(app: &Path) -> String {
    Command::new("codesign")
        .args(["-d", "-r-"])
        .arg(app)
        .output()
        .map(|o| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        })
        .unwrap_or_default()
}

fn plist_version(app: &Path) -> Option<String> {
    let out = Command::new("plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Download, verify and swap in `feed`'s build. On any failure the old app is
/// untouched (or put back). Does not relaunch.
pub fn apply(feed: &Feed, app: &Path) -> Result<()> {
    let parent = app
        .parent()
        .ok_or_else(|| anyhow!("no folder for the app"))?;
    if let Some(msg) = location_message(&classify_location(app, writable(parent))) {
        bail!("{msg}");
    }
    // Work beside the app, so the final move stays on one volume.
    let work = parent.join(format!(".seance-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).context("make a place to unpack")?;
    let result = apply_in(feed, app, &work);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn apply_in(feed: &Feed, app: &Path, work: &Path) -> Result<()> {
    let zip = work.join("update.zip");
    fetch_zip(&feed.url, &zip)?;
    let got = sha256_of(&zip)?;
    if !sha_matches(&feed.sha256, &got) {
        bail!(
            "the download does not match the checksum in the update feed, so it was not installed"
        );
    }
    let unpacked = work.join("unpacked");
    let ok = Command::new("ditto")
        .args(["-x", "-k"])
        .arg(&zip)
        .arg(&unpacked)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        bail!("could not unpack the download");
    }
    let new_app = unpacked.join(app.file_name().ok_or_else(|| anyhow!("no app name"))?);
    if !new_app.is_dir() {
        bail!("the download does not contain the app");
    }
    // Nothing from the internet should carry a quarantine mark, but a feed
    // behind a browser-like proxy might: the app was already opened by the person.
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&new_app)
        .output();
    if plist_version(&new_app).as_deref() != Some(feed.version.as_str()) {
        bail!("the download is not version {}", feed.version);
    }
    let signed = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&new_app)
        .output()
        .is_ok_and(|o| o.status.success());
    if !signed {
        bail!("the new app's signature does not check out, so it was not installed");
    }
    // Same signing identity as the app being replaced (once it has one).
    if let Err(why) = identity_allows_swap(
        &signing_id(&requirement_of(app)),
        &signing_id(&requirement_of(&new_app)),
    ) {
        bail!("{why}");
    }
    swap(app, &new_app, &backup_path(app))
}

// ---- the click is the consent -------------------------------------------------------

fn consent_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support/Seance/update-consent"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Remember that the person pressed Update, so the relaunched app restarts the
/// sessions without asking again.
pub fn grant_consent() {
    if let Some(p) = consent_path() {
        let _ = std::fs::write(p, now_secs().to_string());
    }
}

/// Is `stamp` (the file's text) a fresh consent at `now`?
pub fn consent_is_fresh(stamp: &str, now: u64) -> bool {
    stamp
        .trim()
        .parse::<u64>()
        .is_ok_and(|t| now >= t && now - t <= CONSENT_TTL.as_secs())
}

/// True once, right after an update the person started: the sessions restart
/// without a dialog. Always consumes the mark.
pub fn take_consent() -> bool {
    let Some(p) = consent_path() else {
        return false;
    };
    let stamp = std::fs::read_to_string(&p).unwrap_or_default();
    let _ = std::fs::remove_file(&p);
    consent_is_fresh(&stamp, now_secs())
}

// ---- what a check found ---------------------------------------------------------------

/// Ask the feed. `Ok(Some(feed))` only when it names a newer version.
pub fn check(current: &str, url: &str) -> Result<Option<Feed>> {
    let feed = parse_feed(&fetch_feed(url)?)?;
    Ok((compare(current, &feed.version) == Verdict::Newer).then_some(feed))
}

/// What a check came to, in the words the person (and the log) gets.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Updating is off here: say why.
    Off(String),
    UpToDate(String),
    Ready(Feed),
    Failed(String),
}

/// Pure: a check's result as an outcome.
pub fn outcome(enabled: bool, current: &str, result: Result<Option<Feed>>) -> Outcome {
    if !enabled {
        return Outcome::Off(
            "Updates are off here (this is a personal or remote Seance, or not an installed app)."
                .into(),
        );
    }
    match result {
        Ok(Some(feed)) => Outcome::Ready(feed),
        Ok(None) => Outcome::UpToDate(format!("Seance {current} is up to date.")),
        Err(e) => Outcome::Failed(format!("{e:#}")),
    }
}

/// The line every check writes to gui.stderr.log.
pub fn log_line(enabled: bool, feed: &str, o: &Outcome) -> String {
    let result = match o {
        Outcome::Off(_) => "off".to_string(),
        Outcome::UpToDate(_) => "up to date".to_string(),
        Outcome::Ready(f) => format!("newer {}", f.version),
        Outcome::Failed(e) => format!("failed: {e}"),
    };
    format!("[seance] update check: enabled={enabled}, feed={feed}, result={result}")
}

/// Run a check now, logging it. Blocking: run it off the UI thread.
pub fn check_now() -> Outcome {
    let on = enabled_here();
    let url = feed_url();
    let current = env!("CARGO_PKG_VERSION");
    let o = outcome(
        on,
        current,
        if on { check(current, &url) } else { Ok(None) },
    );
    eprintln!("{}", log_line(on, &url, &o));
    o
}

/// A native dialog (it does not depend on the window's own layout): the text
/// and one or two buttons; returns the one pressed. Blocking.
#[cfg(target_os = "macos")]
pub fn dialog(text: &str, buttons: &[&str]) -> Option<String> {
    let q = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
    let list = buttons.iter().map(|b| q(b)).collect::<Vec<_>>().join(", ");
    let last = q(buttons.last()?);
    let script = format!(
        "display dialog {} with title \"Seance\" buttons {{{list}}} default button {last}",
        q(text)
    );
    let out = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
        .and_then(|s| {
            buttons
                .iter()
                .find(|b| s.contains(&format!("button returned:{b}")))
                .map(|b| b.to_string())
        })
}

#[cfg(not(target_os = "macos"))]
pub fn dialog(_text: &str, _buttons: &[&str]) -> Option<String> {
    None
}

#[cfg(test)]
mod tests;
