# Contributing to NexusGate

Thanks for your interest! NexusGate welcomes contributions of all sizes — from typo fixes to new provider integrations.

---

## Quick Setup

```bash
git clone https://github.com/yourname/nexusgate
cd nexusgate
cp .env.example .env
# Edit .env — set ADMIN_JWT_SECRET and at least one provider API key

# Start Redis (required for rate-limit and budget state)
redis-server --port 6379 --daemonize yes

# Run
cargo run
```

Server starts at `http://localhost:8080`.

---

## Good First Issues

Looking for a place to start? These are well-scoped and self-contained:

- **Add a provider** — Cohere, Mistral, Together AI, etc. (see below)
- **Add streaming support** — implement `stream: true` for OpenAI-compatible SSE
- **Write integration tests** — the test suite currently has 0 tests; any coverage helps
- **Improve dashboard UX** — the dashboard is a single `dashboard/index.html` file, no build step

---

## Adding a New Provider

1. Create `src/providers/myprovider.rs`:

```rust
use async_trait::async_trait;
use crate::{error::Result, models::{ProviderRequest, ProviderResponse, ProviderType}};
use super::LlmProvider;

pub struct MyProvider { /* client, api_key */ }

#[async_trait]
impl LlmProvider for MyProvider {
    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse> {
        // 1. Convert ProviderRequest to provider's format
        // 2. Call provider HTTP API
        // 3. On HTTP 429: return Err with message "RATE_LIMITED:{retry_after_secs}"
        // 4. Normalize response into ProviderResponse
    }
}
```

2. Register in `src/providers/mod.rs`:

```rust
pub mod myprovider;
```

3. Add models to `model_catalog()` in `src/models.rs`.

4. Wire into `build_provider()` in `src/api/chat.rs`.

5. Add your provider key to `.env.example` and `src/config.rs`.

---

## Project Layout

```
src/
  main.rs          startup, migrations, bind
  config.rs        env vars
  models.rs        all types, model catalog, pricing
  auth.rs          key hashing, JWT
  budget.rs        spend check + recording
  rate_limit.rs    Redis-backed rate-limit state
  router.rs        model selection + fallback chain
  api/
    chat.rs        POST /v1/chat/completions
    admin.rs       key management + cost analytics
    health.rs      GET /health
  providers/
    mod.rs         LlmProvider trait
    openai.rs
    anthropic.rs
    gemini.rs
dashboard/
  index.html       single-file dashboard (Alpine.js + Chart.js, no build)
migrations/
  001_init.sql     SQLite schema
```

---

## Pull Request Guidelines

- Keep PRs focused — one feature or fix per PR.
- Add a short description of what changed and why.
- Run `cargo test` and `cargo clippy` before submitting.
- For new providers, include a short note on how you tested it (even a mock/stub is fine).

---

## Code Style

- Standard `rustfmt` formatting (`cargo fmt`).
- Avoid unnecessary comments that just restate what the code does.
- Use early returns rather than deep nesting.
- Errors should flow through `AppError` in `src/error.rs`.

---

## License

By contributing, you agree your changes are licensed under Apache-2.0.
