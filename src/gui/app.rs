//! The egui application: header (engine + service state, service buttons) and three tabs.

use eframe::egui;
use crate::actions::Runner;
use crate::logic::{buttons, fmt_duration_ms, uninstall_prompt, UninstallConfirm};
use crate::poller::Snapshot;
use crate::tray::TrayShared;
use egui::{Color32, RichText};
use process_corral::elevate::Command;
use process_corral::logview::{self, Kind};
use process_corral::paths::{Paths, Scope};
use process_corral::scm::ServiceState;
use process_corral::profile_edit::{self, PField, ProfileForm, ProfileOp};
use process_corral::settings::{self, Doc, Field, Form, SaveError};
use process_corral::topology::{self, Topology};
use process_corral::statusview::NotRunning;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[path = "profiles_tab.rs"]
mod profiles_tab;

const GOOD: Color32 = Color32::from_rgb(90, 200, 110);
const BAD: Color32 = Color32::from_rgb(235, 90, 90);
const WARN: Color32 = Color32::from_rgb(235, 170, 60);
const DIM: Color32 = Color32::from_rgb(150, 150, 150);
const INFO: Color32 = Color32::from_rgb(120, 180, 235);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Status,
    Settings,
    Profiles,
    Log,
}

/// The config file as last read from disk.
struct Loaded {
    bytes: Vec<u8>,
    doc: Doc,
    /// What the file says right now, in form terms; edits are diffed against this.
    saved: Form,
    /// No file on disk: we are editing the engine's defaults and Save will create it.
    missing: bool,
    /// `[Profile.*]` sections that parse, and the ones that don't (name, reason).
    profiles: Vec<ProfileForm>,
    profiles_bad: Vec<(String, String)>,
}

/// One line of the Profiles tab: the form being edited and what is saved in the file.
pub(crate) struct ProfileRow {
    pub form: ProfileForm,
    /// `None`: not saved yet (a new or duplicated profile).
    pub base: Option<ProfileForm>,
    pub deleted: bool,
}

/// What saving would change: new rows are added, deleted saved rows are removed, edited ones edited.
pub(crate) fn ops_from_rows(rows: &[ProfileRow]) -> Vec<ProfileOp> {
    rows.iter()
        .filter_map(|r| match (&r.base, r.deleted) {
            (None, true) => None,
            (None, false) => Some(ProfileOp::Add(r.form.clone())),
            (Some(b), true) => Some(ProfileOp::Delete(b.name.clone())),
            (Some(b), false) => (r.form != *b).then(|| ProfileOp::Edit { base: b.clone(), new: r.form.clone() }),
        })
        .collect()
}

fn rows_from(forms: &[ProfileForm]) -> Vec<ProfileRow> {
    forms.iter().map(|f| ProfileRow { form: f.clone(), base: Some(f.clone()), deleted: false }).collect()
}

/// The open "Pick running process" list: names from a snapshot taken when it opened, and its filter.
pub(crate) struct PickerState {
    pub names: Vec<String>,
    pub filter: String,
}

#[derive(PartialEq, Clone, Copy)]
enum TopoSource {
    Engine,
    Local,
}

fn make_loaded(bytes: Option<&Arc<Vec<u8>>>) -> Result<Loaded, String> {
    let (b, missing) = match bytes {
        Some(b) => (b.as_ref().clone(), false),
        None => (settings::default_file_bytes(), true),
    };
    let doc = Doc::parse(&b)?;
    let (profiles, profiles_bad) = profile_edit::read_profiles(&doc);
    Ok(Loaded { saved: Form::from_doc(&doc), doc, bytes: b, missing, profiles, profiles_bad })
}

pub struct App {
    paths: Paths,
    scope: Scope,
    /// Where `apply-config` writes when no `--config` is given.
    service_config: PathBuf,
    runner: Arc<Runner>,
    shared: Arc<TrayShared>,
    rx: Receiver<Snapshot>,
    snap: Option<Arc<Snapshot>>,
    tab: Tab,

    loaded: Option<Loaded>,
    /// A newer version of the file that arrived while the user had unsaved edits.
    pending: Option<Loaded>,
    load_error: Option<String>,
    form: Form,
    config_version: u64,
    disk_changed_while_dirty: bool,
    save_error: Option<(Option<Field>, String)>,

    profile_rows: Vec<ProfileRow>,
    profile_sel: Option<usize>,
    topo: Option<(Arc<Topology>, TopoSource)>,
    picker: Option<PickerState>,
    /// (profile name, file name) chosen in the Browse dialog, filled in by its worker thread.
    browse_result: Arc<Mutex<Option<(String, String)>>>,
    browse_busy: Arc<AtomicBool>,

    log_filter: String,
    log_follow: bool,
    confirm_uninstall: UninstallConfirm,
}

impl App {
    pub fn new(
        paths: Paths,
        scope: Scope,
        runner: Arc<Runner>,
        shared: Arc<TrayShared>,
        rx: Receiver<Snapshot>,
        service_config: PathBuf,
        start_tab: &str,
    ) -> Self {
        let form = Form::from_doc(&Doc::parse(&settings::default_file_bytes()).expect("default config parses"));
        Self {
            paths,
            scope,
            service_config,
            runner,
            shared,
            rx,
            snap: None,
            tab: match start_tab {
                "settings" => Tab::Settings,
                "profiles" => Tab::Profiles,
                "log" => Tab::Log,
                _ => Tab::Status,
            },
            loaded: None,
            pending: None,
            load_error: None,
            form,
            config_version: 0,
            disk_changed_while_dirty: false,
            save_error: None,
            profile_rows: vec![],
            profile_sel: None,
            topo: None,
            picker: None,
            browse_result: Arc::new(Mutex::new(None)),
            browse_busy: Arc::new(AtomicBool::new(false)),
            log_filter: String::new(),
            log_follow: true,
            confirm_uninstall: UninstallConfirm::default(),
        }
    }

    // ------------------------------------------------------------ data in

    fn ingest(&mut self) {
        let mut latest = None;
        while let Ok(s) = self.rx.try_recv() {
            latest = Some(s);
        }
        let Some(s) = latest else { return };
        let s = Arc::new(s);
        if s.config_version != self.config_version {
            self.config_version = s.config_version;
            self.on_config_changed(&s);
        }
        if let Some(t) = s.engine.status.as_ref().and_then(|st| st.topology.as_ref()) {
            if self.topo.as_ref().is_none_or(|(c, src)| *src != TopoSource::Engine || **c != *t) {
                self.topo = Some((Arc::new(t.clone()), TopoSource::Engine));
            }
        }
        self.snap = Some(s);
    }

    /// Open the standard file dialog on a worker thread (it blocks until the user picks or cancels).
    /// Only the chosen file's *name* is kept; the path is thrown away.
    fn start_browse(&mut self, ctx: &egui::Context, profile: &str) {
        if self.browse_busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let (result, busy, ctx, profile) = (Arc::clone(&self.browse_result), Arc::clone(&self.browse_busy), ctx.clone(), profile.to_string());
        let owner = crate::tray::find_main_window().map_or(0usize, |h| h as usize);
        std::thread::spawn(move || {
            let chosen = crate::picker::browse_for_exe(owner as _).and_then(|p| crate::picker::file_name_of(&p));
            if let (Some(name), Ok(mut slot)) = (chosen, result.lock()) {
                *slot = Some((profile, name));
            }
            busy.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    fn apply_browse_result(&mut self) {
        let picked = self.browse_result.lock().ok().and_then(|mut g| g.take());
        if let Some((profile, name)) = picked {
            if let Some(r) = self.profile_rows.iter_mut().find(|r| !r.deleted && r.form.name == profile) {
                r.form.matcher = name;
            }
        }
    }

    /// The machine's CPU layout: from the engine's status file when it has one, otherwise detected
    /// here (cheap, needs no special rights), otherwise a plain flat layout.
    fn topology(&mut self) -> Arc<Topology> {
        if self.topo.is_none() {
            let t = topology::detect().unwrap_or_else(|_| Topology::simple(std::thread::available_parallelism().map_or(1, |n| n.get())));
            self.topo = Some((Arc::new(t), TopoSource::Local));
        }
        self.topo.as_ref().map(|(t, _)| Arc::clone(t)).expect("set above")
    }

    fn topology_note(&self) -> &'static str {
        match self.topo.as_ref().map(|(_, s)| *s) {
            Some(TopoSource::Engine) => "CPU layout reported by the engine",
            _ => "CPU layout detected locally",
        }
    }

    /// Profile edits as operations on the saved file.
    fn pending_ops(&self) -> Vec<ProfileOp> {
        ops_from_rows(&self.profile_rows)
    }

    fn profile_rows_match(&self, forms: &[ProfileForm]) -> bool {
        let mine: Vec<&ProfileForm> = self.profile_rows.iter().filter(|r| !r.deleted).map(|r| &r.form).collect();
        mine.len() == forms.len() && mine.iter().zip(forms).all(|(a, b)| *a == b)
    }

    /// Problems with profile rows: (row index, field, message).
    fn profile_errors(&mut self) -> Vec<(usize, PField, String)> {
        let topo = self.topology();
        let names: Vec<(usize, String)> = self.profile_rows.iter().enumerate().filter(|(_, r)| !r.deleted).map(|(i, r)| (i, r.form.name.clone())).collect();
        let mut out = vec![];
        for (i, r) in self.profile_rows.iter().enumerate().filter(|(_, r)| !r.deleted) {
            let others: Vec<String> = names.iter().filter(|(j, _)| *j != i).map(|(_, n)| n.clone()).collect();
            out.extend(r.form.errors(&topo, &others).into_iter().map(|(f, m)| (i, f, m)));
        }
        out
    }

    fn on_config_changed(&mut self, s: &Snapshot) {
        let new = match make_loaded(s.config.as_ref()) {
            Ok(l) => l,
            Err(e) => {
                self.load_error = Some(e);
                return;
            }
        };
        self.load_error = None;
        let settings_dirty_old = self.loaded.as_ref().is_some_and(|old| self.form.is_dirty(&old.saved));
        let settings_dirty_new = self.form.is_dirty(&new.saved);
        let profiles_dirty_old = !self.pending_ops().is_empty();
        let profiles_dirty_new = !self.profile_rows_match(&new.profiles);
        let dirty_old = settings_dirty_old || profiles_dirty_old;
        let dirty_new = settings_dirty_new || profiles_dirty_new;
        if !dirty_old || !dirty_new {
            // No unsaved edits, or the edits are exactly what is now on disk (e.g. our own save).
            if !settings_dirty_old {
                self.form = new.saved.clone();
            }
            self.profile_rows = rows_from(&new.profiles);
            self.loaded = Some(new);
            self.pending = None;
            self.disk_changed_while_dirty = false;
            self.save_error = None;
            if self.profile_sel.is_some_and(|i| i >= self.profile_rows.len()) {
                self.profile_sel = None;
            }
        } else {
            // The file changed under unsaved edits: don't silently merge or overwrite.
            self.pending = Some(new);
            self.disk_changed_while_dirty = true;
        }
    }

    fn revert(&mut self) {
        if let Some(p) = self.pending.take() {
            self.loaded = Some(p);
        }
        if let Some(l) = &self.loaded {
            self.form = l.saved.clone();
            self.profile_rows = rows_from(&l.profiles);
        }
        if self.profile_sel.is_some_and(|i| i >= self.profile_rows.len()) {
            self.profile_sel = None;
        }
        self.disk_changed_while_dirty = false;
        self.save_error = None;
    }

    // ------------------------------------------------------------ save

    fn do_save(&mut self) {
        self.save_error = None;
        let Some(loaded) = &self.loaded else { return };
        // Re-read right now: never overwrite a change made since the last poll.
        let on_disk = match std::fs::read(&self.paths.config) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                self.save_error = Some((None, format!("Cannot read the config file to check it before saving: {e}")));
                return;
            }
        };
        let unchanged = match &on_disk {
            Some(b) => *b == loaded.bytes,
            None => loaded.missing,
        };
        if !unchanged {
            self.disk_changed_while_dirty = true;
            self.save_error = Some((None, "The config file changed on disk after you started editing. Revert to reload it, then redo your changes.".into()));
            return;
        }
        let ops = self.pending_ops();
        match profile_edit::build_save_all(&loaded.bytes, &loaded.saved, &self.form, &ops) {
            Ok(bytes) => {
                let dest = (self.paths.config != self.service_config).then(|| self.paths.config.clone());
                self.runner.save(&bytes, dest.as_deref());
            }
            Err(SaveError::NoChanges) => {}
            Err(SaveError::Invalid(_)) => {} // shown inline next to each field
            Err(SaveError::Rejected(msg)) => self.save_error = Some((settings::field_for_error(&msg), msg)),
        }
    }

    // ------------------------------------------------------------ save bar (Settings and Profiles tabs)

    fn save_bar(&mut self, ui: &mut egui::Ui) {
        let saved = match &self.loaded {
            Some(l) => l.saved.clone(),
            None => return,
        };
        let setting_errors = self.form.errors();
        let changed = self.form.changed(&saved);
        let ops = self.pending_ops();
        let profile_errors = self.profile_errors();
        let busy = self.runner.snapshot().busy.is_some();
        if let Some((None, msg)) = &self.save_error {
            ui.colored_label(BAD, format!("Cannot save: {msg}"));
        }
        let nothing = changed.is_empty() && ops.is_empty();
        let blocked: Option<String> = if !setting_errors.is_empty() {
            Some(format!("{} setting(s) have invalid values; fix them first", setting_errors.len()))
        } else if !profile_errors.is_empty() {
            Some(format!("{} profile problem(s); fix them first", profile_errors.len()))
        } else if nothing {
            Some("Nothing has changed".into())
        } else if self.disk_changed_while_dirty {
            Some("The file changed on disk; press Revert first".into())
        } else if !self.runner.helper_present() {
            Some("process-corral.exe was not found next to the GUI".into())
        } else if busy {
            Some("Another action is in progress".into())
        } else {
            None
        };
        ui.horizontal(|ui| {
            let save = ui.add_enabled(blocked.is_none(), egui::Button::new("Save"));
            let save = match &blocked {
                Some(why) => save.on_disabled_hover_text(why),
                None => save.on_hover_text("Validate, then ask Windows for administrator permission to write the config"),
            };
            if save.clicked() {
                self.do_save();
            }
            if ui.add_enabled(!nothing || self.disk_changed_while_dirty, egui::Button::new("Revert")).clicked() {
                self.revert();
            }
            if nothing {
                ui.label(RichText::new("No unsaved changes").color(DIM));
            } else {
                let mut parts: Vec<String> = changed.iter().map(|f| f.key().to_string()).collect();
                if !ops.is_empty() {
                    parts.push(format!("{} profile change(s)", ops.len()));
                }
                ui.label(RichText::new(format!("Unsaved changes: {}", parts.join(", "))).color(WARN));
            }
            if let Some(why) = &blocked {
                if !setting_errors.is_empty() || !profile_errors.is_empty() {
                    ui.colored_label(BAD, why);
                }
            }
        });
    }

    // ------------------------------------------------------------ pieces

    fn header(&mut self, ui: &mut egui::Ui) {
        let snap = self.snap.clone();
        let act = self.runner.snapshot();

        ui.horizontal(|ui| {
            match &snap {
                None => {
                    ui.label(RichText::new("Loading...").color(DIM).heading());
                }
                Some(s) if s.engine.running() => {
                    let mode = s.engine.status.as_ref().map_or("?", |st| st.mode.as_str());
                    dot(ui, GOOD);
                    ui.label(RichText::new(format!("Engine running ({mode} mode)")).color(GOOD).heading());
                }
                Some(s) => {
                    dot(ui, BAD);
                    ui.label(RichText::new("Engine not running").color(BAD).heading());
                    if let Some(why) = &s.engine.not_running {
                        ui.label(RichText::new(why.describe()).color(DIM));
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            let (label, color) = match snap.as_ref().map(|s| s.service) {
                None => ("checking...".to_string(), DIM),
                Some(ServiceState::Running) => ("running".into(), GOOD),
                Some(ServiceState::Stopped) => ("stopped".into(), WARN),
                Some(ServiceState::NotInstalled) => ("not installed".into(), DIM),
                Some(ServiceState::StartPending) => ("starting...".into(), WARN),
                Some(ServiceState::StopPending) => ("stopping...".into(), WARN),
                Some(ServiceState::Other(c)) => (format!("state {c}"), WARN),
                Some(ServiceState::Unknown(e)) => (format!("unknown (error {e})"), BAD),
            };
            ui.label("Windows service 'ProcessCorral':");
            ui.label(RichText::new(label).color(color).strong());
            ui.separator();

            let state = snap.as_ref().map_or(ServiceState::Unknown(0), |s| s.service);
            let b = buttons(state, self.runner.helper_present(), act.busy.is_some());
            if service_button(ui, "Start", b.start) {
                self.runner.service(Command::Start);
            }
            if service_button(ui, "Stop", b.stop) {
                self.runner.service(Command::Stop);
            }
            if let Some(busy) = &act.busy {
                ui.spinner();
                ui.label(format!("{busy}... (approve the Windows prompt)"));
            }
        });

        if !self.runner.helper_present() {
            ui.colored_label(
                BAD,
                match self.runner.helper_path() {
                    Some(p) => format!("process-corral.exe not found at {}. Service control and saving are disabled.", p.display()),
                    None => "process-corral.exe not found next to the GUI. Service control and saving are disabled.".into(),
                },
            );
        }
        if let Some(m) = &act.last {
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(if m.ok { GOOD } else { BAD }, &m.text);
                if ui.small_button("dismiss").clicked() {
                    self.runner.dismiss();
                }
            });
        }
    }

    fn status_tab(&mut self, ui: &mut egui::Ui) {
        let Some(snap) = self.snap.clone() else {
            ui.label("Waiting for the first reading...");
            return;
        };
        let running = snap.engine.running();
        if !running {
            let note = match &snap.engine.not_running {
                Some(NotRunning::Missing) => "No status file has been written yet. Start the service or run `process-corral run`.".to_string(),
                Some(NotRunning::Unreadable(w)) => format!("The status file cannot be read ({w})."),
                Some(other) => format!("Showing the last session's data below (greyed out): {}.", other.describe()),
                None => String::new(),
            };
            ui.colored_label(WARN, note);
            if snap.status_is_cached {
                ui.colored_label(DIM, "(Using the last good reading because the file is unreadable right now.)");
            }
        }
        let Some(st) = snap.engine.status.clone() else { return };

        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            ui.scope(|ui| {
                if !running {
                    ui.disable();
                }
                egui::Grid::new("summary").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                    ui.label("Mode");
                    ui.label(format!("{} (engine {}, pid {})", st.mode, st.engine_version, st.pid));
                    ui.end_row();
                    ui.label("Started");
                    ui.label(pretty_time(&st.started_at));
                    ui.end_row();
                    ui.label("Last update");
                    ui.label(pretty_time(&st.updated_at));
                    ui.end_row();
                    ui.label("Total CPU");
                    ui.add(egui::ProgressBar::new((st.cpu.total_percent / 100.0) as f32).text(format!("{:.1}% of {} cores", st.cpu.total_percent, st.cpu.cores)).desired_width(280.0));
                    ui.end_row();
                    ui.label("Restraints this session");
                    ui.label(st.restraints_this_session.to_string());
                    ui.end_row();
                    ui.label("Config");
                    ui.label(&st.config_path);
                    ui.end_row();
                });

                egui::CollapsingHeader::new("Thresholds in effect").default_open(true).show(ui, |ui| {
                    let t = &st.thresholds;
                    egui::Grid::new("thr").num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
                        let mut row = |k: &str, v: String| {
                            ui.label(k);
                            ui.label(v);
                            ui.end_row();
                        };
                        row("Restraint", if t.restraint_enabled { "on".into() } else { "off".into() });
                        row("Restrain when system CPU above", format!("{}%", t.total_cpu_percent));
                        row("...and process CPU above", format!("{}%", t.per_process_percent));
                        row("...for longer than", format!("{} ms", t.time_over_quota_ms));
                        row("Restore when process CPU below", format!("{}%", t.restore_below_percent));
                        row("Minimum / maximum restraint", format!("{} ms / {}", t.min_restraint_ms, if t.max_restraint_ms == 0 { "no limit".into() } else { format!("{} ms", t.max_restraint_ms) }));
                        row("Only Normal-priority processes", t.tame_only_normal.to_string());
                        row("Lower to Idle (else Below Normal)", t.lower_to_idle.to_string());
                        row("Also restrain by affinity", t.restrain_by_affinity.to_string());
                    });
                });

                ui.separator();
                ui.strong(format!("Restrained processes ({})", st.restrained.len()));
                if st.restrained.is_empty() {
                    ui.label(RichText::new("none right now").color(DIM));
                } else {
                    egui::Grid::new("restrained").striped(true).spacing([16.0, 3.0]).show(ui, |ui| {
                        for h in ["PID", "Name", "Priority", "Restrained for", "CPU at restraint"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for r in &st.restrained {
                            ui.label(r.pid.to_string());
                            ui.label(&r.name);
                            ui.label(format!("{} -> {}", r.original_priority, r.current_priority));
                            ui.label(fmt_duration_ms(r.restrained_for_ms));
                            ui.label(format!("{:.1}%", r.cpu_percent_at_restraint));
                            ui.end_row();
                        }
                    });
                }

                protected_section(ui, &st.untouchable);

                ui.separator();
                ui.strong("Top processes by CPU");
                egui::Grid::new("top").striped(true).spacing([16.0, 3.0]).show(ui, |ui| {
                    for h in ["PID", "Name", "CPU", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for p in &st.top_processes {
                        ui.label(p.pid.to_string());
                        ui.label(&p.name);
                        ui.label(format!("{:.1}%", p.cpu_percent));
                        ui.label(if p.restrained { RichText::new("restrained").color(WARN) } else { RichText::new("") });
                        ui.end_row();
                    }
                });

                self.status_profiles_section(ui, &st);

                ui.separator();
                ui.strong("Recent events");
                for e in st.recent_events.iter().rev() {
                    let k = logview::kind(e.action_id);
                    ui.horizontal_wrapped(|ui| {
                        ui.monospace(e.time.get(11..19).unwrap_or(&e.time));
                        ui.label(RichText::new(k.label()).color(kind_color(k)).monospace());
                        if !e.name.is_empty() && e.pid != 0 {
                            ui.label(format!("{} ({})", e.name, e.pid));
                        }
                        ui.label(&e.action);
                        if !e.info.is_empty() {
                            ui.label(RichText::new(&e.info).color(DIM));
                        }
                    });
                }
            });
        });
    }

    fn status_profiles_section(&self, ui: &mut egui::Ui, st: &process_corral::status::Status) {
        ui.separator();
        ui.strong("Profiles and game mode");
        if let Some(t) = &st.topology_summary {
            let groups: Vec<String> = t
                .l3_groups
                .iter()
                .map(|g| format!("L3 group {} = {:.0} MB{} (CPUs {})", g.index, g.size_mb, if g.label.is_empty() { String::new() } else { format!(", {}", g.label) }, g.cpu_list))
                .collect();
            ui.label(RichText::new(format!("{} logical processors, {} cores{}. {}", t.logical_processors, t.cores, if t.smt { " (SMT)" } else { "" }, groups.join("; "))).color(DIM));
        }
        let g = &st.game_mode;
        if g.active {
            let by = g.triggered_by.as_ref().map(|t| format!("{} (pid {}), profile '{}'", t.process, t.pid, t.profile)).unwrap_or_default();
            ui.colored_label(GOOD, format!("Game mode ACTIVE since {}: {by}. {} background process(es) lowered{}.", g.since.as_deref().map(pretty_time).unwrap_or_default(), g.background_lowered, if g.power_plan_switched { ", power plan switched" } else { "" }));
        } else {
            ui.label(RichText::new("Game mode: not active").color(DIM));
        }
        if st.profiles.is_empty() {
            ui.label(RichText::new("No profiles defined (Profiles tab).").color(DIM));
        } else if st.applied_profiles.is_empty() {
            ui.label(RichText::new(format!("{} profile(s) defined; no matching process is running.", st.profiles.len())).color(DIM));
        }
        if !st.applied_profiles.is_empty() {
            egui::Grid::new("applied_profiles").striped(true).spacing([14.0, 3.0]).show(ui, |ui| {
                for h in ["PID", "Process", "Profile", "Change", "Result"] {
                    ui.strong(h);
                }
                ui.end_row();
                for p in &st.applied_profiles {
                    for c in &p.changes {
                        ui.label(p.pid.to_string());
                        ui.label(&p.process);
                        ui.label(&p.profile);
                        ui.label(format!("{}: {} -> {}", c.aspect, c.original, c.wanted));
                        ui.label(RichText::new(change_text(c)).color(change_color(c)));
                        ui.end_row();
                    }
                }
            });
        }
        if !st.refused.is_empty() {
            ui.colored_label(WARN, format!("{} change(s) were refused by Windows or not possible on this machine:", st.refused.len()));
            for r in &st.refused {
                ui.label(RichText::new(format!("  {} ({}) - {}: {}", r.process, r.pid, r.aspect, r.message)).color(BAD));
            }
        }
    }

    /// Install / Uninstall of the Windows service. The installer normally does this, so it is tucked away.
    fn advanced_section(&mut self, ui: &mut egui::Ui) {
        let state = self.snap.as_ref().map_or(ServiceState::Unknown(0), |s| s.service);
        let b = buttons(state, self.runner.helper_present(), self.runner.snapshot().busy.is_some());
        self.confirm_uninstall.sync(b.uninstall.is_none());
        egui::CollapsingHeader::new("Advanced").id_salt("advanced").default_open(false).show(ui, |ui| {
            ui.label(RichText::new("The installer normally installs and removes the Windows service for you. These buttons are only needed if you run the program files from another folder.").color(DIM));
            if self.confirm_uninstall.asking() {
                // The question gets its own wrapped line so the buttons never fall off a narrow window.
                ui.label(RichText::new(uninstall_prompt()).color(WARN));
                ui.horizontal(|ui| {
                    if service_button(ui, "Yes, uninstall service", b.uninstall) && self.confirm_uninstall.confirm(b.uninstall.is_none()) {
                        self.runner.service(Command::Uninstall);
                    }
                    if ui.button("Cancel").clicked() {
                        self.confirm_uninstall.cancel();
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    if service_button(ui, "Install service", b.install) {
                        self.runner.service(Command::Install);
                    }
                    if service_button(ui, "Uninstall service", b.uninstall) {
                        self.confirm_uninstall.ask(b.uninstall.is_none());
                    }
                });
            }
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("advanced_bar").show(ui, |ui| {
            self.advanced_section(ui);
        });
        ui.colored_label(
            INFO,
            "Saved settings take effect within about 2 seconds (live reload); no restart is needed. Saving asks Windows for administrator permission.",
        );
        if self.scope == Scope::User {
            ui.colored_label(WARN, format!("Editing the per-user console-mode config in {}, not the service's.", self.paths.config.display()));
        }
        if let Some(e) = &self.load_error {
            ui.colored_label(BAD, format!("Cannot edit this config file: {e}"));
            return;
        }
        let Some(loaded) = &self.loaded else {
            ui.label("Waiting for the config file...");
            return;
        };
        if loaded.missing {
            ui.colored_label(WARN, "No config file exists yet. These are the engine's defaults; Save creates the file.");
        }
        if let Some(e) = self.snap.as_ref().and_then(|s| s.config_error.clone()) {
            ui.colored_label(WARN, e);
        }
        if self.disk_changed_while_dirty {
            ui.colored_label(BAD, "The config file changed on disk while you were editing. Saving is disabled; press Revert to load the new version.");
        }
        let lists = settings::readonly_lists(&loaded.doc);
        let saved = loaded.saved.clone();

        let errors = self.form.errors();
        let warnings = self.form.warnings();
        let changed = self.form.changed(&saved);

        egui::Panel::bottom("save_bar").show(ui, |ui| {
            ui.add_space(4.0);
            self.save_bar(ui);
            ui.add_space(4.0);
        });

        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            let groups: [(&str, &[Field]); 4] = [
                ("Engine", &[Field::OocOn]),
                ("When to restrain", &[Field::TotalCpu, Field::PerProcess, Field::TimeOverQuota]),
                (
                    "How to restrain and restore",
                    &[Field::RestorePct, Field::MinRestraint, Field::MaxRestraint, Field::TameOnlyNormal, Field::LowerToIdle, Field::RestrainByAffinity, Field::RestraintAffinity],
                ),
                ("Exemptions", &[Field::ExcludeForeground, Field::ExcludeChildren, Field::ExcludeServices, Field::Exclusions]),
            ];
            for (title, fields) in groups {
                egui::CollapsingHeader::new(title).default_open(true).show(ui, |ui| {
                    egui::Grid::new(title).num_columns(2).min_col_width(340.0).spacing([16.0, 6.0]).show(ui, |ui| {
                        for f in fields {
                            let modified = changed.contains(f);
                            let err = errors.iter().find(|e| e.0 == *f).map(|e| e.1.clone());
                            let warn = warnings.iter().find(|e| e.0 == *f).map(|e| e.1.clone());
                            let save_err = self.save_error.as_ref().filter(|s| s.0 == Some(*f)).map(|s| s.1.clone());

                            ui.horizontal(|ui| {
                                ui.label(if modified { RichText::new("*").color(WARN).strong() } else { RichText::new(" ") });
                                ui.label(f.label());
                            });
                            ui.vertical(|ui| {
                                if f.is_bool() {
                                    if let Some(b) = self.form.bool_mut(*f) {
                                        ui.checkbox(b, "");
                                    }
                                } else if let Some(t) = self.form.text_mut(*f) {
                                    if *f == Field::Exclusions {
                                        ui.add(egui::TextEdit::multiline(t).desired_rows(4).desired_width(320.0));
                                    } else {
                                        let r = ui.add(egui::TextEdit::singleline(t).desired_width(120.0));
                                        if err.is_some() {
                                            r.highlight();
                                        }
                                    }
                                }
                                if let Some(e) = err {
                                    ui.colored_label(BAD, e);
                                }
                                if let Some(e) = save_err {
                                    ui.colored_label(BAD, e);
                                }
                                if let Some(w) = warn {
                                    ui.colored_label(WARN, format!("Note: {w}"));
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
            }

            ui.separator();
            ui.strong("Per-application lists (read-only for now)");
            ui.label(RichText::new("Not edited here. A profile (Profiles tab) takes precedence over these lists for the processes it matches.").color(DIM));
            egui::Grid::new("ro").num_columns(2).min_col_width(340.0).spacing([16.0, 4.0]).show(ui, |ui| {
                for (k, v) in &lists {
                    ui.label(*k);
                    if v.is_empty() {
                        ui.label(RichText::new("(none)").color(DIM));
                    } else {
                        ui.label(RichText::new(v).monospace());
                    }
                    ui.end_row();
                }
            });

        });
    }

    fn log_tab(&mut self, ui: &mut egui::Ui) {
        let Some(snap) = self.snap.clone() else {
            ui.label("Waiting for the first reading...");
            return;
        };
        ui.horizontal(|ui| {
            ui.label("Filter:");
            ui.add(egui::TextEdit::singleline(&mut self.log_filter).hint_text("process, action, text...").desired_width(240.0));
            if ui.small_button("clear").clicked() {
                self.log_filter.clear();
            }
            ui.checkbox(&mut self.log_follow, "Follow newest");
            ui.label(RichText::new(self.paths.log.display().to_string()).color(DIM));
        });
        if let Some(n) = &snap.log_note {
            ui.colored_label(DIM, n);
        }
        let needle = self.log_filter.trim().to_lowercase();
        let rows: Vec<&logview::Row> = snap.log_rows.iter().filter(|r| r.matches(&needle)).collect();
        ui.label(RichText::new(format!("{} of {} entries (times are UTC)", rows.len(), snap.log_rows.len())).color(DIM));
        ui.separator();
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::both()
            .auto_shrink([false; 2])
            .stick_to_bottom(self.log_follow)
            .show_rows(ui, row_h, rows.len(), |ui, range| {
                for r in &rows[range] {
                    let k = logview::kind(r.action_id);
                    ui.horizontal(|ui| {
                        ui.monospace(r.time.get(11..).unwrap_or(&r.time));
                        ui.label(RichText::new(format!("{:<8}", k.label())).color(kind_color(k)).monospace());
                        if r.pid != 0 {
                            ui.monospace(format!("{} ({})", r.name, r.pid));
                        }
                        ui.label(RichText::new(&r.action).color(kind_color(k)));
                        if !r.info.is_empty() {
                            ui.label(RichText::new(&r.info).color(DIM));
                        }
                    });
                }
            });
    }
}

/// A filled status dot (drawn, because the default font has no round-bullet glyph).
fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 7.0, color);
}

/// One-line result of a profile change for display.
pub(crate) fn change_text(c: &process_corral::status::ChangeStatus) -> String {
    match c.result.as_str() {
        "applied" => "applied".into(),
        "applied_but_different" => format!("applied, but differs (now {})", c.current),
        "retrying" => format!("refused, retrying (attempt {})", c.attempts),
        "pending" => "waiting".into(),
        // Short on purpose (it sits in a table cell); the full explanation is the hover text and the
        // refused list on the Status tab.
        _ => match c.win32_error {
            Some(e) => format!("REFUSED (Win32 error {e})"),
            None => format!("not applied: {}", c.message.chars().take(70).collect::<String>()),
        },
    }
}

pub(crate) fn change_color(c: &process_corral::status::ChangeStatus) -> Color32 {
    match c.result.as_str() {
        "applied" => GOOD,
        "applied_but_different" | "retrying" | "pending" => WARN,
        _ => BAD,
    }
}

fn kind_color(k: Kind) -> Color32 {
    match k {
        Kind::Restrain => WARN,
        Kind::Release => GOOD,
        Kind::Exempt => DIM,
        Kind::Error => BAD,
        Kind::Config => INFO,
        Kind::Lifecycle => Color32::from_rgb(200, 160, 235),
        Kind::Rule => Color32::from_rgb(170, 140, 220),
        Kind::Profile => Color32::from_rgb(110, 200, 200),
        Kind::Other => Color32::LIGHT_GRAY,
    }
}

fn pretty_time(iso: &str) -> String {
    format!("{} UTC", iso.trim_end_matches('Z').replace('T', " "))
}

/// A service button: enabled when `why` is None, otherwise greyed out with the reason as its tooltip.
/// Clicking asks Windows for administrator permission (the "runas" path in actions.rs).
fn service_button(ui: &mut egui::Ui, text: &str, why: Option<&str>) -> bool {
    let r = ui.add_enabled(why.is_none(), egui::Button::new(text));
    match why {
        Some(w) => r.on_disabled_hover_text(w).clicked(),
        None => r.on_hover_text("Asks Windows for administrator permission (UAC)").clicked(),
    }
}

/// Processes Windows refused Restraint access to (protected, e.g. by anti-cheat). Shown only when there are any.
fn protected_section(ui: &mut egui::Ui, list: &[process_corral::status::UntouchableProc]) {
    if list.is_empty() {
        return;
    }
    ui.separator();
    ui.strong(format!("Protected, not managed ({})", list.len()));
    ui.label(RichText::new("Windows refused access to these, so Process Corral leaves them alone for as long as they run.").color(DIM));
    for p in list {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("{} (pid {})", p.name, p.pid)).strong());
            ui.label(RichText::new(&p.reason).color(DIM));
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ingest();
        self.apply_browse_result();
        // Also runs while hidden: the tray asked for the window.
        if self.shared.show_requested.swap(false, Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Closing the window hides it to the tray; only File > Exit (or the tray's Exit) quits.
        if ctx.input(|i| i.viewport().close_requested()) && self.shared.tray_ok.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        ctx.request_repaint_after(Duration::from_millis(500));

        egui::Panel::top("menu").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Hide to tray").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                        ui.close();
                    }
                    if ui.button("Exit (the service keeps running)").clicked() {
                        if self.shared.tray_ok.load(Ordering::SeqCst) {
                            self.shared.quit_requested.store(true, Ordering::SeqCst);
                        } else {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        ui.close();
                    }
                });
            });
        });
        egui::Panel::top("header").show(ui, |ui| {
            ui.add_space(4.0);
            self.header(ui);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::Status, "Status");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
                ui.selectable_value(&mut self.tab, Tab::Profiles, "Profiles");
                ui.selectable_value(&mut self.tab, Tab::Log, "Log");
            });
        });
        egui::Panel::bottom("footer").show(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "{}   config: {}   status: {}",
                    if self.scope == Scope::System { "service files" } else { "per-user files" },
                    self.paths.config.display(),
                    self.paths.status.display()
                ))
                .small()
                .color(DIM),
            );
        });
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Status => self.status_tab(ui),
            Tab::Settings => self.settings_tab(ui),
            Tab::Profiles => self.profiles_tab(ui),
            Tab::Log => self.log_tab(ui),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use process_corral::status::ChangeStatus;

    fn form(name: &str) -> ProfileForm {
        let mut f = ProfileForm::new(name);
        f.matcher = format!("{name}.exe");
        f
    }
    fn change(result: &str, err: Option<u32>) -> ChangeStatus {
        ChangeStatus {
            aspect: "affinity".into(),
            wanted: "0x3".into(),
            original: "0xF".into(),
            current: "0xC".into(),
            result: result.into(),
            win32_error: err,
            message: "Windows refused to change the CPU affinity: Access is denied. (Win32 error 5). Access was denied: protected games and anti-cheat software commonly block this, and Process Corral does not try to get around it.".into(),
            attempts: 5,
        }
    }

    #[test]
    fn rows_become_the_right_operations() {
        let saved = form("a");
        let mut edited = saved.clone();
        edited.game_mode = true;
        let rows = vec![
            ProfileRow { form: saved.clone(), base: Some(saved.clone()), deleted: false }, // unchanged: nothing
            ProfileRow { form: edited.clone(), base: Some(saved.clone()), deleted: false }, // edit
            ProfileRow { form: form("gone"), base: Some(form("gone")), deleted: true },     // delete saved
            ProfileRow { form: form("fresh"), base: None, deleted: false },                // add
            ProfileRow { form: form("never"), base: None, deleted: true },                 // added then deleted: nothing
        ];
        let ops = ops_from_rows(&rows);
        assert_eq!(ops.len(), 3);
        assert!(matches!(&ops[0], ProfileOp::Edit { new, .. } if new.game_mode));
        assert!(matches!(&ops[1], ProfileOp::Delete(n) if n == "gone"));
        assert!(matches!(&ops[2], ProfileOp::Add(f) if f.name == "fresh"));
        assert!(ops_from_rows(&rows[..1]).is_empty());
    }

    #[test]
    fn change_results_are_labelled_and_coloured() {
        assert_eq!(change_text(&change("applied", None)), "applied");
        assert_eq!(change_color(&change("applied", None)), GOOD);
        assert!(change_text(&change("applied_but_different", None)).contains("differs (now 0xC)"));
        assert_eq!(change_color(&change("applied_but_different", None)), WARN);
        assert!(change_text(&change("retrying", Some(5))).contains("retrying (attempt 5)"));
        let refused = change_text(&change("refused", Some(5)));
        assert_eq!(refused, "REFUSED (Win32 error 5)", "short enough for a table cell");
        assert_eq!(change_color(&change("refused", Some(5))), BAD);
        assert!(change_text(&change("refused", None)).starts_with("not applied: "));
        assert!(change_text(&change("refused", None)).len() < 100);
    }
}
