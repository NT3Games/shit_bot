use teloxide::{
    prelude::*,
    types::{ChatId, UserId},
    utils::command::BotCommands,
    RequestError,
};

const SHIT_HILL: ChatId = ChatId(0 /*CLEANED*/);
const SOURCE: ChatId = ChatId(0 /*CLEANED*/);
const NT3: UserId = UserId(0 /*CLEANED*/);

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    log::info!("Starting command bot...");

    let bot = Bot::from_env().auto_send();

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
                if msg.chat.id != SOURCE || msg.from().unwrap().id != NT3 {
                    return false;
                }
                if let Some(text) = msg.text() {
                    text.contains('屎') || text.contains('💩')
                } else {
                    false
                }
            })
            .endpoint(forward_shit),
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
}

async fn command_handle(
    bot: AutoSend<Bot>,
    message: Message,
    command: Command,
) -> Result<(), RequestError> {
    match command {
        Command::Help => {
            bot.send_message(message.chat.id, Command::descriptions().to_string())
                .await?;
        }
        Command::Shit => {
            if message.from().is_none() {
                return Ok(());
            }
            if message.chat.id != SOURCE {
                bot.send_message(message.chat.id, "机器人不允许在此处使用")
                    .await?;
                return Ok(());
            };
            let chat_member = bot
                .get_chat_member(SHIT_HILL, message.from().unwrap().id)
                .await;
            if let Err(RequestError::Api(teloxide::ApiError::UserNotFound)) = chat_member {
                let mut request = bot.inner().send_message(
                    message.chat.id,
                    "请先加入 https://t.me/nipple_hill 以使用此命令",
                );
                request.reply_to_message_id = Some(message.id);
                request.send().await?;
                return Ok(());
            } else {
                chat_member?;
            }

            if let Some(reply) = message.reply_to_message() {
                forward_shit(bot, reply.to_owned()).await?;
            } else {
                let mut request = bot.inner().send_message(message.chat.id, "没有选择消息");
                request.reply_to_message_id = Some(message.id);
                request.send().await?;
            };
        }
        Command::Source => {
            bot.send_message(message.chat.id, "https://gitlab.com/71e6fd52/shit_bot")
                .await?;
        }
    };

    Ok(())
}

async fn forward_shit(bot: AutoSend<Bot>, message: Message) -> Result<(), RequestError> {
    let sent = bot
        .forward_message(SHIT_HILL, message.chat.id, message.id)
        .await?;
    let mut request = bot.inner().send_message(
        message.chat.id,
        format!("https://t.me/nipple_hill/{}", sent.id),
    );
    request.reply_to_message_id = Some(message.id);
    request.disable_web_page_preview = Some(true);
    request.send().await?;
    Ok(())
}
