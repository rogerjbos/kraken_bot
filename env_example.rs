// Example of how to read environment variables in your Rust code

use std::env;

fn main() {
    // Load .env file (add this to your main function)
    dotenvy::dotenv().ok();

    // Method 1: Using env::var() with error handling
    let telegram_token = env::var("TELOXIDE_TOKEN")
        .expect("TELOXIDE_TOKEN must be set in .env file");

    // Method 2: Using env::var() with default value
    let kraken_api_key = env::var("KRAKEN_API_KEY")
        .unwrap_or_else(|_| "default_key".to_string());

    // Method 3: Using env::var() with Result handling
    match env::var("KRAKEN_BOT_CHAT_ID") {
        Ok(chat_id) => {
            let chat_id: i64 = chat_id.parse().expect("Invalid chat ID");
            println!("Chat ID: {}", chat_id);
        }
        Err(e) => {
            eprintln!("Failed to read KRAKEN_BOT_CHAT_ID: {}", e);
        }
    }

    // Method 4: Reading numeric values
    let chat_id: i64 = env::var("KRAKEN_BOT_CHAT_ID")
        .expect("KRAKEN_BOT_CHAT_ID must be set")
        .parse()
        .expect("KRAKEN_BOT_CHAT_ID must be a valid number");

    // Method 5: Using teloxide's Bot::from_env() (reads TELOXIDE_TOKEN automatically)
    let bot = teloxide::Bot::from_env();
}

// For constants that need to be read at compile time, you can use:
// const CHAT_ID: i64 = env!("KRAKEN_BOT_CHAT_ID").parse().unwrap(); // This won't work
// Instead, read it at runtime in main() or lazy_static