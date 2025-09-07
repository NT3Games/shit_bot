use anyhow::Result;
use teloxide::types::{Chat, InlineKeyboardMarkup, MessageId, User};

use crate::Bot;

pub trait Handler {
    type Id = MessageId;

    fn send_question(
        &mut self,
        bot: Bot,
        user: User,
        chat: Chat,
        message_id: Self::Id,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn keyboard_patch(&self, keyboard: InlineKeyboardMarkup) -> InlineKeyboardMarkup {
        keyboard
    }

    fn handle_correct(
        &mut self,
        bot: Bot,
        msg_id: MessageId,
    ) -> impl std::future::Future<Output = Result<Option<String>>> + Send;

    fn handle_wrong(
        &mut self,
        bot: Bot,
        msg_id: MessageId,
    ) -> impl std::future::Future<Output = Result<Option<String>>> + Send;

    fn handle_other(
        &mut self,
        bot: Bot,
        word: &str,
        msg_id: MessageId,
    ) -> impl std::future::Future<Output = Result<Option<String>>> + Send;
}

macro_rules! res {
    (($($msg:expr),+)) => {
        res!( format!($($msg),+) )
    };
    ($msg:literal) => {
        res! { $msg.to_string() }
    };
    ($msg:expr) => {
        Ok(Some($msg))
    };
    () => {
        Ok(None)
    };
}
pub(crate) use res;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerKind {
    Join,
    Link,
    Test,
}

impl Handler for HandlerKind {
    async fn send_question(&mut self, bot: Bot, user: User, chat: Chat, message_id: MessageId) -> Result<()> {
        match self {
            HandlerKind::Join => {
                super::join_handler::JoinHandler
                    .send_question(bot, user, chat, ())
                    .await
            }
            HandlerKind::Link => {
                super::link_handler::LinkHandler
                    .send_question(bot, user, chat, message_id)
                    .await
            }
            HandlerKind::Test => {
                unimplemented!()
            }
        }
    }

    fn keyboard_patch(&self, keyboard: InlineKeyboardMarkup) -> InlineKeyboardMarkup {
        match self {
            HandlerKind::Join => super::join_handler::JoinHandler.keyboard_patch(keyboard),
            HandlerKind::Link => super::link_handler::LinkHandler.keyboard_patch(keyboard),
            HandlerKind::Test => {
                unimplemented!()
            }
        }
    }

    async fn handle_correct(&mut self, bot: Bot, msg_id: MessageId) -> Result<Option<String>> {
        match self {
            HandlerKind::Join => super::join_handler::JoinHandler.handle_correct(bot, msg_id).await,
            HandlerKind::Link => super::link_handler::LinkHandler.handle_correct(bot, msg_id).await,
            HandlerKind::Test => {
                unimplemented!()
            }
        }
    }

    async fn handle_wrong(&mut self, bot: Bot, msg_id: MessageId) -> Result<Option<String>> {
        match self {
            HandlerKind::Join => super::join_handler::JoinHandler.handle_wrong(bot, msg_id).await,
            HandlerKind::Link => super::link_handler::LinkHandler.handle_wrong(bot, msg_id).await,
            HandlerKind::Test => {
                unimplemented!()
            }
        }
    }

    async fn handle_other(&mut self, bot: Bot, word: &str, msg_id: MessageId) -> Result<Option<String>> {
        match self {
            HandlerKind::Join => super::join_handler::JoinHandler.handle_other(bot, word, msg_id).await,
            HandlerKind::Link => super::link_handler::LinkHandler.handle_other(bot, word, msg_id).await,
            HandlerKind::Test => {
                unimplemented!()
            }
        }
    }
}
