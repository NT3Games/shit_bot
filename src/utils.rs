use anyhow::Result;
use redis::AsyncCommands;
use teloxide::{
    payloads::SendMessageSetters,
    requests::Requester,
    types::{ChatId, MessageId, ParseMode, User},
    Bot,
};

const LAST_JOIN_RESULT_KEY: &str = "shit_bot_last_join_result";

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

pub fn is_spam_name(name: &str) -> bool {
    name.contains("免费") || name.contains("VPN") || name.contains("梯子")
}

pub fn rank_user(user: &User) -> f64 {
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

pub async fn send_and_delete_join_result(bot: Bot, chat_id: ChatId, message: String) -> Result<()> {
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
    let last: Option<i32> = con.get(format!("{}/{}", LAST_JOIN_RESULT_KEY, chat_id)).await?;
    con.set(LAST_JOIN_RESULT_KEY, res.id.0).await?;
    if let Some(id) = last {
        bot.delete_message(chat_id, MessageId(id)).await?;
    }

    Ok(())
}

pub async fn admin_log(bot: Bot, message: String) -> Result<()> {
    bot.send_message(crate::CONFIG.get().unwrap().admin_chat, message)
        .await?;

    Ok(())
}
