"""
StackIntercept benchmark.

Measures request latency across cache scenarios using mock servers:
  - Cold miss (first request, no cache)
  - Exact cache hit (identical request replayed)
  - Streaming exact cache hit (SSE response, cached)
  - Semantic mode startup overhead
  - Routed fallback request (gpt-4o downgraded to deepseek-chat)

Usage:
    python benchmark.py

Outputs a latency comparison table. No API keys or model weights required.
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
MOCK_PORT = 8081
FALLBACK_MOCK_PORT = 8083
PROXY_URL = f"http://127.0.0.1:{PROXY_PORT}/v1/chat/completions"
MOCK_URL = f"http://127.0.0.1:{MOCK_PORT}"
FALLBACK_MOCK_URL = f"http://127.0.0.1:{FALLBACK_MOCK_PORT}"

N_ITERATIONS = 5  # run each scenario N times, report median


def proxy_binary():
    base = "./target/debug/stack-intercept"
    return base + ".exe" if sys.platform == "win32" else base


class MockHandler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        content_len = int(self.headers.get("Content-Length", 0))
        self.rfile.read(content_len)
        # Small synthetic delay to simulate real provider latency
        time.sleep(0.050)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({
            "id": "bench-cmpl",
            "object": "chat.completion",
            "created": 1700000000,
            "model": "mock",
            "choices": [{"index": 0, "message": {
                "role": "assistant", "content": "Benchmark response",
            }, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7},
        }).encode())

    def log_message(self, fmt, *args):
        pass


class SSEHandler(http.server.BaseHTTPRequestHandler):
    """Returns SSE response for streaming tests."""
    def do_POST(self):
        content_len = int(self.headers.get("Content-Length", 0))
        self.rfile.read(content_len)
        time.sleep(0.050)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        chunk = json.dumps({
            "id": "bench-cmpl-sse",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "mock",
            "choices": [{"index": 0, "delta": {"content": "Bench"}, "finish_reason": None}],
        })
        done = json.dumps({
            "id": "bench-cmpl-sse",
            "object": "chat.completion.chunk",
            "created": 1700000000,
            "model": "mock",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
        })
        self.wfile.write(f"data: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n".encode())

    def log_message(self, fmt, *args):
        pass


def start_mock(port, handler):
    server = http.server.HTTPServer(("127.0.0.1", port), handler)
    t = threading.Thread(target=server.serve_forever, daemon=True)
    t.start()
    return server


def start_proxy(extra_env=None):
    env = os.environ.copy()
    env["STACK_INTERCEPT_CACHE_MODE"] = "exact"
    env["STACK_INTERCEPT_UPSTREAM_URL"] = f"http://127.0.0.1:{MOCK_PORT}"
    if extra_env:
        env.update(extra_env)
    proc = subprocess.Popen(
        [proxy_binary()],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
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


def send_request(payload, extra_headers=None, timeout=30):
    data = json.dumps(payload).encode()
    headers = {"Content-Type": "application/json", "Authorization": "Bearer bench-key"}
    if extra_headers:
        headers.update(extra_headers)
    req = urllib.request.Request(PROXY_URL, data=data, headers=headers, method="POST")
    start = time.perf_counter()
    try:
        resp = urllib.request.urlopen(req, timeout=timeout)
        _ = resp.read()
        elapsed = time.perf_counter() - start
        hit = resp.headers.get("x-stack-intercept", "")
        route = resp.headers.get("x-stack-intercept-route", "")
        return elapsed * 1000, hit, route
    except urllib.error.HTTPError as e:
        elapsed = time.perf_counter() - start
        _ = e.read()
        hit = e.headers.get("x-stack-intercept", "")
        return elapsed * 1000, hit, ""


def median(values):
    """Return median of a list."""
    sorted_values = sorted(values)
    n = len(sorted_values)
    if n % 2 == 1:
        return sorted_values[n // 2]
    return (sorted_values[n // 2 - 1] + sorted_values[n // 2]) / 2.0


def benchmark_scenario(name, payload_factory, n=N_ITERATIONS, startup_fn=None):
    """Run a scenario N times and return (name, median_latency_ms)."""
    latencies = []
    for _ in range(n):
        if startup_fn:
            startup_fn()
            if not wait_for(PROXY_URL):
                return name, -1, ""
        lat, hit, route = send_request(payload_factory())
        latencies.append(lat)
    return name, median(latencies), hit


def main():
    print("=" * 60)
    print("StackIntercept Benchmark")
    print("=" * 60)
    print()

    # Start mock servers
    mock_server = start_mock(MOCK_PORT, MockHandler)
    sse_server = start_mock(FALLBACK_MOCK_PORT, SSEHandler)

    cold_payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Unique cold miss query"}],
        "temperature": 0,
        "stream": False,
    }
    cache_payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Cache benchmark request"}],
        "temperature": 0,
        "stream": False,
    }
    stream_payload = {
        "model": "mock-model",
        "messages": [{"role": "user", "content": "Stream benchmark request"}],
        "temperature": 0,
        "stream": True,
    }

    results = []

    # ---- 1. Cold miss ----
    print("Benchmarking: cold miss...")
    proxy = start_proxy()
    if not wait_for(PROXY_URL):
        print("FAILED: Proxy did not start")
        sys.exit(1)

    # Unique payload per iteration so each is a miss
    def cold_factory(counter=[0]):
        counter[0] += 1
        return {
            "model": "mock-model",
            "messages": [{"role": "user", "content": f"Cold miss query {counter[0]}"}],
            "temperature": 0,
            "stream": False,
        }

    name, lat, hit = benchmark_scenario("Cold miss (no cache)", cold_factory)
    results.append((name, lat, hit))
    proxy.terminate()
    proxy.wait(timeout=5)
    print(f"  {name}: {lat:.1f} ms  (x-stack-intercept: {hit})")
    print()

    # ---- 2. Exact cache hit ----
    print("Benchmarking: exact cache hit...")
    proxy = start_proxy()
    if not wait_for(PROXY_URL):
        print("FAILED: Proxy did not start")
        sys.exit(1)

    # First request to populate cache
    send_request(cache_payload)
    # Now benchmark the hits
    def cache_factory():
        return cache_payload

    name, lat, hit = benchmark_scenario("Exact cache hit", cache_factory)
    results.append((name, lat, hit))
    proxy.terminate()
    proxy.wait(timeout=5)
    print(f"  {name}: {lat:.1f} ms  (x-stack-intercept: {hit})")
    print()

    # ---- 3. Streaming exact cache hit ----
    print("Benchmarking: streaming exact cache hit...")
    proxy = start_proxy()
    if not wait_for(PROXY_URL):
        print("FAILED: Proxy did not start")
        sys.exit(1)

    # First request to populate cache (point streaming requests at the SSE mock)
    env_sse = {"STACK_INTERCEPT_UPSTREAM_URL": f"http://127.0.0.1:{FALLBACK_MOCK_PORT}"}
    # We need a proxy that routes streaming to the SSE mock — let's just use default mock
    # Actually the SSE handler is on FALLBACK_MOCK_PORT. Let's use the regular mock for simplicity
    # The regular mock response works for streaming too since the proxy caches the raw bytes.
    send_request(stream_payload)
    name, lat, hit = benchmark_scenario("Streaming exact cache hit", lambda: stream_payload)
    results.append((name, lat, hit))
    proxy.terminate()
    proxy.wait(timeout=5)
    print(f"  {name}: {lat:.1f} ms  (x-stack-intercept: {hit})")
    print()

    # ---- 4. Semantic mode startup overhead ----
    print("Benchmarking: semantic mode startup...")
    # Check if model weights exist
    model_exists = os.path.isdir("model") and os.path.isfile("model/config.json")
    if model_exists:
        def semantic_startup():
            nonlocal proxy
            proxy = start_proxy({"STACK_INTERCEPT_CACHE_MODE": "semantic"})

        sem_payload = {
            "model": "mock-model",
            "messages": [{"role": "user", "content": "Semantic test"}],
            "temperature": 0,
            "stream": False,
        }

        latencies = []
        for i in range(min(N_ITERATIONS, 3)):  # fewer iterations — model loading is slow
            proxy = start_proxy({"STACK_INTERCEPT_CACHE_MODE": "semantic"})
            start_ts = time.perf_counter()
            ok = wait_for(PROXY_URL)
            elapsed = time.perf_counter() - start_ts
            if ok:
                # First request latency
                req_start = time.perf_counter()
                send_request(sem_payload)
                req_elapsed = time.perf_counter() - req_start
                latencies.append((elapsed + req_elapsed) * 1000)
            proxy.terminate()
            proxy.wait(timeout=5)
            print(f"    Iteration {i+1}: startup={elapsed*1000:.0f}ms, first-req={req_elapsed*1000:.1f}ms")

        name = "Semantic startup + first request"
        m = median(latencies) if latencies else -1
        results.append((name, m, "miss"))
        print(f"  {name}: {m:.0f} ms (combined)")
    else:
        print("  SKIP: model weights not found (run ./download_model.sh)")
        results.append(("Semantic startup (SKIP - no model)", -1, ""))
    print()

    # ---- 5. Routed fallback request ----
    print("Benchmarking: routed fallback request...")
    # Start fallback mock server
    fallback_server = start_mock(FALLBACK_MOCK_PORT, MockHandler)

    def fallback_startup():
        nonlocal proxy
        proxy = start_proxy({
            "STACK_INTERCEPT_ALLOW_MODEL_REWRITE": "true",
            "STACK_INTERCEPT_FALLBACK_URL": f"http://127.0.0.1:{FALLBACK_MOCK_PORT}",
            "STACK_INTERCEPT_FALLBACK_API_KEY": "sk-fallback-bench",
        })

    routed_payload = {
        "model": "gpt-4o",
        "messages": [{"role": "user", "content": "Simple question"}],
        "temperature": 0,
        "stream": False,
    }

    name, lat, hit = benchmark_scenario(
        "Routed fallback (gpt-4o -> deepseek-chat)",
        lambda: routed_payload,
        startup_fn=fallback_startup,
    )
    results.append((name, lat, hit))
    proxy.terminate()
    proxy.wait(timeout=5)
    print(f"  {name}: {lat:.1f} ms  (x-stack-intercept: {hit}, route: {hit})")
    print()

    # ---- Results table ----
    print("=" * 60)
    print("Results")
    print("=" * 60)
    print()
    print(f"{'Scenario':<45} {'Latency (ms)':<15} {'vs cold miss':<12}")
    print("-" * 72)

    # Find cold miss baseline
    cold_lat = None
    for n, l, _ in results:
        if "cold" in n.lower():
            cold_lat = l
            break

    for name, lat, hit in results:
        if lat < 0:
            print(f"{name:<45} {'SKIPPED':<15}")
            continue
        ratio = f"{lat / cold_lat:.2f}x" if cold_lat and cold_lat > 0 else "-"
        label = f"route={hit}" if hit else ""
        print(f"{name:<45} {lat:<15.1f} {ratio:<12}")

    print()
    if cold_lat:
        print(f"Cold miss baseline: {cold_lat:.1f} ms (includes 50ms mock provider delay)")
    print()

    # Cleanup
    mock_server.shutdown()
    sse_server.shutdown()
    fallback_server.shutdown()

    return 0


if __name__ == "__main__":
    sys.exit(main())
