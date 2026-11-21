# pane-gateway

The Pane local gateway. Proxies Pane's LLM requests to OpenClaw, handles sync, sessions, and auth.

## Running

```bash
# Default port 8443, data dir ~/.pane-gateway
OC_URL=ws://localhost:5173 OC_BEARER_TOKEN=<token> cargo run --release

# Custom port and data dir
GATEWAY_PORT=9443 GATEWAY_DATA_DIR=/custom/path cargo run --release

# TLS modes
GATEWAY_TLS_MODE=tailscale cargo run --release   # Tailscale cert
GATEWAY_TLS_MODE=auto      cargo run --release   # Try Tailscale, fall back to self-signed (default)
GATEWAY_TLS_MODE=selfsigned cargo run --release  # Always self-signed
```

## Pairing

On startup, the gateway prints a 6-digit pairing code. Enter it in Pane's onboarding screen to pair.

To generate a fresh code without restarting:
```bash
pane-gateway pairing-code
```

---

## §3 — Parameter Compatibility Layer

### What it does

Some downstream models reject request parameters that were previously accepted. For example, Anthropic deprecated `temperature` and `top_p` for `claude-opus-4-7`. The gateway strips these before forwarding the request, so no Pane client needs to know about provider quirks.

This is the **param-compat layer** (`src/param_compat.rs`). It runs inside `POST /v1/chat/completions` BEFORE the model field is rewritten to `"openclaw"` — which is the only window where the real downstream model name is available.

### Config file

The strip rules live at:
```
~/.pane-gateway/param-compat.json
```

On first gateway run, if this file doesn't exist, the bundled default (`config/param-compat-default.json`) is written there automatically. You can edit it and restart the gateway — **no rebuild required**.

### Adding a new model

1. Open `~/.pane-gateway/param-compat.json`
2. Add an entry to the `"rules"` array:

```json
{
  "match": { "kind": "exact", "value": "anthropic/my-new-model" },
  "strip": ["temperature", "top_p"],
  "reason": "my-new-model rejects sampling controls per Anthropic migration guide 2026-XX"
}
```

3. Restart the gateway: `pkill pane-gateway && pane-gateway`

### Match kinds

| Kind | Behavior | Example |
|------|----------|---------|
| `"exact"` | Model ID must match exactly | `"claude-opus-4-7"` matches only `claude-opus-4-7` |
| `"prefix"` | Model ID must start with the value | `"anthropic/claude-opus-4-8"` matches `anthropic/claude-opus-4-8`, `anthropic/claude-opus-4-8-preview`, etc. |
| `"regex"` | Full-string regex match (auto-anchored by the gateway with `^(?:...)$`) | `"claude-opus-4-[0-9]+"` matches `claude-opus-4-7` but NOT `anthropic/claude-opus-4-7` |

Note on regex: the gateway auto-wraps your pattern in `^(?:<pattern>)$` so it always full-string matches. If you intentionally want substring matching, use `.*pattern.*` in your value.

### Shipped rules (v0.2.0)

| Model | Kind | Stripped params |
|-------|------|-----------------|
| `anthropic/claude-opus-4-7` | exact | `temperature`, `top_p` |
| `claude-opus-4-7` | exact | `temperature`, `top_p` |
| `anthropic/claude-opus-4-8*` | prefix | `temperature`, `top_p` |
| `claude-opus-4-8*` | prefix | `temperature`, `top_p` |

### Observability

Every stripped param is logged at INFO level:
```
[PARAM_COMPAT] Stripped temperature for model claude-opus-4-7: Anthropic migration guide: ...
[PARAM_COMPAT] Stripped top_p for model claude-opus-4-7: ...
[PARAM_COMPAT] 2 param(s) stripped for model claude-opus-4-7
```

---

## Tests

```bash
# Unit tests only
cargo test --lib

# All tests (unit + integration)
cargo test

# Param-compat tests only
cargo test param_compat
```

---

## Architecture notes

- **§22C:** The gateway rewrites `body["model"]` to `"openclaw"` before forwarding to OC. OC resolves `"openclaw"` to the configured primary model. The param-compat strip MUST happen before this overwrite — see `src/routes/chat.rs`.
- **§4A.2.2 spec:** Gateway Parameter Compatibility Layer — see `docs/spec/ai-integration.md`.
- **Versioning:** Gateway has its own `version` in `Cargo.toml`, independent of Pane's `package.json`. Gateway v0.2.0 = param-compat layer added.
