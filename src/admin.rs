use std::{collections::BTreeMap, future::Future, ops::DerefMut};

use anyhow::Result;
use crossbeam_queue::SegQueue;
use handler::Handler;
use serde::Deserialize;
use teloxide::{
    payloads::{AnswerCallbackQuerySetters, EditMessageTextSetters},
    prelude::*,
    types::{ChatMember, InlineKeyboardButton, InlineKeyboardMarkup, MessageId, ParseMode, User},
};
use tokio::{
    sync::{MappedMutexGuard, Mutex, MutexGuard},
    task::AbortHandle,
    time::sleep,
};

use crate::{Bot, question, utils::*};

pub mod auth_database;
pub mod handler;
pub mod join_handler;
pub mod link_handler;

pub const AUTHED_USERS_KEY: &str = "shit_bot_authed_users";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionData {
    pub user: User,
    pub chat_id: ChatId,
    pub message_id: Option<MessageId>, // spam message
    pub correct: usize,
    pub title: &'static str,
    pub options: Vec<&'static String>,
    pub tried_times: u8,
    pub cas: Option<MessageId>, // i32 is message id
    pub left_minutes: u8,
    pub can_change: bool,
    pub handler: handler::HandlerKind,
}

impl QuestionData {
    pub fn new(user: User, chat_id: ChatId, message_id: Option<MessageId>, handler: handler::HandlerKind) -> Self {
        let (title, options, correct) = question::new_question();

        QuestionData {
            user,
            chat_id,
            message_id,
            title,
            options,
            correct,
            tried_times: 0,
            cas: None,
            left_minutes: 5,
            can_change: true,
            handler,
        }
    }

    pub fn message(&self) -> String {
        format!(
            "{}，你有 {} 分钟时间回答以下问题：\n\n{}",
            mention_user(&self.user),
            self.left_minutes,
            self.title
        )
    }

    pub fn keyboard(&self) -> InlineKeyboardMarkup {
        let mut keyboard = InlineKeyboardMarkup::new(
            self.options
                .iter()
                .enumerate()
                .map(|(idx, &text)| vec![InlineKeyboardButton::callback(text, idx.to_string())]),
        );
        if self.can_change {
            keyboard = keyboard.append_row(vec![InlineKeyboardButton::callback("换题🔁", "change")])
        }
        self.handler.keyboard_patch(keyboard)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
struct CasResult {
    pub ok: bool,
}

pub struct WaitingManager {
    // question message id as key
    datas: BTreeMap<i32, QuestionData>,
    handles: BTreeMap<i32, Vec<AbortHandle>>,
}

impl WaitingManager {
    pub const fn new() -> Self {
        Self {
            datas: BTreeMap::new(),
            handles: BTreeMap::new(),
        }
    }

    pub async fn add(&mut self, question_id: i32, data: QuestionData) {
        self.datas.insert(question_id, data);
        self.handles.insert(question_id, vec![]);
    }

    pub async fn finish(&mut self, question_id: i32) -> Option<(i32, QuestionData)> {
        if let Some(handles) = self.handles.remove(&question_id) {
            for handle in handles {
                handle.abort();
            }
        }
        self.datas.remove_entry(&question_id)
    }

    pub fn add_handle(&mut self, question_id: i32, handle: AbortHandle) {
        if let Some(handles) = self.handles.get_mut(&question_id) {
            handles.push(handle);
        }
    }
}

impl Default for WaitingManager {
    fn default() -> Self {
        Self::new()
    }
}

// question message id as key
// static WATING_MESSAGES: Mutex<BTreeMap<i32, QuestionData>> = Mutex::const_new(BTreeMap::new());
// static WATING_MESSAGES_TASKS: Mutex<BTreeMap<i32, Vec<AbortHandle>>> =
//     Mutex::const_new(BTreeMap::new());
static WATING_MANAGER: Mutex<WaitingManager> = Mutex::const_new(WaitingManager::new());

static TO_DELETE_MESSAGE: SegQueue<(ChatId, MessageId)> = SegQueue::new();

pub async fn add_wating_user(question_id: MessageId, data: QuestionData) {
    WATING_MANAGER.lock().await.add(question_id.0, data).await;
}

pub async fn get_data_by_msg(msg_id: &i32) -> Option<MappedMutexGuard<'static, QuestionData>> {
    let users = WATING_MANAGER.lock().await;
    MutexGuard::try_map(users, |users| users.datas.get_mut(msg_id)).ok()
}

pub async fn user_finish(msg_id: MessageId) -> Option<(i32, QuestionData)> {
    WATING_MANAGER.lock().await.finish(msg_id.0).await
}

pub async fn add_wating_handle(msg_id: MessageId, handle: AbortHandle) {
    WATING_MANAGER.lock().await.add_handle(msg_id.0, handle);
}

pub struct CallbackResult {
    pub typ: CallbackResultType,
    pub msg: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackResultType {
    Answer,
    HandleCorrect,
    HandleWrong,
    HandleOther,
}

pub async fn callback(bot: Bot, callback: CallbackQuery) -> Result<()> {
    if callback.message.is_none() || callback.data.is_none() {
        bot.answer_callback_query(callback.id).await?;
        return Ok(());
    }
    let msg_id = callback.message.as_ref().unwrap().id();
    let callback_id = callback.id.clone();

    let (result, mut handler) = {
        if let Some(mut data) = get_data_by_msg(&msg_id.0).await {
            let result = callback_handle(bot.clone(), &callback, data.deref_mut()).await?;

            if result.typ == Answer {
                if let Some(ref msg) = result.msg {
                    bot.answer_callback_query(callback.id)
                        .text(msg)
                        .show_alert(true)
                        .await?;
                } else {
                    bot.answer_callback_query(callback.id).await?;
                }
            }
            (result, data.handler)
        } else {
            bot.answer_callback_query(callback.id).await?;
            return Ok(());
        }
    };

    let bot2 = bot.clone();
    use CallbackResultType::*;
    let res = match result.typ {
        Answer => return Ok(()),
        HandleCorrect => handler.handle_correct(bot, msg_id).await,
        HandleWrong => handler.handle_wrong(bot, msg_id).await,
        HandleOther => handler.handle_other(bot, callback.data.as_ref().unwrap(), msg_id).await,
    };
    let res = match res {
        Ok(res) => res,
        Err(err) => {
            bot2.answer_callback_query(callback_id).await?;
            return Err(err);
        }
    };
    if let Some(msg) = res {
        bot2.answer_callback_query(callback_id)
            .text(msg)
            .show_alert(true)
            .await?;
    } else {
        bot2.answer_callback_query(callback_id).await?;
    }
    while let Some((chat, msg)) = TO_DELETE_MESSAGE.pop() {
        bot2.delete_message(chat, msg).await?;
    }
    Ok(())
}

async fn callback_handle(bot: Bot, callback: &CallbackQuery, data: &mut QuestionData) -> Result<CallbackResult> {
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

    if callback_data.starts_with("admin") {
        let res: std::result::Result<ChatMember, teloxide::RequestError> =
            bot.get_chat_member(origin.chat().id, callback.from.id).await;
        let member: ChatMember = match res {
            Ok(member) => member,
            Err(err) => {
                return res!(Answer, ("{}", err));
            }
        };
        if member.is_privileged() {
            return res!(HandleOther);
        } else {
            return res!(Answer, "只有管理员可以点击此按钮");
        }
    }

    if callback.from.id != data.user.id {
        return res!(
            Answer,
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
        res!(HandleCorrect)
    } else if callback_data == "change" {
        let (title, options, correct_idx) = question::new_question();
        data.correct = correct_idx;
        data.options = options;
        data.title = title;
        data.can_change = false;
        bot.edit_message_text(origin.chat().id, origin.id(), data.message())
            .parse_mode(ParseMode::Html)
            .reply_markup(data.keyboard())
            .await?;
        res!(Answer)
    } else {
        res!(HandleWrong)
    }
}

async fn waiting_answer<Fn, F>(bot: Bot, msg_id: MessageId, timeout: Fn)
where
    Fn: FnOnce((i32, QuestionData)) -> F + Send + 'static,
    F: Future<Output = ()>,
{
    loop {
        sleep(std::time::Duration::from_secs(60)).await;

        if let Some(mut data) = get_data_by_msg(&msg_id.0).await {
            data.left_minutes -= 1;
            if data.left_minutes == 0 {
                break;
            } else {
                bot.edit_message_text(data.chat_id, msg_id, data.message())
                    .parse_mode(ParseMode::Html)
                    .reply_markup(data.keyboard())
                    .await
                    .ok();
            }
        } else {
            break;
        }
    }

    if let Some(data) = user_finish(msg_id).await {
        timeout(data).await;
    }
    while let Some((chat, msg)) = TO_DELETE_MESSAGE.pop() {
        bot.delete_message(chat, msg).await.ok();
    }
}
