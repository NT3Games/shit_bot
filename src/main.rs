#![feature(associated_type_defaults)]
#![allow(incomplete_features)]
#![feature(specialization)]

use std::{fmt, path::Path, time::Instant};

use admin::handler::Handler;
use anyhow::{Result, bail};
use chrono::Local;
use fancy_regex::Regex;
use redis::{AsyncCommands, aio::MultiplexedConnection};
use serde::{
    Deserialize, Deserializer,
    de::{self, Unexpected, Visitor},
};
use teloxide::{
    RequestError,
    dispatching::UpdateFilterExt,
    prelude::*,
    types::MessageId,
    update_listeners::{self},
    utils::command::BotCommands,
};
use tokio::{io::AsyncReadExt, sync::OnceCell};
use utils::EasySendMessage;

pub mod admin;
pub mod error;
pub mod question;
pub mod utils;

pub type Bot = teloxide::Bot;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub token: String,
    pub to_chat: ChatId,
    pub listen_chat: ChatId,
    pub manage_chat: ChatId,
    pub admin_log: ChatId,
    pub master_channel: ChatId,
    pub watch_list: Vec<UserId>,
    #[serde(default)]
    pub questions: Vec<question::Question>,
    #[serde(deserialize_with = "de_regex")]
    pub forward_pattern: Regex,
    pub redis: String,
}

fn de_regex<'de, D>(de: D) -> Result<Regex, D::Error>
where
    D: Deserializer<'de>,
{
    struct RegexVisitor;

    impl Visitor<'_> for RegexVisitor {
        type Value = Regex;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            write!(formatter, "a string containing a regex")
        }

        fn visit_str<E>(self, s: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Regex::new(s).map_err(|_e| de::Error::invalid_value(Unexpected::Str(s), &self))
        }
    }

    de.deserialize_any(RegexVisitor)
}

static CLIENT: OnceCell<redis::Client> = OnceCell::const_new();

static CONNECTION: OnceCell<MultiplexedConnection> = OnceCell::const_new();
async fn get_connection() -> MultiplexedConnection {
    CONNECTION
        .get_or_init(|| async {
            CLIENT
                .get()
                .expect("Redis client not initialized")
                .get_multiplexed_async_connection()
                .await
                .unwrap()
        })
        .await
        .clone()
}

pub static CONFIG: OnceCell<Config> = OnceCell::const_new();

const LAST_SENT_KEY: &str = "_shit_bot_last_send_message";
const LAST_SHIT_KEY: &str = "_shit_bot_last_shit_message";

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();
    log::info!("Starting shit bot...");

    let os_conf;
    let conf: &Path = if let Some(conf) = std::env::var_os("SHIT_BOT_CONFIG") {
        os_conf = conf;
        log::info!("Using config from env: {:?}", os_conf);
        os_conf.as_ref()
    } else {
        log::info!("Using default config: config.yaml");
        "config.yaml".as_ref()
    };
    let os_q;
    let (questions_conf, explicit): (&Path, _) = if let Some(conf) = std::env::var_os("SHIT_BOT_QUESTIONS") {
        os_q = conf;
        log::info!("Using questions file from env: {:?}", os_q);
        (os_q.as_ref(), true)
    } else {
        log::info!("Try default questions file: questions.yaml");
        ("questions.yaml".as_ref(), false)
    };

    let mut config = serde_yaml::from_slice::<Config>(&tokio::fs::read(conf).await?)?;

    match tokio::fs::File::open(questions_conf).await {
        Ok(mut ques_file) => {
            let mut buf = Vec::new();
            ques_file.read_to_end(&mut buf).await?;
            let questions: Vec<question::Question> = serde_yaml::from_slice(&buf)?;
            config.questions.extend(questions);
        }
        Err(e) => {
            if explicit {
                log::error!("Failed to open questions file: {}", e);
            } else {
                log::info!("Default questions file not exists.");
            }
        }
    }
    if config.questions.is_empty() {
        bail!("No questions loaded.");
    } else {
        log::info!("Loaded {} questions.", config.questions.len());
    }

    if let Ok(token) = std::env::var("SHIT_BOT_TOKEN") {
        log::info!("Using token from environment variable.");
        config.token = token;
    }
    if config.token.is_empty() {
        bail!("No bot token provided in config or environment (SHIT_BOT_TOKEN).");
    }

    CLIENT.set(redis::Client::open(config.redis.clone()).unwrap()).unwrap();

    let bot = teloxide::Bot::new(config.token.clone());

    CONFIG.set(config)?;

    let handler = dptree::entry()
        .branch(
            Update::filter_chat_member()
                .filter(|update: ChatMemberUpdated| {
                    update.chat.id == CONFIG.get().unwrap().manage_chat
                        && !update.old_chat_member.is_present()
                        && update.new_chat_member.is_present()
                })
                .endpoint(|bot: Bot, update: ChatMemberUpdated| async move {
                    let res = admin::join_handler::JoinHandler
                        .send_question(
                            bot.clone(),
                            update.new_chat_member.user.clone(),
                            update.chat.clone(),
                            (),
                        )
                        .await;

                    if let Err(err) = res {
                        bot.send_message(CONFIG.get().unwrap().admin_log, format!("{}", err))
                            .await?;
                        return Err(err);
                    }
                    Ok(())
                }),
        )
        .branch(
            Update::filter_message()
                .branch(dptree::filter(|msg: Message| msg.is_automatic_forward()).endpoint(auto_unpin))
                .branch(dptree::entry().filter_command::<Command>().endpoint(command_handle))
                .branch(
                    dptree::filter_async(|msg: Message| async move {
                        let Some(user) = msg.from.as_ref() else {
                            return false;
                        };

                        if !(msg.chat.id == CONFIG.get().unwrap().manage_chat) {
                            return false;
                        }

                        use teloxide::types::MessageEntityKind::*;
                        let has_link = [msg.entities(), msg.caption_entities()]
                            .into_iter()
                            .flatten()
                            .flatten()
                            .any(|e| matches!(e.kind, Url | Mention | TextLink { .. } | TextMention { .. }));

                        if !has_link {
                            return false;
                        }

                        match admin::auth_database::is_authed(user.id.0).await {
                            Ok(is_authed) =>
                            {
                                #[allow(clippy::needless_bool)]
                                if is_authed {
                                    false
                                } else {
                                    true
                                }
                            }
                            Err(err) => {
                                log::error!("Redis error: {}", err);
                                false
                            }
                        }
                    })
                    .endpoint(|bot: Bot, msg: Message| async move {
                        log::debug!("Potential spam message");
                        if let Some(user) = msg.from {
                            let res = admin::link_handler::LinkHandler
                                .send_question(bot.clone(), user.to_owned(), msg.chat.clone(), msg.id)
                                .await;

                            if let Err(err) = res {
                                bot.send_message(CONFIG.get().unwrap().admin_log, format!("{}", err))
                                    .await?;
                                return Err(err);
                            }
                        }

                        Ok(())
                    }),
                )
                .branch(
                    dptree::filter(|msg: Message| {
                        let config = CONFIG.get().unwrap();
                        if msg.from.is_none() {
                            return false;
                        }
                        let id = msg.from.as_ref().unwrap().id;
                        if msg.chat.id != config.listen_chat || !config.watch_list.contains(&id) {
                            return false;
                        }
                        if let Some(text) = msg.text() {
                            let text = text.trim().replace(['\r', '\n'], "");
                            config.forward_pattern.is_match(&text).unwrap()
                        } else {
                            false
                        }
                    })
                    .endpoint(forward_shit),
                ),
        )
        .branch(
            Update::filter_channel_post().endpoint(|_bot: Bot, msg: Message| async move {
                if msg.chat.id == CONFIG.get().unwrap().to_chat {
                    let mut con = get_connection().await;
                    () = con.set(LAST_SHIT_KEY, msg.id.0).await?;
                }
                Ok(())
            }),
        )
        .branch(Update::filter_edited_message().branch(
            dptree::filter(|msg: Message| msg.chat.id == CONFIG.get().unwrap().listen_chat).endpoint(edit_shit),
        ))
        .branch(
            Update::filter_callback_query().endpoint(|bot: Bot, callback: CallbackQuery| async move {
                let result = admin::callback(bot.clone(), callback.clone()).await;
                if let Err(e) = result {
                    bot.send_message(callback.message.unwrap().chat().id, format!("Error: {}", e))
                        .await?;
                }
                Ok(())
            }),
        );

    let polling = update_listeners::polling_default(bot.clone()).await;

    let start_time = Local::now();
    let start_instant = Instant::now();
    let pid = std::process::id();
    let cfg = CONFIG.get().unwrap();
    let authed_count = admin::auth_database::count_authed()
        .await
        .map(|c| c.to_string())
        .unwrap_or_else(|e| {
            log::error!("Failed to count authed users from Redis: {}", e);
            "获取失败".to_string()
        });

    let startup_msg = format!(
        "<b>🚀 机器人启动成功</b>\n\
         <b>PID：</b><code>{}</code>\n\
         <b>启动时间：</b><code>{}</code>\n\
         <b>已通过验证人数：</b><code>{}</code> 人\n\
         <b>题库数量：</b><code>{}</code> 题",
        pid,
        start_time.format("%Y-%m-%d %H:%M:%S %:z"),
        authed_count,
        cfg.questions.len(),
    );
    utils::admin_log(bot.clone(), startup_msg).await.ok();

    Dispatcher::builder(bot.clone(), handler)
        // .dependencies(dptree::deps![parameters])
        .default_handler(|upd| async move {
            log::trace!("Unhandled update: {:?}", upd);
        })
        .error_handler(LoggingErrorHandler::with_custom_text("Dispatcher"))
        .enable_ctrlc_handler()
        .build()
        .dispatch_with_listener(polling, error::UpdateErrorHandler::new())
        .await;

    let stop_time = Local::now();
    let uptime = format_duration(start_instant.elapsed());

    let shutdown_msg = format!(
        "<b>🛑 机器人已关闭</b>\n\
         <b>PID：</b><code>{}</code>\n\
         <b>停止时间：</b><code>{}</code>\n\
         <b>运行时长：</b><code>{}</code>",
        pid,
        stop_time.format("%Y-%m-%d %H:%M:%S %:z"),
        uptime,
    );
    utils::admin_log(bot, shutdown_msg).await.ok();

    Ok(())
}

fn format_duration(duration: std::time::Duration) -> String {
    let total_secs = duration.as_secs();
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if days > 0 {
        format!("{}天 {}小时 {}分 {}秒", days, hours, mins, secs)
    } else if hours > 0 {
        format!("{}小时 {}分 {}秒", hours, mins, secs)
    } else if mins > 0 {
        format!("{}分 {}秒", mins, secs)
    } else {
        format!("{}.{:02}秒", secs, duration.subsec_millis() / 10)
    }
}

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "一个帮助记载屎书的机器人：")]
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
    if message.from.is_none() {
        return Ok(());
    }
    let config = CONFIG.get().unwrap();
    match command {
        Command::Help => {
            bot.send_message(message.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Command::Source => {
            bot.send_message(message.chat.id, "https://github.com/NT3Games/shit_bot")
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
                .get_chat_member(config.to_chat, message.from.as_ref().unwrap().id)
                .await;
            if let Err(RequestError::Api(teloxide::ApiError::UserNotFound)) = chat_member {
                let request = bot
                    .send_message(message.chat.id, "请先加入 https://t.me/nipple_hill 以使用此命令")
                    .reply_to_message_id(message.id);
                replace_send(bot, request).await?;
                return Ok(());
            } else {
                chat_member?;
            }

            if let Some(reply) = message.reply_to_message() {
                forward_shit(bot.clone(), reply.to_owned()).await?;
                bot.delete_message(message.chat.id, message.id).await?;
            } else {
                let request = bot
                    .send_message(message.chat.id, "没有选择消息")
                    .reply_to_message_id(message.id);
                replace_send(bot, request).await?;
            };
        }
        Command::Pull => {
            let id: Option<i32> = {
                let mut con = get_connection().await;
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
                .get_chat_member(config.to_chat, message.from.as_ref().unwrap().id)
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
                let (res, mention) = if let Some(sender) = reply.sender_chat.as_ref() {
                    let name = if let Some(title) = sender.title() {
                        htmlescape::encode_minimal(title)
                    } else if let Some(username) = sender.username() {
                        format!("@{}", username)
                    } else {
                        "频道身份用户".to_string()
                    };
                    let mention = if let Some(username) = sender.username() {
                        format!("<a href=\"https://t.me/{}\">{}</a>", username, name)
                    } else {
                        name
                    };
                    (bot.ban_chat_sender_chat(message.chat.id, sender.id).await, mention)
                } else if let Some(sender) = reply.from.as_ref() {
                    let name = if let Some(username) = sender.username.as_ref() {
                        format!("{} (@{})", htmlescape::encode_minimal(&sender.full_name()), username)
                    } else {
                        htmlescape::encode_minimal(&sender.full_name())
                    };
                    let mention = format!("<a href=\"tg://user?id={}\">{}</a>", sender.id, name);
                    (
                        bot.restrict_chat_member(message.chat.id, sender.id, teloxide::types::ChatPermissions::empty())
                            .await,
                        mention,
                    )
                } else {
                    bot.send_message(message.chat.id, "无法获取目标用户信息")
                        .reply_to_message_id(message.id)
                        .await?;
                    return Ok(());
                };
                match res {
                    Ok(_) => {
                        bot.send_message(message.chat.id, format!("{} 的嘴已被屎球堵上", mention))
                            .reply_to_message_id(message.id)
                            .parse_mode(teloxide::types::ParseMode::Html)
                            .disable_web_page_preview()
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
        let mut con = get_connection().await;
        con.get(message.id.0).await?
    };

    if let Some(id) = sent {
        bot.send_message(CONFIG.get().unwrap().to_chat, message.text().unwrap())
            .reply_to_message_id(MessageId(id))
            .await?;
    }
    Ok(())
}

async fn forward_shit(bot: Bot, message: Message) -> Result<()> {
    let sent = bot
        .forward_message(CONFIG.get().unwrap().to_chat, message.chat.id, message.id)
        .await?;

    {
        let mut con = get_connection().await;
        () = con.set(LAST_SHIT_KEY, sent.id.0).await?;
    }

    let request = bot
        .send_message(message.chat.id, format!("https://t.me/nipple_hill/{}", sent.id))
        .reply_to_message_id(message.id)
        .disable_web_page_preview();
    replace_send(bot, request).await?;

    {
        let mut con = get_connection().await;
        () = con.set(message.id.0, sent.id.0).await?;
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
    let res = message.await?;

    let mut con = get_connection().await;
    let last: Option<i32> = con.get(LAST_SENT_KEY).await?;
    () = con.set(LAST_SENT_KEY, res.id.0).await?;
    if let Some(id) = last {
        bot.delete_message(source, MessageId(id)).await?;
    }
    Ok(())
}

async fn auto_unpin(bot: Bot, message: Message) -> Result<()> {
    let res = bot.unpin_chat_message(message.chat.id).message_id(message.id).await;

    if let Err(err) = res {
        bot.send_message(message.chat.id, err.to_string())
            .reply_to_message_id(message.id)
            .await?;
        return Err(err.into());
    }

    Ok(())
}
