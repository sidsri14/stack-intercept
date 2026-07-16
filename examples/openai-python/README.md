# OpenAI Python SDK Example

This example points the official OpenAI Python SDK at StackIntercept instead of calling the provider directly.

## Setup

```bash
pip install openai
export OPENAI_API_KEY="sk-your-key"
python app.py
```

Run StackIntercept separately:

```bash
docker compose -f docker-compose.trial.yml up --build
```

Expected behavior:
- First identical request: `x-stack-intercept: miss`
- Second identical request: `x-stack-intercept: hit`

