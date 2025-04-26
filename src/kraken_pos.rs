use kraken_async_rs::clients::core_kraken_client::CoreKrakenClient;
use kraken_async_rs::clients::kraken_client::KrakenClient;
use kraken_async_rs::crypto::nonce_provider::{IncreasingNonceProvider, NonceProvider};
use kraken_async_rs::crypto::secrets::Token;
use kraken_async_rs::secrets::secrets_provider::{EnvSecretsProvider, SecretsProvider};
use kraken_async_rs::wss::{BalancesSubscription, KrakenWSSClient, Message, WssMessage, OhlcSubscription, WS_KRAKEN_AUTH, WS_KRAKEN};
use std::{error::Error, sync::Arc, time::Duration, time::Instant, collections::{HashMap, VecDeque}};
use tokio::fs::File;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_stream::StreamExt;
use tokio::io::BufReader;
use tracing::info;
use serde::{Deserialize, Serialize};

pub const KRAKEN_KEY: &str = "KRAKEN_KEY_SOLO";
pub const KRAKEN_SECRET: &str = "KRAKEN_SECRET_SOLO";
pub const INTERVAL_SECONDS: u64 = 120;

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

#[derive(Debug, Clone)]
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

#[derive(Debug)]
pub struct PriceHistory {
    pub closing_prices: VecDeque<f64>,
}

impl PriceHistory {
    pub fn new() -> Self {
        PriceHistory {
            closing_prices: VecDeque::with_capacity(6),
        }
    }

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
    pub async fn new() -> Result<Self, Box<dyn Error>> {
        let secrets_provider: Box<Arc<Mutex<dyn SecretsProvider>>> = Box::new(Arc::new(Mutex::new(
            EnvSecretsProvider::new(KRAKEN_KEY, KRAKEN_SECRET),
        )));
        let nonce_provider: Box<Arc<Mutex<dyn NonceProvider>>> =
            Box::new(Arc::new(Mutex::new(IncreasingNonceProvider::new())));
        let mut kraken_client = CoreKrakenClient::new(secrets_provider, nonce_provider);
        let token_response = kraken_client.get_websockets_token().await?;
        let token = token_response.result.unwrap().token;
        let client = KrakenWSSClient::new();

        // Read and parse the symbols configuration from a file
        let symbols_config = Self::read_symbols_config("/home/rogerbos/rust_home/kraken_bot/symbols_config.json").await?;

        // Initialize mult HashMap with known symbols
        let mut mult_map = HashMap::new();
        let mut symbols_config_map = HashMap::new();
        for symbol_config in &symbols_config {
            mult_map.insert(symbol_config.symbol.clone(), 1.0);
            symbols_config_map.insert(symbol_config.symbol.clone(), Symbol {
                symbol: Box::leak(symbol_config.symbol.clone().into_boxed_str()),
                entry_amount: symbol_config.entry_amount,
                exit_amount: symbol_config.exit_amount,
                entry_threshold: symbol_config.entry_threshold,
                exit_threshold: symbol_config.exit_threshold,
            });
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

    async fn read_symbols_config(file_path: &str) -> Result<Vec<SymbolConfig>, Box<dyn Error>> {
        let content = tokio::fs::read_to_string(file_path).await?;
        let symbols_config: Vec<SymbolConfig> = serde_json::from_str(&content)?;
        Ok(symbols_config)
    }

    pub async fn update_positions(&self, private_stream: &mut kraken_async_rs::wss::KrakenMessageStream<WssMessage>) {
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
                        if let Some(usd_balance) = Self::extract_asset_balance(&message_str, &"USD") {
                            let mut positions = self.positions.lock().await;
                            positions.insert("USD".to_string(), usd_balance);
                        }
                        
                        // Process balances for trading symbols
                        for symbol_config in self.symbols_config.values() {
                            let symbol = &symbol_config.symbol;
                            let base_currency = symbol.split('/').next().unwrap();
                            if let Some(balance) = Self::extract_asset_balance(&message_str, symbol) {
                                let mut positions = self.positions.lock().await;
                                positions.insert(base_currency.to_string(), balance);
                            }
                        }                        
                        break;// We got what we needed, so break out of the loop
                    }
                },
                Ok(Some(Err(e))) => {
                    eprintln!("Error receiving message: {:?}", e);
                    attempts += 1;
                },
                Ok(None) => {
                    eprintln!("Stream ended unexpectedly");
                    break;
                },
                Err(_) => {
                    eprintln!("Timeout waiting for message");
                    attempts += 1;
                }
            }
        }
        
        if attempts >= max_attempts {
            eprintln!("Failed to receive balance data after {} attempts", max_attempts);
        }
    }
    
    fn extract_asset_balance(message_str: &str, symbol: &str) -> Option<f64> {
        let base_currency = symbol.split('/').next().unwrap();
        let asset = format!("asset: \"{}\"", base_currency);
        if let Some(usd_idx) = message_str.find(&asset) {
            if let Some(balance_idx) = message_str[usd_idx..].find("balance: ") {
                let balance_start = usd_idx + balance_idx + "balance: ".len();
                if let Some(balance_end) = message_str[balance_start..].find(|c| c == ',' || c == '}') {
                    let balance_str = &message_str[balance_start..balance_start + balance_end].trim();
                    return balance_str.parse::<f64>().ok();
                }
            }
        }
        None
    }

    pub async fn get_position(&self, symbol: &str) -> f64 {
        let base_currency = symbol.split('/').next().unwrap();
        let positions = self.positions.lock().await;
        positions.get(base_currency).copied().unwrap_or(0.0)
    }
    
    pub async fn get_real_time_price(&self, symbol: &str) -> f64 {
        let prices = self.real_time_prices.lock().await;
        prices.get(symbol).copied().unwrap_or(0.0)
    }

    pub async fn get_mult(&self, symbol: &str) -> f64 {
        let mult = self.mult.lock().await;
        mult.get(symbol).copied().unwrap_or(1.0)
    }

    pub async fn reset_mult(&self, symbol: &str) {
        let mut old_mult = self.mult.lock().await;
        old_mult.insert(symbol.to_string(), 1.0);
    }

    pub async fn set_mult(&self, symbol: &str, value: f64) {
        let mut mult = self.mult.lock().await;
        let new_mult = value + 0.5;
        mult.insert(symbol.to_string(), new_mult);
    }

    pub async fn increase_mult(&self, symbol: &str) {
        let mut mult = self.mult.lock().await;
        let current_mult = mult.get(symbol).copied().unwrap_or(1.0);
        let new_mult = current_mult + 0.5;
        mult.insert(symbol.to_string(), new_mult);
    }

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
                
                let symbol = Self::extract_string_field(ohlc_str, "symbol:", ",").unwrap_or("unknown".to_string());
                let open = Self::extract_float_field(ohlc_str, "open:", ",").unwrap_or(0.0);
                let high = Self::extract_float_field(ohlc_str, "high:", ",").unwrap_or(0.0);
                let low = Self::extract_float_field(ohlc_str, "low:", ",").unwrap_or(0.0);
                let close = Self::extract_float_field(ohlc_str, "close:", ",").unwrap_or(0.0);
                let vwap = Self::extract_float_field(ohlc_str, "vwap:", ",").unwrap_or(0.0);
                let trades = Self::extract_int_field(ohlc_str, "trades:", ",").unwrap_or(0);
                let volume = Self::extract_float_field(ohlc_str, "volume:", ",").unwrap_or(0.0);
                let interval_begin = Self::extract_string_field(ohlc_str, "interval_begin:", ",").unwrap_or("unknown".to_string());
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
    
    fn extract_float_field(text: &str, field_name: &str, delimiter: &str) -> Option<f64> {
        Self::extract_string_field(text, field_name, delimiter)
            .and_then(|s| s.parse::<f64>().ok())
    }
    
    fn extract_int_field(text: &str, field_name: &str, delimiter: &str) -> Option<i64> {
        Self::extract_string_field(text, field_name, delimiter)
            .and_then(|s| s.parse::<i64>().ok())
    }
    

    pub async fn fetch_crypto_data(symbols: Vec<String>, interval: u32, timeout_seconds: u64) -> Result<Vec<OhlcData>, String> {
        let mut client = KrakenWSSClient::new_with_tracing(WS_KRAKEN, WS_KRAKEN_AUTH, true, true);
        let mut public_stream = client.connect::<WssMessage>().await
            .map_err(|e| format!("Failed to connect to Kraken: {}", e))?;
        
        let ohlc_params = OhlcSubscription::new(symbols.clone(), interval.try_into().unwrap());
        let subscription = Message::new_subscription(ohlc_params, 0);
        
        public_stream.send(&subscription).await
            .map_err(|e| format!("Failed to send subscription: {}", e))?;
        
        info!("Subscribed to symbols: {:?} with interval: {}", symbols, interval);
        
        let mut received_data = HashMap::new();
        let mut all_ohlc_data = Vec::new();
        
        let start_time = Instant::now();
        
        while start_time.elapsed() < Duration::from_secs(timeout_seconds) && received_data.len() < symbols.len() {
            match timeout(Duration::from_secs(1), public_stream.next()).await {
                Ok(Some(message)) => {
                    if let Ok(response) = message {
                        if let Some(ohlc_entries) = Self::extract_ohlc_data_from_log(&format!("{:?}", response)) {
                            for entry in ohlc_entries {
                                received_data.insert(entry.symbol.clone(), entry.clone());
                                all_ohlc_data.push(entry);
                            }
                        }
                    }
                },
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
    
    pub fn calculate_return(&self, symbol: &str) -> Option<f64> {
        self.price_history
            .get(symbol)
            .and_then(|ph| ph.calculate_return())
    }

    pub async fn generate_signal(&mut self, symbol: &str) -> String {
        let current_position = self.get_position(symbol).await;
        let symbol_config = self.symbols_config.get(symbol).unwrap_or_else(|| {
            eprintln!("Symbol {} not found in symbols_config", symbol);
            self.symbols_config.values().next().unwrap()
        });
        let m = self.get_mult(symbol).await;
        let entry = symbol_config.entry_threshold * m;
        let exit = symbol_config.exit_threshold * m;

        let history = self.price_history
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