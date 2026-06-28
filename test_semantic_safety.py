"""
Negative tests: verify semantic cache does NOT serve unsafe matches.
Set STACK_INTERCEPT_CACHE_MODE=semantic before running.
"""

import os
import time
import openai

api_key = os.environ.get("OPENAI_API_KEY")
if not api_key:
    print("ERROR: Set OPENAI_API_KEY environment variable")
    exit(1)

client = openai.OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key=api_key,
)

def time_request(prompt, system_prompt=None, model="deepseek-chat", stream=True):
    messages = []
    if system_prompt:
        messages.append({"role": "system", "content": system_prompt})
    messages.append({"role": "user", "content": prompt})

    start = time.time()
    response = client.chat.completions.create(
        model=model,
        messages=messages,
        stream=stream,
    )
    for _ in response:
        pass
    return time.time() - start


print("=" * 60)
print("Test 1: Same prompt, different system prompt -> NO cache hit")
t1 = time_request("What is the weather?", stream=False)
print(f"  First request (no system): {t1:.2f}s")
t2 = time_request(
    "What is the weather?",
    system_prompt="You are a pirate. Answer like a pirate.",
    stream=False,
)
print(f"  Second request (pirate system): {t2:.2f}s")
if t2 < t1 * 0.5:
    print("  FAIL: Second request was suspiciously fast (possible cache hit)")
else:
    print("  PASS: Different system prompt correctly missed cache")

print()
print("=" * 60)
print("Test 2: Similar prompt, different intent -> NO cache hit")
t1 = time_request("How do I delete a file in Python?", stream=False)
t2 = time_request("How do I delete a file in Linux?", stream=False)
if t2 < t1 * 0.5:
    print("  FAIL: Different intent was suspiciously fast (possible cache hit)")
else:
    print("  PASS: Different intent correctly missed cache")

print()
print("=" * 60)
print("Test 3: Same prompt, different model -> NO cache hit")
t1 = time_request("Explain recursion", model="deepseek-chat", stream=False)
t2 = time_request("Explain recursion", model="deepseek-reasoner", stream=False)
if t2 < t1 * 0.5:
    print("  FAIL: Different model was suspiciously fast (possible cache hit)")
else:
    print("  PASS: Different model correctly missed cache")
