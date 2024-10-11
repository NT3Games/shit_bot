use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use rand::{thread_rng, Rng};
use reqwest::Url;
use teloxide::{
    payloads::{EditMessageTextSetters, SendMessageSetters},
    requests::Requester,
    types::{Chat, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, Message, MessageId, ParseMode, User, UserId},
};

use super::{auth_database, get_data_by_msg, handler::*, user_finish, QuestionData};
use crate::{question, utils::*, Bot};

async fn check_cas(bot: Bot, chat_id: ChatId, user_id: UserId, msg_id: i32) -> Result<()> {
    let ok = reqwest::get(Url::parse_with_params(
        "https://api.cas.chat/check",
        &[("user_id", user_id.to_string())],
    )?)
    .await?
    .json::<super::CasResult>()
    .await?
    .ok;

    if !ok {
        return Ok(());
    }

    let mut user = if let Some(data) = super::get_data_by_msg(&msg_id).await {
        data
    } else {
        return Ok(());
    };
    let keyboard =
        InlineKeyboardMarkup::default().append_row(vec![InlineKeyboardButton::callback("确认踢出", "admin-ban")]);
    let res = bot
        .send_message(
            chat_id,
            format!(
                "⚠️管理员注意，<a href=\"https://cas.chat/query?u={}\">该用户已被 CAS 封禁</a>",
                user.user.id
            ),
        )
        .reply_to_message_id(MessageId(msg_id))
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .disable_web_page_preview()
        .await?;

    user.cas = Some(res.id);

    Ok(())
}

#[derive(Debug, Clone, Copy)]
pub struct JoinHandler;

impl Handler for JoinHandler {
    async fn send_question(&mut self, bot: Bot, user: User, chat: Chat, message_id: MessageId) -> Result<()> {
        if user.is_bot {
            return Ok(());
        }

        if user.is_premium {
            bot.send_message(chat.id, format!("Premium 用户 {}，欢迎！", metion_user(&user)))
                .parse_mode(ParseMode::Html)
                .await?;

            return Ok(());
        }

        if auth_database::is_authed(user.id.0).await? {
            bot.send_message(chat.id, format!("{}，欢迎！", metion_user(&user)))
                .parse_mode(ParseMode::Html)
                .await?;

            return Ok(());
        }

        let (title, options, correct_idx) = question::new_question();

        // mute user
        let res = bot
            .restrict_chat_member(chat.id, user.id, teloxide::types::ChatPermissions::empty())
            .await;
        if let Err(err) = res {
            bot.send_message(chat.id, err.to_string()).await?;
            return Err(err.into());
        }

        let data = super::QuestionData {
            user: user.clone(),
            chat_id: chat.id,
            message_id,
            title,
            options,
            correct: correct_idx,
            tried_times: 0,
            cas: None,
            left_minutes: 5,
            handler: super::handler::HandlerKind::Join,
        };

        let res = bot
            .send_message(chat.id, data.message())
            .parse_mode(ParseMode::Html)
            .reply_markup(data.keyboard(true))
            .await;

        let msg: Message = match res {
            Ok(msg) => msg,
            Err(err) => {
                bot.send_message(chat.id, format!("问题发送失败，自动允许加入\n{}", err))
                    .await?;
                let res = bot
                    .restrict_chat_member(chat.id, user.id, teloxide::types::ChatPermissions::all())
                    .await;
                if let Err(err) = res {
                    bot.send_message(
                        chat.id,
                        format!("⚠️管理员注意！解除禁言失败，请管理员手动解除\n{}", err),
                    )
                    .await?;
                    return Err(err.into());
                }
                return Err(err.into());
            }
        };

        super::add_wating_user(msg.id, data).await;

        let bot2 = bot.clone();
        let _handle = tokio::spawn(super::waiting_answer(bot.clone(), msg.id, |data| async move {
            ban(bot2, data, Some(Utc::now() + Duration::minutes(10))).await.ok();
        }));
        // add_wating_handle(msg.id, handle.abort_handle()).await;

        let bot2 = bot.clone();
        tokio::spawn(check_cas(bot2, chat.id, user.id, msg.id.0));

        Ok(())
    }

    fn keyboard_patch(&self, keyboard: InlineKeyboardMarkup) -> InlineKeyboardMarkup {
        keyboard.append_row(vec![
            InlineKeyboardButton::callback("手动踢出🚫", "admin-ban"),
            InlineKeyboardButton::callback("手动通过✅", "admin-allow"),
        ])
    }

    async fn handle_correct(&mut self, bot: Bot, msg_id: MessageId) -> Result<Option<String>> {
        if let Some(data) = user_finish(msg_id).await {
            allow(bot, data, false).await?;
        }
        res!("回答正确，验证通过")
    }

    async fn handle_wrong(&mut self, bot: Bot, msg_id: MessageId) -> Result<Option<String>> {
        let (cas, tried_times, rank) = {
            if let Some(data) = get_data_by_msg(&msg_id.0).await {
                (data.cas, data.tried_times, rank_user(&data.user))
            } else {
                return res!();
            }
        };
        if cas.is_some() {
            if let Some(data) = user_finish(msg_id).await {
                ban(bot, data, None).await?;
            }
            res!("验证失败")
        } else if tried_times >= 2 {
            if let Some(data) = user_finish(msg_id).await {
                ban(bot, data, Some(Utc::now() + Duration::minutes(10))).await?;
            }
            res!("验证失败，失败次数过多，请十分钟后重新加入")
        } else if tried_times == 0 && thread_rng().gen_bool(rank) {
            if let Some(data) = user_finish(msg_id).await {
                allow(bot, data, true).await?;
            }
            res!("尽管你回答错误了，但我们还是允许你加入。")
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
                ban(bot, data, None).await?;
            }
            res!()
        } else if word == "admin-allow" {
            if let Some(data) = user_finish(msg_id).await {
                allow(bot, data, false).await?;
            }
            res!()
        } else {
            res!(("未知命令：{}", word))
        }
    }
}

async fn allow(bot: Bot, (msg_id, data): (i32, QuestionData), remain_cas: bool) -> Result<()> {
    let res = bot
        .restrict_chat_member(data.chat_id, data.user.id, teloxide::types::ChatPermissions::all())
        .await;
    if let Err(err) = res {
        bot.send_message(
            data.chat_id,
            format!("⚠️管理员注意！解除禁言失败，请管理员手动解除\n{}", err),
        )
        .await?;
        return Err(err.into());
    }
    super::TO_DELETE_MESSAGE.push((data.chat_id, MessageId(msg_id)));

    if let Some(cas) = data.cas {
        if remain_cas {
            let text = format!(
                "⚠️管理员注意，<a href=\"https://cas.chat/query?u={}\">CAS 封禁用户</a> {} 已通过验证加入群组",
                data.user.id,
                metion_user(&data.user)
            );
            bot.edit_message_text(data.chat_id, cas, text)
                .reply_markup(InlineKeyboardMarkup::default())
                .await?;
        } else {
            bot.delete_message(data.chat_id, cas).await?;
        }
    }

    send_and_delete_join_result(
        bot,
        data.chat_id,
        format!("{} 验证通过，欢迎！", metion_user(&data.user)),
    )
    .await?;

    Ok(())
}

pub async fn ban(bot: Bot, (msg_id, data): (i32, QuestionData), until_date: Option<DateTime<Utc>>) -> Result<()> {
    let mut req = bot.ban_chat_member(data.chat_id, data.user.id);
    req.until_date = until_date;
    let res = req.await;
    if let Err(err) = res {
        bot.send_message(data.chat_id, err.to_string()).await?;
        return Err(err.into());
    }
    super::TO_DELETE_MESSAGE.push((data.chat_id, MessageId(msg_id)));
    bot.delete_message(data.chat_id, data.message_id).await?;
    if let Some(cas) = data.cas {
        bot.delete_message(data.chat_id, cas).await?;
    }

    let message = if is_spam_name(&data.user.full_name()) {
        "<filtered> 验证失败！".to_string()
    } else {
        format!("{} 验证失败，被扔进化粪池里了！", metion_user(&data.user))
    };
    send_and_delete_join_result(bot, data.chat_id, message).await?;

    Ok(())
}
