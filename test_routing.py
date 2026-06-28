"""
Dynamic routing verification test with two mock servers.

Starts a mock upstream server and a mock fallback server, starts the proxy
with routing enabled, and verifies:
- Premium models with simple prompts are routed to fallback
- Identical requests are cache hits with route headers
- High-reasoning prompts are NOT routed
- Outbound model in fallback requests is correct
- Auth header format is correct (Bearer prefix)
- Route headers are present on all responses
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
MOCK_UPSTREAM_URL = f"http://127.0.0.1:{MOCK_UPSTREAM_PORT}"
MOCK_FALLBACK_URL = f"http://127.0.0.1:{MOCK_FALLBACK_PORT}"

PASS = 0
FAIL = 0


class RequestCapture:
    """Captures details of a single mock request."""
    def __init__(self, method, path, headers, body):
        self.method = method
        self.path = path
        self.headers = headers
        self.body = body


class MockServer:
    """Mock HTTP server that captures requests for verification."""
    def __init__(self, port, response_data):
        self.port = port
        self.response_data = response_data
        self.captured_requests = []
        self.request_lock = threading.Lock()
        self.server = None

    def start(self):
        server = http.server.HTTPServer(("127.0.0.1", self.port), self._make_handler())
        t = threading.Thread(target=server.serve_forever, daemon=True)
        t.start()
        self.server = server
        return server

    def _make_handler(self):
        owner = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                content_len = int(self.headers.get("Content-Length", 0))
                body = self.rfile.read(content_len)
                with owner.request_lock:
                    owner.captured_requests.append(RequestCapture(
                        self.command, self.path, dict(self.headers), body,
                    ))
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(owner.response_data)

            def log_message(self, fmt, *args):
                pass
        return Handler

    @property
    def request_count(self):
        with self.request_lock:
            return len(self.captured_requests)

    @property
    def last_request(self):
        with self.request_lock:
            return self.captured_requests[-1] if self.captured_requests else None

    def shutdown(self):
        if self.server:
            self.server.shutdown()

    def reset(self):
        with self.request_lock:
            self.captured_requests.clear()


# Separate response bodies so we can distinguish which server responded
upstream_response = json.dumps({
    "id": "upstream-cmpl-001",
    "object": "chat.completion",
    "created": 1700000000,
    "model": "gpt-4o",
    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Upstream response"}, "finish_reason": "stop"}],
    "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7},
}).encode()

fallback_response = json.dumps({
    "id": "fallback-cmpl-001",
    "object": "chat.completion",
    "created": 1700000000,
    "model": "deepseek-chat",
    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Fallback response"}, "finish_reason": "stop"}],
    "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7},
}).encode()


def proxy_binary():
    # Use debug binary for CI consistency (release build is separate)
    base = "./target/debug/stack-intercept"
    return base + ".exe" if sys.platform == "win32" else base


def start_proxy(allow_rewrite="true", set_fallback_key=True):
    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_ALLOW_MODEL_REWRITE"] = allow_rewrite
    env["STACK_INTERCEPT_UPSTREAM_URL"] = MOCK_UPSTREAM_URL
    env["STACK_INTERCEPT_FALLBACK_URL"] = MOCK_FALLBACK_URL
    if set_fallback_key:
        env["STACK_INTERCEPT_FALLBACK_API_KEY"] = "sk-fallback-secret"
    # Unset DEEPSEEK_API_KEY if present so test env is clean
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


def send_request(payload, extra_headers=None):
    data = json.dumps(payload).encode()
    headers = {"Content-Type": "application/json", "Authorization": "Bearer test-key"}
    if extra_headers:
        headers.update(extra_headers)
    req = urllib.request.Request(
        PROXY_URL, data=data,
        headers=headers,
        method="POST",
    )
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        hit = resp.headers.get("x-stack-intercept", "")
        route = resp.headers.get("x-stack-intercept-route", "")
        orig_model = resp.headers.get("x-stack-intercept-original-model", "")
        routed_model = resp.headers.get("x-stack-intercept-routed-model", "")
        body = resp.read().decode()
        return hit, route, orig_model, routed_model, resp.status, body
    except urllib.error.HTTPError as e:
        hit = e.headers.get("x-stack-intercept", "")
        route = e.headers.get("x-stack-intercept-route", "")
        orig_model = e.headers.get("x-stack-intercept-original-model", "")
        routed_model = e.headers.get("x-stack-intercept-routed-model", "")
        return hit, route, orig_model, routed_model, e.code, e.read().decode()


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS: {name} {detail}")
    else:
        FAIL += 1
        print(f"  FAIL: {name} {detail}")


def main():
    global PASS, FAIL

    print("Starting mock upstream server (port {})...".format(MOCK_UPSTREAM_PORT))
    upstream_mock = MockServer(MOCK_UPSTREAM_PORT, upstream_response)
    upstream_mock.start()

    print("Starting mock fallback server (port {})...".format(MOCK_FALLBACK_PORT))
    fallback_mock = MockServer(MOCK_FALLBACK_PORT, fallback_response)
    fallback_mock.start()

    print("Starting proxy (routing enabled)...")
    proxy = start_proxy("true")

    try:
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy did not start")
            sys.exit(1)
        print("  Proxy online.\n")

        # =========================================================
        # Test 1: gpt-4o simple prompt -> routed to fallback
        # =========================================================
        print("=" * 60)
        print("Test 1: gpt-4o simple prompt -> routed to fallback")
        payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "What is the capital of France?"}],
            "temperature": 0,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(payload)
        check("status is 200", status == 200)
        check("not a cache hit", hit != "hit")
        check("route is fallback", route == "fallback")
        check("original model is gpt-4o", orig_model == "gpt-4o")
        check("routed model is deepseek-chat", routed_model == "deepseek-chat")
        check("body is from fallback", "Fallback response" in body)
        check("upstream received 0 requests", upstream_mock.request_count == 0)
        check("fallback received 1 request", fallback_mock.request_count == 1)

        # Verify outbound request to fallback
        fb_req = fallback_mock.last_request
        if fb_req:
            fb_body = json.loads(fb_req.body)
            check("fallback model is deepseek-chat", fb_body.get("model") == "deepseek-chat")
            # Verify auth header has Bearer prefix
            auth_header = fb_req.headers.get("authorization", "")
            check("auth has Bearer prefix", auth_header.startswith("Bearer "),
                  f"(auth: {auth_header[:20]}...)")
            check("auth uses fallback key", "sk-fallback-secret" in auth_header)
        print()

        # =========================================================
        # Test 2: Identical request -> cache hit
        # =========================================================
        print("=" * 60)
        print("Test 2: identical request -> cache hit with route headers")
        hit, route, orig_model, routed_model, status, body = send_request(payload)
        check("status is 200", status == 200)
        check("is a cache hit", hit == "hit")
        check("route header on hit", route == "fallback")
        check("original model on hit", orig_model == "gpt-4o")
        check("routed model on hit", routed_model == "deepseek-chat")
        check("body preserved", "Fallback response" in body)
        # No additional requests to either server
        check("upstream still 0 requests", upstream_mock.request_count == 0)
        check("fallback still 1 request", fallback_mock.request_count == 1)
        print()

        # =========================================================
        # Test 3: gpt-4o with high-reasoning keyword -> NOT routed
        # =========================================================
        print("=" * 60)
        print("Test 3: gpt-4o with 'cryptography' -> NOT routed (high reasoning)")
        crypto_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Explain AES cryptography in detail"}],
            "temperature": 0,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(crypto_payload)
        check("status is 200", status == 200)
        check("route is passthrough", route == "passthrough")
        check("original model is gpt-4o", orig_model == "gpt-4o")
        check("routed model equals original", routed_model == "gpt-4o")
        check("body is from upstream", "Upstream response" in body)
        check("upstream received 1 request", upstream_mock.request_count == 1)
        check("fallback still 1 request", fallback_mock.request_count == 1)
        print()

        # =========================================================
        # Test 4: gpt-4o with tools -> NOT routed
        # =========================================================
        print("=" * 60)
        print("Test 4: gpt-4o with tools -> NOT routed (unsafe feature)")
        tools_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "What's the weather?"}],
            "temperature": 0,
            "stream": False,
            "tools": [{"type": "function", "function": {"name": "get_weather", "parameters": {"type": "object", "properties": {}}}}],
        }
        hit, route, orig_model, routed_model, status, body = send_request(tools_payload)
        check("status is 200", status == 200)
        check("route is passthrough", route == "passthrough")
        check("body from upstream", "Upstream response" in body)
        check("upstream got this request", upstream_mock.request_count == 2)
        print()

        # =========================================================
        # Test 5: gpt-4o with temperature > 0 -> NOT routed
        # =========================================================
        print("=" * 60)
        print("Test 5: gpt-4o with temperature=0.7 -> NOT routed (non-deterministic)")
        temp_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Write a poem"}],
            "temperature": 0.7,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(temp_payload)
        check("status is 200", status == 200)
        check("route is passthrough", route == "passthrough")
        check("body from upstream", "Upstream response" in body)
        print()

        # =========================================================
        # Test 6: No-route header -> NOT routed
        # =========================================================
        print("=" * 60)
        print("Test 6: x-stack-intercept-no-route header -> NOT routed")
        simple_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Say hello"}],
            "temperature": 0,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(
            simple_payload, extra_headers={"x-stack-intercept-no-route": "true"}
        )
        check("status is 200", status == 200)
        check("route is passthrough", route == "passthrough")
        check("body from upstream", "Upstream response" in body)
        print()

        # =========================================================
        # Test 7: Routing disabled by default
        # =========================================================
        print("=" * 60)
        print("Test 7: Routing disabled (default) -> all passthrough")
        # Restart proxy without ALLOW_MODEL_REWRITE
        proxy.terminate()
        proxy.wait(timeout=5)
        upstream_mock.reset()
        fallback_mock.reset()

        proxy = start_proxy("false")
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy did not start")
            sys.exit(1)

        no_route_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello world"}],
            "temperature": 0,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(no_route_payload)
        check("status is 200", status == 200)
        check("route is passthrough", route == "passthrough")
        check("body from upstream", "Upstream response" in body)
        check("upstream received request", upstream_mock.request_count == 1)
        check("fallback received 0 requests", fallback_mock.request_count == 0)
        print()

        # =========================================================
        # Test 8: Explicit model requirement in system prompt -> NOT routed
        # =========================================================
        print("=" * 60)
        print("Test 8: 'do not switch models' in system prompt -> NOT routed")

        # Restart proxy with routing for remaining tests
        proxy.terminate()
        proxy.wait(timeout=5)
        upstream_mock.reset()
        fallback_mock.reset()
        proxy = start_proxy("true")
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy did not start")
            sys.exit(1)

        explicit_payload = {
            "model": "gpt-4o",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant. Do not switch models."},
                {"role": "user", "content": "What is the capital of France?"},
            ],
            "temperature": 0,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(explicit_payload)
        check("status is 200", status == 200)
        check("route is passthrough", route == "passthrough")
        check("body from upstream", "Upstream response" in body)
        check("upstream received request", upstream_mock.request_count == 1)
        check("fallback received 0 requests", fallback_mock.request_count == 0)
        print()

        # =========================================================
        # Test 9: 'race condition' keyword -> NOT routed
        # =========================================================
        print("=" * 60)
        print("Test 9: 'race condition' prompt -> NOT routed (expanded keyword)")
        race_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Debug a race condition in this Go program"}],
            "temperature": 0,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(race_payload)
        check("status is 200", status == 200)
        check("route is passthrough", route == "passthrough")
        check("body from upstream", "Upstream response" in body)
        print()

        # =========================================================
        # Test 10: 'security review' keyword -> NOT routed
        # =========================================================
        print("=" * 60)
        print("Test 10: 'security review' prompt -> NOT routed (expanded keyword)")
        sec_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Perform a security review of this authentication system"}],
            "temperature": 0,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(sec_payload)
        check("status is 200", status == 200)
        check("route is passthrough", route == "passthrough")
        check("body from upstream", "Upstream response" in body)
        print()

        # =========================================================
        # Test 11: Routing enabled, no fallback key -> passthrough (no leak)
        # =========================================================
        print("=" * 60)
        print("Test 11: routing enabled + no fallback key -> passthrough (no auth leak)")
        proxy.terminate()
        proxy.wait(timeout=5)
        upstream_mock.reset()
        fallback_mock.reset()

        proxy = start_proxy("true", set_fallback_key=False)
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy did not start")
            sys.exit(1)

        no_key_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Hello world"}],
            "temperature": 0,
            "stream": False,
        }
        hit, route, orig_model, routed_model, status, body = send_request(no_key_payload)
        check("status is 200", status == 200)
        check("route is passthrough (safe fallback)", route == "passthrough")
        check("routed model equals original", routed_model == "gpt-4o")
        check("body from upstream", "Upstream response" in body)
        check("upstream received request", upstream_mock.request_count == 1)
        check("fallback received 0 requests (not leaked)", fallback_mock.request_count == 0)

        # Verify upstream received the original auth, not a leaked key
        up_req = upstream_mock.last_request
        if up_req:
            up_auth = up_req.headers.get("authorization", "")
            check("upstream auth is original", "test-key" in up_auth)
            check("upstream auth does not have fallback key", "sk-fallback-secret" not in up_auth)
        print()

        # =========================================================
        # Summary
        # =========================================================
        print("=" * 60)
        total = PASS + FAIL
        print(f"Results: {PASS}/{total} passed", "ALL PASSED" if FAIL == 0 else f"{FAIL} FAILURES")
        return 0 if FAIL == 0 else 1

    finally:
        proxy.terminate()
        proxy.wait(timeout=5)
        upstream_mock.shutdown()
        fallback_mock.shutdown()


if __name__ == "__main__":
    sys.exit(main())
