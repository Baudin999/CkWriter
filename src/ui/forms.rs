//! Unified forms framework.
//!
//! Every form surface in the app — chapter info, entity inspector, relation
//! editor — owns a `Form<T>` instead of hand-rolling its own draft / dirty /
//! Save plumbing. The framework gives each surface:
//!
//! * a `draft: T` the widgets bind to and a snapshot `original: T`,
//! * a derived `dirty()` (`draft != original`),
//! * `rebase_if_clean(live)` — every frame the surface re-asserts the live
//!   value: a no-op while dirty (so the only way to lose work is the discard
//!   prompt) and a copy-from-live while clean (so external updates flow in),
//! * a `render` entry point that draws a title row with a dirty indicator,
//!   the surface's body, and the framework's Save / Revert buttons.
//!
//! The reseed-eats-edits bug from #0019 lands as a structural property of the
//! framework: nothing else mutates the draft in-place.

use crate::theme;
use egui::RichText;

/// Working buffer for a form surface. Lifetime = while the surface is open;
/// surfaces typically store one as `Option<Form<T>>` on `CkWriterApp` and
/// drop it when the corresponding entity / chapter goes away.
#[derive(Debug, Clone)]
pub struct Form<T: Clone + PartialEq> {
    draft: T,
    original: T,
}

impl<T: Clone + PartialEq> Form<T> {
    pub fn new(live: &T) -> Self {
        Self {
            draft: live.clone(),
            original: live.clone(),
        }
    }

    pub fn draft(&self) -> &T {
        &self.draft
    }

    /// Mutable view of the working buffer for surfaces that render inline
    /// (their own title/buttons) rather than going through [`render`]. Use
    /// the closure form on `render` whenever the standard layout fits;
    /// reach for this only when the surface needs custom chrome.
    pub fn draft_mut(&mut self) -> &mut T {
        &mut self.draft
    }

    pub fn original(&self) -> &T {
        &self.original
    }

    pub fn dirty(&self) -> bool {
        self.draft != self.original
    }

    /// If the form is clean, refresh both halves from `live`. While dirty,
    /// this is a no-op — callers that need to *replace* a dirty draft must
    /// route through the shared discard prompt.
    pub fn rebase_if_clean(&mut self, live: &T) {
        if !self.dirty() {
            self.draft = live.clone();
            self.original = live.clone();
        }
    }

    /// Mark the current draft as the saved baseline. The persistence call
    /// itself is the surface's responsibility; the framework only flips the
    /// dirty bit once it's done.
    pub fn mark_saved(&mut self) {
        self.original = self.draft.clone();
    }

    pub fn revert(&mut self) {
        self.draft = self.original.clone();
    }
}

/// Result of one `render` pass — the surface acts on this after the closure
/// returns so persistence work happens outside the egui body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormAction {
    None,
    Save,
    Revert,
}

/// Draw the title row with a dirty indicator, run the surface's body, and
/// render Save / Revert buttons. Returns the user's choice; the surface
/// applies it (typically: persist on Save, `form.revert()` on Revert).
pub fn render<T: Clone + PartialEq, R>(
    form: &mut Form<T>,
    title: &str,
    ui: &mut egui::Ui,
    body: impl FnOnce(&mut egui::Ui, &mut T) -> R,
) -> FormAction {
    title_row(ui, title, form.dirty());
    body(ui, &mut form.draft);

    ui.add_space(6.0);
    let dirty = form.dirty();
    let mut action = FormAction::None;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(dirty, egui::Button::new("Save"))
            .clicked()
        {
            action = FormAction::Save;
        }
        if ui
            .add_enabled(dirty, egui::Button::new("Revert"))
            .clicked()
        {
            action = FormAction::Revert;
        }
    });
    action
}

fn title_row(ui: &mut egui::Ui, title: &str, dirty: bool) {
    ui.horizontal(|ui| {
        ui.heading(title);
        if dirty {
            ui.label(
                RichText::new("●")
                    .color(theme::ACCENT)
                    .strong(),
            )
            .on_hover_text("unsaved changes");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, PartialEq, Default, Debug)]
    struct Sample {
        name: String,
        count: u32,
    }

    #[test]
    fn new_form_is_clean() {
        let live = Sample {
            name: "a".into(),
            count: 1,
        };
        let f = Form::new(&live);
        assert!(!f.dirty());
        assert_eq!(f.draft(), &live);
        assert_eq!(f.original(), &live);
    }

    #[test]
    fn editing_draft_dirties_form() {
        let mut f = Form::new(&Sample::default());
        // Render passes `&mut form.draft` to the body, so tests poke draft
        // through the same private field rather than a public accessor.
        f.draft.name = "edited".into();
        assert!(f.dirty());
    }

    #[test]
    fn revert_restores_original_and_clears_dirty() {
        let mut f = Form::new(&Sample::default());
        f.draft.name = "edited".into();
        assert!(f.dirty());
        f.revert();
        assert!(!f.dirty());
        assert_eq!(f.draft().name, "");
    }

    #[test]
    fn mark_saved_promotes_draft_to_baseline() {
        let mut f = Form::new(&Sample::default());
        f.draft.name = "saved".into();
        f.mark_saved();
        assert!(!f.dirty());
        assert_eq!(f.original().name, "saved");
    }

    #[test]
    fn rebase_if_clean_is_noop_while_dirty() {
        let mut f = Form::new(&Sample {
            name: "orig".into(),
            count: 0,
        });
        f.draft.name = "edited".into();
        let new_live = Sample {
            name: "external".into(),
            count: 99,
        };
        f.rebase_if_clean(&new_live);
        // Dirty edits are preserved; original snapshot is also unchanged.
        assert!(f.dirty());
        assert_eq!(f.draft().name, "edited");
        assert_eq!(f.original().name, "orig");
    }

    #[test]
    fn rebase_if_clean_refreshes_when_clean() {
        let mut f = Form::new(&Sample {
            name: "orig".into(),
            count: 0,
        });
        let new_live = Sample {
            name: "external".into(),
            count: 99,
        };
        f.rebase_if_clean(&new_live);
        assert!(!f.dirty());
        assert_eq!(f.draft(), &new_live);
        assert_eq!(f.original(), &new_live);
    }
}
