use anyhow::Result;
use teloxide::{
    payloads::SendMessageSetters,
    requests::Requester,
    types::{Chat, InlineKeyboardButton, InlineKeyboardMarkup, Message, MessageId, ParseMode, User},
};

use super::{
    QuestionData, auth_database, get_data_by_msg,
    handler::{Handler, res},
    user_finish,
};
use crate::{Bot, utils::*};

#[derive(Debug, Clone, Copy)]
pub struct LinkHandler;

impl Handler for LinkHandler {
    async fn send_question(&mut self, bot: Bot, user: User, chat: Chat, message_id: MessageId) -> Result<()> {
        if user.is_bot || user.is_premium || auth_database::is_authed(user.id.0).await? {
            return Ok(());
        }

        let data = QuestionData::new(
            user.clone(),
            chat.id,
            Some(message_id),
            super::handler::HandlerKind::Link,
        );

        let res = bot
            .send_message(chat.id, data.message())
            .parse_mode(ParseMode::Html)
            .reply_to_message_id(message_id)
            .reply_markup(data.keyboard())
            .await;

        let msg: Message = match res {
            Ok(msg) => msg,
            Err(err) => {
                admin_log(bot.clone(), format!("问题发送失败，自动允许发送\n{}", err)).await?;
                return Err(err.into());
            }
        };

        super::add_wating_user(msg.id, data).await;

        let _handle = tokio::spawn(super::waiting_answer(bot.clone(), msg.id, |data| async move {
            delete_sent_message(bot, data).await.ok();
        }));

        // add_wating_handle(msg.id, handle.abort_handle()).await;

        Ok(())
    }

    fn keyboard_patch(&self, keyboard: InlineKeyboardMarkup) -> InlineKeyboardMarkup {
        keyboard.append_row(vec![
            InlineKeyboardButton::callback("手动删除🚫", "admin-ban"),
            InlineKeyboardButton::callback("手动允许✅", "admin-allow"),
        ])
    }

    async fn handle_correct(&mut self, bot: Bot, msg_id: MessageId) -> Result<Option<String>> {
        if let Some(data) = user_finish(msg_id).await {
            allow_send_message(bot, data).await?;
        }
        res!("回答正确，验证通过")
    }

    async fn handle_wrong(&mut self, bot: Bot, msg_id: MessageId) -> Result<Option<String>> {
        let tried_times = {
            if let Some(data) = get_data_by_msg(&msg_id.0).await {
                data.tried_times
            } else {
                return res!();
            }
        };
        if tried_times >= 2 {
            if let Some(data) = user_finish(msg_id).await {
                delete_sent_message(bot, data).await?;
            }
            res!("验证失败，失败次数过多，删除消息。")
        } else {
            if let Some(mut data) = get_data_by_msg(&msg_id.0).await {
                data.tried_times += 1;
            }
            res!("验证失败")
        }
    }

    async fn handle_other(&mut self, bot: Bot, word: &str, msg_id: MessageId) -> Result<Option<String>> {
        if word == "admin-ban" {
            if let Some(data) = user_finish(msg_id).await {
                delete_sent_message(bot, data).await?;
            }
            res!()
        } else if word == "admin-allow" {
            if let Some(data) = user_finish(msg_id).await {
                allow_send_message(bot, data).await?;
            }
            res!()
        } else {
            res!(("未知命令：{}", word))
        }
    }
}

async fn delete_sent_message(bot: Bot, (msg_id, data): (i32, QuestionData)) -> Result<()> {
    super::TO_DELETE_MESSAGE.push((data.chat_id, MessageId(msg_id)));
    if let Some(spam_msg_id) = data.message_id {
        bot.delete_message(data.chat_id, spam_msg_id).await?;
    }

    Ok(())
}

async fn allow_send_message(_bot: Bot, (msg_id, data): (i32, QuestionData)) -> Result<()> {
    auth_database::add_authed(data.user.id.0).await?;
    super::TO_DELETE_MESSAGE.push((data.chat_id, MessageId(msg_id)));

    Ok(())
}
