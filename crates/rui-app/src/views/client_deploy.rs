//! The Deploy tab of the client drawer: the command that deploys this client's work and where it goes.
//! The daemon has no read call for it, so the tab shows what was saved this session and a save replaces what is set.

use super::clients::{field, form_buttons, labeled, radio_row};
use super::library::{self as lib, run, toast};
use super::{chip, primary_button};
use crate::api;
use crate::store::Store;
use crate::text_input::TextInput;
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Entity, Render, SharedString, Window, div, prelude::*, px};

const SUBTITLE: &str = "Every deploy waits for your OK in the Inbox.";
const EMPTY: &str = "No deploy command saved yet.";

/// Where the command deploys to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Env {
    Production,
    Staging,
}

impl Env {
    const ALL: [Env; 2] = [Env::Production, Env::Staging];

    /// The daemon's `environment` value.
    fn key(self) -> &'static str {
        match self {
            Env::Production => "production",
            Env::Staging => "staging",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Env::Production => "Production",
            Env::Staging => "Staging",
        }
    }
}

/// The command to save, or what is wrong with it. The daemon wants one non-empty line.
pub fn command_of(text: &str) -> Result<String, String> {
    let cmd = text.trim();
    if cmd.is_empty() {
        return Err("Type the deploy command".into());
    }
    if cmd.contains('\n') {
        return Err("Keep the command on one line".into());
    }
    Ok(cmd.to_string())
}

/// The deploy section of the client drawer.
pub struct DeployPanel {
    store: Entity<Store>,
    client: Option<i64>,
    env: Env,
    command: Entity<TextInput>,
    busy: bool,
    adding: bool,
    /// Command and environment saved from this drawer.
    current: Option<(String, Env)>,
}

impl DeployPanel {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let command = cx.new(|cx| TextInput::new("Command, like \u{201c}make deploy\u{201d}", cx));
        Self { store, client: None, env: Env::Staging, command, busy: false, adding: false, current: None }
    }

    /// Show the form for a client, cleared.
    pub fn open(&mut self, id: i64, cx: &mut Context<Self>) {
        (self.client, self.env, self.busy) = (Some(id), Env::Staging, false);
        (self.adding, self.current) = (false, None);
        self.command.update(cx, |i, cx| i.clear(cx));
        cx.notify();
    }

    fn set_adding(&mut self, on: bool, cx: &mut Context<Self>) {
        self.adding = on;
        if !on {
            self.command.update(cx, |i, cx| i.clear(cx));
        }
        cx.notify();
    }

    fn pick(&mut self, env: Env, cx: &mut Context<Self>) {
        self.env = env;
        cx.notify();
    }

    /// Check the command, then save it with the picked environment.
    fn save(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.client else { return };
        if self.busy {
            return;
        }
        let cmd = match command_of(&self.command.read(cx).content()) {
            Ok(c) => c,
            Err(e) => return toast(&self.store, "Could not save the deploy", &e, cx),
        };
        let env = self.env;
        self.busy = true;
        let shown = cmd.clone();
        run(cx, move || api::set_deploy_config(id, &cmd, env.key()), move |v, res, cx| v.saved(shown.clone(), env, res, cx));
        cx.notify();
    }

    fn saved(&mut self, cmd: String, env: Env, res: Result<(), String>, cx: &mut Context<Self>) {
        self.busy = false;
        match res {
            Ok(()) => {
                (self.current, self.adding) = (Some((cmd, env)), false);
                self.command.update(cx, |i, cx| i.clear(cx));
                toast(&self.store, "Deploy saved", env.label(), cx);
            }
            Err(e) => toast(&self.store, "Could not save the deploy", &e, cx),
        }
        cx.notify();
    }

    /// The environments as a single-select list.
    fn choices(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let option = |env: Env, cx: &mut Context<Self>| {
            let id = SharedString::from(format!("deploy-env-{}", env.key()));
            let el = radio_row(id, env.label(), env == self.env, th);
            el.on_click(cx.listener(move |this, _, _, cx| this.pick(env, cx))).into_any_element()
        };
        let items: Vec<AnyElement> = Env::ALL.into_iter().map(|e| option(e, cx)).collect();
        div().flex().flex_col().gap(px(6.)).children(items).into_any_element()
    }

    /// What is saved, as text, or the empty line.
    fn shown(&self, th: &Theme) -> AnyElement {
        let Some((cmd, env)) = &self.current else { return lib::note(EMPTY, th).into_any_element() };
        let cmd = div().font_family("Menlo").text_size(px(12.5)).text_color(th.fg).child(cmd.clone());
        let env = lib::note(env.label(), th);
        let col = div().flex().flex_col().gap(px(4.)).p(px(12.)).rounded(px(10.)).bg(th.well);
        col.child(labeled("Command", cmd.into_any_element(), th)).child(labeled("Environment", env.into_any_element(), th)).into_any_element()
    }

    fn form(&self, th: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let label = if self.busy { "Saving\u{2026}" } else { "Save" };
        let save = primary_button("deploy-save", label, cx).on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        let cancel = chip("deploy-cancel", "Cancel", cx).on_click(cx.listener(|this, _, _, cx| this.set_adding(false, cx)));
        let col = div().flex().flex_col().gap(px(12.)).child(labeled("Command", field(self.command.clone(), true, th), th));
        col.child(labeled("Environment", self.choices(th, cx), th)).child(form_buttons(save, cancel)).into_any_element()
    }
}

impl Render for DeployPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let open = if self.current.is_some() { "Edit deploy" } else { "+ Set up deploy" };
        let more = if self.adding {
            self.form(&th, cx)
        } else {
            chip("deploy-open", open, cx).on_click(cx.listener(|this, _, _, cx| this.set_adding(true, cx))).into_any_element()
        };
        let col = div().flex().flex_col().items_start().gap(px(12.)).child(div().w_full().child(self.shown(&th)));
        col.child(div().w_full().child(more)).child(lib::note(SUBTITLE, &th))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_must_be_one_non_empty_line() {
        assert_eq!(command_of("  make deploy \n").unwrap(), "make deploy");
        assert_eq!(command_of("   ").unwrap_err(), "Type the deploy command");
        assert!(command_of("a\nb").is_err());
        assert_eq!((Env::Production.key(), Env::Staging.key()), ("production", "staging"));
    }
}
