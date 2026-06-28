"""
Semantic cache hit verification.
Verifies that identical prompts within the same context produce cache hits.

Set STACK_INTERCEPT_CACHE_MODE=semantic before running.
Uses the `requests` library for accurate HTTP timing and header inspection.
"""

import os
import sys
import io
import json
import time
import requests

# Handle Windows console encoding
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')

api_key = os.environ.get("OPENAI_API_KEY")
if not api_key:
    print("ERROR: Set OPENAI_API_KEY environment variable")
    exit(1)

PROXY_URL = "http://127.0.0.1:8080/v1/chat/completions"
HEADERS = {
    "Content-Type": "application/json",
    "Authorization": f"Bearer {api_key}",
}
MODEL = "deepseek-chat"

PASS = 0
FAIL = 0


def check(name, cond, detail=""):
    global PASS, FAIL
    if cond:
        PASS += 1
        print(f"  PASS: {name} {detail}")
    else:
        FAIL += 1
        print(f"  FAIL: {name} {detail}")


def send_nonstream(payload):
    """Send a non-streaming request and return (elapsed, status, headers, body_text)."""
    start = time.time()
    r = requests.post(PROXY_URL, json=payload, headers=HEADERS)
    elapsed = time.time() - start
    return elapsed, r.status_code, r.headers, r.text


def collect_sse_chunks(payload, max_wait=30):
    """Stream a request and collect all SSE data chunks with timing.

    Returns (first_chunk_time, total_time, all_chunks_concatenated, response_headers).
    """
    start = time.time()
    r = requests.post(PROXY_URL, json=payload, headers=HEADERS, stream=True)
    first_chunk_time = None
    chunks = []

    for chunk in r.iter_lines(decode_unicode=True):
        if first_chunk_time is None:
            first_chunk_time = time.time() - start
        if chunk:
            chunks.append(chunk)

    total_time = time.time() - start
    return first_chunk_time, total_time, "\n".join(chunks), r.headers


# ============================================================
# TEST 1: Non-streaming exact cache hit
# ============================================================
print("=" * 60)
print("Test 1: Non-streaming — exact cache hit")

payload1 = {
    "model": MODEL,
    "messages": [{"role": "user", "content": "Say 'hello world'"}],
    "temperature": 0,
    "stream": False,
}

elapsed1, _, h1, body1 = send_nonstream(payload1)
print(f"  First request (cold): {elapsed1:.4f}s")

elapsed2, _, h2, body2 = send_nonstream(payload1)
print(f"  Second request:       {elapsed2:.4f}s")

check("status", "hit" in h2.get("x-stack-intercept", ""),
      f"(x-stack-intercept: {h2.get('x-stack-intercept', 'N/A')})")
check("timing", elapsed2 < 0.1, f"({elapsed2:.4f}s vs 0.1s threshold)")
check("same body", body1 == body2, "(identical response)")
print()


# ============================================================
# TEST 2: Streaming exact cache hit
# ============================================================
print("=" * 60)
print("Test 2: Streaming — exact cache hit")

payload2 = {
    "model": MODEL,
    "messages": [{"role": "user", "content": "Say 'OK' in one word"}],
    "stream": True,
}

first_t, total_t, content2a, h2a = collect_sse_chunks(payload2)
print(f"  First request (cold):  first_chunk={first_t:.4f}s, total={total_t:.4f}s")

first_t2, total_t2, content2b, h2b = collect_sse_chunks(payload2)
print(f"  Second request:        first_chunk={first_t2:.4f}s, total={total_t2:.4f}s")

check("status header", "hit" in h2b.get("x-stack-intercept", ""),
      f"(x-stack-intercept: {h2b.get('x-stack-intercept', 'N/A')})")
check("first chunk fast", first_t2 < 0.1 or "hit" in h2b.get("x-stack-intercept", ""),
      f"(first_chunk={first_t2:.4f}s, x-stack-intercept: {h2b.get('x-stack-intercept', 'N/A')})")
check("same content", content2a == content2b, "(identical SSE output)")
print()


# ============================================================
# TEST 3: Non-streaming semantic cache hit with system prompt
# ============================================================
print("=" * 60)
print("Test 3: Non-streaming — semantic cache hit (with system prompt)")

payload3 = {
    "model": MODEL,
    "messages": [
        {"role": "system", "content": "You are a helpful tutor."},
        {"role": "user", "content": "Explain what an API is in one sentence."},
    ],
    "temperature": 0,
    "stream": False,
}

elapsed3, _, h3, body3 = send_nonstream(payload3)
print(f"  First request (cold): {elapsed3:.4f}s")

elapsed4, _, h4, body4 = send_nonstream(payload3)
print(f"  Second request:       {elapsed4:.4f}s")

check("status header", "hit" in h4.get("x-stack-intercept", ""),
      f"(x-stack-intercept: {h4.get('x-stack-intercept', 'N/A')})")
check("timing", elapsed4 < 0.1, f"({elapsed4:.4f}s)")
check("same body", body3 == body4, "(identical response)")
print()


# ============================================================
# Summary
# ============================================================
print("=" * 60)
total = PASS + FAIL
print(f"Results: {PASS}/{total} passed", "ALL PASSED" if FAIL == 0 else f"{FAIL} FAILURES")

if FAIL > 0:
    exit(1)
