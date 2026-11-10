use super::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct World {
    idle: Option<u64>,
    batt: Option<Battery>,
    sleep_disabled: bool,
    sudoers_ok: bool,
    sets: Vec<bool>,
    prompts: u32,
}

struct Fake(Rc<RefCell<World>>);

impl Sys for Fake {
    fn idle(&mut self) -> Option<u64> {
        self.0.borrow().idle
    }
    fn battery(&mut self) -> Battery {
        self.0.borrow().batt.unwrap_or(Battery {
            pct: Some(80),
            ac: true,
        })
    }
    fn set_sleep_disabled(&mut self, on: bool) -> bool {
        let mut w = self.0.borrow_mut();
        w.sets.push(on);
        w.sleep_disabled = on;
        true
    }
    fn sleep_disabled(&mut self) -> bool {
        self.0.borrow().sleep_disabled
    }
    fn ensure_sudoers(&mut self) -> bool {
        let mut w = self.0.borrow_mut();
        w.prompts += 1;
        w.sudoers_ok
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("afk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn machine(name: &str, sudoers_ok: bool) -> (Machine<Fake>, Rc<RefCell<World>>, PathBuf) {
    let w = Rc::new(RefCell::new(World {
        sudoers_ok,
        ..Default::default()
    }));
    let d = scratch(name);
    (Machine::start(Fake(w.clone()), d.clone()), w, d)
}

#[test]
fn turning_on_keeps_the_mac_awake_and_tells_the_leads() {
    let (mut m, w, d) = machine("on", true);
    assert_eq!(m.tick(100), vec![]);
    set_wanted(&d, true, 100);
    assert_eq!(m.tick(101), vec![Event::On]);
    assert_eq!(w.borrow().sets, vec![true]);
    let s = status(&d);
    assert!(s.on && s.mac_awake && s.since_ms == 100_000 && s.note.is_none());
    // Nothing more happens until the poll is due.
    assert_eq!(m.tick(102), vec![]);
}

#[test]
fn turning_off_lets_the_mac_sleep_again() {
    let (mut m, w, d) = machine("off", true);
    set_wanted(&d, true, 1);
    m.tick(2);
    set_wanted(&d, false, 3);
    assert_eq!(m.tick(4), vec![Event::Off(None)]);
    assert_eq!(w.borrow().sets, vec![true, false]);
    assert!(!status(&d).on && !status(&d).mac_awake);
    assert!(!we_set_file(&d).exists());
}

#[test]
fn a_cancelled_password_step_still_turns_afk_on_and_says_so() {
    let (mut m, w, d) = machine("cancel", false);
    set_wanted(&d, true, 1);
    assert_eq!(m.tick(2), vec![Event::On]);
    assert!(w.borrow().sets.is_empty(), "no pmset without the entry");
    let s = status(&d);
    assert!(s.on && !s.mac_awake);
    assert!(s.note.unwrap().contains("may sleep"));
    // Turning off then touches nothing: we never changed the setting.
    set_wanted(&d, false, 3);
    m.tick(4);
    assert!(w.borrow().sets.is_empty());
}

#[test]
fn coming_back_after_a_minute_away_ends_afk() {
    let (mut m, w, d) = machine("back", true);
    set_wanted(&d, true, 0);
    m.tick(0);
    w.borrow_mut().idle = Some(5); // still at the keyboard: no end
    assert_eq!(m.tick(15), vec![]);
    w.borrow_mut().idle = Some(75); // gone for over a minute
    assert_eq!(m.tick(30), vec![]);
    w.borrow_mut().idle = Some(2); // back
    assert_eq!(m.tick(45), vec![Event::Off(Some(Reason::Back))]);
    assert!(!status(&d).on);
    assert_eq!(w.borrow().sets, vec![true, false]);
    assert!(status(&d).note.unwrap().contains("keyboard"));
}

#[test]
fn a_low_unplugged_battery_ends_afk() {
    let (mut m, w, d) = machine("batt", true);
    set_wanted(&d, true, 0);
    m.tick(0);
    w.borrow_mut().batt = Some(Battery {
        pct: Some(14),
        ac: false,
    });
    assert_eq!(m.tick(15), vec![Event::Off(Some(Reason::LowBattery))]);
    assert!(!status(&d).on);
}

#[test]
fn the_rules_in_plain_readings() {
    let low = Battery {
        pct: Some(14),
        ac: false,
    };
    let plugged_low = Battery {
        pct: Some(14),
        ac: true,
    };
    let fine = Battery {
        pct: Some(15),
        ac: false,
    };
    let desk = Battery {
        pct: None,
        ac: true,
    };
    let mut a = AutoOff::default();
    assert_eq!(
        a.step(Some(3), fine),
        None,
        "at the keyboard from the start"
    );
    assert_eq!(a.step(Some(3), plugged_low), None, "plugged in is fine");
    assert_eq!(a.step(Some(3), desk), None);
    assert_eq!(a.step(None, fine), None, "no idle reading, no change");
    assert_eq!(a.step(Some(60), fine), None, "exactly a minute is away");
    assert_eq!(a.step(Some(59), fine), Some(Reason::Back));
    assert_eq!(
        AutoOff::default().step(Some(500), low),
        Some(Reason::LowBattery)
    );
}

#[test]
fn a_restart_while_on_stays_on_and_does_not_retell_the_leads() {
    let (_, _, d) = machine("restart", true);
    set_wanted(&d, true, 50);
    let w = Rc::new(RefCell::new(World {
        sudoers_ok: true,
        sleep_disabled: true,
        ..Default::default()
    }));
    let mut m = Machine::start(Fake(w.clone()), d.clone());
    assert_eq!(m.tick(60), vec![]);
    assert!(w.borrow().sets.is_empty());
}

#[test]
fn launch_puts_sleep_back_when_we_set_it_and_afk_is_off() {
    let d = scratch("restore");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(we_set_file(&d), "1").unwrap();
    let w = Rc::new(RefCell::new(World {
        sleep_disabled: true,
        ..Default::default()
    }));
    let _m = Machine::start(Fake(w.clone()), d.clone());
    assert_eq!(w.borrow().sets, vec![false]);
    assert!(!we_set_file(&d).exists());
}

#[test]
fn launch_leaves_a_sleep_setting_alone_that_was_not_ours() {
    let d = scratch("notours");
    std::fs::create_dir_all(&d).unwrap();
    let w = Rc::new(RefCell::new(World {
        sleep_disabled: true,
        ..Default::default()
    }));
    let _m = Machine::start(Fake(w.clone()), d);
    assert!(
        w.borrow().sets.is_empty(),
        "his own AFK setup stays as it is"
    );
}

#[test]
fn the_sudoers_file_allows_exactly_the_two_commands() {
    assert_eq!(
        sudoers_text("pat"),
        "# Seance AFK: lets pat keep the Mac awake with the lid closed, and nothing else.\n\
         pat ALL=(root) NOPASSWD: /usr/bin/pmset -a disablesleep 1, /usr/bin/pmset -a disablesleep 0\n"
    );
    let body = sudoers_text("pat");
    assert_eq!(body.matches("NOPASSWD").count(), 1);
    assert!(!body.contains("ALL ALL") && !body.contains(": ALL"));
    assert_eq!(SUDOERS_FILE, "/etc/sudoers.d/seance-afk");
}

#[test]
fn only_plain_login_names_reach_the_sudoers_line() {
    for ok in ["pat", "brian.h", "a_b-c", "user01"] {
        assert!(valid_user(ok), "{ok}");
    }
    for no in [
        "",
        "-x",
        "a b",
        "a;b",
        "a\nb",
        "a'b",
        "root ALL=(ALL) NOPASSWD: ALL",
    ] {
        assert!(!valid_user(no), "{no:?}");
    }
}

#[test]
fn the_admin_script_checks_before_it_installs() {
    let s = admin_shell(Path::new("/tmp/x"));
    assert_eq!(
        s,
        "/usr/sbin/visudo -cf '/tmp/x' && /usr/bin/install -m 0440 -o root -g wheel '/tmp/x' /etc/sudoers.d/seance-afk"
    );
    let a = applescript(&s);
    assert!(a.starts_with("do shell script \"/usr/sbin/visudo -cf '/tmp/x' &&"));
    assert!(a.contains("with administrator privileges"));
    assert_eq!(applescript("a\"b\\c").matches("a\\\"b\\\\c").count(), 1);
}

#[test]
fn readings_parse() {
    let on_battery = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t87%; discharging; 5:10 remaining present: true\n";
    assert_eq!(
        parse_batt(on_battery),
        Battery {
            pct: Some(87),
            ac: false
        }
    );
    let plugged =
        "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t9%; charging; 1:00 remaining\n";
    assert_eq!(
        parse_batt(plugged),
        Battery {
            pct: Some(9),
            ac: true
        }
    );
    assert_eq!(
        parse_batt("Now drawing from 'AC Power'\n"),
        Battery {
            pct: None,
            ac: true
        }
    );
    assert_eq!(
        parse_idle("    | |   \"HIDIdleTime\" = 61000000000\n"),
        Some(61)
    );
    assert_eq!(parse_idle("nothing"), None);
}
