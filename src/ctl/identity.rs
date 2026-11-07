use std::io;

const SHARED_SERVER_ERROR: &str = "cannot establish this pane's identity: a shared Codex app-server can inherit another pane's SEANCE_SESSION. Relaunch Codex in the intended pane with --no-daemon (or create a new pane with --agent codex). Do not override SEANCE_SESSION to guess.";

pub(super) fn validate(from: Option<&str>) -> Result<(), String> {
    let Some(from) = from else {
        return Ok(());
    };
    #[cfg(target_os = "linux")]
    let read_root = session;
    // macOS `ps -E` can't read a SIP-protected pane root's environment
    // (/bin/bash, /bin/zsh), so the daemon answers for the pids it spawned.
    #[cfg(not(target_os = "linux"))]
    let read_root = |pid| session_or_daemon(pid, session, pane_for_root_pid);
    validate_ancestry(from, std::process::id(), process, session, read_root)
}

#[cfg(not(target_os = "linux"))]
fn session_or_daemon(
    pid: u32,
    env: impl FnOnce(u32) -> io::Result<Option<String>>,
    daemon: impl FnOnce(u32) -> io::Result<Option<String>>,
) -> io::Result<Option<String>> {
    match env(pid) {
        Ok(Some(slug)) => Ok(Some(slug)),
        _ => daemon(pid),
    }
}

#[cfg(not(target_os = "linux"))]
fn pane_for_root_pid(pid: u32) -> io::Result<Option<String>> {
    let request = crate::control::ControlRequest::PaneForRootPid {
        pid,
        scope: None,
        from: None,
    };
    let response = super::send_request(&request).map_err(|e| match e {
        super::ConnectError::Connect(e) | super::ConnectError::Io(e) => e,
        super::ConnectError::Protocol(m) => io::Error::other(m),
    })?;
    if !response.ok {
        return Err(io::Error::other(
            response.error.unwrap_or_else(|| "daemon refused".into()),
        ));
    }
    Ok(response
        .data
        .as_ref()
        .and_then(|data| data.get("pane"))
        .and_then(|pane| pane.as_str())
        .map(str::to_string))
}

fn validate_ancestry(
    from: &str,
    mut pid: u32,
    mut read: impl FnMut(u32) -> io::Result<(u32, Vec<String>)>,
    mut read_session: impl FnMut(u32) -> io::Result<Option<String>>,
    // The daemon's own child. Only this branch may ask the daemon: an orphaned
    // chain can belong to a remote daemon (a tunnelled SEANCE_SOCKET), whose
    // pane pids mean nothing on this host.
    mut read_root: impl FnMut(u32) -> io::Result<Option<String>>,
) -> Result<(), String> {
    let mut child = pid;
    // Topmost ancestor whose environment carries SEANCE_SESSION: the pane's
    // root process. After a daemon upgrade the pane processes are reparented
    // to a subreaper (`systemd --user` here) whose environ can't be read —
    // 2026-09-29, every pre-upgrade pane's ctl failed with EACCES — so the
    // identity comes from the pane root, not from whatever sits above it.
    // Environments are only read once the whole chain has been checked for a
    // shared Codex server — nothing below it is trusted before that.
    let mut chain: Vec<u32> = Vec::new();
    for _ in 0..128 {
        if pid <= 1 {
            break;
        }
        let (parent, args) = read(pid)
            .map_err(|e| format!("cannot verify pane identity through process {pid}: {e}"))?;
        let executable = args
            .first()
            .and_then(|arg| std::path::Path::new(arg).file_name());
        if executable.is_some_and(|name| name == "codex")
            && args.get(1).is_some_and(|arg| arg == "app-server")
            && args.iter().any(|arg| {
                arg == "--managed-daemon" || arg == "--listen" || arg.starts_with("--listen=")
            })
        {
            return Err(SHARED_SERVER_ERROR.into());
        }
        // A test daemon or a daemon upgrade may itself have agent ancestors.
        if executable.is_some_and(|name| name == "seance")
            && args.get(1).is_some_and(|arg| arg == "daemon")
        {
            return match_session(from, read_root(child));
        }
        chain.push(pid);
        // PTY children survive upgrades, reparented to init (or a subreaper)
        // after the old daemon exits.
        if parent <= 1 {
            let pane_root = chain
                .iter()
                .rev()
                .copied()
                .find(|p| matches!(read_session(*p), Ok(Some(_))));
            return match_session(from, read_session(pane_root.unwrap_or(pid)));
        }
        if parent == pid {
            break;
        }
        child = pid;
        pid = parent;
    }
    Err("cannot verify pane identity: incomplete process ancestry".into())
}

fn match_session(from: &str, original: io::Result<Option<String>>) -> Result<(), String> {
    let original = original.map_err(|e| format!("cannot verify original pane identity: {e}"))?;
    if original.as_deref() != Some(from) {
        return Err(format!("pane identity mismatch: SEANCE_SESSION claims '{from}', but the pane process identifies as '{}'; relaunch the agent in its intended pane", original.as_deref().unwrap_or("unknown")));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn process(pid: u32) -> io::Result<(u32, Vec<String>)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let parent = stat
        .rsplit_once(") ")
        .and_then(|(_, rest)| rest.split_whitespace().nth(1))
        .and_then(|parent| parent.parse().ok())
        .ok_or_else(|| io::Error::other("invalid process stat"))?;
    let args = std::fs::read(format!("/proc/{pid}/cmdline"))?
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    Ok((parent, args))
}

#[cfg(not(target_os = "linux"))]
fn process(pid: u32) -> io::Result<(u32, Vec<String>)> {
    let output = std::process::Command::new("ps")
        .args([
            "-ww",
            "-o",
            "ppid=",
            "-o",
            "command=",
            "-p",
            &pid.to_string(),
        ])
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut words = text.split_whitespace();
    let parent = words
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| io::Error::other("cannot read process ancestry"))?;
    Ok((parent, words.map(str::to_string).collect()))
}

#[cfg(target_os = "linux")]
fn session(pid: u32) -> io::Result<Option<String>> {
    let environ = std::fs::read(format!("/proc/{pid}/environ"))?;
    Ok(environ
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(b"SEANCE_SESSION="))
        .map(|value| String::from_utf8_lossy(value).into_owned()))
}

#[cfg(not(target_os = "linux"))]
fn session(pid: u32) -> io::Result<Option<String>> {
    let output = std::process::Command::new("ps")
        .args(["-Eww", "-o", "command=", "-p", &pid.to_string()])
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find_map(|word| word.strip_prefix("SEANCE_SESSION="))
        .map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(
        from: &str,
        pid: u32,
        read: impl FnMut(u32) -> io::Result<(u32, Vec<String>)>,
        env: impl Fn(u32) -> io::Result<Option<String>> + Copy,
    ) -> Result<(), String> {
        validate_ancestry(from, pid, read, env, env)
    }

    #[test]
    fn only_the_daemons_child_is_read_through_the_daemon() {
        let unreadable = |_| Ok(None);
        let daemon = |_| Ok(Some("worker".into()));
        // bash(3) <- seance daemon(2): the daemon vouches for its child.
        let under_daemon = |pid| {
            Ok(match pid {
                3 => (2, vec!["/bin/bash".into()]),
                _ => (1, vec!["seance".into(), "daemon".into()]),
            })
        };
        assert!(validate_ancestry("worker", 3, under_daemon, unreadable, daemon).is_ok());
        // Orphaned bash(3) <- launchd: never asks the daemon.
        let orphan = |_| Ok((1, vec!["/bin/bash".into()]));
        let never = |_| -> io::Result<Option<String>> { panic!("orphans must not ask") };
        assert!(validate_ancestry("worker", 3, orphan, unreadable, never).is_err());
    }

    #[test]
    fn inherited_identity_from_shared_codex_is_rejected() {
        let result = check(
            "worker",
            3,
            |pid| {
                Ok(match pid {
                    3 => (
                        2,
                        vec!["bash".into(), "-lc".into(), "seance ctl finish".into()],
                    ),
                    _ => (
                        1,
                        [
                            "/opt/codex",
                            "app-server",
                            "--listen",
                            "unix://",
                            "--managed-daemon",
                        ]
                        .map(str::to_string)
                        .to_vec(),
                    ),
                })
            },
            |_| panic!("must reject the shared server before trusting env"),
        );
        assert!(result.unwrap_err().contains("--no-daemon"));
    }

    #[test]
    fn local_codex_and_pane_boundary_are_safe() {
        let result = check(
            "worker",
            3,
            |pid| {
                Ok(match pid {
                    3 => (2, vec!["/opt/codex".into(), "--no-daemon".into()]),
                    2 => (99, vec!["/opt/seance".into(), "daemon".into()]),
                    _ => panic!("must stop at the pane's daemon"),
                })
            },
            |pid| {
                assert_eq!(pid, 3);
                Ok(Some("worker".into()))
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn stale_snapshot_does_not_override_original_pane() {
        let result = check(
            "wrong-pane",
            3,
            |pid| {
                Ok(match pid {
                    3 => (2, vec!["codex".into(), "--no-daemon".into()]),
                    _ => (1, vec!["seance".into(), "daemon".into()]),
                })
            },
            |_| Ok(Some("actual-pane".into())),
        );
        assert!(result.unwrap_err().contains("identity mismatch"));
        assert!(check("worker", 2, |_| Ok((1, vec!["bash".into()])), |_| Ok(None)).is_err());
    }

    #[test]
    fn daemon_upgrade_reparenting_preserves_identity() {
        assert!(check(
            "worker",
            3,
            |_| Ok((1, vec!["codex".into(), "--no-daemon".into()])),
            |pid| {
                assert_eq!(pid, 3);
                Ok(Some("worker".into()))
            },
        )
        .is_ok());
    }

    #[test]
    fn subreaper_above_the_pane_after_upgrade_is_skipped() {
        // ctl(5) <- claude(4, the pane root) <- systemd --user(3, unreadable) <- init
        let tree = |pid: u32| -> io::Result<(u32, Vec<String>)> {
            Ok(match pid {
                5 => (4, vec!["seance".into(), "ctl".into()]),
                4 => (3, vec!["claude".into()]),
                3 => (1, vec!["/usr/lib/systemd/systemd".into(), "--user".into()]),
                _ => unreachable!(),
            })
        };
        let env = |pid: u32| -> io::Result<Option<String>> {
            match pid {
                3 => Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                _ => Ok(Some("worker".into())),
            }
        };
        assert!(check("worker", 5, tree, env).is_ok());
        // Still checked: the pane root says who the pane is.
        assert!(check("other", 5, tree, env).is_err());
        // No readable identity anywhere still fails closed.
        let none = |pid: u32| -> io::Result<Option<String>> {
            if pid == 3 {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                Ok(None)
            }
        };
        assert!(check("worker", 5, tree, none).is_err());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn daemon_answers_only_when_the_environment_cannot() {
        let readable = session_or_daemon(
            7,
            |_| Ok(Some("env-pane".into())),
            |_| panic!("readable environment must not ask the daemon"),
        );
        assert_eq!(readable.unwrap().as_deref(), Some("env-pane"));
        for env in [Ok(None), Err(io::Error::other("SIP"))] {
            let asked = session_or_daemon(
                7,
                |_| env,
                |pid| {
                    assert_eq!(pid, 7);
                    Ok(Some("daemon-pane".into()))
                },
            );
            assert_eq!(asked.unwrap().as_deref(), Some("daemon-pane"));
        }
    }

    #[test]
    fn unreadable_identity_fails_closed() {
        assert!(check("worker", 3, |_| Err(io::Error::other("gone")), |_| Ok(None)).is_err());
    }
}
