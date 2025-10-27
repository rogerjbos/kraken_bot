use std::{error::Error, time::Duration};

use kraken_async_rs::{
    request_types::{SelfTradePrevention, TimeInForceV2},
    response_types::{BuySell, OrderType},
    wss::{AddOrderParams, FeePreference, Message},
};
use rust_decimal::{
    prelude::{FromPrimitive, ToPrimitive},
    Decimal,
};
use tokio::time::{sleep, timeout};
use tokio_stream::StreamExt;

use crate::kraken_pos::{Symbol, TradingBot};

impl TradingBot {
    /// Executes a single trade based on a trading signal (Buy/Sell).
    ///
    /// Determines order size and limit price according to symbol configuration,
    /// submits the order via Kraken WebSocket API, and waits for confirmation.
    ///
    /// # Arguments
    ///
    /// * `symbol` - `Symbol` struct containing trading parameters (entry/exit
    ///   amounts).
    /// * `signal` - `BuySell` enum indicating buy or sell.
    /// * `private_stream` - Mutable reference to authenticated Kraken WebSocket
    ///   stream for sending orders and receiving responses.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the trade order is successfully accepted or no action
    ///   required.
    /// * `Err(Box<dyn Error>)` if messaging or parsing fails.
    pub async fn execute_trade(
        &self,
        symbol: Symbol,
        signal: BuySell,
        private_stream: &mut kraken_async_rs::wss::KrakenMessageStream<serde_json::Value>,
    ) -> Result<(), Box<dyn Error>> {
        let position = self.get_position(symbol.symbol).await;
        let cash_balance = self.get_position("USD").await;
        let latest_price = self.get_real_time_price(symbol.symbol).await;

        let (order_size, price) = match signal {
            BuySell::Buy if cash_balance > symbol.entry_amount && latest_price > 0.0 => {
                let mut entry_size = Decimal::from_f64(symbol.entry_amount / latest_price)
                    .unwrap()
                    .round_dp(2)
                    .to_f64()
                    .unwrap();
                
                // Adjust order size based on return percentage for BUY orders
                if let Some(return_pct) = self.calculate_return(symbol.symbol) {
                    if return_pct > -0.20 {
                        entry_size *= 4.0;
                    } else if return_pct > -0.15 {
                        entry_size *= 3.0;
                    } else if return_pct > -0.10 {
                        entry_size *= 2.0;
                    }
                }
                
                let limit_price = Decimal::from_f64(latest_price * 1.005)
                    .unwrap()
                    .round_dp(2)
                    .to_f64()
                    .unwrap();
                (entry_size, limit_price)
            }
            BuySell::Sell if position > 0.0 && latest_price > 0.0 => {
                let exit_size = Decimal::from_f64(symbol.exit_amount / latest_price)
                    .unwrap()
                    .round_dp(2)
                    .to_f64()
                    .unwrap();
                let limit_price = Decimal::from_f64(latest_price * 1.0)
                    .unwrap()
                    .round_dp(2)
                    .to_f64()
                    .unwrap();
                (position.min(exit_size), limit_price)
            }
            _ => return Ok(()), // No position to sell or funds to buy
        };

        if order_size <= 0.0 {
            return Ok(());
        }

        println!(
            "\n\n=====> Executing {} {} at {:.2} (Size: {:.4})",
            signal, symbol.symbol, price, order_size
        );

        let new_order = AddOrderParams {
            order_type: OrderType::Limit,
            side: signal,
            symbol: symbol.symbol.to_string(),
            limit_price: Some(Decimal::from_f64(price).unwrap()),
            limit_price_type: None,
            triggers: None,
            time_in_force: Some(TimeInForceV2::GTC),
            order_quantity: Decimal::from_f64(order_size).unwrap(),
            margin: None,
            post_only: Some(true),
            reduce_only: None,
            effective_time: None,
            expire_time: None,
            deadline: None,
            order_user_ref: None,
            conditional: None,
            display_quantity: None,
            fee_preference: Some(FeePreference::Quote),
            no_market_price_protection: None,
            stp_type: Some(SelfTradePrevention::CancelNewest),
            cash_order_quantity: None,
            validate: None,
            sender_sub_id: None,
            token: self.token.clone(),
            client_order_id: None,
        };

        // Use a unique request ID for each order
        static REQUEST_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
        let req_id = REQUEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let order_message = Message {
            method: "add_order".to_string(),
            params: new_order,
            req_id: req_id as i64,
        };

        // Send the order
        private_stream.send(&order_message).await?;
        println!(
            "Order sent for {} req_id={}: {:?}",
            symbol.symbol, req_id, order_message
        );

        // Wait for the actual order response
        let max_attempts = 10;
        for _ in 0..max_attempts {
            match timeout(Duration::from_secs(2), private_stream.next()).await {
                Ok(Some(Ok(response))) => {
                    let response_str = response.to_string();

                    // Check if this is a heartbeat message
                    if response_str.contains("Heartbeat") || response_str.contains("heartbeat") {
                        continue;
                    }

                    // Check if this is the response to our order (should contain our req_id)
                    if response_str.contains(&format!("req_id\": {}", req_id))
                        || (response_str.contains("add_order")
                            && response_str.contains(symbol.symbol))
                    {
                        if response_str.contains("Order rejected") || response_str.contains("error") {
                            eprintln!("Order rejected: {}", response_str);
                            println!("Continuing despite order rejection for req_id={}", req_id);
                            return Ok(());
                        } else if response_str.contains("success") || response_str.contains("accepted") {
                            println!("Order accepted for {}", symbol.symbol);
                            // Add a small delay between orders to avoid rate limiting
                            sleep(Duration::from_secs(1)).await;

                            return Ok(());
                        }
                    } else {
                        println!("Received unrelated message, continuing to wait");
                    }
                }
                _ => {
                    println!("Timeout waiting for response, retrying");
                }
            }
        }

        eprintln!(
            "No order confirmation received for {} after {} attempts",
            symbol.symbol, max_attempts
        );
        println!(
            "Proceeding without confirmation for order req_id={}",
            req_id
        );
        Ok(())
    }
}
