//! Engine-level tests for how profiles interact with the rest of the engine: the older default
//! lists, Restraint, game mode and the shutdown path. Uses the simulated OS throughout.

use crate::config::{Config, ABOVE_NORMAL, BELOW_NORMAL, HIGH, IDLE, NORMAL};
use crate::engine::Engine;
use crate::log::{id, Event, Sink};
use crate::os::Os;
use crate::simos::SimOs;
use crate::topology::{fake_9950x3d, fake_quad};

#[derive(Default)]
struct Rec(Vec<Event>);
impl Sink for Rec {
    fn event(&mut self, e: &Event) {
        self.0.push(e.clone());
    }
}
impl Rec {
    fn ids(&self, i: u32) -> Vec<&Event> {
        self.0.iter().filter(|e| e.id == i).collect()
    }
}

fn cfg(text: &str) -> Config {
    Config::from_bytes(text.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

struct Rig {
    os: SimOs,
    eng: Engine,
    log: Rec,
    t: u64,
}

impl Rig {
    fn new(os: SimOs) -> Rig {
        Rig { os, eng: Engine::new(1), log: Rec::default(), t: 0 }
    }
    fn tick(&mut self, c: &Config) {
        self.t += 1000;
        self.eng.tick(&mut self.os, c, &mut self.log, self.t);
    }
    fn ticks(&mut self, c: &Config, n: u32) {
        for _ in 0..n {
            self.tick(c);
        }
    }
}

/// CPU time per 1s sample that makes a process use `pct`% of a 4-CPU machine.
fn burn(pct: f64) -> u64 {
    (pct / 100.0 * 4.0 * 1000.0 * 10_000.0) as u64
}

const PB: &str = "[OutOfControlProcessRestraint]\nTameOnlyNormal=false\nMinimumTimeOfRestraint=3000\n";

fn hot_rig() -> Rig {
    let mut os = SimOs::new(fake_quad());
    os.sys_pct = 90.0;
    os.add(10, "game.exe").cpu_per_sample = burn(60.0);
    os.add(11, "other.exe").cpu_per_sample = burn(0.0);
    Rig::new(os)
}

// ---- profile wins over the older lists -------------------------------------------------------

#[test]
fn a_profile_beats_default_priorities_default_affinities_and_disallowed_and_says_so_once() {
    let lists = "[ProcessDefaults]\nDefaultPriorities=game.exe:idle\nDefaultAffinities=game.exe:0x1\n[ProcessAllowances]\nDisallowedProcesses=game.exe\n";
    let with_profile = cfg(&format!("{lists}[Profile.G]\nMatch=game.exe\nPriority=high\n"));
    let mut r = Rig::new(SimOs::new(fake_quad()));
    r.os.add(10, "game.exe");
    r.ticks(&with_profile, 5);
    assert!(r.os.killed.is_empty(), "DisallowedProcesses must not kill a process a profile claims");
    assert_eq!(r.os.procs[&10].priority, HIGH, "profile priority, not DefaultPriorities");
    assert_eq!(r.os.procs[&10].affinity.mask, 0xF, "DefaultAffinities not applied");
    let notes = r.log.ids(id::PROFILE_OVERRIDES);
    assert_eq!(notes.len(), 1, "logged once");
    for list in ["DefaultPriorities", "DefaultAffinities", "DisallowedProcesses"] {
        assert!(notes[0].info.contains(list), "{}", notes[0].info);
    }
    assert!(notes[0].info.contains("profile 'G'"));

    // Without the profile the old lists behave exactly as before.
    let mut old = Rig::new(SimOs::new(fake_quad()));
    old.os.add(10, "game.exe");
    old.ticks(&cfg(lists), 2);
    assert_eq!(old.os.killed, vec![10]);
    let defaults_only = "[ProcessDefaults]\nDefaultPriorities=game.exe:idle\nDefaultAffinities=game.exe:0x1\n";
    let mut old = Rig::new(SimOs::new(fake_quad()));
    old.os.add(10, "game.exe");
    old.ticks(&cfg(defaults_only), 2);
    assert_eq!((old.os.procs[&10].priority, old.os.procs[&10].affinity.mask), (IDLE, 1));
    assert!(old.log.ids(id::PROFILE_OVERRIDES).is_empty());
}

#[test]
fn a_profile_matching_a_different_process_does_not_affect_the_lists() {
    let c = cfg("[ProcessDefaults]\nDefaultPriorities=game.exe:idle\n[Profile.X]\nMatch=other.exe\nPriority=high\n");
    let mut r = Rig::new(SimOs::new(fake_quad()));
    r.os.add(10, "game.exe");
    r.ticks(&c, 2);
    assert_eq!(r.os.procs[&10].priority, IDLE);
}

// ---- Restraint ------------------------------------------------------------------------------

#[test]
fn without_a_profile_the_hog_is_restrained() {
    let mut r = hot_rig();
    r.ticks(&cfg(PB), 4);
    assert_eq!(r.os.procs[&10].priority, BELOW_NORMAL);
}

/// ExcludeFromRestraint=true keeps the program out of Restraint; =false behaves as without the key.
#[test]
fn exclude_from_restraint_means_never_restrained_and_the_log_names_the_profile() {
    for key in ["ExcludeFromRestraint"] {
        let c = cfg(&format!("{PB}[Profile.G]\nMatch=game.exe\n{key}=true\n"));
        let mut r = hot_rig();
        r.ticks(&c, 8);
        assert_eq!(r.os.procs[&10].priority, NORMAL, "{key}");
        assert!(r.log.ids(id::PB_BEGIN).is_empty(), "{key}");
        let ex = r.log.ids(id::PB_EXEMPT);
        assert_eq!(ex.len(), 1, "logged once, not every tick ({key})");
        assert!(ex[0].info.contains("profile 'G' is set to never restrain this program"), "{}", ex[0].info);
        assert_eq!(r.os.count("prio"), 0, "the profile itself changed nothing ({key})");
    }
    // =false leaves the program restrainable, as without the key
    for key in ["ExcludeFromRestraint"] {
        let mut r = hot_rig();
        r.ticks(&cfg(&format!("{PB}[Profile.G]\nMatch=game.exe\n{key}=false\n")), 4);
        assert_eq!(r.os.procs[&10].priority, BELOW_NORMAL, "{key}=false");
    }
}

#[test]
fn a_process_already_restrained_when_a_profile_excludes_it_is_released() {
    let mut r = hot_rig();
    r.ticks(&cfg(PB), 4);
    assert_eq!(r.os.procs[&10].priority, BELOW_NORMAL);
    r.tick(&cfg(&format!("{PB}[Profile.G]\nMatch=game.exe\nExcludeFromRestraint=true\n")));
    assert_eq!(r.os.procs[&10].priority, NORMAL);
    let ended = r.log.ids(id::PB_END_EXCLUDED);
    assert_eq!(ended.len(), 1);
    assert!(ended[0].action.contains("profile 'G'"), "{}", ended[0].action);
}

#[test]
fn restraint_restores_the_profile_priority_not_the_pre_profile_one() {
    let c = cfg(&format!("{PB}[Profile.G]\nMatch=game.exe\nPriority=abovenormal\n"));
    let mut r = hot_rig();
    r.ticks(&c, 4);
    assert_eq!(r.os.procs[&10].priority, BELOW_NORMAL, "restrained from the profile's abovenormal");
    r.os.procs.get_mut(&10).unwrap().cpu_per_sample = burn(0.0);
    r.ticks(&c, 6);
    assert_eq!(r.os.procs[&10].priority, ABOVE_NORMAL, "released back to the profile priority");
    // And removing the profile afterwards goes all the way back to the true original.
    r.tick(&cfg(PB));
    assert_eq!(r.os.procs[&10].priority, NORMAL);
}

#[test]
fn a_profile_added_while_the_process_is_restrained_becomes_its_release_priority() {
    let mut r = hot_rig();
    r.ticks(&cfg(PB), 4);
    assert_eq!(r.os.procs[&10].priority, BELOW_NORMAL);
    let c = cfg(&format!("{PB}[Profile.G]\nMatch=game.exe\nPriority=high\n"));
    r.tick(&c);
    assert_eq!(r.os.procs[&10].priority, BELOW_NORMAL, "the restraint is not overridden");
    r.os.procs.get_mut(&10).unwrap().cpu_per_sample = burn(0.0);
    r.ticks(&c, 6);
    assert_eq!(r.os.procs[&10].priority, HIGH);
    r.tick(&cfg(PB));
    assert_eq!(r.os.procs[&10].priority, NORMAL);
}

// ---- game mode vs Restraint ------------------------------------------------------------------

const GM: &str = "[Profile.G]\nMatch=game.exe\nGameMode=true\n";

#[test]
fn game_mode_processes_are_never_restrained_but_others_still_are() {
    let mut r = hot_rig();
    r.os.add(12, "hog2.exe").cpu_per_sample = burn(60.0);
    let c = cfg(&format!("{PB}{GM}"));
    r.ticks(&c, 6);
    assert_eq!(r.os.procs[&10].priority, NORMAL, "the game is exempt");
    assert_eq!(r.os.procs[&12].priority, BELOW_NORMAL, "Restraint keeps working for everything else");
    let ex = r.log.ids(id::PB_EXEMPT);
    assert!(ex.iter().any(|e| e.name == "game.exe" && e.info.contains("game-mode process (profile 'G')")), "{ex:?}");
    // When the game exits, game mode ends with it.
    r.os.remove(10);
    r.ticks(&c, 2);
    assert_eq!(r.log.ids(id::GAME_ENTER).len(), 1);
    assert_eq!(r.log.ids(id::GAME_LEAVE).len(), 1);
}

#[test]
fn shutdown_undoes_profiles_game_mode_and_restraint_in_one_pass() {
    let c = cfg(&format!(
        "{PB}[Profile.G]\nMatch=game.exe\nPriority=high\nAffinity=0-1\nGameMode=true\n[Profile.Z]\nMatch=other.exe\nPriority=idle\n[GameMode]\nBackgroundPriority=true\nPowerPlan={{8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}}\n"
    ));
    let mut r = hot_rig();
    r.os.add(12, "hog2.exe").cpu_per_sample = burn(60.0);
    r.os.add(13, "bystander.exe");
    let power_before = r.os.power;
    r.ticks(&c, 5);
    assert_eq!(r.os.procs[&10].priority, HIGH);
    assert_eq!(r.os.procs[&10].affinity.mask, 0b11);
    assert_eq!(r.os.procs[&11].priority, IDLE);
    assert_eq!(r.os.procs[&12].priority, BELOW_NORMAL, "restrained by Restraint");
    assert_eq!(r.os.procs[&13].priority, BELOW_NORMAL, "lowered by game mode");
    assert_ne!(r.os.power, power_before);

    r.eng.release_all(&mut r.os, &mut r.log);

    for pid in [10, 11, 12, 13] {
        assert_eq!(r.os.procs[&pid].priority, NORMAL, "pid {pid}");
    }
    assert_eq!(r.os.procs[&10].affinity.mask, 0xF);
    assert_eq!(r.os.power, power_before);
    assert!(r.log.ids(id::GAME_LEAVE).iter().any(|e| e.info == "engine stopping"));
    let before = r.os.calls.len();
    r.eng.release_all(&mut r.os, &mut r.log);
    assert_eq!(r.os.calls.len(), before, "a second shutdown pass has nothing left to undo");
}

#[test]
fn a_9950x3d_profile_set_end_to_end_with_reload() {
    // Two games: one pinned to the V-Cache CCD, one to the other; V-Cache is CCD1 on this machine.
    let c1 = cfg("[Profile.Cache]\nMatch=cachegame.exe\nPriority=high\nCpuGroup=vcache\nSmtPolicy=no-smt\nExcludeFromRestraint=true\n[Profile.Freq]\nMatch=freqgame.exe\nCpuGroup=frequency\n");
    let mut r = Rig::new(SimOs::new(fake_9950x3d(false)));
    r.os.add(10, "cachegame.exe");
    r.os.add(11, "freqgame.exe");
    r.ticks(&c1, 3);
    assert_eq!(r.os.procs[&10].affinity.mask, 0x5555_0000, "first thread of each V-Cache core (CPUs 16,18,..,30)");
    assert_eq!(r.os.procs[&11].affinity.mask, 0x0000_FFFF);
    assert_eq!(r.os.procs[&10].priority, HIGH);
    let rep = r.eng.profile_report(&mut r.os, &c1);
    assert_eq!(rep.profiles.len(), 2);
    assert_eq!(rep.applied.len(), 2);
    assert!(rep.refused.is_empty());

    // Reload: swap the groups.
    let c2 = cfg("[Profile.Cache]\nMatch=cachegame.exe\nPriority=high\nCpuGroup=frequency\n[Profile.Freq]\nMatch=freqgame.exe\nCpuGroup=vcache\n");
    r.tick(&c2);
    assert_eq!(r.os.procs[&10].affinity.mask, 0x0000_FFFF);
    assert_eq!(r.os.procs[&11].affinity.mask, 0xFFFF_0000);
    r.eng.release_all(&mut r.os, &mut r.log);
    assert_eq!((r.os.procs[&10].affinity.mask, r.os.procs[&11].affinity.mask), (0xFFFF_FFFF, 0xFFFF_FFFF));
    assert_eq!(r.os.procs[&10].priority, NORMAL);
}

#[test]
fn old_configs_without_profiles_make_no_profile_calls_at_all() {
    let mut r = hot_rig();
    r.ticks(&cfg(PB), 10);
    assert!(r.log.0.iter().all(|e| !(90..=99).contains(&e.id)), "no profile or game-mode log lines");
    let rep = r.eng.profile_report(&mut r.os, &cfg(PB));
    assert!(rep.profiles.is_empty() && rep.applied.is_empty() && !rep.game.active);
    let _ = r.os.processes();
}
