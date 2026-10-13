# Mock OpenAI-compatible server for testing Cue without API keys.
# Usage: python3 scripts/dev/mock_openai_server.py 18080 /tmp/cue-mock-requests.log
# - GET /v1/models: two models; "Bearer bad-key" gets 401
# - POST /v1/chat/completions: streams a Markdown answer; a message containing
#   "ratelimit" returns 429. Every request body is appended to the log file.
import json, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _json(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code); self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        if self.path.endswith("/models"):
            if self.headers.get("Authorization") == "Bearer bad-key":
                return self._json(401, {"error": {"message": "Invalid API key", "type": "invalid_request_error", "code": "invalid_api_key"}})
            return self._json(200, {"object": "list", "data": [{"id": "mock-llama-mini"}, {"id": "mock-llama-large"}]})
        self._json(404, {"error": {"message": "not found"}})
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0)); body = json.loads(self.rfile.read(n) or b"{}")
        with open(sys.argv[2], "a") as f: f.write(json.dumps(body) + "\n")
        last = body["messages"][-1]["content"]
        if "ratelimit" in last:
            return self._json(429, {"error": {"message": "Rate limit reached for requests", "code": "rate_limit_exceeded"}})
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.end_headers()
        turns = len([m for m in body["messages"] if m["role"] == "user"])
        reply = f"**Mock answer** to: _{last}_ (turn {turns}).\n\n```python\nprint('hello from {body['model']}')\n```\n- point one\n- point two"
        for i in range(0, len(reply), 12):
            chunk = {"id": "c1", "model": body["model"], "choices": [{"index": 0, "delta": {"content": reply[i:i+12]}}]}
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode()); self.wfile.flush(); time.sleep(0.03)
        self.wfile.write(("data: " + json.dumps({"id":"c1","model":body["model"],"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}) + "\n\n").encode())
        self.wfile.write(("data: " + json.dumps({"id":"c1","model":body["model"],"choices":[],"usage":{"prompt_tokens":42,"completion_tokens":17}}) + "\n\n").encode())
        self.wfile.write(b"data: [DONE]\n\n"); self.wfile.flush()

print(f"Mock OpenAI-compatible server on http://127.0.0.1:{sys.argv[1]}/v1", flush=True)
ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
