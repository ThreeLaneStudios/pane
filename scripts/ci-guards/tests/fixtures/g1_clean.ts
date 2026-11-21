// G1 fixture: CLEAN — temperature sourced from config, not a literal
// This file exists only to test the G1 guard. It must NOT appear in production source.

async function gatewayRequest(prompt: string, config: RequestConfig): Promise<Response> {
  return fetch('/v1/chat/completions', {
    method: 'POST',
    body: JSON.stringify({
      model: config.modelId,
      temperature: config.temperature,
      messages: [{ role: 'user', content: prompt }],
    }),
  });
}
