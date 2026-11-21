// G1 fixture: VIOLATION — hardcoded temperature literal in TypeScript
// This file exists only to test the G1 guard. It must NOT appear in production source.

async function gatewayRequest(prompt: string): Promise<Response> {
  return fetch('/v1/chat/completions', {
    method: 'POST',
    body: JSON.stringify({
      model: modelId,
      "temperature": 0,
      max_tokens: 16000,
      messages: [{ role: 'user', content: prompt }],
    }),
  });
}
