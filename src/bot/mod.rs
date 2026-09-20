//! Telegram layer: dispatcher schema lives in main.rs (type inference),
//! command list registration here.

pub mod callbacks;
pub mod handlers;
pub mod keyboards;
pub mod send;

use crate::i18n::I18n;
use teloxide::prelude::*;
use teloxide::types::BotCommand;

/// Register the BotFather-style command menu from the English locale strings.
pub async fn register_commands(bot: &Bot, i18n: &I18n) -> Result<(), teloxide::RequestError> {
    let cmds = vec![
        BotCommand { command: "start".to_string(), description: i18n.get("en", "cmd_start") },
        BotCommand { command: "language".to_string(), description: i18n.get("en", "cmd_language") },
        BotCommand { command: "help".to_string(), description: i18n.get("en", "cmd_help") },
        BotCommand { command: "queue".to_string(), description: i18n.get("en", "cmd_queue") },
        BotCommand { command: "cancel".to_string(), description: i18n.get("en", "cmd_cancel") },
        BotCommand { command: "status".to_string(), description: i18n.get("en", "cmd_status") },
        BotCommand { command: "settings".to_string(), description: i18n.get("en", "cmd_settings") },
    ];
    bot.set_my_commands(cmds).await?;
    Ok(())
}
