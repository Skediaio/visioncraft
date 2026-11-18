//! What of the built-in UI an embedding shows: a host app (or an extension) that brings its own
//! chrome hides the tab bar, the file bar, the view bar or the mode panel's frame. Hiding only
//! hides buttons: every action stays a command (`ui.set`, `Ctrl+Z` and the other shortcuts keep
//! working), and agents or the extension can switch the parts back on at run time.

use crate::strings;
use serde_json::{Value, json};

/// The question asked before an open or a new project replaces unsaved work. A host that
/// brings its own wording (or its own language) sets it with [`HostUi::replace_prompt`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacePrompt {
    /// A heading above the text; empty: none.
    pub title: String,
    pub body: String,
    /// The button that goes on and replaces the project.
    pub confirm: String,
    /// The button that keeps the project.
    pub cancel: String,
}

impl Default for ReplacePrompt {
    /// The built-in question (no title).
    fn default() -> Self {
        Self {
            title: String::new(),
            body: strings::REPLACE_PROJECT.into(),
            confirm: strings::REPLACE_ANYWAY.into(),
            cancel: strings::CANCEL.into(),
        }
    }
}

impl ReplacePrompt {
    fn to_json(&self) -> Value {
        json!({"title": self.title, "body": self.body, "confirm": self.confirm, "cancel": self.cancel})
    }

    /// From `ui.set`'s object; keys it leaves out keep the built-in text.
    fn from_json(v: &Value) -> Result<Self, String> {
        let o = v
            .as_object()
            .ok_or_else(|| format!("replace_prompt: an object or null, not {v}"))?;
        let mut out = Self::default();
        for (key, slot) in [
            ("title", &mut out.title),
            ("body", &mut out.body),
            ("confirm", &mut out.confirm),
            ("cancel", &mut out.cancel),
        ] {
            if let Some(t) = o.get(key) {
                *slot = t
                    .as_str()
                    .ok_or_else(|| format!("replace_prompt.{key}: text, not {t}"))?
                    .to_string();
            }
        }
        if let Some(k) = o
            .keys()
            .find(|k| !["title", "body", "confirm", "cancel"].contains(&k.as_str()))
        {
            return Err(format!(
                "replace_prompt: unknown key `{k}` (title, body, confirm, cancel)"
            ));
        }
        Ok(out)
    }
}

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
    /// The *Allow AI agents* box (desktop) or *Experimental: browser AI agent (WebMCP)* box
    /// (browser), top right, with what the last agent did. Hiding it only hides the box: it
    /// does not switch the agent bridge on or off, and the questions the browser agent asks
    /// still appear.
    pub agent_toggle: bool,
    /// The wording of the question before an open or new replaces unsaved work; `None`: the
    /// built-in one. A caller that has asked already opens with
    /// [`ExtensionCx::open_project`](crate::ExtensionCx::open_project), which does not ask.
    pub replace_prompt: Option<ReplacePrompt>,
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
            agent_toggle: true,
            replace_prompt: None,
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

    /// The visibility flags and the replace prompt as `ui.get` reports them.
    pub fn to_json(&self) -> Value {
        json!({
            "tabs": self.tabs,
            "file_bar": self.file_bar,
            "view_bar": self.view_bar,
            "mode_panel_frame": self.mode_panel_frame,
            "agent_toggle": self.agent_toggle,
            "replace_prompt": self.replace_prompt.as_ref().map(ReplacePrompt::to_json),
        })
    }

    /// Applies the keys of `ui.set` that name a flag (`tabs`, `file_bar`, `view_bar`,
    /// `mode_panel_frame`, `agent_toggle`), each a boolean, and `replace_prompt` (an object with
    /// `title`, `body`, `confirm`, `cancel`, or null for the built-in text). Nothing changes
    /// when one is not valid.
    pub fn apply(&mut self, params: &Value) -> Result<(), String> {
        let mut next = self.clone();
        for (key, slot) in [
            ("tabs", &mut next.tabs),
            ("file_bar", &mut next.file_bar),
            ("view_bar", &mut next.view_bar),
            ("mode_panel_frame", &mut next.mode_panel_frame),
            ("agent_toggle", &mut next.agent_toggle),
        ] {
            if let Some(v) = params.get(key) {
                *slot = v
                    .as_bool()
                    .ok_or_else(|| format!("{key}: true or false, not {v}"))?;
            }
        }
        if let Some(v) = params.get("replace_prompt") {
            next.replace_prompt = match v {
                Value::Null => None,
                v => Some(ReplacePrompt::from_json(v)?),
            };
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
        assert!(d.agent_toggle);
        assert_eq!(d.replace_prompt, None);
    }

    #[test]
    fn agent_toggle_is_a_ui_set_key_and_ui_get_reports_it() {
        let mut ui = HostUi::default();
        assert_eq!(ui.to_json()["agent_toggle"], json!(true));
        ui.apply(&json!({"agent_toggle": false})).unwrap();
        assert!(!ui.agent_toggle && ui.tabs);
        assert_eq!(ui.to_json()["agent_toggle"], json!(false));
        assert!(ui.apply(&json!({"agent_toggle": "off"})).is_err());
        assert!(!ui.agent_toggle);
    }

    #[test]
    fn replace_prompt_is_set_read_and_cleared_through_ui_set() {
        let mut ui = HostUi::default();
        assert_eq!(ui.to_json()["replace_prompt"], json!(null));
        ui.apply(&json!({"replace_prompt": {"title": "Open?", "body": "Sure?", "confirm": "Yes"}}))
            .unwrap();
        let p = ui.replace_prompt.clone().unwrap();
        assert_eq!((p.title.as_str(), p.body.as_str()), ("Open?", "Sure?"));
        assert_eq!(p.confirm, "Yes");
        assert_eq!(p.cancel, super::ReplacePrompt::default().cancel);
        assert_eq!(ui.to_json()["replace_prompt"]["confirm"], json!("Yes"));
        assert!(
            ui.apply(&json!({"replace_prompt": {"bogus": "x"}}))
                .is_err()
        );
        assert!(ui.apply(&json!({"replace_prompt": 3})).is_err());
        assert_eq!(ui.replace_prompt, Some(p));
        ui.apply(&json!({"replace_prompt": null})).unwrap();
        assert_eq!(ui.replace_prompt, None);
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
            json!({"tabs": false, "file_bar": true, "view_bar": true, "mode_panel_frame": false,
                "agent_toggle": true, "replace_prompt": null})
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
