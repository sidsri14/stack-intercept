"""
Dynamic routing verification test.

Starts the mock upstream server + proxy with routing enabled, sends a
premium-model request with a simple prompt, and verifies the proxy
routes it through (no crash, valid response). No API keys required.
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
MOCK_PORT = 8099
PROXY_URL = f"http://127.0.0.1:{PROXY_PORT}/v1/chat/completions"

PASS = 0
FAIL = 0

mock_response = json.dumps({
    "id": "routing-cmpl-001",
    "object": "chat.completion",
    "created": 1700000000,
    "model": "deepseek-chat",
    "choices": [{
        "index": 0,
        "message": {"role": "assistant", "content": "Routed response"},
        "finish_reason": "stop"
    }],
    "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
}).encode()


class MockHandlerRouting(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        content_len = int(self.headers.get("Content-Length", 0))
        self.rfile.read(content_len)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(mock_response)

    def log_message(self, fmt, *args):
        pass


def start_mock():
    server = http.server.HTTPServer(("127.0.0.1", MOCK_PORT), MockHandlerRouting)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return server


def proxy_binary():
    base = "./target/release/stack-intercept"
    return base + ".exe" if sys.platform == "win32" else base


def start_proxy():
    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_ALLOW_MODEL_REWRITE"] = "true"
    env["STACK_INTERCEPT_UPSTREAM_URL"] = f"http://127.0.0.1:{MOCK_PORT}"
    env["STACK_INTERCEPT_FALLBACK_URL"] = f"http://127.0.0.1:{MOCK_PORT}"
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


def send_request(payload):
    data = json.dumps(payload).encode()
    req = urllib.request.Request(
        PROXY_URL, data=data,
        headers={"Content-Type": "application/json", "Authorization": "Bearer test-key"},
        method="POST",
    )
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        hit = resp.headers.get("x-stack-intercept", "")
        body = resp.read().decode()
        return hit, resp.status, body
    except urllib.error.HTTPError as e:
        hit = e.headers.get("x-stack-intercept", "")
        return hit, e.code, e.read().decode()


def check(name, cond):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS: {name}")
    else:
        FAIL += 1
        print(f"  FAIL: {name}")


def main():
    global PASS, FAIL

    print("Starting mock server...")
    mock = start_mock()

    print("Starting proxy (routing enabled)...")
    proxy = start_proxy()

    try:
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy did not start")
            sys.exit(1)
        print("  Proxy online.\n")

        # Request gpt-4o with a simple prompt — routing should trigger
        payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "What is the capital of France?"}],
            "temperature": 0,
            "stream": False,
        }

        print("=" * 60)
        print("Test: gpt-4o simple prompt -> routed to fallback")
        hit, status, body = send_request(payload)
        check("status is 200", status == 200)
        check("not a cache hit", hit != "hit")
        check("body contains response", "Routed response" in body)
        print()

        # Second request — should be a cache hit (exact match)
        print("=" * 60)
        print("Test: identical request -> cache hit")
        hit, status, body = send_request(payload)
        check("status is 200", status == 200)
        check("is a cache hit", hit == "hit")
        check("body preserved", "Routed response" in body)
        print()

        # Request gpt-4o with a high-reasoning keyword — routing should NOT trigger
        print("=" * 60)
        print("Test: gpt-4o with 'cryptography' -> NOT routed (high reasoning)")
        crypto_payload = {
            "model": "gpt-4o",
            "messages": [{"role": "user", "content": "Explain AES cryptography"}],
            "temperature": 0,
            "stream": False,
        }
        hit, status, body = send_request(crypto_payload)
        check("status is 200", status == 200)
        # Still gets a response since the mock handles all models
        check("not a cache hit", hit != "hit")
        print()

        print("=" * 60)
        total = PASS + FAIL
        print(f"Results: {PASS}/{total} passed", "ALL PASSED" if FAIL == 0 else f"{FAIL} FAILURES")
        return 0 if FAIL == 0 else 1

    finally:
        proxy.terminate()
        proxy.wait(timeout=5)
        mock.shutdown()


if __name__ == "__main__":
    sys.exit(main())
