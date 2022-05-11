use teloxide::{prelude::*, types::ChatId, utils::command::BotCommands, RequestError};

use std::error::Error;

const SHIT_HILL: ChatId = ChatId(0 /*CLEANED*/);
const SOURCE: ChatId = ChatId(0 /*CLEANED*/);

#[tokio::main]
async fn main() {
    pretty_env_logger::init();
    log::info!("Starting command bot...");

    let bot = Bot::from_env().auto_send();

    teloxide::commands_repl(bot, answer, Command::ty()).await;
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

async fn answer(
    bot: AutoSend<Bot>,
    message: Message,
    command: Command,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if message.chat.id != SOURCE {
        return Ok(());
    };

    match command {
        Command::Help => {
            bot.send_message(message.chat.id, Command::descriptions().to_string())
                .await?
        }
        Command::Shit => {
            if message.from().is_none() {
                return Ok(());
            }
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
            } else {
                chat_member?;
            }

            if let Some(reply) = message.reply_to_message() {
                let sent = bot
                    .forward_message(SHIT_HILL, message.chat.id, reply.id)
                    .await?;
                let mut request = bot.inner().send_message(
                    message.chat.id,
                    format!("https://t.me/nipple_hill/{}", sent.id),
                );
                request.reply_to_message_id = Some(message.id);
                request.send().await?
            } else {
                let mut request = bot.inner().send_message(message.chat.id, "没有选择消息");
                request.reply_to_message_id = Some(message.id);
                request.send().await?
            }
        }
        Command::Source => {
            bot.send_message(message.chat.id, "https://gitlab.com/71e6fd52/shit_bot")
                .await?
        }
    };

    Ok(())
}
