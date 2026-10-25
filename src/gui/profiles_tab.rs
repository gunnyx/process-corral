//! The Profiles tab: a list of per-application profiles on the left, an editor on the right with a
//! CPU picker built from the detected topology, and (from status.json) what the engine actually
//! did to each running matching process.

use super::*;
use process_corral::config::{ABOVE_NORMAL, BELOW_NORMAL, HIGH, IDLE, NORMAL};
use process_corral::profile::{Mech, Smt};
use process_corral::profile_edit::CpuChoice;
use process_corral::topology::GroupRef;
use crate::picker::{self, RuntimeView};
use std::collections::{BTreeMap, BTreeSet};

const PRIORITIES: [(Option<u32>, &str); 6] = [
    (None, "don't change"),
    (Some(IDLE), "Idle"),
    (Some(BELOW_NORMAL), "Below normal"),
    (Some(NORMAL), "Normal"),
    (Some(ABOVE_NORMAL), "Above normal"),
    (Some(HIGH), "High"),
];

fn prio_label(p: Option<u32>) -> &'static str {
    PRIORITIES.iter().find(|(v, _)| *v == p).map_or("?", |(_, l)| l)
}

impl App {
    pub(super) fn profiles_tab(&mut self, ui: &mut egui::Ui) {
        ui.colored_label(INFO, "Saved profiles take effect within about 2 seconds (live reload); no restart is needed. Saving asks Windows for administrator permission.");
        ui.colored_label(
            WARN,
            "Some games and anti-cheat software refuse changes to their priority, CPU affinity or CPU Sets. Process Corral asks Windows, retries a few times, and never tries to get around a refusal. Anything refused is listed under each profile below and on the Status tab.",
        );
        if self.scope == Scope::User {
            ui.colored_label(WARN, format!("Editing the per-user console-mode config in {}, not the service's.", self.paths.config.display()));
        }
        if let Some(e) = &self.load_error {
            ui.colored_label(BAD, format!("Cannot edit this config file: {e}"));
            return;
        }
        if self.loaded.is_none() {
            ui.label("Waiting for the config file...");
            return;
        }
        if self.disk_changed_while_dirty {
            ui.colored_label(BAD, "The config file changed on disk while you were editing. Saving is disabled; press Revert to load the new version.");
        }
        let bad = self.loaded.as_ref().map(|l| l.profiles_bad.clone()).unwrap_or_default();
        for (name, why) in &bad {
            ui.colored_label(BAD, format!("[Profile.{name}] in the file cannot be read ({why}). It is left untouched; fix it by hand."));
        }

        if self.profile_sel.is_none() {
            self.profile_sel = self.profile_rows.iter().position(|r| !r.deleted);
        }
        let topo = self.topology();
        let errs = self.profile_errors();

        egui::Panel::bottom("profiles_save_bar").show(ui, |ui| {
            ui.add_space(4.0);
            self.save_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::left("profiles_list").default_size(240.0).show(ui, |ui| self.profile_list(ui, &errs));
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| self.profile_editor(ui, &topo, &errs));
        });
    }

    fn unique_profile_name(&self, base: &str) -> String {
        let taken = |n: &str| self.profile_rows.iter().any(|r| !r.deleted && r.form.name.eq_ignore_ascii_case(n));
        if !taken(base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).expect("infinite range")
    }

    fn profile_list(&mut self, ui: &mut egui::Ui, errs: &[(usize, PField, String)]) {
        let sel_ok = self.profile_sel.is_some_and(|i| self.profile_rows.get(i).is_some_and(|r| !r.deleted));
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add").clicked() {
                let mut f = ProfileForm::new(&self.unique_profile_name("New profile"));
                f.priority = None;
                self.profile_rows.push(ProfileRow { form: f, base: None, deleted: false });
                self.profile_sel = Some(self.profile_rows.len() - 1);
            }
            if ui.add_enabled(sel_ok, egui::Button::new("Duplicate")).clicked() {
                if let Some(i) = self.profile_sel {
                    let mut f = self.profile_rows[i].form.clone();
                    f.name = self.unique_profile_name(&format!("{} copy", f.name));
                    self.profile_rows.push(ProfileRow { form: f, base: None, deleted: false });
                    self.profile_sel = Some(self.profile_rows.len() - 1);
                }
            }
            if ui.add_enabled(sel_ok, egui::Button::new("Delete")).on_hover_text("Removed from the file when you press Save").clicked() {
                if let Some(i) = self.profile_sel.take() {
                    if self.profile_rows[i].base.is_none() {
                        self.profile_rows.remove(i); // never saved: just forget it
                    } else {
                        self.profile_rows[i].deleted = true;
                    }
                }
            }
        });
        ui.separator();
        let mut clicked = None;
        let sel = self.profile_sel;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, row) in self.profile_rows.iter_mut().enumerate() {
                if row.deleted {
                    continue;
                }
                let modified = row.base.as_ref().is_some_and(|b| *b != row.form);
                let has_err = errs.iter().any(|e| e.0 == i);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut row.form.enabled, "").on_hover_text("Enabled");
                    let tag = if row.base.is_none() { " (new)" } else if modified { " *" } else { "" };
                    let mut text = RichText::new(format!("{}{tag}", row.form.name));
                    if has_err {
                        text = text.color(BAD);
                    } else if !row.form.enabled {
                        text = text.color(DIM);
                    }
                    if ui.selectable_label(sel == Some(i), text).clicked() {
                        clicked = Some(i);
                    }
                });
            }
            if self.profile_rows.iter().all(|r| r.deleted) {
                ui.label(RichText::new("No profiles yet. Press Add.").color(DIM));
            }
        });
        if clicked.is_some() {
            self.profile_sel = clicked;
        }
    }

    fn profile_editor(&mut self, ui: &mut egui::Ui, topo: &Arc<Topology>, errs: &[(usize, PField, String)]) {
        let Some(i) = self.profile_sel.filter(|i| self.profile_rows.get(*i).is_some_and(|r| !r.deleted)) else {
            ui.label(RichText::new("Select a profile on the left, or press Add.").color(DIM));
            return;
        };
        let is_new = self.profile_rows[i].base.is_none();
        let my: Vec<(PField, String)> = errs.iter().filter(|e| e.0 == i).map(|e| (e.1, e.2.clone())).collect();
        let err_of = |f: PField| my.iter().find(|e| e.0 == f).map(|e| e.1.clone());
        let note = self.topology_note();
        let name = self.profile_rows[i].form.name.clone();

        let (mut want_pick, mut want_browse) = (false, false);
        {
            let f = &mut self.profile_rows[i].form;
            egui::Grid::new("profile_basic").num_columns(2).min_col_width(150.0).spacing([16.0, 8.0]).show(ui, |ui| {
                ui.label("Name");
                ui.vertical(|ui| {
                    if is_new {
                        ui.add(egui::TextEdit::singleline(&mut f.name).desired_width(240.0));
                    } else {
                        ui.label(RichText::new(&f.name).strong());
                    }
                    if let Some(e) = err_of(PField::Name) {
                        ui.colored_label(BAD, e);
                    }
                });
                ui.end_row();

                ui.label("Match (program)");
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut f.matcher).hint_text("game.exe   or   C:\\Games\\*\\game.exe").desired_width(260.0));
                        want_pick = ui.button("Pick running process").on_hover_text("Choose from the programs running right now").clicked();
                        want_browse = ui.button("Browse...").on_hover_text("Choose an .exe file; only its file name is used").clicked();
                    });
                    ui.label(RichText::new("Wildcards * and ? work. Text containing a backslash is matched against the full path.").small().color(DIM));
                    if let Some(e) = err_of(PField::Match) {
                        ui.colored_label(BAD, e);
                    }
                });
                ui.end_row();

                ui.label("Priority");
                egui::ComboBox::from_id_salt("profile_priority").selected_text(prio_label(f.priority)).show_ui(ui, |ui| {
                    for (v, l) in PRIORITIES {
                        ui.selectable_value(&mut f.priority, v, l);
                    }
                });
                ui.end_row();

                ui.label("Game mode");
                ui.checkbox(&mut f.game_mode, "While this program runs, enter game mode ([GameMode] in the config sets what that does)");
                ui.end_row();

                ui.label("Restraint");
                ui.checkbox(&mut f.exclude_restraint, "Never restrain this program");
                ui.end_row();
            });
        }

        if want_pick {
            // A fresh snapshot every time the list opens.
            self.picker = if self.picker.is_some() { None } else { Some(PickerState { names: picker::snapshot_running(), filter: String::new() }) };
        }
        if want_browse {
            let ctx = ui.ctx().clone();
            self.start_browse(&ctx, &name);
        }
        if let Some(st) = &mut self.picker {
            let mut chosen: Option<String> = None;
            let mut close = false;
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong("Running programs");
                    ui.label("Filter:");
                    ui.add(egui::TextEdit::singleline(&mut st.filter).hint_text("type to narrow the list").desired_width(180.0));
                    if ui.small_button("Refresh").clicked() {
                        st.names = picker::snapshot_running();
                    }
                    if ui.small_button("Close").clicked() {
                        close = true;
                    }
                });
                let shown = picker::filter_names(&st.names, &st.filter);
                ui.label(RichText::new(format!("{} of {} programs; click one to use its name", shown.len(), st.names.len())).small().color(DIM));
                egui::ScrollArea::vertical().max_height(180.0).id_salt("picker_list").show(ui, |ui| {
                    for n in shown {
                        if ui.selectable_label(false, n).clicked() {
                            chosen = Some(n.clone());
                        }
                    }
                });
            });
            if let Some(n) = chosen {
                self.profile_rows[i].form.matcher = n;
                self.picker = None;
            } else if close {
                self.picker = None;
            }
        }

        ui.separator();
        ui.strong("CPUs");
        let n = topo.logical_count();
        let vcache = topo.vcache_group().is_some();
        let freq = !topo.frequency_groups().is_empty();
        ui.label(RichText::new(format!("{n} logical processors, {} cores{}. {note}.", topo.core_count(), if topo.has_smt() { " (SMT)" } else { "" })).small().color(DIM));
        {
            let f = &mut self.profile_rows[i].form;
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(vcache, egui::Button::new("V-Cache group")).on_disabled_hover_text("No L3 group on this machine is larger than the others").clicked() {
                    f.cpu = CpuChoice::Group(GroupRef::VCache);
                }
                if ui.add_enabled(freq, egui::Button::new("Frequency group")).on_disabled_hover_text("No smaller L3 group on this machine").clicked() {
                    f.cpu = CpuChoice::Group(GroupRef::Frequency);
                }
                if ui.button("All cores").clicked() {
                    f.cpu = CpuChoice::Cpus((0..n).collect());
                }
                if ui.button("No CPU change").clicked() {
                    f.cpu = CpuChoice::None;
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Apply as:");
                ui.radio_value(&mut f.mech, Mech::Affinity, "Affinity (a hard limit)");
                ui.radio_value(&mut f.mech, Mech::CpuSets, "CPU Sets (a softer preference; works across processor groups)");
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("Threads:");
                ui.radio_value(&mut f.smt, Smt::All, "All hardware threads");
                ui.radio_value(&mut f.smt, Smt::NoSmt, "One thread per core (no SMT)");
            });
        }

        let f = self.profile_rows[i].form.clone();
        let resolved = f.resolved_cpus(topo);
        ui.horizontal_wrapped(|ui| {
            ui.label("Resulting mask:");
            ui.monospace(f.mask_text(topo));
            if let Ok(c) = &resolved {
                if !c.is_empty() {
                    ui.label(RichText::new(format!("(CPUs {})", Topology::cpu_list(c))).color(DIM));
                }
            }
        });
        match &f.cpu {
            CpuChoice::Group(g) => {
                ui.label(RichText::new(format!("Chosen by name ('{g}'): resolved from the detected topology when applied, so it follows the right cores on any machine.")).small().color(DIM));
            }
            CpuChoice::None if f.smt == Smt::All => {
                ui.label(RichText::new("No CPU restriction: tick cores below or use a quick button.").small().color(DIM));
            }
            _ => {}
        }
        if let Some(e) = err_of(PField::Cpu) {
            ui.colored_label(BAD, e);
        }

        // Per-core checkboxes, grouped by L3 cache group.
        let mut set: BTreeSet<usize> = resolved.unwrap_or_default().into_iter().collect();
        let mut touched = false;
        let groups: Vec<(String, Vec<usize>)> = if topo.l3.is_empty() {
            vec![("All processors".to_string(), (0..n).collect())]
        } else {
            topo.l3
                .iter()
                .map(|g| {
                    let tag = match topo.label(g.index) {
                        "vcache" => " - V-Cache",
                        "frequency" => " - Frequency",
                        _ => "",
                    };
                    (format!("L3 group {} - {:.0} MB{tag}", g.index, Topology::size_mb(g)), g.cpus.clone())
                })
                .collect()
        };
        for (title, cpus) in groups {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong(&title);
                    if ui.small_button("all").clicked() {
                        set.extend(cpus.iter().copied());
                        touched = true;
                    }
                    if ui.small_button("none").clicked() {
                        for c in &cpus {
                            set.remove(c);
                        }
                        touched = true;
                    }
                });
                let mut cores: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
                for c in &cpus {
                    cores.entry(topo.cpus[*c].core).or_default().push(*c);
                }
                let cores: Vec<(usize, Vec<usize>)> = cores.into_iter().collect();
                // Four cores per row so the layout never runs off the edge, whatever the window width.
                for row in cores.chunks(4) {
                    ui.horizontal(|ui| {
                        for (core, members) in row {
                            ui.allocate_ui_with_layout(egui::vec2(150.0, 20.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.label(RichText::new(format!("core {core:>2}")).small().color(DIM));
                                for c in members {
                                    let mut on = set.contains(c);
                                    if ui.checkbox(&mut on, c.to_string()).on_hover_text(format!("logical processor {c}")).changed() {
                                        if on {
                                            set.insert(*c);
                                        } else {
                                            set.remove(c);
                                        }
                                        touched = true;
                                    }
                                }
                            });
                        }
                    });
                }
            });
        }
        if touched {
            self.profile_rows[i].form.cpu = CpuChoice::Cpus(set.into_iter().collect());
        }

        ui.separator();
        self.profile_runtime(ui, &name);
    }

    /// What the engine actually did to each running process this profile matches, or why nothing.
    fn profile_runtime(&self, ui: &mut egui::Ui, name: &str) {
        ui.strong("Running processes matching this profile");
        let Some(snap) = &self.snap else {
            ui.label(RichText::new("waiting for the engine's status...").color(DIM));
            return;
        };
        let running = snap.engine.running();
        let Some(st) = &snap.engine.status else {
            ui.label(RichText::new("No status from the engine yet.").color(DIM));
            return;
        };
        if st.schema < 2 {
            ui.colored_label(WARN, "The running engine is older than this GUI and does not report profile results. Update process-corral.exe (stop the service, copy both exes, start it).");
            return;
        }
        if !running {
            ui.colored_label(WARN, "Engine not running: showing the last session.");
        }
        let engine_enabled = st.profiles.iter().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| p.enabled);
        let mine: Vec<_> = st.applied_profiles.iter().filter(|p| p.profile.eq_ignore_ascii_case(name)).collect();
        match picker::runtime_view(engine_enabled, &mine) {
            RuntimeView::NotSaved => {
                ui.label(RichText::new("not applied yet: the engine doesn't know this profile until you Save it").color(DIM));
            }
            RuntimeView::Disabled => {
                ui.colored_label(WARN, "profile disabled (tick it in the list and Save to apply it)");
            }
            RuntimeView::NotRunning => {
                ui.label(RichText::new("not running (no program matching this profile is running)").color(DIM));
            }
            RuntimeView::Rows(rows) => {
                ui.scope(|ui| {
                    if !running {
                        ui.disable();
                    }
                    egui::Grid::new("profile_runtime").striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                        for h in ["PID", "Process", "Change", "Original -> now", "Result"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for (pid, process, c) in &rows {
                            ui.label(pid.to_string());
                            ui.label(process);
                            ui.label(&c.aspect);
                            ui.label(format!("{} -> {}", c.original, c.current));
                            ui.label(RichText::new(change_text(c)).color(change_color(c)));
                            ui.end_row();
                        }
                    });
                    // The plain-English reason for anything that did not go cleanly, straight from the engine.
                    for (pid, process, c) in &rows {
                        if let Some(why) = picker::reason_of(c) {
                            ui.colored_label(change_color(c), format!("{process} ({pid}), {}: {why}", c.aspect));
                        }
                    }
                });
            }
        }
    }
}
