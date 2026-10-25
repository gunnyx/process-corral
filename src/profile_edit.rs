//! Lossless editing of `[Profile.*]` sections for the GUI's Profiles tab. Works on the same `Doc` as
//! the Settings editor, so comments, unknown keys, ordering, line endings and encoding all survive;
//! only the keys of profiles the user changed are touched.

use crate::config::{class_name, Config};
use crate::profile::{self, section_name, CpuRule, Mech, Profile, Select, Smt, EXCLUDE_KEY};
use crate::settings::{Doc, Form, SaveError, SECTION};
use crate::topology::{GroupRef, Topology};

#[derive(Clone, Debug, PartialEq)]
pub enum CpuChoice {
    /// The profile leaves CPUs alone (except via SmtPolicy).
    None,
    /// A named L3 group, resolved against the machine at apply time (portable between machines).
    Group(GroupRef),
    /// Explicit logical processors.
    Cpus(Vec<usize>),
}

/// Everything the Profiles tab edits for one profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ProfileForm {
    pub name: String,
    pub matcher: String,
    pub enabled: bool,
    pub priority: Option<u32>,
    pub cpu: CpuChoice,
    /// How `cpu` (and SmtPolicy) is applied.
    pub mech: Mech,
    pub smt: Smt,
    pub game_mode: bool,
    pub exclude_restraint: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PField {
    Name,
    Match,
    Cpu,
}

impl ProfileForm {
    pub fn new(name: &str) -> Self {
        ProfileForm {
            name: name.to_string(),
            matcher: String::new(),
            enabled: true,
            priority: None,
            cpu: CpuChoice::None,
            mech: Mech::Affinity,
            smt: Smt::All,
            game_mode: false,
            exclude_restraint: false,
        }
    }

    pub fn from_profile(p: &Profile) -> Self {
        let (cpu, mech) = match &p.cpu {
            None => (CpuChoice::None, Mech::Affinity),
            Some(CpuRule { select, mech }) => (
                match select {
                    Select::Mask(m) => CpuChoice::Cpus((0..64).filter(|b| m >> b & 1 == 1).collect()),
                    Select::List(v) => CpuChoice::Cpus(v.clone()),
                    Select::Group(g) => CpuChoice::Group(*g),
                    Select::All => CpuChoice::None,
                },
                *mech,
            ),
        };
        ProfileForm {
            name: p.name.clone(),
            matcher: p.matcher.clone(),
            enabled: p.enabled,
            priority: p.priority,
            cpu,
            mech,
            smt: p.smt,
            game_mode: p.game_mode,
            exclude_restraint: p.exclude_restraint,
        }
    }

    /// What the engine will make of this form once it is written out and parsed back.
    pub fn to_profile(&self) -> Profile {
        let cpu = match &self.cpu {
            CpuChoice::None => None,
            CpuChoice::Group(g) => Some(CpuRule { select: Select::Group(*g), mech: self.mech }),
            CpuChoice::Cpus(v) => {
                let mut v = v.clone();
                v.sort_unstable();
                v.dedup();
                Some(CpuRule { select: Select::List(v), mech: self.mech })
            }
        };
        Profile {
            name: self.name.clone(),
            matcher: self.matcher.trim().to_string(),
            enabled: self.enabled,
            priority: self.priority,
            cpu,
            smt: self.smt,
            game_mode: self.game_mode,
            exclude_restraint: self.exclude_restraint,
        }
    }

    /// The logical processors this profile selects on `topo` (for showing checkboxes and the mask).
    pub fn resolved_cpus(&self, topo: &Topology) -> Result<Vec<usize>, String> {
        Ok(self.to_profile().resolve_cpus(topo)?.map(|(_, c)| c).unwrap_or_default())
    }

    /// Hex mask(s) of the selection, one per processor group, e.g. `0x00000000FFFF0000`.
    pub fn mask_text(&self, topo: &Topology) -> String {
        match self.resolved_cpus(topo) {
            Ok(c) if c.is_empty() => "(no CPU restriction)".into(),
            Ok(c) => topo.group_masks(&c).iter().map(|(g, m)| if topo.groups > 1 { format!("group {g}: {m:#018X}") } else { format!("{m:#018X}") }).collect::<Vec<_>>().join("   "),
            Err(e) => format!("({e})"),
        }
    }

    /// Problems to show next to fields. `other_names` are the other profiles' names.
    pub fn errors(&self, topo: &Topology, other_names: &[String]) -> Vec<(PField, String)> {
        let mut e = vec![];
        let n = self.name.trim();
        if n.is_empty() {
            e.push((PField::Name, "a name is required".into()));
        } else if n != self.name {
            e.push((PField::Name, "must not start or end with a space".into()));
        } else if n.contains(['[', ']', '\n', '\r', '=']) {
            e.push((PField::Name, "must not contain [ ] = or line breaks".into()));
        } else if other_names.iter().any(|o| o.eq_ignore_ascii_case(n)) {
            e.push((PField::Name, "another profile already has this name".into()));
        }
        if self.matcher.trim().is_empty() {
            e.push((PField::Match, "enter the program to match, like game.exe or C:\\Games\\*\\game.exe".into()));
        } else if self.matcher.contains(['\n', '\r']) {
            e.push((PField::Match, "must be a single line".into()));
        }
        if let Err(m) = self.to_profile().validate_against(topo) {
            e.push((PField::Cpu, m));
        }
        e
    }
}

/// Read profiles from a document. Invalid sections come back as `(name, error)` so the UI can show
/// them instead of silently dropping them.
pub fn read_profiles(doc: &Doc) -> (Vec<ProfileForm>, Vec<(String, String)>) {
    let (mut ok, mut bad) = (vec![], vec![]);
    for sec in profile::raw_sections(&doc.text()) {
        match profile::parse_section(&sec) {
            Ok(p) => {
                if !ok.iter().any(|f: &ProfileForm| f.name.eq_ignore_ascii_case(&p.name)) {
                    ok.push(ProfileForm::from_profile(&p));
                }
            }
            Err((line, m)) => bad.push((sec.name.clone(), format!("line {line}: {m}"))),
        }
    }
    (ok, bad)
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProfileOp {
    Add(ProfileForm),
    Delete(String),
    Edit { base: ProfileForm, new: ProfileForm },
}

/// The keys to set (`Some`) or remove (`None`) to turn `base` into `new`. Only changed keys appear;
/// the CPU keys are rewritten as a family because they exclude each other.
pub fn form_edits(base: &ProfileForm, new: &ProfileForm) -> Vec<(&'static str, Option<String>)> {
    let mut out = vec![];
    let b = |v: bool| Some(v.to_string());
    if base.matcher.trim() != new.matcher.trim() {
        out.push(("Match", Some(new.matcher.trim().to_string())));
    }
    if base.enabled != new.enabled {
        out.push(("Enabled", b(new.enabled)));
    }
    if base.priority != new.priority {
        out.push(("Priority", new.priority.map(|c| class_name(c).to_string())));
    }
    if base.to_profile().cpu != new.to_profile().cpu {
        out.extend(cpu_keys(new));
    }
    if base.smt != new.smt {
        out.push(("SmtPolicy", if new.smt == Smt::NoSmt { Some("no-smt".into()) } else { None }));
    }
    if base.game_mode != new.game_mode {
        out.push(("GameMode", if new.game_mode { b(true) } else { None }));
    }
    if base.exclude_restraint != new.exclude_restraint {
        out.push((EXCLUDE_KEY, if new.exclude_restraint { b(true) } else { None }));
    }
    out
}

/// Affinity / CpuSets / CpuGroup / CpuMode for a form: set the ones it needs, remove the others.
fn cpu_keys(f: &ProfileForm) -> Vec<(&'static str, Option<String>)> {
    let (mut aff, mut sets, mut grp, mut mode) = (None, None, None, None);
    match &f.cpu {
        CpuChoice::None => {}
        CpuChoice::Group(g) => {
            grp = Some(g.to_string());
            if f.mech == Mech::CpuSets {
                mode = Some("cpusets".to_string());
            }
        }
        CpuChoice::Cpus(v) => {
            let list = Topology::cpu_list(v);
            if f.mech == Mech::CpuSets {
                sets = Some(list);
            } else {
                aff = Some(list);
            }
        }
    }
    vec![("Affinity", aff), ("CpuSets", sets), ("CpuGroup", grp), ("CpuMode", mode)]
}

fn add_keys(f: &ProfileForm) -> Vec<(&'static str, String)> {
    let mut v = vec![("Match", f.matcher.trim().to_string()), ("Enabled", f.enabled.to_string())];
    if let Some(c) = f.priority {
        v.push(("Priority", class_name(c).to_string()));
    }
    v.extend(cpu_keys(f).into_iter().filter_map(|(k, val)| val.map(|x| (k, x))));
    if f.smt == Smt::NoSmt {
        v.push(("SmtPolicy", "no-smt".into()));
    }
    if f.game_mode {
        v.push(("GameMode", "true".into()));
    }
    if f.exclude_restraint {
        v.push((EXCLUDE_KEY, "true".into()));
    }
    v
}

pub fn apply_ops(doc: &mut Doc, ops: &[ProfileOp]) {
    for op in ops {
        match op {
            ProfileOp::Add(f) => {
                let sec = section_name(&f.name);
                for (k, v) in add_keys(f) {
                    doc.set(&sec, k, &v);
                }
            }
            ProfileOp::Delete(name) => {
                doc.remove_section(&section_name(name));
            }
            ProfileOp::Edit { base, new } => {
                let sec = section_name(&base.name);
                for (k, v) in form_edits(base, new) {
                    match v {
                        Some(v) => doc.set(&sec, k, &v),
                        None => {
                            doc.remove_key(&sec, k);
                        }
                    }
                }
            }
        }
    }
}

/// Settings edits and profile edits in one go, verified with the engine's strict parser, and
/// re-read to check that every added or edited profile means exactly what the user set.
pub fn build_save_all(original: &[u8], saved: &Form, form: &Form, ops: &[ProfileOp]) -> Result<Vec<u8>, SaveError> {
    let errors = form.errors();
    if !errors.is_empty() {
        return Err(SaveError::Invalid(errors));
    }
    let edits = form.edits_against(saved);
    if edits.is_empty() && ops.is_empty() {
        return Err(SaveError::NoChanges);
    }
    let mut doc = Doc::parse(original).map_err(SaveError::Rejected)?;
    for e in &edits {
        doc.set(SECTION, e.key, &e.value);
    }
    apply_ops(&mut doc, ops);
    let out = doc.encode();
    let cfg = match Config::from_bytes(&out) {
        Ok(c) => c,
        Err(msg) => {
            return Err(SaveError::Rejected(match Config::from_bytes(original) {
                Err(orig) => format!("the saved config file is already invalid ({orig}); fix that line first"),
                Ok(_) => msg,
            }))
        }
    };
    for op in ops {
        let want = match op {
            ProfileOp::Add(f) => Some(&f.name),
            ProfileOp::Edit { new, .. } => Some(&new.name),
            ProfileOp::Delete(name) => {
                if cfg.profiles.iter().any(|p| p.name.eq_ignore_ascii_case(name)) {
                    return Err(SaveError::Rejected(format!("internal error: profile '{name}' was not removed")));
                }
                None
            }
        };
        if let Some(name) = want {
            let f = match op {
                ProfileOp::Add(f) => f,
                ProfileOp::Edit { new, .. } => new,
                ProfileOp::Delete(_) => unreachable!(),
            };
            match cfg.profiles.iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
                Some(p) if *p == f.to_profile() => {}
                _ => return Err(SaveError::Rejected(format!("internal error: profile '{name}' did not round-trip"))),
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{fake_9950x3d, fake_quad, fake_two_groups};

    const FILE: &str = "; my notes\n[OutOfControlProcessRestraint]\nOocOn=true\n\n[Profile.Game]\n; the main game\nMatch=game.exe\nPriority=high\nCpuGroup=vcache\nNote=keep this unknown key\n\n[Profile.Other]\nMatch=other.exe\n\n[Z_Unknown]\nx=1\n";

    fn doc(s: &str) -> Doc {
        Doc::parse(s.as_bytes()).unwrap()
    }
    fn forms(s: &str) -> Vec<ProfileForm> {
        read_profiles(&doc(s)).0
    }
    fn save(orig: &str, ops: &[ProfileOp]) -> Result<String, SaveError> {
        let d = doc(orig);
        let f = Form::from_doc(&d);
        build_save_all(orig.as_bytes(), &f, &f, ops).map(|b| String::from_utf8(b).unwrap())
    }
    fn lines(s: &str) -> Vec<&str> {
        s.split_inclusive('\n').collect()
    }

    #[test]
    fn reads_profiles_and_reports_broken_ones_without_dropping_the_rest() {
        let (ok, bad) = read_profiles(&doc("[Profile.A]\nMatch=a.exe\n[Profile.B]\nPriority=realtime\nMatch=b.exe\n[Profile.C]\nMatch=c.exe\n"));
        assert_eq!(ok.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["A", "C"]);
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].0, "B");
        assert!(bad[0].1.contains("realtime"), "{:?}", bad);
        let g = &forms(FILE)[0];
        assert_eq!((g.name.as_str(), g.matcher.as_str(), g.priority), ("Game", "game.exe", Some(crate::config::HIGH)));
        assert_eq!(g.cpu, CpuChoice::Group(GroupRef::VCache));
    }

    #[test]
    fn form_round_trips_through_profile() {
        for f in forms(FILE) {
            assert_eq!(ProfileForm::from_profile(&f.to_profile()), f);
        }
        let m = ProfileForm::from_profile(&profile::parse_strict("[Profile.M]\nMatch=a\nAffinity=0xF0").unwrap()[0]);
        assert_eq!(m.cpu, CpuChoice::Cpus(vec![4, 5, 6, 7]));
    }

    #[test]
    fn editing_one_key_changes_one_line_and_keeps_everything_else() {
        let base = forms(FILE)[0].clone();
        let mut new = base.clone();
        new.priority = Some(crate::config::ABOVE_NORMAL);
        let out = save(FILE, &[ProfileOp::Edit { base, new }]).unwrap();
        let (a, b) = (lines(FILE), lines(&out));
        assert_eq!(a.len(), b.len());
        let diff: Vec<_> = a.iter().zip(&b).filter(|(x, y)| x != y).collect();
        assert_eq!(diff, vec![(&"Priority=high\n", &"Priority=abovenormal\n")]);
        assert!(out.contains("Note=keep this unknown key") && out.contains("; the main game") && out.contains("; my notes"));
    }

    #[test]
    fn enabling_and_disabling_is_a_single_line_and_is_reversible() {
        let base = forms(FILE)[1].clone();
        let mut off = base.clone();
        off.enabled = false;
        let out = save(FILE, &[ProfileOp::Edit { base: base.clone(), new: off.clone() }]).unwrap();
        assert!(out.contains("[Profile.Other]\nMatch=other.exe\nEnabled=false\n\n[Z_Unknown]"), "{out}");
        assert!(!Config::parse(&out).profiles[1].enabled);
        let back = save(&out, &[ProfileOp::Edit { base: off, new: base }]).unwrap();
        assert!(back.contains("Enabled=true"));
        assert!(Config::parse(&back).profiles[1].enabled);
    }

    #[test]
    fn switching_between_affinity_cpusets_and_groups_rewrites_the_cpu_keys_as_a_family() {
        let base = forms(FILE)[0].clone();
        let mut sets = base.clone();
        sets.cpu = CpuChoice::Cpus(vec![0, 1, 2, 3, 8]);
        sets.mech = Mech::CpuSets;
        let out = save(FILE, &[ProfileOp::Edit { base: base.clone(), new: sets.clone() }]).unwrap();
        assert!(out.contains("CpuSets=0-3,8\n") && !out.contains("CpuGroup="), "{out}");
        let p = &Config::parse(&out).profiles[0];
        assert_eq!(p.cpu.as_ref().unwrap().mech, Mech::CpuSets);

        let mut grp = sets.clone();
        grp.cpu = CpuChoice::Group(GroupRef::Frequency);
        let out2 = save(&out, &[ProfileOp::Edit { base: sets.clone(), new: grp.clone() }]).unwrap();
        assert!(out2.contains("CpuGroup=frequency\n") && out2.contains("CpuMode=cpusets\n") && !out2.contains("CpuSets="), "{out2}");

        let mut none = grp.clone();
        none.cpu = CpuChoice::None;
        let out3 = save(&out2, &[ProfileOp::Edit { base: grp, new: none }]).unwrap();
        assert!(!out3.contains("CpuGroup") && !out3.contains("CpuMode") && !out3.contains("CpuSets"), "{out3}");
        assert!(out3.contains("Note=keep this unknown key"));
        assert!(Config::from_bytes(out3.as_bytes()).is_ok());
    }

    #[test]
    fn adding_a_profile_appends_a_new_section_in_the_files_style() {
        let mut f = ProfileForm::new("New Game");
        f.matcher = "new.exe".into();
        f.priority = Some(crate::config::HIGH);
        f.cpu = CpuChoice::Group(GroupRef::VCache);
        f.game_mode = true;
        f.exclude_restraint = true;
        f.smt = Smt::NoSmt;
        let out = save(FILE, &[ProfileOp::Add(f.clone())]).unwrap();
        assert!(out.starts_with(FILE.trim_end_matches('\n')), "existing content untouched");
        assert!(out.ends_with("[Z_Unknown]\nx=1\n\n[Profile.New Game]\nMatch=new.exe\nEnabled=true\nPriority=high\nCpuGroup=vcache\nSmtPolicy=no-smt\nGameMode=true\nExcludeFromRestraint=true\n"), "{out}");
        let cfg = Config::from_bytes(out.as_bytes()).unwrap();
        assert_eq!(cfg.profiles.last().unwrap(), &f.to_profile());
        // CRLF file: the new section uses CRLF too.
        let crlf = FILE.replace('\n', "\r\n");
        let out = save(&crlf, &[ProfileOp::Add(f)]).unwrap();
        assert_eq!(out.matches("\r\n").count(), out.matches('\n').count(), "no bare LF introduced");
    }

    #[test]
    fn deleting_a_profile_removes_just_that_section() {
        let out = save(FILE, &[ProfileOp::Delete("Game".into())]).unwrap();
        assert_eq!(out, "; my notes\n[OutOfControlProcessRestraint]\nOocOn=true\n\n[Profile.Other]\nMatch=other.exe\n\n[Z_Unknown]\nx=1\n");
        let out = save(FILE, &[ProfileOp::Delete("Other".into()), ProfileOp::Delete("game".into())]).unwrap();
        assert!(!out.contains("Profile.") && out.contains("[Z_Unknown]") && out.contains("OocOn=true"), "{out}");
        assert!(Config::from_bytes(out.as_bytes()).unwrap().profiles.is_empty());
    }

    #[test]
    fn utf16_files_stay_utf16_through_profile_edits() {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(FILE.replace('\n', "\r\n").encode_utf16().flat_map(|u| u.to_le_bytes()));
        let d = Doc::parse(&bytes).unwrap();
        let f = Form::from_doc(&d);
        let base = read_profiles(&d).0[0].clone();
        let mut new = base.clone();
        new.game_mode = true;
        let out = build_save_all(&bytes, &f, &f, &[ProfileOp::Edit { base, new }]).unwrap();
        assert_eq!(&out[..2], &[0xff, 0xfe]);
        assert!(Config::from_bytes(&out).unwrap().profiles[0].game_mode);
    }

    // ---- the exclude setting ----------------------------------------------------------------

    const EXCLUDE_FILE: &str = "; top comment\n[OutOfControlProcessRestraint]\nOocOn=true\n\n[Profile.Game]\n; keep me\nMatch=game.exe\nExcludeFromRestraint = false\nNote=unknown key\nPriority=high\n\n[Z_Unknown]\nx=1\n";

    fn exclude_edit(file: &str, to: bool) -> String {
        let base = forms(file)[0].clone();
        let mut new = base.clone();
        new.exclude_restraint = to;
        save(file, &[ProfileOp::Edit { base, new }]).unwrap()
    }

    #[test]
    fn editing_the_setting_rewrites_that_line_in_place() {
        let out = exclude_edit(EXCLUDE_FILE, true);
        let (a, b) = (lines(EXCLUDE_FILE), lines(&out));
        assert_eq!(a.len(), b.len(), "no line added or removed");
        let diff: Vec<_> = a.iter().zip(&b).filter(|(x, y)| x != y).collect();
        assert_eq!(diff, vec![(&"ExcludeFromRestraint = false\n", &"ExcludeFromRestraint = true\n")], "same position, same spacing");
        assert!(out.contains("; keep me") && out.contains("; top comment") && out.contains("Note=unknown key") && out.contains("[Z_Unknown]\nx=1"));
        assert!(Config::from_bytes(out.as_bytes()).unwrap().profiles[0].exclude_restraint);
    }

    #[test]
    fn turning_the_setting_off_removes_the_line() {
        let on = EXCLUDE_FILE.replace("= false", "= true");
        let out = exclude_edit(&on, false);
        assert!(!out.to_lowercase().contains("excludefrom"), "{out}");
        assert_eq!(lines(&on).len(), lines(&out).len() + 1);
        assert!(!Config::from_bytes(out.as_bytes()).unwrap().profiles[0].exclude_restraint);
    }

    #[test]
    fn turning_the_setting_on_adds_the_key_once_when_the_profile_lacks_it() {
        let f = "[Profile.G]\nMatch=g.exe\nPriority=high\n";
        let out = exclude_edit(f, true);
        assert_eq!(out.matches("ExcludeFromRestraint=true").count(), 1, "{out}");
        assert!(Config::from_bytes(out.as_bytes()).unwrap().profiles[0].exclude_restraint);
    }

    #[test]
    fn an_unrelated_edit_leaves_the_exclude_line_alone_and_keeps_comments_and_unknown_keys() {
        let base = forms(EXCLUDE_FILE)[0].clone();
        let mut new = base.clone();
        new.priority = Some(crate::config::ABOVE_NORMAL);
        let out = save(EXCLUDE_FILE, &[ProfileOp::Edit { base, new }]).unwrap();
        let (a, b) = (lines(EXCLUDE_FILE), lines(&out));
        let diff: Vec<_> = a.iter().zip(&b).filter(|(x, y)| x != y).collect();
        assert_eq!(diff, vec![(&"Priority=high\n", &"Priority=abovenormal\n")], "only the edited key changes");
        assert!(out.contains("ExcludeFromRestraint = false") && out.contains("; keep me") && out.contains("Note=unknown key"));
    }

    #[test]
    fn crlf_and_utf16_files_stay_intact_when_the_setting_changes() {
        let crlf = EXCLUDE_FILE.replace('\n', "\r\n");
        let out = exclude_edit(&crlf, true);
        assert_eq!(out.matches("\r\n").count(), out.matches('\n').count(), "no bare LF introduced");
        assert!(out.contains("ExcludeFromRestraint = true\r\n"));
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(crlf.encode_utf16().flat_map(|u| u.to_le_bytes()));
        let d = Doc::parse(&bytes).unwrap();
        let f = Form::from_doc(&d);
        let base = read_profiles(&d).0[0].clone();
        let mut new = base.clone();
        new.exclude_restraint = true;
        let out = build_save_all(&bytes, &f, &f, &[ProfileOp::Edit { base, new }]).unwrap();
        assert_eq!(&out[..2], &[0xff, 0xfe]);
        assert!(Config::from_bytes(&out).unwrap().profiles[0].exclude_restraint);
    }

    #[test]
    fn settings_and_profile_changes_save_together() {
        let d = doc(FILE);
        let saved = Form::from_doc(&d);
        let mut form = saved.clone();
        form.total_cpu = "40".into();
        let base = forms(FILE)[1].clone();
        let mut new = base.clone();
        new.exclude_restraint = true;
        let out = String::from_utf8(build_save_all(FILE.as_bytes(), &saved, &form, &[ProfileOp::Edit { base, new }]).unwrap()).unwrap();
        let cfg = Config::from_bytes(out.as_bytes()).unwrap();
        assert_eq!(cfg.restraint.total_cpu_before_restraint, 40.0);
        assert!(cfg.profiles[1].exclude_restraint);
    }

    #[test]
    fn no_changes_and_invalid_settings_are_reported() {
        assert_eq!(save(FILE, &[]), Err(SaveError::NoChanges));
        let d = doc(FILE);
        let saved = Form::from_doc(&d);
        let mut bad = saved.clone();
        bad.total_cpu = "abc".into();
        assert!(matches!(build_save_all(FILE.as_bytes(), &saved, &bad, &[]), Err(SaveError::Invalid(_))));
    }

    #[test]
    fn a_profile_the_parser_would_reject_never_gets_written() {
        let mut f = ProfileForm::new("Bad");
        f.matcher = "x.exe".into();
        f.cpu = CpuChoice::Cpus(vec![]); // an empty explicit list writes an empty key -> no CPU rule at all
        assert!(f.errors(&fake_quad(), &[]).iter().any(|e| e.0 == PField::Cpu && e.1.contains("selects no processors")), "the form flags it");
        assert!(matches!(save(FILE, &[ProfileOp::Add(f)]), Err(SaveError::Rejected(m)) if m.contains("round-trip")), "and the writer refuses to emit it");
        // A name that would break the section header is refused before it reaches the file.
        let topo = fake_quad();
        let mut n = ProfileForm::new("a]b");
        n.matcher = "x".into();
        assert!(n.errors(&topo, &[]).iter().any(|e| e.0 == PField::Name));
    }

    // ---- validation shown next to the fields ----------------------------------------------

    #[test]
    fn errors_are_attached_to_the_right_field() {
        let topo = fake_quad();
        let mut f = ProfileForm::new("Ok");
        assert_eq!(f.errors(&topo, &[]).iter().map(|e| e.0).collect::<Vec<_>>(), vec![PField::Match]);
        f.matcher = "game.exe".into();
        assert!(f.errors(&topo, &[]).is_empty());
        assert_eq!(f.errors(&topo, &["ok".into()])[0].0, PField::Name, "duplicate names, case-insensitively");
        f.name = " pad".into();
        assert_eq!(f.errors(&topo, &[])[0].0, PField::Name);
        f.name = "Ok".into();
        f.cpu = CpuChoice::Cpus(vec![0, 9]);
        let e = f.errors(&topo, &[]);
        assert_eq!(e[0].0, PField::Cpu);
        assert!(e[0].1.contains("processor 9 does not exist"), "{e:?}");
        f.cpu = CpuChoice::Group(GroupRef::VCache);
        assert!(f.errors(&topo, &[])[0].1.contains("no cache-heavy"));
    }

    #[test]
    fn mask_text_and_resolved_cpus_follow_the_detected_machine() {
        let mut f = ProfileForm::new("G");
        f.matcher = "g".into();
        f.cpu = CpuChoice::Group(GroupRef::VCache);
        assert_eq!(f.mask_text(&fake_9950x3d(true)), "0x000000000000FFFF");
        assert_eq!(f.mask_text(&fake_9950x3d(false)), "0x00000000FFFF0000");
        assert_eq!(f.resolved_cpus(&fake_9950x3d(false)).unwrap(), (16..32).collect::<Vec<_>>());
        assert!(f.mask_text(&fake_quad()).contains("no cache-heavy"));
        f.cpu = CpuChoice::None;
        assert_eq!(f.mask_text(&fake_quad()), "(no CPU restriction)");
        f.cpu = CpuChoice::Cpus(vec![0, 64, 65]);
        f.mech = Mech::CpuSets;
        let m = f.mask_text(&fake_two_groups());
        assert!(m.contains("group 0: 0x0000000000000001") && m.contains("group 1: 0x0000000000000003"), "{m}");
        f.smt = Smt::NoSmt;
        assert_eq!(f.resolved_cpus(&fake_two_groups()).unwrap(), vec![0, 64]);
    }

    #[test]
    fn doc_section_and_key_removal_helpers() {
        let mut d = doc("[A]\nk=1\nk=2\nj=3\n\n[B]\nz=1\n");
        assert!(d.remove_key("a", "K"));
        assert_eq!(d.text(), "[A]\nj=3\n\n[B]\nz=1\n");
        assert!(!d.remove_key("A", "nope"));
        assert!(d.remove_section("A"));
        assert_eq!(d.text(), "[B]\nz=1\n");
        assert!(!d.remove_section("A"));
        assert_eq!(d.section_names(), vec!["B"]);
        let mut dup = doc("[X]\na=1\n[Y]\nb=1\n[x]\nc=1\n");
        assert!(dup.remove_section("X"));
        assert_eq!(dup.text(), "[Y]\nb=1\n", "every occurrence of the section goes");
    }
}
