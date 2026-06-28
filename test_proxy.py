"""
StackIntercept cache verification.
Exact cache matches identical requests (same model, messages, temperature=0, etc.).
Semantic cache (opt-in) matches semantically similar prompts within the same exact context.
Set STACK_INTERCEPT_CACHE_MODE=semantic for semantic cache testing.

Run 1: cache miss (cold). Run 2: cache hit via exact or semantic match.
"""

import os
import sys
import io
import openai
import time

# Handle Windows console encoding for Unicode
sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')

api_key = os.environ.get("OPENAI_API_KEY")
if not api_key:
    print("ERROR: Set OPENAI_API_KEY environment variable")
    exit(1)

client = openai.OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key=api_key
)

prompts = [
    "Explain the latency benefits of kernel bypass architecture.",
    "What are the speed advantages of using kernel bypass architectures?",
]

for i, prompt in enumerate(prompts):
    print(f"\n{'='*60}")
    print(f"RUN {i + 1} — {'Cache Miss (cold)' if i == 0 else 'Cache Hit (semantic match expected)'}")
    print(f"Prompt: {prompt}")
    print('='*60)

    start = time.time()
    response = client.chat.completions.create(
        model="deepseek-chat",
        messages=[{"role": "user", "content": prompt}],
        stream=True
    )

    token_count = 0
    for chunk in response:
        content = chunk.choices[0].delta.content
        if content:
            print(content, end="", flush=True)
            token_count += 1

    elapsed = time.time() - start
    print(f"\n\nDuration: {elapsed:.4f}s  |  Tokens: {token_count}")
