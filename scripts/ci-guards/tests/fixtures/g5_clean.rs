// G5 fixture: CLEAN — window size sourced from context_budget module, no integer literal
// This file exists only to test the G5 guard. It must NOT appear in production source.

use crate::ai::context_budget;

fn calculate_token_budget(model_id: &str) -> usize {
    // Clean: window size comes from the authoritative context_budget module
    let window = context_budget::get_context_window(model_id);
    let reserve = context_budget::get_response_reserve(window);
    window - reserve
}
