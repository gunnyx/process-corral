//! Helpers for the Profiles tab's "Pick running process" list and "Browse..." button, and the
//! plain-English "why wasn't it applied" view. The pure parts are unit-tested; the Win32 file dialog
//! is the only untestable bit.

use process_corral::os::ProcInfo;
use process_corral::status::{AppliedProfile, ChangeStatus};

/// Just the file name of a path: `C:\Program Files\My Game\Game.EXE` -> `Game.EXE`. Handles both
/// slash styles and UNC paths, keeps the original capitalisation and extension (or lack of one), and
/// returns `None` for anything without a file name (empty, or a directory with a trailing slash).
pub fn file_name_of(path: &str) -> Option<String> {
    let name = path.trim().rsplit(['\\', '/']).next()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Distinct names, case-insensitively (the first spelling seen wins), sorted alphabetically
/// ignoring case. Empty names are dropped.
pub fn dedupe_sorted(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = names.into_iter().filter(|n| !n.trim().is_empty() && seen.insert(n.to_lowercase())).collect();
    out.sort_by_key(|n| n.to_lowercase());
    out
}

/// Names containing `needle`, ignoring case. An empty (or blank) filter keeps everything.
pub fn filter_names<'a>(names: &'a [String], needle: &str) -> Vec<&'a String> {
    let n = needle.trim().to_lowercase();
    names.iter().filter(|x| n.is_empty() || x.to_lowercase().contains(&n)).collect()
}

/// The distinct exe names in a process snapshot (the idle pseudo-process is not a program).
pub fn running_names(procs: &[ProcInfo]) -> Vec<String> {
    dedupe_sorted(procs.iter().filter(|p| p.pid != 0).map(|p| p.name.clone()))
}

/// A fresh, non-elevated snapshot of what is running right now (Toolhelp: no special rights).
#[cfg(windows)]
pub fn snapshot_running() -> Vec<String> {
    use process_corral::os::{win::WinOs, Os};
    running_names(&WinOs::new().processes())
}

// ---------------------------------------------------------------- results view

/// What to tell the user about one profile's matching processes.
#[derive(Debug, PartialEq)]
pub enum RuntimeView {
    /// The engine doesn't know this profile (new, unsaved, or the engine is older).
    NotSaved,
    Disabled,
    NotRunning,
    Rows(Vec<(u32, String, ChangeStatus)>),
}

/// `engine_enabled`: whether the engine's own config has this profile enabled (`None` = unknown to
/// the engine). Disabled wins: a disabled profile applies to nothing, whatever the last session did.
pub fn runtime_view(engine_enabled: Option<bool>, applied: &[&AppliedProfile]) -> RuntimeView {
    match engine_enabled {
        None => RuntimeView::NotSaved,
        Some(false) => RuntimeView::Disabled,
        Some(true) if applied.is_empty() => RuntimeView::NotRunning,
        Some(true) => RuntimeView::Rows(applied.iter().flat_map(|p| p.changes.iter().map(|c| (p.pid, p.process.clone(), c.clone()))).collect()),
    }
}

/// The plain-English reason shown for a change that did not go cleanly (taken from status.json).
pub fn reason_of(c: &ChangeStatus) -> Option<String> {
    match c.result.as_str() {
        "applied" => None,
        "refused" => Some(if c.message.trim().is_empty() { "Windows refused the change (no further reason was recorded).".to_string() } else { c.message.trim().to_string() }),
        _ => Some(c.message.trim().to_string()).filter(|m| !m.is_empty()),
    }
}

// ---------------------------------------------------------------- file dialog

/// The standard Windows "Open" dialog filtered to programs. Blocks until the user chooses or
/// cancels, so call it from a worker thread. Returns the chosen full path.
#[cfg(windows)]
pub fn browse_for_exe(owner: windows_sys::Win32::Foundation::HWND) -> Option<String> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let filter: Vec<u16> = "Programs (*.exe)\0*.exe\0All files (*.*)\0*.*\0\0".encode_utf16().collect();
    let title: Vec<u16> = "Select the program this profile should match\0".encode_utf16().collect();
    let mut file = vec![0u16; 32_768];
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.nFilterIndex = 1;
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_HIDEREADONLY;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None; // cancelled (or the dialog failed): leave Match as it was
    }
    let len = file.iter().position(|c| *c == 0).unwrap_or(file.len());
    Some(String::from_utf16_lossy(&file[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn file_name_is_extracted_from_any_kind_of_path() {
        let cases = [
            (r"C:\Program Files\My Game\Game.exe", Some("Game.exe")),
            (r"C:\Users\Jane Doe\Desktop\Cool Game (x64).exe", Some("Cool Game (x64).exe")),
            (r"D:\GAMES\STEAM\HALF-LIFE.EXE", Some("HALF-LIFE.EXE")),
            (r"\\server\share\Games\Quake Live\quake.exe", Some("quake.exe")),
            (r"\\?\C:\Games\x\game.exe", Some("game.exe")),
            ("C:/Games/Forward/game.exe", Some("game.exe")),
            (r"C:\mixed/slashes\both/game.exe", Some("game.exe")),
            (r"C:\Games\noextension", Some("noextension")),
            (r"C:\Games\dotted.name.v2.exe", Some("dotted.name.v2.exe")),
            ("game.exe", Some("game.exe")),
            ("  C:\\a\\game.exe  ", Some("game.exe")),
            (r"C:\Games\folder\", None),
            (r"\\server\share\", None),
            ("", None),
            ("   ", None),
        ];
        for (input, want) in cases {
            assert_eq!(file_name_of(input).as_deref(), want, "{input:?}");
        }
    }

    #[test]
    fn the_extracted_name_never_contains_a_path_separator() {
        for p in [r"C:\a b\c d\e.exe", r"\\h\s\f.exe", "x/y/z.exe", r"C:\a\"] {
            if let Some(n) = file_name_of(p) {
                assert!(!n.contains(['\\', '/']), "{p:?} -> {n:?}");
            }
        }
    }

    #[test]
    fn dedupe_is_case_insensitive_keeps_the_first_spelling_and_sorts() {
        let got = dedupe_sorted(s(&["chrome.exe", "Code.exe", "CHROME.EXE", "alpha.exe", "", "  ", "code.exe", "Zeta.exe"]));
        assert_eq!(got, s(&["alpha.exe", "chrome.exe", "Code.exe", "Zeta.exe"]));
        assert_eq!(dedupe_sorted(Vec::<String>::new()), Vec::<String>::new());
        // Sorted ignoring case: "b" sits between "A" and "c" whatever their capitalisation.
        assert_eq!(dedupe_sorted(s(&["c.exe", "B.exe", "a.exe"])), s(&["a.exe", "B.exe", "c.exe"]));
    }

    #[test]
    fn filter_matches_substrings_ignoring_case_and_empty_keeps_all() {
        let names = s(&["chrome.exe", "Code.exe", "explorer.exe", "msedge.exe"]);
        assert_eq!(filter_names(&names, "").len(), 4);
        assert_eq!(filter_names(&names, "   ").len(), 4);
        assert_eq!(filter_names(&names, "CODE"), vec![&names[1]]);
        assert_eq!(filter_names(&names, "e.exe").len(), 3, "chrome, Code and msedge contain it; explorer does not");
        assert_eq!(filter_names(&names, "xplor"), vec![&names[2]]);
        assert!(filter_names(&names, "nothing").is_empty());
        assert_eq!(filter_names(&names, " edge "), vec![&names[3]], "surrounding spaces are ignored");
    }

    #[test]
    fn a_process_snapshot_becomes_a_clean_sorted_unique_list() {
        let p = |pid, name: &str| ProcInfo { pid, ppid: 1, name: name.into() };
        let snap = vec![p(0, "[System Process]"), p(4, "System"), p(10, "chrome.exe"), p(11, "Chrome.exe"), p(12, "chrome.exe"), p(20, "Code.exe"), p(30, "")];
        assert_eq!(running_names(&snap), s(&["chrome.exe", "Code.exe", "System"]));
    }

    #[cfg(windows)]
    #[test]
    fn the_real_snapshot_lists_this_test_process_without_elevation() {
        let names = snapshot_running();
        assert!(!names.is_empty());
        let me = std::env::current_exe().unwrap();
        let mine = file_name_of(&me.display().to_string()).unwrap();
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case(&mine)), "{mine} not in {} names", names.len());
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(names, sorted);
    }

    // ---- "why wasn't it applied" ------------------------------------------------------------

    fn change(result: &str, msg: &str) -> ChangeStatus {
        ChangeStatus {
            aspect: "priority".into(),
            wanted: "high".into(),
            original: "normal".into(),
            current: "normal".into(),
            result: result.into(),
            win32_error: (result == "refused").then_some(5),
            message: msg.into(),
            attempts: 5,
        }
    }
    fn proc(pid: u32, changes: Vec<ChangeStatus>) -> AppliedProfile {
        AppliedProfile { pid, process: "game.exe".into(), profile: "G".into(), time: String::new(), changes }
    }

    #[test]
    fn view_says_why_nothing_is_shown() {
        assert_eq!(runtime_view(None, &[]), RuntimeView::NotSaved);
        assert_eq!(runtime_view(Some(false), &[]), RuntimeView::Disabled);
        assert_eq!(runtime_view(Some(true), &[]), RuntimeView::NotRunning);
        // A disabled profile is "disabled" even if last session's entries are still in the file.
        let old = proc(1, vec![change("applied", "")]);
        assert_eq!(runtime_view(Some(false), &[&old]), RuntimeView::Disabled);
        // Unknown to the engine wins over stale rows too.
        assert_eq!(runtime_view(None, &[&old]), RuntimeView::NotSaved);
    }

    #[test]
    fn rows_flatten_every_change_of_every_matching_process() {
        let a = proc(1, vec![change("applied", ""), change("refused", "no")]);
        let b = proc(2, vec![change("applied", "")]);
        match runtime_view(Some(true), &[&a, &b]) {
            RuntimeView::Rows(r) => assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 1, 2]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn refusals_show_the_plain_english_reason_from_status() {
        let msg = "Windows refused to change the priority class: Access is denied. (Win32 error 5). Access was denied: protected games and anti-cheat software commonly block this";
        assert_eq!(reason_of(&change("refused", msg)).as_deref(), Some(msg));
        assert!(reason_of(&change("refused", "  ")).unwrap().contains("no further reason"));
        assert_eq!(reason_of(&change("applied", "")), None);
        assert_eq!(reason_of(&change("applied_but_different", "wanted 0x3, now 0xC")).as_deref(), Some("wanted 0x3, now 0xC"));
        assert_eq!(reason_of(&change("retrying", "")), None);
    }
}
