// #470 PR 4: the smallest thing that serves a fixed response for the one
// call this journey forces -- fetch_available_models -> fetch_ollama_models
// -> GET /api/tags (src-tauri/src/ai/connections.rs). No chat endpoint: this
// journey never sends a message (traced in the PR 4 scope doc, §3a).
//
// Node's `http` is a builtin -- no new dependency, no new toolchain
// requirement. Matches §25.13's "no model, no cron at run time": a static-
// response listener, nothing else.
import http from 'node:http';

const MODEL_NAME = process.env.MOCK_OLLAMA_MODEL ?? 'llama3.2';

const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url === '/api/tags') {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ models: [{ name: MODEL_NAME }] }));
    return;
  }
  res.writeHead(404);
  res.end();
});

// Ephemeral port (0) -- avoids any fixed-port collision risk on the
// runner; the workflow reads the actual bound port off this one stdout line.
server.listen(0, '127.0.0.1', () => {
  console.log(`MOCK_OLLAMA_PORT=${server.address().port}`);
});
