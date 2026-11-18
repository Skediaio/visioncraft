//! What of the built-in UI an embedding shows: a host app (or an extension) that brings its own
//! chrome hides the tab bar, the file bar, the view bar or the mode panel's frame. Hiding only
//! hides buttons: every action stays a command (`ui.set`, `Ctrl+Z` and the other shortcuts keep
//! working), and agents or the extension can switch the parts back on at run time.

use serde_json::{Value, json};

/// Which parts of the built-in UI are drawn, and where the window starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostUi {
    /// The mode tab bar (Draw, Materials, Objects, Landscape & Sun, Viewpoints, extensions).
    pub tabs: bool,
    /// Open, Save, Undo, Redo.
    pub file_bar: bool,
    /// Layers, Lights, Clay view, Quality, Export, Screenshot, Walk, fps.
    pub view_bar: bool,
    /// The frame of the mode panel with its fold button. When false the open mode's panel
    /// content is drawn bare (no frame, no fold button), so an extension's panel can supply
    /// its own look.
    pub mode_panel_frame: bool,
    /// The mode that is open at start: a built-in id (`"draw"`, `"materials"`, `"objects"`,
    /// `"sun"`, `"viewpoints"`) or an extension's id. `None`: no mode open.
    pub start_mode: Option<&'static str>,
    /// No startup notice when no asset library is found (a host that needs none).
    pub quiet_missing_assets: bool,
}

impl Default for HostUi {
    fn default() -> Self {
        Self {
            tabs: true,
            file_bar: true,
            view_bar: true,
            mode_panel_frame: true,
            start_mode: None,
            quiet_missing_assets: false,
        }
    }
}

impl HostUi {
    /// An explicit choice (the shell's option) wins; otherwise the first extension that has one;
    /// otherwise everything shown.
    pub fn combine(
        explicit: Option<HostUi>,
        extensions: impl IntoIterator<Item = Option<HostUi>>,
    ) -> HostUi {
        explicit
            .or_else(|| extensions.into_iter().flatten().next())
            .unwrap_or_default()
    }

    /// The four visibility flags as `ui.get` reports them.
    pub fn to_json(&self) -> Value {
        json!({
            "tabs": self.tabs,
            "file_bar": self.file_bar,
            "view_bar": self.view_bar,
            "mode_panel_frame": self.mode_panel_frame,
        })
    }

    /// Applies the keys of `ui.set` that name a flag (`tabs`, `file_bar`, `view_bar`,
    /// `mode_panel_frame`); each must be a boolean. Nothing changes when one is not.
    pub fn apply(&mut self, params: &Value) -> Result<(), String> {
        let mut next = self.clone();
        for (key, slot) in [
            ("tabs", &mut next.tabs),
            ("file_bar", &mut next.file_bar),
            ("view_bar", &mut next.view_bar),
            ("mode_panel_frame", &mut next.mode_panel_frame),
        ] {
            if let Some(v) = params.get(key) {
                *slot = v
                    .as_bool()
                    .ok_or_else(|| format!("{key}: true or false, not {v}"))?;
            }
        }
        *self = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::HostUi;
    use serde_json::json;

    fn bare() -> HostUi {
        HostUi {
            tabs: false,
            file_bar: false,
            ..Default::default()
        }
    }

    #[test]
    fn default_shows_everything() {
        let d = HostUi::default();
        assert!(d.tabs && d.file_bar && d.view_bar && d.mode_panel_frame);
        assert_eq!(d.start_mode, None);
        assert!(!d.quiet_missing_assets);
    }

    #[test]
    fn explicit_option_wins_over_extensions() {
        let ext = HostUi {
            view_bar: false,
            ..Default::default()
        };
        assert_eq!(HostUi::combine(Some(bare()), [Some(ext.clone())]), bare());
        assert_eq!(
            HostUi::combine(None, [None, Some(ext.clone()), Some(bare())]),
            ext
        );
        assert_eq!(HostUi::combine(None, [None, None]), HostUi::default());
        assert_eq!(HostUi::combine(None, []), HostUi::default());
    }

    #[test]
    fn ui_set_keys_change_flags_and_ui_get_reports_them() {
        let mut ui = HostUi::default();
        ui.apply(&json!({"tabs": false, "mode_panel_frame": false, "camera": {}}))
            .unwrap();
        assert_eq!(
            ui.to_json(),
            json!({"tabs": false, "file_bar": true, "view_bar": true, "mode_panel_frame": false})
        );
        ui.apply(&json!({"tabs": true})).unwrap();
        assert!(ui.tabs && !ui.mode_panel_frame);
    }

    #[test]
    fn ui_set_rejects_non_booleans_without_changing_anything() {
        let mut ui = HostUi::default();
        let err = ui
            .apply(&json!({"tabs": false, "file_bar": "no"}))
            .unwrap_err();
        assert!(err.contains("file_bar"));
        assert_eq!(ui, HostUi::default());
    }
}
