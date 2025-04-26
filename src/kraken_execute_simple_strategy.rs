use crate::kraken_pos::{TradingBot, INTERVAL_SECONDS, Symbol};
use kraken_async_rs::response_types::{BuySell};
use kraken_async_rs::wss::{ChannelMessage, Message, TickerSubscription, WssMessage};
use kraken_async_rs::wss::Ticker as KrakenTicker;

use rust_decimal::prelude::FromPrimitive;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use std::{collections::HashMap, error::Error, time::Duration};
use serde::{Deserialize, Serialize};
use chrono::Local;

#[derive(Serialize, Deserialize, Debug)]
struct SerializableTicker {
    ask: String,
    ask_quantity: String,
    bid: String,
    bid_quantity: String,
    change: String,
    change_pct: String,
    high: String,
    last: String,
    low: String,
    symbol: String,
    volume: String,
    vwap: String,
}

impl From<&KrakenTicker> for SerializableTicker {
    fn from(ticker: &KrakenTicker) -> Self {
        SerializableTicker {
            ask: ticker.ask.to_string(),
            ask_quantity: ticker.ask_quantity.to_string(),
            bid: ticker.bid.to_string(),
            bid_quantity: ticker.bid_quantity.to_string(),
            change: ticker.change.to_string(),
            change_pct: ticker.change_pct.to_string(),
            high: ticker.high.to_string(),
            last: ticker.last.to_string(),
            low: ticker.low.to_string(),
            symbol: ticker.symbol.clone(),
            volume: ticker.volume.to_string(),
            vwap: ticker.vwap.to_string(),
        }
    }
}

impl TradingBot {
    
    pub async fn execute_simple_strategy(&mut self) -> Result<(), Box<dyn Error>> {
        let mut private_stream = self.client.connect_auth::<WssMessage>().await?;
        let mut price_stream = self.client.connect::<WssMessage>().await?;
        
        // Clone symbols_config to avoid immutable borrowing conflicts
        let symbols_config_clone = self.symbols_config.clone();
        
        // Subscribe to real-time ticker data
        let ticker_params = TickerSubscription::new(symbols_config_clone.keys().cloned().collect());
        let subscription = Message::new_subscription(ticker_params, 0);
        price_stream.send(&subscription).await?;

        // Initial position update
        self.update_positions(&mut private_stream).await;

        let prices_clone = self.real_time_prices.clone();

        while let Some(Ok(message)) = price_stream.next().await {
            match &message {
                WssMessage::Channel(channel_data) => {
                    // Check if this is a ticker message
                    if let ChannelMessage::Ticker(single_response) = channel_data {
                        let ticker = &single_response.data;
                        let serializable_ticker = SerializableTicker::from(ticker);
                        let symbol = serializable_ticker.symbol.clone();
                        let last_price = serializable_ticker.last.parse::<f64>().unwrap_or(0.0);
                        let change_pct = serializable_ticker.change_pct.parse::<f64>().unwrap_or(0.0);

                        // Update real-time prices
                        let mut prices = prices_clone.lock().await;
                        prices.insert(symbol.clone(), last_price);

                        // Retrieve the Symbol struct from symbols_config
                        if let Some(symbol_config) = self.symbols_config.get(&symbol) {
                            let entry_amount = symbol_config.entry_amount;
                            let exit_amount = symbol_config.exit_amount;
                            let entry_threshold = symbol_config.entry_threshold;
                            let exit_threshold = symbol_config.exit_threshold;
                            let m = self.get_mult(&symbol).await;
                            let entry = entry_threshold * m;
                            let exit = exit_threshold * m;
                            let now = Local::now().format("%Y-%m-%d %H:%M:%S");

                            let dp = if symbol == "BTC/USD" { 4 } else { 2 };
                            if change_pct < entry {
                                let entry_size = Decimal::from_f64(entry_amount / last_price).unwrap().round_dp(dp).to_f64().unwrap();
                                let limit_price = Decimal::from_f64(last_price * 0.9).unwrap().round_dp(dp).to_f64().unwrap();
                                self.increase_mult(&symbol).await;
                                println!("{now} {symbol} BUY {entry_size} @ {limit_price}, mult: {m} ({change_pct:.2} < {entry:.2})");
                            } else if change_pct > exit {
                                let exit_size = Decimal::from_f64(exit_amount / last_price).unwrap().round_dp(dp).to_f64().unwrap();
                                let limit_price = Decimal::from_f64(last_price * 1.1).unwrap().round_dp(dp).to_f64().unwrap();
                                self.increase_mult(&symbol).await;
                                println!("{now} {symbol} SELL {exit_size} @ {limit_price}, mult: {m} ({change_pct:.2} > {exit:.2})");
                            } else if (change_pct > -0.5) & (change_pct < 0.5) {
                                self.reset_mult(&symbol).await;
                                let test = self.get_mult(&symbol).await;
                                println!("{now} {symbol} FLAT, resetting mult to {test}");
                            } else {
                                let test = self.get_mult(&symbol).await;
                                println!("{now} {symbol} no action, keeping mult at {test}");
                            }
                        } else {
                            eprintln!("Symbol config not found for symbol: {}", symbol);
                        }
                    }
                }
                _ => {
                    // Handle other message types
                }
            }
        }
        Ok(())
    }
}