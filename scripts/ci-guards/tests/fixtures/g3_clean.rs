// G3 fixture: CLEAN — no hardcoded model ID strings
// This file exists only to test the G3 guard. It must NOT appear in production source.

fn set_default_model(state: &AppState, agent_response: &AgentInfo) {
    // Clean: model ID comes from the API response, not a hardcoded literal
    let default_model = &agent_response.model;
    state.set_model(default_model);
}

fn format_model_display_name(model_id: &str) -> &str {
    // Clean: operates on a variable, not a literal
    model_id.split('/').last().unwrap_or(model_id)
}
