// #3161 PR 1/2 — §39 Image generation, OpenRouter + Venice.
//
// This module holds everything specific to `generate_image` that isn't
// already covered by an existing mechanism:
//   - the OpenRouter image adapter (§39.1): POST /api/v1/images, base64
//     decode, media_type -> extension, 4xx no retry / 5xx one retry, 120s
//     timeout.
//   - the Venice image adapter (§39.1, PR 2): POST /image/generate,
//     per-model sizing-parameter branching from `GET /models?type=image`
//     metadata, `aspect_ratio` -> pixel translation for pixel-only models.
//   - the image-model-list fetch for Settings -> AI -> Image generation
//     (§39.2): GET /api/v1/images/models (OpenRouter) / GET /models?type=
//     image (Venice).
//   - the tool's own logic (§39.3): resolve settings, call the adapter,
//     write through `attachments::save_attachment_bytes`, and (when
//     `note_id` is given) compute the plain-markdown embed line's
//     position in the note.
//
// What this module deliberately does NOT do: the cross-project write
// guard (tools.rs::check_cross_project_write_permission, #3161 PR 1
// commit 1 — lands separately and is called by the dispatch branch in
// chat.rs BEFORE this module's `execute_generate_image` is ever reached),
// tool registration (tools.rs::get_pane_tools), and the `max_images_per_
// turn` cap itself (#3161 PR 2, §39.4) — that's turn-scoped state that
// lives in `ai::chat::ai_chat_stream`'s own locals, not here; this module
// only reads the cap's VALUE (a third `ai_settings` key) the same way it
// already reads `image_provider`/`image_model`.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// `ai_settings` keys §39.2/§39.4 introduce. No migration — `ai_settings`
/// is the existing generic key/value table every other provider-pick
/// setting already uses (embeddings_status, onboarding_completed, ...).
pub(crate) const IMAGE_PROVIDER_SETTING_KEY: &str = "image_provider";
pub(crate) const IMAGE_MODEL_SETTING_KEY: &str = "image_model";
/// §39.4 / #3161 PR 2: "max_images_per_turn (setting, default 4)". Nullable
/// — absent means the default applies (same null-means-default shape as
/// every other `ai_settings` numeric-ish key in this codebase; see
/// `max_images_per_turn_setting` below for the parse-and-default step).
pub(crate) const MAX_IMAGES_PER_TURN_SETTING_KEY: &str = "max_images_per_turn";
/// §39.4's own text: "max_images_per_turn (setting, default 4)".
pub(crate) const DEFAULT_MAX_IMAGES_PER_TURN: u32 = 4;

/// §39.1's build order: OpenRouter first (PR 1), Venice second (PR 2).
/// Compile-time known — the Settings provider dropdown filters connected
/// runtimes against this list, not a live "does this provider support
/// images" probe (Reed's Q3 answer, 2026-11-03: the provider universe is
/// known ahead of time; only the MODEL list needs a fetch, and only after
/// the user picks a provider).
pub(crate) const IMAGE_CAPABLE_PROVIDER_TYPES: &[&str] = &["openrouter", "venice"];

/// §39.4: "the per-turn image count lives on the turn's own state ... read
/// from a third `ai_settings` key, nullable, default-4-when-absent — same
/// no-migration pattern as `image_provider`/`image_model`." Parses a
/// non-negative integer; any other stored value (unset, empty, garbage)
/// falls back to the documented default rather than erroring — a turn
/// cap is a soft limit, not something that should take `generate_image`
/// down entirely over a bad setting value.
pub(crate) fn max_images_per_turn_setting(conn: &rusqlite::Connection) -> u32 {
    crate::database::get_ai_setting(conn, MAX_IMAGES_PER_TURN_SETTING_KEY)
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(DEFAULT_MAX_IMAGES_PER_TURN)
}

/// Shared HTTP retry policy for every image-generation call (§39.1/§39.6,
/// Reed's PR 2 scope commit 2): 4xx never retries; 5xx retries exactly
/// once. `build_request` is a FACTORY, not a built request — a
/// `RequestBuilder` is consumed by `.send()`, so a retry needs a fresh
/// one; each adapter's closure re-captures its own client/url/headers/body
/// (cheap — `reqwest::Client` clones are `Arc`-backed). Shared so the two
/// adapters' retry behavior can never independently drift from each
/// other — written once, here, rather than duplicated per-provider.
pub(crate) async fn post_image_request_with_retry<F>(
    build_request: F,
) -> Result<reqwest::Response, String>
where
    F: Fn() -> reqwest::RequestBuilder,
{
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let response = build_request()
            .send()
            .await
            .map_err(|e| format!("Failed to call image API: {}", e))?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        // 4xx: no retry, ever.
        if status.is_client_error() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(format!("Image API error ({}): {}", status, error_text));
        }

        // 5xx: exactly one retry.
        if attempt >= 2 {
            let error_text = response.text().await.unwrap_or_default();
            return Err(format!("Image API error ({}): {}", status, error_text));
        }
        eprintln!(
            "[IMAGE] provider returned {} on attempt {} — retrying once",
            status, attempt
        );
    }
}

// ============================================================================
// §39.2 — Settings -> AI -> Image generation: the image-model-list fetch.
// ============================================================================

/// One entry in the OpenRouter image-model picker. `price_usd` is `None`
/// when OpenRouter's own `/images/models` list doesn't report a price for
/// that model (per-model pricing lives behind a separate per-endpoint
/// lookup OpenRouter's docs describe, not in this list response) — the
/// Settings UI must show nothing in that case, never "$0.00" (§39.2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenRouterImageModel {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterImageModelWire {
    id: String,
    name: String,
    /// Not documented on the real `/images/models` list response as of
    /// Reed's 2026-11-03 doc check (pricing lives behind the per-model
    /// `/endpoints` lookup instead) — kept `#[serde(default)]` so a future
    /// OpenRouter response that DOES inline a price is picked up for free,
    /// and so today's real (price-less) response still deserializes.
    #[serde(default)]
    pricing: Option<Vec<OpenRouterImagePricingLineWire>>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterImagePricingLineWire {
    #[serde(default)]
    billable: Option<String>,
    #[serde(default)]
    cost_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterImageModelsResponseWire {
    data: Vec<OpenRouterImageModelWire>,
}

/// Fetch the OpenRouter image-model list. Base URL injectable for tests —
/// same seam `fetch_anthropic_models_at`/`fetch_openrouter_models_at`
/// (connections.rs) already use.
pub(crate) async fn fetch_openrouter_image_models_at(
    api_key: &str,
    base_url: &str,
) -> Result<Vec<OpenRouterImageModel>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let url = format!("{}/images/models", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch OpenRouter image models: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!(
            "OpenRouter image-models API error ({}): {}",
            status, error_text
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;
    let parsed: OpenRouterImageModelsResponseWire = serde_json::from_str(&body)
        .map_err(|e| format!("Failed to parse OpenRouter image-models response: {}", e))?;

    Ok(parsed
        .data
        .into_iter()
        .map(|m| {
            let price_usd = m.pricing.as_ref().and_then(|lines| {
                lines
                    .iter()
                    .find(|l| l.billable.as_deref() == Some("output_image"))
                    .and_then(|l| l.cost_usd)
            });
            OpenRouterImageModel {
                id: m.id,
                name: m.name,
                price_usd,
            }
        })
        .collect())
}

pub(crate) async fn fetch_openrouter_image_models(
    api_key: &str,
) -> Result<Vec<OpenRouterImageModel>, String> {
    fetch_openrouter_image_models_at(api_key, "https://openrouter.ai/api/v1").await
}

/// Tauri command: Settings -> AI -> Image generation's provider-picked
/// model dropdown (§39.2). No key field on this section (C50) — the key
/// comes from the OpenRouter connection already on file, read here on the
/// backend, never round-tripped through the frontend.
#[tauri::command]
pub async fn fetch_openrouter_image_models_cmd(
    db: tauri::State<'_, crate::database::DbState>,
) -> Result<Vec<OpenRouterImageModel>, String> {
    let api_key = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::ai::credential_migration::read_provider_key(&conn, "openrouter")
    }
    .ok_or_else(|| {
        "OpenRouter is not connected — add it in Settings → AI → Provider Connections".to_string()
    })?;

    fetch_openrouter_image_models(&api_key).await
}

// ============================================================================
// §39.1 — the OpenRouter image adapter.
// ============================================================================

#[derive(Debug, Deserialize)]
struct OpenRouterImageGenResponseWire {
    data: Vec<OpenRouterImageGenDataWire>,
    #[serde(default)]
    usage: Option<OpenRouterImageGenUsageWire>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterImageGenDataWire {
    b64_json: String,
    /// "omitted only when undeterminable" per OpenRouter's own docs (Reed,
    /// 2026-11-03) — `#[serde(default)]`, not required.
    #[serde(default)]
    media_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenRouterImageGenUsageWire {
    #[serde(default)]
    cost: Option<f64>,
}

/// A decoded, ready-to-save image plus whatever the provider told us about
/// it. `cost_usd: None` means the provider didn't report one — §39.3/§39.4:
/// never printed or serialized as `0`.
#[derive(Debug, Clone)]
pub(crate) struct DecodedImage {
    pub bytes: Vec<u8>,
    pub extension: String,
    pub cost_usd: Option<f64>,
}

/// `media_type` -> file extension. Falls back to `png` when the provider
/// didn't report a type at all — same "undeterminable" case OpenRouter's
/// own docs name; `png` is a safe, always-valid container for arbitrary
/// decoded image bytes, not a guess at the real format being masked.
pub(crate) fn media_type_to_extension(media_type: Option<&str>) -> &'static str {
    match media_type {
        Some("image/png") => "png",
        Some("image/jpeg") | Some("image/jpg") => "jpg",
        Some("image/webp") => "webp",
        Some("image/gif") => "gif",
        Some("image/avif") => "avif",
        _ => "png",
    }
}

/// POST /api/v1/images, with base URL injectable for tests (same seam as
/// the model-list fetch above). §39.1/§39.6: 120s timeout, one retry on a
/// 5xx, none on a 4xx.
pub(crate) async fn generate_openrouter_image_at(
    api_key: &str,
    model: &str,
    prompt: &str,
    aspect_ratio: Option<&str>,
    base_url: &str,
) -> Result<DecodedImage, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let url = format!("{}/images", base_url.trim_end_matches('/'));

    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
    });
    if let Some(ar) = aspect_ratio {
        body["aspect_ratio"] = serde_json::Value::String(ar.to_string());
    }

    let response = post_image_request_with_retry(|| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
    })
    .await?;

    let text = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;
    let parsed: OpenRouterImageGenResponseWire = serde_json::from_str(&text)
        .map_err(|e| format!("Failed to parse OpenRouter image response: {}", e))?;
    let first = parsed
        .data
        .into_iter()
        .next()
        .ok_or_else(|| "OpenRouter returned no image data".to_string())?;
    let bytes = BASE64
        .decode(&first.b64_json)
        .map_err(|e| format!("Invalid base64 image data: {}", e))?;
    let extension = media_type_to_extension(first.media_type.as_deref()).to_string();
    let cost_usd = parsed.usage.and_then(|u| u.cost);
    Ok(DecodedImage {
        bytes,
        extension,
        cost_usd,
    })
}

// ============================================================================
// §39.1 — the Venice image adapter (#3161 PR 2).
// ============================================================================

/// §39.1's Venice row, confirmed against Venice's live docs (`docs.venice.
/// ai/api-reference/endpoint/image/generate`, `.../models/image`, fetched
/// 2026-11-03 — not guessed): sizing is model-specific, and it is
/// genuinely THREE shapes, not the two the spec draft names:
///   - **Pixel** (e.g. `venice-sd35`): `width`/`height` only.
///   - **AspectRatio** (e.g. the Qwen Image family — `qwen-image`,
///     `qwen-image-3`, `qwen-image-3-pro`): `aspect_ratio` only. These
///     REJECT `width`/`height` with a 400 (Venice's own docs, verbatim).
///   - **AspectRatioWithResolution** (e.g. `gpt-image-2`, the Nano Banana
///     family): BOTH `aspect_ratio` AND `resolution` ("1K"/"2K"/"4K").
/// The model list's `model_spec.constraints` genuinely carries the signal
/// needed to tell these apart: `aspectRatios` is "only present for models
/// that support aspect ratio selection" and `resolutions` is "only present
/// for models that support resolution selection" (Venice's own schema doc
/// comments) — so presence/absence of those two array fields, not a
/// hardcoded per-model table, is the branch (`classify_venice_sizing`
/// below). `widthHeightDivisor` is a REQUIRED field on every image model's
/// constraints object regardless of sizing scheme, so its presence can't
/// be used to detect the Pixel case — only `aspectRatios`'s absence can.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VeniceImageSizing {
    Pixel,
    AspectRatio,
    AspectRatioWithResolution,
}

/// Everything the adapter needs to build the right request body for ONE
/// Venice model, parsed from `GET /models?type=image` (§39.1).
#[derive(Debug, Clone)]
pub(crate) struct VeniceImageModelMeta {
    pub id: String,
    pub name: String,
    pub price_usd: Option<f64>,
    pub sizing: VeniceImageSizing,
    /// Pixel-sizing models only: `constraints.widthHeightDivisor` — the
    /// requested width/height must be divisible by this value (Venice's
    /// own docs). Needed by `aspect_ratio_to_pixels` below.
    pub width_height_divisor: Option<u32>,
    /// Resolution-tier models only: `constraints.defaultResolution`. The
    /// tool's own §39.3 params don't expose a `resolution` argument, so
    /// when a resolution-tier model is asked for an `aspect_ratio`, the
    /// adapter sends the model's own default resolution alongside it
    /// rather than inventing one.
    pub default_resolution: Option<String>,
}

/// §39.2's Settings dropdown shape — the public subset of
/// `VeniceImageModelMeta` the frontend actually needs (mirrors
/// `OpenRouterImageModel` above).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VeniceImageModel {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct VeniceImageConstraintsWire {
    #[serde(default, rename = "aspectRatios")]
    aspect_ratios: Option<Vec<String>>,
    #[serde(default)]
    resolutions: Option<Vec<String>>,
    #[serde(default, rename = "defaultResolution")]
    default_resolution: Option<String>,
    #[serde(default, rename = "widthHeightDivisor")]
    width_height_divisor: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct VeniceImagePricingTierWire {
    #[serde(default)]
    usd: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct VeniceImagePricingWire {
    #[serde(default)]
    generation: Option<VeniceImagePricingTierWire>,
    /// Keys are resolution tiers ("1K"/"2K"/"4K") — only present for
    /// resolution-selecting models, "replaces `generation`" per Venice's
    /// own doc comment on this field.
    #[serde(default)]
    resolutions: Option<std::collections::BTreeMap<String, VeniceImagePricingTierWire>>,
}

#[derive(Debug, Deserialize)]
struct VeniceImageModelSpecWire {
    #[serde(default)]
    constraints: Option<VeniceImageConstraintsWire>,
    #[serde(default)]
    pricing: Option<VeniceImagePricingWire>,
}

#[derive(Debug, Deserialize)]
struct VeniceImageModelWire {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    model_spec: Option<VeniceImageModelSpecWire>,
}

#[derive(Debug, Deserialize)]
struct VeniceImageModelsResponseWire {
    data: Vec<VeniceImageModelWire>,
}

/// The branching decision itself (§39.1) — pure, unit-tested directly.
/// `None` (no `constraints` object at all — shouldn't happen per Venice's
/// own schema, where `widthHeightDivisor` is required on every image
/// model, but handled defensively rather than panicking) falls back to
/// `Pixel`: the one scheme every image model's base request shape
/// (`width`/`height`, both with server-side defaults per Venice's own
/// schema) can always satisfy, even with zero model-specific metadata.
fn classify_venice_sizing(constraints: Option<&VeniceImageConstraintsWire>) -> VeniceImageSizing {
    match constraints.and_then(|c| c.aspect_ratios.as_ref()) {
        None => VeniceImageSizing::Pixel,
        Some(_) => match constraints.and_then(|c| c.resolutions.as_ref()) {
            None => VeniceImageSizing::AspectRatio,
            Some(_) => VeniceImageSizing::AspectRatioWithResolution,
        },
    }
}

/// Fetch Venice's image-model list (§39.1/§39.2): `GET /models?type=image`.
/// Base URL injectable for tests — same seam every other fetcher in this
/// module already uses.
pub(crate) async fn fetch_venice_image_models_at(
    api_key: &str,
    base_url: &str,
) -> Result<Vec<VeniceImageModelMeta>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let url = format!("{}/models?type=image", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch Venice image models: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(format!("Venice image-models API error ({}): {}", status, err_text));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;
    let parsed: VeniceImageModelsResponseWire = serde_json::from_str(&body)
        .map_err(|e| format!("Failed to parse Venice image-models response: {}", e))?;

    Ok(parsed
        .data
        .into_iter()
        .map(|m| {
            let constraints = m.model_spec.as_ref().and_then(|s| s.constraints.as_ref());
            let sizing = classify_venice_sizing(constraints);
            let width_height_divisor =
                constraints.and_then(|c| c.width_height_divisor).map(|d| d as u32);
            let default_resolution =
                constraints.and_then(|c| c.default_resolution.clone());
            // Price: a flat `pricing.generation.usd` when present, else the
            // lowest-tier ("1K") `pricing.resolutions` entry when THAT is
            // present, else `None` — never a guessed/synthesized number
            // (§39.2: "nothing shown where it doesn't [report a price]").
            let price_usd = m.model_spec.as_ref().and_then(|s| s.pricing.as_ref()).and_then(|p| {
                p.generation
                    .as_ref()
                    .and_then(|g| g.usd)
                    .or_else(|| {
                        p.resolutions
                            .as_ref()
                            .and_then(|r| r.get("1K").or_else(|| r.values().next()))
                            .and_then(|g| g.usd)
                    })
            });
            VeniceImageModelMeta {
                id: m.id.clone(),
                name: m.name.unwrap_or_else(|| m.id.clone()),
                price_usd,
                sizing,
                width_height_divisor,
                default_resolution,
            }
        })
        .collect())
}

pub(crate) async fn fetch_venice_image_models(
    api_key: &str,
) -> Result<Vec<VeniceImageModelMeta>, String> {
    fetch_venice_image_models_at(api_key, "https://api.venice.ai/api/v1").await
}

/// Tauri command: Settings -> AI -> Image generation's Venice model
/// dropdown (§39.2) — mirrors `fetch_openrouter_image_models_cmd` exactly,
/// down the public subset (`VeniceImageModel`) it returns.
#[tauri::command]
pub async fn fetch_venice_image_models_cmd(
    db: tauri::State<'_, crate::database::DbState>,
) -> Result<Vec<VeniceImageModel>, String> {
    let api_key = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::ai::credential_migration::read_provider_key(&conn, "venice")
    }
    .ok_or_else(|| {
        "Venice is not connected — add it in Settings → AI → Provider Connections".to_string()
    })?;

    Ok(fetch_venice_image_models(&api_key)
        .await?
        .into_iter()
        .map(|m| VeniceImageModel {
            id: m.id,
            name: m.name,
            price_usd: m.price_usd,
        })
        .collect())
}

/// §39.1/Q2 (Reed, 2026-11-03): "`aspect_ratio` -> pixel translation lives
/// in THIS adapter" — Matt's design call. Only reached for a Pixel-sizing
/// model that was asked for an `aspect_ratio` (the tool's only sizing
/// param, §39.3); baseline dimension (1024) is Venice's OWN documented
/// default for `width`/`height` on `POST /image/generate` (not invented),
/// scaled down on the shorter side to preserve the ratio, then rounded
/// DOWN to the nearest multiple of `divisor` so the result never exceeds
/// Venice's own 1280 max while still satisfying "must be divisible by
/// `widthHeightDivisor`" (Venice's own constraint). Returns `None` only
/// for a malformed ratio string (not "W:H", or a zero/negative component)
/// — the caller falls back to omitting width/height entirely in that
/// case, letting Venice's own server-side default apply, rather than
/// guessing.
pub(crate) fn aspect_ratio_to_pixels(aspect_ratio: &str, divisor: u32) -> Option<(u32, u32)> {
    let divisor = divisor.max(1);
    let (w_str, h_str) = aspect_ratio.split_once(':')?;
    let w_ratio: f64 = w_str.trim().parse().ok()?;
    let h_ratio: f64 = h_str.trim().parse().ok()?;
    if w_ratio <= 0.0 || h_ratio <= 0.0 {
        return None;
    }

    const BASELINE: f64 = 1024.0; // Venice's own documented width/height default.
    const MAX_DIM: u32 = 1280; // Venice's own documented width/height maximum.

    let (raw_w, raw_h) = if w_ratio >= h_ratio {
        (BASELINE, BASELINE * h_ratio / w_ratio)
    } else {
        (BASELINE * w_ratio / h_ratio, BASELINE)
    };

    let snap = |v: f64| -> u32 {
        let snapped = ((v / divisor as f64).floor() as u32).max(1) * divisor;
        snapped.min(MAX_DIM)
    };
    Some((snap(raw_w), snap(raw_h)))
}

/// POST /image/generate (§39.1). Branches the request body per the
/// model's `VeniceImageSizing` (passed in, not re-derived here — the
/// caller, `execute_generate_image_at`, fetches the model list once per
/// call and resolves this model's metadata before reaching the adapter).
/// `format: "png"` is always sent explicitly: Venice's generate response
/// carries NO per-image format/media_type field at all (confirmed against
/// the real response schema — unlike OpenRouter's `media_type`), so the
/// extension has to come from what was REQUESTED, not from anything in
/// the response; `png` keeps `decode_image_dimensions` (PNG/JPEG only)
/// able to read the result, where the API's own default (`webp`) would
/// not be. `cost_usd` is always `None` for Venice: the real `POST
/// /image/generate` response has no `usage`/cost field whatsoever —
/// confirmed by reading the live schema, not guessed — so this adapter
/// genuinely has nothing to report, consistent with §39.3/§39.4's "cost
/// omitted when unreported."
pub(crate) async fn generate_venice_image_at(
    api_key: &str,
    model: &str,
    prompt: &str,
    aspect_ratio: Option<&str>,
    model_meta: &VeniceImageModelMeta,
    base_url: &str,
) -> Result<DecodedImage, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let url = format!("{}/image/generate", base_url.trim_end_matches('/'));

    let mut body = serde_json::json!({
        "model": model,
        "prompt": prompt,
        "format": "png",
    });

    match model_meta.sizing {
        VeniceImageSizing::Pixel => {
            // §39.6 mutation target: "send width/height to both — reddens."
            // Pixel models get width/height ONLY — never aspect_ratio.
            if let Some(ar) = aspect_ratio {
                let divisor = model_meta.width_height_divisor.unwrap_or(8);
                if let Some((w, h)) = aspect_ratio_to_pixels(ar, divisor) {
                    body["width"] = serde_json::json!(w);
                    body["height"] = serde_json::json!(h);
                }
                // An unparseable ratio: omit width/height entirely and let
                // Venice's own server-side default apply — never guess.
            }
        }
        VeniceImageSizing::AspectRatio => {
            // Aspect-ratio-only models REJECT width/height with a 400
            // (Venice's own docs) — never send them here.
            if let Some(ar) = aspect_ratio {
                body["aspect_ratio"] = serde_json::Value::String(ar.to_string());
            }
        }
        VeniceImageSizing::AspectRatioWithResolution => {
            if let Some(ar) = aspect_ratio {
                body["aspect_ratio"] = serde_json::Value::String(ar.to_string());
            }
            // The tool's own §39.3 params don't expose `resolution` — use
            // the model's own documented default rather than inventing one.
            if let Some(res) = &model_meta.default_resolution {
                body["resolution"] = serde_json::Value::String(res.clone());
            }
        }
    }

    let response = post_image_request_with_retry(|| {
        client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
    })
    .await?;

    let text = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;
    let parsed: VeniceImageGenResponseWire = serde_json::from_str(&text)
        .map_err(|e| format!("Failed to parse Venice image response: {}", e))?;
    let first_b64 = parsed
        .images
        .into_iter()
        .next()
        .ok_or_else(|| "Venice returned no image data".to_string())?;
    let bytes = BASE64
        .decode(&first_b64)
        .map_err(|e| format!("Invalid base64 image data: {}", e))?;

    Ok(DecodedImage {
        bytes,
        extension: "png".to_string(),
        cost_usd: None,
    })
}

#[derive(Debug, Deserialize)]
struct VeniceImageGenResponseWire {
    images: Vec<String>,
}

/// §39.3's result shape carries `width`/`height`. The `attachments` table
/// has neither column (confirmed — no schema change, §39.8), so these are
/// read straight off the decoded bytes at generation time, not persisted.
/// No new crate dependency: PNG/JPEG dimensions live in a few header bytes
/// each format already documents, so a minimal hand-rolled reader covers
/// the formats OpenRouter's own docs show in practice (PNG in their worked
/// example) without pulling in a full image-decode crate for a number
/// pair. Returns `None` for a format this reader doesn't recognize —
/// the tool result omits width/height in that case rather than guessing.
pub(crate) fn decode_image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // PNG: 8-byte signature, then IHDR chunk (4-byte length + "IHDR" + 4-byte
    // width + 4-byte height, both big-endian).
    const PNG_SIG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.len() >= 24 && &bytes[0..8] == PNG_SIG && &bytes[12..16] == b"IHDR" {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return Some((width, height));
    }

    // JPEG: scan markers for the first Start-Of-Frame (SOF0-SOF3, SOF5-SOF7,
    // SOF9-SOF11, SOF13-SOF15 — 0xC0..0xCF excluding the DHT/JPG/DAC markers
    // 0xC4/0xC8/0xCC). Segment layout: FF xx, 2-byte length, 1-byte
    // precision, 2-byte HEIGHT, 2-byte WIDTH (height comes first in JPEG).
    if bytes.len() >= 4 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        let mut i = 2usize;
        while i + 9 <= bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            if marker == 0xC4 || marker == 0xC8 || marker == 0xCC {
                // DHT/JPG/DAC — not a frame header; skip by its own length.
                let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
                i += 2 + seg_len;
                continue;
            }
            if (0xC0..=0xCF).contains(&marker) {
                let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some((width, height));
            }
            if marker == 0xD8 || marker == 0xD9 {
                i += 2;
                continue;
            }
            if i + 4 > bytes.len() {
                break;
            }
            let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if seg_len < 2 {
                break;
            }
            i += 2 + seg_len;
        }
    }

    None
}

// ============================================================================
// §39.3 — note-embed placement (pure, unit-tested without a DB).
// ============================================================================

/// Strip characters that would corrupt `![alt](src)` on the next parse —
/// `processInline`'s regex (`Editor.svelte`) is `!\[([^\]]*)\]\(([^)]+)\)`,
/// so an unescaped `]` in alt or `)` anywhere truncates the match early.
/// Model-authored text (the default alt is "first 80 chars of the
/// prompt") can easily contain either. Newlines are stripped too — an
/// embedded image is spec'd as one line.
pub(crate) fn sanitize_alt(raw: &str) -> String {
    raw.chars()
        .filter(|c| *c != ']' && *c != ')' && *c != '\n' && *c != '\r')
        .collect()
}

/// §39.3: "alt ... defaults to the first 80 chars of the prompt."
pub(crate) fn default_alt_from_prompt(prompt: &str) -> String {
    let truncated: String = prompt.chars().take(80).collect();
    sanitize_alt(&truncated)
}

/// #3178: the shared "make it safe" rewrite — the ONE function both the
/// notification-body defense-in-depth guard
/// (`notifications::emit_agent_message_notification`) and the one-time
/// repair pass for pre-existing rows
/// (`migrations::repair_absolute_image_paths_once`) call through, rather
/// than each hand-rolling its own copy of this regex. Rewrites any
/// markdown link/embed target that is BOTH filesystem-rooted (starts
/// with `/`) AND ends in the recognized `Assets/<uuid>.<ext>` shape (the
/// same uuid shape `migrations::repair_orphaned_image_links` already
/// matches) to the project-agnostic relative form `Assets/<uuid>.<ext>`
/// — deliberately NOT anchored to any specific real Pane root (no
/// hardcoded `/Users/<name>/Pane`), since the only thing that matters is
/// removing the filesystem-rooted prefix (which leaks the real username
/// and disk layout), not knowing what it was. A link that's already
/// relative (no leading `/`) is left untouched — re-running this against
/// already-safe text is a no-op.
///
/// Returns the rewritten text and how many links were fixed.
pub(crate) fn relativize_absolute_asset_paths(text: &str) -> (String, u32) {
    let re = match regex::Regex::new(
        r"(?P<bang>!)?\[(?P<alt>[^\]]*)\]\(/(?P<prefix>[^()]*?)/Assets/(?P<rest>[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\.[A-Za-z0-9]+)\)",
    ) {
        Ok(re) => re,
        Err(e) => {
            eprintln!("[IMAGE] relativize_absolute_asset_paths: regex compile failed: {}", e);
            return (text.to_string(), 0);
        }
    };
    let mut count = 0u32;
    let new_text = re
        .replace_all(text, |caps: &regex::Captures| {
            count += 1;
            let bang = caps.name("bang").map(|_| "!").unwrap_or("");
            format!("{}[{}](Assets/{})", bang, &caps["alt"], &caps["rest"])
        })
        .into_owned();
    (new_text, count)
}

/// Computes the note's new content with `embed_line` inserted at the
/// resolved `position`, plus an optional human-readable note about what
/// actually happened (surfaced in the tool result — §39.3/§39.6):
///   - "top": embed line becomes the note's first paragraph. No note.
///   - "bottom" (default, and the fallback target for every unrecognized
///     position string): appended at the end, blank-line separated — same
///     shape `execute_append_to_note` (tools.rs) already uses. No note.
///   - "after:<text>": inserted directly after the line containing the
///     FIRST occurrence of the exact substring `<text>`. More than one
///     match -> uses the first, note says how many were found. No match
///     at all -> falls back to bottom, note says so (§39.3's own rule).
///
/// Pure — no DB, no I/O — so every position case is tested directly, not
/// through a full tool-call round trip.
pub(crate) fn insert_image_embed(
    content: &str,
    position: &str,
    embed_line: &str,
) -> (String, Option<String>) {
    let append_bottom = |note: Option<String>| {
        let trimmed = content.trim_end();
        let new_content = if trimmed.is_empty() {
            embed_line.to_string()
        } else {
            format!("{}\n\n{}", trimmed, embed_line)
        };
        (new_content, note)
    };

    if position == "top" {
        let new_content = if content.trim().is_empty() {
            embed_line.to_string()
        } else {
            format!("{}\n\n{}", embed_line, content.trim_start())
        };
        return (new_content, None);
    }

    if let Some(after_text) = position.strip_prefix("after:") {
        if !after_text.is_empty() {
            let match_count = content.matches(after_text).count();
            if match_count == 0 {
                return append_bottom(Some(format!(
                    "after:{} had no match — inserted at the bottom instead",
                    after_text
                )));
            }
            let first_idx = content.find(after_text).expect("match_count > 0 implies a match");
            let line_end = content[first_idx..]
                .find('\n')
                .map(|rel| first_idx + rel)
                .unwrap_or(content.len());
            let (before, after) = content.split_at(line_end);
            let new_content = format!("{}\n\n{}{}", before.trim_end(), embed_line, after);
            let note = if match_count > 1 {
                Some(format!(
                    "after:{} matched {} times — inserted after the first occurrence",
                    after_text, match_count
                ))
            } else {
                None
            };
            return (new_content, note);
        }
    }

    // "bottom", or any unrecognized position string — fail open to the
    // documented default rather than a hard error (§39.3 only documents
    // top/bottom/after: as valid; nothing in the spec says an unexpected
    // value should refuse the whole call).
    append_bottom(None)
}

// ============================================================================
// §39.3 — the tool itself: resolve settings, call the adapter, write
// through `save_attachment_bytes`, embed into a note when asked.
// ============================================================================

/// Home's attachment-location sentinel (`attachments::HOME_ATTACHMENT_
/// PROJECT_SCOPE` is private to that module) — re-declared here rather
/// than made `pub(crate)` there, since it's the one literal value this
/// module needs to pass through, not a shared concept worth exporting.
/// Must stay byte-identical to the real constant; a dedicated test below
/// pins that.
const HOME_PROJECT_SENTINEL: &str = "__global__";

/// §39.3 steps 1-5, minus the cross-project guard (checked by the caller,
/// `ai::chat`'s dispatch branch, BEFORE this function is ever called —
/// `tools::check_cross_project_write_permission`, #3161 PR 1 commit 1 —
/// so the refusal precedes the spend regardless of what this function
/// does). `turn_project_id` is `context`'s `projectId` — set for an
/// interactive project-scoped turn, `None` for Home AND for every job
/// turn (job turns' `context_clone` deliberately omits `projectId`, #3050
/// PR 4 — see `AgentLoopInputs` in `scheduler.rs`). `own_job_id` is
/// `context`'s `ownJobId` — present on every job turn, `None` on every
/// interactive turn — the one value a job turn DOES carry that identifies
/// its own `agent_jobs` row. #3166 C54: the attachment's destination
/// project is resolved HERE, inside this function, with a 4-tier
/// priority — note's own project (if `note_id` given) → the job's own
/// project (via `own_job_id`/`agent_jobs`, if this is a job turn) →
/// `turn_project_id` → Home — matching Matt's verbatim #3166 rule. This
/// does NOT widen `AgentLoopInputs`/`context_clone`'s shape and does NOT
/// touch `check_cross_project_write_permission`'s guard (#3167 is a
/// separate, later PR for that): the guard already runs before this
/// function for every interactive project-scoped turn and already forces
/// note's-project == turn's-project in that case, so tier 1 taking
/// priority over tier 3 there is a no-op — see the cross-project test
/// below pinning that it stays green, unmutated.
pub(crate) async fn execute_generate_image(
    params: &serde_json::Value,
    db: &crate::database::DbState,
    turn_project_id: Option<&str>,
    own_job_id: Option<&str>,
) -> Result<serde_json::Value, String> {
    execute_generate_image_at(
        params,
        db,
        turn_project_id,
        own_job_id,
        "https://openrouter.ai/api/v1",
        "https://api.venice.ai/api/v1",
    )
    .await
}

/// Same as `execute_generate_image` but with injectable OpenRouter/Venice
/// base URLs — the test seam (same shape as `fetch_anthropic_models_at`/
/// `fetch_openrouter_models_at` in connections.rs): every `generate_image`
/// test below points these at `mockito` servers instead of the real
/// provider hosts, so no test ever calls a real provider endpoint. Venice
/// gets exactly one base URL for both its calls (`/models?type=image` and
/// `/image/generate`) — same single-mock-server shape OpenRouter's own
/// `openrouter_base_url` already uses for its two endpoints.
pub(crate) async fn execute_generate_image_at(
    params: &serde_json::Value,
    db: &crate::database::DbState,
    turn_project_id: Option<&str>,
    own_job_id: Option<&str>,
    openrouter_base_url: &str,
    venice_base_url: &str,
) -> Result<serde_json::Value, String> {
    let prompt = params
        .get("prompt")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "prompt required".to_string())?;
    let note_id_raw = params.get("note_id").and_then(|v| v.as_str());
    let position = params
        .get("position")
        .and_then(|v| v.as_str())
        .unwrap_or("bottom");
    let aspect_ratio = params.get("aspect_ratio").and_then(|v| v.as_str());
    let alt = params
        .get("alt")
        .and_then(|v| v.as_str())
        .map(sanitize_alt)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_alt_from_prompt(prompt));

    // §39.3 step 1 — resolve provider + model from settings. Unset -> error,
    // NO call (§39.6: "Settings unset -> tool error without a network
    // call — assert zero requests on the mock").
    let (provider, model) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        (
            crate::database::get_ai_setting(&conn, IMAGE_PROVIDER_SETTING_KEY),
            crate::database::get_ai_setting(&conn, IMAGE_MODEL_SETTING_KEY),
        )
    };
    let provider = provider
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "No image model chosen — Settings → AI → Image generation".to_string())?;
    let model = model
        .filter(|m| !m.is_empty())
        .ok_or_else(|| "No image model chosen — Settings → AI → Image generation".to_string())?;

    if !IMAGE_CAPABLE_PROVIDER_TYPES.contains(&provider.as_str()) {
        return Err(format!(
            "Unsupported image provider '{}' — only OpenRouter and Venice are available in this release",
            provider
        ));
    }

    // §39.3 step 2 — call the adapter. Provider dispatch (§39.1): each
    // provider reads its OWN keychain-stored key, never a shared one.
    let decoded = match provider.as_str() {
        "venice" => {
            let api_key = {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                crate::ai::credential_migration::read_provider_key(&conn, "venice")
            }
            .ok_or_else(|| {
                "Venice is not connected — Settings → AI → Provider Connections".to_string()
            })?;
            // §39.1: the adapter branches per-model from the model-list
            // metadata — fetched here, at generate time, rather than
            // cached: model metadata changes rarely and this call is free
            // (it's a GET against `/models`, not a billed generation), so
            // there's no staleness/cache-invalidation problem to solve for
            // the cost of one extra round trip before the paid call.
            let models = fetch_venice_image_models_at(&api_key, venice_base_url).await?;
            let model_meta = models
                .into_iter()
                .find(|m| m.id == model)
                .ok_or_else(|| {
                    format!(
                        "Venice model '{}' was not found in the current image-model list — Settings → AI → Image generation",
                        model
                    )
                })?;
            generate_venice_image_at(&api_key, &model, prompt, aspect_ratio, &model_meta, venice_base_url)
                .await?
        }
        _ => {
            // "openrouter" — the only other member of
            // IMAGE_CAPABLE_PROVIDER_TYPES, guaranteed by the membership
            // check above.
            let api_key = {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                crate::ai::credential_migration::read_provider_key(&conn, "openrouter")
            }
            .ok_or_else(|| {
                "OpenRouter is not connected — Settings → AI → Provider Connections".to_string()
            })?;
            generate_openrouter_image_at(&api_key, &model, prompt, aspect_ratio, openrouter_base_url)
                .await?
        }
    };
    let (width, height) = decode_image_dimensions(&decoded.bytes).unzip();

    // §39.3 step 3 — resolve the destination project BEFORE the write:
    // note's own project (if `note_id` given) → the job's own project (via
    // `own_job_id`/`agent_jobs`, if this is a job turn) → the turn's
    // project → Home. #3166 C54 — see the doc comment above this function.
    // Resolved once here and reused below (step 4) rather than re-resolved,
    // so the embed target and the file's actual project can never disagree.
    let resolved_note_id: Option<String> = match note_id_raw {
        Some(raw) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            Some(crate::ai::tools::resolve_note_id(&conn, raw)?)
        }
        None => None,
    };
    let note_destination_project_id: Option<String> = match &resolved_note_id {
        Some(rid) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            Some(
                conn.query_row(
                    "SELECT project_id FROM notes WHERE id = ?1 AND deleted_at IS NULL",
                    rusqlite::params![rid],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Note not found: {}", e))?,
            )
        }
        None => None,
    };
    // Tier 2 only applies when tier 1 (note's project) didn't resolve —
    // `own_job_id` is set on every job turn regardless of whether that
    // turn's `generate_image` call also passed a `note_id`, so without
    // this guard a job-turn call WITH a note_id would ignore the note's
    // own project and use the job's instead — wrong per Matt's rule
    // ("with note_id -> the note's project" comes first, unconditionally).
    let job_destination_project_id: Option<String> = if note_destination_project_id.is_none() {
        own_job_id.and_then(|job_id| {
            let conn = db.0.lock().ok()?;
            let raw_job_pid: String = conn
                .query_row(
                    "SELECT project_id FROM agent_jobs WHERE id = ?1",
                    rusqlite::params![job_id],
                    |row| row.get(0),
                )
                .ok()?;
            // Reed's audit of this PR, 2026-11-04: `agent_jobs.project_id`
            // uses "workspace" as ITS OWN Home sentinel (`create_session_
            // internal`'s "workspace" surface id, scheduler.rs; the exact
            // normalization `memory_router.rs::determine_destination`
            // already applies for the identical reason: "'workspace' is
            // not a real project ID") — a DIFFERENT literal than this
            // module's own `HOME_PROJECT_SENTINEL` ("__global__"). Without
            // this translation, a Home job's image would have landed in a
            // bogus `project_id = "workspace"` legacy attachment leaf
            // (`attachments::attachment_location`'s fallback path, which
            // only redirects the LITERAL `"__global__"` string to Home) —
            // not the job's Home project, and not a refusal either; a
            // silent wrong folder one level deeper than the bug C54 fixes.
            // #3167 Commit 5: moved onto the shared
            // `tools::normalize_home_sentinel` predicate (Reed's #3168
            // delta re-audit named this inline comparison as one of three
            // pre-existing independent copies, risking a fourth) — same
            // logic, no behavior change.
            crate::ai::tools::normalize_home_sentinel(&raw_job_pid).map(|s| s.to_string())
        })
    } else {
        None
    };
    let attachment_project_id: String = note_destination_project_id
        .or(job_destination_project_id)
        .unwrap_or_else(|| turn_project_id.unwrap_or(HOME_PROJECT_SENTINEL).to_string());

    // §39.3 step 3 (cont.) — write through the same persistence
    // `save_attachment` uses (attachments.rs::save_attachment_bytes — split
    // out of the IPC command specifically so this call can skip the base64
    // round trip).
    let filename = format!("generated-image.{}", decoded.extension);
    let mime_type = format!(
        "image/{}",
        if decoded.extension == "jpg" {
            "jpeg"
        } else {
            decoded.extension.as_str()
        }
    );
    let saved = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::attachments::save_attachment_bytes(
            &conn,
            &attachment_project_id,
            &filename,
            &mime_type,
            &decoded.bytes,
        )?
    };
    let attachment_id = saved
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "save_attachment_bytes did not return an id".to_string())?
        .to_string();
    // #3178: `saved["path"]` (from `save_attachment_bytes`/`policy.
    // authorize_write()`) is the real, absolute filesystem path —
    // including the user's home directory — and was previously copied
    // verbatim into the tool result the model sees, which the model then
    // echoes into its reply, from where it flows into persisted chat
    // content and notification bodies (Reed's #3178 trace). Build the
    // SAME project-relative `Assets/<uuid>.<ext>` convention `insert_
    // image_embed`'s own `embed_line` below already uses for note
    // embeds, instead of forwarding the absolute value. `attachment_id`
    // alone is already sufficient for the frontend's thumbnail rendering
    // (#3171 PR2: `load_chat_messages` SELECTs `attachment_ids`
    // unconditionally) — the model doesn't need a resolvable path, only
    // one that doesn't leak the real disk layout.
    let path = format!("Assets/{}.{}", attachment_id, decoded.extension);

    // §39.3 step 4 — embed into the note, if asked. `note_id` is already
    // resolved above (step 3) — reused here, not re-resolved, so this
    // block and the destination-resolution block above can never disagree
    // about which note/project is in play.
    let mut position_note: Option<String> = None;
    if let Some(note_id) = resolved_note_id {
        let embed_line = format!("![{}](Assets/{}.{})", alt, attachment_id, decoded.extension);
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let file_path: String = conn
            .query_row(
                "SELECT file_path FROM notes WHERE id = ?1 AND deleted_at IS NULL",
                rusqlite::params![&note_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Note not found: {}", e))?;
        let current_content = conn
            .query_row(
                "SELECT COALESCE(body, content_preview, '') FROM notes WHERE id = ?1",
                rusqlite::params![&note_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_default();

        let (new_content, note) = insert_image_embed(&current_content, position, &embed_line);
        position_note = note;

        if !current_content.trim().is_empty() {
            if let Err(e) = crate::versions::save_version(&conn, &note_id, &current_content, "ai_generate_image") {
                eprintln!(
                    "[IMAGE] Failed to save pre-embed version for note {}: {}",
                    note_id, e
                );
            }
        }

        if file_path.is_empty() {
            eprintln!(
                "[IMAGE] DB-only note (file_path empty), skipping filesystem write: note_id={}",
                note_id
            );
        } else {
            crate::database::write_note_content(&file_path, &new_content, Some(&conn))?;
        }

        let word_count = new_content.split_whitespace().count() as i64;
        let content_preview = new_content.chars().take(200).collect::<String>();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE notes SET body = ?1, content_preview = ?2, word_count = ?3, updated_at = ?4 WHERE id = ?5",
            rusqlite::params![new_content, content_preview, word_count, now, note_id],
        )
        .map_err(|e| format!("Failed to update note metadata: {}", e))?;
    }

    // §39.3 step 5 — ids and a path, never image bytes; cost omitted
    // (never 0) when the provider didn't report one.
    let mut result = serde_json::json!({
        "attachment_id": attachment_id,
        "path": path,
    });
    if let Some(w) = width {
        result["width"] = serde_json::json!(w);
    }
    if let Some(h) = height {
        result["height"] = serde_json::json!(h);
    }
    if let Some(cost) = decoded.cost_usd {
        result["cost"] = serde_json::json!(cost);
    }
    if let Some(note) = position_note {
        result["position_note"] = serde_json::json!(note);
    }
    // §39.4: "every call logs `[IMAGE] provider=… model=… cost=…` (cost
    // omitted when unreported)" — the clause itself is absent, not printed
    // as `cost=n/a` or `cost=0`.
    match decoded.cost_usd {
        Some(cost) => eprintln!("[IMAGE] provider={} model={} cost={:.4}", provider, model, cost),
        None => eprintln!("[IMAGE] provider={} model={}", provider, model),
    }

    Ok(result)
}

/// Settings -> AI -> Image generation's Test button (§39.2): "generates
/// one small image from a fixed prompt into Home/Assets and shows it
/// inline, same pattern as Web search's Test." Reuses `execute_generate_
/// image` with no `note_id` (Home project, nothing to embed into) — not a
/// second, parallel code path.
#[tauri::command]
pub async fn test_generate_image(
    db: tauri::State<'_, crate::database::DbState>,
) -> Result<serde_json::Value, String> {
    let params = serde_json::json!({
        "prompt": "A small, simple test image: a single blue circle on a white background.",
    });
    execute_generate_image(&params, &db, None, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Pins `HOME_PROJECT_SENTINEL` against the real
    /// `attachments::HOME_ATTACHMENT_PROJECT_SCOPE` constant's value —
    /// the two must never drift apart (that constant is private to
    /// `attachments.rs`, so this is a literal-value pin, not a reference).
    #[test]
    fn home_project_sentinel_matches_attachments_module() {
        assert_eq!(HOME_PROJECT_SENTINEL, "__global__");
    }

    // ── execute_generate_image_at: full integration, mocked HTTP only ───
    //
    // Setup mirrors the established pattern elsewhere in this crate
    // (tools.rs's `stage1_backend_emit_tests::setup`, connections.rs's
    // `KeychainGuard`): a fresh in-memory migrated DB, a TempDir as
    // PANE_TEST_ROOT (so Home/Assets writes land in a sandbox, never a
    // real user directory), and `KeychainGuard` installing a MockKeychain
    // so `read_provider_key` never touches the real macOS keychain.
    // `#[serial(keychain_override)]` because KEYCHAIN_OVERRIDE is a single
    // process-wide static — same discipline every other test file touching
    // it already follows.

    fn setup_test_db() -> (crate::database::DbState, tempfile::TempDir) {
        let fs_dir = tempfile::TempDir::new().expect("scratch fs dir");
        unsafe { std::env::set_var("PANE_TEST_ROOT", fs_dir.path()) };
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        (crate::database::DbState::new(conn), fs_dir)
    }

    /// Note with an empty `file_path` (the §2302 "sentinel scope" shape
    /// `execute_append_to_note`'s own tests already use) — skips the real
    /// filesystem note write entirely, so these tests assert the DB body
    /// column directly without needing a real note file on disk too.
    fn seed_db_only_note(db: &crate::database::DbState, note_id: &str, body: &str) {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, body, project_id, file_path, status, note_type, updated_at) \
             VALUES (?1, 'Test Note', ?2, '', '', 'active', NULL, datetime('now'))",
            rusqlite::params![note_id, body],
        )
        .expect("seed note");
    }

    fn note_body(db: &crate::database::DbState, note_id: &str) -> String {
        let conn = db.0.lock().unwrap();
        conn.query_row(
            "SELECT body FROM notes WHERE id = ?1",
            rusqlite::params![note_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// #3166 C54 — same shape as `seed_db_only_note`, but with a real
    /// `project_id` (the plain Home-scope '' is what hid axis-1 of C54:
    /// nothing ever exercised a note that belonged to an actual project).
    fn seed_db_only_note_in_project(
        db: &crate::database::DbState,
        note_id: &str,
        body: &str,
        project_id: &str,
    ) {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, body, project_id, file_path, status, note_type, updated_at) \
             VALUES (?1, 'Test Note', ?2, ?3, '', 'active', NULL, datetime('now'))",
            rusqlite::params![note_id, body, project_id],
        )
        .expect("seed note");
    }

    /// #3166 C54 — a project row, named so `attachments::attachment_
    /// location`'s `current_project_name` lookup resolves to a real
    /// `Projects/<name>/Assets` path (the same mechanism
    /// `attachments.rs`'s own project-scoped tests already rely on).
    fn seed_project(db: &crate::database::DbState, project_id: &str, name: &str) {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES (?1, ?2, NULL)",
            rusqlite::params![project_id, name],
        )
        .expect("seed project");
    }

    /// #3166 C54 — a minimal `agent_jobs` row, enough for the
    /// `own_job_id` -> `agent_jobs.project_id` lookup. Unlike
    /// `tools.rs`'s own `bypass_id` fixture, this crate's canonical
    /// migrated schema (`migrations::run_migrations`, used by
    /// `setup_test_db` above) DOES enforce `agent_jobs`'s `agent_id`/
    /// `session_id` foreign keys — so a real `ai_agents` row and a real
    /// `chat_sessions` row are seeded first.
    /// #3178: `result["path"]` is now the project-agnostic
    /// `Assets/<uuid>.<ext>` form (no absolute disk path, no project
    /// name) — the C54 destination-priority tests below that used to
    /// read the project name back out of `path` now read the real
    /// `attachments.project_id` column by `attachment_id` instead, the
    /// same column `save_attachment_bytes` actually writes.
    fn attachment_project_id(db: &crate::database::DbState, attachment_id: &str) -> String {
        let conn = db.0.lock().unwrap();
        conn.query_row(
            "SELECT project_id FROM attachments WHERE id = ?1",
            rusqlite::params![attachment_id],
            |row| row.get(0),
        )
        .unwrap_or_default()
    }

    fn seed_job(db: &crate::database::DbState, job_id: &str, project_id: &str) {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path) VALUES ('test-agent', 'Test Agent', 'test-agent', '')",
            [],
        )
        .expect("seed agent");
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, title) VALUES ('test-session', ?1, 'Test Session')",
            rusqlite::params![project_id],
        )
        .expect("seed session");
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, status, created_at, updated_at) \
             VALUES (?1, 'once', 'Test Job', 'brief', 'test-agent', ?2, 'test-session', 'pending', datetime('now'), datetime('now'))",
            rusqlite::params![job_id, project_id],
        )
        .expect("seed job");
    }

    const SUCCESS_BODY: &str =
        r#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/png"}],"usage":{"cost":0.05}}"#;

    /// §39.6: "Settings unset -> tool error without a network call (assert
    /// zero requests on the mock)." Mutation: remove the early-return —
    /// reddens (the mock's `.expect(0)` would fail).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn settings_unset_makes_zero_requests() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .expect(0)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse at dusk" });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url()).await;

        assert!(result.is_err(), "unset image settings must refuse, not call the provider");
        assert!(result.unwrap_err().contains("Settings → AI → Image generation"));
        mock.assert_async().await;
    }

    /// Same as above, but `image_provider` IS set — `image_model` alone
    /// being unset must also refuse before any call (both keys are
    /// independently null-checked, §39.2: "Both null until the user picks").
    #[tokio::test]
    #[serial(keychain_override)]
    async fn settings_model_unset_makes_zero_requests() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        {
            let conn = db.0.lock().unwrap();
            crate::database::set_ai_setting(&conn, IMAGE_PROVIDER_SETTING_KEY, "openrouter").unwrap();
        }
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .expect(0)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse at dusk" });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url()).await;

        assert!(result.is_err());
        mock.assert_async().await;
    }

    fn seed_image_settings(db: &crate::database::DbState) {
        let conn = db.0.lock().unwrap();
        crate::database::set_ai_setting(&conn, IMAGE_PROVIDER_SETTING_KEY, "openrouter").unwrap();
        crate::database::set_ai_setting(&conn, IMAGE_MODEL_SETTING_KEY, "test-model").unwrap();
        drop(conn);
        crate::ai::credential_migration::write_provider_key(
            &db.0.lock().unwrap(),
            "openrouter",
            "test-key",
        )
        .unwrap();
    }

    // ── #3166 C54: destination-priority chain ───────────────────────────
    //
    // note's project -> job's project (via `own_job_id`) -> turn's
    // project -> Home. See the doc comment on `execute_generate_image_at`
    // above for the full citation chain.

    /// Reed's case (a): a Home turn (`turn_project_id = None`) targeting
    /// an Aurum note. Before this fix the file landed in `Home/Assets`
    /// while the (project-agnostic) embed line resolved relative to
    /// Aurum -> broken "?" image. Mutation: revert the destination
    /// resolution to `turn_project_id.unwrap_or(HOME_PROJECT_SENTINEL)`
    /// alone (drop the note-project lookup) — reddens: the saved
    /// attachment's `project_id` would be `HOME_PROJECT_SENTINEL`
    /// instead of "aurum-id". (#3178: was asserted via `path` containing
    /// "Aurum"/"Home" — `path` is now the project-agnostic `Assets/
    /// <uuid>.<ext>` form, so this reads the real `attachments.
    /// project_id` column instead.)
    #[tokio::test]
    #[serial(keychain_override)]
    async fn note_project_wins_over_home_turn() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_project(&db, "aurum-id", "Aurum");
        seed_db_only_note_in_project(&db, "note-aurum", "# Aurum note", "aurum-id");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse", "note_id": "note-aurum" });
        // turn_project_id = None (Home turn), own_job_id = None (interactive).
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("settings seeded, mock succeeds");
        let attachment_id = result.get("attachment_id").and_then(|v| v.as_str()).unwrap_or_default();
        let project_id = attachment_project_id(&db, attachment_id);
        assert_eq!(
            project_id, "aurum-id",
            "C54(a): expected the note's own project (Aurum) to win, got project_id={:?}",
            project_id
        );
    }

    /// Reed's case (b), the part axis-2 owns alone: a job turn
    /// (`own_job_id` present, same as every job turn's real
    /// `context_clone`) with NO `note_id` at all (a standalone generated
    /// image, nothing to embed) must still land in the JOB's project, not
    /// Home — even though `turn_project_id` is `None` on every job turn
    /// by the #3050 PR 4 design `image_gen.rs`'s doc comment cites.
    /// Mutation: drop the `own_job_id` -> `agent_jobs` lookup (fall
    /// straight to `turn_project_id.unwrap_or(HOME_PROJECT_SENTINEL)`) —
    /// reddens: the saved attachment's `project_id` would be
    /// `HOME_PROJECT_SENTINEL` instead of "aurum-id". (#3178: `path` no
    /// longer carries the project name — see `attachment_project_id`.)
    #[tokio::test]
    #[serial(keychain_override)]
    async fn job_turn_without_note_id_uses_jobs_own_project() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_project(&db, "aurum-id", "Aurum");
        seed_job(&db, "job-1", "aurum-id");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse" });
        // turn_project_id = None (job turns never carry one, by design),
        // own_job_id = Some("job-1") (every job turn's real context carries
        // this).
        let result = execute_generate_image_at(&params, &db, None, Some("job-1"), &server.url(), &server.url())
            .await
            .expect("settings seeded, mock succeeds");
        let attachment_id = result.get("attachment_id").and_then(|v| v.as_str()).unwrap_or_default();
        let project_id = attachment_project_id(&db, attachment_id);
        assert_eq!(
            project_id, "aurum-id",
            "C54(b): a job's own project must win when note_id is absent, got project_id={:?}",
            project_id
        );
    }

    /// #3166 C54 correction (Reed's audit, 2026-11-04): `agent_jobs.
    /// project_id` for a Home job is the literal string "workspace"
    /// (`create_session_internal`'s own Home surface id, scheduler.rs) —
    /// a DIFFERENT sentinel than this module's `HOME_PROJECT_SENTINEL`
    /// ("__global__"). A job turn with no `note_id` whose OWN job is a
    /// Home job must still land in `Home/Assets`, not a bogus
    /// `project_id = "workspace"` legacy attachment leaf. Mutation: drop
    /// the `raw_job_pid == "workspace"` normalization — reddens: the
    /// saved attachment's `project_id` would be the bogus literal
    /// "workspace" instead of `HOME_PROJECT_SENTINEL` ("__global__").
    /// (#3178: `path` no longer carries the project leaf — see
    /// `attachment_project_id`.)
    #[tokio::test]
    #[serial(keychain_override)]
    async fn job_turn_without_note_id_whose_job_is_a_home_job_lands_in_home() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        // Home job: agent_jobs.project_id = "workspace" (the real Home
        // sentinel scheduler.rs/create_session_internal uses), NOT this
        // module's own "__global__".
        seed_job(&db, "job-home", "workspace");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse" });
        let result = execute_generate_image_at(&params, &db, None, Some("job-home"), &server.url(), &server.url())
            .await
            .expect("settings seeded, mock succeeds");
        let attachment_id = result.get("attachment_id").and_then(|v| v.as_str()).unwrap_or_default();
        let project_id = attachment_project_id(&db, attachment_id);
        assert_eq!(
            project_id, HOME_PROJECT_SENTINEL,
            "a Home job's standalone image must land in Home, got project_id={:?}",
            project_id
        );
    }

    /// #3166 C54 acceptance test — Matt's own 4d repro, end to end: "a job
    /// in Aurum writing an image that lands in Aurum/Assets and renders."
    /// This is Reed's case (b) in full: a job turn (`own_job_id` set,
    /// `turn_project_id` None) ALSO writing into an Aurum note
    /// (`note_id` set) — note's-project (tier 1) and job's-project
    /// (tier 2) agree here (both Aurum), matching Matt's literal repro
    /// ("job, session and note all agree"), but the OLD code ignored
    /// both and used Home regardless. Mutation: either tier dropped
    /// reddens the same way the two tests above do.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn job_in_aurum_writing_image_into_aurum_note_lands_in_aurum_and_renders() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_project(&db, "aurum-id", "Aurum");
        seed_job(&db, "job-1", "aurum-id");
        seed_db_only_note_in_project(&db, "note-aurum", "# Aurum note\n\nExisting content.", "aurum-id");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({
            "prompt": "a lighthouse",
            "note_id": "note-aurum",
            "position": "top",
        });
        let result = execute_generate_image_at(&params, &db, None, Some("job-1"), &server.url(), &server.url())
            .await
            .expect("settings seeded, mock succeeds");

        let attachment_id = result.get("attachment_id").and_then(|v| v.as_str()).unwrap_or_default();
        let project_id = attachment_project_id(&db, attachment_id);
        assert_eq!(
            project_id, "aurum-id",
            "4d acceptance: lands in Aurum, got project_id={:?}",
            project_id
        );

        let body = note_body(&db, "note-aurum");
        assert!(
            body.contains(&format!("Assets/{}", attachment_id)),
            "4d acceptance: note must render the embed (same project as the file, no broken '?'), body: {:?}",
            body
        );
    }

    /// #3166 C54 — the ONLY shape of call that reaches the adapter for an
    /// interactive project-scoped turn today: `check_cross_project_write_
    /// permission`'s own `generate_image` arm (tools.rs) already forces
    /// note's-project == session's-project BEFORE this function ever runs
    /// (refused otherwise) — so tier 1 (note's project) taking priority
    /// over tier 3 (turn's project) here is a no-op in that case: both
    /// values already agree. Proves the reorder doesn't change behavior
    /// for the one case the pre-existing guard allows through. The
    /// guard's OWN refusal test
    /// (`tools::tests::t3161_01_generate_image_cross_project_note_id_
    /// rejected`) is untouched by this commit and still passes — confirmed
    /// by running it, not by asserting it here (it lives in a different
    /// module and needs no DB-backed adapter call).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn interactive_turn_same_project_note_resolves_unchanged() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_project(&db, "aurum-id", "Aurum");
        seed_db_only_note_in_project(&db, "note-aurum", "# Aurum note", "aurum-id");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse", "note_id": "note-aurum" });
        // turn_project_id = Some("aurum-id") — the session's own project,
        // same as the guard already required to let this call through.
        let result = execute_generate_image_at(&params, &db, Some("aurum-id"), None, &server.url(), &server.url())
            .await
            .expect("settings seeded, mock succeeds");
        let attachment_id = result.get("attachment_id").and_then(|v| v.as_str()).unwrap_or_default();
        let project_id = attachment_project_id(&db, attachment_id);
        assert_eq!(
            project_id, "aurum-id",
            "same-project interactive call must still resolve to Aurum, got project_id={:?}",
            project_id
        );
    }

    /// #3178 item 1: `execute_generate_image_at`'s returned `path` must
    /// be the project-agnostic `Assets/<attachment_id>.<ext>` form, never
    /// the real absolute disk path `save_attachment_bytes` returns in
    /// `saved["path"]` (which, pre-fix, contained `/Users/<name>/Pane/
    /// ...` — the real filesystem layout, leaked straight into the tool
    /// result the model sees). **Matt's named mutation:** revert to
    /// `saved["path"]` → this assertion reddens with an absolute path
    /// (and a `/Users/`-containing value) in the field.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn execute_generate_image_at_returns_relative_path_never_absolute() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse at dusk" });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("settings seeded, mock succeeds");

        let attachment_id = result.get("attachment_id").and_then(|v| v.as_str()).unwrap_or_default();
        let path = result.get("path").and_then(|v| v.as_str()).unwrap_or_default();
        assert_eq!(
            path,
            format!("Assets/{}.png", attachment_id),
            "path must be exactly the relative Assets/<id>.<ext> form, got {:?}",
            path
        );
        assert!(
            !path.contains("/Users/"),
            "path must never contain an absolute /Users/ segment, got {:?}",
            path
        );
        assert!(
            !path.starts_with('/'),
            "path must be relative (not filesystem-rooted), got {:?}",
            path
        );
    }

    /// §39.6 / §39.7: Astro "top of Blimpy" case.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn position_top_inserts_exactly_one_embed_line_at_the_top() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_db_only_note(&db, "note-blimpy", "# Blimpy\n\nExisting content.");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({
            "prompt": "a lighthouse at dusk",
            "note_id": "note-blimpy",
            "position": "top",
            "aspect_ratio": "16:9"
        });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("should succeed");

        let body = note_body(&db, "note-blimpy");
        let attachment_id = result["attachment_id"].as_str().unwrap();
        let expected_line = format!("![a lighthouse at dusk](Assets/{}.png)", attachment_id);
        assert_eq!(body, format!("{}\n\n# Blimpy\n\nExisting content.", expected_line));
        assert_eq!(body.matches(&expected_line).count(), 1);
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn position_bottom_is_the_default() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_db_only_note(&db, "note-1", "Some content.");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "p", "note_id": "note-1" });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("should succeed");

        let body = note_body(&db, "note-1");
        let attachment_id = result["attachment_id"].as_str().unwrap();
        assert_eq!(body, format!("Some content.\n\n![p](Assets/{}.png)", attachment_id));
    }

    /// §39.6: "after: with two matches uses the first and says so" — full
    /// integration, not just the pure `insert_image_embed` unit above.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn position_after_two_matches_result_says_which_one() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_db_only_note(
            &db,
            "note-2",
            "## Summary\nFirst.\n\n## Summary\nSecond.",
        );

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({
            "prompt": "p",
            "note_id": "note-2",
            "position": "after:## Summary"
        });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("should succeed");

        let position_note = result["position_note"].as_str().expect("must explain the two-match case");
        assert!(position_note.contains("2 times"));
        assert!(position_note.contains("first"));

        let body = note_body(&db, "note-2");
        let attachment_id = result["attachment_id"].as_str().unwrap();
        assert_eq!(
            body,
            format!(
                "## Summary\n\n![p](Assets/{}.png)\nFirst.\n\n## Summary\nSecond.",
                attachment_id
            )
        );
    }

    /// §39.6: "after: miss falls back to bottom and says so" — full
    /// integration.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn position_after_miss_falls_back_to_bottom_and_result_says_so() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_db_only_note(&db, "note-3", "# Blimpy\n\nSome text.");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({
            "prompt": "p",
            "note_id": "note-3",
            "position": "after:## Nonexistent Heading"
        });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("should succeed");

        let position_note = result["position_note"].as_str().expect("must explain the fallback");
        assert!(position_note.contains("no match"));
        assert!(position_note.contains("bottom"));

        let body = note_body(&db, "note-3");
        let attachment_id = result["attachment_id"].as_str().unwrap();
        assert_eq!(
            body,
            format!("# Blimpy\n\nSome text.\n\n![p](Assets/{}.png)", attachment_id)
        );
    }

    /// §39.3/§39.4: "cost is present only when the provider reports it; a
    /// missing cost is omitted, never printed as 0." Mutation: serialize
    /// `cost: 0` instead of omitting the key — reddens.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn result_omits_cost_key_when_provider_does_not_report_one() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(r#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/png"}]}"#)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "no cost reported" });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("should succeed");

        assert!(
            !result.as_object().unwrap().contains_key("cost"),
            "the cost key must be entirely absent, not present as 0 — got: {:?}",
            result
        );
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn result_includes_cost_when_provider_reports_one() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "p" });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("should succeed");

        assert_eq!(result["cost"].as_f64(), Some(0.05));
    }

    /// §4c (Reed's audit): alt text containing `]`/`)` must still round-
    /// trip as a parseable `![alt](Assets/uuid.ext)` line. Mutation: skip
    /// sanitization — reddens (the line would contain an unescaped `)`
    /// that truncates the markdown src on the next parse).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn alt_text_with_bracket_and_paren_is_sanitized_and_round_trips() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_image_settings(&db);
        seed_db_only_note(&db, "note-alt", "Body.");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({
            "prompt": "p",
            "note_id": "note-alt",
            "alt": "a sign that says [OPEN] (at dusk)"
        });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("should succeed");
        let attachment_id = result["attachment_id"].as_str().unwrap();

        let body = note_body(&db, "note-alt");
        let expected_line = format!(
            "![a sign that says [OPEN (at dusk](Assets/{}.png)",
            attachment_id
        );
        assert_eq!(body, format!("Body.\n\n{}", expected_line));

        // Round-trip: the SAME regex Editor.svelte's processInline uses to
        // parse `![alt](src)` back out must match this exact line with the
        // src group ending at the FIRST `)` and nothing truncated early.
        let re = regex_lite_image_match(&body).expect("must match the image markdown shape");
        assert_eq!(re, format!("Assets/{}.png", attachment_id));
    }

    /// Minimal stand-in for `Editor.svelte::processInline`'s
    /// `!\[([^\]]*)\]\(([^)]+)\)` regex, applied to the src group only —
    /// confirms the written line is parseable the same way the real
    /// frontend parser would read it, without pulling a JS/regex engine
    /// into the Rust test. Returns the src capture for the first match.
    fn regex_lite_image_match(body: &str) -> Option<String> {
        let open = body.find("![")?;
        let alt_start = open + 2;
        // Real regex's alt group is `[^\]]*` — stops at the FIRST `]`.
        let bracket_close = body[alt_start..].find(']')? + alt_start;
        // `(` must immediately follow `]` for the regex to match at all.
        if body.as_bytes().get(bracket_close + 1) != Some(&b'(') {
            return None;
        }
        let src_start = bracket_close + 2;
        // Real regex's src group is `[^)]+` — stops at the FIRST `)`.
        let paren_close = body[src_start..].find(')')? + src_start;
        Some(body[src_start..paren_close].to_string())
    }

    // ── media_type_to_extension ─────────────────────────────────────────

    #[test]
    fn media_type_maps_known_types() {
        assert_eq!(media_type_to_extension(Some("image/png")), "png");
        assert_eq!(media_type_to_extension(Some("image/jpeg")), "jpg");
        assert_eq!(media_type_to_extension(Some("image/jpg")), "jpg");
        assert_eq!(media_type_to_extension(Some("image/webp")), "webp");
        assert_eq!(media_type_to_extension(Some("image/gif")), "gif");
    }

    #[test]
    fn media_type_falls_back_to_png_when_undeterminable() {
        assert_eq!(media_type_to_extension(None), "png");
        assert_eq!(media_type_to_extension(Some("application/octet-stream")), "png");
    }

    // ── sanitize_alt / default_alt_from_prompt ──────────────────────────

    #[test]
    fn sanitize_alt_strips_bracket_paren_and_newline() {
        assert_eq!(sanitize_alt("a sign that says [OPEN]"), "a sign that says [OPEN");
        assert_eq!(sanitize_alt("a lighthouse (at dusk)"), "a lighthouse (at dusk");
        assert_eq!(sanitize_alt("line one\nline two"), "line oneline two");
    }

    #[test]
    fn default_alt_truncates_to_80_chars_after_sanitizing() {
        let prompt = "x".repeat(100);
        let alt = default_alt_from_prompt(&prompt);
        assert_eq!(alt.chars().count(), 80);
    }

    // ── insert_image_embed: top/bottom/after: ───────────────────────────

    #[test]
    fn position_top_prepends() {
        let (content, note) = insert_image_embed("# Blimpy\n\nSome text.", "top", "![a](Assets/x.png)");
        assert_eq!(content, "![a](Assets/x.png)\n\n# Blimpy\n\nSome text.");
        assert!(note.is_none());
    }

    #[test]
    fn position_top_on_empty_note() {
        let (content, note) = insert_image_embed("", "top", "![a](Assets/x.png)");
        assert_eq!(content, "![a](Assets/x.png)");
        assert!(note.is_none());
    }

    #[test]
    fn position_bottom_appends() {
        let (content, note) = insert_image_embed("# Blimpy\n\nSome text.", "bottom", "![a](Assets/x.png)");
        assert_eq!(content, "# Blimpy\n\nSome text.\n\n![a](Assets/x.png)");
        assert!(note.is_none());
    }

    #[test]
    fn position_after_single_match() {
        let (content, note) = insert_image_embed(
            "# Blimpy\n\n## Intro\n\nSome text.\n\n## Outro\n\nEnd.",
            "after:## Intro",
            "![a](Assets/x.png)",
        );
        assert_eq!(
            content,
            "# Blimpy\n\n## Intro\n\n![a](Assets/x.png)\n\nSome text.\n\n## Outro\n\nEnd."
        );
        assert!(note.is_none());
    }

    /// §39.6: "after: with two matches uses the first and says so."
    #[test]
    fn position_after_two_matches_uses_first_and_says_so() {
        let (content, note) = insert_image_embed(
            "## Summary\nFirst section.\n\n## Summary\nSecond section.",
            "after:## Summary",
            "![a](Assets/x.png)",
        );
        assert_eq!(
            content,
            "## Summary\n\n![a](Assets/x.png)\nFirst section.\n\n## Summary\nSecond section."
        );
        let note = note.expect("two matches must produce a note explaining which one was used");
        assert!(note.contains("2 times"));
        assert!(note.contains("first"));
    }

    /// §39.6: "after: miss falls back to bottom and says so."
    #[test]
    fn position_after_miss_falls_back_to_bottom_and_says_so() {
        let (content, note) = insert_image_embed(
            "# Blimpy\n\nSome text.",
            "after:## Nonexistent Heading",
            "![a](Assets/x.png)",
        );
        assert_eq!(content, "# Blimpy\n\nSome text.\n\n![a](Assets/x.png)");
        let note = note.expect("a miss must produce a note explaining the fallback");
        assert!(note.contains("no match"));
        assert!(note.contains("bottom"));
    }

    /// Mutation target (§39.6): "drop the fallback — reddens." Proven here
    /// directly: an `after:` miss must NOT silently produce the embed with
    /// no explanatory note, and must NOT fail to insert at all.
    #[test]
    fn position_after_miss_still_inserts_exactly_one_embed_line() {
        let (content, _note) = insert_image_embed("Body.", "after:nope", "![a](Assets/x.png)");
        assert_eq!(content.matches("![a](Assets/x.png)").count(), 1);
    }

    #[test]
    fn position_unrecognized_falls_back_to_bottom_silently() {
        let (content, note) = insert_image_embed("Body.", "sideways", "![a](Assets/x.png)");
        assert_eq!(content, "Body.\n\n![a](Assets/x.png)");
        assert!(note.is_none());
    }

    // ── relativize_absolute_asset_paths (#3178) ──────────────────────────

    #[test]
    fn relativize_absolute_asset_paths_strips_absolute_prefix() {
        let uuid = "44d12345-1234-5678-9abc-1234567890ab";
        let body = format!(
            "![a lighthouse](/Users/example/Pane/Projects/Aurum/Assets/{}.png)",
            uuid
        );
        let (new_body, count) = relativize_absolute_asset_paths(&body);
        assert_eq!(count, 1);
        assert_eq!(new_body, format!("![a lighthouse](Assets/{}.png)", uuid));
        assert!(!new_body.contains("/Users/"));
    }

    #[test]
    fn relativize_absolute_asset_paths_handles_home_root_too() {
        let uuid = "44d12345-1234-5678-9abc-1234567890ab";
        let body = format!("![a cat](/Users/example/Pane/Assets/{}.jpg)", uuid);
        let (new_body, count) = relativize_absolute_asset_paths(&body);
        assert_eq!(count, 1);
        assert_eq!(new_body, format!("![a cat](Assets/{}.jpg)", uuid));
    }

    #[test]
    fn relativize_absolute_asset_paths_leaves_already_relative_links_untouched() {
        let uuid = "44d12345-1234-5678-9abc-1234567890ab";
        let body = format!("![a lighthouse](Assets/{}.png)", uuid);
        let (new_body, count) = relativize_absolute_asset_paths(&body);
        assert_eq!(count, 0, "an already-relative link must be a no-op (idempotent)");
        assert_eq!(new_body, body);
    }

    #[test]
    fn relativize_absolute_asset_paths_leaves_unrelated_text_untouched() {
        let body = "Just some plain text, no images here.";
        let (new_body, count) = relativize_absolute_asset_paths(body);
        assert_eq!(count, 0);
        assert_eq!(new_body, body);
    }

    // ── decode_image_dimensions ──────────────────────────────────────────

    #[test]
    fn png_dimensions_read_from_ihdr_chunk() {
        // 8-byte signature + 4-byte length (unused) + "IHDR" + width=200 + height=100 + rest ignored.
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0, 0, 0, 13]); // chunk length (unused by our reader)
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&200u32.to_be_bytes());
        bytes.extend_from_slice(&100u32.to_be_bytes());
        assert_eq!(decode_image_dimensions(&bytes), Some((200, 100)));
    }

    #[test]
    fn jpeg_dimensions_read_from_sof0_marker() {
        // SOI, then an APP0 segment to skip over, then SOF0 with height=50 width=75.
        let mut bytes = vec![0xFF, 0xD8]; // SOI
        bytes.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x04, 0xAA, 0xBB]); // APP0, len=4, 2 payload bytes
        bytes.extend_from_slice(&[0xFF, 0xC0]); // SOF0 marker
        bytes.extend_from_slice(&[0x00, 0x0B]); // segment length (unused by our reader)
        bytes.push(0x08); // precision
        bytes.extend_from_slice(&50u16.to_be_bytes()); // height
        bytes.extend_from_slice(&75u16.to_be_bytes()); // width
        assert_eq!(decode_image_dimensions(&bytes), Some((75, 50)));
    }

    #[test]
    fn unrecognized_bytes_return_none_rather_than_guessing() {
        assert_eq!(decode_image_dimensions(b"not an image"), None);
    }

    // ── fetch_openrouter_image_models_at: request shape + decode ───────

    /// §39.6: "Adapter tests per provider with a mocked HTTP server: ...
    /// `media_type` -> extension." Covers the model-list fetch's own
    /// decode: a model WITH a reported `pricing` line surfaces
    /// `price_usd`; one without reports `None` (never `Some(0.0)`).
    #[tokio::test]
    async fn fetch_openrouter_image_models_request_shape_and_price_parsing() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/images/models")
            .match_header("authorization", "Bearer test-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":[{"id":"openai/gpt-image-2","name":"GPT Image 2","pricing":[{"billable":"output_image","unit":"image","cost_usd":0.13}]},{"id":"bytedance-seed/seedream-4.5","name":"Seedream 4.5"}]}"#, // CI-ALLOW: hardcoded-model
            )
            .create_async()
            .await;

        let models = fetch_openrouter_image_models_at("test-key", &server.url())
            .await
            .expect("fetch should succeed against the mocked endpoint");

        mock.assert_async().await;
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "openai/gpt-image-2"); // CI-ALLOW: hardcoded-model
        assert_eq!(models[0].price_usd, Some(0.13));
        assert_eq!(models[1].id, "bytedance-seed/seedream-4.5");
        assert_eq!(
            models[1].price_usd, None,
            "a model the provider reports no price for must be None, never Some(0.0)"
        );
    }

    // ── generate_openrouter_image_at: request shape, decode, retry ─────

    #[tokio::test]
    async fn adapter_request_shape_includes_model_prompt_and_aspect_ratio() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/images")
            .match_header("authorization", "Bearer test-key")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "model": "openai/gpt-image-2", // CI-ALLOW: hardcoded-model
                "prompt": "a lighthouse at dusk",
                "aspect_ratio": "16:9"
            })))
            .with_status(200)
            .with_body(r#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/png"}]}"#)
            .create_async()
            .await;

        let decoded = generate_openrouter_image_at(
            "test-key",
            "openai/gpt-image-2", // CI-ALLOW: hardcoded-model
            "a lighthouse at dusk",
            Some("16:9"),
            &server.url(),
        )
        .await
        .expect("should succeed");

        mock.assert_async().await;
        assert_eq!(decoded.bytes, b"hello");
        assert_eq!(decoded.extension, "png");
    }

    /// Mutation target (§39.6): "swap the decode path — reddens."
    #[tokio::test]
    async fn adapter_decodes_b64_json_as_base64_not_raw_bytes() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(r#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/png"}]}"#)
            .create_async()
            .await;

        let decoded = generate_openrouter_image_at(
            "test-key",
            "openai/gpt-image-2", // CI-ALLOW: hardcoded-model
            "a prompt",
            None,
            &server.url(),
        )
        .await
        .expect("should succeed");

        // "aGVsbG8=" base64-decodes to "hello" — if the decode path were
        // swapped for a raw-bytes passthrough, this would be the literal
        // 8-byte base64 string instead of the 5-byte decoded payload.
        assert_eq!(decoded.bytes, b"hello");
        assert_ne!(decoded.bytes, b"aGVsbG8=".to_vec());
    }

    #[tokio::test]
    async fn adapter_usage_cost_is_surfaced_when_present() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(r#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/png"}],"usage":{"cost":0.07}}"#)
            .create_async()
            .await;

        let decoded = generate_openrouter_image_at("k", "m", "p", None, &server.url())
            .await
            .expect("should succeed");
        assert_eq!(decoded.cost_usd, Some(0.07));
    }

    /// §39.3/§39.4: "cost is present only when the provider reports it; a
    /// missing cost is omitted, never printed as 0."
    #[tokio::test]
    async fn adapter_cost_is_none_not_zero_when_unreported() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(r#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/png"}]}"#)
            .create_async()
            .await;

        let decoded = generate_openrouter_image_at("k", "m", "p", None, &server.url())
            .await
            .expect("should succeed");
        assert_eq!(decoded.cost_usd, None);
    }

    #[tokio::test]
    async fn adapter_media_type_maps_jpeg_to_jpg_extension() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(r#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/jpeg"}]}"#)
            .create_async()
            .await;

        let decoded = generate_openrouter_image_at("k", "m", "p", None, &server.url())
            .await
            .expect("should succeed");
        assert_eq!(decoded.extension, "jpg");
    }

    /// §39.6: "4xx no-retry." Mutation: remove the 4xx short-circuit —
    /// reddens (exactly 1 request would become 2).
    #[tokio::test]
    async fn adapter_4xx_makes_exactly_one_request_no_retry() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/images")
            .with_status(400)
            .with_body(r#"{"error":"bad request"}"#)
            .expect(1)
            .create_async()
            .await;

        let result = generate_openrouter_image_at("k", "m", "p", None, &server.url()).await;
        assert!(result.is_err(), "a 4xx must surface as an error");
        mock.assert_async().await;
    }

    /// §39.6: "5xx one-retry." Mutation: remove the retry — reddens
    /// (the mock's `.expect(2)` would fail after only 1 request).
    #[tokio::test]
    async fn adapter_5xx_retries_exactly_once_then_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/images")
            .with_status(500)
            .with_body("internal error")
            .expect(1)
            .create_async()
            .await;
        let success_mock = server
            .mock("POST", "/images")
            .with_status(200)
            .with_body(r#"{"data":[{"b64_json":"aGVsbG8=","media_type":"image/png"}]}"#)
            .expect(1)
            .create_async()
            .await;

        let result = generate_openrouter_image_at("k", "m", "p", None, &server.url()).await;

        // mockito serves mocks most-recently-created-first by default when
        // multiple match the same route — explicit per-mock `.expect(1)`
        // plus the final `.assert_async()` on each proves both the retry
        // happened AND the retry's result is what the caller sees.
        assert!(
            result.is_ok(),
            "a 5xx followed by a 200 on the retry must succeed overall: {:?}",
            result.err()
        );
        success_mock.assert_async().await;
        mock.assert_async().await;
    }

    /// §39.6/§39.1: a 5xx that never recovers surfaces as an error after
    /// exactly one retry (two total attempts), not an infinite retry loop.
    #[tokio::test]
    async fn adapter_5xx_twice_errors_after_exactly_two_attempts() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/images")
            .with_status(500)
            .with_body("internal error")
            .expect(2)
            .create_async()
            .await;

        let result = generate_openrouter_image_at("k", "m", "p", None, &server.url()).await;
        assert!(result.is_err());
        mock.assert_async().await;
    }

    // ── #3161 PR 2: Venice — sizing classification, pure ────────────────

    fn ar_only_constraints() -> VeniceImageConstraintsWire {
        VeniceImageConstraintsWire {
            aspect_ratios: Some(vec!["1:1".to_string(), "16:9".to_string()]),
            resolutions: None,
            default_resolution: None,
            width_height_divisor: Some(8.0),
        }
    }

    fn ar_and_resolution_constraints() -> VeniceImageConstraintsWire {
        VeniceImageConstraintsWire {
            aspect_ratios: Some(vec!["1:1".to_string(), "16:9".to_string()]),
            resolutions: Some(vec!["1K".to_string(), "2K".to_string(), "4K".to_string()]),
            default_resolution: Some("1K".to_string()),
            width_height_divisor: Some(8.0),
        }
    }

    fn pixel_only_constraints() -> VeniceImageConstraintsWire {
        VeniceImageConstraintsWire {
            aspect_ratios: None,
            resolutions: None,
            default_resolution: None,
            width_height_divisor: Some(8.0),
        }
    }

    /// §39.1: no `aspectRatios` at all → Pixel. Mutation (§39.6's own
    /// wording, applied to the classifier directly): "ignore constraints,
    /// assume pixel for every model" would ALSO pass this one case by
    /// accident — the next two tests are what catch that mutation.
    #[test]
    fn classify_venice_sizing_no_aspect_ratios_is_pixel() {
        assert_eq!(
            classify_venice_sizing(Some(&pixel_only_constraints())),
            VeniceImageSizing::Pixel
        );
        assert_eq!(classify_venice_sizing(None), VeniceImageSizing::Pixel);
    }

    /// Mutation target (§39.6, adapted to the model-list parse step):
    /// "ignore constraints, assume pixel for every model" — reddens here,
    /// since a model that genuinely reports `aspectRatios` with no
    /// `resolutions` must classify as AspectRatio, not Pixel.
    #[test]
    fn classify_venice_sizing_aspect_ratios_no_resolutions_is_aspect_ratio_only() {
        assert_eq!(
            classify_venice_sizing(Some(&ar_only_constraints())),
            VeniceImageSizing::AspectRatio
        );
    }

    #[test]
    fn classify_venice_sizing_aspect_ratios_and_resolutions_is_both() {
        assert_eq!(
            classify_venice_sizing(Some(&ar_and_resolution_constraints())),
            VeniceImageSizing::AspectRatioWithResolution
        );
    }

    // ── aspect_ratio_to_pixels ───────────────────────────────────────────

    #[test]
    fn aspect_ratio_to_pixels_square_is_baseline_both_sides() {
        let (w, h) = aspect_ratio_to_pixels("1:1", 8).expect("1:1 must parse");
        assert_eq!(w, 1024);
        assert_eq!(h, 1024);
    }

    #[test]
    fn aspect_ratio_to_pixels_16_9_preserves_ratio_and_snaps_to_divisor() {
        let (w, h) = aspect_ratio_to_pixels("16:9", 8).expect("16:9 must parse");
        assert_eq!(w, 1024, "the wider side stays at the baseline");
        assert_eq!(h % 8, 0, "must be divisible by the model's widthHeightDivisor");
        // 1024 * 9 / 16 = 576 exactly, already divisible by 8 — asserting
        // the real computed value, not just "divisible," so a ratio-math
        // regression (e.g. swapping w/h) would be caught.
        assert_eq!(h, 576);
    }

    #[test]
    fn aspect_ratio_to_pixels_9_16_swaps_which_side_is_baseline() {
        let (w, h) = aspect_ratio_to_pixels("9:16", 8).expect("9:16 must parse");
        assert_eq!(h, 1024);
        assert_eq!(w, 576);
    }

    #[test]
    fn aspect_ratio_to_pixels_never_exceeds_1280_max() {
        // A divisor of 1 with a 1:1 ratio would otherwise stay at exactly
        // the 1024 baseline — this just pins the ceiling exists at all for
        // any future baseline change.
        let (w, h) = aspect_ratio_to_pixels("1:1", 1).expect("must parse");
        assert!(w <= 1280 && h <= 1280);
    }

    #[test]
    fn aspect_ratio_to_pixels_rejects_malformed_ratio() {
        assert_eq!(aspect_ratio_to_pixels("not-a-ratio", 8), None);
        assert_eq!(aspect_ratio_to_pixels("1:0", 8), None);
        assert_eq!(aspect_ratio_to_pixels("0:1", 8), None);
        assert_eq!(aspect_ratio_to_pixels("-1:2", 8), None);
    }

    // ── max_images_per_turn_setting ──────────────────────────────────────

    #[test]
    fn max_images_per_turn_defaults_to_4_when_unset() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::migrations::run_migrations(&conn).unwrap();
        assert_eq!(max_images_per_turn_setting(&conn), 4);
    }

    #[test]
    fn max_images_per_turn_reads_stored_value() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::migrations::run_migrations(&conn).unwrap();
        crate::database::set_ai_setting(&conn, MAX_IMAGES_PER_TURN_SETTING_KEY, "10").unwrap();
        assert_eq!(max_images_per_turn_setting(&conn), 10);
    }

    #[test]
    fn max_images_per_turn_falls_back_to_default_on_garbage() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::migrations::run_migrations(&conn).unwrap();
        crate::database::set_ai_setting(&conn, MAX_IMAGES_PER_TURN_SETTING_KEY, "not-a-number").unwrap();
        assert_eq!(max_images_per_turn_setting(&conn), 4);
    }

    // ── fetch_venice_image_models_at: request shape + sizing + price ───

    const VENICE_MODELS_BODY: &str = r#"{"data":[
        {"id":"test-pixel-model","name":"Test Pixel Model","model_spec":{
            "constraints":{"promptCharacterLimit":1500,"steps":{"default":20,"max":50},"widthHeightDivisor":8},
            "pricing":{"generation":{"usd":0.01,"diem":0.01}}
        }},
        {"id":"test-ar-model","name":"Test AR Model","model_spec":{
            "constraints":{"aspectRatios":["1:1","16:9"],"defaultAspectRatio":"1:1","promptCharacterLimit":2000,"steps":{"default":8,"max":8},"widthHeightDivisor":8},
            "pricing":{"generation":{"usd":0.05,"diem":0.05}}
        }},
        {"id":"test-res-model","name":"Test Res Model","model_spec":{
            "constraints":{"aspectRatios":["1:1","16:9"],"defaultAspectRatio":"1:1","resolutions":["1K","2K","4K"],"defaultResolution":"1K","promptCharacterLimit":2048,"steps":{"default":25,"max":50},"widthHeightDivisor":8},
            "pricing":{"resolutions":{"1K":{"usd":0.27,"diem":0.27},"2K":{"usd":0.51,"diem":0.51},"4K":{"usd":0.84,"diem":0.84}}}
        }}
    ]}"#; // CI-ALLOW: hardcoded-model

    /// Mutation (§39.6's wording adapted to this parse step): "ignore
    /// `constraints`, assume pixel for every model" — reddens, since the
    /// AR and resolution-tier models would misclassify as `Pixel`.
    #[tokio::test]
    async fn fetch_venice_image_models_request_shape_and_sizing_classification() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::UrlEncoded("type".into(), "image".into()))
            .match_header("authorization", "Bearer test-key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(VENICE_MODELS_BODY)
            .create_async()
            .await;

        let models = fetch_venice_image_models_at("test-key", &server.url())
            .await
            .expect("fetch should succeed against the mock");

        mock.assert_async().await;
        assert_eq!(models.len(), 3);

        let pixel = models.iter().find(|m| m.id == "test-pixel-model").unwrap(); // CI-ALLOW: hardcoded-model
        assert_eq!(pixel.sizing, VeniceImageSizing::Pixel);
        assert_eq!(pixel.width_height_divisor, Some(8));
        assert_eq!(pixel.price_usd, Some(0.01));

        let ar = models.iter().find(|m| m.id == "test-ar-model").unwrap(); // CI-ALLOW: hardcoded-model
        assert_eq!(ar.sizing, VeniceImageSizing::AspectRatio);
        assert_eq!(ar.price_usd, Some(0.05));

        let res = models.iter().find(|m| m.id == "test-res-model").unwrap(); // CI-ALLOW: hardcoded-model
        assert_eq!(res.sizing, VeniceImageSizing::AspectRatioWithResolution);
        assert_eq!(res.default_resolution, Some("1K".to_string()));
        // No flat `pricing.generation` on this model — price must come
        // from the lowest resolution tier ("1K"), never a guessed number.
        assert_eq!(res.price_usd, Some(0.27));
    }

    #[tokio::test]
    async fn fetch_venice_image_models_cmd_maps_to_public_subset() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::UrlEncoded("type".into(), "image".into()))
            .with_status(200)
            .with_body(VENICE_MODELS_BODY)
            .create_async()
            .await;

        let models = fetch_venice_image_models_at("test-key", &server.url())
            .await
            .expect("should succeed")
            .into_iter()
            .map(|m| VeniceImageModel {
                id: m.id,
                name: m.name,
                price_usd: m.price_usd,
            })
            .collect::<Vec<_>>();
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].name, "Test Pixel Model");
    }

    // ── generate_venice_image_at: per-model sizing branching ────────────

    fn pixel_meta() -> VeniceImageModelMeta {
        VeniceImageModelMeta {
            id: "test-pixel-model".to_string(), // CI-ALLOW: hardcoded-model
            name: "Test Pixel Model".to_string(),
            price_usd: None,
            sizing: VeniceImageSizing::Pixel,
            width_height_divisor: Some(8),
            default_resolution: None,
        }
    }

    fn ar_meta() -> VeniceImageModelMeta {
        VeniceImageModelMeta {
            id: "test-ar-model".to_string(), // CI-ALLOW: hardcoded-model
            name: "Test AR Model".to_string(),
            price_usd: None,
            sizing: VeniceImageSizing::AspectRatio,
            width_height_divisor: Some(8),
            default_resolution: None,
        }
    }

    fn res_meta() -> VeniceImageModelMeta {
        VeniceImageModelMeta {
            id: "test-res-model".to_string(), // CI-ALLOW: hardcoded-model
            name: "Test Res Model".to_string(),
            price_usd: None,
            sizing: VeniceImageSizing::AspectRatioWithResolution,
            width_height_divisor: Some(8),
            default_resolution: Some("1K".to_string()),
        }
    }

    const VENICE_GENERATE_SUCCESS_BODY: &str =
        r#"{"id":"generate-image-test","images":["aGVsbG8="],"timing":{"inferenceDuration":1,"inferencePreprocessingTime":1,"inferenceQueueTime":1,"total":3}}"#;

    /// §39.6's named mutation, applied directly: "send width/height to
    /// both — reddens." This test proves the Pixel half: `width`/`height`
    /// present, `aspect_ratio` absent.
    #[tokio::test]
    async fn test_venice_pixel_model_sends_width_height_not_aspect_ratio() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/image/generate")
            // Exact JSON match, not partial — this is what proves
            // `aspect_ratio` is ABSENT (a `PartialJson` match would still
            // pass even if the adapter wrongly sent both).
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "model": "test-pixel-model", // CI-ALLOW: hardcoded-model
                "prompt": "a lighthouse at dusk",
                "format": "png",
                "width": 1024,
                "height": 576
            })))
            .with_status(200)
            .with_body(VENICE_GENERATE_SUCCESS_BODY)
            .create_async()
            .await;

        let decoded = generate_venice_image_at(
            "test-key",
            "test-pixel-model", // CI-ALLOW: hardcoded-model
            "a lighthouse at dusk",
            Some("16:9"),
            &pixel_meta(),
            &server.url(),
        )
        .await
        .expect("should succeed");

        mock.assert_async().await;
        assert_eq!(decoded.bytes, b"hello");
        assert_eq!(decoded.extension, "png");
        assert_eq!(decoded.cost_usd, None, "Venice's generate response reports no cost, ever");
    }

    /// §39.6's named mutation, the other half: an aspect-ratio-only model
    /// must get `aspect_ratio` and must NEVER receive `width`/`height`
    /// (Venice's own docs: these models reject them with a 400).
    #[tokio::test]
    async fn test_venice_aspect_ratio_model_sends_aspect_ratio_not_width_height() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/image/generate")
            // Exact JSON match — proves `width`/`height` are ABSENT (Venice
            // rejects them with a 400 for this model family).
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "model": "test-ar-model", // CI-ALLOW: hardcoded-model
                "prompt": "a lighthouse at dusk",
                "format": "png",
                "aspect_ratio": "16:9"
            })))
            .with_status(200)
            .with_body(VENICE_GENERATE_SUCCESS_BODY)
            .create_async()
            .await;

        let decoded = generate_venice_image_at(
            "test-key",
            "test-ar-model", // CI-ALLOW: hardcoded-model
            "a lighthouse at dusk",
            Some("16:9"),
            &ar_meta(),
            &server.url(),
        )
        .await
        .expect("should succeed");

        mock.assert_async().await;
        assert_eq!(decoded.bytes, b"hello");
    }

    /// The third shape: aspect_ratio AND resolution together, resolution
    /// defaulted from the model's own metadata since the tool exposes no
    /// `resolution` param.
    #[tokio::test]
    async fn test_venice_resolution_tier_model_sends_aspect_ratio_and_default_resolution() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/image/generate")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "aspect_ratio": "16:9",
                "resolution": "1K"
            })))
            .with_status(200)
            .with_body(VENICE_GENERATE_SUCCESS_BODY)
            .create_async()
            .await;

        let decoded = generate_venice_image_at(
            "test-key",
            "test-res-model", // CI-ALLOW: hardcoded-model
            "a lighthouse at dusk",
            Some("16:9"),
            &res_meta(),
            &server.url(),
        )
        .await
        .expect("should succeed");

        mock.assert_async().await;
        assert_eq!(decoded.bytes, b"hello");
    }

    /// Mutation target (§39.6's own "swap the decode path" wording,
    /// applied to Venice's distinct response field): look for
    /// `data[].b64_json` (OpenRouter's shape) instead of `images[]`
    /// (Venice's real shape) — reddens, since the mocked body has no
    /// `data` key at all.
    #[tokio::test]
    async fn test_venice_adapter_decodes_images_array_not_data_b64_json() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/image/generate")
            .with_status(200)
            .with_body(VENICE_GENERATE_SUCCESS_BODY)
            .create_async()
            .await;

        let decoded = generate_venice_image_at(
            "test-key", "m", "p", None, &pixel_meta(), &server.url(),
        )
        .await
        .expect("should succeed");
        assert_eq!(decoded.bytes, b"hello");
        assert_ne!(decoded.bytes, b"aGVsbG8=".to_vec());
    }

    /// Every Venice request explicitly asks for `format: "png"` — the
    /// response carries no per-image format field, so the extension has
    /// to come from what was requested.
    #[tokio::test]
    async fn test_venice_adapter_always_requests_png_format() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/image/generate")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({"format": "png"})))
            .with_status(200)
            .with_body(VENICE_GENERATE_SUCCESS_BODY)
            .create_async()
            .await;

        let decoded = generate_venice_image_at("test-key", "m", "p", None, &pixel_meta(), &server.url())
            .await
            .expect("should succeed");
        mock.assert_async().await;
        assert_eq!(decoded.extension, "png");
    }

    /// A pixel model with NO `aspect_ratio` requested at all omits
    /// width/height entirely — Venice's own server-side default (1024x1024)
    /// applies, rather than this adapter guessing a value.
    #[tokio::test]
    async fn test_venice_pixel_model_with_no_aspect_ratio_omits_width_height() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/image/generate")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "model": "m",
                "prompt": "p",
                "format": "png"
            })))
            .with_status(200)
            .with_body(VENICE_GENERATE_SUCCESS_BODY)
            .create_async()
            .await;

        generate_venice_image_at("test-key", "m", "p", None, &pixel_meta(), &server.url())
            .await
            .expect("should succeed");
        mock.assert_async().await;
    }

    /// Confirms Venice's generate call routes through the SAME shared
    /// retry helper OpenRouter's adapter uses (`post_image_request_with_
    /// retry`) — the full 4xx/5xx-retry matrix is already proven against
    /// that helper by the OpenRouter tests above; this is one targeted
    /// confirmation that Venice's adapter is actually wired through it,
    /// not a second copy of the whole matrix.
    #[tokio::test]
    async fn test_venice_adapter_retries_exactly_once_on_5xx_then_succeeds() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/image/generate")
            .with_status(500)
            .expect(1)
            .create_async()
            .await;
        let success_mock = server
            .mock("POST", "/image/generate")
            .with_status(200)
            .with_body(VENICE_GENERATE_SUCCESS_BODY)
            .expect(1)
            .create_async()
            .await;

        let result =
            generate_venice_image_at("test-key", "m", "p", None, &pixel_meta(), &server.url()).await;
        assert!(result.is_ok(), "5xx then 200 must succeed overall: {:?}", result.err());
        mock.assert_async().await;
        success_mock.assert_async().await;
    }

    // ── execute_generate_image_at: full path with provider = venice ─────

    fn seed_venice_image_settings(db: &crate::database::DbState, model_id: &str) {
        let conn = db.0.lock().unwrap();
        crate::database::set_ai_setting(&conn, IMAGE_PROVIDER_SETTING_KEY, "venice").unwrap();
        crate::database::set_ai_setting(&conn, IMAGE_MODEL_SETTING_KEY, model_id).unwrap();
        drop(conn);
        crate::ai::credential_migration::write_provider_key(
            &db.0.lock().unwrap(),
            "venice",
            "test-venice-key",
        )
        .unwrap();
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn execute_generate_image_at_venice_provider_succeeds_end_to_end() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_venice_image_settings(&db, "test-pixel-model");

        let mut server = mockito::Server::new_async().await;
        let _models_mock = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::UrlEncoded("type".into(), "image".into()))
            .with_status(200)
            .with_body(VENICE_MODELS_BODY)
            .create_async()
            .await;
        let _gen_mock = server
            .mock("POST", "/image/generate")
            .with_status(200)
            .with_body(VENICE_GENERATE_SUCCESS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse at dusk" });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url())
            .await
            .expect("venice provider should succeed end to end");

        assert!(result.get("attachment_id").and_then(|v| v.as_str()).is_some());
        // §39.3/§39.4: Venice never reports a cost — the key must be
        // entirely absent, same as OpenRouter's own "unreported" case.
        assert!(!result.as_object().unwrap().contains_key("cost"));
    }

    /// A model id the Venice list doesn't contain must refuse cleanly —
    /// never guess a sizing scheme for an unknown model.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn execute_generate_image_at_venice_unknown_model_errors() {
        let _kc = crate::keychain::test_support::KeychainGuard::set();
        let (db, _fs) = setup_test_db();
        seed_venice_image_settings(&db, "does-not-exist-model"); // CI-ALLOW: hardcoded-model

        let mut server = mockito::Server::new_async().await;
        let _models_mock = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::UrlEncoded("type".into(), "image".into()))
            .with_status(200)
            .with_body(VENICE_MODELS_BODY)
            .create_async()
            .await;

        let params = serde_json::json!({ "prompt": "a lighthouse at dusk" });
        let result = execute_generate_image_at(&params, &db, None, None, &server.url(), &server.url()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    // ── IMAGE_CAPABLE_PROVIDER_TYPES ─────────────────────────────────────

    #[test]
    fn image_capable_provider_types_includes_venice_pr2() {
        assert!(IMAGE_CAPABLE_PROVIDER_TYPES.contains(&"venice"));
        assert!(IMAGE_CAPABLE_PROVIDER_TYPES.contains(&"openrouter"));
    }
}
