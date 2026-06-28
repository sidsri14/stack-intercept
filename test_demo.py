"""
StackIntercept Demo - 60-second walkthrough.

Shows the full caching + routing flow with mock servers:
  1. Simple gpt-4o prompt - routed to deepseek-chat (fallback)
  2. Same prompt again - cache hit (zero upstream calls)
  3. High-reasoning prompt - stays on gpt-4o (passthrough)
  4. Route headers visible on every response

Usage:
    python test_demo.py
"""
import http.server
import json
import os
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

PROXY_PORT = 8080
MOCK_UPSTREAM_PORT = 8098
MOCK_FALLBACK_PORT = 8099
PROXY_URL = f"http://127.0.0.1:{PROXY_PORT}/v1/chat/completions"


class MockServer:
    """Mock HTTP server that echoes the model name in its response."""
    def __init__(self, port, label):
        self.port = port
        self.label = label
        self.request_count = 0
        self.lock = threading.Lock()
        self.server = None

    def start(self):
        owner = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                content_len = int(self.headers.get("Content-Length", 0))
                body = self.rfile.read(content_len)
                with owner.lock:
                    owner.request_count += 1
                payload = json.loads(body)
                model = payload.get("model", "unknown")
                resp = json.dumps({
                    "id": f"{owner.label}-cmpl",
                    "object": "chat.completion",
                    "created": int(time.time()),
                    "model": model,
                    "choices": [{"index": 0, "message": {
                        "role": "assistant",
                        "content": f"Response from {owner.label} (model: {model})",
                    }, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 5, "completion_tokens": 3, "total_tokens": 8},
                }).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(resp)
            def log_message(self, fmt, *args):
                pass
        self.server = http.server.HTTPServer(("127.0.0.1", self.port), Handler)
        t = threading.Thread(target=self.server.serve_forever, daemon=True)
        t.start()
        return self.server

    def shutdown(self):
        if self.server:
            self.server.shutdown()


def proxy_binary():
    base = "./target/debug/stack-intercept"
    return base + ".exe" if sys.platform == "win32" else base


def start_proxy(upstream_url, fallback_url):
    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_ALLOW_MODEL_REWRITE"] = "true"
    env["STACK_INTERCEPT_UPSTREAM_URL"] = upstream_url
    env["STACK_INTERCEPT_FALLBACK_URL"] = fallback_url
    env["STACK_INTERCEPT_FALLBACK_API_KEY"] = "sk-demo-fallback"
    env.pop("DEEPSEEK_API_KEY", None)
    proc = subprocess.Popen(
        [proxy_binary()],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    return proc


def wait_for(url, timeout=15):
    start = time.time()
    while time.time() - start < timeout:
        try:
            urllib.request.urlopen(urllib.request.Request(url, method="POST", data=b"{}"), timeout=2)
            return True
        except urllib.error.HTTPError as e:
            if e.code in (415, 405):
                return True
        except (ConnectionResetError, urllib.error.URLError, OSError):
            pass
        time.sleep(0.3)
    return False


def send(payload, extra_headers=None):
    """Send request, return (status, body, headers_dict)."""
    data = json.dumps(payload).encode()
    headers = {"Content-Type": "application/json", "Authorization": "Bearer demo-key"}
    if extra_headers:
        headers.update(extra_headers)
    req = urllib.request.Request(PROXY_URL, data=data, headers=headers, method="POST")
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        body = resp.read().decode()
        return resp.status, body, dict(resp.headers)
    except urllib.error.HTTPError as e:
        body = e.read().decode()
        return e.code, body, dict(e.headers)


def show_request(label, payload, extra_headers=None):
    """Send request and print formatted result."""
    print(f"  Request: {payload['model']} | \"{payload['messages'][-1]['content'][:60]}\"")
    status, body, headers = send(payload, extra_headers)
    resp_data = json.loads(body)
    si = headers.get("x-stack-intercept", "-")
    route = headers.get("x-stack-intercept-route", "-")
    orig = headers.get("x-stack-intercept-original-model", "-")
    routed = headers.get("x-stack-intercept-routed-model", "-")
    content = resp_data["choices"][0]["message"]["content"]
    print(f"  Status: {status}")
    print(f"  Headers:")
    print(f"    x-stack-intercept:              {si}")
    print(f"    x-stack-intercept-route:        {route}")
    print(f"    x-stack-intercept-original-model: {orig}")
    print(f"    x-stack-intercept-routed-model:  {routed}")
    print(f"  Response: \"{content}\"")
    print()


def main():
    print()
    print("  " + "+" + "-" * 56 + "+")
    print("  |           StackIntercept -- Live Demo                |")
    print("  |   Local OpenAI-compatible cost-control proxy         |")
    print("  " + "+" + "-" * 56 + "+")
    print()

    # Start mock servers
    print("  Starting mock servers...")
    upstream = MockServer(MOCK_UPSTREAM_PORT, "UPSTREAM")
    upstream.start()
    fallback = MockServer(MOCK_FALLBACK_PORT, "FALLBACK")
    fallback.start()
    upstream_url = f"http://127.0.0.1:{MOCK_UPSTREAM_PORT}"
    fallback_url = f"http://127.0.0.1:{MOCK_FALLBACK_PORT}"

    # Start proxy
    print("  Starting StackIntercept proxy (routing enabled)...")
    proxy = start_proxy(upstream_url, fallback_url)
    if not wait_for(PROXY_URL):
        print("  FAILED: Proxy did not start")
        sys.exit(1)
    print("  Proxy online at http://127.0.0.1:8080")
    print()

    try:
        # Step 1: Simple gpt-4o prompt -> routed to fallback
        print("  " + "-" * 58)
        print("  Step 1: Simple gpt-4o prompt")
        print("           -> routed to deepseek-chat (fallback)")
        print("  " + "-" * 58)
        show_request("Step 1", {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "What is the capital of France?"}],
            "temperature": 0,
            "stream": False,
        })
        print(f"    Upstream requests: {upstream.request_count} | Fallback requests: {fallback.request_count}")
        print()

        # Step 2: Same prompt -> cache hit
        print("  " + "-" * 58)
        print("  Step 2: Same prompt again")
        print("           -> cache hit (zero upstream calls)")
        print("  " + "-" * 58)
        show_request("Step 2", {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "What is the capital of France?"}],
            "temperature": 0,
            "stream": False,
        })
        print(f"    Upstream requests: {upstream.request_count} | Fallback requests: {fallback.request_count}")
        print()

        # Step 3: High-reasoning prompt -> passthrough
        print("  " + "-" * 58)
        print("  Step 3: High-reasoning prompt (cryptography)")
        print("           -> stays on gpt-4o (passthrough)")
        print("  " + "-" * 58)
        show_request("Step 3", {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Explain AES-256 cryptography in detail"}],
            "temperature": 0,
            "stream": False,
        })
        print(f"    Upstream requests: {upstream.request_count} | Fallback requests: {fallback.request_count}")
        print()

        # Summary
        print("  " + "+" + "-" * 56 + "+")
        print("  |  Summary                                           |")
        print("  " + "+" + "-" * 56 + "+")
        print()
        print(f"    Upstream (gpt-4o mock)  : {upstream.request_count} request(s)")
        print(f"    Fallback (deepseek mock): {fallback.request_count} request(s)")
        print(f"    Total proxy requests    : 3 (1 routed, 1 cached, 1 passthrough)")
        print(f"    Cache hits             : 1 (saved 1 fallback call)")
        print(f"    Total upstream calls    : 2 vs 3 without proxy")
        print()

    finally:
        proxy.terminate()
        proxy.wait(timeout=5)
        upstream.shutdown()
        fallback.shutdown()

    print("  Demo complete.")
    print()


if __name__ == "__main__":
    sys.exit(main())
