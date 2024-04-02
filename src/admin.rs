use std::{
    collections::{
        btree_map::{self, OccupiedEntry},
        BTreeMap,
    },
    future::Future,
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
        Chat, ChatMember, InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode, User,
    },
};
use tokio::{sync::Mutex, time::sleep};

use crate::CONFIG;

const LAST_JOIN_RESULT_KEY: &str = "shit_bot_last_join_result";
pub const AUTHED_USERS_KEY: &str = "shit_bot_authed_users";

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
    pub message_id: MessageId, // may the new member message or spam message
    pub correct: usize,
    pub title: &'static str,
    pub options: Vec<&'static String>,
    pub tried_times: u8,
    pub cas: Option<MessageId>, // i32 is message id
    pub left_minutes: u8,
    pub joining: bool,
}

impl QueryData {
    pub fn message(&self) -> String {
        format!(
            "{}，你有 {} 分钟时间回答以下问题：\n\n{}",
            metion_user(&self.user),
            self.left_minutes,
            self.title
        )
    }

    pub fn keyboard(&self, change: bool) -> InlineKeyboardMarkup {
        let mut keyboard = InlineKeyboardMarkup::new(
            self.options
                .iter()
                .enumerate()
                .map(|(idx, &text)| vec![InlineKeyboardButton::callback(text, idx.to_string())]),
        );
        if change {
            keyboard = keyboard.append_row(vec![InlineKeyboardButton::callback("换题🔁", "change")])
        }
        keyboard.append_row(vec![
            InlineKeyboardButton::callback("手动踢出🚫", "admin-ban"),
            InlineKeyboardButton::callback("手动通过✅", "admin-allow"),
        ])
    }
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

fn is_spam_name(name: &str) -> bool {
    name.contains("免费") || name.contains("VPN") || name.contains("梯子")
}

fn rank_user(user: &User) -> f64 {
    if user.is_premium {
        return 1.0;
    }
    if is_spam_name(&user.full_name()) {
        return 0.0;
    }
    let mut result = 0.5;
    if user.username.is_some() {
        result += 0.3;
    }
    result
}

pub async fn send_auth(bot: Bot, user: User, chat: Chat, new_member_id: MessageId) -> Result<()> {
    if user.is_bot {
        return Ok(());
    }

    if user.is_premium {
        bot.send_message(
            chat.id,
            format!("Premium 用户 {}，欢迎！", metion_user(&user)),
        )
        .parse_mode(ParseMode::Html)
        .await?;

        return Ok(());
    }

    let (title, options, correct_idx) = new_question();

    // mute user
    let res = bot
        .restrict_chat_member(chat.id, user.id, teloxide::types::ChatPermissions::empty())
        .await;
    if let Err(err) = res {
        bot.send_message(chat.id, err.to_string()).await?;
        return Err(err.into());
    }

    let data = QueryData {
        user: user.clone(),
        chat_id: chat.id,
        message_id: new_member_id,
        title,
        options,
        correct: correct_idx,
        tried_times: 0,
        cas: None,
        left_minutes: 5,
        joining: true,
    };

    let mut users = UNVERIFIED_USERS.lock().await;

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

    users.insert(msg.id.0, data);

    let bot2 = bot.clone();
    tokio::spawn(waiting_answer(bot.clone(), msg.id, async move {
        let mut users = UNVERIFIED_USERS.lock().await;
        if let btree_map::Entry::Occupied(data) = users.entry(msg.id.0) {
            ban(bot2, data, Some(Utc::now() + Duration::minutes(10)))
                .await
                .ok();
        }
    }));

    let bot2 = bot.clone();
    tokio::spawn(async move {
        let bot = bot2;
        loop {
            sleep(std::time::Duration::from_secs(60)).await;

            let mut users = UNVERIFIED_USERS.lock().await;
            if let btree_map::Entry::Occupied(mut data) = users.entry(msg.id.0) {
                data.get_mut().left_minutes -= 1;
                let data = data.get();
                if data.left_minutes == 0 {
                    break;
                } else {
                    bot.edit_message_text(data.chat_id, msg.id, data.message())
                        .reply_markup(data.keyboard(false))
                        .await
                        .ok();
                }
            } else {
                break;
            }
        }

        let mut users = UNVERIFIED_USERS.lock().await;
        if let btree_map::Entry::Occupied(data) = users.entry(msg.id.0) {
            ban(bot, data, Some(Utc::now() + Duration::minutes(10)))
                .await
                .ok();
        }
    });

    let bot2 = bot.clone();
    tokio::spawn(check_cas(bot2, chat.id, user.id, msg.id.0));

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
        .reply_to_message_id(MessageId(msg_id))
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .disable_web_page_preview(true)
        .await?;

    user.cas = Some(res.id);

    Ok(())
}

pub struct CallbackResult {
    pub typ: CallbackResultType,
    pub msg: Option<String>,
}

pub enum CallbackResultType {
    AdminAllow,
    AdminBan,
    Allow,
    Ban(Option<DateTime<Utc>>),
    Other,
}

pub async fn callback(bot: Bot, callback: CallbackQuery) -> Result<()> {
    if callback.message.is_none() || callback.data.is_none() {
        bot.answer_callback_query(callback.id).await?;
        return Ok(());
    }
    let mut users = UNVERIFIED_USERS.lock().await;
    let mut data_entry = if let btree_map::Entry::Occupied(data) =
        users.entry(callback.message.as_ref().unwrap().id.0)
    {
        data
    } else {
        bot.answer_callback_query(callback.id).await?;
        return Ok(());
    };

    let callback_id = callback.id.clone();

    let result = callback_handle(bot.clone(), &callback, &mut data_entry).await?;

    let data = data_entry.get();

    if let Some(msg) = result.msg {
        bot.answer_callback_query(callback_id)
            .text(msg)
            .show_alert(true)
            .await?;
    } else {
        bot.answer_callback_query(callback.id).await?;
    }

    use CallbackResultType::*;

    if data.joining {
        match result.typ {
            AdminAllow => {
                allow(bot, data_entry, false).await?;
            }
            AdminBan => {
                ban(bot, data_entry, None).await?;
            }
            Allow => {
                allow(bot, data_entry, true).await?;
            }
            Ban(until_date) => {
                ban(bot, data_entry, until_date).await?;
            }
            Other => {}
        };
    } else {
        match result.typ {
            AdminAllow | Allow => {
                allow_send_message(bot, data_entry).await?;
            }
            AdminBan | Ban(_) => {
                delete_sent_message(bot, data_entry).await?;
            }
            Other => {}
        };
    }
    return Ok(());
}

async fn callback_handle(
    bot: Bot,
    callback: &CallbackQuery,
    data_entry: &mut OccupiedEntry<'_, i32, QueryData>,
) -> Result<CallbackResult> {
    use CallbackResultType::*;

    macro_rules! res {
        ($typ:expr, $msg:literal) => {
            res! { $typ, $msg.to_string() }
        };
        ($typ:expr, ($($msg:expr),+)) => {
            res!( $typ, format!($($msg),+) )
        };
        ($typ:expr, $msg:expr) => {
            Ok(CallbackResult {
                typ: $typ,
                msg: Some($msg),
            })
        };
        ($typ:path) => {
            Ok(CallbackResult {
                typ: $typ,
                msg: None,
            })
        };
    }

    let origin = callback.message.as_ref().unwrap();
    let callback_data = callback.data.as_ref().unwrap();

    let data = data_entry.get();

    if callback_data.starts_with("admin") {
        let res = bot.get_chat_member(origin.chat.id, callback.from.id).await;
        let member: ChatMember = match res {
            Ok(member) => member,
            Err(err) => {
                return res!(Other, ("{}", err));
            }
        };
        if member.is_privileged() {
            match &callback_data[6..] {
                "ban" => {
                    return res!(AdminBan);
                }
                "allow" => {
                    return res!(AdminAllow);
                }
                _ => {
                    return res!(Other, ("未知命令：{}", &callback_data[6..]));
                }
            }
        } else {
            return res!(Other, "只有管理员可以点击此按钮");
        }
    }

    if callback.from.id != data.user.id {
        return res!(
            Other,
            {
                if callback_data == &data.correct.to_string() {
                    "回答正确！但是并不会奖励屎给你。"
                } else if callback_data == "change" {
                    "不会就别点！"
                } else {
                    "回答错误！"
                }
            }
            .to_string()
        );
    }

    if callback_data == &data.correct.to_string() {
        return res!(Allow);
    } else if callback_data == "change" {
        let (title, options, correct_idx) = new_question();
        let data = data_entry.get_mut();
        data.correct = correct_idx;
        data.options = options;
        data.title = title;
        bot.edit_message_text(origin.chat.id, origin.id, data.message())
            .parse_mode(ParseMode::Html)
            .reply_markup(data.keyboard(false))
            .await?;
        data_entry.get_mut().correct = correct_idx;
        return res!(Other);
    } else {
        if data.cas.is_some() {
            return res!(Ban(None), "验证失败");
        } else if data.tried_times >= 2 {
            return res!(
                Ban(Some(Utc::now() + Duration::minutes(10))),
                "验证失败，失败次数过多，请十分钟后重新加入"
            );
        } else if data.tried_times == 0 && thread_rng().gen_bool(rank_user(&data.user)) {
            return res!(Allow, "尽管你回答错误了，但我们还是允许你加入。");
        } else {
            data_entry.get_mut().tried_times += 1;
            return res!(Other, "验证失败");
        }
    }
}

async fn ban(
    bot: Bot,
    entry: OccupiedEntry<'_, i32, QueryData>,
    until_date: Option<DateTime<Utc>>,
) -> Result<()> {
    let (msg_id, data) = entry.remove_entry();

    let mut req = bot.ban_chat_member(data.chat_id, data.user.id);
    req.until_date = until_date;
    let res = req.await;
    if let Err(err) = res {
        bot.send_message(data.chat_id, err.to_string()).await?;
        return Err(err.into());
    }
    bot.delete_message(data.chat_id, MessageId(msg_id)).await?;
    bot.delete_message(data.chat_id, data.message_id).await?;
    if let Some(cas) = data.cas {
        bot.delete_message(data.chat_id, cas).await?;
    }

    let message = if is_spam_name(&data.user.full_name()) {
        "<filtered> 验证失败！".to_string()
    } else {
        format!("{} 验证失败，被扔进化粪池里了！", metion_user(&data.user))
    };
    send_join_result(bot, data.chat_id, message).await?;

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
    bot.delete_message(data.chat_id, MessageId(msg_id)).await?;

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

    let mut con = crate::get_client().await.get_async_connection().await?;
    con.sadd(AUTHED_USERS_KEY, data.user.id.0).await?;

    send_join_result(
        bot,
        data.chat_id,
        format!("{} 验证通过，欢迎！", metion_user(&data.user)),
    )
    .await?;

    Ok(())
}

async fn send_join_result(bot: Bot, chat_id: ChatId, message: String) -> Result<()> {
    bot.send_message(crate::CONFIG.get().unwrap().admin_chat, message.clone())
        .parse_mode(ParseMode::Html)
        .disable_web_page_preview(true)
        .await?;
    let res = bot
        .send_message(chat_id, message)
        .parse_mode(ParseMode::Html)
        .disable_web_page_preview(true)
        .await?;

    let mut con = crate::get_client().await.get_async_connection().await?;
    let last: Option<i32> = con
        .get(format!("{}/{}", LAST_JOIN_RESULT_KEY, chat_id))
        .await?;
    con.set(LAST_JOIN_RESULT_KEY, res.id.0).await?;
    if let Some(id) = last {
        bot.delete_message(chat_id, MessageId(id)).await?;
    }

    Ok(())
}

async fn allow_send_message(bot: Bot, entry: OccupiedEntry<'_, i32, QueryData>) -> Result<()> {
    let (msg_id, data) = entry.remove_entry();

    let mut con = crate::get_client().await.get_async_connection().await?;
    con.sadd(AUTHED_USERS_KEY, data.user.id.0).await?;

    bot.delete_message(data.chat_id, MessageId(msg_id)).await?;

    Ok(())
}

async fn delete_sent_message(bot: Bot, entry: OccupiedEntry<'_, i32, QueryData>) -> Result<()> {
    let (msg_id, data) = entry.remove_entry();
    bot.delete_message(data.chat_id, MessageId(msg_id)).await?;
    bot.delete_message(data.chat_id, data.message_id).await?;

    Ok(())
}

pub async fn send_auth_for_channel(
    bot: Bot,
    user: User,
    chat: Chat,
    message_id: MessageId,
) -> Result<()> {
    if user.is_bot || user.is_premium || {
        let mut con = crate::get_client().await.get_async_connection().await?;
        con.sismember(AUTHED_USERS_KEY, user.id.0).await?
    } {
        return Ok(());
    }

    let (title, options, correct_idx) = new_question();

    let data = QueryData {
        user: user.clone(),
        chat_id: chat.id,
        message_id,
        title,
        options,
        correct: correct_idx,
        tried_times: 0,
        cas: None,
        left_minutes: 5,
        joining: false,
    };

    let mut users = UNVERIFIED_USERS.lock().await;

    let res = bot
        .send_message(chat.id, data.message())
        .parse_mode(ParseMode::Html)
        .reply_markup(data.keyboard(true))
        .await;

    let msg: Message = match res {
        Ok(msg) => msg,
        Err(err) => {
            bot.send_message(
                CONFIG.get().unwrap().admin_chat,
                format!("问题发送失败，自动允许发送\n{}", err),
            )
            .await?;
            return Err(err.into());
        }
    };

    users.insert(msg.id.0, data);

    tokio::spawn(waiting_answer(bot.clone(), msg.id, async move {
        let mut users = UNVERIFIED_USERS.lock().await;
        if let btree_map::Entry::Occupied(data) = users.entry(msg.id.0) {
            delete_sent_message(bot, data).await.ok();
        }
    }));

    Ok(())
}

async fn waiting_answer<F>(bot: Bot, msg_id: MessageId, timeout: F)
where
    F: Future<Output = ()>,
{
    loop {
        sleep(std::time::Duration::from_secs(60)).await;

        let mut users = UNVERIFIED_USERS.lock().await;
        if let btree_map::Entry::Occupied(mut data) = users.entry(msg_id.0) {
            data.get_mut().left_minutes -= 1;
            let data = data.get();
            if data.left_minutes == 0 {
                break;
            } else {
                bot.edit_message_text(data.chat_id, msg_id, data.message())
                    .reply_markup(data.keyboard(false))
                    .await
                    .ok();
            }
        } else {
            break;
        }
    }

    timeout.await;
}
