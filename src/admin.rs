use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{offset::Utc, Duration};
use rand::{prelude::SliceRandom, thread_rng, Rng};
use teloxide::{
    payloads::{AnswerCallbackQuerySetters, BanChatMemberSetters, SendMessageSetters},
    prelude::Requester,
    types::{
        CallbackQuery, Chat, InlineKeyboardButton, InlineKeyboardMarkup, ParseMode, ReplyMarkup,
        User, UserId,
    },
};
use tokio::{sync::Mutex, time::sleep};

use crate::{Bot, CONFIG};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueryData {
    pub user_id: UserId,
    pub correct: u8,
    pub tried_times: u8,
}

static UNVERIFIED_USERS: Mutex<BTreeMap<i32, QueryData>> = Mutex::const_new(BTreeMap::new());

pub async fn send_auth(bot: Bot, user: User, chat: Chat) -> Result<()> {
    // let mut rng = thread_rng();
    let question = CONFIG
        .get()
        .unwrap()
        .questions
        .choose(&mut thread_rng())
        .expect("no question");
    let correct = question
        .correct
        .choose(&mut thread_rng())
        .expect("no correct answer");
    let mut buttons = question
        .wrong
        .choose_multiple(&mut thread_rng(), 3)
        .collect::<Vec<_>>();
    let correct_idx = thread_rng().gen_range(0..=buttons.len());
    buttons.insert(correct_idx, correct);

    // mute user
    let res = bot
        .restrict_chat_member(chat.id, user.id, teloxide::types::ChatPermissions::empty())
        .await;
    if let Err(err) = res {
        bot.send_message(chat.id, err.to_string()).await?;
        return Err(err.into());
    }

    let keyboard = InlineKeyboardMarkup::default().append_row(
        buttons
            .into_iter()
            .enumerate()
            .map(|(idx, text)| InlineKeyboardButton::callback(text, idx.to_string())),
    );

    let mut users = UNVERIFIED_USERS.lock().await;

    let msg = bot
        .send_message(
            chat.id,
            format!("你有 5 分钟时间回答以下问题：\n\n{}", question.title),
        )
        .parse_mode(ParseMode::MarkdownV2)
        .reply_markup(ReplyMarkup::InlineKeyboard(keyboard))
        .await?;

    users.insert(
        msg.id,
        QueryData {
            user_id: user.id,
            correct: correct_idx as u8,
            tried_times: 0,
        },
    );

    let bot2 = bot.clone();

    tokio::spawn(async move {
        sleep(std::time::Duration::from_secs(5 * 60)).await;
        let mut users = UNVERIFIED_USERS.lock().await;
        if let Some(_data) = users.get_mut(&msg.id) {
            let res = bot2
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

    if callback.from.id != data.user_id {
        bot.answer_callback_query(callback.id)
            .text("别抢别人的屎！")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    if callback.data.unwrap() == data.correct.to_string() {
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

        users.remove(&origin.id);
    } else {
        if data.tried_times >= 2 {
            bot.answer_callback_query(callback.id)
                .text("验证失败，失败次数过多，请十分钟后重试")
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
