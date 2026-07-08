"""
Mock upstream integration test for StackIntercept.

Starts a mock HTTP server to act as the upstream LLM provider, starts the
StackIntercept proxy pointing at the mock, sends requests and verifies
exact cache hit/miss behavior — all without API keys or model weights.
"""

from http.client import HTTPMessage
import hashlib
import http.server
import json
import os
import signal
import subprocess
import sys
import threading
import time
import urllib.error
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
    # Class-level one-shot override. Set via MockHandler.set_override().
    # Consumed on first do_POST call.
    override = None
    override_lock = threading.Lock()

    @classmethod
    def set_override(cls, status, headers_list, body):
        """Set a one-shot override for the next mock request."""
        with cls.override_lock:
            cls.override = (status, headers_list, body)

    def do_POST(self):
        global mock_request_count
        content_len = int(self.headers.get("Content-Length", 0))
        self.rfile.read(content_len)

        # Check one-shot override (class-level)
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
        pass  # Suppress mock server logs


def set_override(status, headers_list, body):
    """Set a one-shot override for the next mock request."""
    MockHandler.set_override(status, headers_list, body)


def start_mock():
    server = http.server.HTTPServer(("127.0.0.1", MOCK_PORT), MockHandler)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return server


def proxy_binary():
    """Return the proxy binary path, with .exe on Windows."""
    base = "./target/debug/stack-intercept"
    return base + ".exe" if sys.platform == "win32" else base


def start_proxy(extra_env=None):
    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_UPSTREAM_URL"] = f"http://127.0.0.1:{MOCK_PORT}"
    env["STACK_INTERCEPT_DISABLE_PERSISTENCE"] = "true"
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
            # 415/405 means server is up and receiving (just wrong method/body for warmup)
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
        return hit_flag, resp.status, body
    except urllib.error.HTTPError as e:
        hit_flag = e.headers.get("x-stack-intercept", "")
        return hit_flag, e.code, e.read().decode()


def send_admin_get(path, extra_headers=None):
    """Send a GET request to an admin endpoint."""
    url = f"http://127.0.0.1:{PROXY_PORT}/admin{path}"
    headers = {}
    if extra_headers:
        headers.update(extra_headers)
    req = urllib.request.Request(url, headers=headers, method="GET")
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        body = json.loads(resp.read().decode())
        return resp.status, body, resp.headers
    except urllib.error.HTTPError as e:
        body = json.loads(e.read().decode())
        return e.code, body, e.headers


def send_admin_delete(path, extra_headers=None):
    """Send a DELETE request to an admin endpoint."""
    url = f"http://127.0.0.1:{PROXY_PORT}/admin{path}"
    headers = {}
    if extra_headers:
        headers.update(extra_headers)
    req = urllib.request.Request(url, data=b"", headers=headers, method="DELETE")
    try:
        resp = urllib.request.urlopen(req, timeout=15)
        body = json.loads(resp.read().decode())
        return resp.status, body, resp.headers
    except urllib.error.HTTPError as e:
        body = json.loads(e.read().decode())
        return e.code, body, e.headers


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS: {name} {detail}")
    else:
        FAIL += 1
        print(f"  FAIL: {name} {detail}")


def run_basic_tests():
    """Tests 1-4: basic exact cache hit/miss behavior."""
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


def run_non2xx_test():
    """Test 5: non-2xx upstream response is not cached."""
    print("=" * 60)
    print("Test 5: Non-2xx upstream not cached")

    err_payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Error test"}],
        "temperature": 0,
        "stream": False,
    }

    # First: mock returns 500
    set_override(500, [("Content-Type", "application/json")], b'{"error":"server error"}')
    hit, status, body = send_request(err_payload)
    check("miss on 500", hit != "hit", f"(got: {hit})")
    check("status is 500", status == 500, f"(status: {status})")
    check("body has error", '"server error"' in body, f"(body: {body[:50]})")

    # Second: same request, no override -> mock returns 200 (nothing was cached)
    hit, status, body = send_request(err_payload)
    check("miss on retry (500 was not cached)", hit != "hit", f"(got: {hit})")
    check("status is 200 now", status == 200, f"(status: {status})")
    check("body is mock response", "Mock upstream response" in body)

    # Third: now it's cached from the 200
    hit, status, _ = send_request(err_payload)
    check("hit on third request", hit == "hit", f"(got: {hit})")
    print()


def run_streaming_test():
    """Test 6: streaming passthrough preserves status + caching."""
    print("=" * 60)
    print("Test 6: Streaming cache hit/miss")

    # SSE-formatted mock response
    sse_chunk = json.dumps({
        "id": "mock-cmpl-sse",
        "object": "chat.completion.chunk",
        "created": 1700000000,
        "model": "mock-model",
        "choices": [{"index": 0, "delta": {"content": "Mock"}, "finish_reason": None}],
    })
    sse_body = f"data: {sse_chunk}\n\ndata: [DONE]\n\n".encode()

    stream_payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Stream test"}],
        "temperature": 0,
        "stream": True,
    }

    # Set override to return SSE content
    set_override(200, [("Content-Type", "text/event-stream")], sse_body)
    hit, status, body = send_request(stream_payload)
    check("streaming: miss on first request", hit != "hit", f"(got: {hit})")
    check("streaming: status is 200", status == 200, f"(status: {status})")
    check("streaming: body is SSE", "data:" in body, f"(body preview: {body[:40]})")

    # Second identical streaming request -> cache hit
    hit, status, body = send_request(stream_payload)
    check("streaming: hit on second request", hit == "hit", f"(got: {hit})")
    check("streaming: status preserved on hit", status == 200, f"(status: {status})")
    check("streaming: body preserved on hit", "data:" in body, f"(body preview: {body[:40]})")
    print()


def run_tenant_test():
    """Test 7: tenant header isolation."""
    print("=" * 60)
    print("Test 7: Tenant header separation")

    tenant_payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Tenant test"}],
        "temperature": 0,
        "stream": False,
    }

    t1_headers = {"X-Tenant-Id": "tenant-alpha"}
    t2_headers = {"X-Tenant-Id": "tenant-beta"}

    # First request with tenant-alpha -> miss
    hit, status, _ = send_request(tenant_payload, extra_headers=t1_headers)
    check("tenant: alpha first request miss", hit != "hit", f"(got: {hit})")
    check("tenant: alpha status 200", status == 200, f"(status: {status})")

    # Second request with tenant-alpha -> hit (same tenant)
    hit, status, _ = send_request(tenant_payload, extra_headers=t1_headers)
    check("tenant: alpha second request hit", hit == "hit", f"(got: {hit})")

    # First request with tenant-beta -> miss (different tenant)
    hit, status, _ = send_request(tenant_payload, extra_headers=t2_headers)
    check("tenant: beta first request miss (isolated)", hit != "hit", f"(got: {hit})")

    # Second request with tenant-beta -> hit
    hit, status, _ = send_request(tenant_payload, extra_headers=t2_headers)
    check("tenant: beta second request hit", hit == "hit", f"(got: {hit})")
    print()


def run_admin_tests():
    """Tests for admin routes: metrics, cache summary, eviction."""
    print("=" * 60)
    print("Test 8: GET /admin/metrics — zero state")
    status, body, _ = send_admin_get("/metrics")
    check("admin/metrics status 200", status == 200, f"(status: {status})")
    check("admin/metrics has uptime_secs", "uptime_secs" in body)
    check("admin/metrics exact_hits is 0", body.get("exact_hits") == 0)
    check("admin/metrics semantic_hits is 0", body.get("semantic_hits") == 0)
    check("admin/metrics misses is 0", body.get("misses") == 0)
    print()

    print("=" * 60)
    print("Test 9: GET /admin/cache — zero cache")
    status, body, _ = send_admin_get("/cache")
    check("admin/cache status 200", status == 200, f"(status: {status})")
    check("admin/cache has exact.entries", body["exact"]["entries"] == 0)
    check("admin/cache has exact.max_entries", body["exact"]["max_entries"] == 20000)
    check("admin/cache has semantic.entries", body["semantic"]["entries"] == 0)
    check("admin/cache has semantic.buckets", body["semantic"]["buckets"] == 0)
    print()

    print("=" * 60)
    print("Test 10: Metrics after cache hit/miss")
    payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Admin metrics test"}],
        "temperature": 0,
        "stream": False,
    }
    # Miss
    send_request(payload)
    # Hit
    send_request(payload)

    time.sleep(1.5)  # Ensure uptime_secs ticks past 0

    status, body, _ = send_admin_get("/metrics")
    check("exact_hits >= 1 after hit", body.get("exact_hits") >= 1)
    check("misses >= 1 after miss", body.get("misses") >= 1)
    check("uptime_secs > 0", body.get("uptime_secs", 0) > 0)
    print()

    print("=" * 60)
    print("Test 11: GET /admin/cache — after cache insert")
    status, body, _ = send_admin_get("/cache")
    check("admin/cache exact.entries >= 1", body["exact"]["entries"] >= 1)
    print()

    print("=" * 60)
    print("Test 12: DELETE /admin/cache — flush all caches")
    status, body, _ = send_admin_delete("/cache")
    check("flush status 200", status == 200, f"(status: {status})")
    check("flush exact.entries is 0", body["exact"]["entries"] == 0)
    check("flush semantic.entries is 0", body["semantic"]["entries"] == 0)

    # Verify caches are empty
    status, body, _ = send_admin_get("/cache")
    check("cache exact.entries is 0 after flush", body["exact"]["entries"] == 0)

    # The same request should miss again (cache was cleared)
    hit, status, _ = send_request(payload)
    check("miss after flush", hit != "hit", f"(got: {hit})")
    print()

    print("=" * 60)
    print("Test 13: DELETE /admin/cache/exact/:key")
    # Make a cacheable request
    payload2 = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Delete me"}],
        "temperature": 0,
    }
    send_request(payload2)
    hit, _, _ = send_request(payload2)  # Should be hit
    check("hit before key deletion", hit == "hit", f"(got: {hit})")

    # Test with a nonexistent key to verify the endpoint works
    status, body, _ = send_admin_delete("/cache/exact/nonexistentkey")
    check("delete nonexistent key returns removed: false", body.get("removed") == False)
    print()

    print("=" * 60)
    print("Test 14: DELETE /admin/cache/exact/:key removes a real cached key")
    # Compute the exact cache key hash that the Rust proxy uses,
    # then delete it and verify the cached entry is removed.
    key_payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Key deletion test"}],
        "temperature": 0,
    }
    computed_hash = compute_exact_cache_key(
        key_payload,
        upstream_url=f"http://127.0.0.1:{MOCK_PORT}",
    )

    # First request: miss
    hit, status, _ = send_request(key_payload)
    check("first request is miss", hit != "hit", f"(got: {hit})")

    # Second request: hit (cached)
    hit, _, _ = send_request(key_payload)
    check("second request is hit (cached)", hit == "hit", f"(got: {hit})")

    # Delete by computed hash
    status, body, _ = send_admin_delete(f"/cache/exact/{computed_hash}")
    check("delete returns status 200", status == 200, f"(status: {status})")
    check("delete removed: true", body.get("removed") == True,
          f"(got removed={body.get('removed')}, hash={computed_hash[:16]}...)")

    # Should miss again (was deleted)
    hit, _, _ = send_request(key_payload)
    check("miss after exact key deletion", hit != "hit", f"(got: {hit})")

    # Next request should hit (re-cached)
    hit, _, _ = send_request(key_payload)
    check("hit after re-caching", hit == "hit", f"(got: {hit})")
    print()

    print("=" * 60)
    print("Test 15: GET /admin/config — returns 200 with expected keys")
    status, body, _ = send_admin_get("/config")
    check("admin/config status 200", status == 200, f"(status: {status})")
    for key in ("cache_mode", "upstream_base_url", "exact_max_entries", "admin_key"):
        check(f"admin/config has key '{key}'", key in body, f"(got keys: {list(body.keys())})")
    print()

    print("=" * 60)
    print("Test 16: GET /admin/config — secrets are masked")
    status, body, _ = send_admin_get("/config")
    # No admin key configured in test, so it should be null.
    # When configured, it should be "********".
    check("admin/config admin_key is null (no key configured)",
          body.get("admin_key") is None,
          f"(got: {body.get('admin_key')!r})")
    check("admin/config fallback_api_key is masked or null",
          body.get("fallback_api_key") is None or body["fallback_api_key"].endswith("*****"),
          f"(got: {body.get('fallback_api_key')!r})")
    check("admin/config cache_mode is lowercase string",
          isinstance(body.get("cache_mode"), str) and body["cache_mode"] == "exact",
          f"(got: {body.get('cache_mode')!r})")
    print()


def compute_exact_cache_key(payload, upstream_url=None, tenant_id=None):
    """Replicate the Rust cache_key_hash() computation for exact cache lookups.

    The Rust side computes:
        SHA256(extract_hostname(upstream_url) + tenant_id + routing_namespace + canonical_json(payload))

    For passthrough (no routing), routing_namespace is:
        v1|passthrough|<host:port>|<model>

    The payload must be cache-eligible (temperature=0, no tools, etc.).
    """
    hostname = upstream_url.replace("https://", "").replace("http://", "").split("/")[0]
    model = payload.get("model", "mock-model")
    routing_namespace = f"v1|passthrough|{hostname}|{model}"

    hash_input = hostname
    if tenant_id:
        hash_input += tenant_id
    hash_input += routing_namespace
    hash_input += json.dumps(payload, sort_keys=True, separators=(",", ":"))

    return hashlib.sha256(hash_input.encode()).hexdigest()


def reset_mock_count():
    global mock_request_count
    with mock_request_lock:
        mock_request_count = 0


def main():
    global PASS, FAIL, mock_request_count

    print("Starting mock upstream server...")
    mock_server = start_mock()

    print("Starting StackIntercept proxy...")
    proxy = start_proxy()

    try:
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy did not start")
            sys.exit(1)
        print("  Proxy is online.\n")

        run_basic_tests()
        run_non2xx_test()
        run_streaming_test()

        # Restart proxy with tenant header config for isolation test
        proxy.terminate()
        proxy.wait(timeout=5)
        reset_mock_count()

        proxy = start_proxy({"STACK_INTERCEPT_TENANT_ID_HEADER": "X-Tenant-Id"})
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy (tenant config) did not start")
            sys.exit(1)

        run_tenant_test()

        # Restart proxy without tenant header for admin tests
        proxy.terminate()
        proxy.wait(timeout=5)
        reset_mock_count()

        proxy = start_proxy()  # basic config, no extra env vars
        if not wait_for(PROXY_URL):
            print("FAILED: Proxy did not start for admin tests")
            sys.exit(1)

        run_admin_tests()

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
