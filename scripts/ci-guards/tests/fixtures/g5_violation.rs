// G5 fixture: VIOLATION — context window integer literal outside context_budget.rs
// This file exists only to test the G5 guard. It must NOT appear in production source.

fn calculate_token_budget(model_id: &str) -> usize {
    // Violation: hardcoded window size should come from context_budget::get_context_window()
    let window = if model_id.contains("claude") {
        200_000
    } else {
        128_000
    };
    window - 4096
}
