//! Small pure helpers for the GUI, kept free of egui/Win32 so they are unit-testable.

use process_corral::scm::ServiceState;

/// Which service buttons are usable. `None` = enabled, `Some(reason)` = disabled with a tooltip.
#[derive(Debug, PartialEq)]
pub struct Buttons {
    pub install: Option<&'static str>,
    pub uninstall: Option<&'static str>,
    pub start: Option<&'static str>,
    pub stop: Option<&'static str>,
}

pub fn buttons(state: ServiceState, helper_present: bool, busy: bool) -> Buttons {
    let blocked = if busy {
        Some("Another action is in progress")
    } else if !helper_present {
        Some("process-corral.exe was not found next to the GUI")
    } else {
        None
    };
    let installed = !matches!(state, ServiceState::NotInstalled | ServiceState::Unknown(_));
    let pick = |ok: bool, why: &'static str| blocked.or(if ok { None } else { Some(why) });
    Buttons {
        install: pick(
            matches!(state, ServiceState::NotInstalled),
            if installed { "The service is already installed" } else { "Cannot query the service state" },
        ),
        uninstall: pick(installed, "The service is not installed"),
        start: pick(
            matches!(state, ServiceState::Stopped),
            match state {
                ServiceState::NotInstalled => "The service is not installed",
                ServiceState::Running | ServiceState::StartPending => "The service is already running",
                _ => "The service cannot be started right now",
            },
        ),
        stop: pick(
            matches!(state, ServiceState::Running | ServiceState::StartPending),
            match state {
                ServiceState::NotInstalled => "The service is not installed",
                ServiceState::Stopped => "The service is not running",
                _ => "The service cannot be stopped right now",
            },
        ),
    }
}

/// What the "Uninstall service" confirmation says: it names the service and says nothing the user
/// cares about is deleted.
pub fn uninstall_prompt() -> String {
    format!(
        "Remove the Windows service '{}'? The engine stops and no longer starts with Windows. Your settings and logs are kept.",
        process_corral::scm::NAME
    )
}

/// The two-step "Uninstall service" button: the first click only asks, the second one (on the
/// confirmation) acts. Pure state, so it can be tested without a window.
#[derive(Debug, Default)]
pub struct UninstallConfirm {
    asking: bool,
}

impl UninstallConfirm {
    pub fn asking(&self) -> bool {
        self.asking
    }

    /// The Uninstall button was clicked. Never uninstalls by itself; ignored while the button is disabled.
    pub fn ask(&mut self, button_enabled: bool) {
        self.asking = button_enabled;
    }

    /// "Yes, uninstall" was clicked. True exactly once, and only if a question was pending and the
    /// button is still enabled (nothing else started in the meantime).
    pub fn confirm(&mut self, button_enabled: bool) -> bool {
        let go = self.asking && button_enabled;
        self.asking = false;
        go
    }

    pub fn cancel(&mut self) {
        self.asking = false;
    }

    /// Call every frame: a pending question is dropped if the button became unavailable.
    pub fn sync(&mut self, button_enabled: bool) {
        if !button_enabled {
            self.asking = false;
        }
    }
}

/// 0 unknown, 1 not installed, 2 stopped, 3 starting, 4 running, 5 stopping, 6 other (for atomics).
pub fn encode_service(s: ServiceState) -> u8 {
    match s {
        ServiceState::Unknown(_) => 0,
        ServiceState::NotInstalled => 1,
        ServiceState::Stopped => 2,
        ServiceState::StartPending => 3,
        ServiceState::Running => 4,
        ServiceState::StopPending => 5,
        ServiceState::Other(_) => 6,
    }
}

pub fn fmt_duration_ms(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => format!("{}.{}s", s, (ms % 1000) / 100),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// A filled circle with a darker rim, as RGBA. Used for the tray icon and the window icon.
pub fn circle_icon(size: u32, rgb: [u8; 3]) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let c = (size as f32 - 1.0) / 2.0;
    let r = size as f32 / 2.0 - 1.0;
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            if d <= r {
                let rim = d > r - 2.0;
                let k = if rim { 0.55 } else { 1.0 };
                let i = ((y * size + x) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[(rgb[0] as f32 * k) as u8, (rgb[1] as f32 * k) as u8, (rgb[2] as f32 * k) as u8, 255]);
            }
        }
    }
    px
}

pub const RUNNING_RGB: [u8; 3] = [60, 180, 75];
pub const STOPPED_RGB: [u8; 3] = [140, 140, 140];

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ServiceState; 7] = [
        ServiceState::NotInstalled,
        ServiceState::Stopped,
        ServiceState::StartPending,
        ServiceState::Running,
        ServiceState::StopPending,
        ServiceState::Other(7),
        ServiceState::Unknown(5),
    ];

    #[test]
    fn buttons_follow_the_service_state() {
        let b = buttons(ServiceState::NotInstalled, true, false);
        assert_eq!((b.install, b.uninstall.is_some(), b.start.is_some(), b.stop.is_some()), (None, true, true, true));
        let b = buttons(ServiceState::Stopped, true, false);
        assert_eq!((b.install.is_some(), b.uninstall, b.start, b.stop.is_some()), (true, None, None, true));
        let b = buttons(ServiceState::Running, true, false);
        assert_eq!((b.install.is_some(), b.uninstall, b.start.is_some(), b.stop), (true, None, true, None));
        let b = buttons(ServiceState::StartPending, true, false);
        assert_eq!(b.stop, None, "can stop a service that is still starting");
        let b = buttons(ServiceState::StopPending, true, false);
        assert!(b.start.is_some() && b.stop.is_some(), "wait for the stop to finish");
    }

    #[test]
    fn nothing_is_enabled_when_busy_or_the_helper_is_missing() {
        for s in ALL {
            for (helper, busy, needle) in [(false, false, "not found"), (true, true, "in progress"), (false, true, "in progress")] {
                let b = buttons(s, helper, busy);
                for reason in [b.install, b.uninstall, b.start, b.stop] {
                    assert!(reason.is_some_and(|r| r.contains(needle)), "{s:?} helper={helper} busy={busy}: {reason:?}");
                }
            }
        }
    }

    #[test]
    fn unknown_state_only_allows_nothing_destructive() {
        let b = buttons(ServiceState::Unknown(5), true, false);
        assert!(b.install.is_some() && b.uninstall.is_some() && b.start.is_some() && b.stop.is_some());
    }

    /// The Install/Uninstall buttons now live in Settings > Advanced; they keep the rules they had in the header.
    #[test]
    fn advanced_install_and_uninstall_keep_their_enabling_rules() {
        use ServiceState::*;
        let ready = |s| buttons(s, true, false);
        assert_eq!(ready(NotInstalled).install, None, "install only when there is no service");
        assert!(ready(NotInstalled).uninstall.is_some());
        for s in [Stopped, Running, StartPending, StopPending, Other(9)] {
            assert_eq!(ready(s).install, Some("The service is already installed"), "{s:?}");
            assert_eq!(ready(s).uninstall, None, "uninstall whenever a service exists: {s:?}");
        }
        assert!(ready(Unknown(5)).install.is_some() && ready(Unknown(5)).uninstall.is_some());
        // Start and Stop (which stay in the header) are unaffected by the move.
        assert_eq!(ready(Stopped).start, None);
        assert_eq!(ready(Running).stop, None);
        // Busy / helper missing disable both, with the reason shown.
        assert!(buttons(Running, true, true).uninstall.is_some_and(|r| r.contains("in progress")));
        assert!(buttons(NotInstalled, false, false).install.is_some_and(|r| r.contains("not found")));
    }

    #[test]
    fn uninstall_needs_a_confirmation_and_runs_once() {
        let mut c = UninstallConfirm::default();
        assert!(!c.asking());
        c.ask(true);
        assert!(c.asking(), "the first click only asks");
        assert!(!c.confirm(false), "button became disabled meanwhile: nothing runs");
        assert!(!c.asking());
        c.ask(true);
        assert!(c.confirm(true), "confirmed");
        assert!(!c.asking());
        assert!(!c.confirm(true), "a second confirm without a new question does nothing");
    }

    #[test]
    fn cancelling_or_losing_the_button_drops_the_question() {
        let mut c = UninstallConfirm::default();
        c.ask(true);
        c.cancel();
        assert!(!c.asking() && !c.confirm(true));
        c.ask(true);
        c.sync(false);
        assert!(!c.asking(), "another action started: the question goes away");
        c.ask(false);
        assert!(!c.asking(), "a disabled button cannot even ask");
        c.ask(true);
        c.sync(true);
        assert!(c.asking(), "still enabled: the question stays");
    }

    #[test]
    fn the_confirmation_names_the_service_and_says_settings_and_logs_are_kept() {
        let p = uninstall_prompt();
        assert!(p.contains("'ProcessCorral'"), "{p}");
        assert!(p.contains("settings and logs are kept"), "{p}");
    }

    #[test]
    fn encoding_is_distinct_per_state() {
        let codes: std::collections::HashSet<_> = ALL.iter().map(|s| encode_service(*s)).collect();
        assert_eq!(codes.len(), ALL.len());
    }

    #[test]
    fn durations() {
        assert_eq!(fmt_duration_ms(0), "0.0s");
        assert_eq!(fmt_duration_ms(5_230), "5.2s");
        assert_eq!(fmt_duration_ms(65_000), "1m 05s");
        assert_eq!(fmt_duration_ms(3_725_000), "1h 02m");
    }

    #[test]
    fn icons_are_square_rgba_with_transparent_corners() {
        for size in [16u32, 32, 64] {
            let px = circle_icon(size, RUNNING_RGB);
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert_eq!(px[3], 0, "corner is transparent");
            let mid = (((size / 2) * size + size / 2) * 4) as usize;
            assert_eq!(&px[mid..mid + 4], &[60, 180, 75, 255]);
        }
        assert_ne!(circle_icon(32, RUNNING_RGB), circle_icon(32, STOPPED_RGB));
    }
}
