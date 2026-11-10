//! The local daemon on a Mac runs from its own copy of the binary, not from
//! inside Seance.app. A daemon that runs from `Seance.app/Contents/MacOS`
//! keeps the bundle "in use", so Finder refuses to replace the app when you
//! drag a new one over it. The copy lives in
//! `~/Library/Application Support/Seance/bin/seance-<version>`; the app only
//! reads its own binary at launch.
//!
//! Only the current and the previous copy are kept. An older copy goes once
//! no process runs from it.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

const PREFIX: &str = "seance-";
/// The copy for this version, and the one before it (a daemon from the old
/// version may still be running until sessions are restarted).
const KEEP: usize = 2;

/// File name of the copy for `version`.
pub fn copy_name(version: &str) -> String {
    format!("{PREFIX}{version}")
}

/// Is `exe` the main binary of a `.app` bundle?
pub fn in_bundle(exe: &Path) -> bool {
    exe.to_string_lossy().contains(".app/Contents/MacOS/")
}

/// The version a copy's file name carries; none for anything else in the
/// folder (temp files, strays), which are never pruned.
fn version_of(name: &str) -> Option<&str> {
    let v = name.strip_prefix(PREFIX)?;
    crate::update::parse_version(v).map(|_| v)
}

/// Which of `names` (the folder's files) to remove: everything but `current`
/// and the newest other version.
pub fn prune_plan(names: &[String], current: &str) -> Vec<String> {
    let mut others: Vec<(Vec<u64>, &String)> = names
        .iter()
        .filter_map(|n| {
            let v = version_of(n)?;
            (v != current).then(|| (crate::update::parse_version(v).unwrap_or_default(), n))
        })
        .collect();
    others.sort_by(|a, b| b.0.cmp(&a.0));
    others
        .into_iter()
        .skip(KEEP - 1)
        .map(|(_, n)| n.clone())
        .collect()
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    if fa.metadata().map(|m| m.len()).ok() != fb.metadata().map(|m| m.len()).ok() {
        return false;
    }
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let (Ok(na), Ok(nb)) = (ra.read(&mut ba), rb.read(&mut bb)) else {
            return false;
        };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

/// Copy `src` to `dest` through a temp name and a rename, so a launch never
/// sees half a binary and a daemon already running `dest` keeps its inode.
fn install_copy(src: &Path, dest: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = dest.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::copy(src, &tmp).with_context(|| format!("copy to {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    sign_if_needed(src, &tmp);
    std::fs::rename(&tmp, dest).with_context(|| format!("rename to {}", dest.display()))?;
    Ok(())
}

/// What to do about a copy: sign it ad-hoc again, or leave it as it is. Only
/// an ad-hoc source whose copy no longer verifies is re-signed. A source with a
/// real signature (the release's own certificate) is never downgraded to
/// ad-hoc: that would hand macOS a different identity for the daemon and its
/// permission grants would be asked for again.
pub fn needs_adhoc_sign(copy_verifies: bool, source_is_adhoc: bool) -> bool {
    !copy_verifies && source_is_adhoc
}

/// Is this Mach-O ad-hoc signed (or not signed at all)?
fn is_adhoc(path: &Path) -> bool {
    let out = std::process::Command::new("codesign")
        .arg("-dv")
        .arg(path)
        .output();
    let Ok(o) = out else { return true };
    let text = String::from_utf8_lossy(&o.stderr);
    text.contains("Signature=adhoc") || text.contains("not signed at all")
}

/// A copied Mach-O keeps its embedded signature. When the copy no longer
/// verifies and the source was only ad-hoc signed, sign it again (ad-hoc is
/// enough for a process the app starts); see [`needs_adhoc_sign`].
fn sign_if_needed(src: &Path, path: &Path) {
    let ok = std::process::Command::new("codesign")
        .arg("-v")
        .arg(path)
        .output()
        .is_ok_and(|o| o.status.success());
    if needs_adhoc_sign(ok, is_adhoc(src)) {
        let _ = std::process::Command::new("codesign")
            .args(["--force", "-s", "-"])
            .arg(path)
            .output();
    } else if !ok {
        eprintln!(
            "[seance] the daemon copy {} does not verify, but it carries the release's signature, so it is left as it is",
            path.display()
        );
    }
}

/// The file the daemon copy is made from. A signed release carries the daemon
/// as `Contents/Resources/seance-daemon`, signed on its own (a bundle's main
/// executable is bound to the bundle's Info.plist, so a loose copy of it does
/// not verify). Without that file (ad-hoc and dev builds) it is the running
/// binary, as before.
pub fn daemon_source(exe: &Path) -> PathBuf {
    exe.parent()
        .and_then(Path::parent)
        .map(|contents| contents.join("Resources/seance-daemon"))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| exe.to_path_buf())
}

/// Does any process run from `path`?
fn in_use(path: &Path) -> bool {
    let p = path.to_string_lossy();
    std::process::Command::new("ps")
        .args(["-axo", "command="])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .any(|l| l == p || l.starts_with(&format!("{p} ")))
        })
        .unwrap_or(true)
}

/// `~/Library/Application Support/Seance/cli`: holds a `seance` link to the
/// current daemon copy, put ahead of PATH for panes (and Invoke), because a
/// drag-installed app has no `seance` on PATH and Claude is told to run
/// `seance ctl finish`.
pub fn cli_dir() -> Option<PathBuf> {
    Some(bin_dir()?.parent()?.join("cli"))
}

fn link_cli(dest: &Path) {
    let Some(dir) = cli_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let link = dir.join("seance");
    let _ = std::fs::remove_file(&link);
    let _ = std::os::unix::fs::symlink(dest, &link);
}

fn bin_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
    Some(PathBuf::from(home).join("Library/Application Support/Seance/bin"))
}

/// The binary the local daemon runs: this build's copy under Application
/// Support when the app runs from a bundle on a Mac, else the running binary
/// itself (dev builds, Linux). Any trouble making the copy falls back to the
/// running binary, so the daemon still starts.
pub fn local_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    if !cfg!(target_os = "macos") || !in_bundle(&exe) {
        return Ok(exe);
    }
    // Invoke (the phone chat) is carried inside the app: unpack it beside the
    // daemon copy so the app itself is never what runs.
    if !crate::term_config::personal() {
        crate::invoke::stage_from_bundle(&exe);
    }
    match ensure_copy(&exe) {
        Ok(copy) => Ok(copy),
        Err(e) => {
            eprintln!("[seance] could not make the daemon copy, running from the app: {e:#}");
            Ok(exe)
        }
    }
}

fn ensure_copy(exe: &Path) -> Result<PathBuf> {
    let dir = bin_dir().context("no home folder")?;
    std::fs::create_dir_all(&dir)?;
    let version = env!("CARGO_PKG_VERSION");
    let dest = dir.join(copy_name(version));
    // A same-version rebuild differs byte for byte, so the name alone is not
    // enough to reuse a copy.
    let src = daemon_source(exe);
    if !(dest.is_file() && same_bytes(&src, &dest)) {
        install_copy(&src, &dest)?;
    }
    if !crate::term_config::personal() {
        link_cli(&dest);
    }
    let names: Vec<String> = std::fs::read_dir(&dir)?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    for old in prune_plan(&names, version) {
        let path = dir.join(old);
        if !in_use(&path) {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_real_signature_is_never_downgraded_to_ad_hoc() {
        // (copy verifies, source is ad-hoc)
        assert!(!needs_adhoc_sign(true, true)); // fine as it is
        assert!(!needs_adhoc_sign(true, false));
        // The old behavior for ad-hoc builds: a copy that no longer verifies is signed again.
        assert!(needs_adhoc_sign(false, true));
        // The release's own certificate: never replaced by ad-hoc, verified or not.
        assert!(!needs_adhoc_sign(false, false));
    }

    #[test]
    fn the_daemon_is_copied_from_the_signed_standalone_file_when_the_app_has_one() {
        let d = std::env::temp_dir().join(format!("seance-daemon-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let macos = d.join("Seance.app/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::create_dir_all(d.join("Seance.app/Contents/Resources")).unwrap();
        let exe = macos.join("seance");
        std::fs::write(&exe, b"main").unwrap();
        // No standalone file (ad-hoc and dev builds): the running binary.
        assert_eq!(daemon_source(&exe), exe);
        let standalone = d.join("Seance.app/Contents/Resources/seance-daemon");
        std::fs::write(&standalone, b"daemon").unwrap();
        assert_eq!(daemon_source(&exe), standalone);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_copy_is_named_for_its_version() {
        assert_eq!(copy_name("0.27.3"), "seance-0.27.3");
    }

    #[test]
    fn only_a_bundle_binary_gets_a_copy() {
        assert!(in_bundle(Path::new(
            "/Applications/Seance.app/Contents/MacOS/seance"
        )));
        assert!(in_bundle(Path::new(
            "/Users/a/Seance 2.app/Contents/MacOS/seance"
        )));
        assert!(!in_bundle(Path::new("/Users/a/work/target/release/seance")));
    }

    #[test]
    fn current_and_previous_stay_and_older_ones_go() {
        let all = names(&[
            "seance-0.27.1",
            "seance-0.27.3",
            "seance-0.26.9",
            "seance-0.27.2",
        ]);
        let mut gone = prune_plan(&all, "0.27.3");
        gone.sort();
        assert_eq!(gone, ["seance-0.26.9", "seance-0.27.1"]);
        // Ordering is by number, not text: 0.9.x is older than 0.27.
        let all = names(&["seance-0.9.5", "seance-0.27.3", "seance-0.27.2"]);
        assert_eq!(prune_plan(&all, "0.27.3"), ["seance-0.9.5"]);
    }

    #[test]
    fn a_newer_copy_than_this_build_counts_as_the_other_one_kept() {
        // The user dragged an older app back in: the newer copy stays.
        let all = names(&["seance-0.27.3", "seance-0.27.2", "seance-0.27.1"]);
        assert_eq!(prune_plan(&all, "0.27.2"), ["seance-0.27.1"]);
    }

    #[test]
    fn strays_in_the_folder_are_left_alone() {
        let all = names(&[
            "seance-0.27.3",
            "seance-0.27.3.tmp-123",
            "notes.txt",
            "seance-x",
        ]);
        assert!(prune_plan(&all, "0.27.3").is_empty());
    }

    #[test]
    fn a_copy_is_installed_whole_and_a_changed_build_is_noticed() {
        let dir = std::env::temp_dir().join(format!("seance-copy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dest) = (dir.join("src"), dir.join("seance-9.9.9"));
        std::fs::write(&src, b"build one").unwrap();
        install_copy(&src, &dest).unwrap();
        assert!(same_bytes(&src, &dest));
        // No temp file left behind.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::write(&src, b"build two").unwrap();
        assert!(!same_bytes(&src, &dest));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
