"""
StackIntercept Reactive Failover Integration Tests.

Starts mock servers for primary and fallback endpoints, starts the proxy,
and verifies correct failover behavior under different scenarios.
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


def assert_eq(actual, expected, msg):
    global PASS, FAIL
    if actual == expected:
        print(f"  [PASS] {msg}")
        PASS += 1
    else:
        print(f"  [FAIL] {msg} (expected {expected}, got {actual})")
        FAIL += 1


class RequestCapture:
    def __init__(self, method, path, headers, body):
        self.method = method
        self.path = path
        self.headers = headers
        self.body = body


class MockServer:
    def __init__(self, port, response_data, status_code=200):
        self.port = port
        self.response_data = response_data
        self.status_code = status_code
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
                self.send_response(owner.status_code)
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


upstream_response = json.dumps({"id": "primary-01", "choices": [{"message": {"content": "Primary success"}}]}).encode()
fallback_response = json.dumps({"id": "fallback-01", "choices": [{"message": {"content": "Fallback success"}}]}).encode()


def proxy_binary():
    base = "./target/debug/stack-intercept"
    return base + ".exe" if sys.platform == "win32" else base


def start_proxy(extra_env=None):
    env = os.environ.copy()
    env["STACK_INTERCEPT_UPSTREAM_URL"] = MOCK_UPSTREAM_URL
    env["STACK_INTERCEPT_FALLBACK_URL"] = MOCK_FALLBACK_URL
    env["STACK_INTERCEPT_FALLBACK_API_KEY"] = "sk-fallback-test"
    env["STACK_INTERCEPT_DISABLE_PERSISTENCE"] = "true"
    env["STACK_INTERCEPT_REACTIVE_FAILOVER"] = "true"
    if extra_env:
        env.update(extra_env)
    
    proc = subprocess.Popen(
        [proxy_binary()],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    return proc


def wait_for_proxy(timeout=10):
    start = time.time()
    while time.time() - start < timeout:
        try:
            req = urllib.request.Request(f"http://127.0.0.1:{PROXY_PORT}/admin/config", method="GET")
            urllib.request.urlopen(req, timeout=1)
            return True
        except Exception:
            time.sleep(0.2)
    return False


def get_metrics():
    try:
        req = urllib.request.Request(f"http://127.0.0.1:{PROXY_PORT}/admin/metrics", method="GET")
        resp = urllib.request.urlopen(req, timeout=2)
        return json.loads(resp.read().decode())
    except Exception as e:
        print(f"Failed to fetch metrics: {e}")
        return {}


def main():
    print("=" * 60)
    print("StackIntercept Reactive Failover Integration Tests")
    print("=" * 60)
    print()

    # Start mock servers
    upstream_server = MockServer(MOCK_UPSTREAM_PORT, upstream_response, status_code=500)
    fallback_server = MockServer(MOCK_FALLBACK_PORT, fallback_response)
    upstream_server.start()
    fallback_server.start()

    print("Test 1: Failover on 500 Internal Server Error")
    proxy = start_proxy()
    if not wait_for_proxy():
        print("FAILED: Proxy did not start")
        sys.exit(1)

    payload = {
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "Hello failover"}],
        "temperature": 0,
    }
    
    # Send request through proxy
    try:
        req = urllib.request.Request(
            PROXY_URL,
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json", "Authorization": "Bearer sk-primary-key"},
            method="POST"
        )
        resp = urllib.request.urlopen(req, timeout=5)
        res_data = json.loads(resp.read().decode())
        
        # Verify response matches fallback
        assert_eq(res_data["id"], "fallback-01", "Response should be served by fallback mock")
        
        # Verify route headers
        route = resp.headers.get("x-stack-intercept-route")
        assert_eq(route, "fallback", "Route header should report fallback")
        
        # Verify metrics
        metrics = get_metrics()
        assert_eq(metrics.get("reactive_failovers"), 1, "reactive_failovers counter should increment to 1")
    except Exception as e:
        print(f"  [FAIL] Request failed: {e}")
        global FAIL
        FAIL += 1

    proxy.terminate()
    proxy.wait(timeout=5)
    upstream_server.reset()
    fallback_server.reset()
    print()

    print("Test 2: Failover on connection refuse")
    # Point upstream URL to an invalid/offline port
    proxy = start_proxy({"STACK_INTERCEPT_UPSTREAM_URL": "http://127.0.0.1:8077"})
    if not wait_for_proxy():
        print("FAILED: Proxy did not start")
        sys.exit(1)

    try:
        req = urllib.request.Request(
            PROXY_URL,
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json", "Authorization": "Bearer sk-primary-key"},
            method="POST"
        )
        resp = urllib.request.urlopen(req, timeout=5)
        res_data = json.loads(resp.read().decode())
        
        # Verify response matches fallback
        assert_eq(res_data["id"], "fallback-01", "Response should be served by fallback mock")
        
        # Verify route headers
        route = resp.headers.get("x-stack-intercept-route")
        assert_eq(route, "fallback", "Route header should report fallback")
        
        # Verify metrics
        metrics = get_metrics()
        assert_eq(metrics.get("reactive_failovers"), 1, "reactive_failovers counter should increment to 1")
    except Exception as e:
        print(f"  [FAIL] Request failed: {e}")
        FAIL += 1

    proxy.terminate()
    proxy.wait(timeout=5)
    fallback_server.reset()
    print()

    print("Test 3: Model rewrite on failover")
    proxy = start_proxy({
        "STACK_INTERCEPT_UPSTREAM_URL": "http://127.0.0.1:8077",
        "STACK_INTERCEPT_FAILOVER_MODEL": "gpt-4o-mini-failover"
    })
    if not wait_for_proxy():
        print("FAILED: Proxy did not start")
        sys.exit(1)

    try:
        req = urllib.request.Request(
            PROXY_URL,
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json", "Authorization": "Bearer sk-primary-key"},
            method="POST"
        )
        resp = urllib.request.urlopen(req, timeout=5)
        json.loads(resp.read().decode())
        
        # Check what model was sent to fallback mock
        last_req = fallback_server.last_request
        fallback_payload = json.loads(last_req.body.decode())
        assert_eq(fallback_payload["model"], "gpt-4o-mini-failover", "Model name should be rewritten to failover model")
        
        # Verify routed model header
        routed_hdr = resp.headers.get("x-stack-intercept-routed-model")
        assert_eq(routed_hdr, "gpt-4o-mini-failover", "Routed model header should be set to failover model")
    except Exception as e:
        print(f"  [FAIL] Request failed: {e}")
        FAIL += 1

    proxy.terminate()
    proxy.wait(timeout=5)
    fallback_server.reset()
    print()

    print("Test 4: Disable failover")
    # Disable failover using env var
    proxy = start_proxy({
        "STACK_INTERCEPT_UPSTREAM_URL": "http://127.0.0.1:8077",
        "STACK_INTERCEPT_REACTIVE_FAILOVER": "false"
    })
    if not wait_for_proxy():
        print("FAILED: Proxy did not start")
        sys.exit(1)

    try:
        req = urllib.request.Request(
            PROXY_URL,
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json", "Authorization": "Bearer sk-primary-key"},
            method="POST"
        )
        urllib.request.urlopen(req, timeout=5)
        print("  [FAIL] Request succeeded but should have failed since failover was disabled")
        FAIL += 1
    except urllib.error.HTTPError as e:
        assert_eq(e.code, 500, "Should return 500 Internal Server Error when failover is disabled")
        
        # Verify metrics
        metrics = get_metrics()
        assert_eq(metrics.get("reactive_failovers"), 0, "reactive_failovers counter should remain 0")
    except Exception as e:
        print(f"  [FAIL] Unexpected error: {e}")
        FAIL += 1

    proxy.terminate()
    proxy.wait(timeout=5)
    print()

    # Shutdown mock servers
    upstream_server.shutdown()
    fallback_server.shutdown()

    print("=" * 60)
    print(f"Tests finished: {PASS} passed, {FAIL} failed")
    print("=" * 60)
    
    if FAIL > 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
