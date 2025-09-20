use std::{collections::HashMap, sync::Arc};

use chrono::Local;
use kraken_async_rs::{
    response_types::BuySell,
    wss::{Message, TickerSubscription, WssMessage},
};
use telegram_bot::{send_telegram_notification, BotState, NotificationLevel};
use teloxide::{prelude::*, types::ChatId};
use tokio::sync::Mutex;
use tokio_stream::StreamExt;

use crate::kraken_pos::{PriceHistory, TradingBot};

impl TradingBot {
    /// Generates a formatted report of trading signals for all configured symbols.
    ///
    /// Gathers current positions, price data, and calculated returns to build
    /// a human-readable report suitable for sending via Telegram.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - `HashMap<String, (f64, String)>` mapping each symbol to its return percentage and signal action ("BUY", "SELL", or "HOLD").
    /// - A `String` containing the formatted report text (with timestamp and portfolio summary).
    pub async fn generate_signals_report(&mut self) -> (HashMap<String, (f64, String)>, String) {
        let mut signals = HashMap::new();
        let mut total_value: f64 = 0.0;

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        let mut report = String::from(format!("📊 Kraken Bot - {timestamp}\n\n"));

        // Clone symbols_config to avoid immutable borrowing conflicts
        let symbols_config_clone = self.symbols_config.clone();

        for symbol_config in symbols_config_clone.values() {
            let symbol = &symbol_config.symbol;
            let position = self.get_position(symbol).await;
            let signal = self.generate_signal(symbol).await;
            let return_pct = self.calculate_return(symbol).unwrap_or(0.0);
            let price = self.get_real_time_price(symbol).await;
            let m = self.get_mult(symbol).await;
            let value = position * price;
            total_value += value;

            // Add emojis based on signal and return percentage
            let signal_emoji = match signal.as_str() {
                "BUY" => "<b>B</b>",  // 🟢
                "SELL" => "<b>S</b>", // 🔴
                _ => "H",
            };

            // Manually pad the fields
            let base_currency = symbol.split('/').next().unwrap();
            let id = format!("{:<4}", base_currency.chars().take(4).collect::<String>());
            let signal_emoji_str = format!("{:<1}", signal_emoji);
            let return_pct_str = format!("{:>4.1}%", return_pct);
            let price_str = format!("{:>9.2}", price);
            let position_str = format!("{:>6.0}", position);
            let value_str = format!("{:>7.0}$", value);
            let m_str = format!("{:>4.1}x", m);

            let line = format!(
                "{} {} {} {} {} {} {}\n",
                id, signal_emoji_str, return_pct_str, price_str, position_str, value_str, m_str
            );

            println!("{}", line);
            report.push_str(&line);
            signals.insert(symbol.to_string(), (return_pct, signal));
        }

        // Add portfolio summary
        let cash = self.get_position("USD").await;
        let portfolio_total = total_value + cash;
        report.push_str(&format!(
            "\n💰 ${cash:.2}  💼 ${total_value:.2}  🏦 ${portfolio_total:.2}\n"
        ));
        (signals, report)
    }

    /// Executes the trading strategy end-to-end.
    ///
    /// - Subscribes to Kraken WebSocket streams for market tickers and account balances.
    /// - Updates internal positions and price history.
    /// - Fetches OHLC data and calculates trading signals.
    /// - Sends signals report via Telegram and executes trades based on signals.
    ///
    /// # Arguments
    ///
    /// * `_bot_state` - Shared state for controlling the bot lifecycle (start/stop).
    /// * `bot` - Teloxide `Bot` instance for sending Telegram messages.
    /// * `chat_id` - Telegram `ChatId` where notifications and reports are sent.
    ///
    /// # Returns
    ///
    /// * `Ok(())` on successful strategy execution or no actionable signals.
    /// * `Err(Box<dyn std::error::Error>)` if any underlying API or I/O error occurs.
    pub async fn execute_strategy(
        &mut self,
        _bot_state: Arc<Mutex<BotState>>,
        bot: Bot,
        chat_id: ChatId,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut private_stream = self.client.connect_auth::<WssMessage>().await?;
        let mut public_stream = self.client.connect::<WssMessage>().await?;

        // Subscribe to real-time ticker data
        let ticker_params = TickerSubscription::new(self.symbols_config.keys().cloned().collect());

        let subscription = Message::new_subscription(ticker_params, 0);
        public_stream.send(&subscription).await?;

        // Spawn price updater
        let prices_clone = self.real_time_prices.clone();
        let symbols_config_clone = self.symbols_config.clone();

        tokio::spawn(async move {
            while let Some(Ok(message)) = public_stream.next().await {
                // Debug the message to understand its structure
                let message_str = format!("{:?}", message);

                // Check if this is a ticker message
                if message_str.contains("Ticker") {
                    // Extract ticker data for each symbol
                    for symbol_config in symbols_config_clone.values() {
                        let symbol = &symbol_config.symbol;
                        if let Some(price) = extract_price_from_ticker_message(&message_str, symbol)
                        {
                            let mut prices = prices_clone.lock().await;
                            prices.insert(symbol.to_string(), price);
                        }
                    }
                }
            }
        });

        // Initial position update
        self.update_positions(&mut private_stream).await;

        // Fetch crypto data
        let symbols_vec: Vec<String> = self.symbols_config.keys().cloned().collect();
        match Self::fetch_crypto_data(symbols_vec, 60, 30).await {
            Ok(ohlc_data) => {
                // Update price history
                for data in &ohlc_data {
                    let history = self
                        .price_history
                        .entry(data.symbol.clone())
                        .or_insert_with(PriceHistory::new);

                    if history.closing_prices.len() >= 6 {
                        history.closing_prices.pop_front();
                    }
                    history.closing_prices.push_back(data.close);
                }

                // Generate signals with position info
                let (signals, report) = self.generate_signals_report().await;
                send_telegram_notification(
                    &bot,
                    chat_id,
                    NotificationLevel::Important,
                    NotificationLevel::Important,
                    format!("```\n{}\n```", report),
                )
                .await
                .unwrap_or_else(|e| eprintln!("Failed to send status: {}", e));

                // Execute trades
                for (symbol, (_return_pct, signal_str)) in &signals {
                    if let Some(_data) = ohlc_data.iter().find(|d| &d.symbol == symbol) {
                        let signal = match signal_str.as_str() {
                            "BUY" => BuySell::Buy,
                            "SELL" => BuySell::Sell,
                            _ => continue,
                        };

                        // Find the Symbol struct from symbols_config
                        if let Some(symbol_config) = self.symbols_config.get(symbol) {
                            self.execute_trade(symbol_config.clone(), signal, &mut private_stream)
                                .await?;
                        } else {
                            eprintln!("Symbol config not found for symbol: {}", symbol);
                        }
                    }
                }
            }
            Err(e) => eprintln!("Error fetching crypto data: {}", e),
        }

        // Since we've done the work once, just return success
        Ok(())
    }
}

// Helper to extract price from ticker messages
/// Parses a raw ticker WebSocket message to find the "last" price for a specific symbol.
/// Returns `None` if the message does not correspond to the symbol or parsing fails.
fn extract_price_from_ticker_message(message: &str, symbol: &str) -> Option<f64> {
    // First check if this message is for the symbol we're interested in
    if !message.contains(symbol) {
        return None;
    }

    // Extract the "last" price field directly
    if let Some(last_idx) = message.find("last:") {
        let start_idx = last_idx + "last:".len();
        // Find the next comma or closing delimiter after "last:"
        if let Some(end_idx) = message[start_idx..].find(|c| c == ',' || c == '}') {
            let price_str = &message[start_idx..start_idx + end_idx].trim();
            return price_str.parse::<f64>().ok();
        }
    }
    None
}
