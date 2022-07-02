#![feature(const_btree_new)]
use anyhow::Result;
use log::debug;
use redis::AsyncCommands;
use serde::Deserialize;
use teloxide::{
    dispatching::UpdateFilterExt,
    prelude::*,
    types::{ChatId, UserId},
    utils::command::BotCommands,
    RequestError,
};
use tokio::{fs::File, io::AsyncReadExt, sync::OnceCell};

pub mod admin;

type Bot = AutoSend<teloxide::Bot>;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub token: String,
    pub to_chat: ChatId,
    pub listen_chat: ChatId,
    pub watch_list: Vec<UserId>,
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub title: String,
    pub wrong: Vec<String>,
    pub correct: Vec<String>,
}

static CLIENT: OnceCell<redis::Client> = OnceCell::const_new();
async fn get_client() -> &'static redis::Client {
    CLIENT
        .get_or_init(|| async { redis::Client::open("unix:///run/redis/redis.sock").unwrap() })
        .await
}

pub static CONFIG: OnceCell<Config> = OnceCell::const_new();

const LAST_SENT_KEY: &str = "_shit_bot_last_send_message";
const LAST_SHIT_KEY: &str = "_shit_bot_last_shit_message";

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();
    log::info!("Starting shit bot...");

    let mut f = File::open("config.yaml").await?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).await?;

    let config = serde_yaml::from_slice::<Config>(&buf)?;

    let bot = teloxide::Bot::new(config.token.clone()).auto_send();

    CONFIG.set(config)?;

    let handler = dptree::entry()
        .branch(
            Update::filter_message()
                .branch(Message::filter_new_chat_members().endpoint(
                    |bot: Bot, msg: Message| async move {
                        if let Some(users) = msg.new_chat_members() {
                            for user in users {
                                admin::send_auth(bot.clone(), user.to_owned(), msg.chat.clone())
                                    .await?;
                            }
                        }

                        Ok(())
                    },
                ))
                .branch(
                    dptree::filter(|msg: Message| msg.is_automatic_forward()).endpoint(auto_unpin),
                )
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .endpoint(command_handle),
                )
                .branch(
                    dptree::filter(|msg: Message| {
                        let config = CONFIG.get().unwrap();
                        if msg.from().is_none() {
                            return false;
                        }
                        let id = msg.from().unwrap().id;
                        if msg.chat.id != config.listen_chat || !config.watch_list.contains(&id) {
                            return false;
                        }
                        if let Some(text) = msg.text() {
                            let text = text.trim();
                            text.contains("等我长大以后")
                                || (text.chars().nth(5).is_some() // len > 5
                            && (text.contains('屎') || text.contains('💩'))
                            && !(text.contains("屎公仔")
                                || text.contains("屎娃娃")
                                || text.contains("小屎屎"))
                            && !text.ends_with('~'))
                        } else {
                            false
                        }
                    })
                    .endpoint(forward_shit),
                )
                .branch(
                    dptree::filter(|msg: Message| msg.chat.id == CONFIG.get().unwrap().to_chat)
                        .endpoint(|_bot: Bot, message: Message| async move {
                            let mut con = get_client().await.get_async_connection().await?;
                            con.set(LAST_SHIT_KEY, message.id).await?;
                            Ok(())
                        }),
                ),
        )
        .branch(
            Update::filter_edited_message().branch(
                dptree::filter(|msg: Message| msg.chat.id == CONFIG.get().unwrap().listen_chat)
                    .endpoint(edit_shit),
            ),
        )
        .branch(Update::filter_callback_query().endpoint(admin::callback));
    // teloxide::commands_repl(bot, answer, Command::ty()).await;

    Dispatcher::builder(bot, handler)
        // .dependencies(dptree::deps![parameters])
        .default_handler(|upd| async move {
            log::trace!("Unhandled update: {:?}", upd);
        })
        .error_handler(LoggingErrorHandler::with_custom_text(
            "An error has occurred in the dispatcher",
        ))
        .build()
        .setup_ctrlc_handler()
        .dispatch()
        .await;

    Ok(())
}

#[derive(BotCommands, Clone)]
#[command(rename = "lowercase", description = "一个帮助记载屎书的机器人：")]
enum Command {
    #[command(description = "发送帮助文字")]
    Help,
    #[command(description = "转发到屎书")]
    Shit,
    #[command(description = "查看源代码")]
    Source,
    #[command(description = "“拉”出最后的屎")]
    Pull,
    #[command(description = "屎球堵嘴")]
    Bullshit,
}

async fn command_handle(bot: Bot, message: Message, command: Command) -> Result<()> {
    if message.from().is_none() {
        return Ok(());
    }
    let config = CONFIG.get().unwrap();
    match command {
        Command::Help => {
            bot.send_message(message.chat.id, Command::descriptions().to_string())
                .send()
                .await?;
        }
        Command::Source => {
            bot.send_message(message.chat.id, "https://github.com/NT3Games/shit_bot")
                .send()
                .await?;
        }
        Command::Shit => {
            if message.chat.id != config.listen_chat {
                bot.send_message(message.chat.id, "机器人不允许在此处使用")
                    .reply_to_message_id(message.id)
                    .await?;
                return Ok(());
            };
            let chat_member = bot
                .get_chat_member(config.to_chat, message.from().unwrap().id)
                .send()
                .await;
            if let Err(RequestError::Api(teloxide::ApiError::UserNotFound)) = chat_member {
                let request = bot
                    .inner()
                    .send_message(
                        message.chat.id,
                        "请先加入 https://t.me/nipple_hill 以使用此命令",
                    )
                    .reply_to_message_id(message.id);
                replace_send(bot, request).await?;
                return Ok(());
            } else {
                chat_member?;
            }

            if let Some(reply) = message.reply_to_message() {
                forward_shit(bot.clone(), reply.to_owned()).await?;
                bot.delete_message(message.chat.id, message.id)
                    .send()
                    .await?;
            } else {
                let request = bot
                    .inner()
                    .send_message(message.chat.id, "没有选择消息")
                    .reply_to_message_id(message.id);
                replace_send(bot, request).await?;
            };
        }
        Command::Pull => {
            let id: Option<i32> = {
                let mut con = get_client().await.get_async_connection().await?;
                con.get(LAST_SHIT_KEY).await?
            };
            let text = if let Some(id) = id {
                format!("https://t.me/nipple_hill/{}", id)
            } else {
                "未找到！".to_string()
            };

            bot.send_message(message.chat.id, text)
                .reply_to_message_id(message.id)
                .await?;
        }
        Command::Bullshit => {
            let privileged = bot
                .get_chat_member(config.to_chat, message.from().unwrap().id)
                .send()
                .await
                .map(|c| c.is_privileged())
                .unwrap_or(false);
            if !privileged {
                bot.send_message(message.chat.id, "你没有权限使用此命令")
                    .reply_to_message_id(message.id)
                    .await?;
                return Ok(());
            }
            if let Some(reply) = message.reply_to_message() {
                let (res, name) = if let Some(sender) = reply.sender_chat() {
                    (
                        bot.ban_chat_sender_chat(message.chat.id, sender.id).await,
                        if let Some(title) = sender.title() {
                            title.to_string()
                        } else if let Some(username) = sender.username() {
                            format!("@{}", username)
                        } else {
                            "频道身份用户".to_string()
                        },
                    )
                } else {
                    let sender = reply.from().unwrap();
                    (
                        bot.restrict_chat_member(
                            message.chat.id,
                            sender.id,
                            teloxide::types::ChatPermissions::empty(),
                        )
                        .await,
                        if let Some(username) = sender.username.as_ref() {
                            format!("{} (@{})", sender.full_name(), username)
                        } else {
                            sender.full_name()
                        },
                    )
                };
                match res {
                    Ok(_) => {
                        bot.send_message(
                            message.chat.id,
                            format!(
                                "<a href=\"tg://user?id={}\">{}</a> 的嘴已被屎球堵上",
                                reply.from().unwrap().id,
                                name
                            ),
                        )
                        .reply_to_message_id(message.id)
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                    }
                    Err(e) => {
                        bot.send_message(message.chat.id, e.to_string())
                            .reply_to_message_id(message.id)
                            .await?;
                    }
                }
            } else {
                bot.send_message(message.chat.id, "没有选择消息")
                    .reply_to_message_id(message.id)
                    .await?;
            };
        }
    };

    Ok(())
}

async fn edit_shit(bot: Bot, message: Message) -> Result<()> {
    if message.text().is_none() {
        return Ok(());
    }
    let sent: Option<i32> = {
        let mut con = get_client().await.get_async_connection().await?;
        con.get(message.id).await?
    };

    if let Some(id) = sent {
        bot.send_message(
            CONFIG.get().unwrap().to_chat,
            format!("修改为：\n{}", message.text().unwrap()),
        )
        .reply_to_message_id(id)
        .await?;
    }
    Ok(())
}

async fn forward_shit(bot: Bot, message: Message) -> Result<()> {
    let sent = bot
        .forward_message(CONFIG.get().unwrap().to_chat, message.chat.id, message.id)
        .send()
        .await?;

    {
        let mut con = get_client().await.get_async_connection().await?;
        con.set(LAST_SHIT_KEY, sent.id).await?;
    }

    let request = bot
        .inner()
        .send_message(
            message.chat.id,
            format!("https://t.me/nipple_hill/{}", sent.id),
        )
        .reply_to_message_id(message.id)
        .disable_web_page_preview(true);
    replace_send(bot, request).await?;

    {
        let mut con = get_client().await.get_async_connection().await?;
        con.set(message.id, sent.id).await?;
    }

    Ok(())
}

async fn replace_send(
    bot: Bot,
    message: teloxide::requests::JsonRequest<teloxide::payloads::SendMessage>,
) -> Result<()> {
    use teloxide::types::Recipient::Id;

    let source = CONFIG.get().unwrap().listen_chat;

    if message.chat_id != Id(source) {
        panic!()
    }
    let res = message.send().await?;

    let mut con = get_client().await.get_async_connection().await?;
    let last: Option<i32> = con.get(LAST_SENT_KEY).await?;
    con.set(LAST_SENT_KEY, res.id).await?;
    if let Some(id) = last {
        bot.delete_message(source, id).send().await?;
    }
    Ok(())
}

async fn auto_unpin(bot: Bot, message: Message) -> Result<()> {
    let res = bot
        .unpin_chat_message(message.chat.id)
        .message_id(message.id)
        .await;

    if let Err(err) = res {
        bot.send_message(message.chat.id, err.to_string())
            .reply_to_message_id(message.id)
            .await?;
        return Err(err.into());
    }

    Ok(())
}
