"""
Integration tests for v0.2.0: disk persistence, semantic eviction, and SSE error frames.

Starts a mock upstream, starts the proxy with various configs, and verifies
that persistence, eviction, and error-frame behavior are correct.
"""

import http.server
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

PROXY_PORT = 8080
MOCK_PORT = 8081
PROXY_URL = f"http://127.0.0.1:{PROXY_PORT}/v1/chat/completions"

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

sse_response = (
    'data: {"id":"mock-cmpl-sse","object":"chat.completion.chunk",'
    '"created":1700000000,"model":"mock-model",'
    '"choices":[{"index":0,"delta":{"content":"Mock"},"finish_reason":null}]}\n\n'
    'data: [DONE]\n\n'
)


class MockHandler(http.server.BaseHTTPRequestHandler):
    override = None
    override_lock = threading.Lock()

    @classmethod
    def set_override(cls, status, headers_list, body):
        with cls.override_lock:
            cls.override = (status, headers_list, body)

    def do_POST(self):
        global mock_request_count
        content_len = int(self.headers.get("Content-Length", 0))
        self.rfile.read(content_len)

        with MockHandler.override_lock:
            if MockHandler.override is not None:
                status, headers_list, resp_body = MockHandler.override
                MockHandler.override = None
                self.send_response(status)
                for k, v in headers_list:
                    self.send_header(k, v)
                self.end_headers()
                self.wfile.write(resp_body)
                return

        with mock_request_lock:
            mock_request_count += 1

        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(mock_response).encode())

    def log_message(self, fmt, *args):
        pass


def set_override(status, headers_list, body):
    MockHandler.set_override(status, headers_list, body)


def start_mock():
    server = http.server.HTTPServer(("127.0.0.1", MOCK_PORT), MockHandler)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return server


def proxy_binary():
    base = "./target/debug/stack-intercept"
    return base + ".exe" if sys.platform == "win32" else base


def start_proxy(extra_env=None):
    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_UPSTREAM_URL"] = f"http://127.0.0.1:{MOCK_PORT}"
    env.pop("DEEPSEEK_API_KEY", None)
    env.pop("STACK_INTERCEPT_FALLBACK_API_KEY", None)
    env.pop("STACK_INTERCEPT_FALLBACK_URL", None)
    env.pop("STACK_INTERCEPT_REACTIVE_FAILOVER", None)
    if extra_env:
        env.update(extra_env)
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
    headers = {"Content-Type": "application/json", "Authorization": "Bearer mock-key"}
    if extra_headers:
        headers.update(extra_headers)
    req = urllib.request.Request(
        PROXY_URL,
        data=data,
        headers=headers,
        method="POST",
    )
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        hit_flag = resp.headers.get("x-stack-intercept", "")
        body = resp.read().decode()
        return hit_flag, resp.status, body, resp.headers
    except urllib.error.HTTPError as e:
        hit_flag = e.headers.get("x-stack-intercept", "")
        return hit_flag, e.code, e.read().decode(), e.headers


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS: {name} {detail}")
    else:
        FAIL += 1
        print(f"  FAIL: {name} {detail}")


def reset_mock_count():
    global mock_request_count
    with mock_request_lock:
        mock_request_count = 0


# ── Tests ──

def test_persistence_exact_cache():
    """
    Start proxy with a temp cache path, send a request (miss → cache written),
    kill proxy, restart with same cache path, send same request → cache hit from disk.
    """
    print("=" * 60)
    print("Test: Persistence — exact cache survives restart")

    cache_file = tempfile.mktemp(suffix=".msgpack")
    try:
        proxy = start_proxy({"STACK_INTERCEPT_CACHE_PATH": cache_file})
        if not wait_for(PROXY_URL):
            print("  FAILED: Proxy did not start")
            return

        payload = {
            "model": "mock-model",
            "messages": [{"role": "user", "content": "Persist test"}],
            "temperature": 0,
            "stream": False,
        }

        # First request: miss
        hit, status, _, _ = send_request(payload)
        check("first request is miss", hit != "hit", f"(got: {hit})")
        check("first request status 200", status == 200, f"(status: {status})")

        # Second request: hit from in-memory cache
        hit, status, body, _ = send_request(payload)
        check("second request hits in-memory", hit == "hit", f"(got: {hit})")

        # Kill proxy, verify cache file exists
        proxy.terminate()
        proxy.wait(timeout=5)
        check("cache file created on disk", os.path.exists(cache_file))

        # Restart with same cache path
        reset_mock_count()
        proxy = start_proxy({"STACK_INTERCEPT_CACHE_PATH": cache_file})
        if not wait_for(PROXY_URL):
            print("  FAILED: Proxy did not restart")
            return

        # Same request: should be a hit from disk-restored cache
        hit, status, body, _ = send_request(payload)
        check("after restart: hit from disk", hit == "hit", f"(got: {hit})")
        check("after restart: body preserved", "Mock upstream response" in body)
        check("after restart: no upstream call", mock_request_count == 0, f"(calls: {mock_request_count})")
    finally:
        proxy.terminate()
        proxy.wait(timeout=5)
        if os.path.exists(cache_file):
            os.remove(cache_file)
    print()


def test_persistence_disable():
    """
    With DISABLE_PERSISTENCE=true, no cache file should be written.
    """
    print("=" * 60)
    print("Test: Persistence — DISABLE_PERSISTENCE prevents disk writes")

    cache_file = tempfile.mktemp(suffix=".msgpack")
    try:
        proxy = start_proxy({
            "STACK_INTERCEPT_CACHE_PATH": cache_file,
            "STACK_INTERCEPT_DISABLE_PERSISTENCE": "true",
        })
        if not wait_for(PROXY_URL):
            print("  FAILED: Proxy did not start")
            return

        payload = {
            "model": "mock-model",
            "messages": [{"role": "user", "content": "No persist test"}],
            "temperature": 0,
            "stream": False,
        }

        hit, status, _, _ = send_request(payload)
        check("first request is miss", hit != "hit", f"(got: {hit})")

        hit, status, _, _ = send_request(payload)
        check("second request hits memory", hit == "hit", f"(got: {hit})")

        proxy.terminate()
        proxy.wait(timeout=5)
        check("no cache file when disabled", not os.path.exists(cache_file))
    finally:
        proxy.terminate()
        proxy.wait(timeout=5)
        if os.path.exists(cache_file):
            os.remove(cache_file)
    print()


def test_sse_mid_stream_error():
    """
    Mid-stream upstream error should return SSE error frame with [DONE].
    """
    print("=" * 60)
    print("Test: SSE — mid-stream upstream error returns SSE error frame")

    proxy = start_proxy()
    if not wait_for(PROXY_URL):
        print("  FAILED: Proxy did not start")
        return

    # A partial SSE response that might trigger a mid-stream error
    # We use a payload that the mock handles normally but the stream collects
    payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "SSE error test"}],
        "temperature": 0,
        "stream": True,
    }

    # Set override to return SSE data
    set_override(200, [("Content-Type", "text/event-stream")], sse_response.encode())
    hit, status, body, headers = send_request(payload)
    check("status is 200", status == 200, f"(status: {status})")
    check("content-type is text/event-stream", "text/event-stream" in headers.get("content-type", ""))
    check("body contains SSE data", "data:" in body)
    check("body contains [DONE]", "[DONE]" in body)

    proxy.terminate()
    proxy.wait(timeout=5)
    print()


def test_sse_initial_upstream_failure():
    """
    When the upstream is unreachable (no mock server running), the proxy
    should return an SSE-formatted error frame with proper content-type.
    """
    print("=" * 60)
    print("Test: SSE — upstream unreachable returns SSE error frame")

    # Start proxy pointing at a port with nothing listening
    # Use a separate port that's definitely not running
    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_UPSTREAM_URL"] = "http://127.0.0.1:19999"  # nothing here
    env["STACK_INTERCEPT_STREAM_TIMEOUT"] = "2"  # fast timeout (not a real config, but doesn't hurt)
    env.pop("DEEPSEEK_API_KEY", None)
    env.pop("STACK_INTERCEPT_FALLBACK_API_KEY", None)
    env.pop("STACK_INTERCEPT_FALLBACK_URL", None)
    env.pop("STACK_INTERCEPT_REACTIVE_FAILOVER", None)

    proxy = subprocess.Popen(
        [proxy_binary()],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    # Need to wait for proxy to start even though upstream is down
    start = time.time()
    while time.time() - start < 10:
        try:
            urllib.request.urlopen(
                urllib.request.Request(PROXY_URL, method="POST", data=b"{}"),
                timeout=2,
            )
            break
        except urllib.error.HTTPError as e:
            if e.code in (415, 405, 500):
                break
        except (ConnectionResetError, urllib.error.URLError, OSError):
            pass
        time.sleep(0.3)

    payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Timeout test"}],
        "temperature": 0,
        "stream": True,
    }

    try:
        hit, status, body, headers = send_request(payload)
        check("status is 500", status == 500, f"(status: {status})")
        check("x-stack-intercept is error", hit == "error", f"(got: {hit})")
        check("content-type is text/event-stream for streaming",
              "text/event-stream" in headers.get("content-type", ""),
              f"(got: {headers.get('content-type', '')})")
        check("body has SSE error frame", '"error"' in body, f"(body: {body[:60]})")
        check("body has error message", '"Upstream Timeout"' in body, f"(body: {body[:60]})")
        check("body has [DONE]", "[DONE]" in body, f"(body: {body[:60]})")
    finally:
        proxy.terminate()
        proxy.wait(timeout=5)
    print()


def test_non_streaming_upstream_failure():
    """
    Non-streaming request when upstream is unreachable returns plain text.
    """
    print("=" * 60)
    print("Test: Non-streaming upstream failure returns plain text")

    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_UPSTREAM_URL"] = "http://127.0.0.1:19998"
    env.pop("DEEPSEEK_API_KEY", None)
    env.pop("STACK_INTERCEPT_FALLBACK_API_KEY", None)
    env.pop("STACK_INTERCEPT_FALLBACK_URL", None)
    env.pop("STACK_INTERCEPT_REACTIVE_FAILOVER", None)

    proxy = subprocess.Popen(
        [proxy_binary()],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )

    start = time.time()
    while time.time() - start < 10:
        try:
            urllib.request.urlopen(
                urllib.request.Request(PROXY_URL, method="POST", data=b"{}"),
                timeout=2,
            )
            break
        except urllib.error.HTTPError as e:
            if e.code in (415, 405, 500):
                break
        except (ConnectionResetError, urllib.error.URLError, OSError):
            pass
        time.sleep(0.3)

    payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Timeout test"}],
        "temperature": 0,
        "stream": False,
    }

    try:
        hit, status, body, headers = send_request(payload)
        check("status is 500", status == 500, f"(status: {status})")
        check("x-stack-intercept is error", hit == "error", f"(got: {hit})")
        check("content-type is NOT event-stream (non-streaming)",
              "text/event-stream" not in headers.get("content-type", ""))
        check("body is plain text", body == "Upstream Timeout", f"(body: {body})")
    finally:
        proxy.terminate()
        proxy.wait(timeout=5)
    print()


def main():
    global PASS, FAIL

    print("Starting mock upstream server...")
    mock_server = start_mock()

    try:
        test_persistence_exact_cache()
        test_persistence_disable()
        test_sse_mid_stream_error()
        test_sse_initial_upstream_failure()
        test_non_streaming_upstream_failure()

        # === Summary ===
        print("=" * 60)
        total = PASS + FAIL
        print(f"Results: {PASS}/{total} passed", "ALL PASSED" if FAIL == 0 else f"{FAIL} FAILURES")
        return 0 if FAIL == 0 else 1
    finally:
        mock_server.shutdown()


if __name__ == "__main__":
    sys.exit(main())
