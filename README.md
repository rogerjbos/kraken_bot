# Kraken Trading Bot 🤖📈

A simple Telegram-controlled cryptocurrency trading bot for the Kraken exchange, built with Rust for high performance and safety.  The `symbols_config.json` file allows you to customize trading pairs, thresholds, and strategies.  Please see the example file.

## ✨ Features

- **Telegram Integration**: Full control via Telegram bot commands
- **Kraken Exchange**: Direct integration with Kraken's REST API
- **Real-time Trading**: Execute trades and monitor positions
- **Async Architecture**: High-performance async/await implementation
- **Error Handling**: Robust error management and recovery
- **Modular Design**: Clean separation with reusable telegram-bot crate
- **Environment Configuration**: Secure credential management via `.env` files

## 🚀 Quick Start

### Prerequisites

- **Rust**: Install via [rustup](https://rustup.rs/)
- **Nightly Toolchain**: Required for advanced formatting
  ```bash
  rustup install nightly
  rustup component add rustfmt --toolchain nightly
  ```
- **Telegram Bot**: Create a bot via [@BotFather](https://t.me/botfather)
- **Kraken Account**: API credentials from [Kraken](https://kraken.com)

### Installation

1. **Clone the repository**:
   ```bash
   git clone <your-repo-url>
   cd kraken_bot
   ```

2. **Set up environment variables**:
   ```bash
   cp .env.example .env
   # Edit .env with your actual credentials
   ```

3. **Configure your `.env` file**:
   ```env
   # Telegram Bot Configuration
   TELOXIDE_TOKEN=your_telegram_bot_token_here
   
   # Kraken API Configuration  
   KRAKEN_API_KEY=your_kraken_api_key_here
   KRAKEN_SECRET_KEY=your_kraken_secret_key_here
   
   # Chat ID for notifications
   KRAKEN_BOT_CHAT_ID=your_chat_id_here
   
   # Optional: Logging level
   RUST_LOG=info
   ```

4. **Build and run**:
   ```bash
   cargo build --release
   cargo run
   ```

## 🔧 Development

### Project Structure

```
kraken_bot/
├── src/
│   ├── main.rs                     # Application entry point
│   ├── kraken_pos.rs               # Trading bot core logic & Kraken integration
│   ├── kraken_execute_strategy.rs  # Trading strategy execution
│   └── kraken_execute_trade.rs     # Trade execution utilities
├── Cargo.toml                      # Dependencies and metadata
├── rustfmt.toml                    # Rust formatting configuration
├── .env.example                    # Environment variables template
└── README.md                       # This file
```

### Code Formatting

This project uses **nightly rustfmt** with advanced formatting features:

```bash
# Format all code
cargo +nightly fmt

# Or use the convenience script
./format.sh
```

### Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run with logging
RUST_LOG=debug cargo run
```

## 🏗️ Architecture

### Core Components

- **`TradingBot`**: Main bot implementation with Kraken API integration
- **`TelegramBotHandler`**: Generic Telegram bot handler from `telegram-bot` crate
- **`BotState`**: Application state management with thread-safe access
- **Strategy Execution**: Modular trading strategy implementation

### Dependencies

| Crate | Purpose |
|-------|---------|
| `telegram-bot` | Custom Telegram bot framework (local crate) |
| `teloxide` | Telegram Bot API bindings |
| `kraken-async-rs` | Async Kraken exchange API client |
| `tokio` | Async runtime |
| `serde` | Serialization/deserialization |
| `rust_decimal` | High-precision decimal arithmetic |
| `dotenvy` | Environment variable loading |
| `chrono` | Date and time handling |

## 🔒 Security

- **API Keys**: Stored in `.env` file (never commit to git)
- **Environment Variables**: Loaded securely at runtime
- **Error Handling**: Sensitive data excluded from error messages
- **Input Validation**: All user inputs validated and sanitized

## 📱 Telegram Commands

The bot supports various commands through Telegram:

- `/start` - Initialize the bot
- `/status` - Check bot status
- `/positions` - View current positions
- `/execute` - Execute trading strategy
- `/stop` - Stop the bot

## 🔧 Configuration

### Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `TELOXIDE_TOKEN` | Telegram bot token from @BotFather | Yes |
| `KRAKEN_API_KEY` | Kraken API key | Yes |
| `KRAKEN_SECRET_KEY` | Kraken API secret | Yes |
| `KRAKEN_BOT_CHAT_ID` | Telegram chat ID for notifications | Yes |
| `RUST_LOG` | Logging level (debug/info/warn/error) | No |

### Kraken API Setup

1. Log into your Kraken account
2. Go to Settings → API
3. Create a new API key with required permissions:
   - Query funds
   - Query open/closed orders
   - Query ledger entries
   - Create & cancel orders (for trading)
4. Copy the API key and secret to your `.env` file

### Telegram Setup

1. Message [@BotFather](https://t.me/botfather) on Telegram
2. Create a new bot with `/newbot`
3. Copy the bot token to your `.env` file
4. Get your chat ID by messaging [@userinfobot](https://t.me/userinfobot)

## 🚨 Error Handling

The bot includes comprehensive error handling:

- **Network Issues**: Automatic retry with exponential backoff
- **API Errors**: Detailed error logging and user notification
- **Invalid Commands**: Graceful error responses
- **State Recovery**: Automatic state restoration after crashes

## 📊 Logging

Configure logging levels via the `RUST_LOG` environment variable:

```bash
# Debug level (verbose)
RUST_LOG=debug cargo run

# Info level (default)
RUST_LOG=info cargo run

# Warn level (minimal)
RUST_LOG=warn cargo run
```

## 🤝 Contributing

1. Fork the repository
2. Create a feature branch: `git checkout -b feature/amazing-feature`
3. Make your changes
4. Format code: `cargo +nightly fmt`
5. Run tests: `cargo test`
6. Commit changes: `git commit -m 'Add amazing feature'`
7. Push to branch: `git push origin feature/amazing-feature`
8. Open a Pull Request

## 📜 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## ⚠️ Disclaimer

**This software is for educational and research purposes only. Trading cryptocurrencies involves substantial risk of loss and is not suitable for every investor. Past performance does not guarantee future results. Use at your own risk.**

## 🔗 Related Projects

- [`telegram-bot`](../telegram-bot/) - Reusable Telegram bot framework crate
- [Kraken API Documentation](https://docs.kraken.com/rest/)
- [Teloxide Documentation](https://docs.rs/teloxide/)

## 📧 Support

If you encounter any issues or have questions:

1. Check the existing [Issues](../../issues)
2. Create a new issue with detailed information
3. Include relevant log output and configuration (sanitized)

---

**Built with ❤️ and Rust** 🦀