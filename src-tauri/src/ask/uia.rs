//! What's selected in the app in front, and the text it shows, read through
//! UI Automation: no keys pressed and nothing put on the clipboard. Apps built on Chromium (VS Code,
//! Teams, the new Outlook, Edge) are left alone: asking them anything turns
//! on their accessibility mode, and VS Code then offers screen-reader mode.

/// Selections past this many characters are cut.
#[cfg(windows)]
const MAX_SELECTION: i32 = 20_000;
/// A control's showing text past this many characters is cut.
#[cfg(windows)]
const MAX_VISIBLE: i32 = 20_000;

/// Programs built on Chromium or Electron, by name without `.exe`.
const CHROMIUM: &[&str] = &[
    "brave",
    "chrome",
    "claude",
    "code",
    "code - insiders",
    "cursor",
    "discord",
    "docker desktop",
    "figma",
    "ms-teams",
    "msedge",
    "msedgewebview2",
    "notion",
    "obsidian",
    "olk",
    "slack",
    "spotify",
];

/// Whether `app` (e.g. `Code`) is one to leave alone.
pub fn is_chromium(app: &str) -> bool {
    CHROMIUM.contains(&app.to_lowercase().as_str())
}

/// UI Automation, for the thread that made it.
#[cfg(windows)]
pub struct Uia(windows::Win32::UI::Accessibility::IUIAutomation);

#[cfg(windows)]
impl Uia {
    /// Puts this thread in COM's multithreaded apartment.
    pub fn new() -> Option<Self> {
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
        };
        use windows::Win32::UI::Accessibility::CUIAutomation;
        // SAFETY: COM is set up on this thread, which alone uses the object.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
                .ok()
                .map(Uia)
        }
    }

    /// The text selected in the focused control: empty when nothing is,
    /// `None` when the control can't say. Never a password box's.
    pub fn selection(&self) -> Option<String> {
        use windows::Win32::UI::Accessibility::{IUIAutomationTextPattern, UIA_TextPatternId};
        // SAFETY: COM calls on live objects, from the thread that made them.
        unsafe {
            let focused = self.0.GetFocusedElement().ok()?;
            if focused.CurrentIsPassword().map_or(true, |p| p.as_bool()) {
                return None;
            }
            let pattern: IUIAutomationTextPattern =
                focused.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
            let ranges = pattern.GetSelection().ok()?;
            let mut text = String::new();
            for i in 0..ranges.Length().ok()? {
                let range = ranges.GetElement(i).ok()?;
                text.push_str(&range.GetText(MAX_SELECTION).ok()?.to_string());
            }
            Some(text)
        }
    }

    /// The text showing in the focused control: a terminal's screen, a
    /// document's page. `None` when the control can't say; never a
    /// password box's.
    pub fn visible_text(&self) -> Option<String> {
        use windows::Win32::UI::Accessibility::{IUIAutomationTextPattern, UIA_TextPatternId};
        // SAFETY: COM calls on live objects, from the thread that made them.
        unsafe {
            let focused = self.0.GetFocusedElement().ok()?;
            if focused.CurrentIsPassword().map_or(true, |p| p.as_bool()) {
                return None;
            }
            let pattern: IUIAutomationTextPattern =
                focused.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
            let ranges = pattern.GetVisibleRanges().ok()?;
            let mut parts = Vec::new();
            for i in 0..ranges.Length().ok()? {
                let range = ranges.GetElement(i).ok()?;
                parts.push(range.GetText(MAX_VISIBLE).ok()?.to_string());
            }
            Some(parts.join("\n"))
        }
    }
}

#[cfg(not(windows))]
pub struct Uia;

#[cfg(not(windows))]
impl Uia {
    pub fn new() -> Option<Self> {
        None
    }

    pub fn selection(&self) -> Option<String> {
        None
    }

    pub fn visible_text(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_apps_are_known_by_name_in_any_case() {
        assert!(is_chromium("Code"));
        assert!(is_chromium("olk"));
        assert!(is_chromium("ms-teams"));
        assert!(!is_chromium("WindowsTerminal"));
        assert!(!is_chromium("notepad"));
    }

    /// Reads whatever is selected in the window in front right now.
    #[cfg(windows)]
    #[test]
    #[ignore = "reads the live desktop"]
    fn live_reads_the_focused_selection() {
        let uia = Uia::new().expect("UI Automation");
        println!("{:?}", uia.selection().map(|s| s.len()));
    }
}
