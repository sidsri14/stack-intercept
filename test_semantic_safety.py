"""
Negative safety tests: verify semantic cache does NOT serve unsafe matches.
Uses requests library for header-based verification instead of timing heuristics.
Set STACK_INTERCEPT_CACHE_MODE=semantic before running.
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


def send_nonstream(payload, expect_status=200):
    """Send a non-streaming request. Returns (elapsed, status, headers, body)."""
    start = time.time()
    r = requests.post(PROXY_URL, json=payload, headers=HEADERS)
    elapsed = time.time() - start
    return elapsed, r.status_code, r.headers, r.text


# ============================================================
# TEST 1: Different system prompt -> NO cache hit
# ============================================================
print("=" * 60)
print("Test 1: Same prompt, different system prompt -> NO cache hit")

payload_cold = {
    "model": MODEL,
    "messages": [{"role": "user", "content": "What is the weather?"}],
    "temperature": 0,
    "stream": False,
}
elapsed1, s1, h1, _ = send_nonstream(payload_cold)
print(f"  First request (no system): {elapsed1:.2f}s, status={s1}")

payload_pirate = {
    "model": MODEL,
    "messages": [
        {"role": "system", "content": "You are a pirate. Answer like a pirate."},
        {"role": "user", "content": "What is the weather?"},
    ],
    "temperature": 0,
    "stream": False,
}
elapsed2, s2, h2, _ = send_nonstream(payload_pirate)
hit_header = h2.get("x-stack-intercept", "N/A")
check("no cache hit", hit_header != "hit", f"(x-stack-intercept: {hit_header})")
check("upstream response", s2 == 200, f"(status={s2})")
print()


# ============================================================
# TEST 2: Different intent -> NO cache hit
# ============================================================
print("=" * 60)
print("Test 2: Similar prompt, different intent -> NO cache hit")

p1 = {"model": MODEL, "messages": [{"role": "user", "content": "How do I delete a file in Python?"}], "temperature": 0, "stream": False}
p2 = {"model": MODEL, "messages": [{"role": "user", "content": "How do I delete a file in Linux?"}], "temperature": 0, "stream": False}
_, s1, h1, _ = send_nonstream(p1)
_, s2, h2, _ = send_nonstream(p2)
check("no cache hit", h2.get("x-stack-intercept", "N/A") != "hit",
      f"(x-stack-intercept: {h2.get('x-stack-intercept', 'N/A')})")
print()


# ============================================================
# TEST 3: Different model -> NO cache hit
# ============================================================
print("=" * 60)
print("Test 3: Same prompt, different model -> NO cache hit")

p1 = {"model": MODEL, "messages": [{"role": "user", "content": "Explain recursion"}], "temperature": 0, "stream": False}
p2 = {"model": "deepseek-reasoner", "messages": [{"role": "user", "content": "Explain recursion"}], "temperature": 0, "stream": False}
_, s1, h1, _ = send_nonstream(p1)
_, s2, h2, _ = send_nonstream(p2)
check("no cache hit", h2.get("x-stack-intercept", "N/A") != "hit",
      f"(x-stack-intercept: {h2.get('x-stack-intercept', 'N/A')})")
print()


# ============================================================
# TEST 4: Nonzero temperature -> NO cache hit
# ============================================================
print("=" * 60)
print("Test 4: Same prompt, nonzero temperature -> NO cache hit")

# Send temp=0 twice to warm the cache, then temp=0.5 must miss
p_zero = {"model": MODEL, "messages": [{"role": "user", "content": "Tell me a joke"}], "temperature": 0, "stream": False}
p_warm = {"model": MODEL, "messages": [{"role": "user", "content": "Tell me a joke"}], "temperature": 0.5, "stream": False}
_, _, _, _ = send_nonstream(p_zero)      # cold
_, s_hot, h_hot, _ = send_nonstream(p_zero)   # should be hit
_, s_warm, h_warm, _ = send_nonstream(p_warm)  # should be miss
check("temp=0 cached", h_hot.get("x-stack-intercept", "N/A") == "hit",
      f"(x-stack-intercept: {h_hot.get('x-stack-intercept', 'N/A')})")
check("temp=0.5 NOT cached", h_warm.get("x-stack-intercept", "N/A") != "hit",
      f"(x-stack-intercept: {h_warm.get('x-stack-intercept', 'N/A')})")
print()


# ============================================================
# TEST 5: response_format=json_schema -> NO cache hit
# ============================================================
print("=" * 60)
print("Test 5: Same prompt, response_format=json_schema -> NO cache hit")

p1 = {"model": MODEL, "messages": [{"role": "user", "content": "List 3 colors as JSON"}], "temperature": 0, "stream": False}
p2 = {
    "model": MODEL,
    "messages": [{"role": "user", "content": "List 3 colors as JSON"}],
    "temperature": 0,
    "response_format": {
        "type": "json_schema",
        "json_schema": {
            "name": "color_list",
            "schema": {
                "type": "object",
                "properties": {
                    "colors": {"type": "array", "items": {"type": "string"}}
                }
            }
        }
    },
    "stream": False,
}
_, s1, h1, _ = send_nonstream(p1)
# response_format requests still go to upstream — check no cache hit
_, s2, h2, b2 = send_nonstream(p2)
check("no cache hit for response_format", h2.get("x-stack-intercept", "N/A") != "hit",
      f"(x-stack-intercept: {h2.get('x-stack-intercept', 'N/A')})")
print()


# ============================================================
# TEST 6: Tools present -> NO cache hit
# ============================================================
print("=" * 60)
print("Test 6: Same prompt, tools present -> NO cache hit")

p_cold = {"model": MODEL, "messages": [{"role": "user", "content": "What's the weather in Paris?"}], "temperature": 0, "stream": False}
p_with_tools = {
    "model": MODEL,
    "messages": [{"role": "user", "content": "What's the weather in Paris?"}],
    "temperature": 0,
    "tools": [{
        "type": "function",
        "function": {
            "name": "get_weather",
            "description": "Get weather for a city",
            "parameters": {
                "type": "object",
                "properties": {"city": {"type": "string"}},
            },
        },
    }],
    "stream": False,
}
_, s1, h1, _ = send_nonstream(p_cold)
_, s2, h2, _ = send_nonstream(p_with_tools)
check("tools request not cached", h2.get("x-stack-intercept", "N/A") != "hit",
      f"(x-stack-intercept: {h2.get('x-stack-intercept', 'N/A')})")
print()


# ============================================================
# TEST 7: Multi-turn with different history -> NO cache hit
# ============================================================
print("=" * 60)
print("Test 7: Multi-turn 'summarize' with different history -> NO cache hit")

p_hist1 = {
    "model": MODEL,
    "messages": [
        {"role": "user", "content": "What is the capital of Japan?"},
        {"role": "assistant", "content": "Tokyo."},
        {"role": "user", "content": "summarize the above"},
    ],
    "temperature": 0,
    "stream": False,
}
p_hist2 = {
    "model": MODEL,
    "messages": [
        {"role": "user", "content": "What is the capital of France?"},
        {"role": "assistant", "content": "Paris."},
        {"role": "user", "content": "summarize the above"},
    ],
    "temperature": 0,
    "stream": False,
}
_, s1, h1, _ = send_nonstream(p_hist1)
_, s2, h2, _ = send_nonstream(p_hist2)
check("different history not cached", h2.get("x-stack-intercept", "N/A") != "hit",
      f"(x-stack-intercept: {h2.get('x-stack-intercept', 'N/A')})")
print()


# ============================================================
# Summary
# ============================================================
print("=" * 60)
total = PASS + FAIL
print(f"Results: {PASS}/{total} passed", "ALL PASSED" if FAIL == 0 else f"{FAIL} FAILURES")

if FAIL > 0:
    exit(1)
