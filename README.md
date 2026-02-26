# grub

A simple, local-first calorie tracker. No accounts, no cloud, no subscriptions.

Grub stores everything in a local SQLite database on your machine. It uses [OpenFoodFacts](https://openfoodfacts.org) — a free, community-maintained food database — so there's no proprietary data lock-in.

## Install

```sh
cargo install grub
```

## Usage

```sh
# Search for a food and log it
grub log "chicken breast" 200g --meal lunch

# Look up a food by barcode
grub log --barcode 5000159484695 150g --meal dinner

# View today's summary
grub summary

# View a specific date
grub summary 2025-01-15

# Search without logging
grub search "oat milk"

# Start the REST API server (for mobile apps)
grub serve
```

## Features

- **Barcode scanning** — look up foods by barcode via OpenFoodFacts
- **Local food cache** — foods are cached locally after first lookup, works offline
- **Meal tracking** — log meals as breakfast, lunch, dinner, or snack
- **Daily summaries** — calories, protein, carbs, fat, and fiber
- **Recipe support** — create and log custom recipes
- **REST API server** — self-host on your local network for mobile app access
- **JSON output** — every command supports `--json` for scripting

## Building from source

```sh
git clone https://github.com/grub-tools/grub.git
cd grub
cargo build --workspace
```

## Running tests

```sh
cargo test --workspace
```

## License

MIT
