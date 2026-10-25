//! Lossless editing of the config file for the Settings tab.
//!
//! The file is kept as a list of lines, each with its own original terminator, plus the file's
//! encoding (UTF-8, UTF-8 with BOM, or UTF-16LE with BOM). Saving changes only the value of keys
//! the user actually changed; comments, blank lines, unknown keys, section order, line endings and
//! encoding are carried through untouched. A key that doesn't exist yet is inserted at the end of
//! its section (or in a new section at the end of the file) using the file's own line ending.

use crate::config::{parse_mask, Config, DEFAULT_INI};

pub const SECTION: &str = "OutOfControlProcessRestraint";

// ---------------------------------------------------------------- document

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Enc {
    Utf8,
    Utf8Bom,
    Utf16Le,
}

#[derive(Clone, Debug)]
struct Line {
    text: String,
    /// "", "\n" or "\r\n".
    eol: String,
}

#[derive(Clone, Debug)]
pub struct Doc {
    enc: Enc,
    lines: Vec<Line>,
}

enum Cls {
    Section(String),
    Pair { key: String },
    Other,
}

/// Mirrors the engine's own INI reader so we agree on what is a section and what is a setting.
fn classify(text: &str) -> Cls {
    let t = text.trim().trim_start_matches('\u{feff}');
    if t.is_empty() || t.starts_with(';') || t.starts_with('#') {
        return Cls::Other;
    }
    if let Some(s) = t.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
        return Cls::Section(s.to_ascii_lowercase());
    }
    match t.split_once('=') {
        Some((k, _)) => Cls::Pair { key: k.trim().to_ascii_lowercase() },
        None => Cls::Other,
    }
}

impl Doc {
    pub fn parse(bytes: &[u8]) -> Result<Doc, String> {
        let unsupported = || "unsupported text encoding (expected UTF-8 or UTF-16 LE with a byte-order mark)".to_string();
        let (enc, text) = if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
            if rest.len() % 2 != 0 {
                return Err("file ends in the middle of a UTF-16 character".into());
            }
            let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            (Enc::Utf16Le, String::from_utf16(&units).map_err(|_| unsupported())?)
        } else if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
            (Enc::Utf8Bom, String::from_utf8(rest.to_vec()).map_err(|_| unsupported())?)
        } else {
            (Enc::Utf8, String::from_utf8(bytes.to_vec()).map_err(|_| unsupported())?)
        };
        let lines = text
            .split_inclusive('\n')
            .map(|piece| {
                if let Some(t) = piece.strip_suffix("\r\n") {
                    Line { text: t.into(), eol: "\r\n".into() }
                } else if let Some(t) = piece.strip_suffix('\n') {
                    Line { text: t.into(), eol: "\n".into() }
                } else {
                    Line { text: piece.into(), eol: String::new() }
                }
            })
            .collect();
        Ok(Doc { enc, lines })
    }

    pub fn text(&self) -> String {
        self.lines.iter().flat_map(|l| [l.text.as_str(), l.eol.as_str()]).collect()
    }

    pub fn encode(&self) -> Vec<u8> {
        let text = self.text();
        match self.enc {
            Enc::Utf8 => text.into_bytes(),
            Enc::Utf8Bom => [&[0xef, 0xbb, 0xbf][..], text.as_bytes()].concat(),
            Enc::Utf16Le => {
                let mut out = vec![0xff, 0xfe];
                out.extend(text.encode_utf16().flat_map(|u| u.to_le_bytes()));
                out
            }
        }
    }

    /// Raw (trimmed) value of the last `key` in `section`, as the engine would read it.
    pub fn get(&self, section: &str, key: &str) -> Option<String> {
        let (sec, key) = (section.to_ascii_lowercase(), key.to_ascii_lowercase());
        let mut cur = String::new();
        let mut found = None;
        for l in &self.lines {
            match classify(&l.text) {
                Cls::Section(s) => cur = s,
                Cls::Pair { key: k } if cur == sec && k == key => {
                    found = l.text.split_once('=').map(|(_, v)| v.trim().to_string());
                }
                _ => {}
            }
        }
        found
    }

    /// Section names as written (original capitalisation), in file order, duplicates included.
    pub fn section_names(&self) -> Vec<String> {
        self.lines
            .iter()
            .filter_map(|l| {
                let t = l.text.trim().trim_start_matches('\u{feff}');
                t.strip_prefix('[').and_then(|r| r.strip_suffix(']')).map(str::to_string)
            })
            .collect()
    }

    /// Delete every line that sets `key` in `section`. Returns whether anything was removed.
    pub fn remove_key(&mut self, section: &str, key: &str) -> bool {
        let (sec, keyl) = (section.to_ascii_lowercase(), key.to_ascii_lowercase());
        let mut cur = String::new();
        let mut doomed = vec![];
        for (i, l) in self.lines.iter().enumerate() {
            match classify(&l.text) {
                Cls::Section(s) => cur = s,
                Cls::Pair { key: k } if cur == sec && k == keyl => doomed.push(i),
                _ => {}
            }
        }
        for i in doomed.iter().rev() {
            self.lines.remove(*i);
        }
        !doomed.is_empty()
    }

    /// Delete a whole section (header through the line before the next header, including its blank
    /// separator lines). Comments or lines *before* the header belong to whatever precedes it and stay.
    pub fn remove_section(&mut self, section: &str) -> bool {
        let sec = section.to_ascii_lowercase();
        let mut removed = false;
        loop {
            let Some(start) = self.lines.iter().position(|l| matches!(classify(&l.text), Cls::Section(s) if s == sec)) else { break };
            let end = self.lines[start + 1..]
                .iter()
                .position(|l| matches!(classify(&l.text), Cls::Section(_)))
                .map_or(self.lines.len(), |p| start + 1 + p);
            self.lines.drain(start..end);
            removed = true;
        }
        removed
    }

    /// The line ending to use for lines we add: whatever the file mostly uses (CRLF if unknown).
    fn eol_style(&self) -> &'static str {
        let crlf = self.lines.iter().filter(|l| l.eol == "\r\n").count();
        let lf = self.lines.iter().filter(|l| l.eol == "\n").count();
        if lf > crlf { "\n" } else { "\r\n" }
    }

    /// Set `key` in `section`: rewrite the value of the existing line (the last one, which is the one
    /// the engine uses), or insert a new line.
    pub fn set(&mut self, section: &str, key: &str, value: &str) {
        let (sec, keyl) = (section.to_ascii_lowercase(), key.to_ascii_lowercase());
        let mut cur = String::new();
        let (mut target, mut header, mut anchor) = (None, None, None);
        for (i, l) in self.lines.iter().enumerate() {
            match classify(&l.text) {
                Cls::Section(s) => {
                    cur = s;
                    if cur == sec {
                        header = Some(i);
                        anchor = Some(i);
                    }
                }
                Cls::Pair { key: k } if cur == sec => {
                    anchor = Some(i);
                    if k == keyl {
                        target = Some(i);
                    }
                }
                _ => {}
            }
        }
        let eol = self.eol_style();

        if let Some(i) = target {
            let text = &self.lines[i].text;
            let eq = text.find('=').expect("a Pair line contains '='");
            let rest = &text[eq + 1..];
            let lead = &rest[..rest.len() - rest.trim_start().len()];
            self.lines[i].text = format!("{}{}{}", &text[..=eq], lead, value);
        } else if let (Some(_), Some(a)) = (header, anchor) {
            let mut new = Line { text: format!("{key}={value}"), eol: eol.into() };
            if self.lines[a].eol.is_empty() {
                // The anchor was the unterminated last line: keep "no newline at end of file".
                self.lines[a].eol = eol.into();
                new.eol.clear();
            }
            self.lines.insert(a + 1, new);
        } else {
            if let Some(last) = self.lines.last_mut() {
                if last.eol.is_empty() {
                    last.eol = eol.into();
                }
                if !last.text.trim().is_empty() {
                    self.lines.push(Line { text: String::new(), eol: eol.into() });
                }
            }
            self.lines.push(Line { text: format!("[{section}]"), eol: eol.into() });
            self.lines.push(Line { text: format!("{key}={value}"), eol: eol.into() });
        }
    }
}

// ---------------------------------------------------------------- fields

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    OocOn,
    TotalCpu,
    PerProcess,
    TimeOverQuota,
    RestorePct,
    MinRestraint,
    MaxRestraint,
    TameOnlyNormal,
    LowerToIdle,
    RestrainByAffinity,
    RestraintAffinity,
    ExcludeForeground,
    ExcludeChildren,
    ExcludeServices,
    Exclusions,
}

impl Field {
    pub const ALL: [Field; 15] = [
        Field::OocOn,
        Field::TotalCpu,
        Field::PerProcess,
        Field::TimeOverQuota,
        Field::RestorePct,
        Field::MinRestraint,
        Field::MaxRestraint,
        Field::TameOnlyNormal,
        Field::LowerToIdle,
        Field::RestrainByAffinity,
        Field::RestraintAffinity,
        Field::ExcludeForeground,
        Field::ExcludeChildren,
        Field::ExcludeServices,
        Field::Exclusions,
    ];

    /// The key in the INI file.
    pub fn key(self) -> &'static str {
        match self {
            Field::OocOn => "OocOn",
            Field::TotalCpu => "TotalProcessorUsageBeforeRestraint",
            Field::PerProcess => "PerProcessUsageBeforeRestraint",
            Field::TimeOverQuota => "TimeOverQuotaBeforeRestraint",
            Field::RestorePct => "PerProcessUsageForRestore",
            Field::MinRestraint => "MinimumTimeOfRestraint",
            Field::MaxRestraint => "MaximumTimeOfRestraint",
            Field::TameOnlyNormal => "TameOnlyNormal",
            Field::LowerToIdle => "LowerToIdleInsteadOfBelowNormal",
            Field::RestrainByAffinity => "RestrainByAffinity",
            Field::RestraintAffinity => "RestraintAffinity",
            Field::ExcludeForeground => "ExcludeForegroundProcesses2",
            Field::ExcludeChildren => "ExcludeChildrenOfForeground",
            Field::ExcludeServices => "ExcludeServices",
            Field::Exclusions => "OocExclusions",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Field::OocOn => "Restraint enabled",
            Field::TotalCpu => "Total CPU before restraint (%)",
            Field::PerProcess => "Per-process CPU before restraint (%)",
            Field::TimeOverQuota => "Time over quota before restraint (ms)",
            Field::RestorePct => "Per-process CPU for restore (%)",
            Field::MinRestraint => "Minimum restraint time (ms)",
            Field::MaxRestraint => "Maximum restraint time (ms, 0 = no limit)",
            Field::TameOnlyNormal => "Only restrain processes at Normal priority",
            Field::LowerToIdle => "Lower to Idle instead of Below Normal",
            Field::RestrainByAffinity => "Also restrain by CPU affinity",
            Field::RestraintAffinity => "Restraint affinity mask (hex)",
            Field::ExcludeForeground => "Never restrain the foreground window's process",
            Field::ExcludeChildren => "Never restrain children of the foreground app",
            Field::ExcludeServices => "Never restrain services (session 0)",
            Field::Exclusions => "Excluded processes (one per line, wildcards ok)",
        }
    }

    pub fn is_bool(self) -> bool {
        matches!(
            self,
            Field::OocOn
                | Field::TameOnlyNormal
                | Field::LowerToIdle
                | Field::RestrainByAffinity
                | Field::ExcludeForeground
                | Field::ExcludeChildren
                | Field::ExcludeServices
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    B(bool),
    N(f64),
    U(u64),
    Mask(Option<u64>),
    /// Lower-cased: the engine matches case-insensitively.
    List(Vec<String>),
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 { format!("{}", v as i64) } else { format!("{v}") }
}

fn split_list(s: &str) -> Vec<String> {
    s.split([',', '\n', '\r']).map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

// ---------------------------------------------------------------- form

/// Everything the Settings tab edits, as the strings/bools the user sees.
#[derive(Clone, Debug, PartialEq)]
pub struct Form {
    pub ooc_on: bool,
    pub total_cpu: String,
    pub per_process: String,
    pub time_over_quota: String,
    pub restore: String,
    pub min_restraint: String,
    pub max_restraint: String,
    pub tame_only_normal: bool,
    pub lower_to_idle: bool,
    pub restrain_by_affinity: bool,
    pub restraint_affinity: String,
    pub exclude_foreground: bool,
    pub exclude_children: bool,
    pub exclude_services: bool,
    /// One pattern per line.
    pub exclusions: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub key: &'static str,
    pub value: String,
}

#[derive(Debug, PartialEq)]
pub enum SaveError {
    /// Field-level problems, to show next to the settings.
    Invalid(Vec<(Field, String)>),
    NoChanges,
    /// The strict parser refused the resulting file (carries its message).
    Rejected(String),
}

impl Form {
    pub fn from_doc(doc: &Doc) -> Form {
        let cfg = Config::parse(&doc.text()); // lenient: shows what the engine would actually use
        let pb = &cfg.restraint;
        let raw_excl = doc.get(SECTION, "OocExclusions").unwrap_or_default();
        Form {
            ooc_on: pb.enabled,
            total_cpu: fmt_num(pb.total_cpu_before_restraint),
            per_process: fmt_num(pb.per_process_before_restraint),
            time_over_quota: pb.time_over_quota_ms.to_string(),
            restore: fmt_num(pb.per_process_for_restore),
            min_restraint: pb.min_restraint_ms.to_string(),
            max_restraint: pb.max_restraint_ms.to_string(),
            tame_only_normal: pb.tame_only_normal,
            lower_to_idle: pb.lower_to_idle,
            restrain_by_affinity: pb.restrain_by_affinity,
            restraint_affinity: if pb.restraint_affinity == 0 { String::new() } else { format!("0x{:x}", pb.restraint_affinity) },
            exclude_foreground: pb.exclude_foreground,
            exclude_children: pb.exclude_children_of_foreground,
            exclude_services: pb.exclude_services,
            exclusions: split_list(&raw_excl).join("\n"), // keeps the user's own capitalisation
        }
    }

    pub fn bool_mut(&mut self, f: Field) -> Option<&mut bool> {
        Some(match f {
            Field::OocOn => &mut self.ooc_on,
            Field::TameOnlyNormal => &mut self.tame_only_normal,
            Field::LowerToIdle => &mut self.lower_to_idle,
            Field::RestrainByAffinity => &mut self.restrain_by_affinity,
            Field::ExcludeForeground => &mut self.exclude_foreground,
            Field::ExcludeChildren => &mut self.exclude_children,
            Field::ExcludeServices => &mut self.exclude_services,
            _ => return None,
        })
    }

    pub fn text_mut(&mut self, f: Field) -> Option<&mut String> {
        Some(match f {
            Field::TotalCpu => &mut self.total_cpu,
            Field::PerProcess => &mut self.per_process,
            Field::TimeOverQuota => &mut self.time_over_quota,
            Field::RestorePct => &mut self.restore,
            Field::MinRestraint => &mut self.min_restraint,
            Field::MaxRestraint => &mut self.max_restraint,
            Field::RestraintAffinity => &mut self.restraint_affinity,
            Field::Exclusions => &mut self.exclusions,
            _ => return None,
        })
    }

    fn typed(&self, f: Field) -> Result<Val, String> {
        let pct = |s: &str| -> Result<Val, String> {
            match s.trim().parse::<f64>() {
                Ok(x) if x.is_finite() && (0.0..=100.0).contains(&x) => Ok(Val::N(x)),
                Ok(_) => Err("must be between 0 and 100".into()),
                Err(_) => Err("must be a number".into()),
            }
        };
        let ms = |s: &str| -> Result<Val, String> {
            match s.trim().parse::<u64>() {
                Ok(x) if x <= 86_400_000 => Ok(Val::U(x)),
                Ok(_) => Err("must be at most 86400000 ms (24 hours)".into()),
                Err(_) => Err("must be a whole number of milliseconds".into()),
            }
        };
        Ok(match f {
            Field::OocOn => Val::B(self.ooc_on),
            Field::TameOnlyNormal => Val::B(self.tame_only_normal),
            Field::LowerToIdle => Val::B(self.lower_to_idle),
            Field::RestrainByAffinity => Val::B(self.restrain_by_affinity),
            Field::ExcludeForeground => Val::B(self.exclude_foreground),
            Field::ExcludeChildren => Val::B(self.exclude_children),
            Field::ExcludeServices => Val::B(self.exclude_services),
            Field::TotalCpu => pct(&self.total_cpu)?,
            Field::PerProcess => pct(&self.per_process)?,
            Field::RestorePct => pct(&self.restore)?,
            Field::TimeOverQuota => ms(&self.time_over_quota)?,
            Field::MinRestraint => ms(&self.min_restraint)?,
            Field::MaxRestraint => ms(&self.max_restraint)?,
            Field::RestraintAffinity => {
                let s = self.restraint_affinity.trim();
                if s.is_empty() {
                    Val::Mask(None)
                } else {
                    match parse_mask(s) {
                        Some(m) if m != 0 => Val::Mask(Some(m)),
                        _ => return Err("must be a non-zero hexadecimal mask, e.g. 0xF".into()),
                    }
                }
            }
            Field::Exclusions => {
                let items = split_list(&self.exclusions);
                if items.iter().any(|i| i.chars().any(char::is_control)) {
                    return Err("contains control characters".into());
                }
                Val::List(items.iter().map(|i| i.to_lowercase()).collect())
            }
        })
    }

    /// Problems that must be fixed before saving, one per offending setting.
    pub fn errors(&self) -> Vec<(Field, String)> {
        Field::ALL.iter().filter_map(|f| self.typed(*f).err().map(|e| (*f, e))).collect()
    }

    /// Things worth a second look that don't block saving.
    pub fn warnings(&self) -> Vec<(Field, String)> {
        let mut w = vec![];
        if let (Ok(Val::U(min)), Ok(Val::U(max))) = (self.typed(Field::MinRestraint), self.typed(Field::MaxRestraint)) {
            if max > 0 && max < min {
                w.push((Field::MaxRestraint, "is shorter than the minimum restraint time, so the minimum never applies".into()));
            }
        }
        if let (Ok(Val::N(trigger)), Ok(Val::N(restore))) = (self.typed(Field::PerProcess), self.typed(Field::RestorePct)) {
            if restore > trigger {
                w.push((Field::RestorePct, "is above the trigger threshold, so processes may be restrained and restored repeatedly".into()));
            }
        }
        if self.restrain_by_affinity && matches!(self.typed(Field::RestraintAffinity), Ok(Val::Mask(None))) {
            w.push((Field::RestraintAffinity, "is empty, so affinity restraint does nothing".into()));
        }
        w
    }

    /// Settings whose value differs from `saved` (an invalid entry counts as changed).
    pub fn changed(&self, saved: &Form) -> Vec<Field> {
        Field::ALL
            .iter()
            .copied()
            .filter(|f| match (self.typed(*f), saved.typed(*f)) {
                (Ok(a), Ok(b)) => a != b,
                _ => true,
            })
            .collect()
    }

    pub fn is_dirty(&self, saved: &Form) -> bool {
        !self.changed(saved).is_empty()
    }

    fn value_text(&self, f: Field) -> Option<String> {
        Some(match self.typed(f).ok()? {
            Val::B(b) => b.to_string(),
            Val::N(n) => fmt_num(n),
            Val::U(u) => u.to_string(),
            Val::Mask(None) => String::new(),
            Val::Mask(Some(m)) => format!("0x{m:x}"),
            Val::List(_) => split_list(&self.exclusions).join(","),
        })
    }

    /// The INI lines to change: only fields that differ from `saved` and are valid.
    pub fn edits_against(&self, saved: &Form) -> Vec<Edit> {
        self.changed(saved)
            .into_iter()
            .filter_map(|f| Some(Edit { key: f.key(), value: self.value_text(f)? }))
            .collect()
    }
}

/// Apply the user's changes to the original file bytes and verify the result with the engine's
/// strict parser. `saved` is the form the file produced when it was loaded.
pub fn build_save(original: &[u8], saved: &Form, form: &Form) -> Result<Vec<u8>, SaveError> {
    let errors = form.errors();
    if !errors.is_empty() {
        return Err(SaveError::Invalid(errors));
    }
    let edits = form.edits_against(saved);
    if edits.is_empty() {
        return Err(SaveError::NoChanges);
    }
    let mut doc = Doc::parse(original).map_err(SaveError::Rejected)?;
    for e in &edits {
        doc.set(SECTION, e.key, &e.value);
    }
    let out = doc.encode();
    if let Err(msg) = Config::from_bytes(&out) {
        // Line numbers in `msg` refer to the proposed file. If the file on disk is itself broken,
        // report that instead, with line numbers the user can find in their own file.
        return Err(SaveError::Rejected(match Config::from_bytes(original) {
            Err(orig) => format!("the saved config file is already invalid ({orig}); fix that line first"),
            Ok(_) => msg,
        }));
    }
    // Belt and braces: re-reading the result must give back exactly what the user asked for.
    let reread = Form::from_doc(&Doc::parse(&out).map_err(SaveError::Rejected)?);
    if let Some(f) = edits.iter().find_map(|e| Field::ALL.iter().find(|f| f.key() == e.key).filter(|f| reread.typed(**f) != form.typed(**f))) {
        return Err(SaveError::Rejected(format!("internal error: {} did not round-trip", f.key())));
    }
    Ok(out)
}

/// Which setting a strict-parser message refers to, if any.
pub fn field_for_error(msg: &str) -> Option<Field> {
    let low = msg.to_ascii_lowercase();
    Field::ALL.iter().copied().find(|f| low.contains(&f.key().to_ascii_lowercase()))
}

/// Lists shown read-only for now: (section, key, raw value).
pub fn readonly_lists(doc: &Doc) -> Vec<(&'static str, String)> {
    [
        ("ProcessDefaults", "DefaultPriorities"),
        ("ProcessDefaults", "DefaultAffinities"),
        ("ProcessAllowances", "DisallowedProcesses"),
        ("ProcessAllowances", "InstanceLimitedProcesses"),
    ]
    .iter()
    .map(|(s, k)| (*k, doc.get(s, k).unwrap_or_default()))
    .collect()
}

/// What to edit when no config file exists yet: the engine's default file (CRLF, as Windows tools expect).
pub fn default_file_bytes() -> Vec<u8> {
    DEFAULT_INI.replace('\n', "\r\n").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LF: &str = "; my notes\n\n[OutOfControlProcessRestraint]\n; keep this comment\nOocOn=true\nTotalProcessorUsageBeforeRestraint = 23\nPerProcessUsageBeforeRestraint=15\n\n# odd spacing & unknown keys\nSomeFutureSetting=hello world\nOocExclusions=Chrome.exe, code.exe\n\n[ProcessDefaults]\nDefaultPriorities=a.exe:high\nFancy=yes\n\n[Z_Unknown]\nx=1\n";

    fn crlf(s: &str) -> String {
        s.replace('\n', "\r\n")
    }
    fn utf16(s: &str) -> Vec<u8> {
        let mut b = vec![0xff, 0xfe];
        b.extend(s.encode_utf16().flat_map(|u| u.to_le_bytes()));
        b
    }
    fn text_of(bytes: &[u8]) -> String {
        Doc::parse(bytes).unwrap().text()
    }
    /// Lines that differ between two texts (same line count assumed unless noted).
    fn changed_lines(a: &str, b: &str) -> Vec<(String, String)> {
        let (x, y): (Vec<_>, Vec<_>) = (a.split_inclusive('\n').collect(), b.split_inclusive('\n').collect());
        assert_eq!(x.len(), y.len(), "line count");
        x.iter().zip(&y).filter(|(p, q)| p != q).map(|(p, q)| (p.to_string(), q.to_string())).collect()
    }

    fn edited(bytes: &[u8], f: impl FnOnce(&mut Form)) -> (Result<Vec<u8>, SaveError>, Form) {
        let saved = Form::from_doc(&Doc::parse(bytes).unwrap());
        let mut form = saved.clone();
        f(&mut form);
        (build_save(bytes, &saved, &form), saved)
    }

    // ---- round trips -------------------------------------------------------

    #[test]
    fn untouched_files_round_trip_byte_for_byte_in_every_encoding_and_eol() {
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("utf8 lf", SAMPLE_LF.as_bytes().to_vec()),
            ("utf8 crlf", crlf(SAMPLE_LF).into_bytes()),
            ("utf8 bom", [&[0xef, 0xbb, 0xbf][..], SAMPLE_LF.as_bytes()].concat()),
            ("utf16le crlf", utf16(&crlf(SAMPLE_LF))),
            ("no trailing newline", SAMPLE_LF.trim_end().as_bytes().to_vec()),
            ("mixed endings", "[A]\r\nk=1\nj=2\r\n\n".as_bytes().to_vec()),
            ("empty", vec![]),
            ("unicode", "; caf\u{e9} \u{1F600}\n[A]\nk=\u{e9}\n".as_bytes().to_vec()),
            ("lone cr in text", "[A]\nk=a\rb\n".as_bytes().to_vec()),
        ];
        for (name, bytes) in cases {
            assert_eq!(Doc::parse(&bytes).unwrap().encode(), bytes, "{name}");
        }
    }

    #[test]
    fn unsupported_encodings_are_refused_not_mangled() {
        assert!(Doc::parse(&[0xfe, 0xff, 0x00, b'[']).is_err(), "utf-16be");
        assert!(Doc::parse(&[b'[', 0xff, 0xfe, 0xfd]).is_err(), "not utf-8");
        assert!(Doc::parse(&[0xff, 0xfe, b'[']).is_err(), "odd utf-16 length");
    }

    // ---- editing -----------------------------------------------------------

    #[test]
    fn changing_one_key_changes_only_that_line() {
        let (out, _) = edited(SAMPLE_LF.as_bytes(), |f| f.per_process = "30".into());
        let out = String::from_utf8(out.unwrap()).unwrap();
        assert_eq!(
            changed_lines(SAMPLE_LF, &out),
            vec![("PerProcessUsageBeforeRestraint=15\n".to_string(), "PerProcessUsageBeforeRestraint=30\n".to_string())]
        );
    }

    #[test]
    fn spacing_around_equals_and_key_case_are_preserved() {
        let (out, _) = edited(SAMPLE_LF.as_bytes(), |f| f.total_cpu = "40".into());
        let out = String::from_utf8(out.unwrap()).unwrap();
        assert_eq!(
            changed_lines(SAMPLE_LF, &out),
            vec![("TotalProcessorUsageBeforeRestraint = 23\n".to_string(), "TotalProcessorUsageBeforeRestraint = 40\n".to_string())]
        );
        let src = "[OutOfControlProcessRestraint]\r\nooctoggle=1\r\nTAMEONLYNORMAL = true\r\n";
        let (out, _) = edited(src.as_bytes(), |f| f.tame_only_normal = false);
        assert_eq!(String::from_utf8(out.unwrap()).unwrap(), "[OutOfControlProcessRestraint]\r\nooctoggle=1\r\nTAMEONLYNORMAL = false\r\n");
    }

    #[test]
    fn comments_unknown_keys_blank_lines_and_section_order_survive_many_edits() {
        let (out, _) = edited(SAMPLE_LF.as_bytes(), |f| {
            f.ooc_on = false;
            f.total_cpu = "50".into();
            f.exclusions = "chrome.exe\ncode.exe\nnewtool.exe".into();
            f.min_restraint = "6000".into();
        });
        let out = String::from_utf8(out.unwrap()).unwrap();
        for keep in ["; my notes", "; keep this comment", "# odd spacing & unknown keys", "SomeFutureSetting=hello world", "[ProcessDefaults]",
                     "DefaultPriorities=a.exe:high", "Fancy=yes", "[Z_Unknown]", "x=1"] {
            assert!(out.contains(keep), "lost {keep:?}:\n{out}");
        }
        let order: Vec<_> = out.lines().filter(|l| l.starts_with('[')).collect();
        assert_eq!(order, vec!["[OutOfControlProcessRestraint]", "[ProcessDefaults]", "[Z_Unknown]"]);
        assert!(out.contains("OocOn=false") && out.contains("OocExclusions=Chrome.exe, code.exe") == false);
        assert!(out.contains("OocExclusions=chrome.exe,code.exe,newtool.exe"));
        // The inserted MinimumTimeOfRestraint sits at the end of its own section, before the blank
        // line that separates it from the next one.
        assert!(out.contains("OocExclusions=chrome.exe,code.exe,newtool.exe\nMinimumTimeOfRestraint=6000\n\n[ProcessDefaults]"), "{out}");
    }

    #[test]
    fn crlf_and_lf_are_kept_for_edited_and_inserted_lines() {
        for (eol, src) in [("\r\n", crlf(SAMPLE_LF)), ("\n", SAMPLE_LF.to_string())] {
            let (out, _) = edited(src.as_bytes(), |f| {
                f.per_process = "20".into();
                f.max_restraint = "9000".into(); // not present -> inserted
            });
            let out = String::from_utf8(out.unwrap()).unwrap();
            let other = if eol == "\n" { "\r\n" } else { "\n" };
            assert!(out.contains(&format!("PerProcessUsageBeforeRestraint=20{eol}")));
            assert!(out.contains(&format!("MaximumTimeOfRestraint=9000{eol}")));
            assert_eq!(out.matches("\r\n").count(), if eol == "\r\n" { out.matches('\n').count() } else { 0 }, "no mixed endings introduced");
            let _ = other;
        }
    }

    #[test]
    fn utf16_and_bom_files_stay_in_their_encoding() {
        let u16src = utf16(&crlf(SAMPLE_LF));
        let (out, _) = edited(&u16src, |f| f.total_cpu = "55".into());
        let out = out.unwrap();
        assert_eq!(&out[..2], &[0xff, 0xfe], "BOM kept");
        assert_eq!(out.len() % 2, 0);
        let t = text_of(&out);
        assert!(t.contains("TotalProcessorUsageBeforeRestraint = 55\r\n") && t.contains("; keep this comment\r\n"));
        assert_eq!(changed_lines(&crlf(SAMPLE_LF), &t).len(), 1);

        let bom = [&[0xef, 0xbb, 0xbf][..], SAMPLE_LF.as_bytes()].concat();
        let (out, _) = edited(&bom, |f| f.total_cpu = "55".into());
        let out = out.unwrap();
        assert_eq!(&out[..3], &[0xef, 0xbb, 0xbf]);
        assert!(Config::from_bytes(&out).is_ok());
    }

    #[test]
    fn missing_key_is_inserted_in_its_section_missing_section_is_appended() {
        let src = "[Other]\nz=1\n\n[OutOfControlProcessRestraint]\nOocOn=true\n\n[Tail]\nq=2\n";
        let (out, _) = edited(src.as_bytes(), |f| f.min_restraint = "5000".into());
        assert_eq!(
            String::from_utf8(out.unwrap()).unwrap(),
            "[Other]\nz=1\n\n[OutOfControlProcessRestraint]\nOocOn=true\nMinimumTimeOfRestraint=5000\n\n[Tail]\nq=2\n"
        );
        let src = "[Other]\nz=1\n";
        let (out, _) = edited(src.as_bytes(), |f| f.tame_only_normal = false);
        assert_eq!(
            String::from_utf8(out.unwrap()).unwrap(),
            "[Other]\nz=1\n\n[OutOfControlProcessRestraint]\nTameOnlyNormal=false\n"
        );
        // CRLF file without a trailing newline.
        let src = "[Other]\r\nz=1";
        let (out, _) = edited(src.as_bytes(), |f| f.tame_only_normal = false);
        assert_eq!(String::from_utf8(out.unwrap()).unwrap(), "[Other]\r\nz=1\r\n\r\n[OutOfControlProcessRestraint]\r\nTameOnlyNormal=false\r\n");
    }

    #[test]
    fn insert_at_end_of_file_keeps_a_missing_trailing_newline_missing() {
        let src = "[OutOfControlProcessRestraint]\nOocOn=true";
        let (out, _) = edited(src.as_bytes(), |f| f.min_restraint = "5000".into());
        assert_eq!(String::from_utf8(out.unwrap()).unwrap(), "[OutOfControlProcessRestraint]\nOocOn=true\nMinimumTimeOfRestraint=5000");
    }

    #[test]
    fn duplicate_keys_edit_the_one_the_engine_uses_and_section_names_are_case_insensitive() {
        let src = "[outofcontrolprocessrestraint]\nTameOnlyNormal=true\n[x]\na=1\n[OutOfControlProcessRestraint]\nTameOnlyNormal=true\n";
        let saved = Form::from_doc(&Doc::parse(src.as_bytes()).unwrap());
        let mut f = saved.clone();
        f.tame_only_normal = false;
        let out = String::from_utf8(build_save(src.as_bytes(), &saved, &f).unwrap()).unwrap();
        assert_eq!(out, "[outofcontrolprocessrestraint]\nTameOnlyNormal=true\n[x]\na=1\n[OutOfControlProcessRestraint]\nTameOnlyNormal=false\n");
        assert!(!Config::parse(&out).restraint.tame_only_normal, "engine reads the edited (last) one");
    }

    #[test]
    fn value_types_are_written_the_way_the_engine_reads_them() {
        let (out, _) = edited(SAMPLE_LF.as_bytes(), |f| {
            f.restore = "7.5".into();
            f.restrain_by_affinity = true;
            f.restraint_affinity = "FF".into();
            f.lower_to_idle = true;
        });
        let cfg = Config::from_bytes(&out.unwrap()).unwrap();
        assert_eq!(cfg.restraint.per_process_for_restore, 7.5);
        assert!(cfg.restraint.restrain_by_affinity && cfg.restraint.lower_to_idle);
        assert_eq!(cfg.restraint.restraint_affinity, 0xff);
    }

    #[test]
    fn no_edits_when_nothing_changed_including_cosmetic_retyping() {
        let (out, _) = edited(SAMPLE_LF.as_bytes(), |_| {});
        assert_eq!(out, Err(SaveError::NoChanges));
        // "23.0" is the same number as "23"; "Chrome.EXE" is the same exclusion as "Chrome.exe".
        let (out, _) = edited(SAMPLE_LF.as_bytes(), |f| {
            f.total_cpu = "23.0".into();
            f.exclusions = "chrome.EXE\n  code.exe  ".into();
        });
        assert_eq!(out, Err(SaveError::NoChanges));
    }

    #[test]
    fn exclusions_preserve_the_users_capitalisation_until_edited() {
        let form = Form::from_doc(&Doc::parse(SAMPLE_LF.as_bytes()).unwrap());
        assert_eq!(form.exclusions, "Chrome.exe\ncode.exe");
    }

    // ---- validation --------------------------------------------------------

    #[test]
    fn validation_errors_are_reported_per_field() {
        let bad = |f: &dyn Fn(&mut Form)| {
            let mut form = Form::from_doc(&Doc::parse(SAMPLE_LF.as_bytes()).unwrap());
            f(&mut form);
            form.errors()
        };
        assert_eq!(bad(&|f| f.total_cpu = "abc".into())[0].0, Field::TotalCpu);
        assert_eq!(bad(&|f| f.total_cpu = "101".into())[0].0, Field::TotalCpu);
        assert_eq!(bad(&|f| f.per_process = "-1".into())[0].0, Field::PerProcess);
        assert_eq!(bad(&|f| f.per_process = "nan".into())[0].0, Field::PerProcess);
        assert_eq!(bad(&|f| f.restore = "".into())[0].0, Field::RestorePct);
        assert_eq!(bad(&|f| f.time_over_quota = "1.5".into())[0].0, Field::TimeOverQuota);
        assert_eq!(bad(&|f| f.min_restraint = "999999999999".into())[0].0, Field::MinRestraint);
        assert_eq!(bad(&|f| f.max_restraint = "-5".into())[0].0, Field::MaxRestraint);
        assert_eq!(bad(&|f| f.restraint_affinity = "0".into())[0].0, Field::RestraintAffinity);
        assert_eq!(bad(&|f| f.restraint_affinity = "xyz".into())[0].0, Field::RestraintAffinity);
        let two = bad(&|f| {
            f.total_cpu = "x".into();
            f.min_restraint = "y".into();
        });
        assert_eq!(two.iter().map(|e| e.0).collect::<Vec<_>>(), vec![Field::TotalCpu, Field::MinRestraint]);
        assert!(bad(&|_| {}).is_empty());
        assert!(bad(&|f| f.restraint_affinity = "0xF".into()).is_empty());
    }

    #[test]
    fn invalid_input_blocks_save_and_names_the_field() {
        let (out, _) = edited(SAMPLE_LF.as_bytes(), |f| f.total_cpu = "ninety".into());
        match out {
            Err(SaveError::Invalid(e)) => assert_eq!(e[0].0, Field::TotalCpu),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn warnings_do_not_block_saving() {
        let (out, _) = edited(SAMPLE_LF.as_bytes(), |f| {
            f.min_restraint = "5000".into();
            f.max_restraint = "1000".into();
        });
        assert!(out.is_ok());
        let mut f = Form::from_doc(&Doc::parse(SAMPLE_LF.as_bytes()).unwrap());
        f.min_restraint = "5000".into();
        f.max_restraint = "1000".into();
        f.restore = "50".into();
        f.restrain_by_affinity = true;
        let w: Vec<_> = f.warnings().iter().map(|w| w.0).collect();
        assert_eq!(w, vec![Field::MaxRestraint, Field::RestorePct, Field::RestraintAffinity]);
    }

    #[test]
    fn a_file_the_strict_parser_rejects_is_never_produced() {
        // The saved file is already broken somewhere else: saving must refuse, not paper over it.
        let broken = "[OutOfControlProcessRestraint]\nOocOn=true\nthis line is garbage\n";
        let (out, _) = edited(broken.as_bytes(), |f| f.total_cpu = "30".into());
        match out {
            Err(SaveError::Rejected(m)) => assert!(m.contains("saved config file is already invalid") && m.contains("line 3"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(field_for_error("line 5: TotalProcessorUsageBeforeRestraint: 'x' is not a number"), Some(Field::TotalCpu));
        assert_eq!(field_for_error("line 2: nothing relevant"), None);
    }

    // ---- diff / revert / misc ---------------------------------------------

    #[test]
    fn changed_fields_drive_the_modified_markers_and_revert() {
        let saved = Form::from_doc(&Doc::parse(SAMPLE_LF.as_bytes()).unwrap());
        let mut f = saved.clone();
        assert!(!f.is_dirty(&saved));
        f.ooc_on = false;
        f.per_process = "oops".into(); // invalid still counts as modified
        assert_eq!(f.changed(&saved), vec![Field::OocOn, Field::PerProcess]);
        f = saved.clone(); // Revert
        assert!(!f.is_dirty(&saved));
    }

    #[test]
    fn defaults_when_the_file_lacks_keys_match_the_engine_defaults() {
        let form = Form::from_doc(&Doc::parse(b"[Other]\nk=1\n").unwrap());
        let d = crate::config::RestraintSettings::default();
        assert_eq!(form.total_cpu, fmt_num(d.total_cpu_before_restraint));
        assert_eq!(form.min_restraint, d.min_restraint_ms.to_string());
        assert!(form.ooc_on && form.tame_only_normal && !form.lower_to_idle);
        assert_eq!(form.exclusions, "");
    }

    #[test]
    fn readonly_lists_show_raw_values() {
        let doc = Doc::parse(SAMPLE_LF.as_bytes()).unwrap();
        let l = readonly_lists(&doc);
        assert_eq!(l[0], ("DefaultPriorities", "a.exe:high".to_string()));
        assert_eq!(l[2], ("DisallowedProcesses", String::new()));
    }

    #[test]
    fn default_file_for_a_missing_config_is_valid_crlf() {
        let b = default_file_bytes();
        assert!(Config::from_bytes(&b).is_ok());
        assert!(String::from_utf8(b).unwrap().contains("\r\n"));
        // Editing it works like any other file.
        let (out, _) = edited(&default_file_bytes(), |f| f.total_cpu = "30".into());
        assert!(Config::from_bytes(&out.unwrap()).unwrap().restraint.total_cpu_before_restraint == 30.0);
    }

    #[test]
    fn every_field_has_a_unique_engine_key_that_the_engine_understands() {
        let keys: std::collections::HashSet<_> = Field::ALL.iter().map(|f| f.key()).collect();
        assert_eq!(keys.len(), Field::ALL.len());
        // Flip each setting and confirm the engine's own parser sees the change (guards key typos).
        let base = Form::from_doc(&Doc::parse(DEFAULT_INI.as_bytes()).unwrap());
        for f in Field::ALL {
            let mut form = base.clone();
            match f {
                Field::Exclusions => form.exclusions = "zzz.exe".into(),
                Field::RestraintAffinity => form.restraint_affinity = "0x3".into(),
                f if f.is_bool() => {
                    let b = form.bool_mut(f).unwrap();
                    *b = !*b;
                }
                Field::TotalCpu | Field::PerProcess | Field::RestorePct => *form.text_mut(f).unwrap() = "11".into(),
                _ => *form.text_mut(f).unwrap() = "1234".into(),
            }
            let out = build_save(DEFAULT_INI.as_bytes(), &base, &form).unwrap_or_else(|e| panic!("{f:?}: {e:?}"));
            assert_ne!(Config::from_bytes(&out).unwrap(), Config::parse(DEFAULT_INI), "{f:?} ({}) had no effect", f.key());
        }
    }

    #[test]
    fn the_lossless_editor_carries_log_rotation_keys_comments_and_unknown_keys_through() {
        let src = "; my notes\r\n[Logging]\r\n; keep it short\r\nLogDisable=false\r\nMaxLogSizeMB=10\r\nFutureLogKey=7\r\n\r\n[Other]\r\nk=1\r\n";
        let mut doc = Doc::parse(src.as_bytes()).unwrap();
        doc.set("Logging", "MaxLogSizeMB", "25");
        doc.set("Logging", "KeepLogFiles", "8"); // a key that did not exist is added to its section
        let out = String::from_utf8(doc.encode()).unwrap();
        assert!(out.starts_with("; my notes\r\n[Logging]\r\n; keep it short\r\nLogDisable=false\r\nMaxLogSizeMB=25\r\nFutureLogKey=7\r\n"), "{out:?}");
        assert!(out.contains("KeepLogFiles=8\r\n") && out.contains("[Other]\r\nk=1\r\n"), "{out:?}");
        let cfg = crate::config::Config::from_bytes(out.as_bytes()).unwrap();
        assert_eq!(cfg.log_rotation, crate::config::LogRotation { max_mb: 25, keep: 8 });
    }
}
