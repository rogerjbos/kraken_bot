use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use chrono::{Utc, Timelike};
use tokio::time;

mod kraken_pos;
mod kraken_execute_strategy;
mod kraken_execute_simple_strategy;
mod kraken_execute_trade;
mod telegram;

use crate::telegram::{BotState, Command, init_and_run_bot, answer}; // Import necessary items from telegram module

use tokio::sync::Mutex;
use teloxide::prelude::*;
use teloxide::utils::command::BotCommands;
use teloxide::types::ChatId;
use teloxide::update_listeners;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let tg_bot = Bot::from_env();
    let chat_id = ChatId(1430543101);
    tg_bot.send_message(chat_id, "Kraken bot application started").await?;

    let kraken_bot_state = Arc::new(Mutex::new(BotState {
        is_running: true,
        ..Default::default()
    }));

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
                .and_hms(0, 0, 0); // Set time to midnight

            // Convert `now` to NaiveDateTime
            let now_naive = now.naive_utc();

            // Calculate the duration until midnight
            let duration_until_midnight = (next_midnight - now_naive)
                .to_std()
                .unwrap();

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

            // Restart the bot
            {
                let mut state = bot_state_clone.lock().await;
                state.is_running = true;
            }
            tg_bot_clone
                .send_message(chat_id, "Restarting the Kraken bot...")
                .await
                .ok();
        }
    });

    // Start the Kraken bot automatically
    let bot_state_clone = Arc::clone(&kraken_bot_state);
    let tg_bot_clone = tg_bot.clone();
    init_and_run_bot(bot_state_clone, tg_bot_clone, chat_id).await?;

    let cloned_tg_bot = tg_bot.clone();
    teloxide::repl_with_listener(
        tg_bot.clone(),
        move |msg: Message| {
            let kraken_bot_state = Arc::clone(&kraken_bot_state);
            let tg_bot = cloned_tg_bot.clone();
            async move {
                if let Some(cmd) = Command::parse(&msg.text().unwrap_or_default(), tg_bot.get_me().await?.user.username.as_deref().unwrap_or("")).ok() {
                    match answer(tg_bot, msg, cmd, kraken_bot_state).await {
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