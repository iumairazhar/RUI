//! "Deploy" in the client drawer: the command that deploys this client's work and where it goes.
//! The daemon has no read call for it, so the form starts empty and a save replaces what is set.

use super::library::{self as lib, run, toast};
use super::{chip, primary_button};
use crate::api;
use crate::store::Store;
use crate::text_input::TextInput;
use crate::theme::{Theme, theme};
use gpui::{AnyElement, Context, Div, Entity, FontWeight, Render, SharedString, Window, div, prelude::*, px};

const TITLE: &str = "Deploy";
const SUBTITLE: &str = "Every deploy waits for your OK in the Inbox.";

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
}

impl DeployPanel {
    pub fn new(store: Entity<Store>, cx: &mut Context<Self>) -> Self {
        let command = cx.new(|cx| TextInput::new("Command, like \u{201c}make deploy\u{201d}", cx));
        Self { store, client: None, env: Env::Staging, command, busy: false }
    }

    /// Show the form for a client, cleared.
    pub fn open(&mut self, id: i64, cx: &mut Context<Self>) {
        (self.client, self.env, self.busy) = (Some(id), Env::Staging, false);
        self.command.update(cx, |i, cx| i.clear(cx));
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
        let env = self.env.key();
        self.busy = true;
        run(cx, move || api::set_deploy_config(id, &cmd, env), |v, res, cx| v.saved(res, cx));
        cx.notify();
    }

    fn saved(&mut self, res: Result<(), String>, cx: &mut Context<Self>) {
        self.busy = false;
        match res {
            Ok(()) => toast(&self.store, "Deploy saved", self.env.label(), cx),
            Err(e) => toast(&self.store, "Could not save the deploy", &e, cx),
        }
        cx.notify();
    }

    fn choices(&self, cx: &mut Context<Self>) -> AnyElement {
        let button = |env: Env, cx: &mut Context<Self>| {
            let id = SharedString::from(format!("deploy-env-{}", env.key()));
            let el = if env == self.env { primary_button(id, env.label(), cx) } else { chip(id, env.label(), cx) };
            el.on_click(cx.listener(move |this, _, _, cx| this.pick(env, cx))).into_any_element()
        };
        let items: Vec<AnyElement> = Env::ALL.into_iter().map(|e| button(e, cx)).collect();
        div().flex().items_center().gap(px(6.)).children(items).into_any_element()
    }

    fn field(&self, th: &Theme) -> AnyElement {
        let f = div().px(px(10.)).py(px(4.)).rounded(px(8.)).bg(th.well).border_1().border_color(th.hairline);
        f.font_family("Menlo").child(self.command.clone()).into_any_element()
    }
}

fn labeled(label: &str, el: AnyElement, th: &Theme) -> Div {
    let head = div().text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg_2).child(label.to_string());
    div().flex().flex_col().gap(px(4.)).child(head).child(el)
}

impl Render for DeployPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let title = div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(TITLE);
        let head = div().flex().flex_col().gap(px(2.)).child(title).child(lib::note(SUBTITLE, &th));
        let label = if self.busy { "Saving\u{2026}" } else { "Save" };
        let save = primary_button("deploy-save", label, cx).on_click(cx.listener(|this, _, _, cx| this.save(cx)));
        let form = div().flex().flex_col().gap(px(10.)).child(labeled("Command", self.field(&th), &th));
        let form = form.child(labeled("Environment", self.choices(cx), &th)).child(div().flex().child(save));
        div().flex().flex_col().gap(px(12.)).child(head).child(form)
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
