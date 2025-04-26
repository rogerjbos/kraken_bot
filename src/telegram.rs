use std::{error::Error, fmt, sync::Arc};
use teloxide::{prelude::*, utils::command::BotCommands, types::ChatId};
use teloxide::types::ParseMode;
use tokio::{sync::Mutex, time::Duration};

use crate::kraken_pos::INTERVAL_SECONDS;
use crate::kraken_pos::TradingBot;

// Custom error type that implements Send + Sync
#[derive(Debug)]
pub struct BotError(pub String);

impl fmt::Display for BotError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Error for BotError {}
// Safely implement Send and Sync for BotError since it only contains a String which is Send + Sync
unsafe impl std::marker::Send for BotError {}
unsafe impl std::marker::Sync for BotError {}

#[derive(Clone)]
pub struct BotState {
    pub is_running: bool,
    pub notification_level: NotificationLevel,
}

// Create an enum for notification levels
#[derive(Clone, PartialEq, Debug)]
pub enum NotificationLevel {
    All,      // Send all messages
    Important, // Only important updates and errors
    Critical,  // Only critical errors and trade executions
    None      // No messages
}

impl BotState {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for BotState {
    fn default() -> Self {
        Self {
            is_running: false,
            notification_level: NotificationLevel::Important,
        }
    }
}

#[derive(Debug, BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "These commands are supported:")]
pub enum Command {
    #[command(description = "display this text.")]
    Help,
    #[command(description = "handle a username.")]
    Username(String),
    #[command(description = "start the Kraken bot.")]
    StartBot,
    #[command(description = "stop the Kraken bot.")]
    StopBot,
    #[command(description = "check bot status.")]
    Status,
    #[command(description = "set notification level (all/important/critical/none)")]
    Notify(String),
    #[command(description = "request immediate status update")]
    Update,
}

pub async fn answer(bot: Bot, msg: Message, cmd: Command, bot_state: Arc<Mutex<BotState>>) -> ResponseResult<()> {
    // println!("Command received: {:?}", cmd); // Debug print to see if command is received

    match cmd {
        Command::Help => {
            bot.send_message(msg.chat.id, Command::descriptions().to_string()).await?;
        }
        Command::Username(username) => {
            bot.send_message(msg.chat.id, format!("Your username is @{username}.")).await?;
        }
        Command::StartBot => {
            let is_already_running = {
                let state = bot_state.lock().await;
                state.is_running
            };
            
            if is_already_running {
                bot.send_message(msg.chat.id, "Kraken bot is already running.").await?;
            } else {
                // Set is_running to true first
                {
                    let mut state = bot_state.lock().await;
                    state.is_running = true;
                }
                
                bot.send_message(msg.chat.id, "Starting the Kraken bot...").await?;
                
                // We'll use the older thread spawn approach since it works
                let bot_state_clone = Arc::clone(&bot_state);
                let chat_id = msg.chat.id;
                let bot_clone = bot.clone();
                
                // Initialize bot outside of the spawn to handle non-Send errors
                match init_and_run_bot(bot_state_clone, bot_clone, chat_id).await {
                    Ok(()) => println!(" "), //Bot initialization started successfully
                    Err(e) => {
                        let error_msg = format!("Failed to start bot: {}", e);
                        eprintln!("{}", &error_msg);
                        bot.send_message(chat_id, error_msg).await?;
                        let mut state = bot_state.lock().await;
                        state.is_running = false;
                    }
                }
            }
        }
        Command::StopBot => {
            let was_running = {
                let mut state = bot_state.lock().await;
                let was_running = state.is_running;
                state.is_running = false;
                was_running
            };
            
            if was_running {
                bot.send_message(msg.chat.id, "Stopping the Kraken bot...").await?;
                println!("Stop command received, bot will stop on next check");
            } else {
                bot.send_message(msg.chat.id, "Kraken bot is not running.").await?;
            }
        }
        Command::Status => {
            let is_running = {
                let state = bot_state.lock().await;
                state.is_running
            };
            
            let status = if is_running {
                "Kraken bot is currently running."
            } else {
                "Kraken bot is currently stopped."
            };
            
            bot.send_message(msg.chat.id, status).await?;
        }
        Command::Notify(level) => {
            let new_level = match level.to_lowercase().as_str() {
                "all" => NotificationLevel::All,
                "important" => NotificationLevel::Important,
                "critical" => NotificationLevel::Critical,
                "none" => NotificationLevel::None,
                _ => {
                    bot.send_message(
                        msg.chat.id, 
                        "Invalid level. Use: all, important, critical, or none"
                    ).await?;
                    return Ok(());
                }
            };
            
            {
                let mut state = bot_state.lock().await;
                state.notification_level = new_level.clone();
            }
            
            bot.send_message(
                msg.chat.id,
                format!("Notification level set to {:?}", new_level)
            ).await?;
        }
        
        Command::Update => {
            // Request a status update from the trading bot
            // You'd need to implement this based on your bot structure
            bot.send_message(msg.chat.id, "Requesting status update...").await?;
            
            // You could either directly call the status function or use a channel
            // to request updates from the running bot thread
        }

    }

    Ok(())
}

// Create a helper function for sending messages
pub async fn send_telegram_notification(
    bot: &Bot, 
    chat_id: ChatId, 
    level: NotificationLevel,
    current_level: NotificationLevel,
    message: String
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // Only send if the message level is important enough
    if level_is_sufficient(level, current_level) {
        let monomessage = format!("<pre>{}</pre>", message);
        match bot.send_message(chat_id, monomessage).parse_mode(ParseMode::Html).await {
            Ok(_) => Ok(()),
            Err(e) => {
                eprintln!("Failed to send Telegram message: {}", e);
                Err(Box::new(BotError(format!("Telegram error: {}", e))))
            }
        }
    } else {
        Ok(())
    }
}

// Helper to check if notification level is sufficient
fn level_is_sufficient(msg_level: NotificationLevel, current_level: NotificationLevel) -> bool {
    match current_level {
        NotificationLevel::None => false,
        NotificationLevel::Critical => msg_level == NotificationLevel::Critical,
        NotificationLevel::Important => {
            matches!(msg_level, NotificationLevel::Critical | NotificationLevel::Important)
        },
        NotificationLevel::All => true,
    }
}

// Function to initialize and launch bot in a new thread
pub async fn init_and_run_bot(
    bot_state: Arc<Mutex<BotState>>, 
    bot: Bot, 
    chat_id: ChatId
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // println!("Starting bot initialization...");
    
    // Spawn the bot in a new thread to avoid Send issues
    std::thread::spawn(move || {
        // println!("Bot thread started");
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                // Try to initialize the bot
                let init_result = TradingBot::new().await;
                
                match init_result {
                    Ok(mut kraken_bot) => {
                        // Send confirmation message
                        if let Err(e) = bot.send_message(chat_id, "Kraken trading bot has initialized and is now running.").await {
                            eprintln!("Error sending message: {}", e);
                        }
                        
                        let mut check_interval = tokio::time::interval(Duration::from_secs(INTERVAL_SECONDS)); // Every 15 seconds

                        // First ticks are consumed
                        check_interval.tick().await;
                        // status_interval.tick().await;

                        let _last_status_check = std::time::Instant::now();
                        loop {
                            
                            // Check if we should stop
                            let should_run = {
                                let state = bot_state.lock().await;
                                // println!("Lock acquired, is_running: {}", state.is_running);
                                state.is_running
                            };
                            
                            if !should_run {
                                println!("Stop flag detected, shutting down bot");
                                if let Err(e) = bot.send_message(chat_id, "Kraken trading bot has been stopped.").await {
                                    eprintln!("Error sending stop message: {}", e);
                                }
                                break;
                            }
                            
                            // Always wait for the check interval
                            // println!("Waiting for check interval...");
                            check_interval.tick().await;
                            // println!("Check interval triggered");
                            
                            // Execute strategy with timeout protection
                            // println!("Executing trading strategy...");
                            match tokio::time::timeout(
                                Duration::from_secs(60), // Timeout after 60 seconds
                                kraken_bot.execute_strategy(bot_state.clone(), bot.clone(), chat_id)
                            ).await {
                                Ok(Ok(_)) => println!(" "), //Strategy execution completed successfully
                                Ok(Err(e)) => {
                                    let error_msg = format!("Strategy execution failed: {}", e);
                                    eprintln!("{}", &error_msg);
                                    
                                    if let Err(e) = bot.send_message(chat_id, error_msg).await {
                                        eprintln!("Error sending error message: {}", e);
                                    }
                                },
                                Err(_) => {
                                    println!("Strategy execution timed out");
                                    // You might want to send a message or handle the timeout differently
                                }
                            }
                        }


                    }
                    Err(e) => {
                        // Safely handle error
                        let error_msg = format!("Failed to initialize bot: {}", e);
                        eprintln!("{}", &error_msg);
                        
                        if let Err(e) = bot.send_message(chat_id, &error_msg).await {
                            eprintln!("Error sending initialization error message: {}", e);
                        }
                        
                        // Reset the running state
                        let mut state = bot_state.lock().await;
                        state.is_running = false;
                    }
                }
            });
    });
    // Don't wait for thread completion - just return success
    Ok(())
}