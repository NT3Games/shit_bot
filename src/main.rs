use anyhow::Result;
use redis::AsyncCommands;
use teloxide::{
    prelude::*,
    types::{ChatId, UserId},
    utils::command::BotCommands,
    RequestError,
};
use tokio::sync::OnceCell;

const SHIT_HILL: ChatId = ChatId(0 /*CLEANED*/);
const SOURCE: ChatId = ChatId(0 /*CLEANED*/);
const NT3: UserId = UserId(0 /*CLEANED*/);
const TRACEWIND: UserId = UserId(0 /*CLEANED*/);

static CLIENT: OnceCell<redis::Client> = OnceCell::const_new();
async fn get_client() -> &'static redis::Client {
    CLIENT
        .get_or_init(|| async { redis::Client::open("unix:///run/redis/redis.sock").unwrap() })
        .await
}

const LAST_SENT_KEY: &str = "_shit_bot_last_send_message";
const LAST_SHIT_KEY: &str = "_shit_bot_last_shit_message";

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    log::info!("Starting shit bot...");

    let bot = Bot::from_env();

    let handler = Update::filter_message()
        .branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(command_handle),
        )
        .branch(
            dptree::filter(|msg: Message| {
                if msg.from().is_none() {
                    return false;
                }
                let id = msg.from().unwrap().id;
                if msg.chat.id != SOURCE || (id != NT3 && id != TRACEWIND) {
                    return false;
                }
                if let Some(text) = msg.text() {
                    text.len() > 5 && (text.contains('屎') || text.contains('💩'))
                } else {
                    false
                }
            })
            .endpoint(forward_shit),
        )
        .branch(
            dptree::filter(|msg: Message| msg.chat.id == SHIT_HILL).endpoint(
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
    match command {
        Command::Help => {
            bot.send_message(message.chat.id, Command::descriptions().to_string())
                .send()
                .await?;
        }
        Command::Source => {
            bot.send_message(message.chat.id, "https://gitlab.com/71e6fd52/shit_bot")
                .send()
                .await?;
        }
        Command::Shit => {
            if message.from().is_none() {
                return Ok(());
            }
            if message.chat.id != SOURCE {
                let mut request = bot.send_message(message.chat.id, "机器人不允许在此处使用");
                request.reply_to_message_id = Some(message.id);
                request.send().await?;
                return Ok(());
            };
            let chat_member = bot
                .get_chat_member(SHIT_HILL, message.from().unwrap().id)
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
            if message.chat.id != SOURCE {
                let mut request = bot.send_message(message.chat.id, "机器人不允许在此处使用");
                request.reply_to_message_id = Some(message.id);
                request.send().await?;
                return Ok(());
            };

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
        .forward_message(SHIT_HILL, message.chat.id, message.id)
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

    if message.chat_id != Id(SOURCE) {
        panic!()
    }
    let res = message.send().await?;

    let mut con = get_client().await.get_async_connection().await?;
    let last: Option<i32> = con.get(LAST_SENT_KEY).await?;
    con.set(LAST_SENT_KEY, res.id).await?;
    if let Some(id) = last {
        bot.delete_message(SOURCE, id).send().await?;
    }
    Ok(())
}
