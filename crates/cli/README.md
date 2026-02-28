# evm-state-cli

Unified CLI binary for the EVM State-as-a-Service system. Provides a single `evm-state` command that ties together the API server, block replay pipeline, snapshot importer, and state validator.

## Subcommands

| Command | Description |
|---------|-------------|
| `evm-state serve` | Start the HTTP API server |
| `evm-state replay` | Run the block replay pipeline from the SQD Network |
| `evm-state import <file>` | Import a JSON Lines snapshot into the database |
| `evm-state export <file>` | Export the state database to a JSON Lines snapshot |
| `evm-state validate` | Validate local state against a remote Ethereum RPC |

## Global Options

These options apply to all subcommands:

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--db <PATH>` | `EVM_STATE_DB` | `./state.mdbx` | Database path |
| `--chain-id <ID>` | `EVM_STATE_CHAIN_ID` | `137` | Chain ID (1 = Ethereum, 137 = Polygon) |
| `--config <PATH>` | — | — | Path to a TOML config file |
| `--log-level <LEVEL>` | `EVM_STATE_LOG` | `info` | Log level filter |

## Configuration

Settings are resolved in this priority order (highest wins):

1. CLI flags
2. Environment variables
3. TOML config file (`--config`)
4. Built-in defaults

### Config File Format

```toml
db_path = "./state.mdbx"
chain_id = 137
log_level = "info"
listen = "0.0.0.0:3000"
portal = "https://portal.sqd.dev"
dataset = "polygon-mainnet"
rpc_url = "https://polygon-rpc.com"
```

All fields are optional.

## Examples

```sh
# Start the API server on the default port
evm-state serve

# Start with custom listen address and database
evm-state --db /data/polygon.mdbx serve --listen 127.0.0.1:8080

# Replay Polygon blocks from the SQD Network
evm-state replay

# Replay a specific block range on Ethereum
evm-state --chain-id 1 replay --from 19000000 --to 19001000

# Import a snapshot
evm-state import snapshot.jsonl.zst

# Export state to a snapshot
evm-state export snapshot.jsonl.zst

# Validate against an RPC (50 random samples)
evm-state validate --rpc-url https://polygon-rpc.com --samples 50

# Use a config file
evm-state --config config.toml serve
```
