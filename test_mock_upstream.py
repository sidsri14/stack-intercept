"""
Mock upstream integration test for StackIntercept.

Starts a mock HTTP server to act as the upstream LLM provider, starts the
StackIntercept proxy pointing at the mock, sends requests and verifies
exact cache hit/miss behavior — all without API keys or model weights.
"""

import http.server
import json
import os
import signal
import subprocess
import sys
import threading
import time
import urllib.request

PROXY_PORT = 8080
MOCK_PORT = 8081
PROXY_URL = f"http://127.0.0.1:{PROXY_PORT}/v1/chat/completions"
MOCK_URL = f"http://127.0.0.1:{MOCK_PORT}/v1/chat/completions"

PASS = 0
FAIL = 0

mock_request_count = 0
mock_request_lock = threading.Lock()
mock_response = {
    "id": "mock-cmpl-001",
    "object": "chat.completion",
    "created": 1700000000,
    "model": "mock-model",
    "choices": [{
        "index": 0,
        "message": {
            "role": "assistant",
            "content": "Mock upstream response"
        },
        "finish_reason": "stop"
    }],
    "usage": {
        "prompt_tokens": 10,
        "completion_tokens": 5,
        "total_tokens": 15
    }
}


class MockHandler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        global mock_request_count
        content_len = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(content_len)
        # Verify it's valid JSON
        try:
            json.loads(body)
        except json.JSONDecodeError:
            self.send_response(400)
            self.end_headers()
            self.wfile.write(b'{"error": "bad request"}')
            return

        with mock_request_lock:
            mock_request_count += 1

        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(mock_response).encode())

    def log_message(self, fmt, *args):
        pass  # Suppress mock server logs


def start_mock():
    server = http.server.HTTPServer(("127.0.0.1", MOCK_PORT), MockHandler)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return server


def proxy_binary():
    """Return the proxy binary path, with .exe on Windows."""
    base = "./target/debug/stack-intercept"
    return base + ".exe" if sys.platform == "win32" else base


def start_proxy():
    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_UPSTREAM_URL"] = f"http://127.0.0.1:{MOCK_PORT}"
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
            # 415/405 means server is up and receiving (just wrong method/body for warmup)
            if e.code in (415, 405):
                return True
        except (ConnectionResetError, urllib.error.URLError, OSError):
            pass
        time.sleep(0.3)
    return False


def send_request(payload):
    data = json.dumps(payload).encode()
    req = urllib.request.Request(
        PROXY_URL,
        data=data,
        headers={"Content-Type": "application/json", "Authorization": "Bearer mock-key"},
        method="POST",
    )
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        hit_flag = resp.headers.get("x-stack-intercept", "")
        body = resp.read().decode()
        return hit_flag, resp.status, body
    except urllib.error.HTTPError as e:
        hit_flag = e.headers.get("x-stack-intercept", "")
        return hit_flag, e.code, e.read().decode()


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS: {name} {detail}")
    else:
        FAIL += 1
        print(f"  FAIL: {name} {detail}")


def main():
    global mock_request_count

    print("Starting mock upstream server...")
    mock_server = start_mock()

    print("Starting StackIntercept proxy...")
    proxy = start_proxy()

    try:
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy did not start")
            sys.exit(1)
        print("  Proxy is online.\n")

        payload = {
            "model": "mock-model",
            "messages": [{"role": "user", "content": "Hello"}],
            "temperature": 0,
            "stream": False,
        }

        # === Test 1: First request should be a cache miss ===
        print("=" * 60)
        print("Test 1: First request — cache miss")
        hit, status, _ = send_request(payload)
        check("x-stack-intercept is not hit", hit != "hit", f"(got: {hit})")
        check("status is 200", status == 200, f"(status: {status})")
        print()

        # === Test 2: Second identical request should be a cache hit ===
        print("=" * 60)
        print("Test 2: Second request — exact cache hit")
        hit, status, _ = send_request(payload)
        check("x-stack-intercept is hit", hit == "hit", f"(got: {hit})")
        check("status is 200", status == 200, f"(status: {status})")
        print()

        # === Test 3: Mock should have received exactly 1 request ===
        print("=" * 60)
        print("Test 3: Mock upstream call count")
        with mock_request_lock:
            check("only 1 upstream call", mock_request_count == 1, f"(count: {mock_request_count})")
        print()

        # === Test 4: Different prompt should miss ===
        print("=" * 60)
        print("Test 4: Different prompt — cache miss")
        payload2 = {
            "model": "mock-model",
            "messages": [{"role": "user", "content": "Different query"}],
            "temperature": 0,
            "stream": False,
        }
        hit, status, _ = send_request(payload2)
        check("x-stack-intercept is not hit", hit != "hit", f"(got: {hit})")
        print()

        # === Summary ===
        print("=" * 60)
        total = PASS + FAIL
        print(f"Results: {PASS}/{total} passed", "ALL PASSED" if FAIL == 0 else f"{FAIL} FAILURES")

        return 0 if FAIL == 0 else 1

    finally:
        proxy.terminate()
        proxy.wait(timeout=5)
        mock_server.shutdown()


if __name__ == "__main__":
    sys.exit(main())
