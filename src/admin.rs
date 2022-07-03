use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{offset::Utc, Duration};
use rand::{prelude::SliceRandom, thread_rng, Rng};
use reqwest::Url;
use serde::Deserialize;
use teloxide::{
    payloads::{
        AnswerCallbackQuerySetters, BanChatMemberSetters, EditMessageTextSetters,
        SendMessageSetters,
    },
    prelude::Requester,
    types::{
        CallbackQuery, Chat, ChatMember, InlineKeyboardButton, InlineKeyboardMarkup, ParseMode,
        ReplyMarkup, User, UserId,
    },
};
use tokio::{sync::Mutex, time::sleep};

use crate::{Bot, CONFIG};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueryData {
    pub user_id: UserId,
    pub correct: usize,
    pub tried_times: u8,
    pub cas: Option<i32>, // i32 is message id
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
struct CasResult {
    pub ok: bool,
}

// message id as key
static UNVERIFIED_USERS: Mutex<BTreeMap<i32, QueryData>> = Mutex::const_new(BTreeMap::new());

pub fn metion_user(user: &User) -> String {
    format!(
        "[{}](tg://user?id={})",
        user.username
            .as_ref()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| user.full_name()),
        user.id,
    )
}

pub fn new_question() -> (&'static String, Vec<&'static String>, usize) {
    let mut rng = thread_rng();
    let question = CONFIG
        .get()
        .unwrap()
        .questions
        .choose(&mut rng)
        .expect("no question");
    let correct = question
        .correct
        .choose(&mut rng)
        .expect("no correct answer");
    let mut buttons = question
        .wrong
        .choose_multiple(&mut rng, 3)
        .collect::<Vec<_>>();
    let correct_idx = rng.gen_range(0..=buttons.len());
    buttons.insert(correct_idx, correct);

    (&question.title, buttons, correct_idx)
}

pub fn keyboard<S>(buttons: Vec<S>, change: bool) -> InlineKeyboardMarkup
where
    S: Into<String>,
{
    let mut keyboard = InlineKeyboardMarkup::new(
        buttons
            .into_iter()
            .enumerate()
            .map(|(idx, text)| vec![InlineKeyboardButton::callback(text, idx.to_string())]),
    );
    if change {
        keyboard = keyboard.append_row(vec![InlineKeyboardButton::callback("换题🔁", "change")])
    }
    keyboard.append_row(vec![
        InlineKeyboardButton::callback("手动踢出🚫", "admin-ban"),
        InlineKeyboardButton::callback("手动通过✅", "admin-allow"),
    ])
}

pub async fn send_auth(bot: Bot, user: User, chat: Chat) -> Result<()> {
    let (title, buttons, correct_idx) = new_question();

    // mute user
    let res = bot
        .restrict_chat_member(chat.id, user.id, teloxide::types::ChatPermissions::empty())
        .await;
    if let Err(err) = res {
        bot.send_message(chat.id, err.to_string()).await?;
        return Err(err.into());
    }

    let keyboard = keyboard(buttons, true);

    let mut users = UNVERIFIED_USERS.lock().await;

    let msg = bot
        .send_message(
            chat.id,
            format!(
                "{}，你有 5 分钟时间回答以下问题：\n\n{}",
                metion_user(&user),
                title
            ),
        )
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboard(keyboard))
        .await?;

    users.insert(
        msg.id,
        QueryData {
            user_id: user.id,
            correct: correct_idx,
            tried_times: 0,
            cas: None,
        },
    );

    let bot2 = bot.clone();
    tokio::spawn(async move {
        let bot = bot2;
        sleep(std::time::Duration::from_secs(5 * 60)).await;
        let mut users = UNVERIFIED_USERS.lock().await;
        if let Some(_data) = users.get_mut(&msg.id) {
            let res = bot
                .ban_chat_member(chat.id, user.id)
                .until_date(Utc::now() + Duration::minutes(10))
                .await;
            if let Err(err) = res {
                bot.send_message(chat.id, err.to_string()).await.ok();
            }

            let res = bot.delete_message(chat.id, msg.id).await;
            if let Err(err) = res {
                bot.send_message(chat.id, err.to_string()).await.ok();
            }

            users.remove(&msg.id);
        }
    });

    let bot2 = bot.clone();
    tokio::spawn(async move {
        let bot = bot2;
        let ok = reqwest::get(Url::parse_with_params(
            "https://api.cas.chat/check",
            &[("user_id", user.id.to_string())],
        )?)
        .await?
        .json::<CasResult>()
        .await?
        .ok;

        if ok {
            let mut users = UNVERIFIED_USERS.lock().await;

            let user = if let Some(user) = users.get_mut(&msg.id) {
                user
            } else {
                return Ok(());
            };

            let keyboard =
                InlineKeyboardMarkup::default().append_row(vec![InlineKeyboardButton::callback(
                    "确认踢出",
                    "admin-ban",
                )]);
            let res = bot
                .send_message(
                    chat.id,
                    format!(
                        "⚠️管理员注意，[该用户已被 CAS 封禁](https://cas.chat/query?u={})",
                        user.user_id
                    ),
                )
                .reply_to_message_id(msg.id)
                .parse_mode(ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .disable_web_page_preview(true)
                .await?;

            user.cas = Some(res.id);
        }

        anyhow::Ok(())
    });

    Ok(())
}

pub async fn callback(bot: Bot, callback: CallbackQuery) -> Result<()> {
    if callback.message.is_none() || callback.data.is_none() {
        bot.answer_callback_query(callback.id).await?;
        return Ok(());
    }
    let origin = callback.message.as_ref().unwrap();
    let mut users = UNVERIFIED_USERS.lock().await;
    let data = if let Some(data) = users.get_mut(&origin.id) {
        data
    } else {
        bot.answer_callback_query(callback.id).await?;
        return Ok(());
    };

    let callback_data = callback.data.unwrap();

    if callback_data.starts_with("admin") {
        let res = bot.get_chat_member(origin.chat.id, callback.from.id).await;
        let member: ChatMember = match res {
            Ok(member) => member,
            Err(err) => {
                bot.answer_callback_query(callback.id)
                    .text(format!("{}", err))
                    .show_alert(true)
                    .await?;
                return Ok(());
            }
        };
        if member.is_privileged() {
            match &callback_data[6..] {
                "ban" => {
                    let res = bot.ban_chat_member(origin.chat.id, data.user_id).await;
                    if let Err(err) = res {
                        bot.answer_callback_query(callback.id)
                            .text(err.to_string())
                            .show_alert(true)
                            .await?;
                    } else {
                        bot.answer_callback_query(callback.id).await?;
                    }
                    bot.delete_message(origin.chat.id, origin.id).await?;
                    if let Some(cas) = data.cas {
                        bot.delete_message(origin.chat.id, cas).await?;
                    }
                    users.remove(&origin.id);
                }
                "allow" => {
                    let res = bot
                        .restrict_chat_member(
                            origin.chat.id,
                            data.user_id,
                            teloxide::types::ChatPermissions::all(),
                        )
                        .await;
                    if let Err(err) = res {
                        bot.answer_callback_query(callback.id)
                            .text(err.to_string())
                            .show_alert(true)
                            .await?;
                    } else {
                        bot.answer_callback_query(callback.id).await?;
                    }
                    bot.delete_message(origin.chat.id, origin.id).await?;
                    if let Some(cas) = data.cas {
                        bot.delete_message(origin.chat.id, cas).await?;
                    }
                    users.remove(&origin.id);
                }
                _ => {
                    bot.answer_callback_query(callback.id)
                        .text(format!("未知命令：{}", &callback_data[6..]))
                        .show_alert(true)
                        .await?;
                }
            }
        } else {
            bot.answer_callback_query(callback.id)
                .text("只有管理员可以点击此按钮")
                .show_alert(true)
                .await?;
        }

        return Ok(());
    }

    if callback.from.id != data.user_id {
        if callback_data == data.correct.to_string() {
            bot.answer_callback_query(callback.id)
                .text("回答正确！但是并不会奖励屎给你。")
                .show_alert(true)
                .await?;
        } else if callback_data == "change" {
            bot.answer_callback_query(callback.id)
                .text("不会就别点！")
                .show_alert(true)
                .await?;
        } else {
            bot.answer_callback_query(callback.id)
                .text("回答错误！")
                .show_alert(true)
                .await?;
        }
        return Ok(());
    }

    if callback_data == data.correct.to_string() {
        bot.answer_callback_query(callback.id).await?;
        let res = bot
            .restrict_chat_member(
                origin.chat.id,
                data.user_id,
                teloxide::types::ChatPermissions::all(),
            )
            .await;
        if let Err(err) = res {
            bot.send_message(origin.chat.id, err.to_string()).await?;
            return Err(err.into());
        }
        bot.delete_message(origin.chat.id, origin.id).await?;

        if let Some(cas) = data.cas {
            let text = format!(
                "⚠️管理员注意，[CAS 封禁用户](https://cas.chat/query?u={}) {} 已通过验证加入群组",
                data.user_id,
                metion_user(&callback.from)
            );
            bot.edit_message_text(origin.chat.id, cas, text)
                .reply_markup(InlineKeyboardMarkup::default())
                .await?;
        }

        users.remove(&origin.id);
    } else if callback_data == "change" {
        let (title, buttons, correct_idx) = new_question();
        let keyboard = keyboard(buttons, false);
        bot.edit_message_text(origin.chat.id, origin.id, title)
            .reply_markup(keyboard)
            .await?;
        data.correct = correct_idx;
    } else {
        if let Some(cas) = data.cas {
            bot.answer_callback_query(callback.id)
                .text("验证失败")
                .show_alert(true)
                .await?;
            let res = bot.ban_chat_member(origin.chat.id, data.user_id).await;
            if let Err(err) = res {
                bot.send_message(origin.chat.id, err.to_string()).await?;
                return Err(err.into());
            }
            bot.delete_message(origin.chat.id, origin.id).await?;
            bot.delete_message(origin.chat.id, cas).await?;
            users.remove(&origin.id);
        } else if data.tried_times >= 2 {
            bot.answer_callback_query(callback.id)
                .text("验证失败，失败次数过多，请十分钟后重新加入")
                .show_alert(true)
                .await?;
            let res = bot
                .ban_chat_member(origin.chat.id, data.user_id)
                .until_date(Utc::now() + Duration::minutes(10))
                .await;
            if let Err(err) = res {
                bot.send_message(origin.chat.id, err.to_string()).await?;
                return Err(err.into());
            }
            bot.delete_message(origin.chat.id, origin.id).await?;
            users.remove(&origin.id);
        } else {
            bot.answer_callback_query(callback.id)
                .text("验证失败")
                .show_alert(true)
                .await?;
            data.tried_times += 1;
        }
        return Ok(());
    }

    Ok(())
}
