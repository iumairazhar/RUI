//! Asks: questions Rui puts to the owner from chat, shown in the Inbox.

use crate::db::Db;
use rui_core::{ChatAsk, ChatRole};
use tokio::sync::mpsc::UnboundedSender;

/// Most asks Rui may make in one turn.
pub const MAX_ASKS_PER_TURN: i64 = 3;

/// Why an answer was refused.
#[derive(Debug)]
pub enum AskError {
    NotFound,
    Answered,
    Other(anyhow::Error),
}

impl From<anyhow::Error> for AskError {
    fn from(e: anyhow::Error) -> Self {
        AskError::Other(e)
    }
}

/// One owner message with every answer of the turn, one `Re:` line each.
fn combined(asks: &[ChatAsk]) -> String {
    let line = |a: &ChatAsk| format!("Re: {} — {}", a.text, a.answer.as_deref().unwrap_or_default());
    asks.iter().map(line).collect::<Vec<_>>().join("\n")
}

/// Stores the answer. Once every ask of the turn is answered, posts one owner message
/// into the chat and wakes Rui. Returns the answered ask.
pub fn answer(db: &Db, chat_tx: &UnboundedSender<i64>, id: i64, answer: &str) -> Result<ChatAsk, AskError> {
    if db.get_chat_ask(id)?.is_none() {
        return Err(AskError::NotFound);
    }
    let Some(done) = db.answer_chat_ask(id, answer)? else { return Err(AskError::Answered) };
    let turn = db.asks_of_same_turn(id)?;
    if turn.iter().all(|a| a.status == "answered") {
        let msg = db.add_chat_message(done.chat_id, ChatRole::User, &combined(&turn))?;
        let _ = chat_tx.send(msg.id);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (Db, i64, i64) {
        let db = Db::open_test();
        let chat = db.create_chat().unwrap();
        let msg = db.add_chat_message(chat.id, ChatRole::User, "help").unwrap();
        (db, chat.id, msg.id)
    }

    #[test]
    fn answers_of_one_turn_arrive_as_one_message() {
        let (db, chat, msg) = setup();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let a = db.create_chat_ask(chat, Some(msg), "When?", &["Today".into(), "Thursday".into()]).unwrap();
        let b = db.create_chat_ask(chat, Some(msg), "Who for?", &[]).unwrap();
        assert_eq!(a.options, ["Today", "Thursday"]);
        answer(&db, &tx, a.id, "Thursday").unwrap();
        assert!(rx.try_recv().is_err(), "waits for the rest of the turn");
        assert_eq!(db.list_chat(chat, 10).unwrap().len(), 1);
        answer(&db, &tx, b.id, "me").unwrap();
        let sent = rx.try_recv().unwrap();
        let last = db.list_chat(chat, 10).unwrap().pop().unwrap();
        assert_eq!((last.id, last.role), (sent, ChatRole::User));
        assert_eq!(last.content, "Re: When? — Thursday\nRe: Who for? — me");
        assert!(matches!(answer(&db, &tx, a.id, "x"), Err(AskError::Answered)));
        assert!(matches!(answer(&db, &tx, 999, "x"), Err(AskError::NotFound)));
    }

    #[test]
    fn a_single_ask_posts_at_once() {
        let (db, chat, _) = setup();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let a = db.create_chat_ask(chat, None, "Q?", &[]).unwrap();
        answer(&db, &tx, a.id, "A").unwrap();
        assert!(rx.try_recv().is_ok());
        assert_eq!(db.list_chat(chat, 10).unwrap().pop().unwrap().content, "Re: Q? — A");
    }

    #[test]
    fn replying_in_chat_closes_open_asks_and_listing_filters() {
        let (db, chat, msg) = setup();
        db.create_chat_ask(chat, Some(msg), "Q?", &[]).unwrap();
        let open = db.list_chat_asks(Some("open")).unwrap();
        assert_eq!((open.len(), open[0].chat_title.as_str()), (1, "help"));
        assert_eq!(db.close_chat_asks(chat).unwrap(), 1);
        assert!(db.list_chat_asks(Some("open")).unwrap().is_empty());
        assert_eq!(db.list_chat_asks(None).unwrap().len(), 1);
    }
}
