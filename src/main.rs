use std::{error::Error, sync::Arc};

// mod kraken_execute_simple_strategy;
mod kraken_execute_strategy;
mod kraken_execute_trade;
mod kraken_pos;

use telegram_bot::{BotState, Command, TelegramBotHandler};
use teloxide::{prelude::*, types::ChatId, update_listeners, utils::command::BotCommands};
use tokio::sync::Mutex;

use crate::kraken_pos::TradingBot;

pub const INTERVAL_SECONDS: u64 = 300; // How frequently to run the strategy

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let ibkr_bot_token = std::env::var("KRAKEN_TELOXIDE_TOKEN")
        .expect("KRAKEN_TELOXIDE_TOKEN must be set as environment variable");
    let tg_bot = Bot::new(ibkr_bot_token);

    // Read chat ID from environment variable at runtime
    let kraken_bot_chat_id: i64 = std::env::var("KRAKEN_BOT_CHAT_ID")
        .expect("KRAKEN_BOT_CHAT_ID must be set in .env file")
        .parse()
        .expect("KRAKEN_BOT_CHAT_ID must be a valid number");

    let chat_id = ChatId(kraken_bot_chat_id);
    tg_bot
        .send_message(chat_id, "Kraken bot application started")
        .await?;

    let kraken_bot_state = Arc::new(Mutex::new(BotState {
        is_running: true,
        ..Default::default()
    }));

    // Create telegram bot handler
    let (telegram_handler, _request_rx) = TelegramBotHandler::new();
    let telegram_handler = Arc::new(Mutex::new(telegram_handler));

    // Spawn the trading bot task
    let bot_state_clone = Arc::clone(&kraken_bot_state);
    let tg_bot_clone = tg_bot.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(INTERVAL_SECONDS));
        loop {
            interval.tick().await;
            
            let is_running = {
                let state = bot_state_clone.lock().await;
                state.is_running
            };
            
            if is_running {
                // Create and run the trading bot
                match TradingBot::new(INTERVAL_SECONDS).await {
                    Ok(mut bot) => {
                        if let Err(e) = bot.execute_strategy(
                            Arc::clone(&bot_state_clone),
                            tg_bot_clone.clone(),
                            chat_id,
                        ).await {
                            eprintln!("Error executing strategy: {}", e);
                        }
                    }
                    Err(e) => {
                        eprintln!("Error creating trading bot: {}", e);
                    }
                }
            }
        }
    });

    // Set up Telegram command handling
    let cloned_tg_bot = tg_bot.clone();
    let cloned_handler = Arc::clone(&telegram_handler);

    teloxide::repl_with_listener(
        tg_bot.clone(),
        move |msg: Message| {
            let kraken_bot_state = Arc::clone(&kraken_bot_state);
            let tg_bot = cloned_tg_bot.clone();
            let handler = Arc::clone(&cloned_handler);
            async move {
                if let Some(cmd) = Command::parse(
                    &msg.text().unwrap_or_default(),
                    tg_bot
                        .get_me()
                        .await?
                        .user
                        .username
                        .as_deref()
                        .unwrap_or(""),
                )
                .ok()
                {
                    let mut handler_guard = handler.lock().await;
                    match handler_guard
                        .handle_command(tg_bot, msg, cmd, kraken_bot_state)
                        .await
                    {
                        Ok(_) => Ok(()),
                        Err(e) => {
                            eprintln!("Error in command handler: {}", e);
                            Ok(())
                        }
                    }
                } else {
                    Ok(())
                }
            }
        },
        update_listeners::polling_default(tg_bot).await,
    )
    .await;

    Ok(())
}
