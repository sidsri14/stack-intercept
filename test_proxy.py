"""
StackIntercept semantic cache verification.
Run 1: cache miss (cold). Run 2: cache hit via BGE embedding similarity >0.92.
"""

import openai
import time

client = openai.OpenAI(
    base_url="http://127.0.0.1:8080/v1",
    api_key="your-actual-openai-api-key"
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
