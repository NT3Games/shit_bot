use std::collections::{
    btree_map::{self, OccupiedEntry},
    BTreeMap,
};

use anyhow::Result;
use chrono::{offset::Utc, DateTime, Duration};
use rand::{prelude::SliceRandom, thread_rng, Rng};
use redis::AsyncCommands;
use reqwest::Url;
use serde::Deserialize;
use teloxide::{
    payloads::{AnswerCallbackQuerySetters, EditMessageTextSetters, SendMessageSetters},
    prelude::*,
    types::{
        CallbackQuery, Chat, ChatId, ChatMember, InlineKeyboardButton, InlineKeyboardMarkup,
        ParseMode, ReplyMarkup, User, UserId,
    },
};
use tokio::{sync::Mutex, time::sleep};

use crate::{Bot, CONFIG};

const LAST_JOIN_RESULT_KEY: &str = "shit_bot_last_join_result";

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub title: String,
    pub contrary: Option<String>,
    pub wrong: Vec<String>,
    pub correct: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryData {
    pub user: User,
    pub chat_id: ChatId,
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
    if let Some(username) = user.username.as_ref() {
        format!("<a href=\"tg://user?id={}\">@{}</a>", user.id, username)
    } else {
        format!(
            "<a href=\"tg://user?id={}\">{}</a>",
            user.id,
            htmlescape::encode_minimal(&user.full_name()),
        )
    }
}

pub fn new_question() -> (&'static String, Vec<&'static String>, usize) {
    let mut rng = thread_rng();
    let question = CONFIG
        .get()
        .unwrap()
        .questions
        .choose(&mut rng)
        .expect("no question");

    let (title, correct_answers, wrong_answers) =
        if question.contrary.is_some() && rng.gen_bool(0.5) {
            (
                question.contrary.as_ref().unwrap(),
                &question.wrong,
                &question.correct,
            )
        } else {
            (&question.title, &question.correct, &question.wrong)
        };

    let correct = correct_answers.choose(&mut rng).expect("no correct answer");
    let mut buttons = wrong_answers
        .choose_multiple(&mut rng, 3)
        .collect::<Vec<_>>();
    let correct_idx = rng.gen_range(0..=buttons.len());
    buttons.insert(correct_idx, correct);

    (title, buttons, correct_idx)
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

fn rank_user(user: &User) -> f64 {
    if user.first_name.contains("免费")
        || user.first_name.contains("VPN")
        || user.first_name.contains("梯子")
    {
        return 0.0;
    }
    // if user.is_premium {
    //     return 1.0;
    // }
    let mut result = 0.4;
    if user.username.is_some() {
        result += 0.3;
    }
    result
}

pub async fn send_auth(bot: Bot, user: User, chat: Chat) -> Result<()> {
    if user.is_bot {
        return Ok(());
    }

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

    let res = bot
        .send_message(
            chat.id,
            format!(
                "{}，你有 5 分钟时间回答以下问题：\n\n{}",
                metion_user(&user),
                title
            ),
        )
        .parse_mode(ParseMode::Html)
        .reply_markup(ReplyMarkup::InlineKeyboard(keyboard))
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

    users.insert(
        msg.id,
        QueryData {
            user: user.clone(),
            chat_id: chat.id,
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
        if let btree_map::Entry::Occupied(data) = users.entry(msg.id) {
            ban(bot, data, Some(Utc::now() + Duration::minutes(10)))
                .await
                .ok();
        }
    });

    let bot2 = bot.clone();
    tokio::spawn(check_cas(bot2, chat.id, user.id, msg.id));

    Ok(())
}

async fn check_cas(bot: Bot, chat_id: ChatId, user_id: UserId, msg_id: i32) -> Result<()> {
    let ok = reqwest::get(Url::parse_with_params(
        "https://api.cas.chat/check",
        &[("user_id", user_id.to_string())],
    )?)
    .await?
    .json::<CasResult>()
    .await?
    .ok;

    if !ok {
        return Ok(());
    }

    let mut users = UNVERIFIED_USERS.lock().await;

    let user = if let Some(user) = users.get_mut(&msg_id) {
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
            chat_id,
            format!(
                "⚠️管理员注意，<a href=\"https://cas.chat/query?u={}\">该用户已被 CAS 封禁</a>",
                user.user.id
            ),
        )
        .reply_to_message_id(msg_id)
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .disable_web_page_preview(true)
        .await?;

    user.cas = Some(res.id);

    Ok(())
}

pub async fn callback(bot: Bot, callback: CallbackQuery) -> Result<()> {
    if callback.message.is_none() || callback.data.is_none() {
        bot.answer_callback_query(callback.id).await?;
        return Ok(());
    }
    let origin = callback.message.as_ref().unwrap();
    let mut users = UNVERIFIED_USERS.lock().await;
    let mut data_entry = if let btree_map::Entry::Occupied(data) = users.entry(origin.id) {
        data
    } else {
        bot.answer_callback_query(callback.id).await?;
        return Ok(());
    };
    let data = data_entry.get();

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
                    bot.answer_callback_query(callback.id).await?;
                    ban(bot, data_entry, None).await?;
                }
                "allow" => {
                    bot.answer_callback_query(callback.id).await?;
                    allow(bot, data_entry, false).await?;
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

    if callback.from.id != data.user.id {
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
        allow(bot, data_entry, true).await?;
    } else if callback_data == "change" {
        let (title, buttons, correct_idx) = new_question();
        let keyboard = keyboard(buttons, false);
        bot.edit_message_text(origin.chat.id, origin.id, title)
            .reply_markup(keyboard)
            .await?;
        data_entry.get_mut().correct = correct_idx;
    } else {
        if data.cas.is_some() {
            bot.answer_callback_query(callback.id)
                .text("验证失败")
                .show_alert(true)
                .await?;

            ban(bot.clone(), data_entry, None).await?;
        } else if data.tried_times >= 2 {
            bot.answer_callback_query(callback.id)
                .text("验证失败，失败次数过多，请十分钟后重新加入")
                .show_alert(true)
                .await?;

            ban(
                bot.clone(),
                data_entry,
                Some(Utc::now() + Duration::minutes(10)),
            )
            .await?;
        } else if data.tried_times == 0 && thread_rng().gen_bool(rank_user(&data.user)) {
            bot.answer_callback_query(callback.id)
                .text("尽管你回答错误了，但我们还是允许你加入。")
                .show_alert(true)
                .await?;
            allow(bot, data_entry, true).await?;
        } else {
            bot.answer_callback_query(callback.id)
                .text("验证失败")
                .show_alert(true)
                .await?;
            data_entry.get_mut().tried_times += 1;
        }
        return Ok(());
    }

    Ok(())
}

async fn ban(
    bot: Bot,
    entry: OccupiedEntry<'_, i32, QueryData>,
    until_date: Option<DateTime<Utc>>,
) -> Result<()> {
    let (msg_id, data) = entry.remove_entry();

    let mut req = bot.inner().ban_chat_member(data.chat_id, data.user.id);
    req.until_date = until_date;
    let res = req.send().await;
    if let Err(err) = res {
        bot.send_message(data.chat_id, err.to_string()).await?;
        return Err(err.into());
    }
    bot.delete_message(data.chat_id, msg_id).await?;
    if let Some(cas) = data.cas {
        bot.delete_message(data.chat_id, cas).await?;
    }

    send_join_result(
        bot,
        data.chat_id,
        format!("{} 验证失败，被扔进化粪池里了！", metion_user(&data.user)),
    )
    .await?;

    Ok(())
}

async fn allow(bot: Bot, entry: OccupiedEntry<'_, i32, QueryData>, remain_cas: bool) -> Result<()> {
    let (msg_id, data) = entry.remove_entry();

    let res = bot
        .restrict_chat_member(
            data.chat_id,
            data.user.id,
            teloxide::types::ChatPermissions::all(),
        )
        .await;
    if let Err(err) = res {
        bot.send_message(
            data.chat_id,
            format!("⚠️管理员注意！解除禁言失败，请管理员手动解除\n{}", err),
        )
        .await?;
        return Err(err.into());
    }
    bot.delete_message(data.chat_id, msg_id).await?;

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

    send_join_result(
        bot,
        data.chat_id,
        format!("{} 验证通过，欢迎！", metion_user(&data.user)),
    )
    .await?;

    Ok(())
}

async fn send_join_result(bot: Bot, chat_id: ChatId, message: String) -> Result<()> {
    let res = bot
        .send_message(chat_id, message)
        .parse_mode(ParseMode::Html)
        .disable_web_page_preview(true)
        .await?;

    let mut con = crate::get_client().await.get_async_connection().await?;
    let last: Option<i32> = con
        .get(format!("{}/{}", LAST_JOIN_RESULT_KEY, chat_id))
        .await?;
    con.set(LAST_JOIN_RESULT_KEY, res.id).await?;
    if let Some(id) = last {
        bot.delete_message(chat_id, id).await?;
    }

    Ok(())
}
