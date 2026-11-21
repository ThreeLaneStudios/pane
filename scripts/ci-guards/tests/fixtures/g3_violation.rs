// G3 fixture: VIOLATION — hardcoded model ID in production Rust
// This file exists only to test the G3 guard. It must NOT appear in production source.

fn set_default_model(state: &AppState) {
    // Violation: hardcoded model ID literal in production code
    let default_model = "anthropic/claude-opus-4-6";
    state.set_model(default_model);
}

fn other_path() {
    let model = "openai/gpt-4o";
    let anthropic_model = "claude-sonnet-4-5";
}
