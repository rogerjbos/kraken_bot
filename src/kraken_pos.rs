use std::{
    collections::{HashMap, VecDeque},
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use kraken_async_rs::{
    clients::{core_kraken_client::CoreKrakenClient, kraken_client::KrakenClient},
    crypto::{
        nonce_provider::{IncreasingNonceProvider, NonceProvider},
        secrets::Token,
    },
    secrets::secrets_provider::{EnvSecretsProvider, SecretsProvider},
    wss::{
        BalancesSubscription, KrakenWSSClient, Message, OhlcSubscription, WssMessage, WS_KRAKEN,
        WS_KRAKEN_AUTH,
    },
};
use serde::{Deserialize, Serialize};
use telegram_bot::{BotError, BotState, TradingBot as TelegramTradingBot};
use teloxide::{types::ChatId, Bot};
use tokio::{sync::Mutex, time::timeout};
use tokio_stream::StreamExt;
use tracing::info;

pub const KRAKEN_KEY: &str = "KRAKEN_KEY_SOLO";
pub const KRAKEN_SECRET: &str = "KRAKEN_SECRET_SOLO";
pub const INTERVAL_SECONDS: u64 = 300; // How often to run the strategy

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub symbol: &'static str,
    pub entry_amount: f64,
    pub exit_amount: f64,
    pub entry_threshold: f64,
    pub exit_threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolConfig {
    pub symbol: String,
    pub entry_amount: f64,
    pub exit_amount: f64,
    pub entry_threshold: f64,
    pub exit_threshold: f64,
}

/// Represents OHLC (Open, High, Low, Close) data for a trading symbol.
///
/// Contains comprehensive market data for a specific time interval,
/// including price information, volume, and trading statistics.
///
/// # Fields
///
/// * `symbol` - Trading pair symbol (e.g., "BTC/USD", "ETH/EUR")
/// * `open` - Opening price for the interval
/// * `high` - Highest price during the interval
/// * `low` - Lowest price during the interval
/// * `close` - Closing price for the interval
/// * `vwap` - Volume Weighted Average Price
/// * `trades` - Number of trades executed during the interval
/// * `volume` - Total volume traded during the interval
/// * `interval_begin` - Start time of the interval (string format)
/// * `interval` - Interval duration in seconds
#[derive(Debug, Clone)]
#[allow(dead_code)] // OHLC data structure - fields may be used in future features
pub struct OhlcData {
    pub symbol: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub vwap: f64,
    pub trades: i64,
    pub volume: f64,
    pub interval_begin: String,
    pub interval: i64,
}

/// Maintains a rolling history of closing prices for return calculations.
///
/// Stores up to 6 closing prices in a deque structure to enable
/// percentage return calculations over time periods.
///
/// # Fields
///
/// * `closing_prices` - Double-ended queue storing recent closing prices with a
///   capacity of 6 elements for efficient memory usage
///
/// # Usage
///
/// Used internally by the trading bot to track price movements and
/// calculate returns for trading signal generation.
#[derive(Debug)]
pub struct PriceHistory {
    pub closing_prices: VecDeque<f64>,
}

impl PriceHistory {
    /// Creates a new `PriceHistory` instance with a capacity for 6 price
    /// points.
    ///
    /// # Returns
    ///
    /// A new `PriceHistory` with an empty deque initialized with capacity 6.
    pub fn new() -> Self {
        PriceHistory {
            closing_prices: VecDeque::with_capacity(6),
        }
    }

    /// Calculates the percentage return between the oldest and newest price in
    /// the history.
    ///
    /// Requires at least 6 price points to calculate a meaningful return.
    ///
    /// # Returns
    ///
    /// * `Some(f64)` - The percentage return if sufficient data is available
    /// * `None` - If fewer than 6 price points are stored
    ///
    /// # Formula
    ///
    /// `((current_price - old_price) / old_price) * 100.0`
    fn calculate_return(&self) -> Option<f64> {
        if self.closing_prices.len() >= 6 {
            let old_price = self.closing_prices.front()?;
            let current_price = self.closing_prices.back()?;
            Some((current_price - old_price) / old_price * 100.0)
        } else {
            None
        }
    }
}

/// Main trading bot implementation with Kraken exchange integration.
///
/// Provides comprehensive trading functionality including position management,
/// real-time price tracking, signal generation, and WebSocket communication
/// with the Kraken cryptocurrency exchange.
///
/// # Fields
///
/// * `client` - WebSocket client for Kraken API communication
/// * `token` - Authentication token for private API endpoints
/// * `price_history` - Historical price data for return calculations
/// * `positions` - Current asset balances/positions (thread-safe)
/// * `real_time_prices` - Live market prices from WebSocket feeds (thread-safe)
/// * `mult` - Position size multipliers per symbol (thread-safe)
/// * `symbols_config` - Trading configuration for each symbol
///
/// # Thread Safety
///
/// The bot is designed for concurrent access with Arc<Mutex<_>> wrappers
/// around shared data structures, enabling safe usage in multi-threaded
/// environments like async runtimes.
///
/// # Usage
///
/// ```
/// let bot = TradingBot::new().await?;
/// let signal = bot.generate_signal("BTC/USD").await;
/// let position = bot.get_position("BTC/USD").await;
/// ```
pub struct TradingBot {
    pub client: KrakenWSSClient,
    pub token: Token,
    pub price_history: HashMap<String, PriceHistory>,
    pub positions: Arc<Mutex<HashMap<String, f64>>>,
    pub real_time_prices: Arc<Mutex<HashMap<String, f64>>>,
    pub mult: Arc<Mutex<HashMap<String, f64>>>,
    pub symbols_config: HashMap<String, Symbol>,
}

impl TradingBot {
    /// Creates a new instance of the TradingBot with Kraken API integration.
    ///
    /// Initializes the bot with:
    /// - WebSocket client for real-time data
    /// - Authentication using environment variables
    /// - Empty data structures for positions, prices, and price history
    /// - Symbols configuration loaded from file if available
    ///
    /// # Returns
    ///
    /// * `Ok(TradingBot)` - Successfully initialized trading bot
    /// * `Err(Box<dyn Error>)` - If initialization fails (missing credentials,
    ///   network issues, etc.)
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - Kraken API credentials are invalid or missing
    /// - WebSocket connection cannot be established
    /// - Symbol configuration file cannot be read
    pub async fn new() -> Result<Self, Box<dyn Error>> {
        let secrets_provider: Box<Arc<Mutex<dyn SecretsProvider>>> = Box::new(Arc::new(
            Mutex::new(EnvSecretsProvider::new(KRAKEN_KEY, KRAKEN_SECRET)),
        ));
        let nonce_provider: Box<Arc<Mutex<dyn NonceProvider>>> =
            Box::new(Arc::new(Mutex::new(IncreasingNonceProvider::new())));
        let mut kraken_client = CoreKrakenClient::new(secrets_provider, nonce_provider);
        let token_response = kraken_client.get_websockets_token().await?;
        let token = token_response.result.unwrap().token;
        let client = KrakenWSSClient::new();

        // Read and parse the symbols configuration from a file
        let symbols_config =
            Self::read_symbols_config("/Users/rogerbos/rust_home/kraken_bot/symbols_config.json")
                .await?;

        // Initialize mult HashMap with known symbols
        let mut mult_map = HashMap::new();
        let mut symbols_config_map = HashMap::new();
        for symbol_config in &symbols_config {
            mult_map.insert(symbol_config.symbol.clone(), 1.0);
            symbols_config_map.insert(
                symbol_config.symbol.clone(),
                Symbol {
                    symbol: Box::leak(symbol_config.symbol.clone().into_boxed_str()),
                    entry_amount: symbol_config.entry_amount,
                    exit_amount: symbol_config.exit_amount,
                    entry_threshold: symbol_config.entry_threshold,
                    exit_threshold: symbol_config.exit_threshold,
                },
            );
        }

        Ok(Self {
            client,
            token,
            price_history: HashMap::new(),
            positions: Arc::new(Mutex::new(HashMap::new())),
            real_time_prices: Arc::new(Mutex::new(HashMap::new())),
            mult: Arc::new(Mutex::new(mult_map)),
            symbols_config: symbols_config_map,
        })
    }

    /// Reads and parses the symbols configuration file.
    ///
    /// Loads trading symbol configurations from a JSON file containing
    /// symbol settings, multipliers, and trading parameters.
    ///
    /// # Arguments
    ///
    /// * `file_path` - Path to the JSON configuration file
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<SymbolConfig>)` - Successfully parsed symbol configurations
    /// * `Err(Box<dyn Error>)` - If file cannot be read or JSON is invalid
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The file does not exist or cannot be read
    /// - The JSON format is invalid
    /// - Required fields are missing from the configuration
    async fn read_symbols_config(file_path: &str) -> Result<Vec<SymbolConfig>, Box<dyn Error>> {
        let content = tokio::fs::read_to_string(file_path).await?;
        let symbols_config: Vec<SymbolConfig> = serde_json::from_str(&content)?;
        Ok(symbols_config)
    }

    /// Updates current positions by subscribing to Kraken's balance WebSocket
    /// stream.
    ///
    /// Establishes a WebSocket subscription to receive real-time balance
    /// updates from Kraken and processes the incoming messages to maintain
    /// current position data.
    ///
    /// # Arguments
    ///
    /// * `private_stream` - Mutable reference to the Kraken WebSocket message
    ///   stream
    ///
    /// # Behavior
    ///
    /// - Subscribes to balance updates using the bot's authentication token
    /// - Waits for actual balance data (not just subscription confirmation)
    /// - Updates the internal positions HashMap with new balance information
    /// - Times out after 10 attempts to prevent infinite waiting
    ///
    /// # Note
    ///
    /// This function may block while waiting for balance data from the
    /// WebSocket stream.
    pub async fn update_positions(
        &self,
        private_stream: &mut kraken_async_rs::wss::KrakenMessageStream<WssMessage>,
    ) {
        // Subscribe to balances
        let balances_params = BalancesSubscription::new(self.token.clone());
        let subscription = Message::new_subscription(balances_params, 0);

        let result = private_stream.send(&subscription).await;
        assert!(result.is_ok());

        // Wait for actual balance data (not just the subscription confirmation)
        let max_attempts = 10;
        let mut attempts = 0;

        while attempts < max_attempts {
            match timeout(Duration::from_secs(5), private_stream.next()).await {
                Ok(Some(Ok(message))) => {
                    let message_str = format!("{:?}", message);

                    // Check if this is a balance message (containing asset information)
                    if message_str.contains("asset:") || message_str.contains("Balance {") {
                        // Process USD balance
                        if let Some(usd_balance) = Self::extract_asset_balance(&message_str, &"USD")
                        {
                            let mut positions = self.positions.lock().await;
                            positions.insert("USD".to_string(), usd_balance);
                        }

                        // Process balances for trading symbols
                        for symbol_config in self.symbols_config.values() {
                            let symbol = &symbol_config.symbol;
                            let base_currency = symbol.split('/').next().unwrap();
                            if let Some(balance) = Self::extract_asset_balance(&message_str, symbol)
                            {
                                let mut positions = self.positions.lock().await;
                                positions.insert(base_currency.to_string(), balance);
                            }
                        }
                        break; // We got what we needed, so break out of the
                               // loop
                    }
                }
                Ok(Some(Err(e))) => {
                    eprintln!("Error receiving message: {:?}", e);
                    attempts += 1;
                }
                Ok(None) => {
                    eprintln!("Stream ended unexpectedly");
                    break;
                }
                Err(_) => {
                    eprintln!("Timeout waiting for message");
                    attempts += 1;
                }
            }
        }

        if attempts >= max_attempts {
            eprintln!(
                "Failed to receive balance data after {} attempts",
                max_attempts
            );
        }
    }

    /// Extracts the balance for a specific asset from a WebSocket message
    /// string.
    ///
    /// Parses the incoming WebSocket message to find the balance information
    /// for the base currency of the given trading symbol.
    ///
    /// # Arguments
    ///
    /// * `message_str` - Raw WebSocket message string from Kraken
    /// * `symbol` - Trading symbol (e.g., "BTC/USD") to extract balance for
    ///
    /// # Returns
    ///
    /// * `Some(f64)` - The balance amount if found and parsed successfully
    /// * `None` - If the asset is not found in the message or parsing fails
    ///
    /// # Example
    ///
    /// ```
    /// let balance = TradingBot::extract_asset_balance(message, "BTC/USD");
    /// // Returns the BTC balance from the message
    /// ```
    fn extract_asset_balance(message_str: &str, symbol: &str) -> Option<f64> {
        let base_currency = symbol.split('/').next().unwrap();
        let asset = format!("asset: \"{}\"", base_currency);
        if let Some(usd_idx) = message_str.find(&asset) {
            if let Some(balance_idx) = message_str[usd_idx..].find("balance: ") {
                let balance_start = usd_idx + balance_idx + "balance: ".len();
                if let Some(balance_end) =
                    message_str[balance_start..].find(|c| c == ',' || c == '}')
                {
                    let balance_str =
                        &message_str[balance_start..balance_start + balance_end].trim();
                    return balance_str.parse::<f64>().ok();
                }
            }
        }
        None
    }

    /// Retrieves the current position (balance) for a given trading symbol.
    ///
    /// Looks up the balance for the base currency of the trading pair.
    /// For example, for "BTC/USD", it returns the BTC balance.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading symbol (e.g., "BTC/USD", "ETH/EUR")
    ///
    /// # Returns
    ///
    /// The current balance of the base currency, or 0.0 if no position exists.
    ///
    /// # Example
    ///
    /// ```
    /// let btc_balance = bot.get_position("BTC/USD").await; // Returns BTC amount
    /// ```
    pub async fn get_position(&self, symbol: &str) -> f64 {
        let base_currency = symbol.split('/').next().unwrap();
        let positions = self.positions.lock().await;
        positions.get(base_currency).copied().unwrap_or(0.0)
    }

    /// Retrieves the current real-time price for a given trading symbol.
    ///
    /// Returns the most recent price data received from the WebSocket stream.
    /// If no price data is available, returns 0.0.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading symbol (e.g., "BTC/USD", "ETH/EUR")
    ///
    /// # Returns
    ///
    /// The current market price, or 0.0 if no price data is available.
    ///
    /// # Example
    ///
    /// ```
    /// let current_price = bot.get_real_time_price("BTC/USD").await;
    /// ```
    pub async fn get_real_time_price(&self, symbol: &str) -> f64 {
        let prices = self.real_time_prices.lock().await;
        prices.get(symbol).copied().unwrap_or(0.0)
    }

    /// Retrieves the current position multiplier for a given trading symbol.
    ///
    /// The multiplier is used to adjust position sizes in trading strategies.
    /// Returns 1.0 if no specific multiplier is set for the symbol.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading symbol (e.g., "BTC/USD", "ETH/EUR")
    ///
    /// # Returns
    ///
    /// The current multiplier value, or 1.0 as default.
    ///
    /// # Example
    ///
    /// ```
    /// let multiplier = bot.get_mult("BTC/USD").await; // Returns current multiplier
    /// ```
    pub async fn get_mult(&self, symbol: &str) -> f64 {
        let mult = self.mult.lock().await;
        mult.get(symbol).copied().unwrap_or(1.0)
    }

    /// Resets the position multiplier for a given trading symbol to 1.0.
    ///
    /// This effectively removes any position sizing adjustment, returning
    /// the trading strategy to its base position size.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading symbol to reset multiplier for
    ///
    /// # Example
    ///
    /// ```
    /// bot.reset_mult("BTC/USD").await; // Resets BTC/USD multiplier to 1.0
    /// ```
    pub async fn reset_mult(&self, symbol: &str) {
        let mut old_mult = self.mult.lock().await;
        old_mult.insert(symbol.to_string(), 1.0);
    }

    /// Increases the position multiplier for a given trading symbol by 0.5.
    ///
    /// This is typically used in trading strategies to increase position size
    /// after successful trades or favorable market conditions.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading symbol to increase multiplier for
    ///
    /// # Example
    ///
    /// ```
    /// bot.increase_mult("BTC/USD").await; // Increases multiplier by 0.5
    ///                                     // If current multiplier was 1.0, it becomes 1.5
    /// ```
    pub async fn increase_mult(&self, symbol: &str) {
        let mut mult = self.mult.lock().await;
        let current_mult = mult.get(symbol).copied().unwrap_or(1.0);
        let new_mult = current_mult + 0.5;
        mult.insert(symbol.to_string(), new_mult);
    }

    /// Extracts OHLC (Open, High, Low, Close) data from a log line string.
    ///
    /// Parses log messages containing market data responses to extract
    /// structured OHLC information for multiple trading symbols.
    ///
    /// # Arguments
    ///
    /// * `log_line` - Raw log line string containing OHLC data
    ///
    /// # Returns
    ///
    /// * `Some(Vec<OhlcData>)` - Successfully parsed OHLC data for one or more
    ///   symbols
    /// * `None` - If the log line doesn't contain OHLC data or parsing fails
    ///
    /// # Note
    ///
    /// This function specifically looks for log lines containing
    /// "Ohlc(MarketDataResponse" and extracts the structured data from the
    /// Kraken API response format.
    fn extract_ohlc_data_from_log(log_line: &str) -> Option<Vec<OhlcData>> {
        if !log_line.contains("Ohlc(MarketDataResponse") {
            return None;
        }

        let mut ohlc_data = Vec::new();

        let mut pos = 0;
        while let Some(start) = log_line[pos..].find("Ohlc { symbol: ") {
            pos += start;
            if let Some(end) = log_line[pos..].find(" }") {
                let ohlc_str = &log_line[pos..(pos + end + 2)];

                let symbol = Self::extract_string_field(ohlc_str, "symbol:", ",")
                    .unwrap_or("unknown".to_string());
                let open = Self::extract_float_field(ohlc_str, "open:", ",").unwrap_or(0.0);
                let high = Self::extract_float_field(ohlc_str, "high:", ",").unwrap_or(0.0);
                let low = Self::extract_float_field(ohlc_str, "low:", ",").unwrap_or(0.0);
                let close = Self::extract_float_field(ohlc_str, "close:", ",").unwrap_or(0.0);
                let vwap = Self::extract_float_field(ohlc_str, "vwap:", ",").unwrap_or(0.0);
                let trades = Self::extract_int_field(ohlc_str, "trades:", ",").unwrap_or(0);
                let volume = Self::extract_float_field(ohlc_str, "volume:", ",").unwrap_or(0.0);
                let interval_begin = Self::extract_string_field(ohlc_str, "interval_begin:", ",")
                    .unwrap_or("unknown".to_string());
                let interval = Self::extract_int_field(ohlc_str, "interval:", " }").unwrap_or(60);

                ohlc_data.push(OhlcData {
                    symbol,
                    open,
                    high,
                    low,
                    close,
                    vwap,
                    trades,
                    volume,
                    interval_begin,
                    interval,
                });

                pos += end + 2;
            } else {
                break;
            }
        }

        if ohlc_data.is_empty() {
            None
        } else {
            Some(ohlc_data)
        }
    }

    /// Extracts a string field value from a text using a field name and
    /// delimiter.
    ///
    /// Utility function to parse specific string fields from formatted text,
    /// commonly used when parsing API responses or log messages.
    ///
    /// # Arguments
    ///
    /// * `text` - Source text to search in
    /// * `field_name` - Name of the field to find (e.g., "symbol:", "asset:")
    /// * `delimiter` - Character(s) that mark the end of the field value
    ///
    /// # Returns
    ///
    /// * `Some(String)` - The extracted field value if found
    /// * `None` - If the field is not found or cannot be parsed
    ///
    /// # Example
    ///
    /// ```
    /// let result =
    ///     TradingBot::extract_string_field("symbol: \"BTC/USD\", price: 45000", "symbol:", ",");
    /// // Returns Some("\"BTC/USD\"".to_string())
    /// ```
    fn extract_string_field(text: &str, field_name: &str, delimiter: &str) -> Option<String> {
        if let Some(start_idx) = text.find(field_name) {
            let start = start_idx + field_name.len();
            if let Some(end_idx) = text[start..].find(delimiter) {
                let value = text[start..(start + end_idx)].trim();
                let value = value.trim_start_matches('"').trim_end_matches('"');
                return Some(value.to_string());
            }
        }
        None
    }

    /// Extracts a floating-point field value from text.
    ///
    /// Convenience wrapper around `extract_string_field` that automatically
    /// parses the extracted string value as a floating-point number.
    ///
    /// # Arguments
    ///
    /// * `text` - Source text to search in
    /// * `field_name` - Name of the field to find
    /// * `delimiter` - Character(s) that mark the end of the field value
    ///
    /// # Returns
    ///
    /// * `Some(f64)` - The parsed floating-point value if found and valid
    /// * `None` - If the field is not found or cannot be parsed as a number
    ///
    /// # Example
    ///
    /// ```
    /// let price = TradingBot::extract_float_field("price: 45000.50,", "price:", ",");
    /// // Returns Some(45000.50)
    /// ```
    fn extract_float_field(text: &str, field_name: &str, delimiter: &str) -> Option<f64> {
        Self::extract_string_field(text, field_name, delimiter).and_then(|s| s.parse::<f64>().ok())
    }

    /// Extracts an integer field value from text.
    ///
    /// Convenience wrapper around `extract_string_field` that automatically
    /// parses the extracted string value as an integer.
    ///
    /// # Arguments
    ///
    /// * `text` - Source text to search in
    /// * `field_name` - Name of the field to find
    /// * `delimiter` - Character(s) that mark the end of the field value
    ///
    /// # Returns
    ///
    /// * `Some(i64)` - The parsed integer value if found and valid
    /// * `None` - If the field is not found or cannot be parsed as an integer
    ///
    /// # Example
    ///
    /// ```
    /// let trades = TradingBot::extract_int_field("trades: 1250,", "trades:", ",");
    /// // Returns Some(1250)
    /// ```
    fn extract_int_field(text: &str, field_name: &str, delimiter: &str) -> Option<i64> {
        Self::extract_string_field(text, field_name, delimiter).and_then(|s| s.parse::<i64>().ok())
    }

    /// Fetches real-time cryptocurrency OHLC data from Kraken WebSocket API.
    ///
    /// Establishes a WebSocket connection to Kraken and subscribes to OHLC data
    /// for the specified trading symbols. Collects data for a specified
    /// duration and returns the accumulated OHLC information.
    ///
    /// # Arguments
    ///
    /// * `symbols` - Vector of trading symbols to fetch data for (e.g.,
    ///   ["BTC/USD", "ETH/EUR"])
    /// * `interval` - OHLC interval in minutes (e.g., 1, 5, 15, 60)
    /// * `timeout_seconds` - Maximum time to collect data before returning
    ///   results
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<OhlcData>)` - Successfully collected OHLC data for requested
    ///   symbols
    /// * `Err(String)` - If WebSocket connection fails or data collection
    ///   encounters errors
    ///
    /// # Example
    ///
    /// ```
    /// let symbols = vec!["BTC/USD".to_string(), "ETH/USD".to_string()];
    /// let ohlc_data = TradingBot::fetch_crypto_data(symbols, 60, 30).await?;
    /// // Fetches 1-hour OHLC data for BTC and ETH over 30 seconds
    /// ```
    ///
    /// # Note
    ///
    /// This function will block for up to `timeout_seconds` while collecting
    /// data. The WebSocket connection includes tracing for debugging
    /// purposes.
    pub async fn fetch_crypto_data(
        symbols: Vec<String>,
        interval: u32,
        timeout_seconds: u64,
    ) -> Result<Vec<OhlcData>, String> {
        let mut client = KrakenWSSClient::new_with_tracing(WS_KRAKEN, WS_KRAKEN_AUTH, true, true);
        let mut public_stream = client
            .connect::<WssMessage>()
            .await
            .map_err(|e| format!("Failed to connect to Kraken: {}", e))?;

        let ohlc_params = OhlcSubscription::new(symbols.clone(), interval.try_into().unwrap());
        let subscription = Message::new_subscription(ohlc_params, 0);

        public_stream
            .send(&subscription)
            .await
            .map_err(|e| format!("Failed to send subscription: {}", e))?;

        info!(
            "Subscribed to symbols: {:?} with interval: {}",
            symbols, interval
        );

        let mut received_data = HashMap::new();
        let mut all_ohlc_data = Vec::new();

        let start_time = Instant::now();

        while start_time.elapsed() < Duration::from_secs(timeout_seconds)
            && received_data.len() < symbols.len()
        {
            match timeout(Duration::from_secs(1), public_stream.next()).await {
                Ok(Some(message)) => {
                    if let Ok(response) = message {
                        if let Some(ohlc_entries) =
                            Self::extract_ohlc_data_from_log(&format!("{:?}", response))
                        {
                            for entry in ohlc_entries {
                                received_data.insert(entry.symbol.clone(), entry.clone());
                                all_ohlc_data.push(entry);
                            }
                        }
                    }
                }
                _ => {
                    // Continue waiting for messages
                }
            }
        }

        drop(public_stream);

        if received_data.is_empty() {
            eprintln!("No data received within timeout period");
        }

        Ok(all_ohlc_data)
    }

    /// Calculates the percentage return for a trading symbol based on price
    /// history.
    ///
    /// Uses the stored price history to compute the return between the oldest
    /// and most recent prices. Requires sufficient price history data to
    /// calculate.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading symbol to calculate return for
    ///
    /// # Returns
    ///
    /// * `Some(f64)` - The percentage return if sufficient price history exists
    /// * `None` - If no price history exists for the symbol or insufficient
    ///   data
    ///
    /// # Example
    ///
    /// ```
    /// let return_pct = bot.calculate_return("BTC/USD");
    /// // Returns Some(2.5) for a 2.5% return, or None if insufficient data
    /// ```
    pub fn calculate_return(&self, symbol: &str) -> Option<f64> {
        self.price_history
            .get(symbol)
            .and_then(|ph| ph.calculate_return())
    }

    /// Generates trading signals based on current positions and price
    /// movements.
    ///
    /// Analyzes the current position, price history, and configured thresholds
    /// to determine whether to buy, sell, or hold for a given trading symbol.
    ///
    /// # Arguments
    ///
    /// * `symbol` - Trading symbol to generate signal for
    ///
    /// # Returns
    ///
    /// A string indicating the recommended action:
    /// - "BUY" - Enter a position (when no current position and price drop
    ///   exceeds entry threshold)
    /// - "SELL" - Exit position (when holding position and price rise exceeds
    ///   exit threshold)
    /// - "HOLD" - No action recommended (insufficient price movement or
    ///   history)
    ///
    /// # Logic
    ///
    /// - Uses symbol-specific entry/exit thresholds multiplied by current
    ///   multiplier
    /// - Requires sufficient price history to calculate percentage returns
    /// - Entry signals generated when price drops below entry threshold with no
    ///   position
    /// - Exit signals generated when price rises above exit threshold with
    ///   existing position
    ///
    /// # Example
    ///
    /// ```
    /// let signal = bot.generate_signal("BTC/USD").await;
    /// match signal.as_str() {
    ///     "BUY" => println!("Enter BTC position"),
    ///     "SELL" => println!("Exit BTC position"),
    ///     "HOLD" => println!("No action needed"),
    ///     _ => {}
    /// }
    /// ```
    pub async fn generate_signal(&mut self, symbol: &str) -> String {
        let current_position = self.get_position(symbol).await;
        let symbol_config = self.symbols_config.get(symbol).unwrap_or_else(|| {
            eprintln!("Symbol {} not found in symbols_config", symbol);
            self.symbols_config.values().next().unwrap()
        });
        let m = self.get_mult(symbol).await;
        let entry = symbol_config.entry_threshold * m;
        let exit = symbol_config.exit_threshold * m;

        let history = self
            .price_history
            .entry(symbol.to_string())
            .or_insert_with(PriceHistory::new);

        if let Some(return_pct) = history.calculate_return() {
            match (return_pct, current_position) {
                (r, _) if r < entry => {
                    self.increase_mult(symbol).await;
                    "BUY".to_string()
                }
                (r, _) if r > exit && current_position > 0.0 => {
                    self.increase_mult(symbol).await;
                    "SELL".to_string()
                }
                (r, _) if r > -0.5 && r < 0.5 => {
                    self.reset_mult(symbol).await;
                    "HOLD".to_string()
                }
                _ => "HOLD".to_string(),
            }
        } else {
            "HOLD (INSUFFICIENT DATA)".to_string()
        }
    }
}

// Implement the TelegramTradingBot trait for our TradingBot
#[async_trait]
impl TelegramTradingBot for TradingBot {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    async fn new() -> Result<Self, Self::Error>
    where
        Self: Sized,
    {
        Self::new()
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                Box::new(BotError(format!("{}", e)))
            })
    }

    async fn execute_strategy(
        &mut self,
        bot_state: Arc<Mutex<BotState>>,
        telegram_bot: Bot,
        chat_id: ChatId,
    ) -> Result<(), Self::Error> {
        self.execute_strategy(bot_state, telegram_bot, chat_id)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                Box::new(BotError(format!("{}", e)))
            })
    }

    async fn get_status(&self) -> String {
        // Implement status reporting for the Kraken bot
        let positions = self.positions.lock().await;
        let prices = self.real_time_prices.lock().await;

        let mut status = String::from("Kraken Trading Bot Status:\n\n");

        status.push_str("Positions:\n");
        for (symbol, position) in positions.iter() {
            let current_price = prices.get(symbol).unwrap_or(&0.0);
            status.push_str(&format!(
                "  {}: {:.4} units @ ${:.2}\n",
                symbol, position, current_price
            ));
        }

        status.push_str("\nTracked Symbols:\n");
        for (symbol, config) in &self.symbols_config {
            status.push_str(&format!(
                "  {}: Entry ${:.2}, Exit ${:.2}\n",
                symbol, config.entry_amount, config.exit_amount
            ));
        }

        status
    }

    fn get_config_path(&self) -> &str {
        "/Users/rogerbos/rust_home/kraken_bot/symbols_config.json"
    }

    fn get_interval_seconds(&self) -> u64 {
        INTERVAL_SECONDS
    }
}
