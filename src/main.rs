use std::{error::Error, sync::Arc};

use chrono::Utc;
use tokio::time;
use log;

// mod kraken_execute_simple_strategy;
mod kraken_execute_strategy;
mod kraken_execute_trade;
mod kraken_pos;

use telegram_bot::{BotState, Command, TelegramBotHandler};
use teloxide::{prelude::*, types::ChatId, update_listeners, utils::command::BotCommands};
use tokio::sync::Mutex;

use crate::kraken_pos::TradingBot;

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
    let telegram_handler = TelegramBotHandler::<TradingBot>::new(
        "/Users/rogerbos/rust_home/kraken_bot/symbols_config.json".to_string(),
    );

    // Spawn a task to restart the bot every 24 hours at midnight UTC
    let bot_state_clone = Arc::clone(&kraken_bot_state);
    let tg_bot_clone = tg_bot.clone();
    tokio::spawn(async move {
        loop {
            // Calculate the duration until the next midnight UTC
            let now = Utc::now();
            let next_midnight = now
                .date_naive()
                .succ_opt() // Get the next day
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .expect("valid time"); // Set time to midnight

            // Convert `now` to NaiveDateTime
            let now_naive = now.naive_utc();

            // Calculate the duration until midnight
            let duration_until_midnight = (next_midnight - now_naive).to_std().unwrap();

            // Wait until midnight UTC
            time::sleep(duration_until_midnight).await;

            // Stop the bot
            {
                let mut state = bot_state_clone.lock().await;
                state.is_running = false;
            }
            tg_bot_clone
                .send_message(chat_id, "Stopping the Kraken bot for restart...")
                .await
                .ok();
            log::info!("24-hour restart timer triggered");

            // Restart the bot
            {
                let mut state = bot_state_clone.lock().await;
                state.is_running = true;
            }
            tg_bot_clone
                .send_message(chat_id, "Restarting the Kraken bot...")
                .await
                .ok();
            log::info!("Bot restart completed");
        }
    });

    // Start the Kraken bot automatically
    let bot_state_clone = Arc::clone(&kraken_bot_state);
    let tg_bot_clone = tg_bot.clone();
    TelegramBotHandler::<TradingBot>::init_and_run_bot(bot_state_clone, tg_bot_clone, chat_id, 300)
        .await?;

    let cloned_tg_bot = tg_bot.clone();
    let cloned_handler = Arc::new(Mutex::new(telegram_handler));

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
