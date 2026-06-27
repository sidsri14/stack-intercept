"""
StackIntercept proxy verification script.
Run this with your proxy running at localhost:8080.
First run = cache miss (routes to OpenAI). Second run = cache hit (sub-ms local).
"""

import openai
import time

client = openai.OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key="your-actual-openai-api-key"
)

prompt = "Analyze the technical trade-offs between eBPF and kernel bypass for high-frequency packet processing."

for run in range(2):
    print(f"\n{'='*60}")
    print(f"RUN {run + 1} — {'Cache Miss (cold)' if run == 0 else 'Cache Hit (warm)'}")
    print('='*60)

    start = time.time()
    response = client.chat.completions.create(
        model="gpt-4o-mini",
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
