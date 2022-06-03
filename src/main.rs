use anyhow::Result;
use redis::AsyncCommands;
use serde::Deserialize;
use teloxide::{
    prelude::*,
    types::{ChatId, UserId},
    utils::command::BotCommands,
    RequestError,
};
use tokio::{fs::File, io::AsyncReadExt, sync::OnceCell};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub token: String,
    pub to_chat: ChatId,
    pub listen_chat: ChatId,
    pub watch_list: Vec<UserId>,
}

static CLIENT: OnceCell<redis::Client> = OnceCell::const_new();
async fn get_client() -> &'static redis::Client {
    CLIENT
        .get_or_init(|| async { redis::Client::open("unix:///run/redis/redis.sock").unwrap() })
        .await
}

static CONFIG: OnceCell<Config> = OnceCell::const_new();

const LAST_SENT_KEY: &str = "_shit_bot_last_send_message";
const LAST_SHIT_KEY: &str = "_shit_bot_last_shit_message";

#[tokio::main]
async fn main() -> Result<()> {
    pretty_env_logger::init();
    log::info!("Starting shit bot...");

    let mut f = File::open("config.toml").await?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).await?;

    let config = toml::from_slice::<Config>(&buf)?;

    let bot = Bot::new(config.token.clone());

    CONFIG.set(config)?;

    let handler = Update::filter_message()
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
            dptree::filter(|msg: Message| msg.chat.id == CONFIG.get().unwrap().to_chat).endpoint(
                |_bot: Bot, message: Message| async move {
                    let mut con = get_client().await.get_async_connection().await?;
                    con.set(LAST_SHIT_KEY, message.id).await?;
                    Ok(())
                },
            ),
        );
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
}

async fn command_handle(bot: Bot, message: Message, command: Command) -> Result<()> {
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
            if message.from().is_none() {
                return Ok(());
            }
            if message.chat.id != config.listen_chat {
                let mut request = bot.send_message(message.chat.id, "机器人不允许在此处使用");
                request.reply_to_message_id = Some(message.id);
                request.send().await?;
                return Ok(());
            };
            let chat_member = bot
                .get_chat_member(config.to_chat, message.from().unwrap().id)
                .send()
                .await;
            if let Err(RequestError::Api(teloxide::ApiError::UserNotFound)) = chat_member {
                let mut request = bot.send_message(
                    message.chat.id,
                    "请先加入 https://t.me/nipple_hill 以使用此命令",
                );
                request.reply_to_message_id = Some(message.id);
                replace_send(bot, request).await?;
                return Ok(());
            } else {
                chat_member?;
            }

            if let Some(reply) = message.reply_to_message() {
                forward_shit(bot, reply.to_owned()).await?;
            } else {
                let mut request = bot.send_message(message.chat.id, "没有选择消息");
                request.reply_to_message_id = Some(message.id);
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

            let mut request = bot.send_message(message.chat.id, text);
            request.reply_to_message_id = Some(message.id);
            request.send().await?;
        }
    };

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

    let mut request = bot.send_message(
        message.chat.id,
        format!("https://t.me/nipple_hill/{}", sent.id),
    );
    request.reply_to_message_id = Some(message.id);
    request.disable_web_page_preview = Some(true);
    replace_send(bot, request).await?;
    // let res = request.send().await?;
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
