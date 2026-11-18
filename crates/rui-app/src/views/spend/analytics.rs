//! Analytics page: each client's share of the spend and how many threads it ran, from `by_client`.

use super::fmt::{money, share};
use crate::api::{ClientCost, Costs};
use crate::theme::Theme;
use crate::views::activity::section;
use gpui::{AnyElement, Div, FontWeight, div, prelude::*, px, relative};

const LATER: &str = "More analytics come later.";

/// The share bars, then the note that more is coming.
pub fn sections(c: &Costs, th: &Theme) -> Vec<AnyElement> {
    let mut rows: Vec<AnyElement> = c.by_client.iter().map(|r| share_row(r, c.total_usd, th)).collect();
    if rows.is_empty() {
        rows.push(muted("No client spend in this range.", th).into_any_element());
    }
    rows.push(muted(LATER, th).into_any_element());
    vec![section("Spend share per client", rows)]
}

/// "Acme · 3 threads", a bar filled to its share, and "$4.20 · 34%".
fn share_row(r: &ClientCost, total: f64, th: &Theme) -> AnyElement {
    let part = share(r.usd, total);
    let name = div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(th.fg).child(r.name.clone());
    let threads = muted(threads_text(r.tasks), th);
    let head = div().flex().items_center().gap(px(8.)).child(name).child(threads);
    let fill = div().h_full().w(relative(part)).rounded(px(4.)).bg(th.client_color(r.client_id));
    let track = div().flex_1().h(px(10.)).rounded(px(4.)).bg(th.well).child(fill);
    let value = muted(format!("{} \u{b7} {:.0}%", money(r.usd), part * 100.), th).w(px(120.)).flex().justify_end();
    let bar = div().flex().items_center().gap(px(12.)).child(track).child(value);
    div().flex().flex_col().gap(px(6.)).max_w(px(640.)).child(head).child(bar).into_any_element()
}

/// "1 thread", "3 threads".
fn threads_text(n: usize) -> String {
    format!("{n} {}", if n == 1 { "thread" } else { "threads" })
}

fn muted(text: impl Into<String>, th: &Theme) -> Div {
    div().text_size(px(11.5)).text_color(th.muted).child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_counts_read_well() {
        assert_eq!(threads_text(1), "1 thread");
        assert_eq!(threads_text(3), "3 threads");
    }
}
